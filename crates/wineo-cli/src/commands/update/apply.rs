//! Applying updates: `wineo update NAME` for one game and `--all` for every
//! game with an update, with a review first, progress while it works, and
//! what changed after. A game on the installer track is rebuilt from its
//! packages; a Galaxy game is updated, repaired, or moved file by file.

use std::process::ExitCode;

use wineo_core::gog::account::{AccountError, Session};
use wineo_core::gog::api::Secret;
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
    Account, LOGGED_OUT, Reporter, ask_password, ask_wizard, needs_password, offers_wizard,
    password_from_stdin, usage_error, warn, with_finish_hint,
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
    /// Only the branch or pin changes.
    Follow(core_galaxy::FollowPlan),
}

impl Ready {
    fn environment(&self) -> &str {
        match self {
            Self::Installer(pending) => pending.environment(),
            Self::Galaxy(plan) => plan.environment(),
            Self::Follow(plan) => plan.environment(),
        }
    }

    fn kind(&self) -> JournalKind {
        match self {
            Self::Installer(pending) => pending.kind(),
            Self::Galaxy(plan) => plan.kind(),
            Self::Follow(_) => JournalKind::Update,
        }
    }

    fn is_recovery(&self) -> bool {
        match self {
            Self::Installer(pending) => pending.is_recovery(),
            Self::Galaxy(plan) => plan.is_recovery(),
            Self::Follow(_) => false,
        }
    }

    fn warnings(&self) -> &[String] {
        match self {
            Self::Installer(pending) => pending.warnings(),
            Self::Galaxy(plan) => plan.warnings(),
            Self::Follow(plan) => plan.warnings(),
        }
    }

    fn finished(&self) -> String {
        match self {
            Self::Installer(pending) => pending.finished(),
            Self::Galaxy(plan) => plan.finished(),
            Self::Follow(plan) => plan.finished(),
        }
    }
}

/// What planning an update found.
#[allow(clippy::large_enum_variant)]
enum Planned {
    UpToDate(Vec<String>),
    /// A pinned game, with what to say instead of updating it.
    Pinned(String, Vec<String>),
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
    password: Option<&Secret>,
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
            // Finishing what the wizard question interrupted carries its
            // own branch and pin in its journal.
            choice: if wizard {
                core_galaxy::Choice::Keep
            } else {
                args.choice().0
            },
            password: password.cloned(),
            refresh: args.refresh,
            // A dry run caches nothing and records nothing.
            read_only: args.dry_run,
            wizard,
        },
        &cancellation.token,
    )
    .map_err(|error| {
        // The caller asks for the password and plans again.
        if needs_password(&error).is_some() {
            return error;
        }
        let canceled = stopped(&error).is_some();
        if logged_out
            && matches!(
                error.downcast_ref::<AccountError>(),
                Some(AccountError::LoggedOut)
            )
        {
            // Logging in comes first, and says so.
            return anyhow::anyhow!(LOGGED_OUT);
        }
        let error = if args.repair
            && matches!(
                error.downcast_ref::<UpdateError>(),
                Some(UpdateError::NotGalaxy { .. })
            )
        {
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
                    name: name.to_owned(),
                    branch: None,
                }
                .note()
            )
        } else {
            error
        };
        with_finish_hint(paths, error, name, None, canceled)
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
        PlannedUpdate::Galaxy(core_galaxy::update::Planned::Follow(plan)) => {
            Planned::Ready(Ready::Follow(plan))
        }
        PlannedUpdate::Galaxy(core_galaxy::update::Planned::Pinned(pinned)) => {
            Planned::Pinned(pinned.message(), pinned.warnings)
        }
    })
}

fn one(paths: &Paths, args: &Args, name: &str) -> anyhow::Result<ExitCode> {
    let account = Account::new();
    let mut session = account.open(paths)?;
    // Read before anything else, so a script feeding it isn't left waiting.
    let mut password = if args.password_stdin {
        Some(password_from_stdin()?)
    } else {
        None
    };
    let planned = loop {
        match plan(
            paths,
            &mut session,
            name,
            args,
            false,
            password.as_ref(),
            &Cancellation::new()?,
        ) {
            // A followed private branch whose password isn't saved, or a
            // branch named that is one: ask once, then plan again.
            Err(error) if password.is_none() => match needs_password(&error) {
                Some((title, branch)) => password = Some(ask_password(&title, &branch)?),
                None => return Err(error),
            },
            other => break other?,
        }
    };
    let ready = match planned {
        Planned::UpToDate(warnings) => {
            for warning in &warnings {
                warn(warning);
            }
            println!("Up to date.");
            return Ok(ExitCode::SUCCESS);
        }
        // Nothing to change, and the way to unpin.
        Planned::Pinned(message, warnings) => {
            for warning in &warnings {
                warn(warning);
            }
            println!("{message}");
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
                return Err(stopped_as(paths, error, &ready));
            };
            if !ask_wizard(&mut terminal()?, &label)? {
                return Err(stopped_as(paths, error, &ready));
            }
            drop(cancellation);
            // The first attempt dropped its journal, or kept it to finish,
            // so the update is planned again before it runs as a wizard.
            let cancellation = Cancellation::new()?;
            let again = match plan(
                paths,
                &mut session,
                name,
                args,
                true,
                password.as_ref(),
                &cancellation,
            )? {
                Planned::Ready(ready) => ready,
                Planned::UpToDate(_) | Planned::Pinned(..) => {
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
    // be planned, unless it has a journal: finishing one needs no check, and
    // may not need GOG at all.
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut journaled: Vec<(String, wineo_core::gog::folder::Journal)> = Vec::new();
    for error in &batch.errors {
        match error
            .environment
            .as_deref()
            .and_then(|name| update::journal_of(paths, name).map(|journal| (name, journal)))
        {
            Some((name, journal)) => journaled.push((name.to_owned(), journal)),
            None => failures.push((error.subject(), error.message.clone())),
        }
    }
    let mut names = Vec::new();
    let mut skipped = Vec::new();
    let graphical = wineo_core::launch::graphical_session();
    // Every game to finish or update: by name, whether its journal is a
    // Galaxy first install, with its id, and whether it runs in a prefix.
    let mut wanted: Vec<(String, Option<u64>, bool)> = Vec::new();
    for status in &batch.statuses {
        // A game with a journal is finished whatever GOG offers now.
        let finishes = status.unfinished;
        if status.galaxy_install {
            wanted.push((status.environment.clone(), Some(status.gog_id), true));
        } else if status.applicable || finishes {
            wanted.push((
                status.environment.clone(),
                None,
                status.platform == Platform::Windows,
            ));
        } else if let Some(reason @ update::Reason::Password(_)) = &status.reason {
            // Unlike a game that can't be compared for good, this one has a
            // way forward, so it counts as a failure the rest don't wait on.
            failures.push((status.environment.clone(), reason.note()));
        } else if let Some(reason) = &status.reason {
            skipped.push(format!("Skipped {}: {}", status.environment, reason.note()));
        }
    }
    for (name, journal) in journaled {
        let install = journal.kind == JournalKind::Install && journal.target.track == Track::Galaxy;
        wanted.push((
            name,
            install.then_some(journal.target.gog_id),
            journal.target.platform == Platform::Windows,
        ));
    }
    for (name, galaxy_install, windows) in wanted {
        if let Some(id) = galaxy_install {
            // A Galaxy install that didn't finish is finished by installing
            // it again, which the update has no way to do.
            skipped.push(format!(
                "Skipped {name}: Finish the interrupted install with: wineo import gog --game {id}"
            ));
        } else if windows && !graphical {
            skipped.push(format!("Skipped {name}: {NEEDS_DESKTOP}"));
        } else {
            names.push(name);
        }
    }

    let planning = Cancellation::new()?;
    let mut ready: Vec<Ready> = Vec::new();
    for name in &names {
        if planning.token.is_canceled() {
            return Err(Canceled.into());
        }
        match plan(paths, &mut session, name, args, false, None, &planning) {
            Ok(Planned::Ready(item)) => ready.push(item),
            Ok(Planned::UpToDate(_) | Planned::Pinned(..)) => {}
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
        _ if matches!(ready, Ready::Follow(_)) => "Change it?",
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
        Ready::Follow(plan) => return galaxy::follow_review(paths, plan),
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
    apply_raw(paths, session, ready, cancellation).map_err(|error| stopped_as(paths, error, ready))
}

/// `setup_stopped`, naming an interrupted install as one, and saying how to
/// finish whatever else stopped it after the journal was written.
fn stopped_as(paths: &Paths, error: anyhow::Error, ready: &Ready) -> anyhow::Error {
    let mut error = setup_stopped(error, ready.environment(), ready.is_recovery());
    if ready.kind() == JournalKind::Install
        && let Some(interrupted) = error.downcast_mut::<Interrupted>()
    {
        interrupted.install = true;
    }
    let canceled = stopped(&error).is_some();
    with_finish_hint(paths, error, ready.environment(), None, canceled)
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
        Ready::Follow(plan) => core_galaxy::switch::apply(paths, plan, &cancellation.token),
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
        Ready::Follow(_) => Vec::new(),
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
        Ready::Follow(_) => {}
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
