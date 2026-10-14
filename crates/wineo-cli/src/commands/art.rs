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
use wineo_core::download::{Https, Lazy, Transport};
use wineo_core::environment::Environment;
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb::{self, Api, Game, KEY_VAR, Key, Keyring, Removed, Saved};

use crate::cancel;
use crate::commands::desktop;
use crate::prompt::{Canceled, Input, Prompt, Reply, Terminal, numbered};
use crate::text;
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
                Err(err) if cancel::prompted(&err) => {
                    eprintln!("Canceled.");
                    Ok(cancel::exit_code())
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
pub(crate) fn apply_desktop(paths: &Paths, env: &str) -> anyhow::Result<()> {
    if wineo_core::desktop::supported(paths).is_err() {
        return Ok(());
    }
    desktop::apply(paths, env, false)
}

fn keyring() -> Box<dyn Keyring> {
    Box::new(SecretServiceKeyring::steamgriddb())
}

fn key(paths: &Paths, keyring: &dyn Keyring, remove: bool) -> anyhow::Result<()> {
    manage_key(
        &KeyService {
            name: "SteamGridDB",
            ask: "SteamGridDB API key: ",
            var: KEY_VAR,
        },
        remove,
        || Ok(steamgriddb::remove_key(paths, keyring)?),
        |key| {
            if !steamgriddb::check_key(&Https::new()?, key)? {
                bail!("SteamGridDB rejected the key");
            }
            Ok(())
        },
        |key| Ok(steamgriddb::save_key(paths, keyring, key)?),
    )
}

/// A service whose API key `key` commands save and remove.
pub struct KeyService<'a> {
    /// How messages name it.
    pub name: &'a str,
    pub ask: &'a str,
    /// The variable that overrides the saved key.
    pub var: &'a str,
}

/// The flow `art key` and `import --steam key` share. The key is never printed.
pub fn manage_key(
    service: &KeyService<'_>,
    remove: bool,
    remove_saved: impl FnOnce() -> anyhow::Result<Removed>,
    check: impl FnOnce(&str) -> anyhow::Result<()>,
    save: impl FnOnce(&str) -> anyhow::Result<Saved>,
) -> anyhow::Result<()> {
    let name = service.name;
    if std::env::var_os(service.var).is_some_and(|value| !value.is_empty()) {
        eprintln!("note: {} is set and overrides the saved key", service.var);
    }
    if remove {
        let removed = remove_saved()?;
        if removed.keyring || removed.file {
            println!("Removed the {name} key");
        } else {
            println!("No {name} key was saved");
        }
        return Ok(());
    }
    let Some(key) = prompt::secret(service.ask)?.filter(|key| !key.is_empty()) else {
        bail!("no key entered");
    };
    check(&key)?;
    match save(&key)? {
        Saved::Keyring => println!("Saved the {name} key to the keyring"),
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
        // The exact app id outranks a saved game from an old title search unless --game or --search is given.
        let app_id = proxied
            .app_id
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid app_id: not a number."))?;
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

    /// Writes `skip_line`, styled as a warning when the lookup failed.
    pub fn write_skip(&self, err: &mut dyn Write) -> io::Result<()> {
        match (self, self.skip_line()) {
            (KeyCheck::Failed(_), Some(line)) => writeln!(err, "{}", style::warning_stderr(line)),
            (_, Some(line)) => writeln!(err, "{line}"),
            (_, None) => Ok(()),
        }
    }

    /// Prints `skip_line` to stderr.
    pub fn print_skip(&self) {
        crate::output::handle(self.write_skip(&mut crate::output::stderr()));
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
        // Bulk imports ask for a transport per game; the clones share one
        // client instead of each starting its own.
        let https = Lazy::new();
        Self {
            check_key: Box::new(|| check_key(paths)),
            transport: Box::new(move || Ok(Box::new(https.https()?.clone()))),
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
                let _ = check.write_skip(err);
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
        Err(error) if cancel::prompted(&error) => Err(Canceled),
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
        .map(|resolved| text::width(&file(resolved)))
        .max()
        .unwrap_or(0);
    let row = |kind: Kind, resolved: &Resolved| {
        format!(
            "  {kind}  {}  {}",
            text::pad(&file(resolved), width),
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
        Some(Source::Gog(game)) => format!("GOG, game {game}"),
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
mod tests;
