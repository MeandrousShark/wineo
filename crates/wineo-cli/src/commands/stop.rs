use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runtime::Runtime;
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
    /// The supervisor had died, so what the run left behind was killed directly.
    CleanedUp,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    match stop(paths, &args.name, args.force)? {
        Outcome::Stopped => println!("Stopped {}", args.name),
        Outcome::NotRunning => eprintln!("{} is not running", args.name),
        Outcome::CleanedUp => {}
    }
    Ok(ExitCode::SUCCESS)
}

/// Stops an environment and waits until its run has ended.
pub fn stop(paths: &Paths, name: &str, force: bool) -> anyhow::Result<Outcome> {
    Environment::ensure_exists(paths, name)?;
    let global = GlobalConfig::load(paths)?;
    let runtime = Runtime::new(paths, name);
    // Room for the clean stop, the kill after it, and the prefix shutting down.
    let grace = if force { 0 } else { global.stop_timeout };
    let timeout = Duration::from_secs(u64::from(grace) + 10);
    if supervisor::stop(&runtime, force, timeout)
        .with_context(|| format!("failed to stop {name}"))?
    {
        return Ok(Outcome::Stopped);
    }
    if !runtime.status()?.lost {
        return Ok(Outcome::NotRunning);
    }
    let environment = Environment::load(paths, name)?;
    Runner::resolve(&environment.config.runner)?.kill(&environment.prefix(paths)?)?;
    eprintln!("wineo lost track of {name}'s last run, so it killed anything left in the prefix");
    Ok(Outcome::CleanedUp)
}
