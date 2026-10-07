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
use crate::hooks::{Phase, Running};
use crate::paths::{self, Paths, fnv1a, label};

/// A probe holds a shared lock for a moment, so taking one retries for about
/// this long before deciding another command really holds it.
const LOCK_ATTEMPTS: u32 = 20;
const LOCK_RETRY: Duration = Duration::from_millis(5);
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
            if text
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

    pub(crate) fn record_hook(&self, phase: Phase, running: Option<&Running>) -> Result<()> {
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
    ) -> Result<Option<Running>> {
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
            if exclusive {
                record(&file, &path, operation)?;
            }
            guard.files.push((file, path));
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
        let _admission = loop {
            match self.admissions(name, true, operation, true) {
                Err(Error::Busy {
                    holder: Some(Operation::Launch),
                    ..
                }) => thread::sleep(LOCK_RETRY),
                result => break result?,
            }
        };
        drop(guard);
        loop {
            match self.acquire_inner(operation, name, prefixes) {
                Err(Error::Busy {
                    holder: Some(Operation::Launch),
                    ..
                }) => thread::sleep(LOCK_RETRY),
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
        let mut files = Vec::with_capacity(targets.len());
        for target in &targets {
            let file = open(&target.path)?;
            if !take(&file, &target.path, operation.is_shared(), LOCK_ATTEMPTS)? {
                return Err(self.busy(target)?);
            }
            if !operation.is_shared() {
                record(&file, &target.path, operation)?;
            }
            files.push((file, target.path.clone()));
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
        Ok(Guard {
            files,
            exclusive: !operation.is_shared(),
            operation,
            _reservation: None,
        })
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
        fs::create_dir_all(&self.dir)
            .context(|| format!("failed to create {}", self.dir.display()))?;
        let lock = self.config_path(path);
        let file = open(&lock)?;
        if !take(&file, &lock, false, attempts)? {
            return Err(Error::Busy {
                subject: format!("config {}", path.display()),
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
    pub fn hook_group(&self, name: &str) -> Result<Option<(Phase, Running)>> {
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
            Some(Running {
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

/// A readable hint in the lock's file name. The hash is what makes it unique.
#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn locks(root: &Path) -> Locks {
        Locks::at(root.join("locks"))
    }

    #[test]
    fn operations_serialize_as_json_strings_and_round_trip() {
        for operation in [
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
            Operation::Hook(Phase::Before),
            Operation::Hook(Phase::After),
        ] {
            let json = serde_json::to_string(&operation).unwrap();
            assert_eq!(json, format!("\"{}\"", operation.as_str()));
            assert_eq!(serde_json::from_str::<Operation>(&json).unwrap(), operation);
            let state = crate::runtime::OperationState {
                operation,
                program: "setup.exe".to_owned(),
                runner: crate::runner::Runner::fake("wine"),
                prefix: PathBuf::from("/tmp/prefix"),
                pid: None,
                group: None,
                boot_id: None,
                leader_start: None,
                started_at: jiff::Timestamp::now(),
            };
            let text = toml::to_string(&state).unwrap();
            assert!(text.contains(&format!("operation = \"{}\"", operation.as_str())));
            let restored: crate::runtime::OperationState = toml::from_str(&text).unwrap();
            assert_eq!(restored.operation, operation);
            assert_eq!(restored.program, state.program);
            assert_eq!(restored.prefix, state.prefix);
        }
        assert!(serde_json::from_str::<Operation>("\"unknown\"").is_err());
    }

    #[test]
    fn operation_state_reads_legacy_toml_representations() {
        #[derive(Deserialize)]
        struct Record {
            operation: Operation,
        }
        assert_eq!(
            toml::from_str::<Record>("operation = 'install'")
                .unwrap()
                .operation,
            Operation::Install
        );
        for phase in [Phase::Before, Phase::After] {
            let text = format!("[operation]\nhook = '{}'\n", phase.as_str());
            assert_eq!(
                toml::from_str::<Record>(&text).unwrap().operation,
                Operation::Hook(phase)
            );
        }
    }

    #[test]
    fn hook_locks_are_exclusive_and_busy_messages_name_the_phase() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("prefix");
        for phase in [Phase::Before, Phase::After] {
            let held = locks
                .acquire(Operation::Hook(phase), "hades", &[&prefix])
                .unwrap();
            assert_eq!(
                locks.busy_with("hades").unwrap(),
                Some(Operation::Hook(phase))
            );
            for operation in [
                Operation::Launch,
                Operation::Install,
                Operation::Hook(phase),
            ] {
                let err = locks.acquire(operation, "hades", &[&prefix]).unwrap_err();
                assert_eq!(
                    err.to_string(),
                    format!("hades is busy running its {} hook", phase.as_str())
                );
                assert!(
                    matches!(err, Error::Busy { holder: Some(Operation::Hook(found)), .. } if found == phase)
                );
            }
            let alias = locks
                .acquire(Operation::Launch, "other", &[&prefix])
                .unwrap();
            let other_hook = locks
                .acquire(Operation::Hook(phase), "third", &[&prefix])
                .unwrap();
            drop(other_hook);
            drop(alias);
            drop(held);
            assert_eq!(locks.busy_with("hades").unwrap(), None);
        }
    }

    #[test]
    fn after_handoff_does_not_wait_for_another_environment_sharing_the_prefix() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("prefix");
        let other = locks
            .acquire(Operation::Launch, "other", &[&prefix])
            .unwrap();
        let launch = locks
            .acquire(Operation::Launch, "hades", &[&prefix])
            .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker_locks = locks.clone();
        let worker = thread::spawn(move || {
            let _ = sender.send(worker_locks.after_launch(launch, "hades"));
        });
        let result = receiver.recv_timeout(std::time::Duration::from_secs(5));
        if result.is_err() {
            drop(other);
            worker.join().unwrap();
            panic!("after handoff waited for another environment's prefix lock");
        }
        let after = result.unwrap().unwrap();
        worker.join().unwrap();
        assert_eq!(
            locks.busy_with("hades").unwrap(),
            Some(Operation::Hook(Phase::After))
        );
        assert!(
            locks
                .acquire(Operation::Launch, "hades", &[&prefix])
                .is_err()
        );
        let alias = locks
            .acquire(Operation::Launch, "alias", &[&prefix])
            .unwrap();
        drop(alias);
        drop(after);
        drop(other);
    }

    #[test]
    fn hook_group_records_identity_and_ignores_stale_contents() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let held = locks
            .acquire(Operation::Hook(Phase::Before), "hades", &[])
            .unwrap();
        let running = Running {
            group: 123,
            boot_id: "boot".to_owned(),
            leader_start: 456,
        };
        held.record_hook(Phase::Before, Some(&running)).unwrap();
        assert_eq!(
            locks.hook_group("hades").unwrap(),
            Some((Phase::Before, running))
        );
        drop(held);
        assert!(locks.hook_group("hades").unwrap().is_none());
        fs::write(locks.name_path("hades"), "hook-before 1 123 boot 456\n").unwrap();
        assert!(locks.hook_group("hades").unwrap().is_none());
    }

    #[test]
    fn hook_stop_requests_match_the_phase_and_ignore_an_old_before_request() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let guard = locks
            .acquire(Operation::Hook(Phase::After), "app", &[])
            .unwrap();
        let running = Running {
            group: 123,
            boot_id: "boot".to_owned(),
            leader_start: 456,
        };
        guard.record_hook(Phase::After, Some(&running)).unwrap();
        let path = locks.name_path("app");
        let original = fs::read_to_string(&path).unwrap();
        assert!(
            locks
                .request_hook_stop("app", Phase::Before, false)
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        // Stand in for before finishing between the phase check and the append.
        writeln!(
            OpenOptions::new().append(true).open(&path).unwrap(),
            "cancel-before"
        )
        .unwrap();
        let token = crate::cancel::Token::new();
        guard.hook_request(&token).unwrap();
        assert!(!token.is_canceled());
        assert_eq!(
            locks.request_hook_stop("app", Phase::After, true).unwrap(),
            Some(running)
        );
        guard.hook_request(&token).unwrap();
        assert!(token.is_forced());
    }

    #[test]
    fn hook_operations_round_trip_in_toml() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Record {
            operation: Operation,
        }
        for phase in [Phase::Before, Phase::After] {
            let record = Record {
                operation: Operation::Hook(phase),
            };
            let text = toml::to_string(&record).unwrap();
            assert_eq!(toml::from_str::<Record>(&text).unwrap(), record);
        }
    }

    #[test]
    fn launches_share_an_environment() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("p");
        let first = locks
            .acquire(Operation::Launch, "notepad", &[&prefix])
            .unwrap();
        let second = locks
            .acquire(Operation::Launch, "notepad", &[&prefix])
            .unwrap();
        drop((first, second));
    }

    #[test]
    fn an_exclusive_operation_excludes_everything_else() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("p");

        let held = locks
            .acquire(Operation::Winetricks, "notepad", &[&prefix])
            .unwrap();
        for operation in [
            Operation::Launch,
            Operation::Remove,
            Operation::Winetricks,
            Operation::Configure,
        ] {
            let err = locks.acquire(operation, "notepad", &[&prefix]).unwrap_err();
            assert!(
                matches!(
                    &err,
                    Error::Busy {
                        holder: Some(Operation::Winetricks),
                        ..
                    }
                ),
                "{operation:?}: {err}"
            );
        }
        drop(held);
        drop(
            locks
                .acquire(Operation::Remove, "notepad", &[&prefix])
                .unwrap(),
        );
    }

    #[test]
    fn a_launch_blocks_an_exclusive_operation() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("p");
        let held = locks
            .acquire(Operation::Launch, "notepad", &[&prefix])
            .unwrap();
        let err = locks
            .acquire(Operation::Remove, "notepad", &[&prefix])
            .unwrap_err();
        assert!(
            matches!(
                &err,
                Error::Busy {
                    holder: Some(Operation::Launch),
                    ..
                }
            ),
            "{err}"
        );
        drop(held);
    }

    #[test]
    fn different_environments_do_not_block_each_other() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let held = locks
            .acquire(
                Operation::Winetricks,
                "notepad",
                &[&root.path().join("one")],
            )
            .unwrap();
        drop(
            locks
                .acquire(Operation::Remove, "other", &[&root.path().join("two")])
                .unwrap(),
        );
        drop(held);
    }

    #[test]
    fn two_names_for_one_prefix_still_exclude_each_other() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("prefix");
        fs::create_dir_all(&prefix).unwrap();
        let alias = root.path().join("alias");
        symlink(&prefix, &alias).unwrap();

        let held = locks
            .acquire(Operation::Winetricks, "one", &[&prefix])
            .unwrap();
        let err = locks
            .acquire(Operation::Remove, "two", &[&alias])
            .unwrap_err();
        assert!(
            matches!(&err, Error::Busy { subject, .. } if subject.starts_with("prefix ")),
            "{err}"
        );
        drop(held);
        drop(locks.acquire(Operation::Remove, "two", &[&alias]).unwrap());
    }

    #[test]
    fn a_prefix_that_does_not_exist_yet_still_gets_one_lock() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let missing = root.path().join("later/prefix");
        let alias = root.path().join("later/../later/prefix");
        assert_eq!(locks.prefix_path(&missing), locks.prefix_path(&alias));
    }

    #[test]
    fn busy_with_reports_only_exclusive_holders() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("p");
        assert_eq!(locks.busy_with("notepad").unwrap(), None);

        let launch = locks
            .acquire(Operation::Launch, "notepad", &[&prefix])
            .unwrap();
        assert_eq!(locks.busy_with("notepad").unwrap(), None);
        drop(launch);

        let held = locks
            .acquire(Operation::Winetricks, "notepad", &[&prefix])
            .unwrap();
        assert_eq!(
            locks.busy_with("notepad").unwrap(),
            Some(Operation::Winetricks)
        );
        drop(held);
        assert_eq!(locks.busy_with("notepad").unwrap(), None);
    }

    #[test]
    fn a_failed_acquire_holds_nothing() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("p");
        // Holds the prefix under another name, so the name lock is free and the
        // prefix lock is not. The name lock must not stay held after the failure.
        let held = locks
            .acquire(Operation::Winetricks, "other", &[&prefix])
            .unwrap();
        assert!(
            locks
                .acquire(Operation::Remove, "notepad", &[&prefix])
                .is_err()
        );
        drop(locks.acquire(Operation::Remove, "notepad", &[]).unwrap());
        drop(held);
    }

    #[test]
    fn relabeling_keeps_the_locks_and_changes_what_others_see() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("p");
        let mut held = locks
            .acquire(Operation::Create, "notepad", &[&prefix])
            .unwrap();
        held.relabel(Operation::Install).unwrap();
        assert_eq!(
            locks.busy_with("notepad").unwrap(),
            Some(Operation::Install)
        );
        let err = locks
            .acquire(Operation::Launch, "notepad", &[&prefix])
            .unwrap_err();
        assert!(
            matches!(
                &err,
                Error::Busy {
                    holder: Some(Operation::Install),
                    ..
                }
            ),
            "{err}"
        );
        drop(held);
        assert_eq!(locks.busy_with("notepad").unwrap(), None);
    }

    #[test]
    fn relabel_never_lets_go_of_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("p");
        let mut held = locks
            .acquire(Operation::Create, "notepad", &[&prefix])
            .unwrap();
        assert!(
            locks
                .acquire(Operation::Remove, "notepad", &[&prefix])
                .is_err()
        );
        held.relabel(Operation::Install).unwrap();
        // Still held right after relabeling: it never let go in between.
        assert!(
            locks
                .acquire(Operation::Remove, "notepad", &[&prefix])
                .is_err()
        );
        drop(held);
        assert!(
            locks
                .acquire(Operation::Remove, "notepad", &[&prefix])
                .is_ok()
        );
    }

    #[test]
    fn guard_releases_its_locks_even_if_a_forked_child_still_holds_the_fd() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("p");
        let held = locks
            .acquire(Operation::Winetricks, "notepad", &[&prefix])
            .unwrap();
        // Stand in for a forked child's copies of the fds, kept alive past the drop.
        let clones: Vec<File> = held
            .files
            .iter()
            .map(|(file, _)| file.try_clone().unwrap())
            .collect();
        drop(held);
        assert!(
            locks
                .acquire(Operation::Remove, "notepad", &[&prefix])
                .is_ok()
        );
        drop(clones);
    }

    #[test]
    fn config_guard_releases_its_lock_even_if_a_forked_child_still_holds_the_fd() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let config = root.path().join("environments/notepad.toml");
        let held = locks.config(&config).unwrap();
        // Stands in for a forked child's copy of the fd, kept alive past the drop.
        let clone = held.file.try_clone().unwrap();
        drop(held);
        assert!(locks.config(&config).is_ok());
        drop(clone);
    }

    #[test]
    fn probe_releases_its_lock_even_if_a_forked_child_still_holds_the_fd() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("probe.lock");
        let file = open(&path).unwrap();
        // Stands in for a forked child's copy of the fd, kept alive past the probe.
        let clone = file.try_clone().unwrap();
        assert!(probe(&file, &path).unwrap());
        drop(file);
        let fresh = open(&path).unwrap();
        assert!(take(&fresh, &path, false, 1).unwrap());
        drop(clone);
    }

    #[test]
    fn a_config_lock_excludes_another_writer_of_the_same_file() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let config = root.path().join("environments/notepad.toml");
        let held = locks.config(&config).unwrap();
        let err = locks.config_lock(&config, 2).unwrap_err();
        assert!(
            matches!(&err, Error::Busy { subject, .. } if subject.starts_with("config ")),
            "{err}"
        );
        // Another file has a lock of its own.
        drop(
            locks
                .config(&root.path().join("environments/other.toml"))
                .unwrap(),
        );
        drop(held);
        drop(locks.config(&config).unwrap());
    }

    #[test]
    fn a_config_write_waits_for_the_one_before_it() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let config = root.path().join("config.toml");
        let held = locks.config(&config).unwrap();
        let waiter = {
            let (locks, config) = (locks.clone(), config.clone());
            thread::spawn(move || locks.config(&config).map(drop))
        };
        thread::sleep(Duration::from_millis(50));
        drop(held);
        waiter.join().unwrap().unwrap();
    }

    // `set` has to be able to change a target while a game runs, which holds
    // the environment's operation lock the whole time.
    #[test]
    fn config_locks_are_independent_of_operation_locks() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        let prefix = root.path().join("p");
        let config = root.path().join("environments/notepad.toml");
        let held = locks
            .acquire(Operation::Winetricks, "notepad", &[&prefix])
            .unwrap();
        drop(locks.config(&config).unwrap());
        drop(held);
    }

    #[test]
    fn locks_live_outside_the_runtime_dirs() {
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| std::ffi::OsString::from("/home/me")))
                .unwrap();
        let locks = Locks::new(&paths);
        assert_eq!(locks.dir, Path::new("/home/me/.local/state/wineo/.locks"));
        // `.locks` is not a valid environment name, so it can never be one.
        assert!(!crate::name::is_valid(".locks"));
    }

    #[test]
    fn interrupted_restore_blocks_ordinary_operation_after_locking() {
        let root = tempfile::tempdir().unwrap();
        let locks = locks(root.path());
        fs::create_dir_all(&locks.dir).unwrap();
        fs::write(
            locks.dir.join("snapshot-restore-game-restore.json"),
            "marker",
        )
        .unwrap();
        let prefix = root.path().join("prefix");
        assert!(
            locks
                .acquire(Operation::Launch, "game", &[&prefix])
                .is_err()
        );
        assert!(
            locks
                .acquire(Operation::Remove, "game", &[&prefix])
                .is_err()
        );
        drop(
            locks
                .acquire(Operation::Snapshot, "game", &[&prefix])
                .unwrap(),
        );
    }

    fn rooted(root: &Path, portable: Option<&Path>) -> Paths {
        let home = root.to_owned();
        Paths::select(
            move |key| (key == "HOME").then(|| home.clone().into_os_string()),
            portable,
        )
        .unwrap()
    }

    #[test]
    fn prefix_locks_are_shared_across_roots_and_name_locks_are_not() {
        let root = tempfile::tempdir().unwrap();
        let xdg = Locks::new(&rooted(root.path(), None));
        let portable = Locks::new(&rooted(root.path(), Some(&root.path().join("usb"))));
        let prefix = root.path().join("prefix");
        let other = root.path().join("other");

        let held = xdg
            .acquire(Operation::Winetricks, "game", &[&prefix])
            .unwrap();
        let err = portable
            .acquire(Operation::Remove, "else", &[&prefix])
            .unwrap_err();
        assert!(
            matches!(&err, Error::Busy { subject, .. } if subject.starts_with("prefix ")),
            "{err}"
        );
        // The same name in another root is a different environment.
        drop(
            portable
                .acquire(Operation::Remove, "game", &[&other])
                .unwrap(),
        );
        drop(held);
    }

    #[test]
    fn a_portable_restore_marker_is_found_inside_the_folder() {
        let root = tempfile::tempdir().unwrap();
        let usb = root.path().join("usb");
        let paths = rooted(root.path(), Some(&usb));
        let marker = paths.restore_marker("game");
        assert_eq!(
            marker,
            usb.join("prefixes/.wineo-snapshots/game/restore.json")
        );
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, "marker").unwrap();
        let locks = Locks::new(&paths);
        let prefix = usb.join("prefixes/game");
        assert!(
            locks
                .acquire(Operation::Launch, "game", &[&prefix])
                .is_err()
        );
        drop(
            Locks::new(&rooted(root.path(), None))
                .acquire(Operation::Launch, "game", &[&root.path().join("xdg")])
                .unwrap(),
        );
    }

    #[test]
    fn the_same_path_always_hashes_the_same() {
        assert_eq!(fnv1a(b"/home/me/prefix"), fnv1a(b"/home/me/prefix"));
        assert_ne!(fnv1a(b"/home/me/prefix"), fnv1a(b"/home/me/other"));
        assert_eq!(label("Photoshop-2026"), "Photoshop-2026");
        assert_eq!(label("weird name!"), "weirdname");
    }
}
