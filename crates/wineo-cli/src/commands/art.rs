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
use wineo_core::steamgriddb::{self, Api, KEY_VAR, Keyring, Saved};

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
                other => other.map(|()| ExitCode::SUCCESS),
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

fn fetch(paths: &Paths, request: &FetchRequest) -> anyhow::Result<()> {
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
    let kinds = match request.only {
        Some(kind) => vec![kind],
        None => Kind::ALL.to_vec(),
    };
    for kind in kinds {
        match art_fetch::fetch_kind(
            paths,
            env,
            target,
            kind,
            &api,
            game,
            &language,
            request.mode,
        )? {
            Fetched::Stored { asset, .. } => println!("{kind}: fetched SteamGridDB {asset}"),
            Fetched::Skipped { .. } => println!("{kind}: skipped, it already has art"),
            Fetched::NoneFound => println!("{kind}: nothing found"),
            Fetched::AssetNotFound => {
                let Mode::Asset(asset) = request.mode else {
                    unreachable!("only an asset fetch looks for one")
                };
                bail!("asset {asset} is not among this game's {kind} art");
            }
        }
    }
    Ok(())
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
    let choices = numbered(games.into_iter().map(|game| {
        let label = match game.year {
            Some(year) => format!("{} ({year})", game.name),
            None => game.name,
        };
        (label, game.id)
    }));
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
    use super::split;

    #[test]
    fn a_spot_splits_at_its_first_dot() {
        assert_eq!(split("hades"), ("hades", None));
        assert_eq!(split("hades.server"), ("hades", Some("server")));
        assert_eq!(split("hades.a.b"), ("hades", Some("a.b")));
        assert_eq!(split("hades."), ("hades", Some("")));
    }
}
