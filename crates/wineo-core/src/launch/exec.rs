use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use rustix::process::Signal;

use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::error::Error;
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::process::{Ended, Escalation, Step};
use crate::runtime::{Runtime, exit_code};
use crate::supervisor::{Notice, Settle, Supervisor, Watched};

use super::{ExecResolution, Host, Launch, LaunchEvent, ensure_prefix, reconcile, runner_locked};

const POLL: Duration = Duration::from_millis(50);
const READ_BATCH: u64 = 256 * 1024;
/// How long a program has to have been gone, with the prefix still busy,
/// before a frontend says why it's waiting.
const NOTE_AFTER: Duration = Duration::from_millis(250);
/// How long kills keep being retried before a stopped exec gives up waiting,
/// as for installers.
const GIVE_UP: Duration = Duration::from_secs(10);
const UNCONFIRMED: &str = "the program's processes could not be confirmed stopped";

#[derive(Debug)]
pub enum ExecEvent {
    Launch(LaunchEvent),
    Started,
    OutputLine(String),
    Stopping,
    Killing,
    /// Other programs may still be running in the prefix after this exit.
    Exited {
        code: i32,
    },
}

/// Runs an arbitrary program for a frontend, without consulting the current
/// directory. Holds shared launch and build locks until the started process
/// exits. Cancellation only signals its process group, never the whole
/// prefix. A nonzero or canceled process exit is returned as a status.
/// Cancellation before spawning is an error. No run record or playtime is
/// written, and a frontend crash leaves the program running while releasing
/// its locks.
///
/// A program run from a GOG game folder is instead waited for the way a
/// `start` run is: until the prefix is idle, holding every lock, including
/// the folder's in-use lock, so a launcher that hands off to the game keeps
/// the folder in use until the game quits. Canceling then asks the
/// prefix's programs to close, and forcing kills them, like `wineo stop`.
pub fn exec(
    paths: &Paths,
    name: &str,
    program: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    token: &Token,
    mut events: impl FnMut(ExecEvent),
) -> anyhow::Result<ExitStatus> {
    check_cancel(token)?;
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, name)?;
    let (runner, _build_guard) = runner_locked(paths, &global, &environment)?;
    Launch::check_exec_with_resolution(
        paths,
        &global,
        &environment,
        &runner,
        program,
        env,
        ExecResolution::ExplicitPaths,
    )?;
    ensure_prefix(paths, &global, &environment, &runner, token, |event| {
        events(ExecEvent::Launch(event));
    })?;
    check_cancel(token)?;
    let launch = Launch::exec_with_resolution(
        paths,
        &global,
        &environment,
        program,
        args,
        env,
        ExecResolution::ExplicitPaths,
    )?;
    let prefix = launch.host.prefix().expect("exec always runs in a prefix");
    let _guard = Locks::new(paths).acquire(Operation::Launch, name, &[prefix])?;
    Environment::ensure_exists(paths, name)?;
    Runtime::new(paths, name).check_resolved(name)?;
    let folders = super::orchestration::protect_as(paths, &environment, &launch, token, "exec")?;
    reconcile(paths, &global, &environment, &launch, |event| {
        events(ExecEvent::Launch(event));
    })?;
    check_cancel(token)?;
    if folders.is_empty() {
        return run(&launch, global.stop_timeout, token, &mut events);
    }
    run_settled(
        &launch,
        Duration::from_secs(global.stop_timeout.into()),
        token,
        &mut events,
    )
}

/// `run` for a program from a GOG game folder, which lasts until the prefix
/// is idle.
fn run_settled(
    launch: &Launch,
    stop_timeout: Duration,
    token: &Token,
    events: &mut impl FnMut(ExecEvent),
) -> anyhow::Result<ExitStatus> {
    let log = tempfile::tempfile().context("failed to create exec output log")?;
    let mut lines = Lines::new(log.try_clone()?);
    let child = launch
        .command()
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log.try_clone()?))
        .process_group(0)
        .spawn()
        .with_context(|| format!("failed to start {}", launch.host.launcher()))?;
    crate::process::quiet_core_dumps(&child);
    events(ExecEvent::Started);
    let mut failed = None;
    let ended = watch_settled(
        Supervisor::with_token(token),
        &launch.host,
        child,
        stop_timeout,
        |seen| match seen {
            Settling::Tick => {
                if failed.is_none()
                    && let Err(err) = lines.pump(READ_BATCH, events)
                {
                    failed = Some(err);
                }
            }
            Settling::Stopping => events(ExecEvent::Stopping),
            Settling::Killing => events(ExecEvent::Killing),
            // Shown with the program's output, the way the CLI prints one.
            Settling::Warning(err) => events(ExecEvent::OutputLine(format!(
                "warning: {}",
                crate::error::chain(&err)
            ))),
            Settling::Exited | Settling::StillWaiting => {}
        },
    );
    if let Some(err) = failed {
        return Err(err.into());
    }
    let end = log.metadata()?.len();
    lines.pump(end.saturating_sub(lines.read), events)?;
    if !lines.pending.is_empty() {
        lines.finish(events);
    }
    let status = ended.finish()?;
    events(ExecEvent::Exited {
        code: exit_code(status),
    });
    Ok(status)
}

/// What `watch_settled` reports along the way.
#[derive(Debug)]
pub enum Settling {
    /// The started program exited. Others in the prefix may still run.
    Exited,
    /// The program has been gone a moment and the prefix is still busy.
    StillWaiting,
    /// Programs were asked to close. Another stop request forces it.
    Stopping,
    /// Everything left in the prefix is being killed.
    Killing,
    Warning(Error),
    /// Nothing happened, which comes often, for a caller with work to do.
    Tick,
}

/// How a watched program and its prefix came to an end.
#[derive(Debug)]
pub struct Settled {
    status: Option<ExitStatus>,
    settled: crate::error::Result<()>,
}

impl Settled {
    /// The started program's own status, or, stopped or not, why the prefix
    /// couldn't be confirmed idle, as a `start` run reports it.
    pub fn finish(self) -> anyhow::Result<ExitStatus> {
        self.settled?;
        self.status
            .ok_or_else(|| Error::Unsettled("the program's exit status was lost".to_owned()).into())
    }
}

/// Watches `child`, which leads its own process group, and then the prefix
/// until it's idle, by the rule a `start` run ends on: `wineserver -w`, and
/// for Proton the process group. `supervisor` brings the stop requests: the
/// first asks the prefix's programs to close, and the next, or the stop
/// timeout, kills them, retried until idle or for `GIVE_UP`.
pub fn watch_settled(
    supervisor: Supervisor,
    host: &Host,
    child: Child,
    stop_timeout: Duration,
    mut seen: impl FnMut(Settling),
) -> Settled {
    // Read while the program is at least a zombie, which the watch only
    // reaps at the end, so its identity can't be lost to a fast exit.
    let identity = crate::process::Identity::of(child.id());
    let host = host.clone();
    let (sender, watched) = mpsc::channel();
    let watcher = thread::spawn(move || {
        supervisor.watch(
            &host,
            identity,
            child,
            stop_timeout,
            Settle::Confirmed,
            Some(GIVE_UP),
            |event| {
                let _ = sender.send(event);
            },
        )
    });
    let mut exited_at: Option<Instant> = None;
    let mut noted = false;
    loop {
        match watched.recv_timeout(POLL) {
            Ok(Watched::Exited(_)) => {
                exited_at = Some(Instant::now());
                seen(Settling::Exited);
            }
            Ok(Watched::Notice(Notice::Stopping)) => seen(Settling::Stopping),
            Ok(Watched::Notice(Notice::Killing)) => seen(Settling::Killing),
            Ok(Watched::Notice(Notice::Warning(err))) => seen(Settling::Warning(err)),
            Ok(Watched::Notice(Notice::Heartbeat)) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !noted && exited_at.is_some_and(|at| at.elapsed() >= NOTE_AFTER) {
            noted = true;
            seen(Settling::StillWaiting);
        }
        seen(Settling::Tick);
    }
    match watcher.join() {
        Ok(ending) => Settled {
            status: ending.exit_status,
            settled: ending.settled,
        },
        Err(_) => Settled {
            status: None,
            settled: Err(Error::Unsettled("the exec watch failed".to_owned())),
        },
    }
}

fn check_cancel(token: &Token) -> anyhow::Result<()> {
    if token.is_canceled() {
        return Err(Error::Canceled("exec").into());
    }
    Ok(())
}

fn run(
    launch: &Launch,
    stop_timeout: u32,
    token: &Token,
    events: &mut impl FnMut(ExecEvent),
) -> anyhow::Result<ExitStatus> {
    // Descendants can inherit output past the immediate child's exit. A file
    // avoids waiting for pipe EOF or breaking their output when we return.
    let log = tempfile::tempfile().context("failed to create exec output log")?;
    let mut lines = Lines::new(log.try_clone()?);
    let mut child = launch
        .command()
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log.try_clone()?))
        .process_group(0)
        .spawn()
        .with_context(|| format!("failed to start {}", launch.host.launcher()))?;
    crate::process::quiet_core_dumps(&child);
    events(ExecEvent::Started);
    let mut escalation = Escalation::new(Duration::from_secs(stop_timeout.into()), None);
    // Output lands in a file, which has nothing to wait on.
    let stopped = crate::process::stop_group(
        &child,
        Signal::TERM,
        &mut escalation,
        token,
        Some(POLL),
        |step| {
            match step {
                None => return lines.pump(READ_BATCH, events),
                Some(Step::Ask) => events(ExecEvent::Stopping),
                Some(Step::Kill) => events(ExecEvent::Killing),
                Some(Step::Rekill) => {}
            }
            Ok(())
        },
    );
    let failed = match stopped {
        Ok(Ok(ended)) => Ok(ended),
        Ok(Err((err, ended))) => Err((anyhow::Error::from(err), ended == Ended::Stopped)),
        Err(err) => Err((
            anyhow::Error::from(err).context("failed to stop exec process group"),
            false,
        )),
    };
    let ended = match failed {
        Ok(ended) => ended,
        Err((err, settled)) => {
            let reaped = crate::process::kill_and_reap(&mut child);
            return Err(if settled && reaped {
                err
            } else {
                err.context(UNCONFIRMED)
            });
        }
    };
    let status = child.wait().context("failed to wait for exec")?;
    if ended == Ended::Unconfirmed {
        return Err(Error::Unsettled(UNCONFIRMED.to_owned()).into());
    }
    // Read only what is present now; handed-off programs can keep writing,
    // and must not extend the lifetime of this worker.
    let end = log.metadata()?.len();
    lines.pump(end.saturating_sub(lines.read), events)?;
    if !lines.pending.is_empty() {
        lines.finish(events);
    }
    events(ExecEvent::Exited {
        code: exit_code(status),
    });
    Ok(status)
}

struct Lines {
    file: File,
    read: u64,
    pending: Vec<u8>,
}

impl Lines {
    fn new(file: File) -> Self {
        Self {
            file,
            read: 0,
            pending: Vec::new(),
        }
    }

    fn pump(&mut self, mut limit: u64, events: &mut impl FnMut(ExecEvent)) -> io::Result<()> {
        let mut buffer = [0u8; 8192];
        while limit > 0 {
            let size = limit.min(buffer.len() as u64) as usize;
            let count = self.file.read_at(&mut buffer[..size], self.read)?;
            if count == 0 {
                break;
            }
            self.read += count as u64;
            limit -= count as u64;
            for byte in &buffer[..count] {
                if *byte == b'\n' {
                    self.finish(events);
                } else {
                    self.pending.push(*byte);
                }
            }
        }
        Ok(())
    }

    fn finish(&mut self, events: &mut impl FnMut(ExecEvent)) {
        events(ExecEvent::OutputLine(
            String::from_utf8_lossy(&self.pending).into_owned(),
        ));
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn log_lines_survive_read_boundaries_and_temporary_eof() {
        let mut file = tempfile::tempfile().unwrap();
        let mut lines = Lines::new(file.try_clone().unwrap());
        let mut output = Vec::new();
        let mut events = |event| {
            if let ExecEvent::OutputLine(line) = event {
                output.push(line);
            }
        };
        file.write_all(b"first\n\n\xc3").unwrap();
        lines.pump(READ_BATCH, &mut events).unwrap();
        lines.pump(READ_BATCH, &mut events).unwrap();
        file.write_all(b"\xa9\r\nlast").unwrap();
        lines.pump(READ_BATCH, &mut events).unwrap();
        lines.finish(&mut events);
        assert_eq!(output, ["first", "", "\u{e9}\r", "last"]);
    }

    #[test]
    fn a_long_line_is_captured_whole() {
        let mut file = tempfile::tempfile().unwrap();
        let text = "x".repeat(READ_BATCH as usize + 1);
        writeln!(file, "{text}").unwrap();
        let mut lines = Lines::new(file);
        let mut output = Vec::new();
        let mut events = |event| {
            if let ExecEvent::OutputLine(line) = event {
                output.push(line);
            }
        };
        lines.pump(READ_BATCH, &mut events).unwrap();
        lines.pump(READ_BATCH, &mut events).unwrap();
        assert_eq!(output, [text]);
    }
}
