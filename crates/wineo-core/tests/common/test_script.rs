//! Test-only helpers that write executable files and make sure the processes
//! tests start don't outlive them. Shared by every test target through
//! `#[path]`, so there is one copy.

#![allow(dead_code)]

use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};

use rustix::process::{Pid, PidfdFlags, Signal, kill_process_group, pidfd_open, pidfd_send_signal};

/// Writes an executable script with mode 0o755; see [`write_executable`].
pub fn write_script(path: impl AsRef<Path>, body: impl AsRef<[u8]>) {
    write_executable(path, body, 0o755);
}

/// Writes `body` to `path` and sets `mode`, using a child process for both.
///
/// Tests run on parallel threads, and any thread that forks hands every open
/// descriptor to its child until that child execs. If this process held the
/// file open for writing at that moment, running the file would fail with
/// ETXTBSY ("Text file busy"). Writing from a child keeps the writable
/// descriptor out of the test process, so use this instead of `fs::write`
/// followed by `set_permissions` for anything a test goes on to execute.
pub fn write_executable(path: impl AsRef<Path>, body: impl AsRef<[u8]>, mode: u32) {
    let path = path.as_ref();
    // `--` keeps a file name starting with `-` from reading as an option.
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod \"$2\" -- \"$1\"", "sh"])
        .arg(path)
        .arg(format!("{mode:o}"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
    // Close stdin and reap the child before reporting a failed write, so a
    // failure never leaves a zombie behind.
    let written = {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(body.as_ref())
    };
    let status = child.wait();
    match (written, status) {
        (Ok(()), Ok(status)) if status.success() => {}
        (written, status) => panic!(
            "writing {}: write {written:?}, exit {status:?}",
            path.display()
        ),
    }
}

/// A spawned process that is killed and reaped when the guard drops, so a
/// failed assertion can't leave it running after the test.
///
/// It wraps the few `Child` calls tests need instead of exposing the child,
/// so it always knows whether the leader has been reaped. Once it has, the
/// group number is no longer reserved and may belong to an unrelated group by
/// now, so the drop never signals it.
pub struct KillOnDrop {
    child: Option<Child>,
    group: bool,
    reaped: bool,
}

impl KillOnDrop {
    /// Guards `child` alone.
    pub fn new(child: Child) -> Self {
        Self {
            child: Some(child),
            group: false,
            reaped: false,
        }
    }

    /// Guards `child` and its whole process group, for a child spawned with
    /// `process_group(0)` that starts processes of its own, like wineo
    /// running a fake winetricks. Killing only the child would leave those.
    pub fn group(child: Child) -> Self {
        Self {
            child: Some(child),
            group: true,
            reaped: false,
        }
    }

    pub fn id(&self) -> u32 {
        self.child.as_ref().unwrap().id()
    }

    /// Waits for the child to exit. Anything it left in its group is no
    /// longer killed on drop.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.child.as_mut().unwrap().wait()?;
        self.reaped = true;
        Ok(status)
    }

    /// Checks whether the child has exited, with the same effect on the
    /// group as [`wait`](Self::wait) once it has.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.as_mut().unwrap().try_wait()?;
        self.reaped |= status.is_some();
        Ok(status)
    }

    /// Waits for the child and collects its output. The guard is spent, so
    /// this is only for a child that is expected to end by itself: reading
    /// its piped output doesn't finish until everything holding it has
    /// exited.
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        self.child.take().unwrap().wait_with_output()
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // Only while the leader is still unreaped, which keeps its id from
        // being handed to anything else.
        if self.group
            && !self.reaped
            && let Some(pid) = Pid::from_raw(child.id() as i32)
        {
            let _ = kill_process_group(pid, Signal::KILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Kills the fake programs the fake wine and umu scripts left running under
/// `root`, found through the `fake-pids` files they write into their prefix.
///
/// For a fixture whose environment config is already gone, which a stop by
/// name can no longer reach. A process is only killed when it still has that
/// prefix as its `WINEPREFIX`, which the fakes and their `sleep` inherit, so a
/// pid the system has handed to something else is left alone.
pub fn kill_fake_programs(root: &Path) {
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                folders.push(entry.path());
            } else if kind.is_file()
                && matches!(
                    entry.file_name().to_str(),
                    Some("fake-pids" | "forest-pids")
                )
            {
                kill_recorded(&folder, &entry.path());
            }
        }
    }
}

fn kill_recorded(prefix: &Path, pids: &Path) {
    let Ok(text) = fs::read_to_string(pids) else {
        return;
    };
    for pid in text.split_whitespace().filter_map(|pid| pid.parse().ok()) {
        let Some(pid) = Pid::from_raw(pid) else {
            continue;
        };
        // The descriptor pins this process, so the checks and the signal
        // can't land on something that took over its number meanwhile.
        let Ok(pidfd) = pidfd_open(pid, PidfdFlags::empty()) else {
            continue;
        };
        if runs_in_prefix(pid.as_raw_nonzero().get(), prefix) {
            let _ = pidfd_send_signal(&pidfd, Signal::KILL);
        }
    }
}

fn runs_in_prefix(pid: i32, prefix: &Path) -> bool {
    let Ok(environment) = fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    let prefix = fs::canonicalize(prefix).unwrap_or_else(|_| prefix.to_owned());
    // wineo canonicalizes the prefix it hands to Wine, so the fixture root
    // may reach the same folder through a symlink.
    environment
        .split(|byte| *byte == 0)
        .filter_map(|var| var.strip_prefix(b"WINEPREFIX="))
        .any(|value| {
            let value = PathBuf::from(std::ffi::OsStr::from_bytes(value));
            fs::canonicalize(&value).unwrap_or(value) == prefix
        })
}
