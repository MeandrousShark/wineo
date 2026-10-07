use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use anyhow::{Context, bail};
use wineo_core::error::Error;
use wineo_core::installer::Output;
use wineo_core::launch::{self, Overrides, Wayland, parse_launch_env_assignment};
use wineo_core::paths::Paths;
use wineo_core::process;
use wineo_core::runtime::{RunMode, RunState, RunStatus, Runtime};
use wineo_core::supervisor::{Notice, Supervisor};

use crate::commands::build;
use crate::commands::transfer::Cancellation;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to run
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    #[command(flatten)]
    launch: LaunchArgs,
}

/// Options shared by the commands that launch a target.
#[derive(clap::Args)]
pub struct LaunchArgs {
    /// Target to launch instead of the default
    #[arg(short, long, add = clap_complete::ArgValueCompleter::new(crate::completion::targets))]
    target: Option<String>,

    /// Set an environment variable for this run, overriding the config
    #[arg(short, long = "env", value_name = "KEY=VALUE", value_parser = parse_launch_env_assignment)]
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
    let overrides = args.launch.into_overrides();
    let cancellation = Cancellation::new()?;
    let mut cleanup_listener = None;
    let mut signal_error = None;
    let mut prepared = match launch::prepare(
        paths,
        &args.name,
        &overrides,
        &cancellation.token,
        wineo_core::hooks::Output::Inherit,
        |event| {
            if let launch::LaunchEvent::AfterHook(token) = event {
                match Cancellation::after_hook_with(&token) {
                    Ok(listener) => cleanup_listener = Some(listener),
                    Err(err) => {
                        signal_error = Some(err);
                        token.force();
                    }
                }
            } else {
                build::print_event(paths, &args.name, event);
            }
        },
    ) {
        Err(err)
            if err
                .downcast_ref::<Error>()
                .is_some_and(|err| matches!(err, Error::Canceled(_))) =>
        {
            return Ok(ExitCode::from(130));
        }
        Err(err) => return Err(err),
        Ok(prepared) => prepared,
    };
    if let Some(err) = signal_error {
        return Err(err);
    }
    drop(cleanup_listener);
    let mut lock = None;
    let mut already_running = false;
    let preflight = (|| {
        let global = &prepared.global;
        let environment = &prepared.environment;
        let launch = &prepared.launch;
        let runtime = Runtime::new(paths, &environment.name);

        lock = runtime.try_lock()?;
        if lock.is_none() {
            already_running = true;
            bail!("{} is already running", environment.name);
        }
        launch::install_winetricks(
            global,
            environment,
            launch,
            &overrides,
            &cancellation.token,
            Output::Inherit,
            |event| build::print_event(paths, &environment.name, event),
        )?;

        if launch.wayland == Wayland::NoSession {
            note(&format!(
                "Not a Wayland session, starting {} on X11.",
                environment.name
            ));
        }

        let supervisor = Supervisor::with_token(&cancellation.token);
        // A separate process group keeps the terminal's Ctrl+C away from wine, so
        // wineo can ask the app to close instead of it being cut off. It is also
        // what a Proton run is signalled through, so it is recorded with the pid.
        let spawned = launch
            .command()
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
            .with_context(|| format!("failed to start {}", launch.host.launcher()));
        let child = spawned?;
        Ok::<_, anyhow::Error>((runtime, supervisor, child))
    })();
    let (runtime, supervisor, child) = match preflight {
        Ok(ready) => ready,
        Err(err) => {
            if !already_running && prepared.after.is_some() {
                let cleanup = Cancellation::after_hook()?;
                drop(cancellation);
                prepared.failed(paths, lock.take(), &cleanup.token);
            }
            if matches!(err.downcast_ref::<Error>(), Some(Error::Canceled(_))) {
                return Ok(ExitCode::from(130));
            }
            return Err(err);
        }
    };
    let lock = lock.expect("preflight acquired the run lock");
    let global = &prepared.global;
    let environment = &prepared.environment;
    let launch = &prepared.launch;
    let mut state = RunState::running(
        RunMode::Attached,
        launch.target.clone().unwrap_or_default(),
        child.id(),
        Some(child.id()),
        &overrides,
    );
    state.record_host(&launch.host);
    state.boot_id = process::boot_id();
    state.leader_start = process::start_time(child.id());
    // Wine is already running, so bailing out here would orphan it and free the lock.
    if let Err(err) = runtime.write_state(&state) {
        warn(err.into());
    }

    let name = &environment.name;
    let mut forced = false;
    let result = supervisor.wait(
        &runtime,
        launch,
        child,
        &mut state,
        Duration::from_secs(global.stop_timeout.into()),
        &paths.stats_log(name),
        |notice| match notice {
            Notice::Stopping => note(&format!("Stopping {name}, press Ctrl+C again to force it")),
            Notice::Killing => forced = true,
            Notice::Warning(err) => warn(err.into()),
            Notice::Heartbeat => {}
        },
    );
    drop(supervisor);
    let cleanup = if state.status != RunStatus::Running && prepared.after.is_some() {
        Some(Cancellation::after_hook()?)
    } else {
        None
    };
    drop(cancellation);
    if let Some(cleanup) = cleanup {
        let mut hook_state = state.clone();
        if forced {
            hook_state.exit_code = None;
        }
        if let Err(err) = after(paths, &mut prepared, lock, &hook_state, &cleanup) {
            warn(err);
        }
    }
    result?;
    Ok(ExitCode::from(
        state
            .exit_code
            .and_then(|code| u8::try_from(code).ok())
            .unwrap_or(1),
    ))
}

fn after(
    paths: &Paths,
    prepared: &mut launch::PreparedLaunch,
    lock: wineo_core::runtime::RunLock,
    state: &RunState,
    cancellation: &Cancellation,
) -> anyhow::Result<()> {
    let Some(guard) = prepared.after_guard(paths)? else {
        return Ok(());
    };
    drop(lock);
    if let Some(after) = &prepared.after
        && let Err(err) = after.run(
            &guard,
            state,
            &cancellation.token,
            Duration::from_secs(prepared.global.stop_timeout.into()),
            wineo_core::hooks::Output::Inherit,
        )
    {
        warn(err.into());
    }
    Ok(())
}

/// The terminal may already be gone after a SIGHUP, and a failed write must
/// not abort the shutdown.
fn note(message: &str) {
    let _ = writeln!(crate::output::stderr(), "{message}");
}

fn warn(err: anyhow::Error) {
    note(&format!("warning: {err:#}"));
}
