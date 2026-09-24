//! Coordination between wineo commands that can't safely share an environment
//! or a prefix. Kept separate from the run lock, so holding one never makes an
//! environment look like it has a program running.

use std::fs::{self, File, OpenOptions};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::paths::{self, Paths};

/// A probe holds a shared lock for a moment, so taking one retries for about
/// this long before deciding another command really holds it.
const LOCK_ATTEMPTS: u32 = 20;
const LOCK_RETRY: Duration = Duration::from_millis(5);
/// A config write only holds its lock while it reads, checks, and replaces one
/// small file, so another one is waited for, up to about five seconds.
const CONFIG_ATTEMPTS: u32 = 1000;

/// What a command is doing with an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
        }
    }
}

/// The lock files coordinating operations. They live outside the per-environment
/// runtime dirs, so removing an environment never deletes a lock file another
/// command still holds.
#[derive(Debug, Clone)]
pub struct Locks {
    dir: PathBuf,
}

/// Holds an operation's locks until it is dropped. The kernel drops the flocks
/// when the holder dies, so a crash can never leave a stale lock behind.
#[derive(Debug)]
pub struct Guard {
    files: Vec<(File, PathBuf)>,
    exclusive: bool,
}

impl Guard {
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
    }
}

/// Held while one config file is read, changed, and written back, so two
/// commands changing it at once can't overwrite each other's change.
#[derive(Debug)]
pub struct ConfigGuard {
    _file: File,
}

/// One lock to take, and how a message would name what it protects.
struct Target {
    path: PathBuf,
    subject: String,
}

impl Locks {
    pub fn new(paths: &Paths) -> Self {
        Self::at(paths.locks_dir())
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Takes every lock `operation` needs on `name` and each prefix. Targets are
    /// locked in a fixed order, so two commands can never take half of each
    /// other's locks. Nothing is held when this fails.
    pub fn acquire(&self, operation: Operation, name: &str, prefixes: &[&Path]) -> Result<Guard> {
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

        fs::create_dir_all(&self.dir)
            .context(|| format!("failed to create {}", self.dir.display()))?;
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
        if operation != Operation::Snapshot {
            let marker = self.dir.join(crate::snapshot::marker_name(name));
            match marker.symlink_metadata() {
                Ok(_) => {
                    return Err(Error::Busy {
                        subject: format!("environment `{name}`"),
                        holder: "an interrupted snapshot restore needs recovery",
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err).context(|| format!("failed to inspect {}", marker.display()));
                }
            }
        }
        Ok(Guard {
            files,
            exclusive: !operation.is_shared(),
        })
    }

    /// Serializes creating, changing, and removing the config file at `path`.
    /// Independent of the operation locks, so a target setting can still change
    /// while a game runs. Take any operation locks first, then this one, and
    /// read the file again once it is held.
    pub fn config(&self, path: &Path) -> Result<ConfigGuard> {
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
                holder: "another wineo command is changing it",
            });
        }
        Ok(ConfigGuard { _file: file })
    }

    /// The operation holding `name` exclusively, for `status` and `list`. A
    /// momentary check, so the answer can be stale by the time it is used.
    /// Shared holders are left out, since a run already shows up on its own.
    pub fn busy_with(&self, name: &str) -> Result<Option<Operation>> {
        let path = self.name_path(name);
        if !path.exists() {
            return Ok(None);
        }
        self.holder(&path)
            .map(|holder| holder.filter(|operation| !operation.is_shared()))
    }

    fn busy(&self, target: &Target) -> Result<Error> {
        let holder = self.holder(&target.path)?;
        Ok(Error::Busy {
            subject: target.subject.clone(),
            holder: holder.map_or("another wineo command is using it", Operation::describe),
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
        self.keyed_path("prefix", prefix)
    }

    fn config_path(&self, config: &Path) -> PathBuf {
        self.keyed_path("config", config)
    }

    fn keyed_path(&self, kind: &str, path: &Path) -> PathBuf {
        let resolved = paths::resolve(path);
        let label = resolved
            .file_name()
            .and_then(|name| name.to_str())
            .map(label)
            .unwrap_or_default();
        let hash = fnv1a(resolved.as_os_str().as_bytes());
        self.dir.join(format!("{kind}-{label}-{hash:016x}.lock"))
    }
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

/// A single shared attempt, released as soon as `file` closes.
fn probe(file: &File, path: &Path) -> Result<bool> {
    try_flock(file, path, FlockOperation::NonBlockingLockShared)
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
fn label(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect()
}

/// FNV-1a, so the same prefix always maps to the same file, in any build.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn locks(root: &Path) -> Locks {
        Locks::at(root.join("locks"))
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
                matches!(&err, Error::Busy { holder, .. } if *holder == Operation::Winetricks.describe()),
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
            matches!(&err, Error::Busy { holder, .. } if *holder == Operation::Launch.describe()),
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
            matches!(&err, Error::Busy { holder, .. } if *holder == Operation::Install.describe()),
            "{err}"
        );
        drop(held);
        assert_eq!(locks.busy_with("notepad").unwrap(), None);
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

    #[test]
    fn the_same_path_always_hashes_the_same() {
        assert_eq!(fnv1a(b"/home/me/prefix"), fnv1a(b"/home/me/prefix"));
        assert_ne!(fnv1a(b"/home/me/prefix"), fnv1a(b"/home/me/other"));
        assert_eq!(label("Photoshop-2026"), "Photoshop-2026");
        assert_eq!(label("weird name!"), "weirdname");
    }
}
