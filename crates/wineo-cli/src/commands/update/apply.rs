//! Applying updates: `wineo update NAME` for one game and `--all` for every
//! native game with an update, with a review first, progress while it works,
//! and what changed after.

use std::process::ExitCode;

use wineo_core::gog::account::{AccountError, Session};
use wineo_core::gog::folder::{FolderError, JournalKind, Platform, Track};
use wineo_core::gog::migrate::MigrateError;
use wineo_core::gog::update::windows::SetupError;
use wineo_core::gog::update::{
    self, Check, Options, Pending, Plan, ReviewLabel, UpdateError, Updated,
};
use wineo_core::paths::Paths;

use super::Args;
use crate::cancel;
use crate::commands::gog_support::{
    Account, LOGGED_OUT, Reporter, ask_wizard, offers_wizard, usage_error, warn,
};
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
) -> anyhow::Result<Plan> {
    let logged_out = session.is_none();
    update::plan(
        paths,
        session.as_mut(),
        name,
        Options {
            reinstall: args.reinstall,
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
        } else {
            error
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
    let pending = match planned {
        Plan::UpToDate { warnings, .. } => {
            for warning in &warnings {
                warn(warning);
            }
            println!("Up to date.");
            return Ok(ExitCode::SUCCESS);
        }
        Plan::Update(pending) => pending,
    };
    let asking = !args.yes && !args.dry_run;
    if asking && !prompt::interactive() {
        anyhow::bail!(NEEDS_YES);
    }
    print_review(paths, &pending);
    if args.dry_run {
        println!("Dry run, nothing was updated.");
        return Ok(ExitCode::SUCCESS);
    }
    if asking {
        confirm(&mut terminal()?, &pending)?;
    }
    let cancellation = Cancellation::new()?;
    let (pending, updated) = match apply_raw(paths, &mut session, &pending, &cancellation) {
        Ok(updated) => (pending, updated),
        Err(error) => {
            let Some(label) = offers_wizard(&error).filter(|_| asking) else {
                return Err(stopped_as(error, &pending));
            };
            if !ask_wizard(&mut terminal()?, &label)? {
                return Err(stopped_as(error, &pending));
            }
            drop(cancellation);
            // The first attempt dropped its journal, or kept it to finish,
            // so the update is planned again before it runs as a wizard.
            let cancellation = Cancellation::new()?;
            let again = match plan(paths, &mut session, name, args, true, &cancellation)? {
                Plan::Update(pending) => pending,
                Plan::UpToDate { .. } => {
                    println!("Up to date.");
                    return Ok(ExitCode::SUCCESS);
                }
            };
            let updated = apply(paths, &mut session, &again, &cancellation)?;
            (again, updated)
        }
    };
    report(paths, &pending, &updated);
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
        // A game with a journal is finished whatever GOG offers now.
        let finishes = status.unfinished && status.track == Track::Installer;
        if (status.applicable || finishes) && status.platform == Platform::Windows && !graphical {
            skipped.push(format!("Skipped {}: {NEEDS_DESKTOP}", status.environment));
        } else if status.applicable || finishes {
            names.push(status.environment.clone());
        } else if let Some(reason) = &status.reason {
            skipped.push(format!("Skipped {}: {}", status.environment, reason.note()));
        }
    }

    let planning = Cancellation::new()?;
    let mut ready: Vec<Pending> = Vec::new();
    for name in &names {
        if planning.token.is_canceled() {
            return Err(Canceled.into());
        }
        match plan(paths, &mut session, name, args, false, &planning) {
            Ok(Plan::Update(pending)) => ready.push(pending),
            Ok(Plan::UpToDate { .. }) => {}
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
            .any(|pending| pending.environment() == status.environment)
        {
            for warning in &status.warnings {
                warn(format!("{}: {warning}", status.environment));
            }
        }
    }
    for pending in &ready {
        print_review(paths, pending);
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
    for pending in &ready {
        if cancellation.token.is_canceled() {
            stop = Some(anyhow::Error::from(Canceled));
            break;
        }
        println!("Updating {}", pending.environment());
        match apply(paths, &mut session, pending, &cancellation) {
            Ok(updated) => {
                done += 1;
                report(paths, pending, &updated);
            }
            Err(error) if stopped(&error).is_some() => {
                stop = Some(error);
                break;
            }
            Err(error) => failures.push((pending.environment().to_owned(), format!("{error:#}"))),
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

fn confirm(prompt: &mut Ask, pending: &Pending) -> anyhow::Result<()> {
    let question = match (pending.kind(), pending.is_recovery()) {
        (_, true) => "Finish it?",
        (JournalKind::Reinstall, false) => "Reinstall it?",
        _ => "Update it?",
    };
    prompt.confirm_hint(false)?;
    match prompt.confirm(question, true)? {
        Reply::Value(true) => Ok(()),
        _ => Err(Canceled.into()),
    }
}

fn review_lines(paths: &Paths, pending: &Pending) -> Vec<String> {
    let rows: Vec<(String, String)> = pending
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
    let mut lines = vec![format!("{} {}", pending.action(), pending.environment())];
    lines.extend(table::aligned_rows(rows));
    lines
}

fn print_review(paths: &Paths, pending: &Pending) {
    for line in review_lines(paths, pending) {
        println!("{line}");
    }
    for warning in pending.warnings() {
        warn(warning);
    }
}

/// Runs the update, telling Ctrl+C after the first change apart from the
/// rest, since that one leaves something to finish.
fn apply(
    paths: &Paths,
    session: &mut Option<Session<'_>>,
    pending: &Pending,
    cancellation: &Cancellation,
) -> anyhow::Result<Updated> {
    apply_raw(paths, session, pending, cancellation).map_err(|error| stopped_as(error, pending))
}

/// `setup_stopped`, naming an interrupted install as one.
fn stopped_as(error: anyhow::Error, pending: &Pending) -> anyhow::Error {
    let mut error = setup_stopped(error, pending.environment(), pending.is_recovery());
    if pending.kind() == JournalKind::Install
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
    pending: &Pending,
    cancellation: &Cancellation,
) -> anyhow::Result<Updated> {
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

/// Says how an update stopped when it had started changing the folder, and
/// how a Windows installer that stopped on its own left things. An
/// installer that never started on a first attempt changed nothing, since
/// the update drops its journal then; finishing one keeps it.
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
fn report(paths: &Paths, pending: &Pending, updated: &Updated) {
    println!("{}.", pending.finished());
    for line in table::aligned_rows(pending.changes(updated)) {
        println!("{line}");
    }
    if updated.claimed {
        println!(
            "{} now owns {}.",
            updated.environment,
            style::path(paths, &updated.folder)
        );
    }
    if !updated.saved.is_empty() {
        println!("Moved your files aside where the new version has files:");
        for saved in &updated.saved {
            println!("  {} -> {}", saved.path, saved.saved_as);
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
    // The plan's warnings came first, and the review already showed them.
    for warning in updated.warnings.iter().skip(pending.warnings().len()) {
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
