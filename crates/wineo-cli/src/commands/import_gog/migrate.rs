//! Moving the games Heroic installed into wineo, one at a time, all at once,
//! or picked from a list, and forgetting a game folder's saved state.

use std::collections::HashSet;
use std::path::Path;
use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::gog::folder::{self, Journal, Platform};
use wineo_core::gog::migrate::{
    self, MigrateError, Migration, Outcome, Plan, ReviewLabel, StateChange, Target,
};
use wineo_core::heroic::{
    self, HeroicGogConflict, HeroicGogEntry, HeroicGogLookup, HeroicImport, HeroicRunnerKind,
    HeroicSource,
};
use wineo_core::heroic_import::{self, RunnerSuggestion, suggest_runner};
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;

use super::Args;
use super::extras::Batch;
use crate::commands::gog_support::{Account, finish_command, journal_for, usage_error, warn};
use crate::commands::import::{ReturnToSourceMenu, check_review};
use crate::commands::import_heroic::{
    ChosenRunner, ensure_components_available, finish_registered, pick_runner, report_playtime,
};
use crate::commands::transfer::Cancellation;
use crate::picker;
use crate::prompt::{self, Ask, Canceled, Prompt, Reply, Terminal, numbered, terminal};
use crate::style;
use crate::text;

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
    format!("Heroic doesn't have GOG game {id}.")
}

/// Refuses the install options for a game Heroic has, since `--game`
/// migrates it instead.
fn refuse_install_options(args: &Args, title: &str) -> Option<ExitCode> {
    let given: Vec<&str> = [
        (args.platform.is_some(), "--platform"),
        (args.track.is_some(), "--track"),
        (args.language.is_some(), "--language"),
        (args.runner.is_some(), "--runner"),
        (args.arch.is_some(), "--arch"),
        (args.dir.is_some(), "--dir"),
    ]
    .into_iter()
    .filter_map(|(given, flag)| given.then_some(flag))
    .collect();
    if given.is_empty() {
        return None;
    }
    let flags = super::listed(&given);
    Some(usage_error(format!(
        "Heroic has {title} installed, so --game migrates it. Leave out {flags}."
    )))
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
    /// Refuses a game the configs put in different folders.
    fn find(&self, paths: &Paths, id: &str) -> anyhow::Result<Option<HeroicGogEntry<'_>>> {
        match heroic::find_gog_game(&self.sources, id)? {
            HeroicGogLookup::NotInstalled => Ok(None),
            HeroicGogLookup::Found(entry) => Ok(Some(entry)),
            HeroicGogLookup::Conflict(conflict) => {
                anyhow::bail!("GOG game {id} is {}.", conflict_text(paths, &conflict))
            }
        }
    }

    /// Every GOG game Heroic installed, once each, and what couldn't be used.
    fn games(&self, paths: &Paths) -> (Vec<HeroicGogEntry<'_>>, Vec<Unreadable>) {
        let listing = heroic::gog_games(&self.sources);
        let mut unreadable: Vec<Unreadable> = listing
            .problems
            .into_iter()
            .map(|problem| Unreadable {
                subject: problem
                    .id
                    .map_or_else(|| "Heroic".to_owned(), |id| format!("GOG game {id}")),
                message: problem.message,
            })
            .collect();
        unreadable.extend(listing.conflicts.iter().map(|conflict| Unreadable {
            subject: format!("GOG game {}", conflict.id),
            message: conflict_text(paths, conflict),
        }));
        (listing.games, unreadable)
    }
}

fn conflict_text(paths: &Paths, conflict: &HeroicGogConflict) -> String {
    let count = match conflict.places.len() {
        2 => "two".to_owned(),
        count => count.to_string(),
    };
    format!(
        "installed in {count} places: {}",
        conflict.places_text(|folder| style::path(paths, folder))
    )
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
    // A new environment gets desktop entries like a Heroic import's, except
    // in a portable root. An attached one keeps its own setting.
    let desktop = paths.portable_root().is_none();
    if let Some(plan) = migration.native_plan_mut() {
        plan.set_desktop_enabled(desktop);
    }
    if let Some(plan) = migration.game_plan_mut() {
        plan.set_desktop_enabled(desktop);
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

fn review_lines(paths: &Paths, migration: &Migration, entry: &HeroicGogEntry) -> Vec<String> {
    let show = |path: &Path| style::path(paths, path);
    let game = migration.game();
    let mut lines = vec![format!("Migrate {} ({})", game.title, game.id)];
    lines.extend(
        migration
            .review_rows(migrate::heroic_note(entry, show), show)
            .into_iter()
            .map(|(label, value)| format!("  {:<13}{value}", review_label(label))),
    );
    lines
}

fn review_label(label: ReviewLabel) -> &'static str {
    match label {
        ReviewLabel::Environment => "environment",
        ReviewLabel::Folder => "folder",
        ReviewLabel::Heroic => "heroic",
        ReviewLabel::Track => "track",
        ReviewLabel::Version => "version",
        ReviewLabel::Language => "language",
        ReviewLabel::Dlc => "dlc",
        // One DLC per line; the rest sit under the first one's value.
        ReviewLabel::MoreDlc => "",
        ReviewLabel::Runner => "runner",
        ReviewLabel::Prefix => "prefix",
        ReviewLabel::FolderState => "folder state",
    }
}

fn print_review(paths: &Paths, migration: &Migration, entry: &HeroicGogEntry) {
    for line in review_lines(paths, migration, entry) {
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
    extras: &mut super::extras::Batch<'_>,
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
            }
            // Before the desktop entries, so a new icon is in them.
            let game = migration.game();
            let icon = game.id.parse().is_ok_and(|gog_id| {
                extras.import(paths, global, &migrated.environment, gog_id, &game.title)
            });
            match &migrated.registered {
                Some(registered) => finish_registered(paths, global, registered)?,
                None if icon => {
                    if let Err(error) =
                        crate::commands::art::apply_desktop(paths, &migrated.environment)
                    {
                        warn(format!("Couldn't update desktop entries: {error:#}"));
                    }
                }
                None => {}
            }
            println!(
                "Migrated {} as {}.",
                migration.game().title,
                migrated.environment
            );
            if matches!(migrated.state, StateChange::Reclaim { journal: true }) {
                let journal = journal_for(paths, &migrated.environment, None);
                println!("{}", finish_hint(journal.as_ref(), &migrated.environment));
            }
            println!("{HEROIC_NOTE}");
            Ok(true)
        }
    }
}

/// How the interrupted work a reclaimed folder came back with is finished,
/// by its journal: a Galaxy first install only by installing again.
fn finish_hint(journal: Option<&Journal>, name: &str) -> String {
    match journal {
        Some(journal) if journal.galaxy_install().is_some() => format!(
            "Finish the interrupted install with: {}",
            finish_command(journal, name)
        ),
        _ => format!("Finish the interrupted update with: wineo update {name}"),
    }
}

/// The question for a game named on the command line, where there is no
/// earlier question to go back to.
fn confirm(prompt: &mut Ask) -> anyhow::Result<()> {
    prompt.confirm_hint(false)?;
    match prompt.confirm("Migrate it?", true)? {
        Reply::Value(true) => Ok(()),
        _ => Err(Canceled.into()),
    }
}

pub(super) fn one(paths: &Paths, args: &Args, id: &str, verbose: bool) -> anyhow::Result<ExitCode> {
    host_only(paths)?;
    let global = GlobalConfig::load(paths)?;
    let heroic = Heroic::load(paths)?;
    let Some(entry) = heroic.find(paths, id)? else {
        return super::install::one(paths, args, id);
    };
    if let Some(code) = refuse_install_options(args, &entry.game.title) {
        return Ok(code);
    }
    let source = entry.source;
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
        anyhow::bail!(NEEDS_YES);
    }
    let mut prompt = if asking { Some(terminal()?) } else { None };
    prepare(paths, &global, &mut migration, prompt.as_mut(), verbose)?;
    print_review(paths, &migration, &entry);
    if args.dry_run {
        println!("Dry run, nothing was migrated.");
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(prompt) = prompt.as_mut() {
        confirm(prompt)?;
    }
    let outcome = apply(paths, &global, source, &migration)?;
    let account = Account::new();
    report(
        paths,
        &global,
        &migration,
        outcome,
        &mut Batch::new(&account),
    )?;
    Ok(ExitCode::SUCCESS)
}

pub(super) fn all(paths: &Paths, args: &Args, verbose: bool) -> anyhow::Result<ExitCode> {
    host_only(paths)?;
    let asking = !args.yes && !args.dry_run;
    if asking && !prompt::interactive() {
        anyhow::bail!(NEEDS_YES);
    }
    let global = GlobalConfig::load(paths)?;
    let heroic = Heroic::load(paths)?;
    let (games, unreadable) = heroic.games(paths);
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

    let mut ready: Vec<(HeroicGogEntry, Migration)> = Vec::new();
    let mut already: Vec<String> = Vec::new();
    let mut names = HashSet::new();
    for entry in games {
        let planned =
            plan_one(paths, entry.source, &entry.game.id).and_then(|planned| match planned {
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
                print_review(paths, &migration, &entry);
                ready.push((entry, *migration));
            }
            Err(error) if super::canceled(&error) => return Err(error),
            Err(error) => failures.push((entry.game.title, format!("{error:#}"))),
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
            let noun = text::plural(total, "game", "games");
            match prompt.confirm(&format!("Migrate {total} {noun}?"), true)? {
                Reply::Value(true) => {}
                _ => return Err(Canceled.into()),
            }
        }
        // One login session and one set of HTTP clients for every game's extras.
        let account = Account::new();
        let mut extras = Batch::new(&account);
        for (entry, migration) in &ready {
            match apply(paths, &global, entry.source, migration)
                .and_then(|outcome| report(paths, &global, migration, outcome, &mut extras))
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
        println!("{}", text::games_summary("Migrated", done, total));
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Game,
    Confirm,
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
    let (games, unreadable) = heroic.games(paths);
    for entry in unreadable {
        warn(format!("{}: {}", entry.subject, entry.message));
    }
    let mut candidates: Vec<(HeroicGogEntry, Migration)> = Vec::new();
    for entry in games {
        match plan_one(paths, entry.source, &entry.game.id) {
            Ok(Planned::Ready(migration)) => candidates.push((entry, *migration)),
            Ok(Planned::Already { .. }) => {}
            Err(error) => warn(format!("{}: {error:#}", entry.game.title)),
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
    let mut step = Step::Game;
    let mut picked = 0;
    loop {
        prompt.set_first_question(step == Step::Game && first_question);
        match step {
            Step::Game => match prompt.choose("Which GOG game?", "Game", &choices, None, true)? {
                Reply::Value(index) => {
                    picked = index;
                    step = Step::Confirm;
                }
                Reply::Back if first_question => return Err(Canceled.into()),
                Reply::Back => return Err(ReturnToSourceMenu.into()),
            },
            Step::Confirm => {
                let (entry, planned) = &candidates[picked];
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
                    step = Step::Game;
                    continue;
                }
                for line in review_lines(paths, &migration, entry) {
                    prompt.say(line)?;
                }
                if args.dry_run {
                    prompt.say("Dry run, nothing was migrated.")?;
                    return Ok(ExitCode::SUCCESS);
                }
                let answer = if args.yes {
                    Reply::Value(true)
                } else {
                    prompt.confirm_hint(true)?;
                    prompt.confirm("Migrate it?", true)?
                };
                match answer {
                    Reply::Back => step = Step::Game,
                    Reply::Value(false) => return Err(Canceled.into()),
                    Reply::Value(true) => {
                        let outcome = apply(paths, &global, entry.source, &migration)?;
                        let account = Account::new();
                        report(
                            paths,
                            &global,
                            &migration,
                            outcome,
                            &mut Batch::new(&account),
                        )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use wineo_core::gog::folder::{JournalKind, Target, Track};

    fn journal(kind: JournalKind, track: Track) -> Journal {
        Journal {
            operation: "op".to_owned(),
            kind,
            target: Target {
                gog_id: 1207658924,
                platform: Platform::Windows,
                language: "en-US".to_owned(),
                track,
                products: Vec::new(),
            },
            packages: Vec::new(),
            attempted: Vec::new(),
        }
    }

    #[test]
    fn a_reclaimed_galaxy_install_is_finished_by_installing_again() {
        let galaxy = journal(JournalKind::Install, Track::Galaxy);
        assert_eq!(
            finish_hint(Some(&galaxy), "papers-please"),
            "Finish the interrupted install with: wineo import gog --game 1207658924"
        );
    }

    #[test]
    fn any_other_reclaimed_journal_is_finished_by_updating() {
        for journal in [
            journal(JournalKind::Update, Track::Galaxy),
            journal(JournalKind::Install, Track::Installer),
        ] {
            assert_eq!(
                finish_hint(Some(&journal), "rimworld"),
                "Finish the interrupted update with: wineo update rimworld"
            );
        }
        assert_eq!(
            finish_hint(None, "rimworld"),
            "Finish the interrupted update with: wineo update rimworld"
        );
    }
}
