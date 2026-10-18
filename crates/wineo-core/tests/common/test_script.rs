//! Test-only helpers that write executable files and make sure the processes
//! tests start don't outlive them. Shared by every test target through
//! `#[path]`, so there is one copy.

#![allow(dead_code)]

use std::fs;
use std::io::{self, Write};
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

use rustix::process::{Pid, Signal, kill_process, kill_process_group};

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
pub struct KillOnDrop {
    child: Option<Child>,
    group: bool,
}

impl KillOnDrop {
    /// Guards `child` alone.
    pub fn new(child: Child) -> Self {
        Self {
            child: Some(child),
            group: false,
        }
    }

    /// Guards `child` and its whole process group, for a child spawned with
    /// `process_group(0)` that starts processes of its own, like wineo
    /// running a fake winetricks. Killing only the child would leave those.
    pub fn group(child: Child) -> Self {
        Self {
            child: Some(child),
            group: true,
        }
    }

    /// Waits for the child and collects its output. The guard is spent, so
    /// this is only for a child that is expected to end by itself: reading
    /// its piped output doesn't finish until everything holding it has
    /// exited.
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        self.child.take().unwrap().wait_with_output()
    }
}

impl Deref for KillOnDrop {
    type Target = Child;

    fn deref(&self) -> &Child {
        self.child.as_ref().unwrap()
    }
}

impl DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut Child {
        self.child.as_mut().unwrap()
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // Before the reap below, while the id is still the child's own.
        if self.group
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
/// name can no longer reach. A pid is only killed when the process still has
/// that prefix as its `WINEPREFIX`, which the fakes and their `sleep` inherit,
/// so a pid the system has handed to something else is left alone.
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
        if runs_in_prefix(pid, prefix)
            && let Some(pid) = Pid::from_raw(pid as i32)
        {
            let _ = kill_process(pid, Signal::KILL);
        }
    }
}

fn runs_in_prefix(pid: u32, prefix: &Path) -> bool {
    let Ok(environment) = fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    let mut wanted = b"WINEPREFIX=".to_vec();
    wanted.extend_from_slice(prefix.as_os_str().as_encoded_bytes());
    environment
        .split(|byte| *byte == 0)
        .any(|var| var == wanted)
}
