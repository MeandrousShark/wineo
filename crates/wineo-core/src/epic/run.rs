//! Running one short Legendary or Python command to the end: its own
//! process group, nothing on stdin, both pipes drained, a time limit, and
//! a cancel that interrupts the whole group.

use std::io::Read;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::Signal;

use super::Error;
use crate::cancel::Token;
use crate::process::{self, Ended, Escalation};

/// How long an interrupted command gets to clean up before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// How often the time limit is looked at.
const POLL: Duration = Duration::from_millis(50);

/// A whole owned list fits many times over.
const STDOUT_LIMIT: usize = 64 * 1024 * 1024;

/// Only the end of stderr is ever shown, and tracebacks are long.
pub(crate) const STDERR_LIMIT: usize = 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Output {
    pub stdout: String,
    pub stderr: String,
}

/// Runs `command` to the end and returns what it printed, with `secret`,
/// if given, replaced wherever it appears. `label` names the command in
/// errors and never holds the secret. An exit code that isn't 0 is an
/// error; 0 proves nothing, and the caller checks the rest.
pub(crate) fn run(
    mut command: Command,
    label: &str,
    timeout: Duration,
    token: &Token,
    secret: Option<&str>,
) -> Result<Output, Error> {
    let failed = |detail: String| Error::Failed {
        command: label.to_owned(),
        detail,
    };
    if token.is_canceled() {
        return Err(crate::Error::Canceled("legendary").into());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| failed(format!("couldn't start it: {error}")))?;
    let stdout = drain(
        child.stdout.take().expect("stdout is piped"),
        STDOUT_LIMIT,
        false,
    );
    let stderr = drain(
        child.stderr.take().expect("stderr is piped"),
        STDERR_LIMIT,
        true,
    );
    let deadline = Instant::now() + timeout;
    // An interrupt, the way Legendary's own Ctrl+C handling expects.
    let mut escalation = Escalation::new(STOP_GRACE, None);
    let stopped = process::stop_group(
        &child,
        Signal::INT,
        &mut escalation,
        token,
        Some(POLL),
        |_| {
            if Instant::now() >= deadline {
                Err(())
            } else {
                Ok(())
            }
        },
    );
    let ended = match stopped {
        Ok(Ok(Ended::Exited)) => {
            // Anything it left behind in its group would hold the pipes
            // open. The leader is still unreaped, so the group number is
            // still its own.
            let _ = process::signal_led_group(&child, Signal::KILL);
            Ok(())
        }
        Ok(Ok(Ended::Stopped | Ended::Unconfirmed)) => {
            Err(Error::from(crate::Error::Canceled("legendary")))
        }
        Ok(Err(((), _))) => Err(Error::TimedOut {
            command: label.to_owned(),
            seconds: timeout.as_secs(),
        }),
        Err(error) => Err(failed(format!("couldn't wait for it: {error}"))),
    };
    if let Err(error) = ended {
        process::kill_and_reap(&mut child);
        // Readers of a group that couldn't be confirmed gone are left to
        // finish on their own.
        let _ = (stdout.wait(), stderr.wait());
        return Err(error);
    }
    let status = child
        .wait()
        .map_err(|error| failed(format!("couldn't wait for it: {error}")))?;
    let (stdout, stdout_whole) = stdout.wait().unwrap_or_default();
    let (mut stderr, stderr_whole) = stderr.wait().unwrap_or_default();
    if !stderr_whole {
        // The cut can fall inside a line, even inside the secret, so the
        // part line before the first newline goes before anything is
        // scrubbed or shown.
        let start = stderr
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(stderr.len(), |at| at + 1);
        stderr.drain(..start);
    }
    let hide = |bytes: Vec<u8>| {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        match secret.filter(|secret| !secret.is_empty()) {
            Some(secret) => text.replace(secret, "<redacted>"),
            None => text,
        }
    };
    let output = Output {
        stdout: hide(stdout),
        stderr: hide(stderr),
    };
    if !status.success() {
        let code = match (status.code(), status.signal()) {
            (Some(code), _) => format!("exit code {code}"),
            (None, Some(signal)) => format!("killed by signal {signal}"),
            (None, None) => status.to_string(),
        };
        return Err(failed(match last_line(&output.stderr) {
            Some(line) => format!("{code}: {line}"),
            None => code,
        }));
    }
    if !stdout_whole {
        return Err(failed(format!(
            "it printed more than {} MiB",
            STDOUT_LIMIT / 1024 / 1024
        )));
    }
    Ok(output)
}

/// The last line of `text` that says anything, made fit for one line of
/// an error.
pub(crate) fn last_line(text: &str) -> Option<String> {
    let line = text.lines().rev().find(|line| !line.trim().is_empty())?;
    let line = crate::error::one_line(line);
    Some(match line.char_indices().nth(300) {
        Some((at, _)) => format!("{}...", &line[..at]),
        None => line,
    })
}

/// A pipe read to its end on a thread of its own, so neither pipe can fill
/// up and stall the program while the other is read.
struct Drain(mpsc::Receiver<(Vec<u8>, bool)>);

impl Drain {
    /// What was read, and whether it all fit, once the pipe closes. None
    /// when it stays open past the cleanup bound.
    fn wait(&self) -> Option<(Vec<u8>, bool)> {
        self.0.recv_timeout(process::CLEANUP_BOUND).ok()
    }
}

/// Past `limit`, `keep_end` keeps the newest bytes and otherwise the first.
fn drain(mut pipe: impl Read + Send + 'static, limit: usize, keep_end: bool) -> Drain {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut whole = true;
        let mut buffer = [0; 64 * 1024];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) if keep_end => {
                    kept.extend_from_slice(&buffer[..count]);
                    if kept.len() > limit {
                        whole = false;
                        kept.drain(..kept.len() - limit);
                    }
                }
                Ok(count) => {
                    let room = limit.saturating_sub(kept.len());
                    if count > room {
                        whole = false;
                    }
                    kept.extend_from_slice(&buffer[..count.min(room)]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = sender.send((kept, whole));
    });
    Drain(receiver)
}
