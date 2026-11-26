//! `wineo import epic --game` and `--forget`: a review from what Legendary
//! says about the game, progress while it downloads the game and each DLC,
//! and Epic's art after, like a GOG install.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use wineo_core::config::GlobalConfig;
use wineo_core::download::Lazy;
use wineo_core::epic;
use wineo_core::epic::install::{self, Event, Existing, InstallError, Installed, Plan, Request};
use wineo_core::epic::legendary::Legendary;
use wineo_core::epic::owned;
use wineo_core::paths::Paths;

use super::setup::{self, Found};
use super::{Args, HOST_ONLY};
use crate::cancel;
use crate::commands::gog_support::{usage_error, warn};
use crate::commands::table;
use crate::commands::transfer::Cancellation;
use crate::picker;
use crate::prompt::{self, Reply, terminal};
use crate::style;

const NEEDS_YES: &str = "Installing without a terminal needs --yes.";

pub(super) fn one(paths: &Paths, args: &Args, app: &str) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(paths.portable_root().is_none(), HOST_ONLY);
    if wineo_core::config::validate_epic_id(app).is_err() {
        return Ok(usage_error(format!("{app:?} isn't an Epic game id.")));
    }
    // Whether wineo already has it is known without Epic or a question.
    let existing = install::existing(paths, app)?;
    if let Existing::Installed { name, folder } = &existing {
        println!(
            "Already installed. {name} owns {}.",
            style::path(paths, folder)
        );
        if args.dry_run {
            println!("Dry run, nothing was installed.");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let asking = !args.yes && !args.dry_run;
    if asking && !prompt::interactive() {
        anyhow::bail!(NEEDS_YES);
    }
    let global = GlobalConfig::load(paths)?;
    let Some(legendary) = setup::obtain(paths, &global, args)? else {
        return Ok(ExitCode::SUCCESS);
    };
    let owned = {
        let cancellation = Cancellation::new()?;
        let owned = owned::load(&legendary, false, &cancellation.token);
        cancellation.result(owned.map_err(anyhow::Error::from))?
    };
    if let Some(warning) = &owned.warning {
        warn(warning);
    }
    let Some(game) = owned.games.iter().find(|game| game.app_name == app) else {
        return Err(InstallError::NotOwned {
            app: app.to_owned(),
        }
        .into());
    };
    // An unfinished install carries on as it began.
    if let Existing::Unfinished { owner: Some(_), .. } = &existing {
        let ignored: Vec<&str> = [
            (args.runner.is_some(), "--runner"),
            (args.arch.is_some(), "--arch"),
        ]
        .into_iter()
        .filter_map(|(given, flag)| given.then_some(flag))
        .collect();
        if !ignored.is_empty() {
            return Ok(usage_error(format!(
                "{}'s install didn't finish, so --game finishes it. Leave out {}.",
                game.title,
                ignored.join(" and ")
            )));
        }
    }
    let request = Request {
        app_name: app.to_owned(),
        dir: args
            .dir
            .as_deref()
            .map(|dir| paths.absolute(&picker::clean_path(dir)))
            .transpose()?,
        runner: args.runner.clone(),
        arch: args.arch,
        // New environments get desktop entries, as a GOG install's do.
        desktop: true,
    };
    let plan = {
        let cancellation = Cancellation::new()?;
        let planned = install::plan(&legendary, &request, game, &existing, &cancellation.token);
        match planned {
            Err(error) if install::is_cancel(&error) || cancellation.token.is_canceled() => {
                return Ok(canceled(paths, app));
            }
            other => other?,
        }
    };
    print_review(paths, &plan);
    if args.dry_run {
        println!("Dry run, nothing was installed.");
        return Ok(ExitCode::SUCCESS);
    }
    if asking && !confirm(&plan)? {
        eprintln!("Install canceled.");
        return Ok(cancel::exit_code());
    }
    match apply(paths, &legendary, &plan) {
        Ok(installed) => {
            report(paths, &global, &plan, &installed);
            Ok(ExitCode::SUCCESS)
        }
        Err(error) if install::is_cancel(&error) => Ok(canceled(paths, app)),
        // The mark there is the other install's, which is still going.
        Err(error) if matches!(error.downcast_ref(), Some(InstallError::Running { .. })) => {
            Err(error)
        }
        Err(error) if install::left_unfinished(paths, app) => {
            eprintln!("{}", style::error_stderr(format!("error: {error:#}")));
            say_finish(app);
            Ok(ExitCode::FAILURE)
        }
        Err(error) => Err(error),
    }
}

/// Says how a cancel left things, and exits with the cancel's code.
fn canceled(paths: &Paths, app: &str) -> ExitCode {
    if install::left_unfinished(paths, app) {
        eprintln!("Install interrupted.");
        say_finish(app);
    } else {
        eprintln!("Install canceled.");
    }
    cancel::exit_code()
}

fn say_finish(app: &str) {
    eprintln!("Finish the interrupted install with: wineo import epic --game {app}");
}

fn confirm(plan: &Plan) -> anyhow::Result<bool> {
    let mut prompt = terminal()?;
    let question = if plan.resumes() {
        "Finish it?"
    } else {
        "Install it?"
    };
    prompt.confirm_hint(false)?;
    Ok(matches!(
        prompt.confirm(question, true)?,
        Reply::Value(true)
    ))
}

fn size(bytes: Option<u64>, dlc: usize) -> String {
    let size = bytes.map_or_else(|| "unknown".to_owned(), style::size);
    match dlc {
        0 => size,
        count => format!("{size} plus {count} DLC"),
    }
}

fn review_lines(paths: &Paths, plan: &Plan) -> Vec<String> {
    let kind = if plan.creation().is_some() {
        " (new Windows environment)"
    } else {
        ""
    };
    let mut rows = vec![
        (
            "environment".to_owned(),
            format!("{}{kind}", plan.environment()),
        ),
        ("folder".to_owned(), style::path(paths, plan.folder())),
    ];
    if let Some(creation) = plan.creation() {
        rows.push(("runner".to_owned(), creation.runner().name().to_owned()));
        rows.push(("arch".to_owned(), creation.resolved_arch().to_string()));
    }
    rows.push((
        "version".to_owned(),
        plan.version().unwrap_or("unknown").to_owned(),
    ));
    // One DLC per line; the rest sit under the first one's value.
    for (index, dlc) in plan.dlc().iter().enumerate() {
        let label = if index == 0 { "dlc" } else { "" };
        rows.push((label.to_owned(), dlc.title.clone()));
    }
    // Legendary can't say a DLC's size ahead.
    rows.push((
        "download".to_owned(),
        size(plan.download_size(), plan.dlc().len()),
    ));
    rows.push((
        "installed".to_owned(),
        size(plan.install_size(), plan.dlc().len()),
    ));
    let action = if plan.resumes() {
        "Finish the interrupted install of"
    } else {
        "Install"
    };
    let mut lines = vec![format!("{action} {} ({})", plan.title(), plan.app_name())];
    lines.extend(table::aligned_rows(rows));
    lines
}

fn print_review(paths: &Paths, plan: &Plan) {
    for line in review_lines(paths, plan) {
        println!("{line}");
    }
    for warning in plan.warnings() {
        warn(warning);
    }
}

/// How long a download may go without progress before it is said. A debug
/// build reads `WINEO_TEST_EPIC_STALL_SECONDS`, so tests needn't wait five
/// minutes; release builds never look.
fn stall() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(seconds) = std::env::var("WINEO_TEST_EPIC_STALL_SECONDS")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
    {
        return Duration::from_secs(seconds);
    }
    install::STALL
}

fn apply(paths: &Paths, legendary: &Legendary, plan: &Plan) -> anyhow::Result<Installed> {
    let cancellation = Cancellation::new()?;
    let name = plan.environment().to_owned();
    let mut progress = Progress::default();
    let applied = install::apply(
        legendary,
        plan,
        stall(),
        &cancellation.token,
        |event| match event {
            Event::Create(event) => {
                crate::commands::import_gog::install::created(paths, &name, event)
            }
            event => progress.show(&event),
        },
    );
    if cancellation.token.is_canceled() && applied.is_err() {
        return Err(InstallError::Canceled {
            marked: install::left_unfinished(paths, plan.app_name()),
        }
        .into());
    }
    applied
}

/// Prints a line as each run starts, then its percentage about once a
/// second.
#[derive(Default)]
struct Progress {
    last: Option<Instant>,
    /// The whole percentage last said, for the run it was said for.
    said: Option<(usize, u64)>,
}

impl Progress {
    fn label(index: usize, total: usize) -> String {
        if total == 1 {
            "Downloading".to_owned()
        } else {
            format!("Downloading {index} of {total}")
        }
    }

    fn show(&mut self, event: &Event) {
        match *event {
            Event::Started {
                index,
                total,
                ref title,
            } => {
                eprintln!("{}: {title}", Self::label(index, total));
                self.last = None;
            }
            Event::Progress {
                index,
                total,
                percent,
            } => {
                let whole = percent.floor() as u64;
                let due = self
                    .last
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(1));
                if self.said == Some((index, whole)) || !(due || whole == 100) {
                    return;
                }
                self.said = Some((index, whole));
                self.last = Some(Instant::now());
                eprintln!("{}: {whole}%", Self::label(index, total));
            }
            Event::Stalled { minutes, .. } => warn(format!(
                "No download progress for {minutes} minutes. It keeps trying; press Ctrl+C to stop, and run the same command again to resume."
            )),
            // The run's first line already says it is downloading, and
            // Legendary prints several lines a second that aren't progress.
            Event::Working { .. } | Event::DlcFailed { .. } | Event::Create(_) => {}
        }
    }
}

/// Says what the install did, then adds Epic's art and the desktop entries.
fn report(paths: &Paths, global: &GlobalConfig, plan: &Plan, installed: &Installed) {
    for warning in &installed.warnings {
        warn(warning);
    }
    let name = installed.environment.as_str();
    art(paths, global, plan, name);
    if let Err(error) = crate::commands::art::apply_desktop(paths, name) {
        warn(format!("Couldn't update desktop entries: {error:#}"));
    }
    if installed.resumed {
        println!("Finished the interrupted install of {name}.");
    } else {
        println!("Installed {} as {name}.", installed.title);
    }
}

/// Epic's art, when the owned list has addresses for it. The install has
/// already succeeded, so a failure here is a warning.
fn art(paths: &Paths, global: &GlobalConfig, plan: &Plan, name: &str) {
    let kinds = global.art.fetch.kinds();
    if kinds.is_empty() || plan.images().is_empty() {
        return;
    }
    let https = Lazy::new();
    let transport = match https.https() {
        Ok(transport) => transport,
        Err(error) => {
            warn(format!("Skipped Epic art: {error:#}"));
            return;
        }
    };
    let Ok(cancellation) = Cancellation::new() else {
        return;
    };
    let request = epic::art::Request {
        name,
        app_name: plan.app_name(),
        images: plan.images(),
        kinds: &kinds,
        transport,
    };
    match epic::art::import(paths, &request, &cancellation.token) {
        Ok(report) => {
            if let Some(line) = report.line() {
                println!("{line}");
            }
            for note in &report.notes {
                warn(note);
            }
        }
        Err(error) => warn(format!("Skipped Epic art: {error:#}")),
    }
}

/// `--forget`: drops wineo's and Legendary's records of a game no
/// environment owns. Never asks to download Legendary; without one, only a
/// game Legendary has no record of can be forgotten.
pub(super) fn forget(paths: &Paths, app: &str) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(paths.portable_root().is_none(), HOST_ONLY);
    if wineo_core::config::validate_epic_id(app).is_err() {
        return Ok(usage_error(format!("{app:?} isn't an Epic game id.")));
    }
    let global = GlobalConfig::load(paths)?;
    let legendary = match setup::find(paths, &global)? {
        Found::Usable(legendary) => Some(legendary),
        Found::Unusable { .. } => None,
    };
    let cancellation = Cancellation::new()?;
    let forgotten = install::forget(paths, legendary.as_deref(), app, &cancellation.token);
    let forgotten = cancellation.result(forgotten)?;
    println!(
        "Forgot Epic game {app}. Its files are still in {}.",
        style::path(paths, &forgotten.folder)
    );
    Ok(ExitCode::SUCCESS)
}
