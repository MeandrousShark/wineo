//! Owning a run: waiting for wine, handling stop requests, and recording state.
//! `run` does this attached to the terminal, and `start` hands it to a detached
//! `wineo __supervise` process.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process, setsid, waitid};
use serde::{Deserialize, Serialize};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGUSR1};
use signal_hook::iterator::exfiltrator::raw::WithRawSiginfo;
use signal_hook::iterator::{Handle, SignalsInfo};

use crate::cancel::Token;
use crate::error::{self, Error, IoContext, Result};
use crate::launch::{Host, Launch, Overrides};
use crate::log::{Cap, Log};
use crate::operation::{Locks, Operation};
use crate::prefix;
use crate::runtime::{RunMode, RunState, RunStatus, Runtime, exit_code};

mod steam;

pub(crate) use steam::request as request_steam;

/// The hidden command a detached supervisor runs as.
pub const SUPERVISE_COMMAND: &str = "__supervise";

/// Runs the private supervisor entrypoint before a caller parses arguments.
pub fn dispatch_if_supervise() -> Option<Result<()>> {
    if let Some(result) = steam::dispatch_request_entry() {
        return Some(result);
    }
    std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == SUPERVISE_COMMAND)
        .then(serve)
}

/// How often a kill repeats until the run ends. A program that was still
/// starting up can be invisible to wineserver the first time.
const KILL_RETRY: Duration = Duration::from_secs(1);

enum Event {
    Exited(io::Result<ExitStatus>),
    WaitFailed(Error),
    Idle(Result<()>),
    Stop,
    Force,
    StopTimeout,
    /// Nothing else happened for `HEARTBEAT_INTERVAL`, so the run is still live.
    Heartbeat,
    Pulse,
}

/// How long the run waits between events before it counts as a heartbeat.
/// Reuses the loop's own wait rather than a separate timer, so a live run
/// costs one wakeup a minute rather than none.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

/// What the run loop tells whoever is watching it.
#[derive(Debug)]
pub enum Notice {
    /// Programs were asked to close. Another stop request forces it.
    Stopping,
    /// Everything left in the prefix is being killed.
    Killing,
    /// Something failed without ending the run.
    Warning(Error),
    /// The run is still live, for whoever wants to record it.
    Heartbeat,
}

/// What watching a program reports along the way.
#[derive(Debug)]
pub enum Watched {
    /// The program exited, with its code when it could be collected. What it
    /// started may still be running, and is waited for next.
    Exited(Option<i32>),
    Notice(Notice),
}

/// How a watch knows that everything the program started has stopped.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Settle {
    /// Once wineserver goes idle. A Proton run is over when umu returns.
    Idle,
    /// Only once that is confirmed, including that nothing is left in a Proton
    /// program's process group.
    Confirmed,
}

/// How a watched program and everything it started came to an end.
#[derive(Debug)]
pub(crate) struct Ending {
    pub exit_code: Option<i32>,
    /// The same, as the status it came from, for a caller that reports it.
    pub exit_status: Option<ExitStatus>,
    /// A stop was requested, so an exit code can't tell a kill apart.
    pub stopped: bool,
    /// Err when it could not be confirmed that nothing is left running.
    pub settled: Result<()>,
}

/// Owns a run until it ends.
pub struct Supervisor {
    events: Receiver<Event>,
    sender: Sender<Event>,
    signals: Option<Handle>,
    signal_thread: Option<JoinHandle<()>>,
    signal_stop: Option<Arc<AtomicBool>>,
    token: Option<Token>,
}

impl Supervisor {
    /// Starts listening for stop signals, before wine starts so an early one
    /// still stops it cleanly. SIGINT, SIGTERM, and SIGHUP ask for a clean stop
    /// and force it when repeated. SIGUSR1 forces it right away.
    pub fn listen() -> Result<Self> {
        // A quick second Ctrl+C must survive arriving before the listener polls.
        let mut signals = SignalsInfo::<WithRawSiginfo>::new([SIGINT, SIGTERM, SIGHUP, SIGUSR1])
            .context(|| "failed to set up signal handling".to_owned())?;
        let handle = signals.handle();
        let (sender, events) = mpsc::channel();
        let signal_sender = sender.clone();
        let signal_stop = Arc::new(AtomicBool::new(false));
        let thread_stop = signal_stop.clone();
        let signal_thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                // Only SIGINT counts every delivery; other signals keep their
                // original once-per-batch behavior.
                let mut seen = BTreeSet::new();
                for received in signals.pending() {
                    let signal = received.si_signo;
                    if signal != SIGINT && !seen.insert(signal) {
                        continue;
                    }
                    let event = if signal == SIGUSR1 {
                        Event::Force
                    } else {
                        Event::Stop
                    };
                    if signal_sender.send(event).is_err() {
                        return;
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        Ok(Self {
            events,
            sender,
            signals: Some(handle),
            signal_thread: Some(signal_thread),
            signal_stop: Some(signal_stop),
            token: None,
        })
    }

    /// Watches stop requests supplied by a caller rather than process signals.
    pub fn with_token(token: &Token) -> Self {
        let (sender, events) = mpsc::channel();
        Self {
            events,
            sender,
            signals: None,
            signal_thread: None,
            signal_stop: None,
            token: Some(token.clone()),
        }
    }

    /// Waits until the program has exited and nothing is left running in the
    /// prefix, recording in `state` when the program exits and how the run
    /// ended, and appending the finished session to `stats_log`.
    #[allow(clippy::too_many_arguments)]
    pub fn wait(
        &self,
        runtime: &Runtime,
        launch: &Launch,
        child: Child,
        state: &mut RunState,
        stop_timeout: Duration,
        stats_log: &Path,
        mut notify: impl FnMut(Notice),
    ) -> Result<()> {
        let group = state.group;
        let outlives = launch.host.outlives_program();
        // A run is over once the prefix looks idle, whether or not that could
        // be confirmed, so `settled` is not looked at.
        let ending = self.watch(
            &launch.host,
            group,
            state.boot_id.clone(),
            state.leader_start,
            child,
            stop_timeout,
            Settle::Idle,
            None,
            |watched| match watched {
                Watched::Exited(code) => {
                    state.program_exited(code, outlives);
                    if let Err(err) = runtime.write_state(state) {
                        notify(Notice::Warning(err));
                    }
                }
                Watched::Notice(Notice::Heartbeat) => {
                    state.heartbeat();
                    if let Err(err) = runtime.write_state(state) {
                        notify(Notice::Warning(err));
                    }
                }
                Watched::Notice(notice) => notify(notice),
            },
        );

        if let Err(error) = ending.settled {
            // Keep the run marked live so ordinary commands cannot pass a
            // process we failed to observe or settle. A failure here must not
            // hide the settle error, which is the one worth reporting.
            if let Err(write_err) = runtime.write_state(state) {
                notify(Notice::Warning(write_err));
            }
            return Err(error);
        }

        // Exit codes can't tell a kill from a normal exit, so a stop is only
        // known from the request.
        let status = match (ending.stopped, state.exit_code) {
            (true, _) => RunStatus::Stopped,
            (false, Some(0)) => RunStatus::Exited,
            (false, _) => RunStatus::Failed,
        };
        state.finish(status);
        runtime.write_state(state)?;
        // A failed append must not fail the run: it already happened.
        if let Err(err) = crate::stats::append(stats_log, &session(state, status)) {
            notify(Notice::Warning(err));
        }
        Ok(())
    }

    /// Waits for `child` and then for everything it started, asking programs
    /// to close on the first stop request and killing them on the next one or
    /// once `stop_timeout` passes. Kills repeat until the prefix is idle, or
    /// until `give_up` has passed since the first one, when a caller has to
    /// report rather than wait on for good.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn watch(
        &self,
        host: &Host,
        group: Option<u32>,
        boot_id: Option<String>,
        leader_start: Option<u64>,
        mut child: Child,
        stop_timeout: Duration,
        settle: Settle,
        give_up: Option<Duration>,
        mut watched: impl FnMut(Watched),
    ) -> Ending {
        let sender = self.sender.clone();
        let (reap, release_child) = mpsc::channel();
        let (reaped, child_reaped) = mpsc::channel::<()>();
        thread::spawn(move || {
            let pid = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
            if let Some(pid) = pid {
                loop {
                    match release_child.try_recv() {
                        Ok(()) => {
                            let _ = child.wait();
                            let _ = reaped.send(());
                            return;
                        }
                        Err(TryRecvError::Disconnected) => return,
                        Err(TryRecvError::Empty) => {}
                    }
                    match waitid(
                        WaitId::Pid(pid),
                        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
                    ) {
                        Ok(Some(status)) => {
                            let _ = sender.send(Event::Exited(Ok(wait_status(status))));
                            break;
                        }
                        Ok(None) => thread::sleep(Duration::from_millis(20)),
                        Err(err) => {
                            let err = io::Error::from(err);
                            if err.kind() == io::ErrorKind::Interrupted {
                                continue;
                            }
                            let _ = sender.send(Event::WaitFailed(Error::Io {
                                context: "failed to inspect the launched process".to_owned(),
                                source: err,
                            }));
                            return;
                        }
                    }
                }
            } else {
                let _ = sender.send(Event::WaitFailed(Error::Unsettled(
                    "invalid child process id".to_owned(),
                )));
                return;
            }
            if release_child.recv().is_err() {
                return;
            }
            let _ = child.wait();
            let _ = reaped.send(());
        });

        let mut exit = None;
        let mut exit_status = None;
        let mut observed_exit = false;
        let mut stopping = false;
        let mut saw_cancel = false;
        let mut saw_force = false;
        let mut heartbeat_at = Instant::now() + HEARTBEAT_INTERVAL;
        let mut killing_since: Option<Instant> = None;
        // Tells when the request to close is done, since it runs wine too.
        let mut ending_session: Option<Receiver<()>> = None;
        // When the clean stop gives up, or the next kill is due.
        let mut deadline: Option<Instant> = None;
        let lost = || Err(Error::Unsettled("lost track of the program".to_owned()));
        let settled = loop {
            let event = if self.token.as_ref().is_some_and(Token::is_forced) && !saw_force {
                saw_force = true;
                saw_cancel = true;
                Event::Force
            } else if self.token.as_ref().is_some_and(Token::is_canceled) && !saw_cancel {
                saw_cancel = true;
                Event::Stop
            } else if self.token.is_some() {
                let now = Instant::now();
                let until = deadline.unwrap_or(heartbeat_at);
                let timeout = until
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(20));
                match self.events.recv_timeout(timeout) {
                    Ok(event) => event,
                    Err(RecvTimeoutError::Timeout)
                        if deadline.is_some_and(|d| d <= Instant::now()) =>
                    {
                        Event::StopTimeout
                    }
                    Err(RecvTimeoutError::Timeout) if heartbeat_at <= Instant::now() => {
                        heartbeat_at = Instant::now() + HEARTBEAT_INTERVAL;
                        Event::Heartbeat
                    }
                    Err(RecvTimeoutError::Timeout) => Event::Pulse,
                    Err(RecvTimeoutError::Disconnected) => break lost(),
                }
            } else {
                match deadline {
                    Some(deadline) => match self
                        .events
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    {
                        Ok(event) => event,
                        Err(RecvTimeoutError::Timeout) => Event::StopTimeout,
                        Err(RecvTimeoutError::Disconnected) => break lost(),
                    },
                    None => match self.events.recv_timeout(HEARTBEAT_INTERVAL) {
                        Ok(event) => event,
                        Err(RecvTimeoutError::Timeout) => Event::Heartbeat,
                        Err(RecvTimeoutError::Disconnected) => break lost(),
                    },
                }
            };
            match event {
                Event::Pulse => continue,
                Event::Heartbeat => watched(Watched::Notice(Notice::Heartbeat)),
                Event::Exited(status) => {
                    observed_exit = true;
                    exit_status = status.ok();
                    exit = exit_status.map(exit_code);
                    watched(Watched::Exited(exit));
                    // Launchers often hand off to another process and exit, so
                    // this lasts until nothing is left running in the prefix.
                    let sender = self.sender.clone();
                    let (host, boot_id) = (host.clone(), boot_id.clone());
                    thread::spawn(move || {
                        let result = match settle {
                            Settle::Idle => host.wait_idle(group, boot_id.as_deref(), leader_start),
                            Settle::Confirmed => {
                                host.wait_settled_live(group, boot_id.as_deref(), leader_start)
                            }
                        };
                        let _ = sender.send(Event::Idle(result));
                    });
                }
                Event::WaitFailed(error) => break Err(error),
                Event::Idle(result) => break result,
                Event::Stop if !stopping => {
                    stopping = true;
                    deadline = Some(Instant::now() + stop_timeout);
                    watched(Watched::Notice(Notice::Stopping));
                    let (host, boot_id) = (host.clone(), boot_id.clone());
                    let (done, ended) = mpsc::channel();
                    ending_session = Some(ended);
                    thread::spawn(move || {
                        let _ = host.end_session_live(group, boot_id.as_deref(), leader_start);
                        let _ = done.send(());
                    });
                }
                Event::Stop | Event::Force | Event::StopTimeout => {
                    stopping = true;
                    let since = *killing_since.get_or_insert_with(|| {
                        watched(Watched::Notice(Notice::Killing));
                        Instant::now()
                    });
                    if let Some(limit) = give_up
                        && since.elapsed() >= limit
                    {
                        break Err(Error::StillRunning((stop_timeout + limit).as_secs().max(1)));
                    }
                    match host.kill_live(group, boot_id.as_deref(), leader_start) {
                        Ok(
                            crate::process::GroupOwnership::Owned
                            | crate::process::GroupOwnership::Gone,
                        ) => {}
                        Ok(crate::process::GroupOwnership::Unknown) => {
                            watched(Watched::Notice(Notice::Warning(Error::Unsettled(
                                "process-group ownership could not be confirmed".to_owned(),
                            ))))
                        }
                        Err(err) => watched(Watched::Notice(Notice::Warning(err))),
                    }
                    deadline = Some(Instant::now() + KILL_RETRY);
                }
            }
        };
        // The request to close can still be starting wine when the prefix
        // goes idle, so it gets a moment to finish before this counts as over.
        if let Some(ended) = ending_session {
            let _ = ended.recv_timeout(stop_timeout.max(KILL_RETRY));
        }
        if observed_exit {
            let _ = reap.send(());
            let _ = child_reaped.recv();
        }
        Ending {
            exit_code: exit,
            exit_status,
            stopped: stopping,
            settled,
        }
    }
}

/// The session a finished run adds to its environment's play log.
fn session(state: &RunState, status: RunStatus) -> crate::stats::Session {
    let end = match status {
        RunStatus::Exited => crate::stats::End::Exited,
        RunStatus::Failed => crate::stats::End::Failed,
        RunStatus::Stopped => crate::stats::End::Stopped,
        RunStatus::Running => unreachable!("a finished run is never still running"),
    };
    crate::stats::Session::new(
        state.target.clone(),
        state.started_at,
        state.ended_at.unwrap_or(state.started_at),
        end,
    )
}

fn wait_status(status: rustix::process::WaitIdStatus) -> ExitStatus {
    let raw = if let Some(code) = status.exit_status() {
        code << 8
    } else if let Some(signal) = status.terminating_signal() {
        signal | i32::from(status.dumped()) << 7
    } else {
        0
    };
    ExitStatus::from_raw(raw)
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        if let Some(stop) = &self.signal_stop {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(signals) = &self.signals {
            signals.close();
        }
        if let Some(thread) = self.signal_thread.take() {
            let _ = thread.join();
        }
    }
}

/// Everything a detached supervisor needs. `start` resolves it beforehand, so
/// config problems are reported there.
#[derive(Debug, Serialize, Deserialize)]
pub struct Spec {
    pub claim: Claim,
    pub runtime_dir: PathBuf,
    pub stats_log: PathBuf,
    pub stop_timeout: u32,
    pub log_limit_mb: u32,
    pub launch: Launch,
    pub overrides: Overrides,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<crate::launch::AfterHook>,
}

/// What a detached supervisor needs to hold its own operation lock and confirm
/// the environment is still there once it has it.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claim {
    pub name: String,
    pub locks: Locks,
    pub config: PathBuf,
    #[serde(default)]
    pub builds: Vec<crate::build_lock::Lock>,
    /// The in-use locks of the GOG folders the run uses.
    #[serde(default)]
    pub folders: Vec<crate::gog::folder::InUseLock>,
}

/// How handing a run to a new supervisor went.
#[derive(Debug, PartialEq, Eq)]
pub enum Started {
    /// Wine is running with this pid.
    Running(u32),
    /// Something else started the environment first.
    AlreadyRunning,
    SteamRequested,
    SteamAdopted,
}

/// The single line a supervisor sends back to `start`.
#[derive(Debug, PartialEq, Eq)]
enum Report {
    Started(u32),
    AlreadyRunning,
    SteamRequested,
    SteamAdopted,
    Failed(String),
}

impl Report {
    fn to_line(&self) -> String {
        match self {
            Report::Started(pid) => format!("started {pid}\n"),
            Report::SteamRequested => "steam-requested\n".to_owned(),
            Report::SteamAdopted => "steam-adopted\n".to_owned(),
            Report::AlreadyRunning => "already-running\n".to_owned(),
            Report::Failed(message) => format!("failed {}\n", message.replace('\n', " ")),
        }
    }

    fn parse(line: &str) -> Option<Self> {
        // Without the newline, the supervisor died partway through writing.
        let line = line.strip_suffix('\n')?;
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match word {
            "started" => rest.parse().ok().map(Report::Started),
            "already-running" => Some(Report::AlreadyRunning),
            "steam-requested" => Some(Report::SteamRequested),
            "steam-adopted" => Some(Report::SteamAdopted),
            "failed" => Some(Report::Failed(rest.to_owned())),
            _ => None,
        }
    }
}

/// Starts a detached supervisor by running `exe __supervise`, then waits for it
/// to report whether wine is running. A portable folder is passed along with
/// `--portable`, so the run stays tied to the folder it was started from.
pub fn start(exe: &Path, portable: Option<&Path>, spec: &Spec) -> Result<Started> {
    let text = toml::to_string(spec)
        .map_err(|err| Error::StartFailed(format!("failed to prepare the run: {err}")))?;
    let mut child = supervise_command(exe, portable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context(|| format!("failed to run {}", exe.display()))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    // If this fails the supervisor died early, and the missing report says so.
    let _ = stdin.write_all(text.as_bytes());
    drop(stdin);

    let mut line = String::new();
    let stdout = child.stdout.take().expect("stdout is piped");
    let report = BufReader::new(stdout)
        .read_line(&mut line)
        .ok()
        .and_then(|_| Report::parse(&line));
    match report {
        Some(Report::Started(pid)) => {
            // Reaps the supervisor when it's done, for callers that outlive it.
            thread::spawn(move || child.wait());
            Ok(Started::Running(pid))
        }
        Some(Report::SteamRequested | Report::SteamAdopted) => {
            let adopted = matches!(report, Some(Report::SteamAdopted));
            thread::spawn(move || child.wait());
            Ok(if adopted {
                Started::SteamAdopted
            } else {
                Started::SteamRequested
            })
        }
        Some(Report::AlreadyRunning) => {
            let _ = child.wait();
            Ok(Started::AlreadyRunning)
        }
        Some(Report::Failed(message)) => {
            let _ = child.wait();
            Err(Error::StartFailed(message))
        }
        None => {
            let status = child
                .wait()
                .context(|| "failed to wait for the supervisor".to_owned())?;
            Err(Error::StartFailed(format!(
                "the supervisor exited without reporting back ({status})"
            )))
        }
    }
}

fn supervise_command(exe: &Path, portable: Option<&Path>) -> Command {
    let mut command = Command::new(exe);
    command.arg(SUPERVISE_COMMAND);
    if let Some(root) = portable {
        command.arg("--portable").arg(root);
    }
    command
}

/// Runs a detached supervisor: reads a `Spec` from stdin, reports to `start`
/// on stdout, then owns the run until it ends.
pub fn serve() -> Result<()> {
    let mut reporter = Reporter::default();
    let result = serve_run(&mut reporter);
    if let Err(err) = &result {
        reporter.send(Report::Failed(error::chain(err)));
    }
    result
}

fn serve_run(reporter: &mut Reporter) -> Result<()> {
    // Out of the terminal's session, closing the terminal doesn't end the run.
    // This only fails for a process that already leads a session.
    let _ = setsid();
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .context(|| "failed to read the run spec".to_owned())?;
    let spec: Spec = toml::from_str(&text)
        .map_err(|err| Error::StartFailed(format!("invalid run spec: {err}")))?;

    // `start` holds a lock of its own until this one is taken, but it lets go
    // when it exits, so the run keeps its protection here instead.
    let held: Vec<&Path> = spec.launch.host.prefix().into_iter().collect();
    let claim = spec
        .claim
        .locks
        .acquire(Operation::Launch, &spec.claim.name, &held)?;
    let _build_guards = spec
        .claim
        .builds
        .iter()
        .map(crate::build_lock::Lock::shared)
        .collect::<Result<Vec<_>>>()?;
    // `start` still holds these, so no update can have taken one since.
    let _folder_guards = spec
        .claim
        .folders
        .iter()
        .map(|lock| {
            lock.try_shared()
                .and_then(|guard| {
                    guard.ok_or_else(|| {
                        anyhow::anyhow!("{} is being updated", lock.folder().display())
                    })
                })
                .map_err(|err| Error::StartFailed(format!("{err:#}")))
        })
        .collect::<Result<Vec<_>>>()?;
    // The spec was prepared before the lock, so it can be stale by now.
    if !spec.claim.config.is_file() {
        return Err(Error::UnknownEnvironment(spec.claim.name.clone()));
    }
    if let Host::Wine { runner, prefix } = &spec.launch.host
        && !prefix::is_prefix(&runner.wine_prefix(prefix))
    {
        return Err(Error::MissingPrefix(prefix.clone()));
    }

    let native = matches!(spec.launch.host, Host::Native { .. });
    let runtime = Runtime::at(spec.runtime_dir.clone());
    let Some(lock) = runtime.try_lock()? else {
        reporter.send(Report::AlreadyRunning);
        return Ok(());
    };
    let supervisor = Supervisor::listen()?;
    let log = runtime.rotate_log()?;
    let session = Log::new(runtime.log_path());
    if let Err(err) = session.begin() {
        log_notice(&log, native, Notice::Warning(err));
    }
    // Anything wineo itself prints from here on, even a panic, lands in the log.
    let _ = rustix::stdio::dup2_stderr(&log);

    let log_path = runtime.log_path();
    let clone_log = || {
        log.try_clone()
            .context(|| format!("failed to open {}", log_path.display()))
    };
    if matches!(spec.launch.host, Host::Proxied { .. }) {
        let (mut state, adopted) = supervisor.begin_steam(
            &runtime,
            &spec.launch,
            RunMode::Detached,
            &spec.overrides,
            &spec.claim.name,
            |notice| log_notice(&log, false, notice),
        )?;
        reporter.send(if adopted {
            Report::SteamAdopted
        } else {
            Report::SteamRequested
        });
        let cap = Cap::start(&session, clone_log()?, spec.log_limit_mb);
        let result = supervisor.wait_steam(
            &runtime,
            &spec.launch,
            &mut state,
            &spec.claim.name,
            Duration::from_secs(spec.stop_timeout.into()),
            |notice| log_notice(&log, false, notice),
        );
        drop(cap);
        if state.status != RunStatus::Running
            && spec.after.is_some()
            && let Err(err) = finish_after(&spec, claim, lock, &state, &log, false)
        {
            log_notice(&log, false, Notice::Warning(err));
        }
        return result;
    }
    // Its own process group, so the whole tree can be signalled as one and a
    // signal to it never reaches this supervisor.
    let spawned = spec
        .launch
        .command()
        .stdin(Stdio::null())
        .stdout(clone_log()?)
        .stderr(clone_log()?)
        .process_group(0)
        .spawn()
        .context(|| format!("failed to start {}", spec.launch.host.launcher()));
    let child = spawned?;
    let mut state = RunState::running(
        RunMode::Detached,
        spec.launch.target.clone().unwrap_or_default(),
        child.id(),
        Some(child.id()),
        &spec.overrides,
    );
    state.record_host(&spec.launch.host);
    state.boot_id = crate::process::boot_id();
    state.leader_start = crate::process::start_time(child.id());
    if let Err(err) = runtime.write_state(&state) {
        log_notice(&log, native, Notice::Warning(err));
    }
    reporter.send(Report::Started(child.id()));

    // Started once wine is up, so an oversized log is cleared throughout the
    // run, including while background processes keep writing after the program
    // itself has exited.
    let cap = Cap::start(&session, clone_log()?, spec.log_limit_mb);

    let stop_timeout = Duration::from_secs(spec.stop_timeout.into());
    let mut forced = false;
    let result = supervisor.wait(
        &runtime,
        &spec.launch,
        child,
        &mut state,
        stop_timeout,
        &spec.stats_log,
        |notice| {
            if matches!(notice, Notice::Killing) {
                forced = true;
            }
            log_notice(&log, native, notice);
        },
    );
    // One final check before the run lock goes, so whoever is following the
    // log can still see the last reset.
    drop(cap);
    if state.status != RunStatus::Running && spec.after.is_some() {
        let mut hook_state = state.clone();
        if forced {
            hook_state.exit_code = None;
        }
        if let Err(err) = finish_after(&spec, claim, lock, &hook_state, &log, native) {
            log_notice(&log, native, Notice::Warning(err));
        }
    }
    result
}

fn finish_after(
    spec: &Spec,
    claim: crate::operation::Guard,
    lock: crate::runtime::RunLock,
    state: &RunState,
    log: &File,
    native: bool,
) -> Result<()> {
    let Some(after) = &spec.after else {
        return Ok(());
    };
    let guard = spec
        .claim
        .locks
        .after_launch(claim, &spec.claim.name)
        .map_err(|err| {
            Error::StartFailed(format!(
                "the after hook was skipped: {}",
                error::chain(&err)
            ))
        })?;
    drop(lock);
    let output = log
        .try_clone()
        .context(|| "failed to clone hook output".to_owned())?;
    if let Err(err) = after.run(
        &guard,
        state,
        &Token::new(),
        Duration::from_secs(spec.stop_timeout.into()),
        crate::hooks::Output::File(output),
    ) {
        log_notice(log, native, Notice::Warning(err));
    }
    Ok(())
}

fn log_notice(mut log: &File, native: bool, notice: Notice) {
    let message = match notice {
        Notice::Stopping => "stopping, asking programs to close".to_owned(),
        Notice::Killing if native => "killing everything left in the process group".to_owned(),
        Notice::Killing => "killing everything left in the prefix".to_owned(),
        Notice::Warning(err) => format!("warning: {}", error::chain(&err)),
        // Silent: nothing worth telling whoever is following the log.
        Notice::Heartbeat => return,
    };
    let _ = writeln!(log, "wineo: {message}");
}

#[derive(Default)]
struct Reporter {
    sent: bool,
}

impl Reporter {
    /// Sends the one report `start` waits for, then lets go of stdout, since
    /// the supervisor outlives `start`.
    fn send(&mut self, report: Report) {
        if std::mem::replace(&mut self.sent, true) {
            return;
        }
        let mut stdout = io::stdout().lock();
        let _ = stdout
            .write_all(report.to_line().as_bytes())
            .and_then(|()| stdout.flush());
        drop(stdout);
        if let Ok(null) = OpenOptions::new().write(true).open("/dev/null") {
            let _ = rustix::stdio::dup2_stdout(&null);
        }
    }
}

/// Asks whatever owns a run to stop it, SIGTERM for a clean stop or SIGUSR1 to
/// force it, then waits for the run to end. Returns false if nothing was running.
pub fn stop(runtime: &Runtime, force: bool, timeout: Duration) -> Result<bool> {
    stop_with_after(runtime, force, timeout, false)
}

pub(crate) fn stop_with_after(
    runtime: &Runtime,
    force: bool,
    timeout: Duration,
    has_after: bool,
) -> Result<bool> {
    if !runtime.is_running()? {
        return Ok(false);
    }
    let deadline = Instant::now() + timeout;
    let timed_out = || Error::StillRunning(timeout.as_secs());
    let signal = if force { Signal::USR1 } else { Signal::TERM };
    loop {
        if !runtime.is_running()? {
            return Ok(true);
        }
        // The pid is missing for a moment while a new holder starts up.
        if let Some(raw) = runtime.holder_pid()?
            && let Some(pid) = i32::try_from(raw).ok().and_then(Pid::from_raw)
        {
            match kill_process(pid, signal) {
                Ok(()) => break,
                // Already gone, so the lock is about to be released.
                Err(Errno::SRCH) => {}
                Err(err) => {
                    return Err(io::Error::from(err))
                        .context(|| format!("failed to signal process {raw}"));
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
        thread::sleep(Duration::from_millis(50));
    }
    while runtime.is_running()? {
        // The after handoff retains the run lock only until its exclusive
        // guard is ready. The program's deadline ends with its recorded end.
        if has_after
            && runtime
                .read_state()?
                .is_some_and(|run| run.status != RunStatus::Running)
        {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::runner::Runner;

    #[test]
    fn stop_keeps_its_deadline_after_the_end_record_without_an_after_hook() {
        let root = tempfile::tempdir().unwrap();
        let runtime = Runtime::at(root.path().join("app"));
        let _lock = runtime.try_lock().unwrap().unwrap();
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        // Keep the lock held past the end record, but send stop's signal to
        // a disposable child rather than the test process holding the lock.
        std::fs::write(runtime.dir().join("lock"), child.id().to_string()).unwrap();
        let mut state = RunState::running(
            RunMode::Detached,
            "app".to_owned(),
            child.id(),
            None,
            &Overrides::default(),
        );
        state.finish(RunStatus::Exited);
        runtime.write_state(&state).unwrap();
        let timeout = Duration::from_millis(50);
        let result = stop(&runtime, false, timeout);
        let _ = child.kill();
        child.wait().unwrap();
        assert!(matches!(result, Err(Error::StillRunning(_))));
    }

    #[test]
    fn stop_leaves_the_after_handoff_wait_to_the_caller() {
        let root = tempfile::tempdir().unwrap();
        let runtime = Runtime::at(root.path().join("app"));
        let _lock = runtime.try_lock().unwrap().unwrap();
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        std::fs::write(runtime.dir().join("lock"), child.id().to_string()).unwrap();
        let mut state = RunState::running(
            RunMode::Detached,
            "app".to_owned(),
            child.id(),
            None,
            &Overrides::default(),
        );
        state.finish(RunStatus::Exited);
        runtime.write_state(&state).unwrap();
        let result = stop_with_after(&runtime, false, Duration::from_millis(50), true);
        let _ = child.kill();
        child.wait().unwrap();
        assert!(result.unwrap());
        assert!(runtime.is_running().unwrap());
    }

    #[test]
    fn reports_round_trip_as_single_lines() {
        for report in [
            Report::Started(4242),
            Report::AlreadyRunning,
            Report::Failed("failed to start wine: No such file or directory".to_owned()),
        ] {
            let line = report.to_line();
            assert_eq!(line.matches('\n').count(), 1, "{line}");
            assert_eq!(Report::parse(&line), Some(report));
        }
        assert_eq!(
            Report::parse(&Report::Failed("two\nlines".to_owned()).to_line()),
            Some(Report::Failed("two lines".to_owned()))
        );
        assert_eq!(Report::parse("started 42"), None);
        assert_eq!(Report::parse("started nope\n"), None);
        assert_eq!(Report::parse(""), None);
        assert_eq!(Report::parse("what\n"), None);
    }

    #[test]
    fn detached_runs_carry_the_portable_folder() {
        let args = |portable: Option<&Path>| {
            supervise_command(Path::new("/usr/bin/wineo"), portable)
                .get_args()
                .map(|arg| arg.to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(args(None), [SUPERVISE_COMMAND]);
        assert_eq!(
            args(Some(Path::new("/mnt/usb/wineo"))),
            [SUPERVISE_COMMAND, "--portable", "/mnt/usb/wineo"]
        );
    }

    #[test]
    fn spec_round_trips_through_toml() {
        let spec = Spec {
            claim: Claim {
                name: "app".to_owned(),
                locks: Locks::at(PathBuf::from("/state/wineo/.locks")),
                config: PathBuf::from("/config/wineo/environments/app.toml"),
                builds: Vec::new(),
                folders: Vec::new(),
            },
            runtime_dir: PathBuf::from("/state/wineo/app"),
            stats_log: PathBuf::from("/data/wineo/stats/app.jsonl"),
            stop_timeout: 10,
            log_limit_mb: 50,
            launch: Launch {
                host: Host::Wine {
                    runner: Runner::fake("wine"),
                    prefix: PathBuf::from("/prefixes/app"),
                },
                target: Some("app".to_owned()),
                args: vec![r"C:\Program Files\App\app.exe".to_owned(), "-x".to_owned()],
                cwd: PathBuf::from("/prefixes/app/drive_c"),
                env: BTreeMap::from([("DXVK_HUD".to_owned(), "fps".to_owned())]),
                wrappers: crate::launch::Wrappers {
                    gamescope: Some(PathBuf::from("/usr/bin/gamescope")),
                    gamescope_args: vec!["-f".to_owned(), "--mangoapp".to_owned()],
                    mangohud: None,
                    gamemode: Some(PathBuf::from("/usr/bin/gamemoderun")),
                    env: Some(PathBuf::from("/usr/bin/env")),
                    sh: Some(PathBuf::from("/usr/bin/sh")),
                    wineserver: Some(PathBuf::from("/usr/bin/wineserver")),
                },
                wayland: crate::launch::Wayland::On,
                gameid: Some("umu-hades".to_owned()),
                store: Some("gog".to_owned()),
                build_guards: Vec::new(),
            },
            overrides: Overrides {
                target: None,
                env: BTreeMap::from([("DXVK_HUD".to_owned(), "fps".to_owned())]),
                args: vec!["-x".to_owned()],
            },
            after: None,
        };
        let text = toml::to_string(&spec).unwrap();
        let parsed: Spec = toml::from_str(&text).unwrap();
        assert_eq!(format!("{parsed:?}"), format!("{spec:?}"));
    }
}
