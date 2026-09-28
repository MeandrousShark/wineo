//! Winetricks' installed record and process runner.

#![allow(clippy::too_many_arguments)]

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process_group};

use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::error::Error;
use crate::install::{self, Wanted};
use crate::installer::Output;
use crate::launch::{self, LaunchEvent, merge_env};
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::prefix;
use crate::runner::{self, Runner, with_menu_builder_disabled};
use crate::runtime::Runtime;

const MANAGED: [&str; 3] = ["WINEPREFIX", "WINE", "WINESERVER"];
const MANAGED_PROTON: [&str; 1] = ["WINEPREFIX"];
const POLL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub enum WinetricksEvent {
    Installing(Vec<String>),
    OutputLine(String),
    Stopping,
    Killing,
    Warning(String),
    Done(ExitStatus),
}

#[derive(Debug, Clone, Copy)]
pub enum EnsureOutput<'a> {
    Inherit,
    Stderr,
    Log(&'a Path),
}

#[derive(Debug)]
pub enum EnsureEvent {
    Launch(LaunchEvent),
    Winetricks(WinetricksEvent),
}

struct OperationContext {
    global: GlobalConfig,
    environment: Environment,
    runner: Runner,
    prefix: std::path::PathBuf,
}

fn context(
    paths: &Paths,
    global: GlobalConfig,
    environment: Environment,
) -> anyhow::Result<OperationContext> {
    let runner = launch::runner(paths, &global, &environment)?;
    let prefix = environment.prefix(paths)?;
    Ok(OperationContext {
        global,
        environment,
        runner,
        prefix,
    })
}

/// Installs the missing verbs declared by an environment config.
pub fn ensure(
    paths: &Paths,
    name: &str,
    overrides: &BTreeMap<String, String>,
    token: &Token,
    output: EnsureOutput<'_>,
    mut events: impl FnMut(EnsureEvent),
) -> anyhow::Result<()> {
    let environment = Environment::load(paths, name)?;
    let global = GlobalConfig::load_or_default(paths)?;
    let context = context(paths, global, environment)?;
    let wanted = &context.environment.config.winetricks;
    if wanted.is_empty() {
        return Ok(());
    }

    let runtime = Runtime::new(paths, name);
    if runtime.is_running()? {
        return Err(Error::Running {
            name: name.to_owned(),
            action: "before running winetricks",
        }
        .into());
    }
    runtime.check_resolved(name)?;
    if !missing_for(&context.runner, &context.prefix, wanted)?.is_empty() {
        check_available(&context.runner)?;
    }
    let _guard = Locks::new(paths).acquire(Operation::Winetricks, name, &[&context.prefix])?;
    Environment::ensure_exists(paths, name)?;
    if !prefix::is_prefix(&context.runner.wine_prefix(&context.prefix)) {
        return Err(Error::MissingPrefix(context.prefix).into());
    }
    ensure_idle(&runtime, name, true)?;
    reconcile(paths, &context, &mut events)?;
    let env = merged_env(
        &context.runner,
        &context.global,
        &context.environment,
        overrides,
    )?;
    install_missing_for_context(name, &context, &env, token, output, |event| {
        events(EnsureEvent::Winetricks(event));
    })
}

/// Runs arbitrary winetricks arguments in an environment.
pub fn run_in(
    paths: &Paths,
    name: &str,
    overrides: &BTreeMap<String, String>,
    args: &[String],
    token: &Token,
    output: Output<'_>,
    mut events: impl FnMut(EnsureEvent),
) -> anyhow::Result<ExitStatus> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, name)?;
    let context = context(paths, global, environment)?;
    let env = merged_env(
        &context.runner,
        &context.global,
        &context.environment,
        overrides,
    )?;
    if !context.runner.is_proton()
        && !prefix::is_prefix(&context.runner.wine_prefix(&context.prefix))
    {
        check_available(&context.runner)?;
    }
    launch::ensure_prefix(
        paths,
        &context.global,
        &context.environment,
        &context.runner,
        token,
        |event| events(EnsureEvent::Launch(event)),
    )?;

    let _guard = Locks::new(paths).acquire(Operation::Winetricks, name, &[&context.prefix])?;
    Environment::ensure_exists(paths, name)?;
    if !prefix::is_prefix(&context.runner.wine_prefix(&context.prefix)) {
        return Err(Error::MissingPrefix(context.prefix).into());
    }
    ensure_idle(&Runtime::new(paths, name), name, false)?;
    reconcile(paths, &context, &mut events)?;
    run(
        &context.runner,
        &context.prefix,
        &env,
        args,
        context.global.stop_timeout,
        token,
        output,
        |event| events(EnsureEvent::Winetricks(event)),
    )
}

fn ensure_idle(runtime: &Runtime, name: &str, validate_style: bool) -> anyhow::Result<()> {
    if runtime.is_running()? {
        return Err(Error::Running {
            name: name.to_owned(),
            action: "before running winetricks",
        }
        .into());
    }
    if runtime.status().is_ok_and(|status| status.lost) {
        if validate_style {
            bail!("{name}'s last run was lost, clean it up with `wineo stop {name}` first");
        }
        bail!(
            "{name}'s last run was lost, so wine may still be running in its prefix. Clean it up with `wineo stop {name}` first"
        );
    }
    runtime.check_resolved(name)?;
    Ok(())
}

fn reconcile(
    paths: &Paths,
    context: &OperationContext,
    events: &mut impl FnMut(EnsureEvent),
) -> anyhow::Result<()> {
    let wanted = Wanted::from_config(&context.environment.config);
    let changes = install::reconcile(
        paths,
        &context.global,
        &context.environment.name,
        &context.runner,
        &context.prefix,
        &wanted,
    )?;
    if !changes.is_empty() {
        events(EnsureEvent::Launch(LaunchEvent::Components(changes)));
    }
    let changes = crate::desktop::reconcile(
        paths,
        &context.global,
        &context.environment,
        Some(&context.runner),
        &context.prefix,
    )?;
    if !changes.is_empty() {
        events(EnsureEvent::Launch(LaunchEvent::Desktop(changes)));
    }
    Ok(())
}

fn install_missing_for_context(
    name: &str,
    context: &OperationContext,
    env: &BTreeMap<String, String>,
    token: &Token,
    output: EnsureOutput<'_>,
    events: impl FnMut(WinetricksEvent),
) -> anyhow::Result<()> {
    let (output, to_stderr) = match output {
        EnsureOutput::Inherit => (Output::Inherit, false),
        EnsureOutput::Stderr => (Output::Inherit, true),
        EnsureOutput::Log(path) => (Output::Log(path), false),
    };
    install_missing_with_output(
        name,
        &context.runner,
        &context.prefix,
        env,
        &context.environment.config.winetricks,
        context.global.stop_timeout,
        token,
        output,
        to_stderr,
        events,
    )
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
    token: &Token,
    output: Output<'_>,
    events: impl FnMut(WinetricksEvent),
) -> anyhow::Result<()> {
    install_missing_with_output(
        name,
        runner,
        prefix,
        env,
        wanted,
        stop_timeout,
        token,
        output,
        false,
        events,
    )
}

pub fn install_missing_to_stderr(
    name: &str,
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    wanted: &[String],
    stop_timeout: u32,
    token: &Token,
    events: impl FnMut(WinetricksEvent),
) -> anyhow::Result<()> {
    install_missing_with_output(
        name,
        runner,
        prefix,
        env,
        wanted,
        stop_timeout,
        token,
        Output::Inherit,
        true,
        events,
    )
}

fn install_missing_with_output(
    name: &str,
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    wanted: &[String],
    stop_timeout: u32,
    token: &Token,
    output: Output<'_>,
    to_stderr: bool,
    mut events: impl FnMut(WinetricksEvent),
) -> anyhow::Result<()> {
    let missing = missing_for(runner, prefix, wanted)?;
    if missing.is_empty() {
        return Ok(());
    }
    check_available(runner)?;
    events(WinetricksEvent::Installing(missing.clone()));
    if let Err(err) = install_unattended_with_output(
        name,
        runner,
        prefix,
        env,
        &missing,
        stop_timeout,
        token,
        output,
        to_stderr,
        &mut events,
    ) {
        if err
            .downcast_ref::<Error>()
            .is_some_and(|err| matches!(err, Error::Canceled(_)))
        {
            return Err(err);
        }
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
    token: &Token,
    output: Output<'_>,
    events: impl FnMut(WinetricksEvent),
) -> anyhow::Result<ExitStatus> {
    run_with_output(
        runner,
        prefix,
        env,
        args,
        stop_timeout,
        token,
        output,
        false,
        events,
    )
}

fn run_with_output(
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    args: &[String],
    stop_timeout: u32,
    token: &Token,
    output: Output<'_>,
    to_stderr: bool,
    mut events: impl FnMut(WinetricksEvent),
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
    command
        .args(args)
        .envs(env)
        .env("WINEPREFIX", prefix)
        .stdin(Stdio::null())
        .process_group(0);
    match output {
        Output::Inherit if to_stderr => {
            command.stdout(Stdio::from(std::io::stderr()));
        }
        Output::Inherit => {}
        Output::Log(path) => {
            let log = File::create(path)
                .with_context(|| format!("failed to create {}", path.display()))?;
            command
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log));
        }
    }

    let mut child = command.spawn().context("failed to run winetricks")?;
    let group = child.id();
    let mut stopping = None;
    let mut killing = false;
    let status = loop {
        if token.is_canceled() && stopping.is_none() {
            stopping = Some(Instant::now());
            events(WinetricksEvent::Stopping);
            signal_group(group, Signal::INT)?;
        }
        if stopping.is_some_and(|started| {
            token.is_forced() || started.elapsed() >= Duration::from_secs(stop_timeout.into())
        }) && !killing
        {
            killing = true;
            events(WinetricksEvent::Killing);
            signal_group(group, Signal::KILL)?;
            runner.kill(prefix, None)?;
        }
        if let Some(status) = child.try_wait().context("failed to wait for winetricks")? {
            break status;
        }
        token.wait_timeout(POLL);
    };
    settle(
        runner,
        prefix,
        group,
        token,
        stop_timeout,
        stopping,
        killing,
        &mut events,
    )?;
    events(WinetricksEvent::Done(status));
    if token.is_canceled() {
        return Err(Error::Canceled("winetricks").into());
    }
    Ok(status)
}

/// Install the supplied verbs in one unattended call. The caller owns the
/// lock, idle check, and missing-verb decision.
#[allow(clippy::too_many_arguments)]
pub fn install_unattended(
    name: &str,
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    verbs: &[String],
    stop_timeout: u32,
    token: &Token,
    output: Output<'_>,
    events: impl FnMut(WinetricksEvent),
) -> anyhow::Result<()> {
    install_unattended_with_output(
        name,
        runner,
        prefix,
        env,
        verbs,
        stop_timeout,
        token,
        output,
        false,
        events,
    )
}

fn install_unattended_with_output(
    name: &str,
    runner: &Runner,
    prefix: &Path,
    env: &BTreeMap<String, String>,
    verbs: &[String],
    stop_timeout: u32,
    token: &Token,
    output: Output<'_>,
    to_stderr: bool,
    events: impl FnMut(WinetricksEvent),
) -> anyhow::Result<()> {
    let args: Vec<String> = std::iter::once("-q".to_owned())
        .chain(verbs.iter().cloned())
        .collect();
    if !run_with_output(
        runner,
        prefix,
        env,
        &args,
        stop_timeout,
        token,
        output,
        to_stderr,
        events,
    )?
    .success()
    {
        bail!("winetricks failed for {name}");
    }
    Ok(())
}

fn settle(
    runner: &Runner,
    prefix: &Path,
    group: u32,
    token: &Token,
    stop_timeout: u32,
    mut stopping: Option<Instant>,
    mut killing: bool,
    events: &mut impl FnMut(WinetricksEvent),
) -> anyhow::Result<()> {
    if runner.is_proton() {
        return Ok(());
    }
    let (sender, idle) = mpsc::channel();
    let (waiting_runner, waiting_prefix) = (runner.clone(), prefix.to_owned());
    thread::spawn(move || {
        let _ = sender.send(waiting_runner.wait_idle(&waiting_prefix));
    });
    let mut ending = None;
    let mut ending_started = false;
    let mut idle_result = None;
    loop {
        if idle_result.is_none() {
            match idle.recv_timeout(POLL) {
                Ok(result) => idle_result = Some(result),
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
        } else {
            token.wait_timeout(POLL);
        }
        if token.is_canceled() && stopping.is_none() {
            stopping = Some(Instant::now());
            events(WinetricksEvent::Stopping);
        }
        if stopping.is_some() && !ending_started {
            ending_started = true;
            let (sender, receiver) = mpsc::channel();
            let (ending_runner, ending_prefix) = (runner.clone(), prefix.to_owned());
            thread::spawn(move || {
                let _ = sender.send(ending_runner.end_session(&ending_prefix, None));
            });
            ending = Some(receiver);
        }
        if stopping.is_some_and(|started| {
            token.is_forced() || started.elapsed() >= Duration::from_secs(stop_timeout.into())
        }) && !killing
        {
            killing = true;
            events(WinetricksEvent::Killing);
            signal_group(group, Signal::KILL)?;
            runner.kill(prefix, None)?;
        }
        if let Some(receiver) = ending.as_ref() {
            match receiver.try_recv() {
                Ok(Ok(())) | Err(mpsc::TryRecvError::Disconnected) => ending = None,
                Ok(Err(err)) => {
                    events(WinetricksEvent::Warning(err.to_string()));
                    ending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if ending.is_none()
            && let Some(result) = idle_result
        {
            return Ok(result?);
        }
    }
    Ok(())
}

fn signal_group(group: u32, signal: Signal) -> anyhow::Result<()> {
    let pid = Pid::from_raw(i32::try_from(group)?).context("invalid winetricks process group")?;
    match kill_process_group(pid, signal) {
        Ok(()) | Err(Errno::SRCH) => Ok(()),
        Err(err) => Err(io::Error::from(err))
            .with_context(|| format!("failed to signal winetricks process group {group}")),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

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

    #[test]
    fn token_cancels_from_another_thread_and_cleans_partial_work() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let script = bin.join("winetricks");
        fs::write(
            &script,
            "#!/bin/sh\ntouch \"$WINEPREFIX/partial\"\ntrap 'rm -f \"$WINEPREFIX/partial\"; exit 130' INT\nwhile :; do sleep 0.05; done\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "winetricks::tests::token_cancel_child",
                "--nocapture",
            ])
            .env("WINEO_WINETRICKS_CANCEL_ROOT", root.path())
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn token_cancel_child() {
        let Some(root) = std::env::var_os("WINEO_WINETRICKS_CANCEL_ROOT") else {
            return;
        };
        let prefix = std::path::PathBuf::from(root).join("prefix");
        fs::create_dir(&prefix).unwrap();
        let token = Token::new();
        let request = token.clone();
        let marker = prefix.join("partial");
        let waiter = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !marker.exists() {
                assert!(Instant::now() < deadline, "winetricks never started");
                thread::sleep(Duration::from_millis(10));
            }
            request.cancel();
        });
        let mut events = Vec::new();
        let result = run(
            &Runner::fake("test"),
            &prefix,
            &BTreeMap::new(),
            &["-q".to_owned(), "corefonts".to_owned()],
            1,
            &token,
            Output::Log(&prefix.join("output.log")),
            |event| events.push(event),
        );
        waiter.join().unwrap();
        assert!(
            result
                .unwrap_err()
                .downcast_ref::<Error>()
                .is_some_and(|err| matches!(err, Error::Canceled("winetricks")))
        );
        assert!(!prefix.join("partial").exists());
        assert!(
            events
                .iter()
                .any(|event| matches!(event, WinetricksEvent::Stopping))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, WinetricksEvent::Done(_)))
        );
    }
}
