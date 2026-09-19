use std::process::ExitCode;

use anyhow::{Context, bail};
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::launch::{Launch, Overrides};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::runtime::{RunStatus, Runtime};
use wineo_core::supervisor::{self, Claim, Spec, Started};

use crate::commands::components;
use crate::commands::run::LaunchArgs;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to start
    name: String,

    #[command(flatten)]
    launch: LaunchArgs,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    if start(paths, &args.name, args.launch.into_overrides())? {
        println!("Started {}", args.name);
    }
    Ok(ExitCode::SUCCESS)
}

/// Starts an environment detached. Returns false, after printing a note, if it
/// was already running.
pub fn start(paths: &Paths, name: &str, overrides: Overrides) -> anyhow::Result<bool> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, name)?;
    let launch = Launch::target(paths, &global, &environment, &overrides)?;
    let locks = Locks::new(paths);
    let _guard = locks.acquire(Operation::Launch, name, &[&launch.prefix])?;
    // The config was read before the lock, so it could already be gone.
    Environment::ensure_exists(paths, name)?;
    components::reconcile_config(paths, &global, &environment, &launch.runner, &launch.prefix)?;
    let runtime = Runtime::new(paths, name);

    if runtime.is_running()? {
        // The previous run's state doesn't matter for a new one, so an
        // unreadable state file only skips the target check.
        let running = runtime
            .read_state()
            .ok()
            .flatten()
            .filter(|run| run.status == RunStatus::Running);
        if let Some(run) = &running
            && Some(run.target.as_str()) != launch.target.as_deref()
        {
            bail!(
                "{name} is already running target `{}`, stop it before starting `{}`",
                run.target,
                launch.target.as_deref().unwrap_or_default()
            );
        }
        already_running(name, &overrides);
        return Ok(false);
    }

    let spec = Spec {
        claim: Claim {
            name: name.to_owned(),
            locks_dir: paths.locks_dir(),
            config: environment.path.clone(),
        },
        runtime_dir: runtime.dir().to_owned(),
        stop_timeout: global.stop_timeout,
        // Read now, so a changed limit applies from the next start.
        log_limit_mb: global.log_limit_mb,
        launch,
        overrides,
    };
    let exe = std::env::current_exe().context("failed to find the wineo binary")?;
    match supervisor::start(&exe, &spec).with_context(|| format!("failed to start {name}"))? {
        Started::Running(_) => Ok(true),
        Started::AlreadyRunning => {
            already_running(name, &spec.overrides);
            Ok(false)
        }
    }
}

fn already_running(name: &str, overrides: &Overrides) {
    if overrides.env.is_empty() && overrides.args.is_empty() {
        eprintln!("{name} is already running");
    } else {
        eprintln!(
            "{name} is already running, so the options weren't applied. Pass them to `wineo restart` instead"
        );
    }
}
