use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use signal_hook::consts::SIGINT;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::launch::{self, Host, Launch, parse_env_assignment};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::process;
use wineo_core::runtime::{Runtime, exit_code};

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
    let mut launch = Launch::exec(
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
    let guard = locks.acquire(Operation::Launch, &environment.name, &[prefix])?;
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

    // A game folder stays in use until everything the program started has
    // exited, so a launcher that hands off to the game keeps it held. The
    // rest of the run needs only the folder locks.
    let whole_group = !folders.is_empty();
    if whole_group {
        process::become_subreaper().context("failed to watch the program's descendants")?;
    }
    let (code, interrupted) = run_attached(&launch)?;
    if whole_group {
        drop((
            guard,
            _build_guard,
            std::mem::take(&mut launch.build_guards),
        ));
        wait_for_descendants(&interrupted, global.stop_timeout, &args.program);
    }
    drop(folders);
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

/// How long descendants Ctrl+C killed get to exit before wineo stops
/// waiting for them regardless.
const KILL_GRACE: Duration = Duration::from_secs(2);

/// Waits until nothing the program started is left in wineo's group. Ctrl+C
/// reaches them already, since they share it; past the stop timeout, or on a
/// second Ctrl+C, they're killed, and the wait then ends even if they can't
/// be inspected. Only called once the program itself has been waited for,
/// so reaping can't take its status.
fn wait_for_descendants(interrupted: &AtomicBool, stop_timeout: u32, program: &str) {
    wait_with(
        interrupted,
        stop_timeout,
        program,
        process::own_group_has_live_descendants,
        process::kill_own_group_descendants,
        process::reap_adopted,
        |line| eprintln!("{line}"),
    );
}

/// `wait_for_descendants`, with how they're inspected, killed, and reaped,
/// and where its status lines go, passed in.
fn wait_with(
    interrupted: &AtomicBool,
    stop_timeout: u32,
    program: &str,
    mut live: impl FnMut() -> Option<bool>,
    mut kill: impl FnMut(),
    mut reap: impl FnMut(),
    mut say: impl FnMut(&str),
) {
    let mut waiting = false;
    let mut stopping: Option<Instant> = None;
    let mut forced = false;
    let mut killed: Option<Instant> = None;
    loop {
        reap();
        let live = live();
        if live == Some(false) {
            return;
        }
        if !waiting {
            waiting = true;
            say(&format!(
                "Waiting for programs {} started. Press Ctrl+C to stop them.",
                file_name(program)
            ));
        }
        if interrupted.swap(false, Ordering::Relaxed) {
            forced |= stopping.is_some();
            if stopping.is_none() {
                stopping = Some(Instant::now());
                say("Press Ctrl+C again to force them to stop.");
            }
        }
        if killed.is_none()
            && stopping.is_some_and(|at| {
                forced || at.elapsed() >= Duration::from_secs(stop_timeout.into())
            })
        {
            kill();
            killed = Some(Instant::now());
        }
        if killed.is_some_and(|at| live.is_none() || at.elapsed() >= KILL_GRACE) {
            reap();
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The last part of a program as given, Windows path or not.
fn file_name(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

/// Runs `launch` attached to the terminal. Also returns whether Ctrl+C has
/// been pressed since it started.
fn run_attached(launch: &Launch) -> anyhow::Result<(i32, Arc<AtomicBool>)> {
    // The program shares our process group and gets Ctrl+C itself. wineo keeps
    // waiting instead of exiting out from under it.
    let interrupted = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGINT, Arc::clone(&interrupted))
        .context("failed to set up signal handling")?;
    let mut child = launch
        .command()
        .spawn()
        .with_context(|| format!("failed to start {}", launch.host.launcher()))?;
    process::quiet_core_dumps(&child);
    let status = child
        .wait()
        .with_context(|| format!("failed to start {}", launch.host.launcher()))?;
    Ok((exit_code(status), interrupted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_ctrl_c_ends_the_wait_even_when_nothing_can_be_inspected() {
        let interrupted = AtomicBool::new(true);
        let mut kills = 0;
        let mut polls = 0;
        let mut said = Vec::new();
        wait_with(
            &interrupted,
            60,
            "C:\\Games\\launcher.exe",
            || {
                polls += 1;
                if polls == 2 {
                    interrupted.store(true, Ordering::Relaxed);
                }
                None
            },
            || kills += 1,
            || {},
            |line| said.push(line.to_owned()),
        );
        assert_eq!(kills, 1);
        assert_eq!(polls, 2);
        assert_eq!(
            said,
            [
                "Waiting for programs launcher.exe started. Press Ctrl+C to stop them.",
                "Press Ctrl+C again to force them to stop.",
            ]
        );
    }

    #[test]
    fn nothing_is_said_when_nothing_is_left() {
        let mut said = Vec::new();
        wait_with(
            &AtomicBool::new(false),
            60,
            "launcher.exe",
            || Some(false),
            || {},
            || {},
            |line| said.push(line.to_owned()),
        );
        assert!(said.is_empty());
    }

    #[test]
    fn ctrl_c_kills_what_outlives_the_stop_timeout() {
        let interrupted = AtomicBool::new(true);
        let mut kills = 0;
        let mut polls = 0;
        wait_with(
            &interrupted,
            0,
            "/games/launcher",
            || {
                polls += 1;
                (polls < 3).then_some(true).or(Some(false))
            },
            || kills += 1,
            || {},
            |_| {},
        );
        assert_eq!(kills, 1);
        assert_eq!(polls, 3);
    }
}
