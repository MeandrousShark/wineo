use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, bail};
use wineo_core::config::{DEFAULT_STOP_TIMEOUT, GlobalConfig};
use wineo_core::environment::Environment;
use wineo_core::installer::{self, Cleaned};
use wineo_core::operation::Locks;
use wineo_core::paths::Paths;
use wineo_core::runner::Killed;
use wineo_core::runners::Runners;
use wineo_core::runtime::{RunStatus, Runtime};
use wineo_core::supervisor;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to stop
    name: String,

    /// Kill everything in the prefix right away instead of asking programs to close
    #[arg(long)]
    force: bool,
}

pub enum Outcome {
    Stopped,
    NotRunning,
    /// The supervisor or an installer had died, so what it left behind was
    /// killed directly.
    CleanedUp,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let name = &args.name;
    // Only this command cleans up after an installer, as the one the refusals
    // point at. `restart` stops a run without it and is then refused.
    let outcome = match stop(paths, name, args.force)? {
        Outcome::NotRunning if clean_up_installer(paths, name)? => Outcome::CleanedUp,
        outcome => outcome,
    };
    match outcome {
        Outcome::Stopped => println!("Stopped {name}"),
        Outcome::NotRunning => eprintln!("{name} is not running"),
        Outcome::CleanedUp => {}
    }
    Ok(ExitCode::SUCCESS)
}

/// Stops an environment and waits until its run has ended.
pub fn stop(paths: &Paths, name: &str, force: bool) -> anyhow::Result<Outcome> {
    Environment::ensure_exists(paths, name)?;
    // An operation is not an app run, so it is never something to signal.
    if let Some(operation) = Locks::new(paths).busy_with(name)? {
        bail!(
            "{name} is busy: {}, so there is no run to stop. Finish or cancel it in its terminal",
            operation.describe()
        );
    }
    // A broken global config must not keep a running environment from stopping.
    let stop_timeout = GlobalConfig::load(paths).map_or_else(
        |err| {
            warn(err.into());
            DEFAULT_STOP_TIMEOUT
        },
        |global| global.stop_timeout,
    );
    let runtime = Runtime::new(paths, name);
    // Room for the clean stop, the kill after it, and the prefix shutting down.
    let grace = if force { 0 } else { stop_timeout };
    let timeout = Duration::from_secs(u64::from(grace) + 10);
    if supervisor::stop(&runtime, force, timeout)
        .with_context(|| format!("failed to stop {name}"))?
    {
        return Ok(Outcome::Stopped);
    }
    clean_up_lost_run(paths, name, &runtime)
}

/// Stops whatever an installer left running when wineo could not confirm it
/// was done, which is what other commands are refused over until then.
fn clean_up_installer(paths: &Paths, name: &str) -> anyhow::Result<bool> {
    let global = GlobalConfig::load(paths).unwrap_or_default();
    let timeout = Duration::from_secs(u64::from(global.stop_timeout) + 10);
    let record = Runtime::new(paths, name).operation_path();
    let Some(cleaned) = installer::clean_up(paths, &global, name, timeout).with_context(|| {
        format!(
            "failed to clean up after {name}'s last installer. If nothing is running in its prefix any more, delete {} to clear this",
            record.display()
        )
    })?
    else {
        return Ok(false);
    };
    let outcome = match cleaned {
        Cleaned::Stopped => "so it stopped anything left behind",
        Cleaned::Gone => "and what it started had already exited",
        Cleaned::Untracked => "and it recorded nothing to stop, so check the prefix yourself",
    };
    eprintln!("wineo could not confirm that {name}'s last installer had finished, {outcome}");
    Ok(true)
}

/// Kills whatever a run left in the prefix when the process that owned it
/// died, then records the run as stopped so this only happens once.
fn clean_up_lost_run(paths: &Paths, name: &str, runtime: &Runtime) -> anyhow::Result<Outcome> {
    match runtime.status() {
        Ok(status) if status.lost => {}
        Ok(_) => return Ok(Outcome::NotRunning),
        Err(err) => {
            warn(err.into());
            return Ok(Outcome::NotRunning);
        }
    }
    // Holding the lock keeps a new run from starting in the middle of this,
    // and the state is read again in case one already came and went.
    let Some(_lock) = runtime.try_lock()? else {
        return Ok(Outcome::NotRunning);
    };
    let mut run = match runtime.read_state()? {
        Some(run) if run.status == RunStatus::Running => run,
        _ => return Ok(Outcome::NotRunning),
    };
    let environment = Environment::load(paths, name)?;
    let global = GlobalConfig::load(paths).unwrap_or_default();
    let runner = match &run.runner {
        Some(runner) => runner.clone(),
        None => Runners::new(paths, &global)?.resolve(&environment.config.runner)?,
    };
    let killed = runner.kill(&environment.prefix(paths)?, run.group)?;
    run.finish(RunStatus::Stopped);
    runtime.write_state(&run)?;
    let outcome = match killed {
        Killed::Done => "so it killed anything left behind",
        Killed::Gone => "and what it started had already exited",
        Killed::Untracked => {
            "and the run is too old to record what to kill, so check the prefix yourself"
        }
    };
    eprintln!("wineo lost track of {name}'s last run, {outcome}");
    Ok(Outcome::CleanedUp)
}

fn warn(err: anyhow::Error) {
    eprintln!("warning: {err:#}");
}
