//! Applying updates: `wineo update NAME` for one game and `--all` for every
//! game with an update, with a review first, progress while it works, and
//! what changed after. A game on the installer track is rebuilt from its
//! packages; a Galaxy game is updated, repaired, or moved file by file.

use std::process::ExitCode;

use wineo_core::gog::account::{AccountError, Session};
use wineo_core::gog::folder::{FolderError, JournalKind, Platform, Track};
use wineo_core::gog::migrate::MigrateError;
use wineo_core::gog::update::galaxy::{self as core_galaxy, GalaxyError, PlannedUpdate};
use wineo_core::gog::update::windows::{Event, SetupError};
use wineo_core::gog::update::{
    self, Check, Options, Pending, Plan, ReviewLabel, UpdateError, Updated,
};
use wineo_core::paths::Paths;

use super::Args;
use crate::cancel;
use crate::commands::galaxy;
use crate::commands::gog_support::{
    Account, LOGGED_OUT, Reporter, ask_wizard, offers_wizard, usage_error, warn,
};
use crate::commands::setup::watched_message;
use crate::commands::table;
use crate::commands::transfer::Cancellation;
use crate::prompt::{self, Ask, Canceled, Reply, terminal};
use crate::style;
use crate::text;

const NEEDS_YES: &str = "Updating without a terminal needs --yes.";
const NAME_A_GAME: &str = "Name a game to update, or use --all.";
const NEEDS_DESKTOP: &str =
    "Updating a Windows game needs a graphical session. Run this from your desktop.";

/// Ctrl+C stopped an update after it had started changing the game folder,
/// which `wineo update` finishes.
#[derive(Debug)]
struct Interrupted {
    name: String,
    /// Whether it was finishing an interrupted install.
    install: bool,
}

impl std::fmt::Display for Interrupted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} was interrupted while updating", self.name)
    }
}

impl std::error::Error for Interrupted {}

/// An update ready to review and apply, on the track the game is on.
#[allow(clippy::large_enum_variant)]
enum Ready {
    Installer(Pending),
    Galaxy(core_galaxy::update::Plan),
}

impl Ready {
    fn environment(&self) -> &str {
        match self {
            Self::Installer(pending) => pending.environment(),
            Self::Galaxy(plan) => plan.environment(),
        }
    }

    fn kind(&self) -> JournalKind {
        match self {
            Self::Installer(pending) => pending.kind(),
            Self::Galaxy(plan) => plan.kind(),
        }
    }

    fn is_recovery(&self) -> bool {
        match self {
            Self::Installer(pending) => pending.is_recovery(),
            Self::Galaxy(plan) => plan.is_recovery(),
        }
    }

    fn warnings(&self) -> &[String] {
        match self {
            Self::Installer(pending) => pending.warnings(),
            Self::Galaxy(plan) => plan.warnings(),
        }
    }

    fn finished(&self) -> String {
        match self {
            Self::Installer(pending) => pending.finished(),
            Self::Galaxy(plan) => plan.finished(),
        }
    }
}

/// What planning an update found.
#[allow(clippy::large_enum_variant)]
enum Planned {
    UpToDate(Vec<String>),
    Ready(Ready),
}

pub(super) fn run(paths: &Paths, args: &Args) -> anyhow::Result<ExitCode> {
    if paths.portable_root().is_some() {
        return Err(MigrateError::Portable.into());
    }
    let result = if args.all {
        all(paths, args)
    } else if let Some(name) = &args.name {
        one(paths, args, name)
    } else {
        Ok(usage_error(NAME_A_GAME))
    };
    match result {
        Err(error) => match stopped(&error) {
            Some(Stop::Interrupted(name)) => {
                eprintln!("Update interrupted.");
                eprintln!("Finish the interrupted update with: wineo update {name}");
                Ok(cancel::exit_code())
            }
            Some(Stop::InstallInterrupted(name)) => {
                eprintln!("Install interrupted.");
                eprintln!("Finish the interrupted install with: wineo update {name}");
                Ok(cancel::exit_code())
            }
            Some(Stop::Canceled) => {
                eprintln!("Update canceled.");
                Ok(cancel::exit_code())
            }
            None => Err(error),
        },
        other => other,
    }
}

enum Stop {
    Canceled,
    Interrupted(String),
    InstallInterrupted(String),
}

/// Whether Ctrl+C or a refused question ended the command, and how far the
/// update had got.
fn stopped(error: &anyhow::Error) -> Option<Stop> {
    if let Some(Interrupted { name, install }) = error.downcast_ref() {
        return Some(if *install {
            Stop::InstallInterrupted(name.clone())
        } else {
            Stop::Interrupted(name.clone())
        });
    }
    let canceled = cancel::prompted(error)
        || matches!(
            error.downcast_ref::<UpdateError>(),
            Some(UpdateError::Canceled { .. })
        )
        || matches!(
            error.downcast_ref::<SetupError>(),
            Some(SetupError::Canceled { .. })
        )
        || matches!(
            error.downcast_ref::<FolderError>(),
            Some(FolderError::Canceled)
        )
        || cancel::core(error);
    canceled.then_some(Stop::Canceled)
}

/// Reads what updating `name` would do. Logged out, a game with a journal
/// and its packages still cached can finish; anything else needs the login.
fn plan(
    paths: &Paths,
    session: &mut Option<Session<'_>>,
    name: &str,
    args: &Args,
    wizard: bool,
    cancellation: &Cancellation,
) -> anyhow::Result<Planned> {
    let logged_out = session.is_none();
    let planned = core_galaxy::plan_update(
        paths,
        session.as_mut(),
        name,
        Options {
            reinstall: args.reinstall,
            repair: args.repair,
            track: args.track.map(Into::into),
            refresh: args.refresh,
            // A dry run caches nothing and records nothing.
            read_only: args.dry_run,
            wizard,
        },
        &cancellation.token,
    )
    .map_err(|error| {
        if logged_out
            && matches!(
                error.downcast_ref::<AccountError>(),
                Some(AccountError::LoggedOut)
            )
        {
            anyhow::anyhow!(LOGGED_OUT)
        } else if matches!(
            error.downcast_ref::<UpdateError>(),
            Some(UpdateError::NotGalaxy { .. })
        ) {
            anyhow::anyhow!(
                "Only Galaxy games can be repaired. Move it with: wineo update {name} --reinstall --track galaxy"
            )
        } else if matches!(error.downcast_ref::<GalaxyError>(), Some(GalaxyError::NoBuild { .. }))
            && !(args.reinstall && args.track.is_some())
        {
            // A Galaxy game GOG has no Galaxy build of is left to its
            // installers, as the check says.
            anyhow::anyhow!(
                "{}",
                update::Reason::Galaxy {
                    name: name.to_owned()
                }
                .note()
            )
        } else {
            error
        }
    })?;
    Ok(match planned {
        PlannedUpdate::Installer(Plan::UpToDate { warnings, .. })
        | PlannedUpdate::Galaxy(core_galaxy::update::Planned::UpToDate { warnings, .. }) => {
            Planned::UpToDate(warnings)
        }
        PlannedUpdate::Installer(Plan::Update(pending)) => {
            Planned::Ready(Ready::Installer(pending))
        }
        PlannedUpdate::Galaxy(core_galaxy::update::Planned::Update(plan)) => {
            Planned::Ready(Ready::Galaxy(plan))
        }
    })
}

fn one(paths: &Paths, args: &Args, name: &str) -> anyhow::Result<ExitCode> {
    let account = Account::new();
    let mut session = account.open(paths)?;
    let planned = plan(
        paths,
        &mut session,
        name,
        args,
        false,
        &Cancellation::new()?,
    )?;
    let ready = match planned {
        Planned::UpToDate(warnings) => {
            for warning in &warnings {
                warn(warning);
            }
            println!("Up to date.");
            return Ok(ExitCode::SUCCESS);
        }
        Planned::Ready(ready) => ready,
    };
    let asking = !args.yes && !args.dry_run;
    if asking && !prompt::interactive() {
        anyhow::bail!(NEEDS_YES);
    }
    print_review(paths, &ready);
    if args.dry_run {
        println!("Dry run, nothing was updated.");
        return Ok(ExitCode::SUCCESS);
    }
    if asking {
        confirm(&mut terminal()?, &ready)?;
    }
    let cancellation = Cancellation::new()?;
    let (ready, updated) = match apply_raw(paths, &mut session, &ready, &cancellation) {
        Ok(updated) => (ready, updated),
        Err(error) => {
            let Some(label) = offers_wizard(&error).filter(|_| asking) else {
                return Err(stopped_as(error, &ready));
            };
            if !ask_wizard(&mut terminal()?, &label)? {
                return Err(stopped_as(error, &ready));
            }
            drop(cancellation);
            // The first attempt dropped its journal, or kept it to finish,
            // so the update is planned again before it runs as a wizard.
            let cancellation = Cancellation::new()?;
            let again = match plan(paths, &mut session, name, args, true, &cancellation)? {
                Planned::Ready(ready) => ready,
                Planned::UpToDate(_) => {
                    println!("Up to date.");
                    return Ok(ExitCode::SUCCESS);
                }
            };
            let updated = apply(paths, &mut session, &again, &cancellation)?;
            (again, updated)
        }
    };
    report(paths, &ready, &updated);
    Ok(ExitCode::SUCCESS)
}

fn all(paths: &Paths, args: &Args) -> anyhow::Result<ExitCode> {
    let asking = !args.yes && !args.dry_run;
    let account = Account::new();
    let mut session = account.open(paths)?;
    let batch = match session.as_mut() {
        Some(session) => update::check_all_with(
            session,
            Check {
                refresh: args.refresh,
                read_only: args.dry_run,
            },
        )?,
        None => anyhow::bail!(LOGGED_OUT),
    };

    // A game that couldn't even be checked is a failure like one that can't
    // be planned.
    let mut failures: Vec<(String, String)> = batch
        .errors
        .iter()
        .map(|error| (error.subject(), error.message.clone()))
        .collect();
    let mut names = Vec::new();
    let mut skipped = Vec::new();
    let graphical = wineo_core::launch::graphical_session();
    for status in &batch.statuses {
        // A Galaxy install that didn't finish is finished by installing it
        // again, which the update has no way to do.
        if status.galaxy_install {
            skipped.push(format!(
                "Skipped {}: Finish the interrupted install with: wineo import gog --game {}",
                status.environment, status.gog_id
            ));
            continue;
        }
        // A game with a journal is finished whatever GOG offers now.
        let finishes = status.unfinished;
        if (status.applicable || finishes) && status.platform == Platform::Windows && !graphical {
            skipped.push(format!("Skipped {}: {NEEDS_DESKTOP}", status.environment));
        } else if status.applicable || finishes {
            names.push(status.environment.clone());
        } else if let Some(reason) = &status.reason {
            skipped.push(format!("Skipped {}: {}", status.environment, reason.note()));
        }
    }

    let planning = Cancellation::new()?;
    let mut ready: Vec<Ready> = Vec::new();
    for name in &names {
        if planning.token.is_canceled() {
            return Err(Canceled.into());
        }
        match plan(paths, &mut session, name, args, false, &planning) {
            Ok(Planned::Ready(item)) => ready.push(item),
            Ok(Planned::UpToDate(_)) => {}
            Err(error) if stopped(&error).is_some() => return Err(error),
            Err(error) => failures.push((name.clone(), format!("{error:#}"))),
        }
    }
    drop(planning);

    // Only something to confirm needs a terminal, so a library with nothing
    // to update is fine without one.
    if asking && !ready.is_empty() && !prompt::interactive() {
        anyhow::bail!(NEEDS_YES);
    }
    for line in &skipped {
        println!("{line}");
    }
    // A game being updated says its warnings in its review.
    for status in &batch.statuses {
        if !ready
            .iter()
            .any(|item| item.environment() == status.environment)
        {
            for warning in &status.warnings {
                warn(format!("{}: {warning}", status.environment));
            }
        }
    }
    for item in &ready {
        print_review(paths, item);
    }
    let total = ready.len();
    if ready.is_empty() && failures.is_empty() {
        println!("Nothing to update.");
        return Ok(ExitCode::SUCCESS);
    }
    if args.dry_run {
        for (name, reason) in &failures {
            eprintln!("{name}: would fail: {reason}");
        }
        println!("Dry run, nothing was updated.");
        return Ok(failure_code(&failures));
    }
    if !ready.is_empty() && asking {
        let noun = text::plural(total, "game", "games");
        match terminal()?.confirm(&format!("Update {total} {noun}?"), true)? {
            Reply::Value(true) => {}
            _ => return Err(Canceled.into()),
        }
    }

    let cancellation = Cancellation::new()?;
    let mut done = 0;
    let mut stop = None;
    for item in &ready {
        if cancellation.token.is_canceled() {
            stop = Some(anyhow::Error::from(Canceled));
            break;
        }
        println!("Updating {}", item.environment());
        match apply(paths, &mut session, item, &cancellation) {
            Ok(updated) => {
                done += 1;
                report(paths, item, &updated);
            }
            Err(error) if stopped(&error).is_some() => {
                stop = Some(error);
                break;
            }
            Err(error) => failures.push((item.environment().to_owned(), format!("{error:#}"))),
        }
    }
    for (name, reason) in &failures {
        eprintln!("Failed {name}: {reason}");
    }
    if total > 0 {
        println!("{}", text::games_summary("Updated", done, total));
    }
    if let Some(error) = stop {
        return Err(error);
    }
    if !failures.is_empty() {
        let names: Vec<_> = failures.iter().map(|(name, _)| name.as_str()).collect();
        println!("{} failed: {}", failures.len(), names.join(", "));
    }
    Ok(failure_code(&failures))
}

fn failure_code(failures: &[(String, String)]) -> ExitCode {
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn confirm(prompt: &mut Ask, ready: &Ready) -> anyhow::Result<()> {
    let question = match (ready.kind(), ready.is_recovery()) {
        (_, true) => "Finish it?",
        (JournalKind::Reinstall, false) => "Reinstall it?",
        (JournalKind::Repair, false) => "Repair it?",
        _ => "Update it?",
    };
    prompt.confirm_hint(false)?;
    match prompt.confirm(question, true)? {
        Reply::Value(true) => Ok(()),
        _ => Err(Canceled.into()),
    }
}

fn review_lines(paths: &Paths, ready: &Ready) -> Vec<String> {
    let pending = match ready {
        Ready::Installer(pending) => pending,
        Ready::Galaxy(plan) => return galaxy::update_review(paths, plan),
    };
    let mut rows: Vec<(String, String)> = pending
        .review_rows(|path| style::path(paths, path))
        .into_iter()
        .map(|(label, value)| {
            let label = match label {
                ReviewLabel::Folder => "folder".to_owned(),
                ReviewLabel::Product(label) => label,
                ReviewLabel::Owner => "owner".to_owned(),
                ReviewLabel::Download => "download".to_owned(),
                ReviewLabel::Unpacked => "unpacked".to_owned(),
            };
            (label, value)
        })
        .collect();
    // A game moving off Galaxy says so, right after the folder.
    if let Some(from) = pending.moves_from() {
        rows.insert(
            1,
            (
                "track".to_owned(),
                format!(
                    "{} -> {}",
                    track_word(from),
                    track_word(pending.target().track)
                ),
            ),
        );
    }
    let mut lines = vec![format!("{} {}", pending.action(), pending.environment())];
    lines.extend(table::aligned_rows(rows));
    lines
}

fn track_word(track: Track) -> &'static str {
    match track {
        Track::Galaxy => "Galaxy",
        Track::Installer => "installer",
    }
}

fn print_review(paths: &Paths, ready: &Ready) {
    for line in review_lines(paths, ready) {
        println!("{line}");
    }
    for warning in ready.warnings() {
        warn(warning);
    }
}

/// Runs the update, telling Ctrl+C after the first change apart from the
/// rest, since that one leaves something to finish.
fn apply(
    paths: &Paths,
    session: &mut Option<Session<'_>>,
    ready: &Ready,
    cancellation: &Cancellation,
) -> anyhow::Result<Updated> {
    apply_raw(paths, session, ready, cancellation).map_err(|error| stopped_as(error, ready))
}

/// `setup_stopped`, naming an interrupted install as one.
fn stopped_as(error: anyhow::Error, ready: &Ready) -> anyhow::Error {
    let mut error = setup_stopped(error, ready.environment(), ready.is_recovery());
    if ready.kind() == JournalKind::Install
        && let Some(interrupted) = error.downcast_mut::<Interrupted>()
    {
        interrupted.install = true;
    }
    error
}

/// The update with core's own errors, so a Windows installer that never
/// started can still be told apart.
fn apply_raw(
    paths: &Paths,
    session: &mut Option<Session<'_>>,
    ready: &Ready,
    cancellation: &Cancellation,
) -> anyhow::Result<Updated> {
    match ready {
        Ready::Installer(pending) => {
            let mut progress = Reporter::new(pending.product_labels());
            update::apply(
                paths,
                session.as_mut(),
                pending,
                &cancellation.token,
                |step| {
                    progress.show(&step);
                    #[cfg(debug_assertions)]
                    crate::commands::gog_support::pause_for_test(
                        pending.environment(),
                        &step,
                        &cancellation.token,
                    );
                },
            )
        }
        Ready::Galaxy(plan) => {
            let session = session
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!(LOGGED_OUT))?;
            let mut progress = galaxy::Reporter::new(std::time::Duration::from_secs(1));
            core_galaxy::update::apply_with_events(
                paths,
                session,
                plan,
                &cancellation.token,
                |step| {
                    if let Some(line) = progress.line(&step) {
                        eprintln!("{line}");
                    }
                    #[cfg(debug_assertions)]
                    galaxy::pause_for_test(plan.environment(), &step, &cancellation.token);
                },
                |event| {
                    if let Event::Watched(watched) = event
                        && let Some(line) = watched_message(watched)
                    {
                        eprintln!("{line}");
                    }
                },
            )
        }
    }
}

/// Says how an update stopped when it had started changing the folder, and
/// how a Windows installer that stopped on its own left things. An
/// installer that never started on a first attempt changed nothing, since
/// the update drops its journal then; finishing one keeps it. A Galaxy
/// setup step that failed leaves the journal for `wineo update` to finish.
fn setup_stopped(error: anyhow::Error, name: &str, recovery: bool) -> anyhow::Error {
    let interrupted = || {
        Interrupted {
            name: name.to_owned(),
            install: false,
        }
        .into()
    };
    if let Some(UpdateError::Canceled { journal: true }) = error.downcast_ref() {
        return interrupted();
    }
    if let Some(GalaxyError::Setup {
        title,
        label,
        detail,
    }) = error.downcast_ref()
    {
        return anyhow::anyhow!(
            "Setting up {title} stopped: {label} failed ({detail}). Finish it with: wineo update {name}"
        );
    }
    match error.downcast_ref::<SetupError>() {
        Some(SetupError::Canceled { .. }) if recovery => interrupted(),
        Some(SetupError::NotStarted { label, code, .. }) => {
            if recovery {
                anyhow::anyhow!(
                    "{label}'s installer never started (exit code {code}). Finish the update with: wineo update {name}"
                )
            } else {
                anyhow::anyhow!(
                    "{label}'s installer never started (exit code {code}), so nothing changed."
                )
            }
        }
        _ => error,
    }
}

/// Says what an update changed, and the files of yours it touched.
fn report(paths: &Paths, ready: &Ready, updated: &Updated) {
    println!("{}.", ready.finished());
    let rows = match ready {
        Ready::Installer(pending) => pending.changes(updated),
        Ready::Galaxy(plan) => galaxy::changes(paths, plan, updated),
    };
    for line in table::aligned_rows(rows) {
        println!("{line}");
    }
    if updated.claimed {
        println!(
            "{} now owns {}.",
            updated.environment,
            style::path(paths, &updated.folder)
        );
    }
    match ready {
        Ready::Galaxy(plan) => {
            for line in galaxy::saved_lines(plan, &updated.saved) {
                println!("{line}");
            }
        }
        Ready::Installer(_) => {
            if !updated.saved.is_empty() {
                println!("Moved your files aside where the new version has files:");
                for saved in &updated.saved {
                    println!("  {} -> {}", saved.path, saved.saved_as);
                }
            }
        }
    }
    if !updated.kept.is_empty() {
        println!("Kept these old files you changed, which the new version no longer ships:");
        for path in &updated.kept {
            println!("  {path}");
        }
    }
    if !updated.overwritten.is_empty() {
        println!("Overwrote these files that differed from the new version:");
        for path in &updated.overwritten {
            println!("  {path}");
        }
    }
    for line in galaxy::restart_lines(&updated.needs_restart) {
        println!("{line}");
    }
    // The plan's warnings came first, and the review already showed them.
    for warning in updated.warnings.iter().skip(ready.warnings().len()) {
        warn(warning);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn not_started() -> anyhow::Error {
        SetupError::NotStarted {
            product: 1,
            label: "Disco".to_owned(),
            code: 3,
            wizard: true,
        }
        .into()
    }

    fn canceled() -> anyhow::Error {
        SetupError::Canceled {
            product: 1,
            label: "Disco".to_owned(),
        }
        .into()
    }

    #[test]
    fn an_installer_that_never_started_is_one_line() {
        assert_eq!(
            setup_stopped(not_started(), "disco", false).to_string(),
            "Disco's installer never started (exit code 3), so nothing changed."
        );
        assert_eq!(
            setup_stopped(not_started(), "disco", true).to_string(),
            "Disco's installer never started (exit code 3). Finish the update with: wineo update disco"
        );
    }

    #[test]
    fn an_installer_canceled_is_a_cancel_or_an_interruption() {
        assert!(matches!(
            stopped(&setup_stopped(canceled(), "disco", false)),
            Some(Stop::Canceled)
        ));
        assert!(matches!(
            stopped(&setup_stopped(canceled(), "disco", true)),
            Some(Stop::Interrupted(name)) if name == "disco"
        ));
    }
}
