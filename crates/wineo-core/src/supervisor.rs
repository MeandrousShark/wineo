//! Owning a run: waiting for wine, handling stop requests, and recording state.
//! `run` does this attached to the terminal, and `start` hands it to a detached
//! `wineo __supervise` process.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process, setsid};
use serde::{Deserialize, Serialize};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGUSR1};
use signal_hook::iterator::{Handle, Signals};

use crate::error::{self, Error, IoContext, Result};
use crate::launch::{Launch, Overrides};
use crate::log::{Cap, Log};
use crate::operation::{Locks, Operation};
use crate::prefix;
use crate::runtime::{RunMode, RunState, RunStatus, Runtime, exit_code};

/// The hidden command a detached supervisor runs as.
pub const SUPERVISE_COMMAND: &str = "__supervise";

/// How often a kill repeats until the run ends. A program that was still
/// starting up can be invisible to wineserver the first time.
const KILL_RETRY: Duration = Duration::from_secs(1);

enum Event {
    Exited(io::Result<ExitStatus>),
    Idle,
    Stop,
    Force,
    StopTimeout,
}

/// What the run loop tells whoever is watching it.
#[derive(Debug)]
pub enum Notice {
    /// Programs were asked to close. Another stop request forces it.
    Stopping,
    /// Everything left in the prefix is being killed.
    Killing,
    /// Something failed without ending the run.
    Warning(Error),
}

/// Owns a run until it ends.
pub struct Supervisor {
    events: Receiver<Event>,
    sender: Sender<Event>,
    signals: Handle,
    signal_thread: Option<JoinHandle<()>>,
}

impl Supervisor {
    /// Starts listening for stop signals, before wine starts so an early one
    /// still stops it cleanly. SIGINT, SIGTERM, and SIGHUP ask for a clean stop
    /// and force it when repeated. SIGUSR1 forces it right away.
    pub fn listen() -> Result<Self> {
        let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP, SIGUSR1])
            .context(|| "failed to set up signal handling".to_owned())?;
        let handle = signals.handle();
        let (sender, events) = mpsc::channel();
        let signal_sender = sender.clone();
        let signal_thread = thread::spawn(move || {
            for signal in signals.forever() {
                let event = if signal == SIGUSR1 {
                    Event::Force
                } else {
                    Event::Stop
                };
                if signal_sender.send(event).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            events,
            sender,
            signals: handle,
            signal_thread: Some(signal_thread),
        })
    }

    /// Waits until the program has exited and nothing is left running in the
    /// prefix, recording in `state` when the program exits and how the run ended.
    pub fn wait(
        self,
        runtime: &Runtime,
        launch: &Launch,
        mut child: Child,
        state: &mut RunState,
        stop_timeout: Duration,
        mut notify: impl FnMut(Notice),
    ) -> Result<()> {
        let sender = self.sender.clone();
        thread::spawn(move || {
            let _ = sender.send(Event::Exited(child.wait()));
        });

        let mut stopping = false;
        let mut killed = false;
        // When the clean stop gives up, or the next kill is due.
        let mut deadline: Option<Instant> = None;
        loop {
            let event = match deadline {
                Some(deadline) => match self
                    .events
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                {
                    Ok(event) => event,
                    Err(RecvTimeoutError::Timeout) => Event::StopTimeout,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match self.events.recv() {
                    Ok(event) => event,
                    Err(_) => break,
                },
            };
            match event {
                Event::Exited(status) => {
                    state.program_exited(status.ok().map(exit_code));
                    if let Err(err) = runtime.write_state(state) {
                        notify(Notice::Warning(err));
                    }
                    // Launchers often hand off to another process and exit, so the
                    // run lasts until nothing is left running in the prefix.
                    let sender = self.sender.clone();
                    let (runner, prefix) = (launch.runner.clone(), launch.prefix.clone());
                    thread::spawn(move || {
                        let _ = runner.wait_idle(&prefix);
                        let _ = sender.send(Event::Idle);
                    });
                }
                Event::Idle => break,
                Event::Stop if !stopping => {
                    stopping = true;
                    deadline = Some(Instant::now() + stop_timeout);
                    notify(Notice::Stopping);
                    let (runner, prefix) = (launch.runner.clone(), launch.prefix.clone());
                    thread::spawn(move || {
                        let _ = runner.end_session(&prefix);
                    });
                }
                Event::Stop | Event::Force | Event::StopTimeout => {
                    stopping = true;
                    if !killed {
                        killed = true;
                        notify(Notice::Killing);
                    }
                    if let Err(err) = launch.runner.kill(&launch.prefix) {
                        notify(Notice::Warning(err));
                    }
                    deadline = Some(Instant::now() + KILL_RETRY);
                }
            }
        }

        // Exit codes can't tell a kill from a normal exit, so a stop is only
        // known from the request.
        let status = match (stopping, state.exit_code) {
            (true, _) => RunStatus::Stopped,
            (false, Some(0)) => RunStatus::Exited,
            (false, _) => RunStatus::Failed,
        };
        state.finish(status);
        runtime.write_state(state)
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.signals.close();
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
    pub stop_timeout: u32,
    pub log_limit_mb: u32,
    pub launch: Launch,
    pub overrides: Overrides,
}

/// What a detached supervisor needs to hold its own operation lock and confirm
/// the environment is still there once it has it.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claim {
    pub name: String,
    pub locks_dir: PathBuf,
    pub config: PathBuf,
}

/// How handing a run to a new supervisor went.
#[derive(Debug, PartialEq, Eq)]
pub enum Started {
    /// Wine is running with this pid.
    Running(u32),
    /// Something else started the environment first.
    AlreadyRunning,
}

/// The single line a supervisor sends back to `start`.
#[derive(Debug, PartialEq, Eq)]
enum Report {
    Started(u32),
    AlreadyRunning,
    Failed(String),
}

impl Report {
    fn to_line(&self) -> String {
        match self {
            Report::Started(pid) => format!("started {pid}\n"),
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
            "failed" => Some(Report::Failed(rest.to_owned())),
            _ => None,
        }
    }
}

/// Starts a detached supervisor by running `exe __supervise`, then waits for it
/// to report whether wine is running.
pub fn start(exe: &Path, spec: &Spec) -> Result<Started> {
    let text = toml::to_string(spec)
        .map_err(|err| Error::StartFailed(format!("failed to prepare the run: {err}")))?;
    let mut child = Command::new(exe)
        .arg(SUPERVISE_COMMAND)
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
    let locks = Locks::at(spec.claim.locks_dir.clone());
    let _claim = locks.acquire(Operation::Launch, &spec.claim.name, &[&spec.launch.prefix])?;
    // The spec was prepared before the lock, so it can be stale by now.
    if !spec.claim.config.is_file() {
        return Err(Error::UnknownEnvironment(spec.claim.name.clone()));
    }
    if !prefix::is_prefix(&spec.launch.prefix) {
        return Err(Error::MissingPrefix(spec.launch.prefix.clone()));
    }

    let runtime = Runtime::at(spec.runtime_dir.clone());
    let Some(lock) = runtime.try_lock()? else {
        reporter.send(Report::AlreadyRunning);
        return Ok(());
    };
    let supervisor = Supervisor::listen()?;
    let log = runtime.rotate_log()?;
    let session = Log::new(runtime.log_path());
    if let Err(err) = session.begin() {
        log_notice(&log, Notice::Warning(err));
    }
    // Anything wineo itself prints from here on, even a panic, lands in the log.
    let _ = rustix::stdio::dup2_stderr(&log);

    let log_path = runtime.log_path();
    let clone_log = || {
        log.try_clone()
            .context(|| format!("failed to open {}", log_path.display()))
    };
    let child = spec
        .launch
        .command()
        .stdin(Stdio::null())
        .stdout(clone_log()?)
        .stderr(clone_log()?)
        .spawn()
        .context(|| "failed to start wine".to_owned())?;
    let mut state = RunState::running(
        RunMode::Detached,
        spec.launch.target.clone().unwrap_or_default(),
        child.id(),
        &spec.overrides,
    );
    if let Err(err) = runtime.write_state(&state) {
        log_notice(&log, Notice::Warning(err));
    }
    reporter.send(Report::Started(child.id()));

    // Started once wine is up, so an oversized log is cleared throughout the
    // run, including while background processes keep writing after the program
    // itself has exited.
    let cap = Cap::start(&session, clone_log()?, spec.log_limit_mb);

    let stop_timeout = Duration::from_secs(spec.stop_timeout.into());
    let result = supervisor.wait(
        &runtime,
        &spec.launch,
        child,
        &mut state,
        stop_timeout,
        |notice| log_notice(&log, notice),
    );
    // One final check before the run lock goes, so whoever is following the
    // log can still see the last reset.
    drop(cap);
    drop(lock);
    result
}

fn log_notice(mut log: &File, notice: Notice) {
    let message = match notice {
        Notice::Stopping => "stopping, asking programs to close".to_owned(),
        Notice::Killing => "killing everything left in the prefix".to_owned(),
        Notice::Warning(err) => format!("warning: {}", error::chain(&err)),
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
    fn spec_round_trips_through_toml() {
        let spec = Spec {
            claim: Claim {
                name: "app".to_owned(),
                locks_dir: PathBuf::from("/state/wineo/.locks"),
                config: PathBuf::from("/config/wineo/environments/app.toml"),
            },
            runtime_dir: PathBuf::from("/state/wineo/app"),
            stop_timeout: 10,
            log_limit_mb: 50,
            launch: Launch {
                runner: Runner::fake("wine"),
                prefix: PathBuf::from("/prefixes/app"),
                target: Some("app".to_owned()),
                args: vec![r"C:\Program Files\App\app.exe".to_owned(), "-x".to_owned()],
                cwd: PathBuf::from("/prefixes/app/drive_c"),
                env: BTreeMap::from([("DXVK_HUD".to_owned(), "fps".to_owned())]),
            },
            overrides: Overrides {
                target: None,
                env: BTreeMap::from([("DXVK_HUD".to_owned(), "fps".to_owned())]),
                args: vec!["-x".to_owned()],
            },
        };
        let text = toml::to_string(&spec).unwrap();
        let parsed: Spec = toml::from_str(&text).unwrap();
        assert_eq!(format!("{parsed:?}"), format!("{spec:?}"));
    }
}
