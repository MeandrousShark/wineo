//! Winetricks' installed record and process runner.

#![allow(clippy::too_many_arguments)]

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use rustix::process::Signal;

use crate::build_lock;
use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::error::Error;
use crate::files;
use crate::install::{self, Wanted};
use crate::installer::Output;
use crate::launch::{self, LaunchEvent, merge_env};
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::prefix;
use crate::process::{self, Escalation};
use crate::runner::{self, Runner, with_menu_builder_disabled};
use crate::runners::Runners;
use crate::runtime::Runtime;

const MANAGED: [&str; 3] = ["WINEPREFIX", "WINE", "WINESERVER"];
const MANAGED_PROTON: [&str; 1] = ["WINEPREFIX"];
const POLL: Duration = Duration::from_millis(100);
const UNCONFIRMED: &str = "winetricks' processes could not be confirmed stopped";

#[derive(Debug)]
pub enum WinetricksEvent {
    Installing(Vec<String>),
    /// One complete line of winetricks output. Only emitted when the output
    /// goes to a log file, because that file is what gets read back.
    OutputLine(String),
    /// Winetricks started one of the requested verbs.
    Progress(Step),
    /// The output log is created, emptied, and winetricks is running, so a
    /// reader can follow it without seeing a previous run's text. Only emitted
    /// for log output, and independent of whether winetricks prints anything.
    LogStarted,
    Stopping,
    Killing,
    Warning(String),
    Done(ExitStatus),
}

/// Which requested verb winetricks is on, counting from one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub verb: String,
    pub position: usize,
    pub total: usize,
}

/// Winetricks prints this from `w_try` before it runs each verb, top-level or
/// pulled in as a dependency: `Executing w_do_call gdiplus`.
const VERB_MARKER: &str = "Executing w_do_call ";

/// The verb a winetricks output line says it is starting, if any.
fn started_verb(line: &str) -> Option<&str> {
    line.trim_end()
        .strip_prefix(VERB_MARKER)?
        .split_whitespace()
        .next()
}

/// Follows winetricks output to find which requested verb is running.
pub struct Progress {
    verbs: Vec<String>,
    at: Option<usize>,
}

impl Progress {
    pub fn new(verbs: &[String]) -> Self {
        Self {
            verbs: verbs.to_vec(),
            at: None,
        }
    }

    /// Returns a step when `line` starts a requested verb later than the last
    /// one seen. Dependencies that are not requested are skipped, and a
    /// requested verb that shows up as a dependency of an earlier one is not
    /// counted again when its own turn comes.
    pub fn observe(&mut self, line: &str) -> Option<Step> {
        let verb = started_verb(line)?;
        let index = self.verbs.iter().position(|wanted| wanted == verb)?;
        if self.at.is_some_and(|at| index <= at) {
            return None;
        }
        self.at = Some(index);
        Some(Step {
            verb: verb.to_owned(),
            position: index + 1,
            total: self.verbs.len(),
        })
    }
}

/// Reads complete lines back from the log winetricks is writing.
struct LineTail {
    file: File,
    pending: Vec<u8>,
    /// Set after an oversized line was dropped, until its newline arrives.
    discarding: bool,
}

/// A line longer than this many bytes, not counting its newline, is dropped
/// rather than held forever.
const LINE_CAP: usize = 64 * 1024;

impl LineTail {
    fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            pending: Vec::new(),
            discarding: false,
        })
    }

    fn pump(&mut self, events: &mut impl FnMut(WinetricksEvent)) {
        let mut buf = [0u8; 8192];
        // Progress is best effort, so a failed read just ends this pass.
        while let Ok(read) = self.file.read(&mut buf) {
            if read == 0 {
                break;
            }
            self.pending.extend_from_slice(&buf[..read]);
            while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                // The rest of an oversized line is not a line of its own, and a
                // line that only became complete after passing the cap is
                // dropped like one that overflowed while waiting.
                if std::mem::take(&mut self.discarding) || end > LINE_CAP {
                    continue;
                }
                events(WinetricksEvent::OutputLine(
                    String::from_utf8_lossy(&line[..end]).into_owned(),
                ));
            }
            if self.discarding {
                self.pending.clear();
            } else if self.pending.len() > LINE_CAP {
                self.pending.clear();
                self.discarding = true;
            }
        }
    }
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
    _build_guard: Option<build_lock::Guard>,
}

fn context(
    paths: &Paths,
    global: GlobalConfig,
    environment: Environment,
) -> anyhow::Result<OperationContext> {
    let (runner, build_guard) =
        Runners::new(paths, &global)?.resolve_locked(paths, &environment.wine()?.runner)?;
    let prefix = environment.prefix(paths)?;
    Ok(OperationContext {
        global,
        environment,
        runner,
        prefix,
        _build_guard: build_guard,
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
    let loaded = GlobalConfig::load_or_default(paths)?;
    if environment.config.winetricks.is_empty() {
        return Ok(());
    }
    if let Some(warning) = loaded.warning {
        events(EnsureEvent::Winetricks(WinetricksEvent::Warning(warning)));
    }
    let context = context(paths, loaded.config, environment)?;
    let wanted = &context.environment.config.winetricks;

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

/// Builds a missing prefix, then installs the environment's missing declared
/// verbs with the same locking and idle checks as `ensure`.
pub fn ensure_in(
    paths: &Paths,
    name: &str,
    overrides: &BTreeMap<String, String>,
    token: &Token,
    output: EnsureOutput<'_>,
    mut events: impl FnMut(EnsureEvent),
) -> anyhow::Result<()> {
    let environment = Environment::load(paths, name)?;
    if environment.config.winetricks.is_empty() {
        return Ok(());
    }
    let global = GlobalConfig::load(paths)?;
    {
        let context = context(paths, global, environment)?;
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
    }
    ensure(paths, name, overrides, token, output, events)
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
    let Some(log) = files::read_to_string_optional(&wine_prefix.join("winetricks.log"))? else {
        return Ok(HashSet::new());
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
    let mut tail = None;
    match output {
        Output::Inherit if to_stderr => {
            command.stdout(Stdio::from(std::io::stderr()));
        }
        Output::Inherit => {}
        Output::Log(path) => {
            let log = File::create(path)
                .with_context(|| format!("failed to create {}", path.display()))?;
            tail = LineTail::open(path).ok();
            command
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log));
        }
    }

    let mut child = command.spawn().context("failed to run winetricks")?;
    process::quiet_core_dumps(&child);
    if tail.is_some() {
        events(WinetricksEvent::LogStarted);
    }
    // SIGINT, the way Ctrl+C asks a shell script to stop.
    let mut escalation = Escalation::new(Duration::from_secs(stop_timeout.into()), None);
    // Only a log to follow needs waking up for.
    let tick = tail.is_some().then_some(POLL);
    // Reported once the group is gone, so the stop still finishes.
    let mut cleanup = Ok(());
    let stopped = process::stop_group(&child, Signal::INT, &mut escalation, token, tick, |step| {
        match step {
            None => {
                if let Some(tail) = &mut tail {
                    tail.pump(&mut events);
                }
            }
            Some(process::Step::Ask) => events(WinetricksEvent::Stopping),
            Some(process::Step::Kill) => {
                events(WinetricksEvent::Killing);
                cleanup = runner.kill(prefix, None).map(drop);
            }
            Some(process::Step::Rekill) => {}
        }
        anyhow::Ok(())
    });
    let failed = match stopped {
        Ok(Ok(ended)) => Ok(ended),
        Ok(Err((err, ended))) => Err((err, ended == process::Ended::Stopped)),
        Err(err) => Err((
            anyhow::Error::from(err).context("failed to stop the winetricks process group"),
            false,
        )),
    };
    let ended = match failed {
        Ok(ended) => ended,
        Err((err, settled)) => {
            let reaped = process::kill_and_reap(&mut child);
            return Err(if settled && reaped {
                err
            } else {
                err.context(UNCONFIRMED)
            });
        }
    };
    if let Err(err) = cleanup {
        let _ = child.wait();
        return Err(err.into());
    }
    if ended == process::Ended::Unconfirmed {
        let _ = child.wait();
        return Err(Error::Unsettled(UNCONFIRMED.to_owned()).into());
    }
    if let Some(tail) = &mut tail {
        tail.pump(&mut events);
    }
    settle(runner, prefix, &child, token, &mut escalation, &mut events)?;
    let status = child.wait().context("failed to wait for winetricks")?;
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
    mut events: impl FnMut(WinetricksEvent),
) -> anyhow::Result<()> {
    let args: Vec<String> = std::iter::once("-q".to_owned())
        .chain(verbs.iter().cloned())
        .collect();
    let mut progress = Progress::new(verbs);
    let events = |event: WinetricksEvent| {
        let step = match &event {
            WinetricksEvent::OutputLine(line) => progress.observe(line),
            _ => None,
        };
        events(event);
        if let Some(step) = step {
            events(WinetricksEvent::Progress(step));
        }
    };
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

/// Waits for wine to go idle once winetricks is gone, with the leader still
/// unreaped so its group can be killed by number.
fn settle(
    runner: &Runner,
    prefix: &Path,
    leader: &Child,
    token: &Token,
    escalation: &mut Escalation,
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
        match escalation.follow(token, Instant::now()) {
            Some(process::Step::Ask) => events(WinetricksEvent::Stopping),
            Some(process::Step::Kill) => {
                events(WinetricksEvent::Killing);
                process::signal_led_group(leader, Signal::KILL)
                    .context("failed to signal the winetricks process group")?;
                runner.kill(prefix, None)?;
            }
            Some(process::Step::Rekill) | None => {}
        }
        if escalation.stopping() && !ending_started {
            ending_started = true;
            let (sender, receiver) = mpsc::channel();
            let (ending_runner, ending_prefix) = (runner.clone(), prefix.to_owned());
            thread::spawn(move || {
                let _ = sender.send(ending_runner.end_session(&ending_prefix, None));
            });
            ending = Some(receiver);
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

#[cfg(test)]
mod tests;
