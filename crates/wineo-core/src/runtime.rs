use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use jiff::Timestamp;
use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::files::write_atomic;
use crate::paths::Paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Running,
    Exited,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    Detached,
    Attached,
}

/// What `state.toml` records about the latest run of an environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub status: RunStatus,
    pub mode: RunMode,
    pub target: String,
    pub pid: u32,
    pub started_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Command line overrides, kept so a restart can reuse them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// The runtime state directory of one environment.
#[derive(Debug, Clone)]
pub struct Runtime {
    dir: PathBuf,
}

/// Held for as long as an environment runs. The kernel drops the flock when
/// the holder dies, so a crash can never leave a stale lock behind.
#[derive(Debug)]
pub struct RunLock {
    _file: File,
}

impl Runtime {
    pub fn new(paths: &Paths, name: &str) -> Self {
        Self {
            dir: paths.runtime_dir(name),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Takes the run lock without blocking. None means something else holds it.
    pub fn try_lock(&self) -> Result<Option<RunLock>> {
        let path = self.dir.join("lock");
        fs::create_dir_all(&self.dir)
            .context(|| format!("failed to create {}", self.dir.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .context(|| format!("failed to open {}", path.display()))?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Some(RunLock { _file: file })),
            Err(Errno::WOULDBLOCK) => Ok(None),
            Err(err) => {
                Err(io::Error::from(err)).context(|| format!("failed to lock {}", path.display()))
            }
        }
    }

    pub fn read_state(&self) -> Result<Option<RunState>> {
        let path = self.state_path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
        };
        toml::from_str(&text)
            .map(Some)
            .map_err(|source| Error::StateSyntax {
                path,
                source: Box::new(source),
            })
    }

    pub fn write_state(&self, state: &RunState) -> Result<()> {
        let text = toml::to_string(state).expect("run state always serializes");
        write_atomic(&self.state_path(), &text)
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.toml")
    }
}

/// Shell convention: a process killed by a signal reports 128 plus the signal number.
pub fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn runtime(root: &Path) -> Runtime {
        let home = root.to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        Runtime::new(&paths, "notepad")
    }

    #[test]
    fn lock_is_exclusive_until_dropped() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let first = runtime.try_lock().unwrap();
        assert!(first.is_some());
        assert!(runtime.try_lock().unwrap().is_none());
        drop(first);
        assert!(runtime.try_lock().unwrap().is_some());
    }

    #[test]
    fn state_round_trips() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        assert_eq!(runtime.read_state().unwrap(), None);

        let state = RunState {
            status: RunStatus::Stopped,
            mode: RunMode::Attached,
            target: "notepad".to_owned(),
            pid: 4242,
            started_at: "2026-09-16T21:02:11Z".parse().unwrap(),
            ended_at: Some("2026-09-16T21:05:00Z".parse().unwrap()),
            exit_code: Some(0),
            args: vec!["-x".to_owned()],
            env: BTreeMap::from([("DXVK_HUD".to_owned(), "1".to_owned())]),
        };
        runtime.write_state(&state).unwrap();
        assert_eq!(runtime.read_state().unwrap(), Some(state));
        assert_eq!(
            runtime.dir(),
            root.path().join(".local/state/wineo/notepad")
        );
    }

    #[test]
    fn exit_codes_follow_shell_convention() {
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
        assert_eq!(exit_code(ExitStatus::from_raw(9)), 137);
    }
}
