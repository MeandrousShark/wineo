//! Applying updates: `wineo update NAME` for one game and `--all` for every
//! native game with an update, with a review first, progress while it works,
//! and what changed after.

use std::io;
use std::process::ExitCode;
use std::time::Duration;

use wineo_core::gog::account::{AccountError, Session};
use wineo_core::gog::folder::{FolderError, JournalKind, Platform, Track};
use wineo_core::gog::migrate::MigrateError;
use wineo_core::gog::update::windows::SetupError;
use wineo_core::gog::update::{
    self, Check, Options, Pending, Plan, Progress, UpdateError, Updated,
};
use wineo_core::paths::Paths;

use super::Args;
use crate::commands::gog_support::{
    Account, LOGGED_OUT, Reporter, ask_wizard, offers_wizard, reason_note, usage_error, warn,
};
use crate::commands::transfer::Cancellation;
use crate::prompt::{self, Canceled, Prompt, Reply, Terminal};
use crate::style;

type Ask = Prompt<Terminal, crate::output::Stream<io::Stdout>>;

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
                Ok(ExitCode::from(130))
            }
            Some(Stop::InstallInterrupted(name)) => {
                eprintln!("Install interrupted.");
                eprintln!("Finish the interrupted install with: wineo update {name}");
                Ok(ExitCode::from(130))
            }
            Some(Stop::Canceled) => {
                eprintln!("Update canceled.");
                Ok(ExitCode::from(130))
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
    let canceled = error.is::<Canceled>()
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
        || matches!(
            error.downcast_ref::<wineo_core::Error>(),
            Some(wineo_core::Error::Canceled(_))
        );
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
        .map(|error| {
            (
                crate::commands::gog_support::error_subject(error),
                error.message.clone(),
            )
        })
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
            skipped.push(format!(
                "Skipped {}: {}",
                status.environment,
                reason_note(reason)
            ));
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
        let noun = noun(total);
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
        println!("Updated {done} of {total} {}.", noun(total));
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

fn noun(count: usize) -> &'static str {
    if count == 1 { "game" } else { "games" }
}

fn failure_code(failures: &[(String, String)]) -> ExitCode {
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn terminal() -> anyhow::Result<Ask> {
    Ok(Prompt::new(Terminal::open()?, crate::output::stdout()).with_first_question(true))
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

/// How the review and the result name what is being done.
fn action(pending: &Pending) -> &'static str {
    match (pending.kind(), pending.is_recovery()) {
        (JournalKind::Reinstall, false) => "Reinstall",
        (JournalKind::Reinstall, true) => "Finish the interrupted reinstall of",
        (JournalKind::Install, true) => "Finish the interrupted install of",
        (_, true) => "Finish the interrupted update of",
        (_, false) => "Update",
    }
}

/// The base game's title, which a recovery from saved packages doesn't
/// have: then the game is named by its environment. A DLC is `NAME [ID]`.
fn product_label(pending: &Pending, id: u64, title: Option<&str>) -> String {
    if id != pending.target().gog_id {
        return pending.dlc_label(id);
    }
    title.map_or_else(|| pending.environment().to_owned(), str::to_owned)
}

/// `1.0 -> 1.1`, or the version alone when a rebuild installs the same one.
fn change(installed: Option<&str>, version: &str) -> String {
    match installed {
        Some(installed) if installed == version => format!("{version} (rebuilt)"),
        Some(installed) => format!("{installed} -> {version}"),
        None => format!("unknown -> {version}"),
    }
}

fn review_lines(paths: &Paths, pending: &Pending) -> Vec<String> {
    let mut rows = vec![("folder".to_owned(), style::path(paths, pending.folder()))];
    for product in pending.products() {
        let label = product_label(pending, product.id, product.title.as_deref());
        let value = match &product.version {
            Some(version) => change(product.installed.as_deref(), version),
            None => "left as it is".to_owned(),
        };
        rows.push((label, value));
    }
    if pending.claims() {
        rows.push((
            "owner".to_owned(),
            format!("none yet, {} takes this folder", pending.environment()),
        ));
    }
    if pending.is_offline() {
        rows.push((
            "download".to_owned(),
            "none, uses the packages already downloaded".to_owned(),
        ));
    } else {
        rows.push(("download".to_owned(), style::size(pending.download_size())));
    }
    rows.push((
        "unpacked".to_owned(),
        format!(
            "about {} (an estimate)",
            style::size(pending.unpacked_estimate())
        ),
    ));
    let width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines = vec![format!("{} {}", action(pending), pending.environment())];
    lines.extend(
        rows.into_iter()
            .map(|(label, value)| format!("  {label:<width$}  {value}")),
    );
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
    let mut progress = Reporter::new(
        pending
            .products()
            .iter()
            .map(|product| {
                (
                    product.id,
                    product_label(pending, product.id, product.title.as_deref()),
                )
            })
            .collect(),
    );
    update::apply(
        paths,
        session.as_mut(),
        pending,
        &cancellation.token,
        |step| {
            progress.show(&step);
            #[cfg(debug_assertions)]
            pause_for_test(pending.environment(), &step, &cancellation.token);
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

/// Lets a test stop an update at a known point: with
/// `WINEO_TEST_UPDATE_PAUSE=download` or `publish`, the first step of that
/// kind waits for Ctrl+C, and `publish:NAME` only for that environment.
/// Release builds never look at it.
#[cfg(debug_assertions)]
fn pause_for_test(environment: &str, step: &Progress, token: &wineo_core::cancel::Token) {
    let wanted = std::env::var("WINEO_TEST_UPDATE_PAUSE").unwrap_or_default();
    let (at, only) = wanted.split_once(':').unwrap_or((wanted.as_str(), ""));
    let here = matches!(
        (at, step),
        ("download", Progress::Download { .. }) | ("publish", Progress::Publish { .. })
    ) && (only.is_empty() || only == environment);
    while here && !token.is_canceled() {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Says what an update changed, and the files of yours it touched.
fn report(paths: &Paths, pending: &Pending, updated: &Updated) {
    println!("{}.", finished(pending));
    let rows: Vec<(String, String)> = updated
        .products
        .iter()
        .filter_map(|product| {
            let version = product.version.as_ref()?;
            let planned = pending.products().iter().find(|p| p.id == product.id);
            let label = product_label(
                pending,
                product.id,
                planned.and_then(|p| p.title.as_deref()),
            );
            let installed = planned.and_then(|p| p.installed.as_deref());
            Some((label, change(installed, version)))
        })
        .collect();
    let width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    for (label, value) in rows {
        println!("  {label:<width$}  {value}");
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

fn finished(pending: &Pending) -> String {
    let name = pending.environment();
    match (pending.kind(), pending.is_recovery()) {
        (JournalKind::Reinstall, false) => format!("Reinstalled {name}"),
        (JournalKind::Reinstall, true) => format!("Finished the interrupted reinstall of {name}"),
        (JournalKind::Install, true) => format!("Finished the interrupted install of {name}"),
        (_, true) => format!("Finished the interrupted update of {name}"),
        (_, false) => format!("Updated {name}"),
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
