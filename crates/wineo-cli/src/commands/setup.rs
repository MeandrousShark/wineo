use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::bail;
use wineo_core::config::{GlobalConfig, Kind, proxied_key_message};
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::installer::{self, Cleaned, Installer, Outcome, Output};
use wineo_core::launch::{self, Host};
use wineo_core::operation::Locks;
use wineo_core::paths::Paths;
use wineo_core::prefix::{self, BuildPlan, PrefixEvent};
use wineo_core::runners::Runners;
use wineo_core::runtime::Runtime;
use wineo_core::supervisor::{Notice, Watched};
use wineo_core::targets::{Batch, Written};

use super::transfer::Cancellation;
use super::{build, components, desktop};
use crate::cancel;
use crate::picker::{self, Place};
use crate::prompt::{self, Canceled, Input, Prompt, Reply, numbered};

/// The shell's code for a command ended by Ctrl+C.
#[derive(clap::Args)]
pub struct Args {
    /// Environment to set up
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let name = &args.name;
    Environment::ensure_exists(paths, name)?;
    if let Ok(environment) = Environment::load(paths, name) {
        refuse_proxied(&environment)?;
    }
    if !prompt::interactive() {
        bail!(
            "wineo setup needs a terminal. Without one, use `wineo programs {name}` and `wineo set {name} targets.<target>.exe=<path>`"
        );
    }
    // A broken config is reported with its path, before anything is asked.
    Environment::load(paths, name)?;
    let global = GlobalConfig::load(paths)?;
    let mut prompt = prompt::terminal()?;
    match setup(paths, &global, name, &mut prompt) {
        Err(err) if cancel::prompted(&err) => {
            eprintln!("Canceled. Unsaved changes were dropped.");
            Ok(cancel::exit_code())
        }
        other => other,
    }
}

/// A proxied environment has no targets to set up, since its launcher owns
/// the launch.
fn refuse_proxied(environment: &Environment) -> anyhow::Result<()> {
    match &environment.config.kind {
        Kind::Wine(_) | Kind::Native => Ok(()),
        Kind::Proxied(proxied) => bail!(
            "{}",
            proxied_key_message(&environment.name, proxied.launcher, "targets")
        ),
    }
}

#[derive(Clone, Copy)]
enum Action {
    Add,
    Edit,
    Remove,
    Install,
    Finish,
}

/// How running an installer went.
pub(crate) enum Ran {
    /// It finished, so the targets can be picked from what it installed.
    Finished,
    /// Nothing ran, and the menu comes back.
    Back,
    /// It failed or was canceled, which ends setup with this code.
    Ended(ExitCode),
}

/// Recovery guidance after an error that happened after environment creation.
/// A retained operation record means setup cannot use the prefix yet.
pub(crate) fn recovery_message(paths: &Paths, name: &str) -> String {
    let runtime = Runtime::new(paths, name);
    let status = runtime.status();
    if runtime.check_resolved(name).is_err()
        || status.as_ref().is_err()
        || status.as_ref().is_ok_and(|status| status.lost)
    {
        format!("Kept {name}. Clean up first with `wineo stop {name}`.")
    } else if status.is_ok_and(|status| status.state == wineo_core::runtime::State::Running) {
        format!("Kept {name}. Stop it first with `wineo stop {name}`.")
    } else {
        format!("Kept {name}. Continue with `wineo setup {name}`.")
    }
}

pub(crate) fn report_recovery<I: Input, W: Write>(
    paths: &Paths,
    name: &str,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    prompt.say(recovery_message(paths, name))?;
    Ok(())
}

pub(crate) fn setup<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<ExitCode> {
    if !offer_cleanup(paths, global, name, prompt)? {
        return Ok(ExitCode::FAILURE);
    }
    let environment = Environment::load(paths, name)?;
    refuse_proxied(&environment)?;
    // No prefix to scan or build, so targets are only ever added by path.
    if let Kind::Native = environment.config.kind {
        let mut native_env = global.env.clone();
        native_env.extend(environment.config.env.clone());
        let place = Place {
            paths,
            name,
            wine_prefix: None,
            native_env: Some(native_env),
            game_folder: None,
        };
        return menu(
            paths,
            global,
            name,
            prompt,
            &place,
            Batch::new(environment.config.targets),
        );
    }
    let dir = environment.prefix(paths)?;
    let wine = environment.wine()?;
    prompt.say(format!(
        "{name}: runs on {}, prefix {}",
        wine.runner,
        paths.shown(&wine.prefix)
    ))?;
    let wine_prefix = match prefix::wine_prefix_in(&dir) {
        Some(wine_prefix) => Some(wine_prefix),
        None => offer_build(paths, global, &environment, prompt)?,
    };
    let place = Place {
        paths,
        name,
        wine_prefix,
        native_env: None,
        game_folder: None,
    };
    menu(
        paths,
        global,
        name,
        prompt,
        &place,
        Batch::new(environment.config.targets),
    )
}

/// Offers to build a missing prefix, so there is something to pick programs
/// from. Returns the wine prefix once there is one.
fn offer_build<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<Option<PathBuf>> {
    let name = &environment.name;
    let dir = environment.prefix(paths)?;
    let checked = launch::runner_locked(paths, global, environment).and_then(|(runner, guard)| {
        let root = global.prefix_root(paths)?;
        let plan = prefix::build_plan(paths, &root, &runner, &dir, true)?;
        Ok((runner, guard, plan))
    });
    let (runner, _build_guard) = match checked {
        Ok((_, _, BuildPlan::Ready)) => return Ok(prefix::wine_prefix_in(&dir)),
        Ok((runner, guard, BuildPlan::Build)) => (runner, guard),
        Err(err) => {
            prompt.say(format!("{err:#}"))?;
            return Ok(None);
        }
    };
    let skip = numbered([("Skip for now".to_owned(), ())]);
    if prompt.review(
        "Its prefix doesn't exist yet.",
        None,
        "create it",
        &skip,
        false,
    )? != Reply::Value(None)
    {
        prompt.say("Its prefix is missing, so there is nothing installed to pick from.")?;
        return Ok(None);
    }
    let cancellation = Cancellation::new()?;
    let log = paths.runtime_dir(name).join("create.log");
    let built = environment.build_prefix(
        paths,
        global,
        &runner,
        true,
        &cancellation.token,
        Output::Log(&log),
        |event| {
            if let PrefixEvent::Started { dir } = event {
                let _ = prompt.say(build::creating_note(paths, name, &runner, &dir));
            }
        },
    );
    if let Err(err) = built {
        if matches!(err, Error::Canceled(_)) || prompt.input().interrupted() {
            return Err(Canceled.into());
        }
        return Err(err.into());
    }
    Ok(prefix::wine_prefix_in(&dir))
}

/// The setup menu, from a batch that may already hold changes, like the
/// targets guided `new` just picked.
pub(crate) fn menu<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    prompt: &mut Prompt<I, W>,
    place: &Place,
    mut batch: Batch,
) -> anyhow::Result<ExitCode> {
    let mut shown = None;
    loop {
        let targets = batch.targets();
        if shown.as_ref() != Some(&targets) {
            list(prompt, &batch)?;
            shown = Some(targets.clone());
        }
        let mut entries = vec![
            ("Add targets".to_owned(), Action::Add),
            ("Edit a target".to_owned(), Action::Edit),
            ("Remove a target".to_owned(), Action::Remove),
        ];
        // An installer runs in a prefix, which a native environment has none of.
        if place.native_env.is_none() {
            entries.push(("Run an installer or updater".to_owned(), Action::Install));
        }
        entries.push(("Finish".to_owned(), Action::Finish));
        let default = if targets.is_empty() {
            "1".to_owned()
        } else {
            entries.len().to_string()
        };
        let choices = numbered(entries);
        prompt.set_first_question(true);
        let Reply::Value(action) =
            prompt.choose("What next?", "Choice", &choices, Some(&default), false)?
        else {
            unreachable!("the menu offers no way back");
        };
        match action {
            Action::Add => {
                picker::pick(prompt, place, &mut batch)?;
            }
            Action::Edit => {
                picker::edit(prompt, place, &mut batch)?;
            }
            Action::Remove => {
                picker::remove(prompt, place, &mut batch)?;
            }
            Action::Install => {
                if !batch.is_empty() {
                    prompt.say("You have unsaved changes:")?;
                    for line in picker::review(&batch) {
                        prompt.say(line)?;
                    }
                    if prompt.review::<()>(
                        "They are saved before the installer runs.",
                        None,
                        "save and run the installer",
                        &[],
                        true,
                    )? == Reply::Back
                    {
                        continue;
                    }
                    if save(paths, global, name, prompt, &mut batch, false)? == Reply::Back {
                        continue;
                    }
                }
                match install(paths, global, name, prompt)? {
                    Ran::Back => {}
                    Ran::Ended(code) => return Ok(code),
                    Ran::Finished => {
                        // Picked against the config as it is now.
                        batch = Batch::new(Environment::load(paths, name)?.config.targets);
                        picker::pick(prompt, place, &mut batch)?;
                    }
                }
            }
            Action::Finish => {
                if !batch.is_empty()
                    && save(paths, global, name, prompt, &mut batch, false)? == Reply::Back
                {
                    continue;
                }
                return finish(paths, global, name, prompt, &batch);
            }
        }
    }
}

/// The last step of a flow once everything is saved: the desktop entries,
/// which this also retries after an earlier failure, then what comes next.
pub(crate) fn finish<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    prompt: &mut Prompt<I, W>,
    batch: &Batch,
) -> anyhow::Result<ExitCode> {
    let reconciled = reconcile(paths, global, name, prompt);
    next_steps(prompt, name, batch)?;
    Ok(if reconciled {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Offers to stop what an installer left running when it could not confirm
/// it had finished, since everything that needs the prefix is refused until
/// then. Changing targets isn't, so it can also be left for later.
fn offer_cleanup<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<bool> {
    if !check_run_recovery(paths, name, prompt)? {
        return Ok(false);
    }
    if let Some(operation) = Locks::new(paths).busy_with(name)? {
        prompt.say(format!(
            "{name} is busy: {}. Its targets can still be changed.",
            operation.describe()
        ))?;
        return Ok(true);
    }
    if Runtime::new(paths, name).check_resolved(name).is_ok() {
        return Ok(true);
    }
    let heading = format!("{name}'s last installer didn't finish cleanly.");
    let choices = numbered([
        ("Stop what it left running".to_owned(), true),
        ("Leave it for now".to_owned(), false),
    ]);
    if prompt.choose(&heading, "Choice", &choices, Some("1"), false)? != Reply::Value(true) {
        prompt.say(format!(
            "Cleanup remains unresolved. Run `wineo stop {name}` before scanning or running anything in the prefix."
        ))?;
        return Ok(false);
    }
    let timeout = Duration::from_secs(u64::from(global.stop_timeout) + 10);
    match installer::clean_up(paths, global, name, timeout) {
        Ok(Some(Cleaned::Stopped)) => prompt.say("Stopped what it left running.")?,
        Ok(Some(Cleaned::Gone)) => prompt.say("What it started had already exited.")?,
        Ok(Some(Cleaned::Untracked)) => {
            prompt.say(format!(
                "Cleanup is unresolved: the installer recorded no process group. Check the prefix, then remove {} only after confirming nothing it started is still running.",
                Runtime::new(paths, name).operation_path().display()
            ))?;
            return Ok(false);
        }
        Ok(Some(Cleaned::Unresolved)) => {
            prompt.say(format!(
                "Cleanup is unresolved because ownership could not be confirmed. Check the prefix, then remove {} only after confirming nothing it started is still running.",
                Runtime::new(paths, name).operation_path().display()
            ))?;
            return Ok(false);
        }
        Ok(None) => {}
        Err(err) => {
            prompt.say(format!(
                "Could not clean up after it: {:#}. Try `wineo stop {name}`",
                anyhow::Error::from(err)
            ))?;
            return Ok(false);
        }
    }
    if !check_run_recovery(paths, name, prompt)? {
        return Ok(false);
    }
    if Runtime::new(paths, name).check_resolved(name).is_err() {
        prompt.say(format!(
            "Cleanup remains unresolved. Run `wineo stop {name}` before scanning or running anything in the prefix."
        ))?;
        return Ok(false);
    }
    Ok(true)
}

fn check_run_recovery<I: Input, W: Write>(
    paths: &Paths,
    name: &str,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<bool> {
    match Runtime::new(paths, name).status() {
        Ok(status) if status.lost => {
            prompt.say(format!(
                "{name}'s last run is unresolved. Run `wineo stop {name}` before scanning or running anything in the prefix."
            ))?;
            Ok(false)
        }
        Err(err) => {
            prompt.say(format!(
                "Could not read {name}'s run state: {err:#}. Run `wineo stop {name}` before scanning or running anything in the prefix."
            ))?;
            Ok(false)
        }
        _ => Ok(true),
    }
}

fn list<I: Input, W: Write>(prompt: &mut Prompt<I, W>, batch: &Batch) -> anyhow::Result<()> {
    let targets = batch.targets();
    if targets.is_empty() {
        prompt.say("No targets yet.")?;
        return Ok(());
    }
    prompt.say("Targets:")?;
    let default = batch.default();
    let width = targets.keys().map(String::len).max().unwrap_or(0);
    for (target, spec) in &targets {
        let mut notes = Vec::new();
        if default.as_deref() == Some(target.as_str()) {
            notes.push("default".to_owned());
        }
        if spec.desktop {
            notes.push(format!(
                "desktop entry \"{}\"",
                spec.name.as_deref().unwrap_or(target)
            ));
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!("  {}", notes.join(", "))
        };
        prompt.say(format!("  {target:width$}  {}{notes}", spec.exe))?;
    }
    Ok(())
}

/// Reviews and writes the batch. Several targets need a default, so one is
/// asked for first when none is marked. Back means keep editing.
pub(crate) fn save<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    prompt: &mut Prompt<I, W>,
    batch: &mut Batch,
    review_before_save: bool,
) -> anyhow::Result<Reply<()>> {
    let mut review_before_save = review_before_save;
    loop {
        if batch.targets().len() > 1 && batch.default().is_none() {
            let heading = format!("Which target should `wineo start {name}` launch?");
            if picker::choose_default(prompt, batch, &heading)? == Reply::Back {
                return Ok(Reply::Back);
            }
        }
        if review_before_save {
            prompt.say(format!("Changes to {name}:"))?;
            for line in picker::review(batch) {
                prompt.say(line)?;
            }
            let choices = numbered([("Keep editing".to_owned(), ())]);
            if prompt.review("Save these changes?", None, "save them", &choices, false)?
                != Reply::Value(None)
            {
                return Ok(Reply::Back);
            }
        }
        match wineo_core::targets::write(paths, name, batch)? {
            Written::Saved => {
                prompt.say(format!("Saved {name}."))?;
                *batch = Batch::new(Environment::load(paths, name)?.config.targets);
                reconcile(paths, global, name, prompt);
                return Ok(Reply::Value(()));
            }
            Written::Changed(current) => {
                let (rebased, dropped) = batch.rebase(current);
                *batch = rebased;
                review_before_save = true;
                prompt.say("The config changed meanwhile. Review again:")?;
                for note in dropped {
                    prompt.say(note)?;
                }
                if batch.is_empty() {
                    prompt.say("Nothing is left to save.")?;
                    return Ok(Reply::Value(()));
                }
            }
        }
    }
}

/// Brings the desktop entries in line with the saved config. A failure only
/// costs the entries, since the config is already saved, so it is reported
/// rather than ending setup.
fn reconcile<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    prompt: &mut Prompt<I, W>,
) -> bool {
    let reconciled = Environment::load(paths, name)
        .map_err(anyhow::Error::from)
        .and_then(|environment| {
            match &environment.config.kind {
                Kind::Wine(_) => {}
                Kind::Native => return desktop::reconcile_native(paths, global, &environment),
                // Proxied environments get no entries yet.
                Kind::Proxied(_) => return Ok(()),
            }
            let prefix = environment.prefix(paths)?;
            let runner = Runners::new(paths, global)
                .ok()
                .and_then(|runners| runners.resolve(&environment.wine().ok()?.runner).ok());
            desktop::reconcile(paths, global, &environment, runner.as_ref(), &prefix)
        });
    match reconciled {
        Ok(()) => true,
        Err(err) => {
            let _ = prompt.say(format!(
                "Saved, but desktop entries failed: {err:#}. Rerun `wineo setup {name}` to retry."
            ));
            false
        }
    }
}

fn install<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<Ran> {
    let reply = prompt.text(
        "Installer or updater, as a Unix path or a Windows one",
        None,
        |answer| {
            let answer = picker::clean_path(answer);
            if answer.is_empty() {
                Err("Enter the path to the installer".to_owned())
            } else {
                wineo_core::launch::check_program_path(paths, &answer)
                    .map(|()| answer)
                    .map_err(|err| format!("{err}"))
            }
        },
    )?;
    let Reply::Value(program) = reply else {
        return Ok(Ran::Back);
    };
    let log = Runtime::new(paths, name).install_log_path();
    let shown_log = paths
        .contract(&log)
        .unwrap_or_else(|_| log.display().to_string());
    let heading = format!(
        "Run {} in {name}? Its output goes to {shown_log}",
        file_name(&program)
    );
    let choices = numbered([("Run it".to_owned(), ())]);
    if prompt.choose(&heading, "Choice", &choices, Some("1"), true)? == Reply::Back {
        return Ok(Ran::Back);
    }

    let installer = match Installer::prepare(paths, global, name, &program) {
        Ok(installer) => installer,
        Err(err) => {
            prompt.say(format!("{:#}", anyhow::Error::from(err)))?;
            let resolved = Runtime::new(paths, name).check_resolved(name).is_ok();
            report_recovery(paths, name, prompt)?;
            return if resolved {
                Ok(Ran::Back)
            } else {
                Ok(Ran::Ended(ExitCode::FAILURE))
            };
        }
    };
    run_installer(paths, global, name, prompt, installer)
}

/// Runs a prepared installer with its output in `install.log`, and says how
/// it went. Anything but a clean finish ends the flow.
pub(crate) fn run_installer<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    prompt: &mut Prompt<I, W>,
    installer: Installer,
) -> anyhow::Result<Ran> {
    let log = Runtime::new(paths, name).install_log_path();
    ready_installer(paths, global, name, installer.launch())?;
    prompt.say(RUNNING)?;
    let cancellation = Cancellation::new()?;
    let outcome = installer.run(
        &cancellation.token,
        Duration::from_secs(global.stop_timeout.into()),
        Output::Log(&log),
        |watched| {
            if let Some(message) = watched_message(watched) {
                let _ = prompt.say(message);
            }
        },
    );
    // Those were the installer's to act on, not a request to leave setup.
    prompt.input().forget_interrupts();
    report_installer(paths, name, prompt, outcome)
}

/// Said right before an installer starts.
pub(crate) const RUNNING: &str = "Running it now. Ctrl+C asks it to close.";

/// Brings the prefix's components in line with the config, once the installer
/// holds the prefix and right before it starts.
pub(crate) fn ready_installer(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    launch: &launch::Launch,
) -> anyhow::Result<()> {
    // Read under the installer's lock, which settings that change the prefix
    // have to wait for, so this matches what the installer runs against.
    let environment = Environment::load(paths, name)?;
    if let Host::Wine { runner, prefix } = &launch.host {
        components::reconcile_config(paths, global, &environment, runner, prefix)?;
    }
    Ok(())
}

/// What to tell whoever is watching an installer, if anything.
pub(crate) fn watched_message(watched: Watched) -> Option<String> {
    Some(match watched {
        // Nothing worth telling whoever is watching the installer.
        Watched::Notice(Notice::Heartbeat) => return None,
        Watched::Exited(_) => {
            "The installer exited. Waiting for anything it started to finish".to_owned()
        }
        Watched::Notice(Notice::Stopping) => {
            "Canceling the installer, press Ctrl+C again to force it".to_owned()
        }
        Watched::Notice(Notice::Killing) => {
            "Killing everything the installer left running".to_owned()
        }
        Watched::Notice(Notice::Warning(err)) => {
            format!("warning: {:#}", anyhow::Error::from(err))
        }
    })
}

/// Says how an installer run went.
pub(crate) fn report_installer<I: Input, W: Write>(
    paths: &Paths,
    name: &str,
    prompt: &mut Prompt<I, W>,
    outcome: wineo_core::Result<Outcome>,
) -> anyhow::Result<Ran> {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(err) => {
            prompt.say(format!("error: {:#}", anyhow::Error::from(err)))?;
            report_recovery(paths, name, prompt)?;
            return Ok(Ran::Ended(ExitCode::FAILURE));
        }
    };
    Ok(match outcome {
        Outcome::Finished { exit_code: Some(0) } => {
            if Runtime::new(paths, name).check_resolved(name).is_err() {
                prompt.say("The installer finished, but cleanup is unresolved.")?;
                report_recovery(paths, name, prompt)?;
                return Ok(Ran::Ended(ExitCode::FAILURE));
            }
            prompt.say("The installer finished and the prefix is idle.")?;
            Ran::Finished
        }
        Outcome::Finished { exit_code } => {
            report_failed_exit(paths, name, prompt, exit_code)?;
            report_recovery(paths, name, prompt)?;
            let code = exit_code
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1);
            Ran::Ended(ExitCode::from(code))
        }
        Outcome::Canceled { .. } => {
            prompt.say("The installer was canceled.")?;
            report_recovery(paths, name, prompt)?;
            Ran::Ended(cancel::exit_code())
        }
        Outcome::Unresolved {
            exit_code, error, ..
        } => {
            prompt.say(format!(
                "error: couldn't confirm the installer's processes stopped: {:#}",
                anyhow::Error::from(error)
            ))?;
            match exit_code {
                Some(0) => prompt.say("The installer itself finished.")?,
                Some(code) => prompt.say(format!("The installer exited with code {code}."))?,
                None => {}
            }
            report_recovery(paths, name, prompt)?;
            Ran::Ended(ExitCode::FAILURE)
        }
    })
}

/// Says that an installer finished with a failing or unknown exit code, and
/// where its output went.
pub(crate) fn report_failed_exit<I: Input, W: Write>(
    paths: &Paths,
    name: &str,
    prompt: &mut Prompt<I, W>,
    exit_code: Option<i32>,
) -> anyhow::Result<()> {
    let log = Runtime::new(paths, name).install_log_path();
    let shown_log = paths
        .contract(&log)
        .unwrap_or_else(|_| log.display().to_string());
    match exit_code {
        Some(code) => prompt.say(format!("The installer exited with code {code}."))?,
        None => prompt.say("The installer's exit code could not be collected.")?,
    }
    prompt.say(format!("Its output is in {shown_log}."))?;
    Ok(())
}

/// The commands for what comes next, printed rather than run.
fn next_steps<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    name: &str,
    batch: &Batch,
) -> anyhow::Result<()> {
    let targets = batch.targets();
    let Some(first) = targets.keys().next().cloned() else {
        prompt.say(format!("Add a target later with: wineo setup {name}"))?;
        return Ok(());
    };
    // A lone target is launched without being marked.
    let default = batch
        .default()
        .or_else(|| (targets.len() == 1).then(|| first.clone()));
    let mut lines = Vec::new();
    let started = match default {
        Some(default) => {
            lines.push(("Start it", format!("wineo start {name}")));
            default
        }
        None => {
            lines.push(("Start one", format!("wineo start {name} --target {first}")));
            first
        }
    };
    if let Some(other) = targets.keys().find(|target| **target != started) {
        lines.push((
            "Start another",
            format!("wineo start {name} --target {other}"),
        ));
    }
    lines.push(("Change setup", format!("wineo setup {name}")));
    lines.push(("See its output", format!("wineo logs {name}")));
    let width = lines
        .iter()
        .map(|(label, _)| label.len() + 1)
        .max()
        .unwrap_or(0);
    for (label, command) in lines {
        prompt.say(format!("{:width$}  {command}", format!("{label}:")))?;
    }
    Ok(())
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}
