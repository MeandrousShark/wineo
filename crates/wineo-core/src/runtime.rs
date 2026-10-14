use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::FileExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use jiff::Timestamp;
use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use crate::config::Launcher;
use crate::error::{Error, IoContext, Result, chain};
use crate::files::{self, write_atomic};
use crate::launch::{Host, Overrides};
use crate::operation::Operation;
use crate::paths::Paths;
use crate::process::GroupOwnership;
use crate::runner::{self, Killed, Runner};

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
pub enum RunPhase {
    Starting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    Detached,
    Attached,
}

/// What a recorded run ran on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Wine,
    Native,
    Proxied(Launcher),
}

/// What `state.toml` records about the latest run of an environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub status: RunStatus,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lost: bool,
    /// Present only while Steam has accepted a launch but its reaper has not appeared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<RunPhase>,
    pub mode: RunMode,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<Runner>,
    /// Concrete prefix used by this run, preserved if its alias changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<PathBuf>,
    /// A native run, which has no runner or prefix and is tracked only
    /// through its process group.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native: bool,
    /// The launcher of a proxied run. Never set in a file from an older wineo,
    /// so those still read as Wine or native runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<Launcher>,
    pub pid: u32,
    /// The process group the run was spawned into, so what it left behind can
    /// still be killed once whatever owned it is gone. Missing in a state file
    /// written before wineo recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u32>,
    /// The boot and leader identity needed to recover a lost process group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader_start: Option<u64>,
    pub started_at: Timestamp,
    /// Steam play time begins when its reaper is claimed, not at dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_at: Option<Timestamp>,
    /// Stamped about once a minute while the run is live, so a lost run's
    /// session can end here instead of whenever it is cleaned up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_at: Option<Timestamp>,
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
            lost: false,
            phase: None,
            mode,
            target,
            runner: None,
            prefix: None,
            native: false,
            launcher: None,
            pid,
            group,
            boot_id: None,
            leader_start: None,
            started_at: Timestamp::now(),
            running_at: None,
            heartbeat_at: None,
            program_exited_at: None,
            ended_at: None,
            exit_code: None,
            requested_target: overrides.target.clone(),
            args: overrides.args.clone(),
            env: overrides.env.clone(),
        }
    }

    /// Records what the run runs on, so a lost one can be cleaned up.
    pub fn record_host(&mut self, host: &Host) {
        match host {
            Host::Wine { runner, prefix } => {
                self.runner = Some(runner.clone());
                self.prefix = Some(prefix.clone());
            }
            Host::Native { .. } => self.native = true,
            Host::Proxied { .. } => {
                self.launcher = Some(Launcher::Steam);
                self.group = None;
            }
        }
    }

    pub fn starting(&mut self) {
        self.phase = Some(RunPhase::Starting);
    }

    pub fn claimed(&mut self, pid: u32, boot_id: String, start_time: u64) {
        self.phase = None;
        self.pid = pid;
        self.boot_id = Some(boot_id);
        self.leader_start = Some(start_time);
        self.running_at = Some(Timestamp::now());
    }

    /// What the run ran on, from the fields `state.toml` has always had.
    pub fn kind(&self) -> RunKind {
        match (self.launcher, self.native) {
            (Some(launcher), _) => RunKind::Proxied(launcher),
            (None, true) => RunKind::Native,
            (None, false) => RunKind::Wine,
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

    /// Stamps that the run is still alive, for a lost run's session to end at.
    pub fn heartbeat(&mut self) {
        self.heartbeat_at = Some(Timestamp::now());
    }

    pub fn finish(&mut self, status: RunStatus) {
        self.status = status;
        self.phase = None;
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

/// What `operation.toml` records while an operation runs programs in the
/// prefix, so what it started can still be found and stopped if wineo dies
/// first. It stays until everything has stopped, which is what keeps other
/// work refused until then.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationState {
    pub operation: Operation,
    /// What was run, for messages.
    pub program: String,
    /// Resolved when the operation started, so a cleanup reaches the same
    /// wineserver even if the environment's runner has changed since.
    pub runner: Runner,
    /// The configured prefix, which is what `Runner` methods take.
    pub prefix: PathBuf,
    /// Missing until the program is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u32>,
    /// The boot it ran in and when its group's leader started, so a cleanup
    /// never signals a later process group that reused the number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader_start: Option<u64>,
    pub started_at: Timestamp,
}

impl OperationState {
    pub fn identity(&self) -> Option<crate::process::Identity> {
        crate::process::Identity::recorded(self.group, self.boot_id.as_deref(), self.leader_start)
    }

    pub fn group_ownership(&self) -> crate::process::GroupOwnership {
        crate::process::group_ownership(self.identity().as_ref())
    }
}

impl RunState {
    pub fn identity(&self) -> Option<crate::process::Identity> {
        crate::process::Identity::recorded(self.group, self.boot_id.as_deref(), self.leader_start)
    }

    pub fn group_ownership(&self) -> crate::process::GroupOwnership {
        crate::process::group_ownership(self.identity().as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Starting,
    #[serde(rename = "running")]
    SteamRunning,
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
            State::Starting => "starting",
            State::SteamRunning => "running (started from Steam)",
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

/// What cleaning up after a lost run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LostRunCleanup {
    /// There was no lost run, or another command dealt with it first.
    NotLost,
    /// What the run started had already exited. `warning` is set if its
    /// session could not be logged.
    Gone { warning: Option<String> },
    /// What the run left behind was killed. `warning` is set if its session
    /// could not be logged.
    Killed { warning: Option<String> },
    /// Ownership or cleanup could not be confirmed, so the state is kept.
    /// `warning` is the error behind it, when there is one.
    Unresolved { warning: Option<String> },
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
        // A forked child can hold a copy of the fd, which keeps a flock held on
        // the open file description until it exits or execs. Unlocking
        // explicitly frees it the moment this holder is done, whatever copies
        // still exist.
        let _ = flock(&self.file, FlockOperation::Unlock);
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
        Ok(!self.probe(&file)?)
    }

    /// A single shared attempt, released explicitly the instant it succeeds so
    /// a forked child holding a copy of the fd can never keep it held.
    fn probe(&self, file: &File) -> Result<bool> {
        // Shared, so probes running at the same time never see each other as a run.
        let acquired = self.flock(file, FlockOperation::NonBlockingLockShared)?;
        if acquired {
            let _ = flock(file, FlockOperation::Unlock);
        }
        Ok(acquired)
    }

    /// The pid the lock holder recorded. None while a new holder is still
    /// writing it, or when nothing ran yet.
    pub fn holder_pid(&self) -> Result<Option<u32>> {
        let path = self.lock_path();
        let text = files::read_to_string_optional(&path)
            .context(|| format!("failed to read {}", path.display()))?;
        Ok(text.and_then(|text| text.trim().parse().ok()))
    }

    pub fn status(&self) -> Result<Status> {
        // State is read before the lock is checked, so a run that is just
        // starting never shows up with the previous run's details.
        let run = self.read_state()?;
        if self.is_running()? {
            return Ok(Status {
                state: if run
                    .as_ref()
                    .is_some_and(|run| run.phase == Some(RunPhase::Starting))
                {
                    State::Starting
                } else {
                    State::Running
                },
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
        let Some(text) = files::read_to_string_optional(&path)
            .context(|| format!("failed to read {}", path.display()))?
        else {
            return Ok(None);
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

    /// The operation `operation.toml` records. To a caller holding an operation
    /// lock on the environment, one being there means whatever wrote it is gone
    /// without having confirmed that everything it started has stopped.
    pub fn read_operation(&self) -> Result<Option<OperationState>> {
        let path = self.operation_path();
        let Some(text) = files::read_to_string_optional(&path)
            .context(|| format!("failed to read {}", path.display()))?
        else {
            return Ok(None);
        };
        toml::from_str(&text)
            .map(Some)
            .map_err(|source| Error::StateSyntax {
                path,
                source: Box::new(source),
            })
    }

    pub fn write_operation(&self, state: &OperationState) -> Result<()> {
        let text = toml::to_string(state).expect("operation state always serializes");
        write_atomic(&self.operation_path(), &text)
    }

    pub fn clear_operation(&self) -> Result<()> {
        let path = self.operation_path();
        files::remove_if_present(&path)
            .map(drop)
            .context(|| format!("failed to remove {}", path.display()))
    }

    /// Refuses work that needs the prefix to itself: a run going on, a run
    /// whose owner died, or an operation that never confirmed it was done.
    /// Only meaningful under an exclusive operation lock.
    pub fn check_quiet(&self, name: &str) -> Result<()> {
        if self.is_running()? {
            return Err(Error::Running {
                name: name.to_owned(),
                action: "first",
            });
        }
        match self.status() {
            Ok(status) if status.lost => return Err(Error::LostRun(name.to_owned())),
            Err(Error::StateSyntax { .. }) => return Err(Error::LostRun(name.to_owned())),
            Err(err) => return Err(err),
            Ok(_) => {}
        }
        self.check_resolved(name)
    }

    /// Refuses anything that would use the prefix while an operation that
    /// never confirmed it was done may still have programs running in it.
    pub fn check_resolved(&self, name: &str) -> Result<()> {
        match self.read_operation() {
            Ok(None) => Ok(()),
            // An unreadable record still says something was left unresolved.
            Ok(Some(_)) | Err(Error::StateSyntax { .. }) => Err(Error::Unresolved(name.to_owned())),
            Err(err) => Err(err),
        }?;
        match self.status() {
            Ok(status) if status.lost => Err(Error::LostRun(name.to_owned())),
            Err(Error::StateSyntax { .. }) => Err(Error::LostRun(name.to_owned())),
            Err(err) => Err(err),
            Ok(_) => Ok(()),
        }
    }

    /// Kills whatever a run left in the prefix when the process that owned it
    /// died, then records the run as stopped so this only happens once. Its
    /// session is logged to `stats_log` as `lost`, ending at its last
    /// heartbeat rather than whenever this cleanup happened to run.
    pub fn clean_up_lost_run(
        &self,
        settle_timeout: Duration,
        stats_log: &Path,
    ) -> Result<LostRunCleanup> {
        let unresolved = |warning: Option<String>| Ok(LostRunCleanup::Unresolved { warning });
        match self.status() {
            Ok(status) if status.lost => {}
            Ok(_) => return Ok(LostRunCleanup::NotLost),
            Err(err) => return unresolved(Some(chain(&err))),
        }
        // Holding the lock keeps a new run from starting in the middle of this,
        // and the state is read again in case one already came and went.
        let Some(_lock) = self.try_lock()? else {
            return Ok(LostRunCleanup::NotLost);
        };
        let mut run = match self.read_state() {
            Ok(Some(run)) if run.status == RunStatus::Running => run,
            Ok(_) => return Ok(LostRunCleanup::NotLost),
            Err(err) => return unresolved(Some(chain(&err))),
        };
        if matches!(run.kind(), RunKind::Proxied(_)) {
            run.finish(RunStatus::Stopped);
            run.lost = true;
            run.ended_at = Some(run.heartbeat_at.unwrap_or(run.started_at));
            self.write_state(&run)?;
            return Ok(LostRunCleanup::Gone { warning: None });
        }
        // None for a native run, which is cleaned up through its group like Proton.
        let wine = match (run.kind(), run.runner.clone(), run.prefix.clone()) {
            (RunKind::Wine, Some(runner), Some(prefix)) => Some((runner, prefix)),
            (RunKind::Native, None, None) => None,
            (RunKind::Proxied(_), ..) => unreachable!("handled above"),
            _ => return unresolved(None),
        };
        let wineserver = wine.as_ref().is_some_and(|(runner, _)| !runner.is_proton());
        let Some(identity) = run.identity() else {
            return unresolved(None);
        };
        let group = match identity.ownership() {
            GroupOwnership::Unknown => return unresolved(None),
            GroupOwnership::Gone => GroupOwnership::Gone,
            GroupOwnership::Owned => {
                match identity.signal_if_owned(rustix::process::Signal::KILL) {
                    Ok(ownership) => ownership,
                    Err(err) => return unresolved(Some(chain(&err))),
                }
            }
        };
        if group == GroupOwnership::Unknown {
            return unresolved(None);
        }
        if let Some((runner, prefix)) = &wine
            && wineserver
        {
            match runner.kill(prefix, run.group) {
                Ok(Killed::Untracked) => return unresolved(None),
                Ok(Killed::Done | Killed::Gone) => {}
                Err(err) => return unresolved(Some(chain(&err))),
            }
        }
        if (wineserver || group == GroupOwnership::Owned)
            && let Err(err) = wait_settled_bounded(settle_timeout, move || match &wine {
                Some((runner, prefix)) => runner.wait_settled_owned(prefix, Some(&identity)),
                None => runner::wait_for_recorded_group(Some(&identity)),
            })
        {
            return unresolved(Some(chain(&err)));
        }
        // The state write is what makes this idempotent: it stops any retry
        // from seeing the run as lost again, so it must go first and its
        // error must stop this from appending a second `lost` session.
        run.finish(RunStatus::Stopped);
        self.write_state(&run)?;
        let ended_at = run.heartbeat_at.unwrap_or(run.started_at);
        // Best effort: a lost session is worth logging, but not worth failing
        // the cleanup over if it can't be, since the state is already saved.
        let warning = crate::stats::append(
            stats_log,
            &crate::stats::Session::new(
                run.target.clone(),
                run.started_at,
                ended_at,
                crate::stats::End::Lost,
            ),
        )
        .err()
        .map(|err| chain(&err));
        Ok(if group == GroupOwnership::Gone {
            LostRunCleanup::Gone { warning }
        } else {
            LostRunCleanup::Killed { warning }
        })
    }

    /// What to tell someone when a lost run could not be cleaned up.
    pub fn unresolved_run_message(&self, name: &str) -> String {
        format!(
            "wineo could not confirm ownership of {name}'s last run. Verify its processes have stopped, then remove {} to clear this block",
            self.state_path().display()
        )
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

    /// Where an operation's recovery record is, for telling someone to delete
    /// it when nothing else can clear it.
    pub fn operation_path(&self) -> PathBuf {
        self.dir.join("operation.toml")
    }

    /// Where an installer's output goes when it is not shown on the terminal.
    pub fn install_log_path(&self) -> PathBuf {
        self.dir.join("install.log")
    }
}

fn wait_settled_bounded(
    timeout: Duration,
    wait: impl FnOnce() -> Result<()> + Send + 'static,
) -> Result<()> {
    let (sender, settled) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(wait());
    });
    match settled.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(Error::Unsettled(format!(
            "cleanup did not settle within {} ms",
            timeout.as_millis()
        ))),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Unsettled(
            "cleanup settlement check stopped unexpectedly".to_owned(),
        )),
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
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use super::*;
    use crate::process;

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
            lost: false,
            phase: None,
            mode: RunMode::Detached,
            target: "notepad".to_owned(),
            runner: None,
            prefix: None,
            native: false,
            launcher: None,
            pid: 4242,
            group: Some(4242),
            boot_id: None,
            leader_start: None,
            started_at: "2026-09-16T21:02:11Z".parse().unwrap(),
            running_at: None,
            heartbeat_at: None,
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
    fn run_lock_releases_even_if_a_forked_child_still_holds_the_fd() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let lock = runtime.try_lock().unwrap().unwrap();
        // Stands in for a forked child's copy of the fd, kept alive past the drop.
        let clone = lock.file.try_clone().unwrap();
        drop(lock);
        assert!(runtime.try_lock().unwrap().is_some());
        drop(clone);
    }

    #[test]
    fn is_running_probe_releases_its_lock_even_if_a_forked_child_still_holds_the_fd() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let file = runtime.open_lock().unwrap();
        // Stands in for a forked child's copy of the fd, kept alive past the probe.
        let clone = file.try_clone().unwrap();
        assert!(runtime.probe(&file).unwrap());
        drop(file);
        let fresh = runtime.open_lock().unwrap();
        assert!(
            runtime
                .flock(&fresh, FlockOperation::NonBlockingLockExclusive)
                .unwrap()
        );
        drop(clone);
    }

    #[test]
    fn dropping_and_retaking_the_run_lock_survives_concurrent_forks() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let stop = Arc::new(AtomicBool::new(false));
        let forkers: Vec<_> = (0..4)
            .map(|_| {
                let stop = stop.clone();
                thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let _ = std::process::Command::new("true").status();
                    }
                })
            })
            .collect();
        for _ in 0..500 {
            let lock = runtime.try_lock().unwrap();
            assert!(lock.is_some(), "the previous drop should have freed it");
            drop(lock);
        }
        stop.store(true, Ordering::Relaxed);
        for forker in forkers {
            forker.join().unwrap();
        }
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
            prefix: None,
            boot_id: None,
            leader_start: None,
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

    /// `state.toml` files from before proxied runs read as the kind they
    /// always were.
    #[test]
    fn older_state_files_keep_their_kind() {
        let base = "status = \"running\"\nmode = \"detached\"\ntarget = \"game\"\npid = 1\nstarted_at = \"2026-09-16T21:02:11Z\"\n";
        for (extra, kind) in [
            ("native = true\n", RunKind::Native),
            ("native = false\n", RunKind::Wine),
            ("", RunKind::Wine),
        ] {
            let root = tempfile::tempdir().unwrap();
            let runtime = runtime(root.path());
            fs::create_dir_all(runtime.dir()).unwrap();
            fs::write(runtime.dir().join("state.toml"), format!("{base}{extra}")).unwrap();
            let state = runtime.read_state().unwrap().unwrap();
            assert_eq!(state.kind(), kind, "{extra:?}");
            assert_eq!(state.launcher, None, "{extra:?}");
        }
    }

    #[test]
    fn a_proxied_run_round_trips_its_launcher() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let state = RunState {
            launcher: Some(Launcher::Steam),
            ..run_state(RunStatus::Running)
        };
        runtime.write_state(&state).unwrap();
        let text = fs::read_to_string(runtime.dir().join("state.toml")).unwrap();
        assert!(text.contains("launcher = \"steam\""), "{text}");
        assert!(!text.contains("native"), "{text}");
        let read = runtime.read_state().unwrap().unwrap();
        assert_eq!(read.kind(), RunKind::Proxied(Launcher::Steam));
        assert_eq!(read, state);
    }

    /// A lost Steam supervisor releases the run while leaving Steam's live tree alone.
    #[test]
    fn a_lost_proxied_run_is_released_without_signaling() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut run = run_state(RunStatus::Running);
        run.launcher = Some(Launcher::Steam);
        run.pid = pid;
        run.group = Some(pid);
        run.boot_id = process::boot_id();
        run.leader_start = process::start_time(pid);
        run.heartbeat_at = Some("2026-09-16T21:03:00Z".parse().unwrap());
        assert_eq!(run.group_ownership(), GroupOwnership::Owned);
        runtime.write_state(&run).unwrap();
        assert!(!runtime.is_running().unwrap());
        let stats_log = root.path().join("stats.jsonl");
        let cleaned = runtime
            .clean_up_lost_run(Duration::from_secs(1), &stats_log)
            .unwrap();
        assert!(
            matches!(cleaned, LostRunCleanup::Gone { warning: None }),
            "{cleaned:?}"
        );
        assert_eq!(
            runtime.read_state().unwrap().unwrap().status,
            RunStatus::Stopped
        );
        assert_eq!(
            runtime.read_state().unwrap().unwrap().ended_at,
            run.heartbeat_at
        );
        assert!(runtime.read_state().unwrap().unwrap().lost);
        assert!(child.try_wait().unwrap().is_none());
        assert!(runtime.try_lock().unwrap().is_some());
        runtime.check_quiet("game").unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!stats_log.exists());
    }

    #[test]
    fn a_starting_steam_run_keeps_the_legacy_running_status_on_disk() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let mut run = run_state(RunStatus::Running);
        run.launcher = Some(Launcher::Steam);
        run.starting();
        let lock = runtime.try_lock().unwrap().unwrap();
        runtime.write_state(&run).unwrap();
        let text = fs::read_to_string(runtime.state_path()).unwrap();
        assert!(text.contains("status = \"running\""), "{text}");
        assert!(text.contains("phase = \"starting\""), "{text}");
        assert_eq!(runtime.status().unwrap().state, State::Starting);
        drop(lock);
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
    fn check_quiet_refuses_runs_lost_runs_and_unresolved_operations() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        runtime.check_quiet("notepad").unwrap();

        let lock = runtime.try_lock().unwrap().unwrap();
        assert!(matches!(
            runtime.check_quiet("notepad"),
            Err(Error::Running { .. })
        ));
        runtime.write_state(&run_state(RunStatus::Running)).unwrap();
        drop(lock);
        assert!(matches!(
            runtime.check_quiet("notepad"),
            Err(Error::LostRun(_))
        ));

        runtime.write_state(&run_state(RunStatus::Stopped)).unwrap();
        runtime
            .write_operation(&OperationState {
                operation: Operation::Install,
                program: "setup.exe".to_owned(),
                runner: Runner::fake("wine"),
                prefix: root.path().join("prefix"),
                pid: None,
                group: None,
                boot_id: None,
                leader_start: None,
                started_at: Timestamp::now(),
            })
            .unwrap();
        assert!(matches!(
            runtime.check_quiet("notepad"),
            Err(Error::Unresolved(_))
        ));
        runtime.clear_operation().unwrap();
        runtime.check_quiet("notepad").unwrap();

        // An unreadable state file cannot make the lost-run block disappear.
        fs::write(runtime.dir().join("state.toml"), "garbage [").unwrap();
        assert!(matches!(
            runtime.check_quiet("notepad"),
            Err(Error::LostRun(_))
        ));
        assert!(matches!(
            runtime.check_resolved("notepad"),
            Err(Error::LostRun(_))
        ));
        assert!(runtime.state_path().is_file());
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

    #[test]
    fn a_lost_run_is_logged_exactly_once_even_across_a_failed_state_write() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let mut run = run_state(RunStatus::Running);
        run.native = true;
        // A boot id that never matches the current one, so ownership resolves
        // to `Gone` without touching any real process.
        run.boot_id = Some("an-earlier-boot".to_owned());
        run.leader_start = Some(1);
        run.heartbeat_at = Some("2026-09-16T21:03:00Z".parse().unwrap());
        runtime.write_state(&run).unwrap();
        let stats_log = root.path().join("stats.jsonl");

        // The runtime dir can't be written to, so the state write inside
        // cleanup fails before anything is appended.
        let dir = runtime.dir().to_owned();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let failed = runtime.clean_up_lost_run(Duration::from_secs(5), &stats_log);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(failed.is_err(), "{failed:?}");
        assert!(!stats_log.exists());
        assert_eq!(
            runtime.read_state().unwrap().unwrap().status,
            RunStatus::Running,
            "a failed write must not mark the run finished"
        );

        // A retry now succeeds, and logs the lost session exactly once.
        let cleaned = runtime
            .clean_up_lost_run(Duration::from_secs(5), &stats_log)
            .unwrap();
        assert!(matches!(cleaned, LostRunCleanup::Gone { warning: None }));
        let log = fs::read_to_string(&stats_log).unwrap();
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(log.contains("\"end\":\"lost\""), "{log}");
        assert!(
            log.contains("\"ended_at\":\"2026-09-16T21:03:00Z\""),
            "{log}"
        );
    }
}
