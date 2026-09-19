use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::FileExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::thread;
use std::time::Duration;

use jiff::Timestamp;
use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::files::write_atomic;
use crate::launch::Overrides;
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
    /// The process group the run was spawned into, so what it left behind can
    /// still be killed once whatever owned it is gone. Missing in a state file
    /// written before wineo recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u32>,
    pub started_at: Timestamp,
    /// The run lasts until the prefix goes idle, which can be well after the
    /// launched program exits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program_exited_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Command line overrides, kept so a restart can reuse them. The target is
    /// only kept when `--target` was passed, so a restart picks up a changed default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl RunState {
    pub fn running(
        mode: RunMode,
        target: String,
        pid: u32,
        group: Option<u32>,
        overrides: &Overrides,
    ) -> Self {
        Self {
            status: RunStatus::Running,
            mode,
            target,
            pid,
            group,
            started_at: Timestamp::now(),
            program_exited_at: None,
            ended_at: None,
            exit_code: None,
            requested_target: overrides.target.clone(),
            args: overrides.args.clone(),
            env: overrides.env.clone(),
        }
    }

    /// The launched program is gone. The timestamp is only recorded when the
    /// run can outlive it, since otherwise the program's exit and the run's end
    /// are the same moment and `ended_at` already says when.
    pub fn program_exited(&mut self, exit_code: Option<i32>, lingering: bool) {
        if lingering {
            self.program_exited_at = Some(Timestamp::now());
        }
        self.exit_code = exit_code;
    }

    pub fn finish(&mut self, status: RunStatus) {
        self.status = status;
        self.ended_at = Some(Timestamp::now());
    }

    /// The command line overrides this run was started with.
    pub fn overrides(&self) -> Overrides {
        Overrides {
            target: self.requested_target.clone(),
            env: self.env.clone(),
            args: self.args.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Exited,
    Failed,
    Stopped,
    /// Never started.
    Inactive,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Exited => "exited",
            State::Failed => "failed",
            State::Stopped => "stopped",
            State::Inactive => "inactive",
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An environment's state, derived from its run lock and `state.toml`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub state: State,
    /// The latest run. Missing if there never was one, or while a new run is
    /// still starting up.
    pub run: Option<RunState>,
    /// `state.toml` says running but nothing holds the lock, so whatever owned
    /// the run died before it could record how the run ended.
    pub lost: bool,
}

/// A probe from `is_running` holds a shared lock for a moment, so taking the
/// run lock retries for about this long before deciding another run holds it.
const LOCK_ATTEMPTS: u32 = 20;
const LOCK_RETRY: Duration = Duration::from_millis(5);

/// The runtime state directory of one environment.
#[derive(Debug, Clone)]
pub struct Runtime {
    dir: PathBuf,
}

/// Held for as long as an environment runs. The kernel drops the flock when
/// the holder dies, so a crash can never leave a stale lock behind.
#[derive(Debug)]
pub struct RunLock {
    file: File,
}

impl Drop for RunLock {
    fn drop(&mut self) {
        // Best effort. `stop` only trusts the pid while the lock is held, and
        // a cleared file keeps it from ever reading a pid that was reused.
        let _ = self.file.set_len(0);
    }
}

impl Runtime {
    pub fn new(paths: &Paths, name: &str) -> Self {
        Self::at(paths.runtime_dir(name))
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Takes the run lock without blocking and records this process as its
    /// holder. None means something else holds it.
    pub fn try_lock(&self) -> Result<Option<RunLock>> {
        let file = self.open_lock()?;
        let mut attempts = 1;
        while !self.flock(&file, FlockOperation::NonBlockingLockExclusive)? {
            if attempts == LOCK_ATTEMPTS {
                return Ok(None);
            }
            attempts += 1;
            thread::sleep(LOCK_RETRY);
        }
        let path = self.lock_path();
        file.set_len(0)
            .and_then(|()| file.write_all_at(format!("{}\n", std::process::id()).as_bytes(), 0))
            .context(|| format!("failed to write {}", path.display()))?;
        Ok(Some(RunLock { file }))
    }

    /// A momentary check, so the answer can be stale by the time it is used.
    pub fn is_running(&self) -> Result<bool> {
        if !self.lock_path().exists() {
            return Ok(false);
        }
        let file = self.open_lock()?;
        // Shared, so probes running at the same time never see each other as a run.
        Ok(!self.flock(&file, FlockOperation::NonBlockingLockShared)?)
    }

    /// The pid the lock holder recorded. None while a new holder is still
    /// writing it, or when nothing ran yet.
    pub fn holder_pid(&self) -> Result<Option<u32>> {
        let path = self.lock_path();
        match fs::read_to_string(&path) {
            Ok(text) => Ok(text.trim().parse().ok()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).context(|| format!("failed to read {}", path.display())),
        }
    }

    pub fn status(&self) -> Result<Status> {
        // State is read before the lock is checked, so a run that is just
        // starting never shows up with the previous run's details.
        let run = self.read_state()?;
        if self.is_running()? {
            return Ok(Status {
                state: State::Running,
                run: run.filter(|run| run.status == RunStatus::Running),
                lost: false,
            });
        }
        let run = match run {
            // The run may have finished between reading the state and checking the lock.
            Some(run) if run.status == RunStatus::Running => self.read_state()?,
            run => run,
        };
        let Some(run) = run else {
            return Ok(Status {
                state: State::Inactive,
                run: None,
                lost: false,
            });
        };
        let (state, lost) = match run.status {
            RunStatus::Running => (State::Failed, true),
            RunStatus::Exited => (State::Exited, false),
            RunStatus::Failed => (State::Failed, false),
            RunStatus::Stopped => (State::Stopped, false),
        };
        Ok(Status {
            state,
            run: Some(run),
            lost,
        })
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

    /// Output of the latest detached run.
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("output.log")
    }

    /// Output of the detached run before the latest one.
    pub fn previous_log_path(&self) -> PathBuf {
        self.dir.join("output.log.1")
    }

    /// Moves the latest log aside and opens a fresh one for a new run.
    pub fn rotate_log(&self) -> Result<File> {
        fs::create_dir_all(&self.dir)
            .context(|| format!("failed to create {}", self.dir.display()))?;
        let (log, previous) = (self.log_path(), self.previous_log_path());
        match fs::rename(&log, &previous) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).context(|| format!("failed to rotate {}", log.display())),
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .context(|| format!("failed to open {}", log.display()))
    }

    fn open_lock(&self) -> Result<File> {
        let path = self.lock_path();
        fs::create_dir_all(&self.dir)
            .context(|| format!("failed to create {}", self.dir.display()))?;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .context(|| format!("failed to open {}", path.display()))
    }

    /// Returns false if another open file holds a conflicting lock.
    fn flock(&self, file: &File, operation: FlockOperation) -> Result<bool> {
        match flock(file, operation) {
            Ok(()) => Ok(true),
            Err(Errno::WOULDBLOCK) => Ok(false),
            Err(err) => Err(io::Error::from(err))
                .context(|| format!("failed to lock {}", self.lock_path().display())),
        }
    }

    fn lock_path(&self) -> PathBuf {
        self.dir.join("lock")
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
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use super::*;

    fn runtime(root: &Path) -> Runtime {
        let home = root.to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        Runtime::new(&paths, "notepad")
    }

    fn run_state(status: RunStatus) -> RunState {
        RunState {
            status,
            mode: RunMode::Detached,
            target: "notepad".to_owned(),
            pid: 4242,
            group: Some(4242),
            started_at: "2026-09-16T21:02:11Z".parse().unwrap(),
            program_exited_at: None,
            ended_at: None,
            exit_code: None,
            requested_target: None,
            args: Vec::new(),
            env: BTreeMap::new(),
        }
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
    fn probes_never_mistake_each_other_for_a_run() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        drop(runtime.try_lock().unwrap());
        let until = Instant::now() + Duration::from_millis(300);
        let probers: Vec<_> = (0..4)
            .map(|_| {
                let runtime = runtime.clone();
                thread::spawn(move || {
                    let mut seen_running = false;
                    while Instant::now() < until {
                        seen_running |= runtime.is_running().unwrap();
                    }
                    seen_running
                })
            })
            .collect();
        for prober in probers {
            assert!(!prober.join().unwrap());
        }
    }

    #[test]
    fn a_new_run_gets_the_lock_while_something_probes() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        drop(runtime.try_lock().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let prober = {
            let (runtime, stop) = (runtime.clone(), stop.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    runtime.is_running().unwrap();
                    thread::sleep(Duration::from_millis(1));
                }
            })
        };
        for _ in 0..50 {
            assert!(runtime.try_lock().unwrap().is_some());
        }
        stop.store(true, Ordering::Relaxed);
        prober.join().unwrap();
    }

    #[test]
    fn lock_records_the_holder_pid_until_released() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        assert_eq!(runtime.holder_pid().unwrap(), None);
        assert!(!runtime.is_running().unwrap());

        let lock = runtime.try_lock().unwrap().unwrap();
        assert_eq!(runtime.holder_pid().unwrap(), Some(std::process::id()));
        assert!(runtime.is_running().unwrap());
        // Probing must not disturb the holder's pid.
        assert_eq!(runtime.holder_pid().unwrap(), Some(std::process::id()));

        drop(lock);
        assert_eq!(runtime.holder_pid().unwrap(), None);
        assert!(!runtime.is_running().unwrap());
    }

    #[test]
    fn state_round_trips() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        assert_eq!(runtime.read_state().unwrap(), None);

        let state = RunState {
            status: RunStatus::Stopped,
            mode: RunMode::Attached,
            program_exited_at: Some("2026-09-16T21:04:59Z".parse().unwrap()),
            ended_at: Some("2026-09-16T21:05:00Z".parse().unwrap()),
            exit_code: Some(0),
            requested_target: Some("notepad".to_owned()),
            args: vec!["-x".to_owned()],
            env: BTreeMap::from([("DXVK_HUD".to_owned(), "1".to_owned())]),
            ..run_state(RunStatus::Running)
        };
        runtime.write_state(&state).unwrap();
        assert_eq!(runtime.read_state().unwrap(), Some(state));
        assert_eq!(
            runtime.dir(),
            root.path().join(".local/state/wineo/notepad")
        );
    }

    #[test]
    fn state_without_newer_fields_still_parses() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        fs::create_dir_all(runtime.dir()).unwrap();
        fs::write(
            runtime.dir().join("state.toml"),
            "status = \"exited\"\nmode = \"attached\"\ntarget = \"notepad\"\npid = 1\nstarted_at = \"2026-09-16T21:02:11Z\"\n",
        )
        .unwrap();
        let state = runtime.read_state().unwrap().unwrap();
        assert_eq!(state.status, RunStatus::Exited);
        assert_eq!(state.requested_target, None);
        assert_eq!(state.group, None);
    }

    #[test]
    fn overrides_round_trip_through_state() {
        let overrides = Overrides {
            target: Some("bridge".to_owned()),
            env: BTreeMap::from([("A".to_owned(), "1".to_owned())]),
            args: vec!["-nosplash".to_owned()],
        };
        let mut state = RunState::running(
            RunMode::Detached,
            "bridge".to_owned(),
            7,
            Some(7),
            &overrides,
        );
        assert_eq!(state.status, RunStatus::Running);
        let kept = state.overrides();
        assert_eq!(kept.target, overrides.target);
        assert_eq!(kept.env, overrides.env);
        assert_eq!(kept.args, overrides.args);

        state.program_exited(Some(3), true);
        assert!(state.program_exited_at.is_some());
        assert_eq!(state.exit_code, Some(3));
        assert_eq!(state.ended_at, None);
        state.finish(RunStatus::Failed);
        assert_eq!(state.status, RunStatus::Failed);
        assert!(state.ended_at.is_some());
    }

    #[test]
    fn status_follows_lock_and_state() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let status = runtime.status().unwrap();
        assert_eq!(
            (status.state, status.run, status.lost),
            (State::Inactive, None, false)
        );

        for (recorded, state) in [
            (RunStatus::Exited, State::Exited),
            (RunStatus::Failed, State::Failed),
            (RunStatus::Stopped, State::Stopped),
        ] {
            runtime.write_state(&run_state(recorded)).unwrap();
            let status = runtime.status().unwrap();
            assert_eq!(status.state, state);
            assert!(!status.lost);
            assert_eq!(status.run.unwrap().status, recorded);
        }

        // A new run holds the lock before it writes its state.
        let lock = runtime.try_lock().unwrap().unwrap();
        let status = runtime.status().unwrap();
        assert_eq!((status.state, status.run), (State::Running, None));

        runtime.write_state(&run_state(RunStatus::Running)).unwrap();
        let status = runtime.status().unwrap();
        assert_eq!(status.state, State::Running);
        assert!(status.run.is_some());

        // The holder died without finishing its state.
        drop(lock);
        let status = runtime.status().unwrap();
        assert_eq!(status.state, State::Failed);
        assert!(status.lost);
        assert!(status.run.is_some());
    }

    #[test]
    fn rotating_keeps_one_previous_log() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        writeln!(runtime.rotate_log().unwrap(), "first").unwrap();
        assert!(!runtime.previous_log_path().exists());
        writeln!(runtime.rotate_log().unwrap(), "second").unwrap();
        writeln!(runtime.rotate_log().unwrap(), "third").unwrap();
        assert_eq!(fs::read_to_string(runtime.log_path()).unwrap(), "third\n");
        assert_eq!(
            fs::read_to_string(runtime.previous_log_path()).unwrap(),
            "second\n"
        );
    }

    #[test]
    fn exit_codes_follow_shell_convention() {
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
        assert_eq!(exit_code(ExitStatus::from_raw(9)), 137);
    }
}
