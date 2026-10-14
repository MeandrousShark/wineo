use std::collections::HashSet;
use std::io::{self, Write};
use std::process::ExitCode;

use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Https, Lazy};
use wineo_core::key_store::{Keyring, Saved};
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steam::api::SteamApi;
use wineo_core::steam::owned;
use wineo_core::steam::{self, InstallState};
use wineo_core::steam_import::{
    self, ApplyOutcome, EntryState, NO_KEY_HINT, OwnedData as Data, OwnedGames, Plan, PlanOutcome,
};
use wineo_core::steamgriddb::Key;

use super::art::{self, KeyCheck};
use super::import::ReturnToSourceMenu;
use super::table::render_columns;
use super::transfer::Cancellation;
use crate::cancel;
use crate::prompt::{self, Canceled, Input, Prompt, Reply, Terminal, numbered};
use crate::style;
use crate::text;

/// Flags that mean nothing next to `--key` or `--remove-key`.
const KEY_CONFLICTS: [&str; 9] = [
    "list", "game", "name", "all", "owned", "yes", "art", "dry_run", "refresh",
];

#[derive(clap::Args, Default)]
pub struct Args {
    /// List Steam games without prompting
    #[arg(long)]
    pub list: bool,
    /// Steam app id
    #[arg(long)]
    pub game: Option<String>,
    /// Name for the new environment
    #[arg(short, long)]
    pub name: Option<String>,
    /// Import every installed Steam game
    #[arg(long, conflicts_with_all = ["game", "name", "list"])]
    pub all: bool,
    /// Also import the games you own but haven't installed
    #[arg(long, requires = "all")]
    pub owned: bool,
    /// Skip the bulk confirmation
    #[arg(short, long)]
    pub yes: bool,
    /// Fetch missing art from SteamGridDB by Steam app id
    #[arg(long)]
    pub art: bool,
    /// Fetch the owned games again instead of using the saved list
    #[arg(long)]
    pub refresh: bool,
    /// Review the import without changing anything
    #[arg(long, conflicts_with = "list")]
    pub dry_run: bool,
    /// Save a Steam Web API key, to see the games you haven't installed
    #[arg(long, conflicts_with = "remove_key", conflicts_with_all = KEY_CONFLICTS)]
    pub key: bool,
    /// Delete the saved Steam Web API key
    #[arg(long, conflicts_with_all = KEY_CONFLICTS)]
    pub remove_key: bool,
}

struct OutputOnly;

impl Input for OutputOnly {
    fn line(&mut self, _: &str) -> io::Result<crate::prompt::Line> {
        unreachable!("Steam review has no prompts")
    }
}

pub fn run(paths: &Paths, args: Args, first_question: bool) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        paths.portable_root().is_none(),
        "Steam games can't be imported into a portable folder."
    );
    let global = GlobalConfig::load(paths)?;
    let result = (|| {
        if args.key || args.remove_key {
            key(paths, &global, args.remove_key)
        } else if args.list {
            list(paths, &global, &args)
        } else if args.all {
            bulk(paths, &global, &args)
        } else if let Some(app_id) = &args.game {
            single(paths, &global, &args, app_id)
        } else {
            anyhow::ensure!(
                !args.dry_run && prompt::interactive(),
                "guided Steam import needs a terminal; pass --game <appid> or --all --yes"
            );
            let mut out = Prompt::new(Terminal::open()?, crate::output::stdout());
            guided(paths, &global, &args, &mut out, first_question)
        }
    })();
    match result {
        Err(error) if canceled(&error) => {
            eprintln!("Import canceled.");
            Ok(cancel::exit_code())
        }
        other => other,
    }
}

fn single(
    paths: &Paths,
    global: &GlobalConfig,
    args: &Args,
    app_id: &str,
) -> anyhow::Result<ExitCode> {
    // Installed games import with no key and no network, so the list is only
    // loaded when nothing on disk settles the game.
    let data = if steam_import::needs_owned(paths, global, app_id) {
        let data = Data::load(paths, global, args.refresh, !args.dry_run)?;
        data.require()?;
        data
    } else {
        Data::Unloaded
    };
    let games = data.games();
    match steam_import::plan(paths, global, games, app_id, args.name.as_deref())? {
        PlanOutcome::AlreadyImported(name) => {
            println!("Already imported as {name}.");
            if args.dry_run {
                println!("Dry run, nothing was imported.");
            }
            Ok(ExitCode::SUCCESS)
        }
        PlanOutcome::Ready(plan) => {
            data.warn();
            let mut out = Prompt::new(OutputOnly, crate::output::stdout());
            let key = art_key(paths, global, args.art);
            review(&plan, key.is_some(), &mut out)?;
            if args.dry_run {
                println!("Dry run, nothing was imported.");
            } else {
                let cancellation = Cancellation::new()?;
                apply(
                    paths,
                    global,
                    games,
                    &plan,
                    key.as_ref(),
                    &Lazy::new(),
                    &cancellation.token,
                )?;
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// How the CLI reports what the owned list lacks.
trait Notes {
    fn require(&self) -> anyhow::Result<()>;
    fn notes(&self, hint: bool);
    fn warn(&self);
}

impl Notes for Data {
    /// For commands that can't go on without the list. A missing key is left
    /// to the import, which says a game isn't installed.
    fn require(&self) -> anyhow::Result<()> {
        match (self, self.problem()) {
            (Data::KeyFailed(_) | Data::Failed(_), Some(problem)) => anyhow::bail!("{problem}"),
            _ => Ok(()),
        }
    }

    /// Says what is wrong with the list, for commands that go on without it
    /// or with an old one. With `hint`, a missing key is said too.
    fn notes(&self, hint: bool) {
        if hint && matches!(self, Data::NoKey) {
            eprintln!("{NO_KEY_HINT}");
        } else if let Some(problem) = self.problem() {
            eprintln!("{}", style::warning_stderr(problem));
        }
    }

    fn warn(&self) {
        self.notes(false);
    }
}

fn canceled(error: &anyhow::Error) -> bool {
    cancel::prompted(error) || error.is::<wineo_core::import::Cancelled>() || cancel::core(error)
}

fn list(paths: &Paths, global: &GlobalConfig, args: &Args) -> anyhow::Result<ExitCode> {
    let data = Data::load(paths, global, args.refresh, true)?;
    let entries = steam_import::listing(paths, global, data.games())?;
    if !entries.is_empty() {
        let mut rows = vec![["APP ID", "TITLE", "SIZE", "STATE", "IMPORTED"].map(str::to_owned)];
        rows.extend(entries.iter().map(|entry| {
            [
                entry.app_id.clone(),
                entry.title.clone().unwrap_or_else(|| "-".to_owned()),
                entry
                    .size_on_disk
                    .map(style::size)
                    .unwrap_or_else(|| "-".to_owned()),
                state_label(entry),
                entry.imported_as.clone().unwrap_or_default(),
            ]
        }));
        for line in render_columns(&rows) {
            println!("{line}");
        }
    }
    data.notes(true);
    Ok(ExitCode::SUCCESS)
}

fn state_label(entry: &steam_import::Entry) -> String {
    match (&entry.state, entry.install_progress) {
        (EntryState::Installed, _) => "installed".to_owned(),
        (EntryState::Updating, _) => "updating".to_owned(),
        (EntryState::Installing, Some(percent)) => format!("installing {percent}%"),
        (EntryState::Installing, None) => "installing".to_owned(),
        (EntryState::InstallQueued, _) => "install queued".to_owned(),
        (EntryState::Uninstalling, _) => "uninstalling".to_owned(),
        (EntryState::NotInstalled, _) => "not installed".to_owned(),
        (EntryState::Unreadable(_), _) => "unreadable manifest".to_owned(),
    }
}

fn keyring() -> Box<dyn Keyring> {
    Box::new(SecretServiceKeyring::for_spec(&steam::key::KEY_SPEC))
}

/// Saves or removes the Steam Web API key, like `art key` does for
/// SteamGridDB's. The key is never printed.
fn key(paths: &Paths, global: &GlobalConfig, remove: bool) -> anyhow::Result<ExitCode> {
    let var = steam::key::KEY_VAR;
    if std::env::var_os(var).is_some_and(|value| !value.is_empty()) {
        eprintln!("note: {var} is set and overrides the saved key");
    }
    if remove {
        let removed = steam::key::remove_key(paths, &*keyring())?;
        if removed.keyring || removed.file {
            println!("Removed the Steam Web API key");
        } else {
            println!("No Steam Web API key was saved");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let Some(key) = prompt::secret("Steam Web API key: ")?.filter(|key| !key.is_empty()) else {
        anyhow::bail!("no key entered");
    };
    let install =
        steam::active_for(paths, global).ok_or_else(|| anyhow::anyhow!("Steam wasn't found."))?;
    let api = SteamApi::from_env().map_err(|error| anyhow::anyhow!("{error}"))?;
    owned::check_key(&api, &install, &key)
        .map_err(|error| anyhow::anyhow!("Couldn't check the key: {error}"))?;
    match steam::key::save_key(paths, &*keyring(), &key)? {
        Saved::Keyring => println!("Saved the Steam Web API key to the keyring"),
        Saved::File(path) => eprintln!(
            "{}",
            style::warning_stderr(format!(
                "warning: no keyring, so the key is stored in plain text in {}",
                path.display()
            ))
        ),
    }
    Ok(ExitCode::SUCCESS)
}

fn review<I: Input, W: Write>(plan: &Plan, fetch: bool, out: &mut Prompt<I, W>) -> io::Result<()> {
    out.say(format!("Ready to import {}:", plan.name()))?;
    out.say(format!("  game      {} ({})", plan.title(), plan.app_id()))?;
    if plan.state() == InstallState::NotInstalled {
        out.say("  state     not installed")?;
    } else {
        out.say(format!("  size      {}", style::size(plan.size_on_disk())))?;
    }
    out.say("  launcher  Steam")?;
    out.say("  tags      game")?;
    out.say("  desktop   off")?;
    out.say(if fetch {
        "  art       Steam cache, then SteamGridDB"
    } else {
        "  art       Steam cache"
    })
}

fn art_key(paths: &Paths, global: &GlobalConfig, wanted: bool) -> Option<Key> {
    if !wanted || global.art.fetch.kinds().is_empty() {
        return None;
    }
    let check = art::check_key(paths);
    check.print_skip();
    match check {
        KeyCheck::Found(key) => Some(key),
        _ => None,
    }
}

fn apply(
    paths: &Paths,
    global: &GlobalConfig,
    games: OwnedGames<'_>,
    plan: &Plan,
    key: Option<&Key>,
    https: &Lazy<Https>,
    cancel: &Token,
) -> anyhow::Result<ApplyOutcome> {
    let outcome = steam_import::apply(paths, global, games, plan, cancel)?;
    let applied = match &outcome {
        ApplyOutcome::Imported(applied) => applied,
        ApplyOutcome::AlreadyImported(name) => {
            println!("Already imported as {name}.");
            return Ok(outcome);
        }
    };
    for warning in &applied.warnings {
        eprintln!("warning: {warning}");
    }
    println!("Imported {}.", applied.environment.name);
    if cancel.is_canceled() {
        return Err(Canceled.into());
    }
    if let Some(key) = key {
        fetch_art(paths, global, plan, key, https, cancel)?;
    }
    Ok(outcome)
}

fn fetch_art(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &Plan,
    key: &Key,
    https: &Lazy<Https>,
    cancel: &Token,
) -> anyhow::Result<()> {
    if cancel.is_canceled() {
        return Err(Canceled.into());
    }
    let result = (|| {
        let api = wineo_core::steamgriddb::Api::new(https.https()?, &key.value);
        let language = wineo_core::steamgriddb::language(|var| std::env::var_os(var));
        wineo_core::art_fetch::fetch_steam(
            paths,
            plan.name(),
            plan.app_id()
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid app_id: not a number."))?,
            &api,
            &language,
            &global.art.fetch.kinds(),
        )
    })();
    match result {
        Ok(results) => {
            for (kind, fetched) in results {
                if let Err(error) = fetched {
                    eprintln!("warning: {kind} art couldn't be fetched: {error:#}.");
                }
            }
        }
        Err(error) => eprintln!("warning: SteamGridDB art couldn't be fetched: {error:#}."),
    }
    if cancel.is_canceled() {
        return Err(Canceled.into());
    }
    Ok(())
}

fn bulk(paths: &Paths, global: &GlobalConfig, args: &Args) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        args.dry_run || args.yes || prompt::interactive(),
        "bulk Steam import needs a terminal; pass --yes to skip the confirmation"
    );
    // One fetch for the whole run. Without the owned games, only installed
    // ones are imported.
    let data = if args.owned {
        let data = Data::load(paths, global, args.refresh, !args.dry_run)?;
        match &data {
            Data::NoKey => anyhow::bail!("Add a Steam Web API key to import uninstalled games."),
            other => other.require()?,
        }
        data.warn();
        data
    } else {
        Data::Unloaded
    };
    let games = data.games();
    let key = art_key(paths, global, args.art);
    let mut ready = Vec::new();
    let mut failures = Vec::new();
    let mut done = 0;
    let mut already_imported = Vec::new();
    let mut names = HashSet::new();
    let mut out = Prompt::new(OutputOnly, crate::output::stdout());
    for entry in steam_import::listing(paths, global, games)? {
        if let Some(name) = entry.imported_as {
            println!("Already imported as {name}.");
            continue;
        }
        match steam_import::plan(paths, global, games, &entry.app_id, None) {
            Ok(PlanOutcome::Ready(plan)) => {
                if names.insert(plan.name().to_owned()) {
                    review(&plan, key.is_some(), &mut out)?;
                    ready.push(plan);
                } else {
                    failures.push((
                        entry.app_id,
                        plan.name().to_owned(),
                        wineo_core::Error::EnvironmentExists(plan.name().to_owned()).to_string(),
                    ));
                }
            }
            Ok(PlanOutcome::AlreadyImported(name)) => println!("Already imported as {name}."),
            Err(error) => {
                let name = entry
                    .title
                    .as_deref()
                    .map(|title| wineo_core::heroic_import::default_name(title, None))
                    .unwrap_or_else(|| entry.app_id.clone());
                failures.push((entry.app_id, name, format!("{error:#}")));
            }
        }
    }
    let mut total = ready.len();
    if args.dry_run {
        println!("Dry run, nothing was imported.");
    } else if !ready.is_empty() {
        if !args.yes {
            let mut out =
                Prompt::new(Terminal::open()?, crate::output::stdout()).with_first_question(true);
            match out.confirm(
                &format!("Import {}?", text::counted(ready.len(), "game", "games")),
                true,
            )? {
                Reply::Value(true) => {}
                _ => return Err(Canceled.into()),
            }
        }
        let cancellation = Cancellation::new()?;
        // One client for every game's art, not one per game.
        let https = Lazy::new();
        for plan in ready {
            if cancellation.token.is_canceled() {
                return Err(Canceled.into());
            }
            match apply(
                paths,
                global,
                games,
                &plan,
                key.as_ref(),
                &https,
                &cancellation.token,
            ) {
                Ok(ApplyOutcome::Imported(_)) => done += 1,
                Ok(ApplyOutcome::AlreadyImported(name)) => {
                    total -= 1;
                    already_imported.push(name);
                }
                Err(error) if canceled(&error) => return Err(error),
                Err(error) => failures.push((
                    plan.app_id().to_owned(),
                    plan.name().to_owned(),
                    format!("{error:#}"),
                )),
            }
        }
    }
    for (app_id, _, reason) in &failures {
        eprintln!("Failed {app_id}: {reason}");
    }
    if !args.dry_run {
        println!("{}", text::games_summary("Imported", done, total));
        if !already_imported.is_empty() {
            println!(
                "{} already imported: {}",
                already_imported.len(),
                already_imported.join(", ")
            );
        }
        if !failures.is_empty() {
            let names: Vec<_> = failures.iter().map(|(_, name, _)| name.as_str()).collect();
            println!("{} failed: {}", failures.len(), names.join(", "));
        }
    }
    Ok(if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Game,
    Name,
    Confirm,
}

fn guided<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    args: &Args,
    out: &mut Prompt<I, W>,
    first_question: bool,
) -> anyhow::Result<ExitCode> {
    let data = Data::load(paths, global, args.refresh, true)?;
    let games = data.games();
    let entries: Vec<_> = steam_import::listing(paths, global, games)?
        .into_iter()
        .filter(|entry| {
            entry.imported_as.is_none() && !matches!(entry.state, EntryState::Unreadable(_))
        })
        .collect();
    data.notes(true);
    anyhow::ensure!(!entries.is_empty(), "Steam has no games to import.");
    let choices = numbered(entries.iter().map(|entry| {
        let title = entry.title.as_deref().unwrap_or("-");
        let label = if entry.state == EntryState::NotInstalled {
            format!("{title} ({}, not installed)", entry.app_id)
        } else {
            format!("{title} ({})", entry.app_id)
        };
        (label, entry.app_id.clone())
    }));
    let mut step = Step::Game;
    let mut game = String::new();
    let mut name = args.name.clone();
    loop {
        out.set_first_question(step == Step::Game && first_question);
        match step {
            Step::Game => match out.choose("Which Steam game?", "Game", &choices, None, true)? {
                Reply::Value(value) => {
                    game = value;
                    name = args.name.clone();
                    step = Step::Name;
                }
                Reply::Back if first_question => return Err(Canceled.into()),
                Reply::Back => return Err(ReturnToSourceMenu.into()),
            },
            Step::Name => {
                let entry = entries.iter().find(|entry| entry.app_id == game).unwrap();
                let default = wineo_core::heroic_import::default_name(
                    entry.title.as_deref().unwrap_or(""),
                    None,
                );
                match out.text(
                    "Environment name",
                    Some(name.as_deref().unwrap_or(&default)),
                    |value| {
                        steam_import::plan(paths, global, games, &game, Some(value))
                            .map(|_| value.to_owned())
                            .map_err(|error| error.to_string())
                    },
                )? {
                    Reply::Value(value) => {
                        name = Some(value);
                        step = Step::Confirm;
                    }
                    Reply::Back => step = Step::Game,
                }
            }
            Step::Confirm => {
                let plan = match steam_import::plan(paths, global, games, &game, name.as_deref())? {
                    PlanOutcome::Ready(plan) => plan,
                    PlanOutcome::AlreadyImported(name) => {
                        out.say(format!("Already imported as {name}."))?;
                        return Ok(ExitCode::SUCCESS);
                    }
                };
                let key = art_key(paths, global, args.art);
                review(&plan, key.is_some(), out)?;
                out.say("Press Enter to import it, Esc to go back, Ctrl+C to cancel.")?;
                match out.confirm("Import it?", true)? {
                    Reply::Back => step = Step::Name,
                    Reply::Value(false) => return Err(Canceled.into()),
                    Reply::Value(true) => {
                        let cancellation = Cancellation::new()?;
                        apply(
                            paths,
                            global,
                            games,
                            &plan,
                            key.as_ref(),
                            &Lazy::new(),
                            &cancellation.token,
                        )?;
                        return Ok(ExitCode::SUCCESS);
                    }
                }
            }
        }
    }
}
