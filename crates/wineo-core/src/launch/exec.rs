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
/// How long a group a cancel killed gets to finish exiting before an exec
/// stops waiting for it regardless, so a cancel always ends the exec.
const KILL_GRACE: Duration = Duration::from_secs(2);

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
/// exits. A program run from a GOG game folder also holds the folder's
/// in-use lock until its whole process group has exited, so a launcher that
/// hands off to the game keeps the folder in use until the game quits.
/// Cancellation only signals its process group, never the whole prefix. A
/// nonzero or canceled process exit is returned as a status, the started
/// process's own. Cancellation before spawning is an error. No run record or
/// playtime is written, and a frontend crash leaves the program running
/// while releasing its locks.
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
    let mut launch = Launch::exec_with_resolution(
        paths,
        &global,
        &environment,
        program,
        args,
        env,
        ExecResolution::ExplicitPaths,
    )?;
    let prefix = launch.host.prefix().expect("exec always runs in a prefix");
    let guard = Locks::new(paths).acquire(Operation::Launch, name, &[prefix])?;
    Environment::ensure_exists(paths, name)?;
    Runtime::new(paths, name).check_resolved(name)?;
    let folders = super::orchestration::protect_as(paths, &environment, &launch, token, "exec")?;
    reconcile(paths, &global, &environment, &launch, |event| {
        events(ExecEvent::Launch(event));
    })?;
    check_cancel(token)?;
    let builds = std::mem::take(&mut launch.build_guards);
    let release = move || drop((guard, _build_guard, builds));
    let result = run(
        &launch,
        global.stop_timeout,
        token,
        !folders.is_empty(),
        release,
        &mut events,
    );
    drop(folders);
    result
}

fn check_cancel(token: &Token) -> anyhow::Result<()> {
    if token.is_canceled() {
        return Err(Error::Canceled("exec").into());
    }
    Ok(())
}

/// Runs the program until it exits, calling `release` then. With `whole_group`
/// it then keeps going, still cancelable, until nothing in its process group
/// is left.
fn run(
    launch: &Launch,
    stop_timeout: u32,
    token: &Token,
    whole_group: bool,
    release: impl FnOnce(),
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
    let mut killed_at = None;
    let mut exited = None;
    let mut release = Some(release);
    let result = (|| {
        loop {
            let status = match exited {
                Some(status) => status,
                None => match child.try_wait().context("failed to wait for exec")? {
                    Some(status) => {
                        exited = Some(status);
                        // Waiting on the rest of the group needs only the
                        // folder locks; otherwise everything lasts through
                        // the exit event, as it always has.
                        if whole_group && let Some(release) = release.take() {
                            release();
                        }
                        status
                    }
                    None => {
                        stop(
                            token,
                            group,
                            stop_timeout,
                            &mut stopping,
                            &mut killing,
                            events,
                        )?;
                        lines.pump(READ_BATCH, events)?;
                        token.wait_timeout(POLL);
                        continue;
                    }
                },
            };
            if !whole_group || settled(group_live(group), killing, killed_at) {
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
            stop(
                token,
                group,
                stop_timeout,
                &mut stopping,
                &mut killing,
                events,
            )?;
            if killing && killed_at.is_none() {
                killed_at = Some(Instant::now());
            }
            lines.pump(READ_BATCH, events)?;
            token.wait_timeout(POLL);
        }
    })();
    if result.is_err() && exited.is_none() {
        let _ = signal(group, Signal::KILL);
        let _ = child.wait();
    }
    result
}

/// Whether anything in the group can still run. The kernel says for sure
/// when the group is gone, with no `/proc` needed. A group that's left can
/// hold just a zombie nobody reaps, like a worker whose parent left the
/// group, so it's then looked at member by member.
fn group_live(group: u32) -> Option<bool> {
    match crate::process::group_still_exists(group) {
        Some(false) => Some(false),
        _ => crate::process::group_has_live_members(group),
    }
}

/// Whether the wait on a group is over. Unknown counts as live, so a folder
/// is never let go early, unless a cancel has already killed the group,
/// which then gets `KILL_GRACE` at most.
fn settled(live: Option<bool>, killing: bool, killed_at: Option<Instant>) -> bool {
    let gone = match live {
        Some(live) => !live,
        None => killing,
    };
    gone || killed_at.is_some_and(|at| at.elapsed() >= KILL_GRACE)
}

/// Signals the group once canceled, and kills it once forced or past the
/// stop timeout.
fn stop(
    token: &Token,
    group: u32,
    stop_timeout: u32,
    stopping: &mut Option<Instant>,
    killing: &mut bool,
    events: &mut impl FnMut(ExecEvent),
) -> anyhow::Result<()> {
    if token.is_canceled() && stopping.is_none() {
        *stopping = Some(Instant::now());
        events(ExecEvent::Stopping);
        signal(group, Signal::TERM)?;
    }
    if !*killing
        && stopping.is_some_and(|at| {
            token.is_forced() || at.elapsed() >= Duration::from_secs(stop_timeout.into())
        })
    {
        *killing = true;
        events(ExecEvent::Killing);
        signal(group, Signal::KILL)?;
    }
    Ok(())
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
    fn a_killed_group_ends_the_wait_even_when_it_cant_be_inspected() {
        assert!(settled(Some(false), false, None));
        assert!(!settled(Some(true), false, None));
        assert!(!settled(None, false, None));
        assert!(settled(None, true, Some(Instant::now())));
        assert!(!settled(Some(true), true, Some(Instant::now())));
        let long_ago = Instant::now() - KILL_GRACE;
        assert!(settled(Some(true), true, Some(long_ago)));
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
