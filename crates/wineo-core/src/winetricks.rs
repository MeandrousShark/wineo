//! Winetricks' installed record and process runner.

#![allow(clippy::too_many_arguments)]

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
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
    process::stop_group(&child, Signal::INT, &mut escalation, token, tick, |step| {
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
    })
    .context("failed to stop the winetricks process group")??;
    if let Err(err) = cleanup {
        let _ = child.wait();
        return Err(err.into());
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
mod tests {
    use crate::test_script::write_script;

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

    fn verbs(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn step(verb: &str, position: usize, total: usize) -> Step {
        Step {
            verb: verb.to_owned(),
            position,
            total,
        }
    }

    #[test]
    fn progress_follows_requested_verbs_in_order() {
        let mut progress = Progress::new(&verbs(&["corefonts", "gdiplus", "vcrun2022"]));
        assert_eq!(
            progress.observe("Using winetricks 20250102 with wine-10.0"),
            None
        );
        assert_eq!(
            progress.observe("Executing w_do_call corefonts"),
            Some(step("corefonts", 1, 3))
        );
        assert_eq!(progress.observe("Executing wine cabextract -q x.exe"), None);
        assert_eq!(
            progress.observe("Executing w_do_call gdiplus\r"),
            Some(step("gdiplus", 2, 3))
        );
        assert_eq!(
            progress.observe("Executing w_do_call vcrun2022"),
            Some(step("vcrun2022", 3, 3))
        );
    }

    #[test]
    fn progress_ignores_dependencies_that_were_not_requested() {
        let mut progress = Progress::new(&verbs(&["dotnet48"]));
        assert_eq!(progress.observe("Executing w_do_call dotnet40"), None);
        assert_eq!(
            progress.observe("Executing w_do_call dotnet_verifier"),
            None
        );
        assert_eq!(
            progress.observe("Executing w_do_call dotnet48"),
            Some(step("dotnet48", 1, 1))
        );
    }

    #[test]
    fn progress_does_not_repeat_or_go_backwards() {
        let mut progress = Progress::new(&verbs(&["dotnet48", "gdiplus", "corefonts"]));
        assert_eq!(
            progress.observe("Executing w_do_call dotnet48"),
            Some(step("dotnet48", 1, 3))
        );
        // A dependency of an earlier verb that is also requested counts once.
        assert_eq!(
            progress.observe("Executing w_do_call corefonts"),
            Some(step("corefonts", 3, 3))
        );
        assert_eq!(progress.observe("Executing w_do_call gdiplus"), None);
        assert_eq!(progress.observe("Executing w_do_call corefonts"), None);
        assert_eq!(progress.observe("Executing w_do_call dotnet48"), None);
    }

    #[test]
    fn progress_matches_whole_verb_names_only() {
        let mut progress = Progress::new(&verbs(&["vcrun2022", "renderer=vulkan"]));
        assert_eq!(progress.observe("Executing w_do_call vcrun2022_arm"), None);
        assert_eq!(progress.observe("Executing w_do_call vcrun"), None);
        assert_eq!(
            progress.observe("running Executing w_do_call vcrun2022"),
            None
        );
        assert_eq!(
            progress.observe("Executing w_do_call renderer=vulkan"),
            Some(step("renderer=vulkan", 2, 2))
        );
    }

    /// Runs `child` in a subprocess whose PATH starts with a fake winetricks.
    fn with_fake_winetricks(script_body: &str, child: &str) {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let script = bin.join("winetricks");
        write_script(&script, script_body);
        let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", child, "--nocapture"])
            .env("WINEO_WINETRICKS_PROGRESS_ROOT", root.path())
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
    fn install_reports_progress_from_the_log() {
        with_fake_winetricks(
            "#!/bin/sh\nfor verb in \"$@\"; do\n  case \"$verb\" in -q) ;; *)\n    echo \"Executing w_do_call dep\" >&2\n    echo \"Executing w_do_call $verb\" >&2 ;; esac\ndone\n",
            "winetricks::tests::install_progress_child",
        );
    }

    #[test]
    fn a_silent_install_still_announces_its_log() {
        with_fake_winetricks(
            "#!/bin/sh\nsleep 0.3\nprintf 'no newline'\n",
            "winetricks::tests::silent_install_child",
        );
    }

    #[test]
    fn silent_install_child() {
        let Some(root) = std::env::var_os("WINEO_WINETRICKS_PROGRESS_ROOT") else {
            return;
        };
        let prefix = std::path::PathBuf::from(root).join("prefix");
        fs::create_dir(&prefix).unwrap();
        let log = prefix.join("install.log");
        // A previous run's text must be gone by the time the log is announced.
        fs::write(&log, "stale\n").unwrap();
        let mut seen = Vec::new();
        install_unattended(
            "game",
            &Runner::fake("test"),
            &prefix,
            &BTreeMap::new(),
            &verbs(&["corefonts"]),
            1,
            &Token::new(),
            Output::Log(&log),
            |event| match event {
                WinetricksEvent::LogStarted => {
                    assert!(fs::read(&log).unwrap().is_empty());
                    seen.push("started");
                }
                WinetricksEvent::OutputLine(_) => seen.push("line"),
                WinetricksEvent::Progress(_) => seen.push("progress"),
                _ => {}
            },
        )
        .unwrap();
        assert_eq!(seen, ["started"]);
    }

    /// Feeds `chunks` to a tail one read at a time and returns the lines.
    fn pump_chunks(chunks: &[&[u8]]) -> Vec<String> {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("install.log");
        fs::write(&path, "").unwrap();
        let mut tail = LineTail::open(&path).unwrap();
        let mut writer = fs::OpenOptions::new().append(true).open(&path).unwrap();
        let mut lines = Vec::new();
        for chunk in chunks {
            io::Write::write_all(&mut writer, chunk).unwrap();
            tail.pump(&mut |event| {
                if let WinetricksEvent::OutputLine(line) = event {
                    lines.push(line);
                }
            });
        }
        lines
    }

    #[test]
    fn a_line_of_exactly_the_cap_is_kept() {
        let line = "x".repeat(LINE_CAP);
        assert_eq!(
            pump_chunks(&[line.as_bytes(), b"\n", b"next\n"]),
            [line.as_str(), "next"]
        );
        assert_eq!(
            pump_chunks(&[format!("{line}\nnext\n").as_bytes()]),
            [line.as_str(), "next"]
        );
    }

    #[test]
    fn a_line_one_over_the_cap_is_dropped_even_if_its_newline_comes_later() {
        let line = "x".repeat(LINE_CAP + 1);
        assert_eq!(
            pump_chunks(&[line.as_bytes(), b"\n", b"Executing w_do_call gdiplus\n"]),
            ["Executing w_do_call gdiplus"]
        );
        assert_eq!(
            pump_chunks(&[format!("{line}\nnext\n").as_bytes()]),
            ["next"]
        );
    }

    #[test]
    fn carriage_returns_stay_in_the_line_and_progress_ignores_them() {
        // Lines are raw apart from the newline, so a CRLF split across reads
        // arrives as one line ending in the carriage return.
        let lines = pump_chunks(&[b"abc\r", b"\n", b"Executing w_do_call gdiplus\r\n"]);
        assert_eq!(lines, ["abc\r", "Executing w_do_call gdiplus\r"]);
        let mut progress = Progress::new(&verbs(&["gdiplus"]));
        assert_eq!(
            lines.iter().find_map(|line| progress.observe(line)),
            Some(step("gdiplus", 1, 1))
        );
    }

    #[test]
    fn the_rest_of_an_oversized_line_is_dropped() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("install.log");
        fs::write(&path, "").unwrap();
        let mut tail = LineTail::open(&path).unwrap();
        let mut lines = Vec::new();
        let mut collect = |event: WinetricksEvent| {
            if let WinetricksEvent::OutputLine(line) = event {
                lines.push(line);
            }
        };
        let mut writer = fs::OpenOptions::new().append(true).open(&path).unwrap();
        // Multibyte text so any cut inside a character would show up.
        let long = "\u{3042}".repeat(LINE_CAP);
        io::Write::write_all(&mut writer, b"before\n").unwrap();
        io::Write::write_all(&mut writer, long.as_bytes()).unwrap();
        tail.pump(&mut collect);
        io::Write::write_all(
            &mut writer,
            b"suffix of the long line\nExecuting w_do_call gdiplus\n",
        )
        .unwrap();
        tail.pump(&mut collect);
        assert_eq!(lines, ["before", "Executing w_do_call gdiplus"]);
        let mut progress = Progress::new(&verbs(&["gdiplus"]));
        assert_eq!(
            lines.iter().find_map(|line| progress.observe(line)),
            Some(step("gdiplus", 1, 1))
        );
    }

    #[test]
    fn install_progress_child() {
        let Some(root) = std::env::var_os("WINEO_WINETRICKS_PROGRESS_ROOT") else {
            return;
        };
        let prefix = std::path::PathBuf::from(root).join("prefix");
        fs::create_dir(&prefix).unwrap();
        let log = prefix.join("install.log");
        let mut steps = Vec::new();
        let mut lines = 0;
        install_unattended(
            "game",
            &Runner::fake("test"),
            &prefix,
            &BTreeMap::new(),
            &verbs(&["corefonts", "gdiplus"]),
            1,
            &Token::new(),
            Output::Log(&log),
            |event| match event {
                WinetricksEvent::Progress(step) => steps.push(step),
                WinetricksEvent::OutputLine(_) => lines += 1,
                _ => {}
            },
        )
        .unwrap();
        assert_eq!(lines, 4);
        assert_eq!(steps, [step("corefonts", 1, 2), step("gdiplus", 2, 2)]);
    }

    #[test]
    fn token_cancels_from_another_thread_and_cleans_partial_work() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let script = bin.join("winetricks");
        write_script(
            &script,
            "#!/bin/sh\ntouch \"$WINEPREFIX/partial\"\ntrap 'rm -f \"$WINEPREFIX/partial\"; exit 130' INT\nwhile :; do sleep 0.05; done\n",
        );
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
