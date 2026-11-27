//! `wineo update` for Epic games: their rows in `--check`, updating and
//! repairing one game, and their part of `--all`. Legendary does the work;
//! wineo checks what it did, as an install does.

use std::process::ExitCode;

use serde_json::{Value, json};
use wineo_core::config::GlobalConfig;
use wineo_core::epic::game::Work;
use wineo_core::epic::legendary::Legendary;
use wineo_core::epic::owned::Owned;
use wineo_core::epic::update::{
    self as core_update, Batch, CheckError, DlcState, Plan, Planned, Status, Updated,
};
use wineo_core::paths::Paths;

use super::Args;
use crate::commands::gog_support::{update_label, warn};
use crate::commands::import_epic::install::{Progress, stall};
use crate::commands::import_epic::setup::{self, Found};
use crate::commands::import_epic::{self, list};
use crate::commands::table;
use crate::commands::transfer::Cancellation;
use crate::prompt::{self, Canceled, Reply, terminal};
use crate::style;

const NEEDS_YES: &str = "Updating without a terminal needs --yes.";

/// The GOG-only flag given, in the order they're named, for refusing it on
/// an Epic game.
pub(super) fn gog_flag(args: &Args) -> Option<&'static str> {
    [
        (args.reinstall, "--reinstall"),
        (args.track.is_some(), "--track"),
        (args.branch.is_some(), "--branch"),
        (args.default_branch, "--default-branch"),
        (args.build.is_some(), "--build"),
        (args.latest, "--latest"),
        (args.builds, "--builds"),
        (args.password_stdin, "--password-stdin"),
    ]
    .into_iter()
    .find_map(|(given, flag)| given.then_some(flag))
}

/// Whether `name` is an Epic game's environment, as far as its config says.
pub(super) fn is_epic(paths: &Paths, name: &str) -> bool {
    paths.portable_root().is_none()
        && (wineo_core::epic::game::epic_id_of(paths, name).is_some()
            || wineo_core::epic::game::names_epic_id(paths, name))
}

/// Whether any environment owns an Epic game, so `update` checks Epic too.
pub(super) fn has_games(paths: &Paths) -> anyhow::Result<bool> {
    Ok(!core_update::environments(paths)?.is_empty())
}

/// The owned list to check against, never asking to download Legendary:
/// Legendary's, else the saved one with a warning, else why there is none.
/// The warnings are the second part.
pub(super) fn offered(
    paths: &Paths,
    refresh: bool,
) -> anyhow::Result<(Result<Owned, String>, Vec<String>)> {
    let global = GlobalConfig::load(paths)?;
    let mut warnings = Vec::new();
    let found = match setup::find(paths, &global) {
        Ok(found) => found,
        Err(error) if stopped(&error) => return Err(error),
        Err(error) => return Ok((Err(format!("{error:#}")), warnings)),
    };
    let owned = match found {
        Found::Usable(legendary) => match list::load(&legendary, refresh) {
            Ok(owned) => Ok(owned),
            Err(error) if stopped(&error) => return Err(error),
            Err(error) => Err(format!("{error:#}")),
        },
        Found::Unusable { missing } => match wineo_core::epic::owned::cached(paths) {
            Some(owned) => {
                warnings.push(list::saved_list_warning(missing.as_ref()));
                Ok(owned)
            }
            None => Err(list::not_listed(missing.as_ref()).to_string()),
        },
    };
    if let Ok(owned) = &owned {
        warnings.extend(owned.warning.iter().cloned());
    }
    Ok((owned, warnings))
}

/// Every Epic game's status against `offered`.
pub(super) fn check_all(paths: &Paths, offered: &Result<Owned, String>) -> anyhow::Result<Batch> {
    core_update::check_all(paths, offered.as_ref().map_err(String::as_str))
}

/// `wineo update --check NAME` for an Epic game. A failure is the error.
pub(super) fn check_one(paths: &Paths, args: &Args, name: &str) -> anyhow::Result<ExitCode> {
    let (offered, warnings) = offered(paths, args.refresh)?;
    let status = core_update::check(paths, name, offered.as_ref().map_err(String::as_str))?;
    for warning in &warnings {
        warn(warning);
    }
    if args.json {
        crate::commands::json::print(&json!({
            "environments": [status_json(&status)],
            "errors": [],
            "warnings": warnings,
        }))?;
    } else {
        let mut rows = vec![["NAME", "INSTALLED", "AVAILABLE", "STATE"].map(str::to_owned)];
        rows.push(table_row(&status));
        for line in table::render_columns(&rows) {
            println!("{line}");
        }
        for note in notes(&status) {
            println!("{note}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// An Epic row of `update --check --json`: GOG's fields, with GOG's own
/// track, branch, and pin null.
pub(super) fn status_json(status: &Status) -> Value {
    json!({
        "name": status.environment,
        "id": status.app_name,
        "store": "epic",
        "track": Value::Null,
        "installed": status.installed,
        "available": status.available,
        "branch": Value::Null,
        "pinned": Value::Null,
        "update": status.state.as_str(),
        "applicable": status.applicable,
        "reason": status.reason,
        "unfinished": status.unfinished.is_some(),
        "dlc": status.dlc.iter().map(|dlc| json!({
            "id": dlc.app_name,
            "title": dlc.title,
            "state": dlc.state.as_str(),
        })).collect::<Vec<_>>(),
    })
}

pub(super) fn error_json(error: &CheckError) -> Value {
    json!({
        "name": error.environment,
        "id": error.id,
        "message": error.message,
    })
}

/// A table row, as GOG's: a version not known reads as `-`.
pub(super) fn table_row(status: &Status) -> [String; 4] {
    let shown = |version: &Option<String>| version.clone().unwrap_or_else(|| "-".to_owned());
    [
        status.environment.clone(),
        shown(&status.installed),
        shown(&status.available),
        update_label(status.state).to_owned(),
    ]
}

/// The lines under a game's row: why it can't be compared, what finishes
/// work left on it, and its DLC that aren't up to date.
pub(super) fn notes(status: &Status) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(reason) = &status.reason {
        notes.push(format!("  {reason}"));
    }
    if let (Some(work), Some(command)) = (status.unfinished, status.finish_command()) {
        notes.push(format!(
            "  Finish the interrupted {} with: {command}",
            work_noun(work)
        ));
    }
    for dlc in &status.dlc {
        let what = match dlc.state {
            DlcState::UpToDate | DlcState::Unknown => continue,
            DlcState::Available => format!(
                "{} -> {}",
                dlc.installed.as_deref().unwrap_or("?"),
                dlc.available.as_deref().unwrap_or("?")
            ),
            DlcState::CantCompare => "can't compare".to_owned(),
            DlcState::Failed => "failed".to_owned(),
            DlcState::Unavailable => "unavailable".to_owned(),
            DlcState::NotInstalled => "not installed".to_owned(),
        };
        let label = dlc.title.as_deref().unwrap_or(&dlc.app_name);
        notes.push(format!("  DLC {label}: {what}"));
    }
    notes
}

fn work_noun(work: Work) -> &'static str {
    match work {
        Work::Install => "install",
        Work::Update => "update",
        Work::Repair | Work::Unknown => "repair",
    }
}

/// Ctrl+C stopped an Epic update or repair after its mark was written, so
/// `wineo update` finishes it.
#[derive(Debug)]
pub(super) struct Interrupted {
    pub name: String,
    pub work: Work,
}

impl std::fmt::Display for Interrupted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} was interrupted while {}",
            self.name,
            if self.work == Work::Repair {
                "repairing"
            } else {
                "updating"
            }
        )
    }
}

impl std::error::Error for Interrupted {}

impl Interrupted {
    /// What the command says as it exits with the cancel's code.
    pub fn say(&self) {
        let noun = work_noun(self.work);
        let mut word = noun.to_owned();
        word[..1].make_ascii_uppercase();
        eprintln!("{word} interrupted.");
        eprintln!(
            "Finish the interrupted {noun} with: {}",
            core_update::finish_command(&self.name, "", self.work)
        );
    }
}

/// Whether `error` is a cancel: Ctrl+C, or a question answered no.
pub(super) fn stopped(error: &anyhow::Error) -> bool {
    crate::cancel::prompted(error)
        || crate::cancel::core(error)
        || core_update::is_cancel(error)
        || error
            .downcast_ref::<wineo_core::epic::Error>()
            .is_some_and(wineo_core::epic::Error::is_canceled)
        || error.downcast_ref::<Interrupted>().is_some()
}

/// The import flags `setup` reads, for finding Legendary from `update`.
fn import_args(args: &Args) -> import_epic::Args {
    import_epic::Args {
        yes: args.yes,
        dry_run: args.dry_run,
        refresh: args.refresh,
        ..import_epic::Args::default()
    }
}

/// The owned list a dry run uses: the saved one, since a dry run fetches
/// nothing. None after saying why there's nothing to show.
fn dry_run_owned(paths: &Paths, args: &Args) -> anyhow::Result<Option<Owned>> {
    let global = GlobalConfig::load(paths)?;
    if let Found::Unusable {
        missing: Some(missing),
    } = setup::find(paths, &global)?
    {
        setup::check_can_download()?;
        setup::say_dry_run(&missing);
        return Ok(None);
    }
    let mut warnings = Vec::new();
    let owned = list::dry_run(paths, &global, &import_args(args), &mut warnings)?;
    for warning in &warnings {
        warn(warning);
    }
    Ok(owned)
}

/// `wineo update NAME` for an Epic game: an update, or with `--repair` a
/// check of every file, either finishing work left on it.
pub(super) fn one(paths: &Paths, args: &Args, name: &str) -> anyhow::Result<ExitCode> {
    // wineo's own refusals come before Legendary is looked for.
    core_update::precheck(paths, name, args.repair)?;
    let (legendary, owned) = if args.dry_run {
        match dry_run_owned(paths, args)? {
            Some(owned) => (None, owned),
            None => return Ok(ExitCode::SUCCESS),
        }
    } else {
        let global = GlobalConfig::load(paths)?;
        let legendary =
            setup::obtain(paths, &global, &import_args(args))?.expect("only a dry run stops");
        let owned = list::load(&legendary, args.refresh)?;
        if let Some(warning) = &owned.warning {
            warn(warning);
        }
        (Some(legendary), owned)
    };
    let plan = match core_update::plan(paths, name, Some(&owned), args.repair)? {
        Planned::UpToDate => {
            println!("Up to date.");
            return Ok(ExitCode::SUCCESS);
        }
        Planned::Ready(plan) => plan,
    };
    let asking = !args.yes && !args.dry_run;
    if asking && !prompt::interactive() {
        anyhow::bail!(NEEDS_YES);
    }
    print_review(paths, &plan);
    if args.dry_run {
        println!("Dry run, nothing was updated.");
        return Ok(ExitCode::SUCCESS);
    }
    if asking {
        confirm(&plan)?;
    }
    let legendary = legendary.expect("a real run has Legendary");
    let updated = apply(paths, &legendary, &plan)?;
    report(paths, &plan, &updated);
    Ok(ExitCode::SUCCESS)
}

/// The Epic part of `update --all`: what to update, what was skipped and
/// why, what failed, and the Legendary to update with.
#[derive(Default)]
pub(super) struct All {
    pub ready: Vec<Plan>,
    pub skipped: Vec<String>,
    pub failures: Vec<(String, String)>,
    pub warnings: Vec<String>,
    legendary: Option<Box<Legendary>>,
}

/// Checks every Epic game and plans each one with an update or with work
/// left to finish. Asks to download Legendary as an Epic command does,
/// unless this is a dry run. A failure to get the owned list fails each
/// game, and the GOG games carry on.
pub(super) fn plan_all(paths: &Paths, args: &Args) -> anyhow::Result<All> {
    let mut all = All::default();
    let games = core_update::environments(paths)?;
    if games.is_empty() {
        return Ok(all);
    }
    let fail_all = |all: &mut All, message: String| {
        for (name, _) in &games {
            all.failures.push((name.clone(), message.clone()));
        }
    };
    let owned = if args.dry_run {
        match dry_run_owned(paths, args) {
            Ok(Some(owned)) => owned,
            Ok(None) => return Ok(all),
            Err(error) if stopped(&error) => return Err(error),
            Err(error) => {
                fail_all(&mut all, format!("{error:#}"));
                return Ok(all);
            }
        }
    } else {
        let global = GlobalConfig::load(paths)?;
        let got = setup::obtain(paths, &global, &import_args(args))
            .map(|legendary| legendary.expect("only a dry run stops"))
            .and_then(|legendary| {
                let owned = list::load(&legendary, args.refresh)?;
                Ok((legendary, owned))
            });
        match got {
            Ok((legendary, owned)) => {
                all.legendary = Some(legendary);
                owned
            }
            Err(error) if stopped(&error) => return Err(error),
            Err(error) => {
                fail_all(&mut all, format!("{error:#}"));
                return Ok(all);
            }
        }
    };
    all.warnings.extend(owned.warning.iter().cloned());
    let batch = core_update::check_all(paths, Ok(&owned))?;
    for error in batch.errors {
        all.failures.push((error.environment, error.message));
    }
    for status in &batch.statuses {
        let name = &status.environment;
        match status.unfinished {
            // An install that didn't finish is finished by installing again.
            Some(Work::Install) => {
                all.skipped.push(format!(
                    "Skipped {name}: Finish the interrupted install with: {}",
                    status.finish_command().unwrap_or_default()
                ));
                continue;
            }
            Some(_) => {}
            None if status.applicable => {}
            None => {
                if let Some(reason) = &status.reason {
                    all.skipped.push(format!("Skipped {name}: {reason}"));
                }
                continue;
            }
        }
        match core_update::plan(paths, name, Some(&owned), false) {
            Ok(Planned::Ready(plan)) => all.ready.push(plan),
            Ok(Planned::UpToDate) => {}
            Err(error) => all.failures.push((name.clone(), format!("{error:#}"))),
        }
    }
    Ok(all)
}

impl All {
    /// Updates one planned game, for `--all`.
    pub(super) fn apply(&self, paths: &Paths, plan: &Plan) -> anyhow::Result<Updated> {
        let legendary = self
            .legendary
            .as_deref()
            .expect("a real run has Legendary");
        apply(paths, legendary, plan)
    }
}

fn confirm(plan: &Plan) -> anyhow::Result<()> {
    let question = match (plan.work(), plan.resumes()) {
        (_, true) => "Finish it?",
        (Work::Repair, false) => "Repair it?",
        _ => "Update it?",
    };
    let mut prompt = terminal()?;
    prompt.confirm_hint(false)?;
    match prompt.confirm(question, true)? {
        Reply::Value(true) => Ok(()),
        _ => Err(Canceled.into()),
    }
}

fn action(plan: &Plan) -> &'static str {
    match (plan.work(), plan.resumes()) {
        (Work::Repair, false) => "Repair",
        (Work::Repair, true) => "Finish the interrupted repair of",
        (_, true) => "Finish the interrupted update of",
        (_, false) => "Update",
    }
}

fn finished(plan: &Plan) -> String {
    let name = plan.environment();
    match (plan.work(), plan.resumes()) {
        (Work::Repair, false) => format!("Repaired {name}."),
        (Work::Repair, true) => format!("Finished the interrupted repair of {name}."),
        (_, true) => format!("Finished the interrupted update of {name}."),
        (_, false) => format!("Updated {name}."),
    }
}

fn version(version: Option<&str>) -> &str {
    version.unwrap_or("unknown")
}

pub(super) fn review_lines(paths: &Paths, plan: &Plan) -> Vec<String> {
    let mut rows = vec![("folder".to_owned(), style::path(paths, plan.folder()))];
    if plan.rebuilds() {
        rows.push((
            "record".to_owned(),
            "missing, rebuilt from Legendary's".to_owned(),
        ));
    }
    let repair = plan.work() == Work::Repair;
    for run in &plan.runs()[plan.first()..] {
        let value = if repair {
            version(run.from.as_deref()).to_owned()
        } else if run.new {
            format!("not installed -> {}", version(run.to.as_deref()))
        } else {
            format!(
                "{} -> {}",
                version(run.from.as_deref()),
                version(run.to.as_deref())
            )
        };
        rows.push((run.title.clone(), value));
    }
    if repair {
        rows.push(("check".to_owned(), "every file".to_owned()));
        rows.push((
            "download".to_owned(),
            "only what's missing or damaged".to_owned(),
        ));
    }
    let mut lines = vec![format!("{} {}", action(plan), plan.environment())];
    lines.extend(table::aligned_rows(rows));
    lines
}

pub(super) fn print_review(paths: &Paths, plan: &Plan) {
    for line in review_lines(paths, plan) {
        println!("{line}");
    }
}

/// Runs the update or repair with progress, and says how a cancel left it.
fn apply(paths: &Paths, legendary: &Legendary, plan: &Plan) -> anyhow::Result<Updated> {
    let cancellation = Cancellation::new()?;
    let repair = plan.work() == Work::Repair;
    let mut progress = Progress::new(if repair { "Repairing" } else { "Downloading" });
    let applied = core_update::apply(legendary, plan, stall(), &cancellation.token, |event| {
        progress.show(&event)
    });
    let left = core_update::left_unfinished(paths, plan.app_name());
    match applied {
        Ok(updated) => Ok(updated),
        Err(error) if stopped(&error) || cancellation.token.is_canceled() => match left {
            Some(work) => Err(Interrupted {
                name: plan.environment().to_owned(),
                work,
            }
            .into()),
            None => Err(Canceled.into()),
        },
        Err(error) => match left {
            Some(work) => {
                let command = core_update::finish_command(plan.environment(), "", work);
                let message = format!("{error:#}");
                if message.contains(&command) {
                    return Err(error);
                }
                let sep = if message.ends_with('.') { "" } else { "." };
                Err(anyhow::anyhow!("{message}{sep} Finish it with: {command}"))
            }
            None => Err(error),
        },
    }
}

/// Says what the update or repair did.
pub(super) fn report(paths: &Paths, plan: &Plan, updated: &Updated) {
    println!("{}", finished(plan));
    let repair = plan.work() == Work::Repair;
    let rows: Vec<(String, String)> = updated
        .changes
        .iter()
        .map(|change| {
            let value = if repair {
                version(change.to.as_deref()).to_owned()
            } else {
                format!(
                    "{} -> {}",
                    change
                        .from
                        .as_deref()
                        .unwrap_or(if change.dlc { "not installed" } else { "unknown" }),
                    version(change.to.as_deref())
                )
            };
            (change.title.clone(), value)
        })
        .collect();
    for line in table::aligned_rows(rows) {
        println!("{line}");
    }
    if updated.program_changed
        && let Ok(environment) = wineo_core::environment::Environment::load(paths, &updated.environment)
        && let Some(target) = environment.config.targets.get(wineo_core::epic::install::TARGET)
    {
        println!(
            "{}'s main program is now {}.",
            updated.environment,
            style::path(paths, std::path::Path::new(&target.exe))
        );
    }
    for warning in &updated.warnings {
        warn(warning);
    }
}

