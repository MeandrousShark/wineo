use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::bail;
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Subcommand, ValueHint};
use clap_complete::ArgValueCompleter;
use wineo_core::art::{self, Kind, Listing, Resolved, Source, Warning};
use wineo_core::art_fetch::{self, Fetched, Mode};
use wineo_core::config::ArtFetch;
use wineo_core::download::{Https, Transport};
use wineo_core::environment::Environment;
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb::{self, Api, Game, KEY_VAR, Key, Keyring, Saved};

use crate::commands::desktop;
use crate::prompt::{Canceled, Input, Prompt, Reply, Terminal, numbered};
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
            art::set_manual(paths, env, target, kind, &file, None)?;
            println!("Set {kind} for {spot}");
            if kind == Kind::Icon {
                apply_desktop(paths, env)?;
            }
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
                kinds: ArtFetch::All,
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
            if kind == Kind::Icon {
                apply_desktop(paths, env)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Brings the entries in line with a changed icon. Portable environments have
/// none, so they get nothing said about them either.
fn apply_desktop(paths: &Paths, env: &str) -> anyhow::Result<()> {
    if wineo_core::desktop::supported(paths).is_err() {
        return Ok(());
    }
    desktop::apply(paths, env, false)
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
    kinds: ArtFetch,
    mode: Mode,
}

fn fetch(paths: &Paths, request: &FetchRequest) -> anyhow::Result<bool> {
    let (env, target) = split(request.spot);
    art::check_spot(paths, env, target)?;
    let key = steamgriddb::require_key(paths, |var| std::env::var_os(var), &*keyring())?;
    let transport = Https::new()?;
    let api = Api::new(&transport, &key.value);
    let Some(game) = fetch_game(paths, env, request, &api)? else {
        return Ok(true);
    };
    if art::saved_game(paths, env)? != Some(game) {
        art::save_game(paths, env, game)?;
    }
    let language = steamgriddb::language(|var| std::env::var_os(var));
    let spot = Spot { paths, env, target };
    let mut icon = false;
    let fetched = fetch_kinds(
        &spot,
        request,
        &api,
        game,
        &language,
        &mut icon,
        &mut crate::output::stdout(),
    );
    // An icon fetched before a later kind failed is still in the store.
    if icon {
        apply_desktop(paths, env)?;
    }
    fetched
}

fn fetch_game(
    paths: &Paths,
    env: &str,
    request: &FetchRequest,
    api: &Api,
) -> anyhow::Result<Option<u64>> {
    if let Some(game) = request.game {
        return Ok(Some(game));
    }
    // Art commands can still work when the environment config can't be read.
    if request.search.is_none()
        && let Ok(environment) = Environment::load(paths, env)
        && let Some(proxied) = environment.config.proxied()
    {
        let app_id = proxied.app_id.parse()?;
        let game = api
            .steam_game(app_id)?
            .ok_or_else(|| anyhow::anyhow!("SteamGridDB has no game for Steam app {app_id}"))?;
        return Ok(Some(game.id));
    }
    match (request.search, art::saved_game(paths, env)?) {
        (None, Some(saved)) => Ok(Some(saved)),
        (search, _) => pick_game(api, search.unwrap_or(env)),
    }
}

/// Where fetched art goes.
struct Spot<'a> {
    paths: &'a Paths,
    env: &'a str,
    target: Option<&'a str>,
}

/// Fetches each kind the request asks for, printing a line per kind as it
/// goes, so the kinds already fetched show before any error. Returns false
/// when `--only` named a kind that found nothing. Sets `icon` when the icon
/// was stored.
fn fetch_kinds(
    spot: &Spot,
    request: &FetchRequest,
    api: &Api,
    game: u64,
    language: &str,
    icon: &mut bool,
    out: &mut dyn io::Write,
) -> anyhow::Result<bool> {
    let kinds = match request.only {
        Some(kind) => vec![kind],
        None => request.kinds.kinds(),
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
            Fetched::Stored { asset, .. } => {
                *icon |= kind == Kind::Icon;
                writeln!(out, "{kind}: fetched SteamGridDB {asset}")?;
            }
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
    format!("{}, {}", game_name(game), game.id)
}

/// A game's name, with its year when known.
fn game_name(game: &Game) -> String {
    match game.year {
        Some(year) => format!("{} ({year})", game.name),
        None => game.name.clone(),
    }
}

/// Printed by `new` and `import` in place of offering art when no key is saved.
pub const NO_KEY_NOTE: &str = "note: run `wineo art key` to fetch art from SteamGridDB";

/// Whether `new` and `import` can offer art.
pub enum KeyCheck {
    Found(Key),
    Missing,
    /// The lookup failed, like a dismissed keyring prompt.
    Failed(anyhow::Error),
}

impl KeyCheck {
    /// The unstyled line to print instead of offering art, or None with a key.
    pub fn skip_line(&self) -> Option<String> {
        match self {
            KeyCheck::Found(_) => None,
            KeyCheck::Missing => Some(NO_KEY_NOTE.to_owned()),
            KeyCheck::Failed(err) => Some(format!("warning: {err:#}, skipping art")),
        }
    }

    /// Prints `skip_line` to stderr, styled as a warning when the lookup failed.
    pub fn print_skip(&self) {
        match (self, self.skip_line()) {
            (KeyCheck::Failed(_), Some(line)) => eprintln!("{}", style::warning_stderr(line)),
            (_, Some(line)) => eprintln!("{line}"),
            (_, None) => {}
        }
    }
}

/// Looks up the SteamGridDB key without checking it against the API.
pub fn check_key(paths: &Paths) -> KeyCheck {
    check_key_with(paths, |var| std::env::var_os(var), &*keyring())
}

fn check_key_with(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<std::ffi::OsString>,
    keyring: &dyn Keyring,
) -> KeyCheck {
    match steamgriddb::find_key(paths, lookup, keyring) {
        Ok(Some(key)) => KeyCheck::Found(key),
        Ok(None) => KeyCheck::Missing,
        Err(err) => KeyCheck::Failed(err.into()),
    }
}

/// Art `new` and `import` fetch for the environment they made.
pub struct AutoFetch<'a> {
    pub env: &'a str,
    pub term: &'a str,
    pub kinds: ArtFetch,
}

/// How the game is picked from the search results.
pub enum Pick<'a, I, W> {
    /// The numbered list, with a last entry that skips art.
    Ask(&'a mut Prompt<I, W>),
    /// The first result, for when nobody is there to pick, named on the writer.
    Top(&'a mut dyn Write),
}

/// The key lookup and connection guided flows fetch art with, which tests
/// stand in for.
pub struct Access<'a> {
    pub check_key: Box<dyn Fn() -> KeyCheck + 'a>,
    pub transport: Box<dyn Fn() -> anyhow::Result<Box<dyn Transport>> + 'a>,
}

impl<'a> Access<'a> {
    pub fn steamgriddb(paths: &'a Paths) -> Self {
        Self {
            check_key: Box::new(|| check_key(paths)),
            transport: Box::new(|| Ok(Box::new(Https::new()?))),
        }
    }

    /// Reviews the automatic pick without saving it or fetching images.
    pub fn review(&self, term: &str, kinds: ArtFetch, out: &mut dyn Write, err: &mut dyn Write) {
        if kinds.kinds().is_empty() {
            return;
        }
        let key = match (self.check_key)() {
            KeyCheck::Found(key) => key,
            check => {
                match (&check, check.skip_line()) {
                    (KeyCheck::Failed(_), Some(line)) => {
                        let _ = writeln!(err, "{}", style::warning_stderr(line));
                    }
                    (_, Some(line)) => {
                        let _ = writeln!(err, "{line}");
                    }
                    (_, None) => {}
                }
                return;
            }
        };
        let result = (|| -> anyhow::Result<()> {
            let transport = (self.transport)()?;
            let games = Api::new(&*transport, &key.value).search(term)?;
            if games.is_empty() {
                bail!("no SteamGridDB games match `{term}`");
            }
            writeln!(out, "Art from SteamGridDB, searching \"{term}\":")?;
            for (index, game) in games.iter().take(3).enumerate() {
                writeln!(
                    out,
                    "  {}. {}, game {}{}",
                    index + 1,
                    game_name(game),
                    game.id,
                    if index == 0 {
                        " <- would be picked automatically"
                    } else {
                        ""
                    }
                )?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            warn_failed(&error, err);
        }
    }

    /// `fetch_new` over this access's transport, which failing to set up is
    /// one more warning.
    pub fn fetch<I: Input, W: Write>(
        &self,
        paths: &Paths,
        key: &Key,
        request: &AutoFetch,
        pick: Pick<I, W>,
        err: &mut dyn Write,
    ) -> Result<(), Canceled> {
        if request.kinds.kinds().is_empty() {
            return Ok(());
        }
        match (self.transport)() {
            Ok(transport) => fetch_new(paths, &*transport, key, request, pick, err),
            Err(error) => {
                warn_failed(&error, err);
                Ok(())
            }
        }
    }
}

fn warn_failed(error: &anyhow::Error, err: &mut dyn Write) {
    let line = format!("warning: art fetch failed: {error:#}");
    let _ = writeln!(err, "{}", style::warning_stderr(line));
}

/// Fetches art into the environment's store with `Mode::Fill`, printing its
/// lines through the prompt or the `Top` writer. Any failure is a warning on
/// `err` and kinds already stored stay, so art never changes how
/// `new` or `import` ends. Only canceling the pick list is an error.
pub fn fetch_new<I: Input, W: Write>(
    paths: &Paths,
    transport: &dyn Transport,
    key: &Key,
    request: &AutoFetch,
    pick: Pick<I, W>,
    err: &mut dyn Write,
) -> Result<(), Canceled> {
    if request.kinds.kinds().is_empty() {
        return Ok(());
    }
    match try_fetch_new(paths, transport, key, request, pick) {
        Ok(()) => Ok(()),
        Err(error) if error.is::<Canceled>() => Err(Canceled),
        Err(error) => {
            warn_failed(&error, err);
            Ok(())
        }
    }
}

fn try_fetch_new<I: Input, W: Write>(
    paths: &Paths,
    transport: &dyn Transport,
    key: &Key,
    request: &AutoFetch,
    pick: Pick<I, W>,
) -> anyhow::Result<()> {
    let env = request.env;
    let api = Api::new(transport, &key.value);
    let (game, term, out): (u64, String, &mut dyn Write) = match pick {
        Pick::Top(out) => {
            let games = api.search(request.term)?;
            let Some(top) = games.first() else {
                bail!("no SteamGridDB games match `{}`", request.term);
            };
            writeln!(
                out,
                "{env}: art from SteamGridDB game {}, {}",
                top.id,
                game_name(top)
            )?;
            (top.id, request.term.to_owned(), out)
        }
        Pick::Ask(prompt) => match ask_game(&api, request.term, prompt, true)? {
            Some((game, term)) => (game, term, prompt.writer()),
            None => {
                writeln!(
                    prompt.writer(),
                    "Skipped art. Fetch later with: wineo art fetch {env} --search <term>"
                )?;
                return Ok(());
            }
        },
    };
    if art::saved_game(paths, env)? != Some(game) {
        art::save_game(paths, env, game)?;
    }
    let fetch = FetchRequest {
        spot: env,
        search: Some(&term),
        game: Some(game),
        only: (request.kinds == ArtFetch::Icon).then_some(Kind::Icon),
        kinds: request.kinds,
        mode: Mode::Fill,
    };
    let spot = Spot {
        paths,
        env,
        target: None,
    };
    let language = steamgriddb::language(|var| std::env::var_os(var));
    let mut icon = false;
    let fetched = fetch_kinds(&spot, &fetch, &api, game, &language, &mut icon, out);
    // An icon fetched before a later kind failed is still in the store.
    if icon {
        apply_desktop(paths, env)?;
    }
    fetched.map(drop)
}

/// An entry in the pick list.
#[derive(Clone, Copy)]
enum GamePick {
    Game(u64),
    SearchAgain,
    Skip,
}

/// Searches for `term` and asks which match is the game. A terminal is
/// needed for more than one match, and to search again after an empty one.
fn pick_game(api: &Api, term: &str) -> anyhow::Result<Option<u64>> {
    if !prompt::interactive() {
        let games = api.search(term)?;
        if games.is_empty() {
            bail!("no SteamGridDB games match `{term}`");
        }
        bail!("picking a game needs a terminal, pass --game instead");
    }
    let mut prompt = Prompt::new(Terminal::open()?, crate::output::stdout());
    Ok(ask_game(api, term, &mut prompt, false)?.map(|(game, _)| game))
}

/// Shows the pick list, searching again on request or after an empty result,
/// until a game is picked. `skip` adds an entry that gives up instead, as Esc
/// after an empty result does. Returns the term that found the picked game.
fn ask_game<I: Input, W: Write>(
    api: &Api,
    term: &str,
    prompt: &mut Prompt<I, W>,
    skip: bool,
) -> anyhow::Result<Option<(u64, String)>> {
    let mut term = term.to_owned();
    let mut games = api.search(&term)?;
    loop {
        if games.is_empty() {
            match ask_after_no_matches(prompt, &term)? {
                Reply::Value(next) => {
                    term = next;
                    games = api.search(&term)?;
                    continue;
                }
                Reply::Back => return Ok(None),
            }
        }
        let skip_entry = skip.then(|| ("None of these, skip art".to_owned(), GamePick::Skip));
        let entries = games
            .iter()
            .map(|game| (game_label(game), GamePick::Game(game.id)))
            .chain([(
                "Search for something else".to_owned(),
                GamePick::SearchAgain,
            )])
            .chain(skip_entry);
        let choices = numbered(entries);
        match prompt.choose("Which game is it?", "Game", &choices, Some("1"), false)? {
            Reply::Value(GamePick::Game(game)) => return Ok(Some((game, term))),
            Reply::Value(GamePick::Skip) => return Ok(None),
            Reply::Value(GamePick::SearchAgain) => match ask_search_term(prompt)? {
                Reply::Value(next) => {
                    term = next;
                    games = api.search(&term)?;
                }
                // Esc here goes back to the same list.
                Reply::Back => {}
            },
            Reply::Back => unreachable!("the pick list has no back"),
        }
    }
}

/// Prints the no-matches line and asks for a new term.
fn ask_after_no_matches<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    term: &str,
) -> anyhow::Result<Reply<String>> {
    prompt.say(format!("No SteamGridDB games match `{term}`."))?;
    ask_search_term(prompt)
}

/// An empty line editor, re-asked on an empty answer.
fn ask_search_term<I: Input, W: Write>(prompt: &mut Prompt<I, W>) -> anyhow::Result<Reply<String>> {
    prompt.text("Search for", None, |answer| {
        if answer.is_empty() {
            Err("Enter a search term.".to_owned())
        } else {
            Ok(answer.to_owned())
        }
    })
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

/// A SteamGridDB stand-in for tests here and in the flows that fetch art.
#[cfg(test)]
pub(crate) mod fake {
    use std::collections::HashMap;
    use std::io::{Cursor, Read};

    use anyhow::bail;
    use wineo_core::art::Kind;
    use wineo_core::download::{Response, Transport};
    use wineo_core::steamgriddb::{self, Api, Filters};

    pub(crate) const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    /// Canned answers by URL. Anything else is a failed request.
    #[derive(Default, Clone)]
    pub(crate) struct Fake(pub(crate) HashMap<String, (u16, Vec<u8>)>);

    impl Fake {
        pub(crate) fn page(&mut self, kind: Kind, ids: &[u64]) {
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

        /// Hades, 2020, as game 7, and Hades II without a year as game 8.
        pub(crate) fn search(&mut self, term: &str, found: bool) {
            let data = if found {
                r#"[{"id":7,"name":"Hades","release_date":1600000000},{"id":8,"name":"Hades II"}]"#
            } else {
                "[]"
            };
            self.0.insert(
                format!("{}/search/autocomplete/{term}", steamgriddb::API),
                (
                    200,
                    format!("{{\"success\":true,\"data\":{data}}}").into_bytes(),
                ),
            );
        }

        /// A single named game, for a search under a different term that
        /// should turn up something other than Hades.
        pub(crate) fn search_one(&mut self, term: &str, id: u64, name: &str) {
            self.0.insert(
                format!("{}/search/autocomplete/{term}", steamgriddb::API),
                (
                    200,
                    format!("{{\"success\":true,\"data\":[{{\"id\":{id},\"name\":\"{name}\"}}]}}")
                        .into_bytes(),
                ),
            );
        }

        /// An icon for game 7 and nothing for the other kinds.
        pub(crate) fn icon_only(&mut self) {
            self.page(Kind::Icon, &[1]);
            self.0
                .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
            for kind in [Kind::Grid, Kind::Hero, Kind::Logo] {
                self.page(kind, &[]);
            }
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
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;

    use wineo_core::steamgriddb::{KeySource, KeyringError, NoKeyring};

    use super::fake::{Fake, PNG};
    use super::*;
    use crate::prompt::Script;

    #[test]
    fn a_spot_splits_at_its_first_dot() {
        assert_eq!(split("hades"), ("hades", None));
        assert_eq!(split("hades.server"), ("hades", Some("server")));
        assert_eq!(split("hades.a.b"), ("hades", Some("a.b")));
        assert_eq!(split("hades."), ("hades", Some("")));
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

    fn fetch_request() -> FetchRequest<'static> {
        FetchRequest {
            spot: "hades",
            search: None,
            game: None,
            only: Some(Kind::Icon),
            kinds: ArtFetch::All,
            mode: Mode::Fill,
        }
    }

    #[test]
    fn steam_fetch_uses_the_app_id_even_with_a_saved_game() {
        let (_root, paths) = paths();
        fs::write(
            paths.environment_config("hades"),
            "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1145360\"\n",
        )
        .unwrap();
        let mut fake = Fake::default();
        fake.0.insert(
            format!("{}/games/steam/1145360", steamgriddb::API),
            (
                200,
                br#"{"success":true,"data":{"id":7,"name":"Hades"}}"#.to_vec(),
            ),
        );
        fake.icon_only();
        let request = fetch_request();
        let api = Api::new(&fake, "k");
        for saved in [None, Some(99)] {
            if let Some(saved) = saved {
                art::save_game(&paths, "hades", saved).unwrap();
            }
            assert_eq!(
                fetch_game(&paths, "hades", &request, &api).unwrap(),
                Some(7)
            );
        }
        let game = fetch_game(&paths, "hades", &request, &api)
            .unwrap()
            .unwrap();
        let spot = Spot {
            paths: &paths,
            env: "hades",
            target: None,
        };
        let mut icon = false;
        let mut out = Vec::new();
        assert!(fetch_kinds(&spot, &request, &api, game, "en", &mut icon, &mut out).unwrap());
        assert!(icon);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "icon: fetched SteamGridDB 1\n"
        );
        assert!(paths.art_dir("hades").join("icon.png").exists());
    }

    #[test]
    fn steam_fetch_honors_explicit_game_and_search_overrides() {
        let (_root, paths) = paths();
        fs::write(
            paths.environment_config("hades"),
            "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1145360\"\n",
        )
        .unwrap();
        art::save_game(&paths, "hades", 99).unwrap();
        let mut request = fetch_request();
        request.game = Some(42);
        let fake = Fake::default();
        assert_eq!(
            fetch_game(&paths, "hades", &request, &Api::new(&fake, "k")).unwrap(),
            Some(42)
        );

        let mut fake = Fake::default();
        fake.search("other", false);
        request.game = None;
        request.search = Some("other");
        let err = fetch_game(&paths, "hades", &request, &Api::new(&fake, "k")).unwrap_err();
        assert_eq!(err.to_string(), "no SteamGridDB games match `other`");
    }

    #[test]
    fn steam_fetch_reports_a_missing_app_id_match_without_searching() {
        let (_root, paths) = paths();
        fs::write(
            paths.environment_config("hades"),
            "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1145360\"\n",
        )
        .unwrap();
        let mut fake = Fake::default();
        fake.0.insert(
            format!("{}/games/steam/1145360", steamgriddb::API),
            (404, Vec::new()),
        );
        let err = fetch_game(&paths, "hades", &fetch_request(), &Api::new(&fake, "k")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "SteamGridDB has no game for Steam app 1145360"
        );
    }

    #[test]
    fn non_steam_fetches_keep_saved_games_and_title_searches() {
        let (_root, paths) = paths();
        for config in [
            "prefix = \"~/p\"\nrunner = \"wine\"\narch = \"win64\"\n",
            "kind = \"native\"\n",
            "invalid config",
        ] {
            fs::write(paths.environment_config("hades"), config).unwrap();
            let request = fetch_request();
            let mut fake = Fake::default();
            fake.search("hades", false);
            let api = Api::new(&fake, "k");
            let err = fetch_game(&paths, "hades", &request, &api).unwrap_err();
            assert_eq!(err.to_string(), "no SteamGridDB games match `hades`");
            art::save_game(&paths, "hades", 99).unwrap();
            assert_eq!(
                fetch_game(&paths, "hades", &request, &api).unwrap(),
                Some(99)
            );
            fs::remove_dir_all(paths.art_dir("hades")).unwrap();
        }
    }

    fn run(paths: &Paths, fake: &Fake, only: Option<Kind>) -> (anyhow::Result<bool>, String) {
        let request = FetchRequest {
            spot: "hades",
            search: None,
            game: None,
            only,
            kinds: ArtFetch::All,
            mode: Mode::Fill,
        };
        let spot = Spot {
            paths,
            env: "hades",
            target: None,
        };
        let mut out = Vec::new();
        let mut icon = false;
        let result = fetch_kinds(
            &spot,
            &request,
            &Api::new(fake, "k"),
            7,
            "en",
            &mut icon,
            &mut out,
        );
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

    fn key() -> Key {
        Key {
            value: "k".into(),
            source: KeySource::Variable,
        }
    }

    struct Outcome {
        result: Result<(), Canceled>,
        out: String,
        err: String,
    }

    fn fetch_new_with(
        paths: &Paths,
        fake: &Fake,
        kinds: ArtFetch,
        answers: Option<&[&str]>,
    ) -> Outcome {
        let request = AutoFetch {
            env: "hades",
            term: "hades",
            kinds,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut prompt = answers.map(|answers| Prompt::new(Script::new(answers), Vec::new()));
        let pick = match &mut prompt {
            Some(prompt) => Pick::Ask(prompt),
            None => Pick::Top(&mut out),
        };
        let result = fetch_new(paths, fake, &key(), &request, pick, &mut err);
        if let Some(prompt) = prompt {
            out = prompt.output().clone();
        }
        Outcome {
            result,
            out: String::from_utf8(out).unwrap(),
            err: String::from_utf8(err).unwrap(),
        }
    }

    struct FailingKeyring;

    impl Keyring for FailingKeyring {
        fn get(&self) -> Result<Option<String>, KeyringError> {
            Err(KeyringError::Failed("dismissed".into()))
        }

        fn set(&self, _key: &str) -> Result<(), KeyringError> {
            unreachable!()
        }

        fn delete(&self) -> Result<bool, KeyringError> {
            unreachable!()
        }
    }

    #[test]
    fn a_missing_key_notes_how_to_add_one() {
        let (_root, paths) = paths();
        let check = check_key_with(&paths, |_| None, &NoKeyring);
        assert!(matches!(check, KeyCheck::Missing));
        assert_eq!(
            check.skip_line().unwrap(),
            "note: run `wineo art key` to fetch art from SteamGridDB"
        );

        let check = check_key_with(&paths, |_| None, &FailingKeyring);
        assert!(matches!(check, KeyCheck::Failed(_)));
        assert_eq!(
            check.skip_line().unwrap(),
            "warning: the keyring failed: dismissed, skipping art"
        );

        let check = check_key_with(&paths, |_| Some("k".into()), &FailingKeyring);
        assert!(matches!(check, KeyCheck::Found(_)));
        assert_eq!(check.skip_line(), None);
    }

    #[test]
    fn top_takes_the_first_match_and_names_it() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.icon_only();
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, None);
        assert!(outcome.result.is_ok());
        assert_eq!(
            outcome.out,
            "hades: art from SteamGridDB game 7, Hades (2020)\n\
             icon: fetched SteamGridDB 1\ngrid: nothing found\nhero: nothing found\nlogo: nothing found\n"
        );
        assert_eq!(outcome.err, "");
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(7));
        assert!(paths.art_dir("hades").join("icon.png").exists());
    }

    #[test]
    fn asking_offers_a_skip_entry_that_fetches_nothing() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.icon_only();
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["4"]));
        assert!(outcome.result.is_ok());
        let out = &outcome.out;
        assert!(out.contains("1) Hades (2020), 7"), "{out}");
        assert!(out.contains("2) Hades II, 8"), "{out}");
        assert!(out.contains("3) Search for something else"), "{out}");
        assert!(out.contains("4) None of these, skip art"), "{out}");
        assert!(
            out.ends_with(
                "Game [1]: Skipped art. Fetch later with: wineo art fetch hades --search <term>\n"
            ),
            "{out}"
        );
        assert!(!out.contains("icon:"), "{out}");
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), None);
        assert!(!paths.art_dir("hades").join("icon.png").exists());
    }

    #[test]
    fn asking_again_searches_a_new_term_and_saves_the_pick() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.search_one("arena", 42, "Hades Arena");
        fake.page(Kind::Icon, &[]);
        fake.page(Kind::Grid, &[]);
        fake.page(Kind::Hero, &[]);
        fake.page(Kind::Logo, &[]);
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["3", "arena", "1"]));
        assert!(outcome.result.is_ok(), "{}", outcome.err);
        let out = &outcome.out;
        assert!(out.contains("3) Search for something else"), "{out}");
        assert!(out.contains("Search for: "), "{out}");
        assert!(out.contains("1) Hades Arena, 42"), "{out}");
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(42));
    }

    #[test]
    fn backing_out_of_a_new_search_returns_to_the_same_list() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.icon_only();
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["3", "<", "1"]));
        assert!(outcome.result.is_ok(), "{}", outcome.err);
        assert_eq!(
            outcome.out.matches("Which game is it?").count(),
            2,
            "{}",
            outcome.out
        );
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(7));
    }

    #[test]
    fn asking_with_no_matches_asks_for_a_term_and_succeeds() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", false);
        fake.search("hadesii", true);
        fake.icon_only();
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["hadesii", "1"]));
        assert!(outcome.result.is_ok(), "{}", outcome.err);
        let out = &outcome.out;
        assert!(
            out.contains("No SteamGridDB games match `hades`.\n"),
            "{out}"
        );
        assert!(out.contains("Search for: "), "{out}");
        assert!(out.contains("icon: fetched SteamGridDB 1\n"), "{out}");
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(7));
    }

    #[test]
    fn backing_out_of_an_empty_search_skips_art() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", false);
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["<"]));
        assert!(outcome.result.is_ok(), "{}", outcome.err);
        assert!(
            outcome
                .out
                .contains("Skipped art. Fetch later with: wineo art fetch hades --search <term>\n"),
            "{}",
            outcome.out
        );
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), None);
    }

    #[test]
    fn asking_fetches_the_picked_game() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.icon_only();
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["1"]));
        assert!(outcome.result.is_ok());
        assert!(
            outcome
                .out
                .contains("Game [1]: icon: fetched SteamGridDB 1\n"),
            "{}",
            outcome.out
        );
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(7));
    }

    #[test]
    fn canceling_the_pick_list_is_an_error() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", true);
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["q"]));
        assert!(outcome.result.is_err());
        assert_eq!(outcome.err, "");
    }

    #[test]
    fn automatic_fetch_respects_custom_kinds_and_all_off() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.page(Kind::Grid, &[1]);
        fake.0
            .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
        let outcome = fetch_new_with(&paths, &fake, "grid".parse().unwrap(), None);
        assert!(outcome.result.is_ok());
        assert!(outcome.out.contains("grid: fetched"));
        assert!(!outcome.out.contains("icon:"));
        assert_eq!(outcome.err, "");
        assert!(paths.art_dir("hades").join("grid.png").exists());
        assert!(!paths.art_dir("hades").join("icon.png").exists());

        let empty = fetch_new_with(&paths, &Fake::default(), "none".parse().unwrap(), None);
        assert!(empty.result.is_ok());
        assert_eq!(empty.out, "");
        assert_eq!(empty.err, "");
    }

    #[test]
    fn the_icon_setting_fetches_only_the_icon() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.page(Kind::Icon, &[1]);
        fake.0
            .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::Icon, None);
        assert!(outcome.result.is_ok());
        assert_eq!(
            outcome.out,
            "hades: art from SteamGridDB game 7, Hades (2020)\nicon: fetched SteamGridDB 1\n"
        );
        assert_eq!(outcome.err, "");
    }

    #[test]
    fn a_failed_download_warns_and_keeps_earlier_kinds() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.page(Kind::Icon, &[1]);
        fake.0
            .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
        fake.page(Kind::Grid, &[2]);
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, None);
        assert!(outcome.result.is_ok());
        assert!(
            outcome.out.ends_with("icon: fetched SteamGridDB 1\n"),
            "{}",
            outcome.out
        );
        assert!(
            outcome.err.contains("warning: art fetch failed:"),
            "{}",
            outcome.err
        );
        assert!(outcome.err.contains("cdn.example/2"), "{}", outcome.err);
        assert!(paths.art_dir("hades").join("icon.png").exists());
    }

    #[test]
    fn top_with_no_matches_warns_and_fetches_nothing() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", false);
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, None);
        assert!(outcome.result.is_ok());
        assert_eq!(outcome.out, "");
        assert!(
            outcome
                .err
                .contains("warning: art fetch failed: no SteamGridDB games match `hades`"),
            "{}",
            outcome.err
        );
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), None);
    }

    #[test]
    fn a_rejected_key_warns() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.0.insert(
            format!("{}/search/autocomplete/hades", steamgriddb::API),
            (401, Vec::new()),
        );
        let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, None);
        assert!(outcome.result.is_ok());
        assert!(
            outcome.err.contains("SteamGridDB rejected the key"),
            "{}",
            outcome.err
        );
    }

    /// Drives `ask_game`, the interactive core `pick_game` uses once it knows
    /// it has a terminal, with a scripted prompt and the fake transport.
    fn ask_game_with(
        fake: &Fake,
        term: &str,
        answers: &[&str],
    ) -> (anyhow::Result<Option<u64>>, String) {
        let mut prompt = Prompt::new(Script::new(answers), Vec::new());
        let result = ask_game(&Api::new(fake, "k"), term, &mut prompt, false)
            .map(|pick| pick.map(|(game, _)| game));
        (result, String::from_utf8(prompt.output().clone()).unwrap())
    }

    #[test]
    fn ask_game_offers_a_search_again_entry_last() {
        let mut fake = Fake::default();
        fake.search("hades", true);
        let (result, out) = ask_game_with(&fake, "hades", &["1"]);
        assert_eq!(result.unwrap(), Some(7));
        assert!(out.contains("1) Hades (2020), 7"), "{out}");
        assert!(out.contains("2) Hades II, 8"), "{out}");
        assert!(out.contains("3) Search for something else"), "{out}");
    }

    #[test]
    fn ask_game_again_searches_a_new_term_and_picks_from_it() {
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.search_one("arena", 42, "Hades Arena");
        let (result, out) = ask_game_with(&fake, "hades", &["3", "arena", "1"]);
        assert_eq!(result.unwrap(), Some(42));
        assert!(out.contains("3) Search for something else"), "{out}");
        assert!(out.contains("Search for: "), "{out}");
        assert!(out.contains("1) Hades Arena, 42"), "{out}");
    }

    #[test]
    fn ask_game_backs_out_of_a_new_search_to_the_same_list() {
        let mut fake = Fake::default();
        fake.search("hades", true);
        let (result, out) = ask_game_with(&fake, "hades", &["3", "<", "1"]);
        assert_eq!(result.unwrap(), Some(7));
        assert_eq!(out.matches("Which game is it?").count(), 2, "{out}");
    }

    #[test]
    fn ask_game_with_no_matches_asks_for_a_term_and_succeeds() {
        let mut fake = Fake::default();
        fake.search("hades", false);
        fake.search("hadesii", true);
        let (result, out) = ask_game_with(&fake, "hades", &["hadesii", "1"]);
        assert_eq!(result.unwrap(), Some(7));
        assert!(
            out.contains("No SteamGridDB games match `hades`.\n"),
            "{out}"
        );
        assert!(out.contains("Search for: "), "{out}");
    }

    #[test]
    fn ask_game_backing_out_of_an_empty_search_gives_up_the_pick() {
        let mut fake = Fake::default();
        fake.search("hades", false);
        let (result, out) = ask_game_with(&fake, "hades", &["<"]);
        assert_eq!(result.unwrap(), None);
        assert!(
            out.contains("No SteamGridDB games match `hades`.\n"),
            "{out}"
        );
    }

    #[test]
    fn picking_from_a_new_search_saves_that_game() {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.search_one("arena", 42, "Hades Arena");
        let (result, _) = ask_game_with(&fake, "hades", &["3", "arena", "1"]);
        let game = result.unwrap().unwrap();
        art::save_game(&paths, "hades", game).unwrap();
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(42));
    }
}
