use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::bail;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::installer::{self, Cleaned, Installer, Outcome, Output};
use wineo_core::operation::Locks;
use wineo_core::paths::Paths;
use wineo_core::prefix;
use wineo_core::runners::Runners;
use wineo_core::runtime::Runtime;
use wineo_core::supervisor::{Notice, Watched};
use wineo_core::targets::{Batch, Written};

use super::{components, desktop};
use crate::picker::{self, Place};
use crate::prompt::{self, Canceled, Input, Prompt, Reply, Terminal, numbered};

/// The shell's code for a command ended by Ctrl+C.
const CANCELED: u8 = 130;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to set up
    name: String,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let name = &args.name;
    Environment::ensure_exists(paths, name)?;
    if !prompt::interactive() {
        bail!(
            "wineo setup needs a terminal. Without one, use `wineo programs {name}` and `wineo set {name} targets.<target>.exe=<path>`"
        );
    }
    // A broken config is reported with its path, before anything is asked.
    Environment::load(paths, name)?;
    let global = GlobalConfig::load(paths)?;
    let mut prompt = Prompt::new(Terminal::open()?, io::stdout());
    match setup(paths, &global, name, &mut prompt) {
        Err(err) if err.is::<Canceled>() => {
            eprintln!("Canceled. Unsaved changes were dropped.");
            Ok(ExitCode::from(CANCELED))
        }
        other => other,
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

pub(crate) fn setup<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<ExitCode> {
    offer_cleanup(paths, global, name, prompt)?;
    let environment = Environment::load(paths, name)?;
    let dir = environment.prefix(paths)?;
    let place = Place {
        paths,
        name,
        wine_prefix: prefix::wine_prefix_in(&dir),
    };
    prompt.say(format!(
        "{name}: runs on {}, prefix {}",
        environment.config.runner, environment.config.prefix
    ))?;
    if place.wine_prefix.is_none() {
        prompt.say("Its prefix is missing, so there is nothing installed to pick from.")?;
    }
    menu(
        paths,
        global,
        name,
        prompt,
        &place,
        Batch::new(environment.config.targets),
    )
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
        let choices = numbered([
            ("Add targets".to_owned(), Action::Add),
            ("Edit a target".to_owned(), Action::Edit),
            ("Remove a target".to_owned(), Action::Remove),
            ("Run an installer or updater".to_owned(), Action::Install),
            ("Finish".to_owned(), Action::Finish),
        ]);
        let default = if targets.is_empty() { "1" } else { "5" };
        let Reply::Value(action) =
            prompt.choose("What next?", "Choice", &choices, Some(default), false)?
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
                    let choices = numbered([("Save and run the installer".to_owned(), ())]);
                    let heading = "They are saved before the installer runs.";
                    if prompt.choose(heading, "Choice", &choices, Some("1"), true)? == Reply::Back {
                        continue;
                    }
                    if save(paths, global, name, prompt, &mut batch)? == Reply::Back {
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
                    && save(paths, global, name, prompt, &mut batch)? == Reply::Back
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
) -> anyhow::Result<()> {
    if let Some(operation) = Locks::new(paths).busy_with(name)? {
        prompt.say(format!(
            "{name} is busy: {}. Its targets can still be changed.",
            operation.describe()
        ))?;
        return Ok(());
    }
    if Runtime::new(paths, name).check_resolved(name).is_ok() {
        return Ok(());
    }
    let heading = format!("{name}'s last installer didn't finish cleanly.");
    let choices = numbered([
        ("Stop what it left running".to_owned(), true),
        ("Leave it for now".to_owned(), false),
    ]);
    if prompt.choose(&heading, "Choice", &choices, Some("1"), false)? != Reply::Value(true) {
        return Ok(());
    }
    let timeout = Duration::from_secs(u64::from(global.stop_timeout) + 10);
    match installer::clean_up(paths, global, name, timeout) {
        Ok(Some(Cleaned::Stopped)) => prompt.say("Stopped what it left running.")?,
        Ok(Some(Cleaned::Gone)) => prompt.say("What it started had already exited.")?,
        Ok(Some(Cleaned::Untracked)) => {
            prompt.say("It recorded nothing to stop, so check the prefix yourself.")?;
        }
        Ok(None) => {}
        Err(err) => prompt.say(format!(
            "Could not clean up after it: {:#}. Try `wineo stop {name}`",
            anyhow::Error::from(err)
        ))?,
    }
    Ok(())
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
) -> anyhow::Result<Reply<()>> {
    loop {
        if batch.targets().len() > 1 && batch.default().is_none() {
            let heading = format!("Which target should `wineo start {name}` launch?");
            if picker::choose_default(prompt, batch, &heading)? == Reply::Back {
                return Ok(Reply::Back);
            }
        }
        prompt.say(format!("Changes to {name}:"))?;
        for line in picker::review(batch) {
            prompt.say(line)?;
        }
        let choices = numbered([
            ("Save them".to_owned(), true),
            ("Keep editing".to_owned(), false),
        ]);
        if prompt.choose("Save these changes?", "Choice", &choices, Some("1"), false)?
            != Reply::Value(true)
        {
            return Ok(Reply::Back);
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
    let result = Environment::load(paths, name).and_then(|environment| {
        let prefix = environment.prefix(paths)?;
        Ok((environment, prefix))
    });
    let reconciled = result
        .map_err(anyhow::Error::from)
        .and_then(|(environment, prefix)| {
            let runner = Runners::new(paths, global)
                .ok()
                .and_then(|runners| runners.resolve(&environment.config.runner).ok());
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
            return Ok(Ran::Back);
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
    let shown_log = paths
        .contract(&log)
        .unwrap_or_else(|_| log.display().to_string());
    // Read under the installer's lock, which settings that change the prefix
    // have to wait for, so this matches what the installer runs against.
    let environment = Environment::load(paths, name)?;
    let launch = installer.launch();
    components::reconcile_config(paths, global, &environment, &launch.runner, &launch.prefix)?;
    prompt.say("Running it now. Ctrl+C asks it to close.")?;
    let outcome = installer.run(
        Duration::from_secs(global.stop_timeout.into()),
        Output::Log(&log),
        |watched| {
            let message = match watched {
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
            };
            let _ = prompt.say(message);
        },
    );
    // Those were the installer's to act on, not a request to leave setup.
    prompt.input().forget_interrupts();
    let kept = format!("Kept {name}. Continue with `wineo setup {name}`.");
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(err) => {
            prompt.say(format!("error: {:#}", anyhow::Error::from(err)))?;
            prompt.say(kept)?;
            return Ok(Ran::Ended(ExitCode::FAILURE));
        }
    };
    Ok(match outcome {
        Outcome::Finished { exit_code: Some(0) } => {
            prompt.say("The installer finished and the prefix is idle.")?;
            Ran::Finished
        }
        Outcome::Finished { exit_code } => {
            match exit_code {
                Some(code) => prompt.say(format!("The installer exited with code {code}."))?,
                None => prompt.say("The installer's exit code could not be collected.")?,
            }
            prompt.say(format!("Its output is in {shown_log}. {kept}"))?;
            let code = exit_code
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1);
            Ran::Ended(ExitCode::from(code))
        }
        Outcome::Canceled { .. } => {
            prompt.say(format!("The installer was canceled. {kept}"))?;
            Ran::Ended(ExitCode::from(CANCELED))
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
            prompt.say(format!("Kept {name}. Clean up with `wineo stop {name}`."))?;
            Ran::Ended(ExitCode::FAILURE)
        }
    })
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
