use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::bail;
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Subcommand, ValueHint};
use clap_complete::ArgValueCompleter;
use wineo_core::art::{self, Kind, Listing, Resolved, Source, Warning};
use wineo_core::art_fetch::{self, Fetched, Mode};
use wineo_core::download::Https;
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb::{self, Api, Game, KEY_VAR, Keyring, Saved};

use crate::prompt::{Canceled, Prompt, Reply, Terminal, numbered};
use crate::{prompt, style};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Copy an image into the store as an environment's or a target's art
    Set {
        #[arg(value_name = "ENV[.TARGET]", add = ArgValueCompleter::new(crate::completion::spots))]
        spot: String,
        #[arg(value_parser = kind_parser())]
        kind: Kind,
        #[arg(value_hint = ValueHint::FilePath)]
        file: PathBuf,
    },
    /// Show an environment's art and its targets' overrides
    List {
        #[arg(add = ArgValueCompleter::new(crate::completion::environments))]
        env: String,
    },
    /// Fetch art from SteamGridDB for kinds that have none
    Fetch {
        #[arg(value_name = "ENV[.TARGET]", add = ArgValueCompleter::new(crate::completion::spots))]
        spot: String,
        /// Search for TERM instead of the environment's name
        #[arg(long, value_name = "TERM", conflicts_with = "game")]
        search: Option<String>,
        /// Use this SteamGridDB game ID instead of searching
        #[arg(long, value_name = "ID")]
        game: Option<u64>,
        /// Fetch only this kind
        #[arg(long, value_name = "KIND", value_parser = kind_parser())]
        only: Option<Kind>,
        /// Replace art the kinds already have, manual files included
        #[arg(long, conflicts_with = "asset")]
        replace: bool,
        /// Fetch this SteamGridDB asset ID for the --only kind, replacing its art
        #[arg(long, value_name = "ID", requires = "only")]
        asset: Option<u64>,
    },
    /// Save a SteamGridDB API key, or remove it
    Key {
        /// Delete the saved key from the keyring and the key file
        #[arg(long)]
        remove: bool,
    },
    /// Remove one piece of art
    Rm {
        #[arg(value_name = "ENV[.TARGET]", add = ArgValueCompleter::new(crate::completion::spots))]
        spot: String,
        #[arg(value_parser = kind_parser())]
        kind: Kind,
    },
}

fn kind_parser() -> impl TypedValueParser<Value = Kind> {
    PossibleValuesParser::new(Kind::ALL.map(Kind::as_str))
        .map(|kind| kind.parse().expect("only a possible value gets here"))
}

/// Environment names never contain a dot, so the first one starts the target.
fn split(spot: &str) -> (&str, Option<&str>) {
    match spot.split_once('.') {
        Some((env, target)) => (env, Some(target)),
        None => (spot, None),
    }
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    match args.command {
        Command::Set { spot, kind, file } => {
            let (env, target) = split(&spot);
            art::set_manual(paths, env, target, kind, &file)?;
            println!("Set {kind} for {spot}");
        }
        Command::List { env } => {
            let listing = art::list(paths, &env)?;
            let (lines, warnings) = render(&env, &paths.art_dir(&env), &listing);
            for line in lines {
                println!("{line}");
            }
            for warning in warnings {
                eprintln!("{}", style::warning_stderr(format!("warning: {warning}")));
            }
        }
        Command::Key { remove } => key(paths, &*keyring(), remove)?,
        Command::Fetch {
            spot,
            search,
            game,
            only,
            replace,
            asset,
        } => {
            let mode = match (asset, replace) {
                (Some(asset), _) => Mode::Asset(asset),
                (None, true) => Mode::Replace,
                (None, false) => Mode::Fill,
            };
            let request = FetchRequest {
                spot: &spot,
                search: search.as_deref(),
                game,
                only,
                mode,
            };
            return match fetch(paths, &request) {
                Err(err) if err.is::<Canceled>() => {
                    eprintln!("Canceled.");
                    Ok(ExitCode::from(CANCELED))
                }
                Ok(true) => Ok(ExitCode::SUCCESS),
                Ok(false) => Ok(ExitCode::FAILURE),
                Err(err) => Err(err),
            };
        }
        Command::Rm { spot, kind } => {
            let (env, target) = split(&spot);
            art::remove(paths, env, target, kind)?;
            println!("Removed {kind} from {spot}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn keyring() -> Box<dyn Keyring> {
    Box::new(SecretServiceKeyring::steamgriddb())
}

fn key(paths: &Paths, keyring: &dyn Keyring, remove: bool) -> anyhow::Result<()> {
    if std::env::var_os(KEY_VAR).is_some_and(|value| !value.is_empty()) {
        eprintln!("note: {KEY_VAR} is set and overrides the saved key");
    }
    if remove {
        let removed = steamgriddb::remove_key(paths, keyring)?;
        if removed.keyring || removed.file {
            println!("Removed the SteamGridDB key");
        } else {
            println!("No SteamGridDB key was saved");
        }
        return Ok(());
    }
    let Some(key) = prompt::secret("SteamGridDB API key: ")?.filter(|key| !key.is_empty()) else {
        bail!("no key entered");
    };
    if !steamgriddb::check_key(&Https::new()?, &key)? {
        bail!("SteamGridDB rejected the key");
    }
    match steamgriddb::save_key(paths, keyring, &key)? {
        Saved::Keyring => println!("Saved the SteamGridDB key to the keyring"),
        Saved::File(path) => eprintln!(
            "{}",
            style::warning_stderr(format!(
                "warning: no keyring, so the key is stored in plain text in {}",
                path.display()
            ))
        ),
    }
    Ok(())
}

const CANCELED: u8 = 130;

struct FetchRequest<'a> {
    spot: &'a str,
    search: Option<&'a str>,
    game: Option<u64>,
    only: Option<Kind>,
    mode: Mode,
}

fn fetch(paths: &Paths, request: &FetchRequest) -> anyhow::Result<bool> {
    let (env, target) = split(request.spot);
    art::check_spot(paths, env, target)?;
    let key = steamgriddb::require_key(paths, |var| std::env::var_os(var), &*keyring())?;
    let transport = Https::new()?;
    let api = Api::new(&transport, &key.value);
    let game = match (request.game, request.search, art::saved_game(paths, env)?) {
        (Some(game), ..) => game,
        (None, None, Some(saved)) => saved,
        (None, search, _) => pick_game(&api, search.unwrap_or(env))?,
    };
    if art::saved_game(paths, env)? != Some(game) {
        art::save_game(paths, env, game)?;
    }
    let language = steamgriddb::language(|var| std::env::var_os(var));
    let spot = Spot { paths, env, target };
    fetch_kinds(&spot, request, &api, game, &language, &mut io::stdout())
}

/// Where fetched art goes.
struct Spot<'a> {
    paths: &'a Paths,
    env: &'a str,
    target: Option<&'a str>,
}

/// Fetches each kind the request asks for, printing a line per kind as it
/// goes, so the kinds already fetched show before any error. Returns false
/// when `--only` named a kind that found nothing.
fn fetch_kinds(
    spot: &Spot,
    request: &FetchRequest,
    api: &Api,
    game: u64,
    language: &str,
    out: &mut dyn io::Write,
) -> anyhow::Result<bool> {
    let kinds = match request.only {
        Some(kind) => vec![kind],
        None => Kind::ALL.to_vec(),
    };
    let mut found = true;
    for kind in kinds {
        let fetched = art_fetch::fetch_kind(
            spot.paths,
            spot.env,
            spot.target,
            kind,
            api,
            game,
            language,
            request.mode,
        )?;
        match fetched {
            Fetched::Stored { asset, .. } => writeln!(out, "{kind}: fetched SteamGridDB {asset}")?,
            Fetched::Skipped { .. } => writeln!(out, "{kind}: skipped, it already has art")?,
            Fetched::NoneFound => {
                writeln!(out, "{kind}: nothing found")?;
                found = request.only.is_none();
            }
            Fetched::AssetNotFound => {
                let Mode::Asset(asset) = request.mode else {
                    unreachable!("only an asset fetch looks for one")
                };
                bail!("asset {asset} is not among this game's {kind} art");
            }
        }
    }
    Ok(found)
}

/// A search result as the pick list shows it.
fn game_label(game: &Game) -> String {
    match game.year {
        Some(year) => format!("{} ({year}), {}", game.name, game.id),
        None => format!("{}, {}", game.name, game.id),
    }
}

/// Searches for `term` and asks which match is the game.
fn pick_game(api: &Api, term: &str) -> anyhow::Result<u64> {
    let games = api.search(term)?;
    if games.is_empty() {
        bail!("no SteamGridDB games match `{term}`");
    }
    if !prompt::interactive() {
        bail!("picking a game needs a terminal, pass --game instead");
    }
    let choices = numbered(games.iter().map(|game| (game_label(game), game.id)));
    let mut prompt = Prompt::new(Terminal::open()?, io::stdout());
    match prompt.choose("Which game is it?", "Game", &choices, Some("1"), false)? {
        Reply::Value(game) => Ok(game),
        Reply::Back => unreachable!("the first question has no back"),
    }
}

/// The lines `art list` prints, and its warnings.
fn render(env: &str, art_dir: &Path, listing: &Listing) -> (Vec<String>, Vec<String>) {
    let file = |resolved: &Resolved| {
        resolved
            .found
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let width = listing
        .kinds
        .iter()
        .filter_map(|(_, resolved)| resolved.as_ref())
        .chain(
            listing
                .targets
                .iter()
                .flat_map(|(_, overrides)| overrides.iter().map(|(_, resolved)| resolved)),
        )
        .map(|resolved| file(resolved).len())
        .max()
        .unwrap_or(0);
    let row = |kind: Kind, resolved: &Resolved| {
        format!(
            "  {kind}  {:<width$}  {}",
            file(resolved),
            source(resolved.source.as_ref())
        )
    };

    let mut lines = vec![match listing.game {
        Some(game) => format!("{env} (SteamGridDB game {game})"),
        None => env.to_owned(),
    }];
    for (kind, resolved) in &listing.kinds {
        lines.push(match resolved {
            Some(resolved) => row(*kind, resolved),
            None => format!("  {kind}  none"),
        });
    }
    for (target, overrides) in &listing.targets {
        lines.push(format!("{env}.{target}"));
        lines.extend(
            overrides
                .iter()
                .map(|(kind, resolved)| row(*kind, resolved)),
        );
    }

    let shown = |path: &Path| {
        path.strip_prefix(art_dir)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    let mut warnings = Vec::new();
    for warning in &listing.warnings {
        match warning {
            Warning::Duplicates {
                used,
                ignored,
                recorded,
                ..
            } => {
                let reason = if *recorded {
                    "is recorded"
                } else {
                    "takes precedence"
                };
                for path in ignored {
                    warnings.push(format!("{} ignored, {} {reason}", shown(path), shown(used)));
                }
            }
            Warning::MissingFile { path, .. } => {
                warnings.push(format!("{} is missing", shown(path)));
            }
            Warning::UnknownTarget(target) => {
                warnings.push(format!("art for unknown target `{target}`"));
            }
        }
    }
    (lines, warnings)
}

fn source(source: Option<&Source>) -> String {
    match source {
        Some(Source::SteamGridDb(asset)) => format!("SteamGridDB {asset}"),
        Some(Source::Manual(original)) => format!("manual, from {original}"),
        None => "unrecorded".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::fs;
    use std::io::{Cursor, Read};

    use wineo_core::download::{Response, Transport};
    use wineo_core::steamgriddb::Filters;

    use super::*;

    #[test]
    fn a_spot_splits_at_its_first_dot() {
        assert_eq!(split("hades"), ("hades", None));
        assert_eq!(split("hades.server"), ("hades", Some("server")));
        assert_eq!(split("hades.a.b"), ("hades", Some("a.b")));
        assert_eq!(split("hades."), ("hades", Some("")));
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    /// Canned answers by URL. Anything else is a failed request.
    #[derive(Default)]
    struct Fake(HashMap<String, (u16, Vec<u8>)>);

    impl Fake {
        fn page(&mut self, kind: Kind, ids: &[u64]) {
            let data: Vec<String> = ids
                .iter()
                .map(|id| format!("{{\"id\":{id},\"url\":\"https://cdn.example/{id}\"}}"))
                .collect();
            let body = format!("{{\"success\":true,\"data\":[{}]}}", data.join(","));
            self.0.insert(
                Api::images_url(7, kind, 0, Filters::Ranked),
                (200, body.into_bytes()),
            );
            self.0.insert(
                Api::images_url(7, kind, 1, Filters::Ranked),
                (200, b"{\"success\":true,\"data\":[]}".to_vec()),
            );
        }
    }

    impl Transport for Fake {
        fn get(&self, url: &str) -> anyhow::Result<Box<dyn Read>> {
            bail!("unexpected request to {url}")
        }

        fn request(&self, url: &str, _: Option<&str>) -> anyhow::Result<Response> {
            let Some((status, body)) = self.0.get(url) else {
                bail!("connection to {url} failed");
            };
            Ok(Response {
                status: *status,
                body: Box::new(Cursor::new(body.clone())),
            })
        }
    }

    fn paths() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        let config = paths.environment_config("hades");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            config,
            "prefix = \"~/p\"\nrunner = \"wine\"\narch = \"win64\"\n",
        )
        .unwrap();
        (root, paths)
    }

    fn run(paths: &Paths, fake: &Fake, only: Option<Kind>) -> (anyhow::Result<bool>, String) {
        let request = FetchRequest {
            spot: "hades",
            search: None,
            game: None,
            only,
            mode: Mode::Fill,
        };
        let spot = Spot {
            paths,
            env: "hades",
            target: None,
        };
        let mut out = Vec::new();
        let result = fetch_kinds(&spot, &request, &Api::new(fake, "k"), 7, "en", &mut out);
        (result, String::from_utf8(out).unwrap())
    }

    #[test]
    fn games_are_listed_with_their_year_and_id() {
        let game = |year| Game {
            id: 1234,
            name: "Hades".into(),
            year,
        };
        assert_eq!(game_label(&game(Some(2020))), "Hades (2020), 1234");
        assert_eq!(game_label(&game(None)), "Hades, 1234");
    }

    #[test]
    fn each_kind_gets_a_line_and_only_an_empty_only_fails() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.page(Kind::Icon, &[1]);
        fake.0
            .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
        fake.page(Kind::Grid, &[]);
        fake.page(Kind::Hero, &[]);
        fake.page(Kind::Logo, &[]);
        let (result, out) = run(&paths, &fake, None);
        assert!(result.unwrap());
        assert_eq!(
            out,
            "icon: fetched SteamGridDB 1\ngrid: nothing found\nhero: nothing found\nlogo: nothing found\n"
        );

        let (result, out) = run(&paths, &fake, Some(Kind::Icon));
        assert!(result.unwrap());
        assert_eq!(out, "icon: skipped, it already has art\n");
        let (result, out) = run(&paths, &fake, Some(Kind::Hero));
        assert!(!result.unwrap());
        assert_eq!(out, "hero: nothing found\n");
    }

    #[test]
    fn kinds_fetched_before_an_error_are_shown_and_kept() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.page(Kind::Icon, &[1]);
        fake.0
            .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
        fake.page(Kind::Grid, &[2]);
        let (result, out) = run(&paths, &fake, None);
        let err = result.unwrap_err();
        assert!(err.to_string().contains("cdn.example/2"), "{err}");
        assert_eq!(out, "icon: fetched SteamGridDB 1\n");
        assert!(paths.art_dir("hades").join("icon.png").exists());
    }
}
