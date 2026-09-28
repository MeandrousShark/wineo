use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, bail};
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::launch::{Launch, Overrides, Wayland};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::runtime::{RunStatus, Runtime};
use wineo_core::supervisor::{self, Claim, Spec, Started};

use crate::commands::build;
use crate::commands::run::LaunchArgs;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to start
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
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
    build::prepare_target(paths, &global, &environment, &overrides)?;
    let launch = Launch::target(paths, &global, &environment, &overrides)?;
    let locks = Locks::new(paths);
    let held: Vec<&Path> = launch.host.prefix().into_iter().collect();
    let _guard = locks.acquire(Operation::Launch, name, &held)?;
    // The config was read before the lock, so it could already be gone.
    Environment::ensure_exists(paths, name)?;
    let runtime = Runtime::new(paths, name);
    runtime.check_resolved(name)?;
    build::reconcile(paths, &global, &environment, &launch)?;

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

    build::install_winetricks(&global, &environment, &launch, &overrides)?;

    if launch.wayland == Wayland::NoSession {
        eprintln!("Not a Wayland session, starting {name} on X11.");
    }

    let spec = Spec {
        claim: Claim {
            name: name.to_owned(),
            locks: Locks::new(paths),
            config: environment.path.clone(),
        },
        runtime_dir: runtime.dir().to_owned(),
        stats_log: paths.stats_log(name),
        stop_timeout: global.stop_timeout,
        // Read now, so a changed limit applies from the next start.
        log_limit_mb: global.log_limit_mb,
        launch,
        overrides,
    };
    let exe = std::env::current_exe().context("failed to find the wineo binary")?;
    match supervisor::start(&exe, paths.portable_root(), &spec)
        .with_context(|| format!("failed to start {name}"))?
    {
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
