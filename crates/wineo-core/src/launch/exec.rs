use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::os::unix::process::CommandExt;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::Context;
use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process_group};

use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::error::Error;
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::runtime::{Runtime, exit_code};

use super::{ExecResolution, Launch, LaunchEvent, ensure_prefix, reconcile, runner_locked};

const POLL: Duration = Duration::from_millis(50);
const READ_BATCH: u64 = 256 * 1024;

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
/// exits. Cancellation only signals its process group, never the whole prefix.
/// A nonzero or canceled process exit is returned as a status. Cancellation
/// before spawning is an error. No run record or playtime is written, and a
/// frontend crash leaves the program running while releasing its locks.
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
    reconcile(paths, &global, &environment, &launch, |event| {
        events(ExecEvent::Launch(event));
    })?;
    check_cancel(token)?;
    run(&launch, global.stop_timeout, token, &mut events)
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
    let group = child.id();
    let mut stopping = None;
    let mut killing = false;
    let mut exited = false;
    let result = (|| {
        loop {
            if let Some(status) = child.try_wait().context("failed to wait for exec")? {
                exited = true;
                // Read only what is present now; handed-off programs can keep
                // writing, and must not extend the lifetime of this worker.
                let end = log.metadata()?.len();
                lines.pump(end.saturating_sub(lines.read), events)?;
                if !lines.pending.is_empty() {
                    lines.finish(events);
                }
                events(ExecEvent::Exited {
                    code: exit_code(status),
                });
                return Ok(status);
            }
            if token.is_canceled() && stopping.is_none() {
                stopping = Some(Instant::now());
                events(ExecEvent::Stopping);
                signal(group, Signal::TERM)?;
            }
            if !killing
                && stopping.is_some_and(|at| {
                    token.is_forced() || at.elapsed() >= Duration::from_secs(stop_timeout.into())
                })
            {
                killing = true;
                events(ExecEvent::Killing);
                signal(group, Signal::KILL)?;
            }
            lines.pump(READ_BATCH, events)?;
            token.wait_timeout(POLL);
        }
    })();
    if result.is_err() && !exited {
        let _ = signal(group, Signal::KILL);
        let _ = child.wait();
    }
    result
}

fn signal(group: u32, signal: Signal) -> anyhow::Result<()> {
    let pid = Pid::from_raw(group as i32).context("invalid exec process group")?;
    match kill_process_group(pid, signal) {
        Ok(()) | Err(Errno::SRCH) => Ok(()),
        Err(err) => Err(io::Error::from(err)).context("failed to stop exec process group"),
    }
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
