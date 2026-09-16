use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::process::{ExitCode, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::launch::{Launch, Overrides, parse_env_assignment};
use wineo_core::paths::Paths;
use wineo_core::runtime::{RunMode, RunState, RunStatus, Runtime, exit_code};

#[derive(clap::Args)]
pub struct Args {
    /// Environment to run
    name: String,

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

enum Event {
    Exited(io::Result<ExitStatus>),
    Idle,
    Signal,
    StopTimeout,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, &args.name)?;
    let overrides = Overrides {
        target: args.target,
        env: args.env.into_iter().collect(),
        args: args.args,
    };
    let launch = Launch::target(paths, &global, &environment, &overrides)?;
    let runtime = Runtime::new(paths, &environment.name);
    let Some(_lock) = runtime.try_lock()? else {
        bail!("{} is already running", environment.name);
    };

    // Registered before wine starts so an early Ctrl+C still stops it cleanly.
    let mut signals =
        Signals::new([SIGINT, SIGTERM, SIGHUP]).context("failed to set up signal handling")?;
    let signal_handle = signals.handle();
    let (events, receiver) = mpsc::channel();
    let signal_events = events.clone();
    let signal_thread = thread::spawn(move || {
        for _ in signals.forever() {
            if signal_events.send(Event::Signal).is_err() {
                break;
            }
        }
    });

    // A separate process group keeps the terminal's Ctrl+C away from wine, so
    // wineo can ask the app to close instead of it being cut off.
    let mut child = launch
        .command()
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .context("failed to start wine")?;
    let mut state = RunState::running(
        RunMode::Attached,
        launch.target.clone().unwrap_or_default(),
        child.id(),
        overrides.args,
        overrides.env,
    );
    runtime.write_state(&state)?;

    let exit_events = events.clone();
    thread::spawn(move || {
        let _ = exit_events.send(Event::Exited(child.wait()));
    });

    let stop_timeout = Duration::from_secs(global.stop_timeout.into());
    let mut code = None;
    let mut deadline: Option<Instant> = None;
    let mut killed = false;
    loop {
        let event = match deadline {
            Some(deadline) if !killed => {
                match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(event) => event,
                    Err(RecvTimeoutError::Timeout) => Event::StopTimeout,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            _ => match receiver.recv() {
                Ok(event) => event,
                Err(_) => break,
            },
        };
        match event {
            Event::Exited(status) => {
                code = status.ok().map(exit_code);
                // Launchers often hand off to another process and exit, so the
                // run lasts until nothing is left running in the prefix.
                let idle_events = events.clone();
                let (runner, prefix) = (launch.runner.clone(), launch.prefix.clone());
                thread::spawn(move || {
                    let _ = runner.wait_idle(&prefix);
                    let _ = idle_events.send(Event::Idle);
                });
            }
            Event::Idle => break,
            Event::Signal if deadline.is_none() => {
                note(&format!(
                    "Stopping {}, press Ctrl+C again to force it",
                    environment.name
                ));
                deadline = Some(Instant::now() + stop_timeout);
                let (runner, prefix) = (launch.runner.clone(), launch.prefix.clone());
                thread::spawn(move || {
                    let _ = runner.end_session(&prefix);
                });
            }
            Event::Signal | Event::StopTimeout if !killed => {
                killed = true;
                if let Err(err) = launch.runner.kill(&launch.prefix) {
                    note(&format!("error: {err:#}"));
                }
            }
            Event::Signal | Event::StopTimeout => {}
        }
    }
    signal_handle.close();
    let _ = signal_thread.join();

    let status = match (deadline, code) {
        (Some(_), _) => RunStatus::Stopped,
        (None, Some(0)) => RunStatus::Exited,
        (None, _) => RunStatus::Failed,
    };
    state.finish(status, code);
    runtime.write_state(&state)?;
    Ok(ExitCode::from(
        code.and_then(|code| u8::try_from(code).ok()).unwrap_or(1),
    ))
}

/// The terminal may already be gone after a SIGHUP, and a failed write must
/// not abort the shutdown.
fn note(message: &str) {
    let _ = writeln!(io::stderr(), "{message}");
}
