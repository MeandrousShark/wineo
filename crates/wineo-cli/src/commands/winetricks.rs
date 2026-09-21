use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use signal_hook::consts::SIGINT;
use signal_hook::iterator::Signals;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::launch::{merge_env, parse_env_assignment};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::prefix;
use wineo_core::runner::{self, Runner, with_menu_builder_disabled};
use wineo_core::runners::Runners;
use wineo_core::runtime::{Runtime, exit_code};

/// Variables wineo sets itself, so winetricks always drives the environment's
/// own runner and prefix whatever the caller's shell has exported. umu runs
/// its own winetricks against Proton, so it is never told which wine to use.
const MANAGED: [&str; 3] = ["WINEPREFIX", "WINE", "WINESERVER"];
const MANAGED_PROTON: [&str; 1] = ["WINEPREFIX"];

/// How often the cancellation wait checks for another Ctrl+C.
const POLL: Duration = Duration::from_millis(100);

#[derive(clap::Args)]
// clap's own -h would otherwise be caught before the name's trailing arguments,
// and `-h` is winetricks' own help.
#[command(disable_help_flag = true)]
pub struct Args {
    /// Print this help
    #[arg(long, action = clap::ArgAction::Help)]
    help: Option<bool>,

    /// Set an environment variable, overriding the config
    #[arg(short, long = "env", value_name = "KEY=VALUE", value_parser = parse_env_assignment)]
    env: Vec<(String, String)>,

    /// The environment, then what to pass winetricks, like `-q dotnet48`.
    /// Nothing after the name opens the winetricks GUI.
    #[arg(
        required = true,
        value_name = "NAME [WINETRICKS ARGS]...",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    rest: Vec<String>,
}

impl Args {
    /// Splits the environment name off the arguments winetricks gets. Keeping
    /// them one clap argument is what stops clap claiming winetricks' own
    /// flags, like `-q` or its `-e`.
    fn split(&self) -> (&str, &[String]) {
        let (name, forwarded) = self.rest.split_first().expect("at least one value");
        // An optional separator right after the name, for arguments that would
        // otherwise be hard to tell apart from wineo's own.
        match forwarded.split_first() {
            Some((first, rest)) if first == "--" => (name, rest),
            _ => (name, forwarded),
        }
    }
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let (name, forwarded) = args.split();
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, name)?;
    let runner = Runners::new(paths, &global)?.resolve(&environment.config.runner)?;

    let overrides: BTreeMap<String, String> = args.env.iter().cloned().collect();
    let env = merge_env(&runner, &global, &environment, &overrides)?;
    let managed: &[&str] = if runner.is_proton() {
        &MANAGED_PROTON
    } else {
        &MANAGED
    };
    if let Some(key) = managed.iter().find(|key| env.contains_key(**key)) {
        bail!("{key} is managed by wineo for this command and can't be overridden");
    }

    let prefix = environment.prefix(paths)?;
    let locks = Locks::new(paths);
    // Held until winetricks is done and the prefix is idle. Several winetricks
    // verbs wait for every wine process to exit, so nothing else may start one.
    let _guard = locks.acquire(Operation::Winetricks, name, &[&prefix])?;
    Environment::ensure_exists(paths, name)?;
    if !prefix::is_prefix(&runner.wine_prefix(&prefix)) {
        return Err(Error::MissingPrefix(prefix).into());
    }
    crate::commands::components::reconcile_config(paths, &global, &environment, &runner, &prefix)?;

    let runtime = Runtime::new(paths, name);
    if runtime.is_running()? {
        bail!("{name} is running, stop it before running winetricks");
    }
    if runtime.status().is_ok_and(|status| status.lost) {
        bail!(
            "{name}'s last run was lost, so wine may still be running in its prefix. Clean it up with `wineo stop {name}` first"
        );
    }
    runtime.check_resolved(name)?;

    crate::commands::desktop::reconcile(paths, &global, &environment, Some(&runner), &prefix)?;

    // umu ships winetricks and runs it inside the container, so a Proton
    // environment needs no winetricks on PATH and no WINE to point at.
    let mut command = if runner.is_proton() {
        let mut command = runner.command(&prefix, None)?;
        command.arg("winetricks");
        command
    } else {
        let winetricks =
            runner::find_on_path("winetricks").ok_or(Error::ProgramNotFound("winetricks"))?;
        let mut command = Command::new(&winetricks);
        command
            .env(
                "WINEDLLOVERRIDES",
                with_menu_builder_disabled(std::env::var("WINEDLLOVERRIDES").ok().as_deref()),
            )
            .env("WINE", runner.wine_path().expect("a wine runner has one"))
            .env(
                "WINESERVER",
                runner.wineserver_path().expect("a wine runner has one"),
            );
        command
    };
    // The prefix is reapplied last: for Proton it is the compatdata directory
    // umu makes `pfx/` under, which is the same path the config names.
    command
        .args(forwarded)
        .envs(&env)
        .env("WINEPREFIX", &prefix);

    // Counted rather than flagged, so a second Ctrl+C can force the cleanup.
    let interrupts = Arc::new(AtomicUsize::new(0));
    let mut signals = Signals::new([SIGINT]).context("failed to set up signal handling")?;
    let handle = signals.handle();
    let counter = interrupts.clone();
    let listener = thread::spawn(move || {
        for _ in signals.forever() {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    });

    // winetricks shares the process group and gets Ctrl+C itself.
    let status = command.status().context("failed to run winetricks")?;
    let canceled = interrupts.load(Ordering::Relaxed) > 0;
    let result = settle(
        &runner,
        &prefix,
        canceled.then_some((&interrupts, global.stop_timeout)),
    );

    handle.close();
    let _ = listener.join();
    result?;
    Ok(ExitCode::from(u8::try_from(exit_code(status)).unwrap_or(1)))
}

/// Waits for the prefix to go idle before the lock is released. A cancelled run
/// gets the clean stop first, then the timeout or another Ctrl+C forces it.
fn settle(
    runner: &Runner,
    prefix: &Path,
    canceled: Option<(&Arc<AtomicUsize>, u32)>,
) -> anyhow::Result<()> {
    // umu shares this process group, so a Ctrl+C already reached it, and once
    // it returns its container is gone. There is nothing left to settle.
    if runner.is_proton() {
        return Ok(());
    }
    let Some((interrupts, stop_timeout)) = canceled else {
        return Ok(runner.wait_idle(prefix)?);
    };

    eprintln!("Canceling winetricks, press Ctrl+C again to force it");
    // Wine only, so the prefix is what gets signalled and no group is needed.
    if let Err(err) = runner.end_session(prefix, None) {
        eprintln!("warning: {err}");
    }

    let (sender, idle) = mpsc::channel();
    let (waiting_runner, waiting_prefix) = (runner.clone(), prefix.to_owned());
    thread::spawn(move || {
        let _ = sender.send(waiting_runner.wait_idle(&waiting_prefix));
    });

    let forced = interrupts.load(Ordering::Relaxed) + 1;
    let deadline = Instant::now() + Duration::from_secs(stop_timeout.into());
    loop {
        match idle.recv_timeout(POLL) {
            Ok(result) => return Ok(result?),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if interrupts.load(Ordering::Relaxed) >= forced || Instant::now() >= deadline {
            break;
        }
    }

    eprintln!("Killing everything left in the prefix");
    runner.kill(prefix, None)?;
    Ok(runner.wait_idle(prefix)?)
}
