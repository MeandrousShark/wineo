//! Winetricks' installed record and process runner.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use signal_hook::consts::SIGINT;
use signal_hook::iterator::Signals;

use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::error::Error;
use crate::launch::merge_env;
use crate::runner::{self, Runner, with_menu_builder_disabled};

const MANAGED: [&str; 3] = ["WINEPREFIX", "WINE", "WINESERVER"];
const MANAGED_PROTON: [&str; 1] = ["WINEPREFIX"];
const POLL: Duration = Duration::from_millis(100);
const SIGNAL_POLL: Duration = Duration::from_millis(10);

enum SignalControl {
    Sync(mpsc::Sender<()>),
    Stop,
}

/// Read winetricks' installed record from the actual Wine prefix. For Proton,
/// pass `runner.wine_prefix(configured_prefix)` so this reads `pfx/`.
pub fn installed(wine_prefix: &Path) -> io::Result<HashSet<String>> {
    let log = match fs::read_to_string(wine_prefix.join("winetricks.log")) {
        Ok(log) => log,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(err) => return Err(err),
    };
    Ok(log
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

pub fn missing(wanted: &[String], installed: &HashSet<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    wanted
        .iter()
        .filter(|verb| !installed.contains(*verb) && seen.insert(verb.as_str()))
        .cloned()
        .collect()
}

pub fn missing_for(runner: &Runner, prefix: &Path, wanted: &[String]) -> io::Result<Vec<String>> {
    Ok(missing(wanted, &installed(&runner.wine_prefix(prefix))?))
}

/// The caller holds the environment and prefix lock and has checked the
/// environment is idle.
pub fn install_missing(
    name: &str,
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    wanted: &[String],
    stop_timeout: u32,
) -> anyhow::Result<()> {
    install_missing_with_output(name, runner, prefix, env, wanted, stop_timeout, false)
}

pub fn install_missing_to_stderr(
    name: &str,
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    wanted: &[String],
    stop_timeout: u32,
) -> anyhow::Result<()> {
    install_missing_with_output(name, runner, prefix, env, wanted, stop_timeout, true)
}

fn install_missing_with_output(
    name: &str,
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    wanted: &[String],
    stop_timeout: u32,
    to_stderr: bool,
) -> anyhow::Result<()> {
    let missing = missing_for(runner, prefix, wanted)?;
    if missing.is_empty() {
        return Ok(());
    }
    check_available(runner)?;
    eprintln!("Installing {} with winetricks", missing.join(", "));
    if install_unattended_with_output(name, runner, prefix, env, &missing, stop_timeout, to_stderr)
        .is_err()
    {
        let remaining = missing_for(runner, prefix, wanted)?;
        let named = match remaining.as_slice() {
            [] => missing.join(" and "),
            _ => remaining.join(" and "),
        };
        bail!("winetricks failed installing {named} for {name}");
    }
    Ok(())
}

pub fn validate_verb(verb: &str) -> anyhow::Result<()> {
    if verb.is_empty() {
        bail!("Invalid winetricks verb: can't be empty.");
    }
    if verb.chars().any(char::is_whitespace) {
        bail!("Invalid winetricks verb {verb:?}: can't contain whitespace.");
    }
    if verb.starts_with('-') {
        bail!("Invalid winetricks verb {verb:?}: can't start with '-'.");
    }
    Ok(())
}

/// Merge configured and command-line variables, refusing variables wineo owns.
pub fn merged_env(
    runner: &Runner,
    global: &GlobalConfig,
    environment: &Environment,
    overrides: &BTreeMap<String, String>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let env = merge_env(runner, global, environment, overrides)?;
    let managed: &[&str] = if runner.is_proton() {
        &MANAGED_PROTON
    } else {
        &MANAGED
    };
    if let Some(key) = managed.iter().find(|key| env.contains_key(**key)) {
        bail!("{key} is managed by wineo for this command and can't be overridden");
    }
    Ok(env)
}

/// Check availability before building a missing prefix, as the CLI does today.
pub fn check_available(runner: &Runner) -> anyhow::Result<()> {
    if !runner.is_proton() {
        runner::find_on_path("winetricks").ok_or(Error::ProgramNotFound("winetricks"))?;
    }
    Ok(())
}

/// Run winetricks with the configured prefix and runner. The caller holds the
/// winetricks lock and checks that the prefix is idle before entering here.
pub fn run(
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    args: &[String],
    stop_timeout: u32,
) -> anyhow::Result<ExitStatus> {
    run_with_output(runner, prefix, env, args, stop_timeout, false)
}

fn run_with_output(
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    args: &[String],
    stop_timeout: u32,
    to_stderr: bool,
) -> anyhow::Result<ExitStatus> {
    // umu ships winetricks and runs it inside the container.
    let mut command = if runner.is_proton() {
        let mut command = runner.command(prefix, &[])?;
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
    command.args(args).envs(env).env("WINEPREFIX", prefix);
    if to_stderr {
        command.stdout(Stdio::from(std::io::stderr()));
    }

    // Counted rather than flagged, so a second Ctrl+C can force the cleanup.
    let interrupts = Arc::new(AtomicUsize::new(0));
    let mut signals = Signals::new([SIGINT]).context("failed to set up signal handling")?;
    let counter = interrupts.clone();
    let (control, requests) = mpsc::channel();
    let listener = thread::spawn(move || {
        loop {
            for _ in signals.pending() {
                counter.fetch_add(1, Ordering::Relaxed);
            }
            match requests.recv_timeout(SIGNAL_POLL) {
                Ok(SignalControl::Sync(ack)) => {
                    for _ in signals.pending() {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                    let _ = ack.send(());
                }
                Ok(SignalControl::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    });

    // winetricks shares the process group and gets Ctrl+C itself.
    let result = (|| {
        let status = command.status().context("failed to run winetricks")?;
        // A Ctrl+C that ended winetricks may not be counted yet, and missing
        // it would skip the clean stop.
        let (ack, synced) = mpsc::channel();
        control.send(SignalControl::Sync(ack))?;
        synced.recv()?;
        let canceled = interrupts.load(Ordering::Relaxed) > 0;
        settle(
            runner,
            prefix,
            canceled.then_some((&interrupts, stop_timeout)),
        )?;
        Ok(status)
    })();
    let _ = control.send(SignalControl::Stop);
    let _ = listener.join();
    result
}

/// Install the supplied verbs in one unattended call. The caller owns the
/// lock, idle check, and missing-verb decision.
pub fn install_unattended(
    name: &str,
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    verbs: &[String],
    stop_timeout: u32,
) -> anyhow::Result<()> {
    install_unattended_with_output(name, runner, prefix, env, verbs, stop_timeout, false)
}

fn install_unattended_with_output(
    name: &str,
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    verbs: &[String],
    stop_timeout: u32,
    to_stderr: bool,
) -> anyhow::Result<()> {
    let args: Vec<String> = std::iter::once("-q".to_owned())
        .chain(verbs.iter().cloned())
        .collect();
    if !run_with_output(runner, prefix, env, &args, stop_timeout, to_stderr)?.success() {
        bail!("winetricks failed for {name}");
    }
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_reads_whole_trimmed_lines() {
        let root = tempfile::tempdir().unwrap();
        assert!(installed(root.path()).unwrap().is_empty());
        fs::write(root.path().join("winetricks.log"), "").unwrap();
        assert!(installed(root.path()).unwrap().is_empty());
        fs::write(
            root.path().join("winetricks.log"),
            "vcrun2022_arm  \nwin10\nrenderer=vulkan\n  corefonts \n\n",
        )
        .unwrap();
        let verbs = installed(root.path()).unwrap();
        assert_eq!(verbs.len(), 4);
        for verb in ["vcrun2022_arm", "win10", "renderer=vulkan", "corefonts"] {
            assert!(verbs.contains(verb));
        }
        assert!(!verbs.contains("vcrun2022"));
    }

    #[test]
    fn missing_keeps_order_without_duplicates_or_prefix_matches() {
        let installed = HashSet::from(["vcrun2022_arm".to_owned(), "win10".to_owned()]);
        let wanted = [
            "win10",
            "vcrun2022",
            "corefonts",
            "vcrun2022",
            "vcrun2022_arm",
        ]
        .map(str::to_owned);
        assert_eq!(missing(&wanted, &installed), ["vcrun2022", "corefonts"]);
        let installed = HashSet::from(["vcrun2022".to_owned()]);
        assert_eq!(
            missing(&["vcrun2022_arm".to_owned()], &installed),
            ["vcrun2022_arm"]
        );
    }

    #[test]
    fn verb_shape() {
        for verb in ["vcrun2022", "win10", "renderer=vulkan"] {
            assert!(validate_verb(verb).is_ok());
        }
        for verb in ["", "two words", "line\nbreak", "--force", "-q"] {
            assert!(validate_verb(verb).is_err(), "{verb:?}");
        }
    }
}
