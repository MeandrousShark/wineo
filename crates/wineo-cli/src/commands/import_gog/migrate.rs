//! Moving the games Heroic installed into wineo, one at a time, all at once,
//! or picked from a list, and forgetting a game folder's saved state.

use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::gog::folder::{self, Platform, Track};
use wineo_core::gog::migrate::{self, MigrateError, Migration, Outcome, Plan, StateChange, Target};
use wineo_core::heroic::{HeroicGogGame, HeroicImport, HeroicRunnerKind, HeroicSource};
use wineo_core::heroic_import::{self, RunnerSuggestion, suggest_runner};
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;

use super::Args;
use crate::commands::gog_support::{usage_error, warn};
use crate::commands::import::{ReturnToSourceMenu, check_review};
use crate::commands::import_heroic::{
    ChosenRunner, ensure_components_available, finish_registered, pick_runner, report_playtime,
};
use crate::commands::transfer::Cancellation;
use crate::picker;
use crate::prompt::{self, Canceled, Prompt, Reply, Terminal, numbered};
use crate::style;

type Ask = Prompt<Terminal, crate::output::Stream<io::Stdout>>;

const NEEDS_YES: &str = "Migrating without a terminal needs --yes.";
const HEROIC_NOTE: &str =
    "Remove it from Heroic's library, without deleting its files, so both don't update it.";

fn host_only(paths: &Paths) -> anyhow::Result<()> {
    if paths.portable_root().is_some() {
        return Err(MigrateError::Portable.into());
    }
    Ok(())
}

fn not_installed(id: &str) -> String {
    format!("Heroic hasn't installed GOG game {id}. Installing games comes in a later release.")
}

/// Every Heroic configuration found, in the places Heroic keeps one.
struct Heroic {
    sources: Vec<HeroicSource>,
}

impl Heroic {
    fn load(paths: &Paths) -> anyhow::Result<Self> {
        Ok(Self {
            sources: heroic_import::sources(paths, None)?,
        })
    }

    /// The configuration that installed game `id`, None when no Heroic did.
    fn find(&self, id: &str) -> anyhow::Result<Option<&HeroicSource>> {
        let mut found = Vec::new();
        for source in &self.sources {
            if source.gog_game(id)?.is_some() {
                found.push(source);
            }
        }
        match found.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(only)),
            _ => anyhow::bail!("More than one Heroic configuration has installed GOG game {id}."),
        }
    }

    /// Every GOG game Heroic installed, and what couldn't be read.
    fn games(&self) -> (Vec<(&HeroicSource, HeroicGogGame)>, Vec<Unreadable>) {
        let mut games = Vec::new();
        let mut unreadable = Vec::new();
        for source in &self.sources {
            match source.gog_installed() {
                Ok(listing) => {
                    games.extend(listing.games.into_iter().map(|game| (source, game)));
                    unreadable.extend(listing.issues.into_iter().map(|issue| {
                        Unreadable {
                            subject: issue
                                .field
                                .map_or_else(|| "Heroic".to_owned(), |id| format!("GOG game {id}")),
                            message: issue.message,
                        }
                    }));
                }
                Err(error) => unreadable.push(Unreadable {
                    subject: "Heroic".to_owned(),
                    message: format!("Couldn't read its GOG games: {error:#}"),
                }),
            }
        }
        (games, unreadable)
    }
}

/// Something in Heroic's records that can't be used, and what it is.
struct Unreadable {
    subject: String,
    message: String,
}

enum Planned {
    Already {
        environment: String,
        folder: std::path::PathBuf,
    },
    Ready(Box<Migration>),
}

fn plan_one(paths: &Paths, source: &HeroicSource, id: &str) -> anyhow::Result<Planned> {
    match migrate::plan(paths, source, id)? {
        Plan::NotInstalled { gog_id } => anyhow::bail!(not_installed(&gog_id)),
        Plan::AlreadyMigrated {
            environment,
            folder,
        } => Ok(Planned::Already {
            environment,
            folder,
        }),
        Plan::Migrate(migration) => Ok(Planned::Ready(Box::new(migration))),
    }
}

fn say_already(paths: &Paths, environment: &str, folder: &Path) {
    println!(
        "Already migrated. {environment} owns {}.",
        style::path(paths, folder)
    );
}

/// Settles what a migration still needs: a runner for a new Windows
/// environment, and the checks `apply` would fail on anyway, so a dry run or
/// a question never comes before them. With `ask`, a person picks the runner;
/// without one, Heroic's own is used when wineo has it, or else the newest
/// Proton.
fn prepare(
    paths: &Paths,
    global: &GlobalConfig,
    migration: &mut Migration,
    mut ask: Option<&mut Ask>,
    verbose: bool,
) -> anyhow::Result<()> {
    if let Target::Windows {
        imported,
        plan: None,
    } = migration.target()
    {
        let imported = imported.clone();
        let chosen = match ask.as_deref_mut() {
            Some(prompt) => pick_runner(paths, global, &imported, None, prompt, verbose)?,
            None => automatic_runner(paths, global, &imported, &migration.game().title)?,
        };
        migration.plan_runner(paths, global, &chosen.name, None, None)?;
        if let Some(plan) = migration.game_plan_mut() {
            plan.set_runner_dependencies(chosen.dependencies);
        }
    }
    if let Some(plan) = migration.game_plan_mut() {
        match ask {
            Some(prompt) => ensure_components_available(paths, global, plan, prompt)?,
            None => plan.check_builds(paths, global)?,
        }
        check_review(paths, global, plan.request(), plan.inspection())?;
    }
    if migration.name_taken(paths) {
        let name = migration.environment().unwrap_or_default();
        return Err(wineo_core::Error::EnvironmentExists(name.to_owned()).into());
    }
    Ok(())
}

fn automatic_runner(
    paths: &Paths,
    global: &GlobalConfig,
    imported: &HeroicImport,
    title: &str,
) -> anyhow::Result<ChosenRunner> {
    if let RunnerSuggestion::Use(name) = suggest_runner(paths, global, imported, None)? {
        return Ok(ChosenRunner::named(name));
    }
    if imported.runner.kind == HeroicRunnerKind::Proton {
        return match Runners::new(paths, global)?.newest_proton()? {
            Some(runner) => Ok(ChosenRunner::named(runner.selection().to_owned())),
            None => anyhow::bail!(
                "{title} needs a Proton runner. Get one with: wineo download ge-proton"
            ),
        };
    }
    anyhow::bail!(
        "{title} needs a Wine runner, and Heroic's {} isn't installed in wineo.",
        imported.runner.name
    )
}

fn review_lines(paths: &Paths, migration: &Migration) -> Vec<String> {
    let game = migration.game();
    let committed = migration.committed();
    let environment = migration.environment().unwrap_or("?");
    let kind = match migration.target() {
        Target::Existing { .. } => "existing",
        Target::Windows { .. } => "new Windows environment",
        Target::Native(_) => "new native environment",
    };
    let version = committed
        .products
        .iter()
        .find(|product| product.id == committed.gog_id)
        .and_then(|product| match committed.track {
            Track::Installer => product.version.clone(),
            Track::Galaxy => product.build.clone().map(|build| format!("build {build}")),
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let mut rows = vec![
        ("environment", format!("{environment} ({kind})")),
        ("folder", style::path(paths, migration.folder())),
        (
            "track",
            match committed.track {
                Track::Installer => "installer",
                Track::Galaxy => "galaxy",
            }
            .to_owned(),
        ),
        ("version", version),
        ("language", committed.language.clone()),
    ];
    let dlc: Vec<String> = committed
        .products
        .iter()
        .filter(|product| product.id != committed.gog_id)
        .map(|product| product.id.to_string())
        .collect();
    if !dlc.is_empty() {
        rows.push(("dlc", format!("{} ({})", dlc.len(), dlc.join(", "))));
    }
    if let Target::Windows {
        plan: Some(plan), ..
    } = migration.target()
    {
        rows.push(("runner", plan.runner().name().to_owned()));
        rows.push((
            "prefix",
            format!(
                "{} (Heroic's, kept where it is)",
                style::path(paths, plan.destination())
            ),
        ));
    }
    if let StateChange::Reclaim { journal } = migration.state() {
        rows.push((
            "folder state",
            if journal {
                "kept from before, with an interrupted update"
            } else {
                "kept from before"
            }
            .to_owned(),
        ));
    }
    let mut lines = vec![format!("Migrate {} ({})", game.title, game.id)];
    lines.extend(
        rows.into_iter()
            .map(|(label, value)| format!("  {label:<13}{value}")),
    );
    lines
}

fn print_review(paths: &Paths, migration: &Migration) {
    for line in review_lines(paths, migration) {
        println!("{line}");
    }
}

fn apply(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    migration: &Migration,
) -> anyhow::Result<Outcome> {
    let cancellation = Cancellation::new()?;
    // A migration that finished stays done even if Ctrl+C came at the end.
    cancellation.published_result(migrate::apply(
        paths,
        global,
        source,
        migration,
        &cancellation.token,
        |_| {},
    ))
}

/// Says what a finished migration did. Returns whether it migrated anything.
fn report(
    paths: &Paths,
    global: &GlobalConfig,
    migration: &Migration,
    outcome: Outcome,
) -> anyhow::Result<bool> {
    match outcome {
        Outcome::AlreadyMigrated {
            environment,
            folder,
        } => {
            say_already(paths, &environment, &folder);
            Ok(false)
        }
        Outcome::Migrated(migrated) => {
            if let Some(registered) = &migrated.registered {
                report_playtime(&registered.playtime);
                finish_registered(paths, global, registered)?;
            }
            println!(
                "Migrated {} as {}.",
                migration.game().title,
                migrated.environment
            );
            if matches!(migrated.state, StateChange::Reclaim { journal: true }) {
                println!(
                    "An interrupted update was kept. Finish it with: wineo update {}",
                    migrated.environment
                );
            }
            println!("{HEROIC_NOTE}");
            Ok(true)
        }
    }
}

/// The question for a game named on the command line, where there is no
/// earlier question to go back to.
fn confirm(prompt: &mut Ask) -> anyhow::Result<()> {
    prompt.say("Press Enter to migrate it, Esc or Ctrl+C to cancel.")?;
    match prompt.confirm("Migrate it?", true)? {
        Reply::Value(true) => Ok(()),
        _ => Err(Canceled.into()),
    }
}

fn terminal() -> anyhow::Result<Ask> {
    Ok(Prompt::new(Terminal::open()?, crate::output::stdout()).with_first_question(true))
}

pub(super) fn one(paths: &Paths, args: &Args, id: &str, verbose: bool) -> anyhow::Result<ExitCode> {
    host_only(paths)?;
    let global = GlobalConfig::load(paths)?;
    let heroic = Heroic::load(paths)?;
    let Some(source) = heroic.find(id)? else {
        anyhow::bail!(not_installed(id));
    };
    let mut migration = match plan_one(paths, source, id)? {
        Planned::Already {
            environment,
            folder,
        } => {
            say_already(paths, &environment, &folder);
            if args.dry_run {
                println!("Dry run, nothing was migrated.");
            }
            return Ok(ExitCode::SUCCESS);
        }
        Planned::Ready(migration) => *migration,
    };
    let asking = !args.yes && !args.dry_run;
    if asking && !prompt::interactive() {
        return Ok(usage_error(NEEDS_YES));
    }
    let mut prompt = if asking { Some(terminal()?) } else { None };
    prepare(paths, &global, &mut migration, prompt.as_mut(), verbose)?;
    print_review(paths, &migration);
    if args.dry_run {
        println!("Dry run, nothing was migrated.");
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(prompt) = prompt.as_mut() {
        confirm(prompt)?;
    }
    let outcome = apply(paths, &global, source, &migration)?;
    report(paths, &global, &migration, outcome)?;
    Ok(ExitCode::SUCCESS)
}

pub(super) fn all(paths: &Paths, args: &Args, verbose: bool) -> anyhow::Result<ExitCode> {
    host_only(paths)?;
    let asking = !args.yes && !args.dry_run;
    if asking && !prompt::interactive() {
        return Ok(usage_error(NEEDS_YES));
    }
    let global = GlobalConfig::load(paths)?;
    let heroic = Heroic::load(paths)?;
    let (games, unreadable) = heroic.games();
    // A record that can't be read is a game that can't be migrated.
    let mut failures: Vec<(String, String)> = unreadable
        .into_iter()
        .map(|entry| (entry.subject, entry.message))
        .collect();
    if games.is_empty() && failures.is_empty() {
        println!("Heroic has no GOG games.");
        return Ok(ExitCode::SUCCESS);
    }
    let mut prompt = if asking { Some(terminal()?) } else { None };

    let mut ready: Vec<(&HeroicSource, Migration)> = Vec::new();
    let mut already: Vec<String> = Vec::new();
    let mut names = HashSet::new();
    for (source, game) in games {
        let planned = plan_one(paths, source, &game.id).and_then(|planned| match planned {
            Planned::Ready(mut migration) => {
                prepare(paths, &global, &mut migration, prompt.as_mut(), verbose)?;
                let name = migration.environment().unwrap_or_default().to_owned();
                if !names.insert(name.clone()) {
                    return Err(wineo_core::Error::EnvironmentExists(name).into());
                }
                Ok(Planned::Ready(migration))
            }
            already => Ok(already),
        });
        match planned {
            Ok(Planned::Already {
                environment,
                folder,
            }) => {
                say_already(paths, &environment, &folder);
                already.push(environment);
            }
            Ok(Planned::Ready(migration)) => {
                print_review(paths, &migration);
                ready.push((source, *migration));
            }
            Err(error) if super::canceled(&error) => return Err(error),
            Err(error) => failures.push((game.title, format!("{error:#}"))),
        }
    }

    let total = ready.len();
    let mut done = 0;
    if args.dry_run {
        for (title, reason) in &failures {
            eprintln!("{title}: would fail: {reason}");
        }
        println!("Dry run, nothing was migrated.");
        return Ok(if failures.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    if !ready.is_empty() {
        if let Some(prompt) = prompt.as_mut() {
            let noun = if total == 1 { "game" } else { "games" };
            match prompt.confirm(&format!("Migrate {total} {noun}?"), true)? {
                Reply::Value(true) => {}
                _ => return Err(Canceled.into()),
            }
        }
        for (source, migration) in &ready {
            match apply(paths, &global, source, migration)
                .and_then(|outcome| report(paths, &global, migration, outcome))
            {
                Ok(true) => done += 1,
                Ok(false) => {
                    already.push(migration.environment().unwrap_or_default().to_owned());
                }
                Err(error) if super::canceled(&error) => return Err(error),
                Err(error) => failures.push((migration.game().title.clone(), format!("{error:#}"))),
            }
        }
    }
    for (title, reason) in &failures {
        eprintln!("Failed {title}: {reason}");
    }
    if total > 0 {
        let noun = if total == 1 { "game" } else { "games" };
        println!("Migrated {done} of {total} {noun}.");
    } else if failures.is_empty() && already.is_empty() {
        println!("Nothing to migrate.");
    }
    if !already.is_empty() {
        println!("{} already migrated: {}", already.len(), already.join(", "));
    }
    if !failures.is_empty() {
        let titles: Vec<_> = failures.iter().map(|(title, _)| title.as_str()).collect();
        println!("{} failed: {}", failures.len(), titles.join(", "));
    }
    Ok(if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub(super) fn guided(
    paths: &Paths,
    args: &Args,
    first_question: bool,
    verbose: bool,
) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        prompt::interactive(),
        "guided GOG migration needs a terminal; pass --game <id>, or --all --yes"
    );
    host_only(paths)?;
    let global = GlobalConfig::load(paths)?;
    let heroic = Heroic::load(paths)?;
    let (games, unreadable) = heroic.games();
    for entry in unreadable {
        warn(format!("{}: {}", entry.subject, entry.message));
    }
    let mut candidates: Vec<(&HeroicSource, Migration)> = Vec::new();
    for (source, game) in games {
        match plan_one(paths, source, &game.id) {
            Ok(Planned::Ready(migration)) => candidates.push((source, *migration)),
            Ok(Planned::Already { .. }) => {}
            Err(error) => warn(format!("{}: {error:#}", game.title)),
        }
    }
    anyhow::ensure!(
        !candidates.is_empty(),
        "Heroic has no GOG games to migrate."
    );
    candidates.sort_by_cached_key(|(_, migration)| migration.game().title.to_lowercase());

    let mut prompt = Prompt::new(Terminal::open()?, crate::output::stdout());
    let choices = numbered(
        candidates
            .iter()
            .enumerate()
            .map(|(index, (_, migration))| {
                let game = migration.game();
                let platform = match game.platform {
                    Platform::Linux => "Linux",
                    Platform::Windows => "Windows",
                };
                (format!("{} ({}, {platform})", game.title, game.id), index)
            }),
    );
    let mut step = 0;
    let mut picked = 0;
    loop {
        prompt.set_first_question(step == 0 && first_question);
        match step {
            0 => match prompt.choose("Which GOG game?", "Game", &choices, None, true)? {
                Reply::Value(index) => {
                    picked = index;
                    step = 1;
                }
                Reply::Back if first_question => return Err(Canceled.into()),
                Reply::Back => return Err(ReturnToSourceMenu.into()),
            },
            _ => {
                let (source, planned) = &candidates[picked];
                let mut migration = planned.clone();
                // A dry run asks nothing that could write, and --yes
                // prepares the way it does everywhere else.
                let asking = !args.dry_run && !args.yes;
                if let Err(error) = prepare(
                    paths,
                    &global,
                    &mut migration,
                    asking.then_some(&mut prompt),
                    verbose,
                ) {
                    if super::canceled(&error) {
                        return Err(error);
                    }
                    prompt.say(format!("Can't migrate: {error:#}"))?;
                    step = 0;
                    continue;
                }
                for line in review_lines(paths, &migration) {
                    prompt.say(line)?;
                }
                if args.dry_run {
                    prompt.say("Dry run, nothing was migrated.")?;
                    return Ok(ExitCode::SUCCESS);
                }
                let answer = if args.yes {
                    Reply::Value(true)
                } else {
                    prompt.say("Press Enter to migrate it, Esc to go back, Ctrl+C to cancel.")?;
                    prompt.confirm("Migrate it?", true)?
                };
                match answer {
                    Reply::Back => step = 0,
                    Reply::Value(false) => return Err(Canceled.into()),
                    Reply::Value(true) => {
                        let outcome = apply(paths, &global, source, &migration)?;
                        report(paths, &global, &migration, outcome)?;
                        return Ok(ExitCode::SUCCESS);
                    }
                }
            }
        }
    }
}

pub(super) fn forget(paths: &Paths, path: &str) -> anyhow::Result<ExitCode> {
    host_only(paths)?;
    // The game's folder may be gone, which is why its state gets forgotten,
    // so the path can't be required to exist.
    let folder = wineo_core::paths::resolve(&paths.absolute(&picker::clean_path(path))?);
    let cancellation = Cancellation::new()?;
    let removed = cancellation.published_result(folder::forget(paths, &folder, &|| {
        cancellation.token.is_canceled()
    }))?;
    let shown = style::path(paths, &folder);
    if removed {
        println!("Forgot the GOG state of {shown}.");
    } else {
        println!("No GOG state is saved for {shown}.");
    }
    Ok(ExitCode::SUCCESS)
}
