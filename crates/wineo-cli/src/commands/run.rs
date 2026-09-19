use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use anyhow::{Context, bail};
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::launch::{Launch, Overrides, parse_env_assignment};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::runtime::{RunMode, RunState, Runtime};
use wineo_core::supervisor::{Notice, Supervisor};

#[derive(clap::Args)]
pub struct Args {
    /// Environment to run
    name: String,

    #[command(flatten)]
    launch: LaunchArgs,
}

/// Options shared by the commands that launch a target.
#[derive(clap::Args)]
pub struct LaunchArgs {
    /// Target to launch instead of the default
    #[arg(short, long)]
    target: Option<String>,

    /// Set an environment variable for this run, overriding the config
    #[arg(short, long = "env", value_name = "KEY=VALUE", value_parser = parse_env_assignment)]
    env: Vec<(String, String)>,

    /// Extra arguments passed to the target
    #[arg(last = true)]
    args: Vec<String>,
}

impl LaunchArgs {
    pub fn is_empty(&self) -> bool {
        self.target.is_none() && self.env.is_empty() && self.args.is_empty()
    }

    pub fn into_overrides(self) -> Overrides {
        Overrides {
            target: self.target,
            env: self.env.into_iter().collect(),
            args: self.args,
        }
    }
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, &args.name)?;
    let overrides = args.launch.into_overrides();
    let launch = Launch::target(paths, &global, &environment, &overrides)?;
    let locks = Locks::new(paths);
    let _guard = locks.acquire(Operation::Launch, &environment.name, &[&launch.prefix])?;
    Environment::ensure_exists(paths, &environment.name)?;

    let runtime = Runtime::new(paths, &environment.name);
    let Some(_lock) = runtime.try_lock()? else {
        bail!("{} is already running", environment.name);
    };

    let supervisor = Supervisor::listen()?;
    // A separate process group keeps the terminal's Ctrl+C away from wine, so
    // wineo can ask the app to close instead of it being cut off. It is also
    // what a Proton run is signalled through, so it is recorded with the pid.
    let child = launch
        .command()
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .context("failed to start wine")?;
    let mut state = RunState::running(
        RunMode::Attached,
        launch.target.clone().unwrap_or_default(),
        child.id(),
        Some(child.id()),
        &overrides,
    );
    // Wine is already running, so bailing out here would orphan it and free the lock.
    if let Err(err) = runtime.write_state(&state) {
        warn(err.into());
    }

    let name = &environment.name;
    supervisor.wait(
        &runtime,
        &launch,
        child,
        &mut state,
        Duration::from_secs(global.stop_timeout.into()),
        |notice| match notice {
            Notice::Stopping => note(&format!("Stopping {name}, press Ctrl+C again to force it")),
            Notice::Killing => {}
            Notice::Warning(err) => warn(err.into()),
        },
    )?;
    Ok(ExitCode::from(
        state
            .exit_code
            .and_then(|code| u8::try_from(code).ok())
            .unwrap_or(1),
    ))
}

/// The terminal may already be gone after a SIGHUP, and a failed write must
/// not abort the shutdown.
fn note(message: &str) {
    let _ = writeln!(io::stderr(), "{message}");
}

fn warn(err: anyhow::Error) {
    note(&format!("warning: {err:#}"));
}
