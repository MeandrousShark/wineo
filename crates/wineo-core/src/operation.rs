//! Coordination between wineo commands that can't safely share an environment
//! or a prefix. Kept separate from the run lock, so holding one never makes an
//! environment look like it has a program running. Hooked launches reserve the
//! name through preparation and cleanup. Admission gates close while a shared
//! Launch guard hands off to an exclusive Hook guard, letting existing users
//! drain without admitting new ones. Ordinary operations only join existing
//! gates, so environments without hooks keep their original lock files.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::hooks::Phase;
use crate::paths::{self, Paths, fnv1a, label};
use crate::process::Identity;

/// A probe holds a shared lock for a moment, so taking one retries for about
/// this long before deciding another command really holds it.
const LOCK_ATTEMPTS: u32 = 20;
const LOCK_RETRY: Duration = Duration::from_millis(5);
/// Where the after hook's retries settle, since another launch of the same
/// environment can hold its lock for a whole game session.
const DRAIN_RETRY_MAX: Duration = Duration::from_millis(250);
/// A config write only holds its lock while it reads, checks, and replaces one
/// small file, so another one is waited for, up to about five seconds.
const CONFIG_ATTEMPTS: u32 = 1000;

/// What a command is doing with an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Creating or adopting a prefix.
    Create,
    /// Removing an environment, and possibly its prefix.
    Remove,
    /// Installing components with winetricks.
    Winetricks,
    /// Pointing an environment at a different prefix.
    Configure,
    /// Running a program in the prefix.
    Launch,
    /// Running an installer or updater, until everything it started exits.
    Install,
    /// Stopping what an installer left running when it could not finish.
    Cleanup,
    /// Creating, restoring, deleting, or recovering a snapshot.
    Snapshot,
    /// Copying an environment into a portable folder.
    Copy,
    /// Mutating or recovering a durable trash entry.
    Trash,
    /// Attaching a GOG game migrated from Heroic.
    Migrate,
    /// Updating, reinstalling, or finishing an update of a GOG game.
    Update,
    /// Running a launch hook.
    Hook(Phase),
}

impl Serialize for Operation {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Operation {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Representation {
            Word(String),
            LegacyHook { hook: Phase },
        }
        match Representation::deserialize(deserializer)? {
            Representation::Word(word) => Self::parse(&word)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown operation: {word}"))),
            Representation::LegacyHook { hook } => Ok(Self::Hook(hook)),
        }
    }
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Operation::Create => "create",
            Operation::Remove => "remove",
            Operation::Winetricks => "winetricks",
            Operation::Configure => "configure",
            Operation::Launch => "launch",
            Operation::Install => "install",
            Operation::Cleanup => "cleanup",
            Operation::Snapshot => "snapshot",
            Operation::Copy => "copy",
            Operation::Trash => "trash",
            Operation::Migrate => "migrate",
            Operation::Update => "update",
            Operation::Hook(Phase::Before) => "hook-before",
            Operation::Hook(Phase::After) => "hook-after",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        [
            Operation::Create,
            Operation::Remove,
            Operation::Winetricks,
            Operation::Configure,
            Operation::Launch,
            Operation::Install,
            Operation::Cleanup,
            Operation::Snapshot,
            Operation::Copy,
            Operation::Trash,
            Operation::Migrate,
            Operation::Update,
            Operation::Hook(Phase::Before),
            Operation::Hook(Phase::After),
        ]
        .into_iter()
        .find(|operation| operation.as_str() == word)
    }

    /// Launching shares access, so an `exec` can run alongside a started app.
    /// Everything else needs the environment and its prefix to itself.
    fn is_shared(self) -> bool {
        matches!(self, Operation::Launch)
    }

    /// How a message names this operation while it blocks another command.
    pub fn describe(self) -> &'static str {
        match self {
            Operation::Create => "it is being created",
            Operation::Remove => "it is being removed",
            Operation::Winetricks => "winetricks is running",
            Operation::Configure => "its prefix is being changed",
            Operation::Launch => "a program is running in it",
            Operation::Install => "an installer is running in it",
            Operation::Cleanup => "what an installer left running is being stopped",
            Operation::Snapshot => "a snapshot operation is running",
            Operation::Copy => "it is being copied to a portable folder",
            Operation::Trash => "its trash entry is being changed",
            Operation::Migrate => "a GOG game is being migrated to it",
            Operation::Update => "its GOG game is being updated",
            Operation::Hook(Phase::Before) => "running its before hook",
            Operation::Hook(Phase::After) => "running its after hook",
        }
    }
}

/// The lock files coordinating operations. They live outside the per-environment
/// runtime dirs, so removing an environment never deletes a lock file another
/// command still holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Locks {
    dir: PathBuf,
    prefix_dir: PathBuf,
    /// The portable root, which decides where restore markers are.
    portable: Option<PathBuf>,
    paths: Option<Paths>,
}

/// Holds an operation's locks until it is dropped. The kernel drops the flocks
/// when the holder dies, so a crash can never leave a stale lock behind.
#[derive(Debug)]
pub struct Guard {
    files: Vec<(File, PathBuf)>,
    exclusive: bool,
    operation: Operation,
    _reservation: Option<ConfigGuard>,
}

impl Guard {
    pub(crate) fn hook_request(&self, token: &crate::cancel::Token) -> Result<()> {
        for (_, path) in &self.files {
            let text = fs::read_to_string(path)
                .context(|| format!("failed to read {}", path.display()))?;
            let phase = match self.operation {
                Operation::Hook(phase) => phase,
                _ => continue,
            };
            let pinned_force = if phase == Phase::After {
                let mut recorded = text.lines().next().unwrap_or_default().split_whitespace();
                let identity = (|| {
                    if recorded.next()? != Operation::Hook(phase).as_str() {
                        return None;
                    }
                    recorded.next()?;
                    Some(Identity {
                        group: recorded.next()?.parse().ok()?,
                        boot_id: recorded.next()?.to_owned(),
                        leader_start: recorded.next()?.parse().ok()?,
                    })
                })();
                text.lines().skip(1).any(|line| {
                    let mut words = line.split_whitespace();
                    if words.next() != Some("force-after-for") {
                        return false;
                    }
                    let request = (|| {
                        Some(Identity {
                            group: words.next()?.parse().ok()?,
                            boot_id: words.next()?.to_owned(),
                            leader_start: words.next()?.parse().ok()?,
                        })
                    })();
                    words.next().is_none() && request.is_some() && request == identity
                })
            } else {
                false
            };
            if pinned_force
                || text
                    .split_whitespace()
                    .any(|word| word == format!("force-{}", phase.as_str()))
            {
                token.force();
            } else if text
                .split_whitespace()
                .any(|word| word == format!("cancel-{}", phase.as_str()))
            {
                token.cancel();
            }
        }
        Ok(())
    }
    /// Hands exclusive locks on to another exclusive operation without letting
    /// go of them, so nothing can slip in between. `new` does this to run an
    /// installer in the prefix it has just made.
    pub fn relabel(&mut self, operation: Operation) -> Result<()> {
        assert!(
            self.exclusive && !operation.is_shared(),
            "only an exclusive operation can take over exclusive locks"
        );
        for (file, path) in &self.files {
            record(file, path, operation)?;
        }
        self.operation = operation;
        Ok(())
    }

    pub(crate) fn record_hook(&self, phase: Phase, running: Option<&Identity>) -> Result<()> {
        if self.operation != Operation::Hook(phase) {
            return Err(Error::Unsettled(format!(
                "{} hook requires its matching operation guard",
                phase.as_str()
            )));
        }
        for (file, path) in &self.files {
            let mut line = format!("{} {}", self.operation.as_str(), std::process::id());
            if let Some(running) = running {
                line.push_str(&format!(
                    " {} {} {}",
                    running.group, running.boot_id, running.leader_start
                ));
            }
            line.push('\n');
            file.set_len(0)
                .and_then(|()| file.write_all_at(line.as_bytes(), 0))
                .context(|| format!("failed to write {}", path.display()))?;
        }
        Ok(())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Best effort, and only for locks nobody else can be writing. A cleared
        // file keeps a later probe from reporting an operation that is over.
        if self.exclusive {
            for (file, _) in &self.files {
                let _ = file.set_len(0);
            }
        }
        // A forked child can hold a copy of the fd, which keeps a flock held on
        // the open file description until it exits or execs. Unlocking
        // explicitly frees it the moment this holder is done, whatever copies
        // still exist.
        for (file, _) in &self.files {
            let _ = flock(file, FlockOperation::Unlock);
        }
    }
}

/// Held while one config file is read, changed, and written back, so two
/// commands changing it at once can't overwrite each other's change.
#[derive(Debug)]
pub struct ConfigGuard {
    file: File,
}

impl Drop for ConfigGuard {
    fn drop(&mut self) {
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

/// Keeps exclusive operations out while the launcher prepares and cleans up.
#[derive(Debug)]
pub struct LaunchReservation {
    _guard: ConfigGuard,
    locks: Locks,
    name: String,
}

impl LaunchReservation {
    pub(crate) fn acquire(&self, operation: Operation, prefixes: &[&Path]) -> Result<Guard> {
        let _admission = self.locks.admissions(&self.name, false, operation, true)?;
        self.locks.acquire_inner(operation, &self.name, prefixes)
    }
}

/// One lock to take, and how a message would name what it protects.
struct Target {
    path: PathBuf,
    subject: String,
}

impl Locks {
    /// Prevents another launch or exclusive operation from entering preparation.
    pub fn reserve_launch(&self, name: &str) -> Result<LaunchReservation> {
        fs::create_dir_all(&self.dir)
            .context(|| format!("failed to create {}", self.dir.display()))?;
        let path = self.dir.join(format!("launch-{name}.lock"));
        let file = open(&path)?;
        if !take(&file, &path, false, LOCK_ATTEMPTS)? {
            if let Some(holder) = self.busy_with(name)? {
                return Err(Error::Busy {
                    subject: format!("environment `{name}`"),
                    holder: Some(holder),
                    holder_label: holder.describe(),
                });
            }
            return Err(Error::AlreadyRunning(name.to_owned()));
        }
        record(&file, &path, Operation::Launch)?;
        Ok(LaunchReservation {
            _guard: ConfigGuard { file },
            locks: self.clone(),
            name: name.to_owned(),
        })
    }

    /// The hook owner observes requests even if the shell traps TERM successfully.
    pub fn request_hook_stop(
        &self,
        name: &str,
        expected: Phase,
        force: bool,
    ) -> Result<Option<Identity>> {
        let Some((phase, running)) = self.hook_group(name)? else {
            return Ok(None);
        };
        if phase != expected {
            return Ok(None);
        }
        let path = self.name_path(name);
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .context(|| format!("failed to open {}", path.display()))?;
        // A phase-tagged request cannot cancel after if before ends during this write.
        writeln!(
            file,
            "{}-{}",
            if force { "force" } else { "cancel" },
            expected.as_str()
        )
        .context(|| format!("failed to write {}", path.display()))?;
        Ok(Some(running))
    }

    /// A stale confirmation cannot force a later after hook using the same lock.
    pub fn request_hook_force_for(&self, name: &str, expected: &Identity) -> Result<bool> {
        if self.hook_group(name)? != Some((Phase::After, expected.clone())) {
            return Ok(false);
        }
        let path = self.name_path(name);
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .context(|| format!("failed to open {}", path.display()))?;
        let request = format!(
            "force-after-for {} {} {}\n",
            expected.group, expected.boot_id, expected.leader_start
        );
        file.write_all(request.as_bytes())
            .context(|| format!("failed to write {}", path.display()))?;
        Ok(true)
    }

    fn admissions(
        &self,
        name: &str,
        exclusive: bool,
        operation: Operation,
        create: bool,
    ) -> Result<Guard> {
        let targets = [self.dir.join(format!("admit-{name}.lock"))];
        let mut guard = Guard {
            files: Vec::new(),
            exclusive,
            operation,
            _reservation: None,
        };
        for path in targets {
            if !create && !path.exists() {
                continue;
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .context(|| format!("failed to create {}", parent.display()))?;
            }
            let file = open(&path)?;
            // take uses shared access, so an exclusive gate passes false.
            if !take(&file, &path, !exclusive, LOCK_ATTEMPTS)? {
                return Err(self.busy(&Target {
                    path,
                    subject: format!("environment `{name}`"),
                })?);
            }
            // In the guard before recording, so a failed record still unlocks it.
            guard.files.push((file, path));
            if exclusive {
                let (file, path) = guard.files.last().expect("just pushed");
                record(file, path, operation)?;
            }
        }
        Ok(guard)
    }

    /// Blocks new operations while existing shared users drain. The caller
    /// retains the run lock until this returns its exclusive Hook guard.
    pub fn after_launch(&self, guard: Guard, name: &str) -> Result<Guard> {
        self.after_hook(Some(guard), name)
    }

    pub(crate) fn after_hook(&self, guard: Option<Guard>, name: &str) -> Result<Guard> {
        let operation = Operation::Hook(Phase::After);
        let prefixes = &[];
        let mut retry = LOCK_RETRY;
        let _admission = loop {
            match self.admissions(name, true, operation, true) {
                Err(Error::Busy {
                    holder: Some(Operation::Launch),
                    ..
                }) => {
                    thread::sleep(retry);
                    retry = drain_backoff(retry);
                }
                result => break result?,
            }
        };
        drop(guard);
        let mut retry = LOCK_RETRY;
        loop {
            match self.acquire_inner(operation, name, prefixes) {
                Err(Error::Busy {
                    holder: Some(Operation::Launch),
                    ..
                }) => {
                    thread::sleep(retry);
                    retry = drain_backoff(retry);
                }
                result => return result,
            }
        }
    }
    pub fn new(paths: &Paths) -> Self {
        Self {
            dir: paths.locks_dir(),
            prefix_dir: paths.prefix_locks_dir(),
            portable: paths.portable_root().map(Path::to_owned),
            paths: Some(paths.clone()),
        }
    }

    /// Locks in one directory, with no `Paths` behind them, for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn at(dir: PathBuf) -> Self {
        Self {
            prefix_dir: dir.clone(),
            dir,
            portable: None,
            paths: None,
        }
    }

    /// Takes every lock `operation` needs on `name` and each prefix. Targets are
    /// locked in a fixed order, so two commands can never take half of each
    /// other's locks. Nothing is held when this fails.
    pub fn acquire(&self, operation: Operation, name: &str, prefixes: &[&Path]) -> Result<Guard> {
        let prefixes = if matches!(operation, Operation::Hook(_)) {
            &[]
        } else {
            prefixes
        };
        let path = self.dir.join(format!("launch-{name}.lock"));
        // Hooked launches create reservations. Joining only existing ones
        // keeps hook-less operations from creating new coordination files.
        let reservation = if !operation.is_shared() && path.exists() {
            let file = open(&path)?;
            // take's flag means shared, the inverse of exclusive admission.
            if !take(&file, &path, true, LOCK_ATTEMPTS)? {
                let holder = self.busy_with(name)?.unwrap_or(Operation::Launch);
                return Err(Error::Busy {
                    subject: format!("environment `{name}`"),
                    holder: Some(holder),
                    holder_label: holder.describe(),
                });
            }
            Some(ConfigGuard { file })
        } else {
            None
        };
        // Only hook owners create admission gates. Ordinary operations join
        // existing gates so they cannot enter during the after handoff.
        let _admission = self.admissions(name, false, operation, false)?;
        let mut guard = self.acquire_inner(operation, name, prefixes)?;
        guard._reservation = reservation;
        Ok(guard)
    }

    fn acquire_inner(&self, operation: Operation, name: &str, prefixes: &[&Path]) -> Result<Guard> {
        let mut targets = vec![Target {
            path: self.name_path(name),
            subject: format!("environment `{name}`"),
        }];
        for prefix in prefixes {
            targets.push(Target {
                path: self.prefix_path(prefix),
                subject: format!("prefix {}", prefix.display()),
            });
        }
        targets.sort_by(|left, right| left.path.cmp(&right.path));
        targets.dedup_by(|left, right| left.path == right.path);

        for dir in [&self.dir, &self.prefix_dir] {
            fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
        }
        // Held in a guard from the first lock, so an error below unlocks what
        // was already taken instead of leaving it to a forked child's fd copy.
        let mut guard = Guard {
            files: Vec::with_capacity(targets.len()),
            exclusive: !operation.is_shared(),
            operation,
            _reservation: None,
        };
        for target in &targets {
            let file = open(&target.path)?;
            if !take(&file, &target.path, operation.is_shared(), LOCK_ATTEMPTS)? {
                return Err(self.busy(target)?);
            }
            guard.files.push((file, target.path.clone()));
            if !operation.is_shared() {
                let (file, path) = guard.files.last().expect("just pushed");
                record(file, path, operation)?;
            }
        }
        if operation != Operation::Snapshot && operation != Operation::Trash {
            let marker = paths::restore_marker(&self.dir, self.portable.as_deref(), name);
            match marker.symlink_metadata() {
                Ok(_) => {
                    return Err(Error::Busy {
                        subject: format!("environment `{name}`"),
                        holder: None,
                        holder_label: "an interrupted snapshot restore needs recovery",
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err).context(|| format!("failed to inspect {}", marker.display()));
                }
            }
        }
        if operation != Operation::Trash
            && let Some(paths) = &self.paths
        {
            crate::trash::check_pending(paths, name, prefixes)
                .map_err(|err| Error::Unsettled(err.to_string()))?;
        }
        Ok(guard)
    }

    /// Serializes creating, changing, and removing the config file at `path`.
    /// Independent of the operation locks, so a target setting can still change
    /// while a game runs. Take any operation locks first, then this one, and
    /// read the file again once it is held.
    pub fn config(&self, path: &Path) -> Result<ConfigGuard> {
        let guard = self.config_lock(path, CONFIG_ATTEMPTS)?;
        if let Some(paths) = &self.paths
            && let Some(name) = path.file_stem().and_then(|stem| stem.to_str())
            && path.parent() == Some(paths.environments_dir().as_path())
        {
            crate::trash::check_pending(paths, name, &[])
                .map_err(|err| Error::Unsettled(err.to_string()))?;
        }
        Ok(guard)
    }

    /// Internal trash recovery already holds the name and entry locks and
    /// deliberately works while that name has a pending record.
    pub(crate) fn config_recovery(&self, path: &Path) -> Result<ConfigGuard> {
        self.config_lock(path, CONFIG_ATTEMPTS)
    }

    fn config_lock(&self, path: &Path, attempts: u32) -> Result<ConfigGuard> {
        self.write_lock(
            &self.config_path(path),
            format!("config {}", path.display()),
            attempts,
        )
    }

    /// Serializes registering, installing, and uninstalling one Steam app
    /// across environment names.
    pub(crate) fn steam_app(&self, app_id: &str) -> Result<ConfigGuard> {
        self.write_lock(
            &self.dir.join(format!("steam-{}.lock", label(app_id))),
            format!("Steam app {app_id}"),
            CONFIG_ATTEMPTS,
        )
    }

    fn write_lock(&self, lock: &Path, subject: String, attempts: u32) -> Result<ConfigGuard> {
        fs::create_dir_all(&self.dir)
            .context(|| format!("failed to create {}", self.dir.display()))?;
        let file = open(lock)?;
        if !take(&file, lock, false, attempts)? {
            return Err(Error::Busy {
                subject,
                holder: None,
                holder_label: "another wineo command is changing it",
            });
        }
        Ok(ConfigGuard { file })
    }

    /// The operation holding `name` exclusively, for `status` and `list`. A
    /// momentary check, so the answer can be stale by the time it is used.
    /// Shared holders are left out, since a run already shows up on its own.
    pub fn busy_with(&self, name: &str) -> Result<Option<Operation>> {
        for path in [
            self.name_path(name),
            self.dir.join(format!("admit-{name}.lock")),
        ] {
            if path.exists()
                && let Some(holder) = self
                    .holder(&path)?
                    .filter(|operation| !operation.is_shared())
            {
                return Ok(Some(holder));
            }
        }
        Ok(None)
    }

    /// Only active Hook holders expose a group; stale lock contents are ignored.
    pub fn hook_group(&self, name: &str) -> Result<Option<(Phase, Identity)>> {
        let path = self.name_path(name);
        if !path.exists() {
            return Ok(None);
        }
        let file = open(&path)?;
        if probe(&file, &path)? {
            return Ok(None);
        }
        let text =
            fs::read_to_string(&path).context(|| format!("failed to read {}", path.display()))?;
        let mut words = text.split_whitespace();
        let Some(Operation::Hook(phase)) = words.next().and_then(Operation::parse) else {
            return Ok(None);
        };
        let _holder = words.next();
        let running = (|| {
            Some(Identity {
                group: words.next()?.parse().ok()?,
                boot_id: words.next()?.to_owned(),
                leader_start: words.next()?.parse().ok()?,
            })
        })();
        Ok(running.map(|running| (phase, running)))
    }

    fn busy(&self, target: &Target) -> Result<Error> {
        let holder = self.holder(&target.path)?;
        Ok(Error::Busy {
            subject: target.subject.clone(),
            holder,
            holder_label: holder.map_or("another wineo command is using it", Operation::describe),
        })
    }

    /// Who holds `path`. A shared lock only fails against an exclusive holder,
    /// so anything else means the holders are launches.
    fn holder(&self, path: &Path) -> Result<Option<Operation>> {
        let file = open(path)?;
        if probe(&file, path)? {
            return Ok(Some(Operation::Launch));
        }
        Ok(fs::read_to_string(path)
            .ok()
            .and_then(|text| Operation::parse(text.split_whitespace().next().unwrap_or(""))))
    }

    /// Names are limited to letters, digits, `-`, and `_`, so they are always
    /// usable as a file name and never collide with a prefix lock.
    fn name_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("name-{name}.lock"))
    }

    fn prefix_path(&self, prefix: &Path) -> PathBuf {
        keyed_path(&self.prefix_dir, "prefix", prefix)
    }

    fn config_path(&self, config: &Path) -> PathBuf {
        keyed_path(&self.dir, "config", config)
    }
}

fn keyed_path(dir: &Path, kind: &str, path: &Path) -> PathBuf {
    let resolved = paths::resolve(path);
    let label = resolved
        .file_name()
        .and_then(|name| name.to_str())
        .map(label)
        .unwrap_or_default();
    let hash = fnv1a(resolved.as_os_str().as_bytes());
    dir.join(format!("{kind}-{label}-{hash:016x}.lock"))
}

/// The next after hook wait, doubling up to `DRAIN_RETRY_MAX`.
fn drain_backoff(retry: Duration) -> Duration {
    (retry * 2).min(DRAIN_RETRY_MAX)
}

fn open(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .context(|| format!("failed to open {}", path.display()))
}

fn take(file: &File, path: &Path, shared: bool, attempts: u32) -> Result<bool> {
    let operation = if shared {
        FlockOperation::NonBlockingLockShared
    } else {
        FlockOperation::NonBlockingLockExclusive
    };
    let mut attempt = 1;
    while !try_flock(file, path, operation)? {
        if attempt >= attempts {
            return Ok(false);
        }
        attempt += 1;
        thread::sleep(LOCK_RETRY);
    }
    Ok(true)
}

/// A single shared attempt, released explicitly the instant it succeeds so a
/// forked child holding a copy of the fd can never keep it held.
fn probe(file: &File, path: &Path) -> Result<bool> {
    let acquired = try_flock(file, path, FlockOperation::NonBlockingLockShared)?;
    if acquired {
        let _ = flock(file, FlockOperation::Unlock);
    }
    Ok(acquired)
}

/// Returns false if another open file holds a conflicting lock.
fn try_flock(file: &File, path: &Path, operation: FlockOperation) -> Result<bool> {
    match flock(file, operation) {
        Ok(()) => Ok(true),
        Err(Errno::WOULDBLOCK) => Ok(false),
        Err(err) => {
            Err(std::io::Error::from(err)).context(|| format!("failed to lock {}", path.display()))
        }
    }
}

fn record(file: &File, path: &Path, operation: Operation) -> Result<()> {
    let line = format!("{} {}\n", operation.as_str(), std::process::id());
    file.set_len(0)
        .and_then(|()| file.write_all_at(line.as_bytes(), 0))
        .context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests;
