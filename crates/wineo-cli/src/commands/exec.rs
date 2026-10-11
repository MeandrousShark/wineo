use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::Context;
use signal_hook::consts::SIGINT;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::launch::{self, Host, Launch, Settling, parse_env_assignment};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::process;
use wineo_core::runtime::{Runtime, exit_code};
use wineo_core::supervisor::Supervisor;

use crate::commands::build;
use crate::commands::transfer::Cancellation;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to run the program in
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    /// Set an environment variable, overriding the config
    #[arg(short, long = "env", value_name = "KEY=VALUE", value_parser = parse_env_assignment)]
    env: Vec<(String, String)>,

    /// Windows path, Unix path, or a name wine knows like winecfg
    #[arg(value_hint = clap::ValueHint::FilePath)]
    program: String,

    /// Arguments passed to the program
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, &args.name)?;
    let env = args.env.into_iter().collect();
    let (runner, _build_guard) = launch::runner_locked(paths, &global, &environment)?;
    Launch::check_exec(paths, &global, &environment, &runner, &args.program, &env)?;
    let cancellation = Cancellation::new()?;
    match launch::ensure_prefix(
        paths,
        &global,
        &environment,
        &runner,
        &cancellation.token,
        |event| build::print_event(paths, &environment.name, event),
    ) {
        Err(err)
            if err
                .downcast_ref::<Error>()
                .is_some_and(|err| matches!(err, Error::Canceled(_))) =>
        {
            return Ok(ExitCode::from(130));
        }
        Err(err) => return Err(err),
        Ok(()) => {}
    }
    drop(cancellation);
    let launch = Launch::exec(
        paths,
        &global,
        &environment,
        &args.program,
        &args.args,
        &env,
    )?;
    let Host::Wine { prefix, .. } = &launch.host else {
        unreachable!("exec launches always run in a prefix");
    };
    let locks = Locks::new(paths);
    let _guard = locks.acquire(Operation::Launch, &environment.name, &[prefix])?;
    Environment::ensure_exists(paths, &environment.name)?;
    Runtime::new(paths, &environment.name).check_resolved(&environment.name)?;
    // Claiming a folder can wait on its state lock, so Ctrl+C still works
    // until the program starts and takes Ctrl+C itself.
    let cancellation = Cancellation::new()?;
    let folders = launch::protect(paths, &environment, &launch, &cancellation.token);
    drop(cancellation);
    let folders = match folders {
        Err(err) if matches!(err.downcast_ref(), Some(Error::Canceled(_))) => {
            return Ok(ExitCode::from(130));
        }
        result => result?,
    };
    launch::reconcile(paths, &global, &environment, &launch, |event| {
        build::print_event(paths, &environment.name, event)
    })?;

    let code = if folders.is_empty() {
        run_attached(&launch)?
    } else {
        run_settled(
            &launch,
            Duration::from_secs(global.stop_timeout.into()),
            &args.program,
        )?
    };
    drop(folders);
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

/// Runs a program from a GOG game folder until the prefix is idle, the way
/// `wineo run` does, so the folder stays in use while anything it started
/// still runs. Its own process group keeps the terminal's Ctrl+C away from
/// it: the first asks the prefix's programs to close, the next forces them.
fn run_settled(launch: &Launch, stop_timeout: Duration, program: &str) -> anyhow::Result<i32> {
    // Listening before the program starts, so an early Ctrl+C still counts.
    let supervisor = Supervisor::listen()?;
    let child = launch
        .command()
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .with_context(|| format!("failed to start {}", launch.host.launcher()))?;
    process::quiet_core_dumps(&child);
    let mut lines = WaitLines::default();
    let settled =
        launch::watch_settled(
            supervisor,
            &launch.host,
            child,
            stop_timeout,
            |seen| match seen {
                Settling::Warning(err) => note(&format!("warning: {:#}", anyhow::Error::from(err))),
                seen => lines.see(&seen, program, note),
            },
        );
    Ok(exit_code(settled.finish()?))
}

/// The lines saying why exec is still waiting, and how to stop it.
#[derive(Default)]
struct WaitLines {
    waiting: bool,
    stopping: bool,
}

impl WaitLines {
    fn see(&mut self, seen: &Settling, program: &str, mut say: impl FnMut(&str)) {
        match seen {
            Settling::StillWaiting => {
                self.waiting = true;
                say(&format!(
                    "Waiting for programs {} started. Press Ctrl+C to stop them.",
                    file_name(program)
                ));
                // A Ctrl+C before this line has already begun stopping them.
                if self.stopping {
                    say(FORCE_HINT);
                }
            }
            Settling::Stopping => {
                self.stopping = true;
                if self.waiting {
                    say(FORCE_HINT);
                }
            }
            _ => {}
        }
    }
}

const FORCE_HINT: &str = "Press Ctrl+C again to force them to stop.";

/// The last part of a program as given, Windows path or not.
fn file_name(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

fn note(message: &str) {
    let _ = writeln!(crate::output::stderr(), "{message}");
}

/// Runs `launch` attached to the terminal.
fn run_attached(launch: &Launch) -> anyhow::Result<i32> {
    // The program shares our process group and gets Ctrl+C itself. wineo keeps
    // waiting instead of exiting out from under it.
    signal_hook::flag::register(SIGINT, Arc::new(AtomicBool::new(false)))
        .context("failed to set up signal handling")?;
    let mut child = launch
        .command()
        .spawn()
        .with_context(|| format!("failed to start {}", launch.host.launcher()))?;
    process::quiet_core_dumps(&child);
    let status = child
        .wait()
        .with_context(|| format!("failed to start {}", launch.host.launcher()))?;
    Ok(exit_code(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAITING: &str = "Waiting for programs launcher.exe started. Press Ctrl+C to stop them.";

    fn said(seen: &[Settling]) -> Vec<String> {
        let mut lines = WaitLines::default();
        let mut said = Vec::new();
        for seen in seen {
            lines.see(seen, "C:\\Games\\launcher.exe", |line| {
                said.push(line.to_owned())
            });
        }
        said
    }

    #[test]
    fn a_ctrl_c_during_the_wait_offers_to_force() {
        assert_eq!(
            said(&[Settling::Exited, Settling::StillWaiting, Settling::Stopping]),
            [WAITING, FORCE_HINT]
        );
    }

    #[test]
    fn a_ctrl_c_before_the_waiting_line_offers_to_force_right_after_it() {
        assert_eq!(
            said(&[Settling::Exited, Settling::Stopping, Settling::StillWaiting]),
            [WAITING, FORCE_HINT]
        );
    }

    #[test]
    fn nothing_is_said_when_the_wait_ends_at_once() {
        assert!(said(&[Settling::Exited, Settling::Tick]).is_empty());
        assert!(said(&[Settling::Stopping, Settling::Tick]).is_empty());
    }
}
