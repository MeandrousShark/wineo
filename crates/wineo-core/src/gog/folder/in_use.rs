//! A folder's in-use lock. Launches hold it shared for their whole run, and
//! update and migration take it exclusively, never waiting for a game to
//! exit. A launch also leaves a record naming its environment while it
//! holds the lock, so a refused update can say what is running.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use super::holders::{self, Record};
use super::{FolderError, InUseAction, check_folder, key};
use crate::paths::Paths;

/// A folder's in-use lock, described so a supervisor can reopen it before
/// the launch that took it lets go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InUseLock {
    path: PathBuf,
    /// Where the holders' records are kept.
    holders: PathBuf,
    folder: PathBuf,
}

impl InUseLock {
    pub fn new(paths: &Paths, folder: &Path) -> Result<Self> {
        check_folder(folder)?;
        let key = key(folder);
        Ok(Self {
            path: paths.gog_folder_in_use_lock(&key),
            holders: paths.gog_folder_holders(&key),
            folder: folder.to_owned(),
        })
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Takes the lock shared, as a launch of the environment `holder` does.
    /// None means an update or migration holds it.
    pub fn try_shared(&self, holder: &str) -> Result<Option<InUseGuard>> {
        // The record comes first, so an update that loses a race with this
        // launch usually still finds a name. It is only a name: a launch
        // that can't write one still runs, and a refusal then falls back to
        // not naming anyone.
        let record = Record::create(&self.holders, holder).ok();
        self.take(FlockOperation::NonBlockingLockShared, record)
    }

    /// Takes the lock exclusively, as an update or migration does. None
    /// means something is running from the folder.
    pub fn try_exclusive(&self) -> Result<Option<InUseGuard>> {
        self.take(FlockOperation::NonBlockingLockExclusive, None)
    }

    /// The environments running from the folder, sorted and without
    /// repeats, as far as they can be told. Records of holders that are gone
    /// are cleaned up on the way.
    pub fn holders(&self) -> Vec<String> {
        holders::live(&self.holders)
    }

    fn take(
        &self,
        operation: FlockOperation,
        record: Option<Record>,
    ) -> Result<Option<InUseGuard>> {
        let dir = self.path.parent().expect("a lock has a parent");
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
        loop {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .open(&self.path)
                .with_context(|| format!("failed to open {}", self.path.display()))?;
            loop {
                match flock(&file, operation) {
                    Ok(()) => break,
                    Err(Errno::WOULDBLOCK) => return Ok(None),
                    Err(Errno::INTR) => {}
                    Err(error) => {
                        return Err(io::Error::from(error))
                            .with_context(|| format!("failed to lock {}", self.path.display()));
                    }
                }
            }
            // A lock on a file a hand cleanup deleted would be one nobody
            // else sees, so it's taken again on the file that's there now.
            let held = file.metadata()?;
            match fs::metadata(&self.path) {
                Ok(now) if now.dev() == held.dev() && now.ino() == held.ino() => {
                    return Ok(Some(InUseGuard {
                        file,
                        lock: self.clone(),
                        record,
                    }));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            drop(file);
        }
    }
}

/// Holds an in-use lock until dropped.
#[derive(Debug)]
pub struct InUseGuard {
    file: File,
    lock: InUseLock,
    record: Option<Record>,
}

impl InUseGuard {
    pub fn lock(&self) -> &InUseLock {
        &self.lock
    }
}

impl Drop for InUseGuard {
    fn drop(&mut self) {
        // The name goes first, so a refusal never names a holder that has
        // already let go.
        drop(self.record.take());
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

/// Takes a folder's in-use lock exclusively for an update, refusing while
/// anything runs from it. `name` is how the game is named in the refusal.
pub fn lock_for_update(paths: &Paths, folder: &Path, name: &str) -> Result<InUseGuard> {
    lock_for(paths, folder, name, InUseAction::Update)
}

/// `lock_for_update`, for a migration.
pub fn lock_for_migration(paths: &Paths, folder: &Path, name: &str) -> Result<InUseGuard> {
    lock_for(paths, folder, name, InUseAction::Migrate)
}

fn lock_for(paths: &Paths, folder: &Path, name: &str, action: InUseAction) -> Result<InUseGuard> {
    let lock = InUseLock::new(paths, folder)?;
    lock.try_exclusive()?.ok_or_else(|| {
        FolderError::InUse {
            name: name.to_owned(),
            holders: lock.holders(),
            action,
        }
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gog::test_support::setup;

    #[test]
    fn launches_share_and_updates_never_wait() {
        let (root, paths) = setup();
        let folder = root.path().join("g");
        let lock = InUseLock::new(&paths, &folder).unwrap();
        let first = lock.try_shared("rimworld-copy").unwrap().unwrap();
        let second = lock.try_shared("rimworld-copy").unwrap().unwrap();
        let start = std::time::Instant::now();
        let error = lock_for_update(&paths, &folder, "rimworld").unwrap_err();
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert!(matches!(
            error.downcast_ref::<FolderError>(),
            Some(FolderError::InUse { name, .. }) if name == "rimworld"
        ));
        assert_eq!(
            error.to_string(),
            "rimworld-copy is running from this game folder. Stop it before updating rimworld."
        );
        drop(first);
        assert!(lock.try_exclusive().unwrap().is_none());
        drop(second);
        let updating = lock_for_update(&paths, &folder, "rimworld").unwrap();
        assert!(lock.try_shared("rimworld-copy").unwrap().is_none());
        drop(updating);
        assert!(lock.try_shared("rimworld-copy").unwrap().is_some());
    }

    #[test]
    fn the_in_use_lock_is_not_the_state_lock() {
        let (root, paths) = setup();
        let folder = root.path().join("g");
        let _state = super::super::FolderLock::exclusive(&paths, &folder, &|| false).unwrap();
        assert!(lock_for_update(&paths, &folder, "rimworld").is_ok());
    }

    #[test]
    fn a_deleted_lock_file_is_replaced_rather_than_trusted() {
        let (root, paths) = setup();
        let folder = root.path().join("g");
        let lock = InUseLock::new(&paths, &folder).unwrap();
        let _orphaned = lock.try_shared("rimworld-copy").unwrap().unwrap();
        fs::remove_file(&lock.path).unwrap();
        let updating = lock.try_exclusive().unwrap().unwrap();
        assert!(lock.path.is_file());
        assert!(lock.try_shared("rimworld-copy").unwrap().is_none());
        drop(updating);
    }

    #[test]
    fn locks_are_described_for_a_supervisor_and_stay_on_the_host() {
        let (root, paths) = setup();
        let home = root.path().to_owned();
        let portable = Paths::select(
            move |key| (key == "HOME").then(|| home.clone().into_os_string()),
            Some(&root.path().join("usb")),
        )
        .unwrap();
        let folder = root.path().join("g");
        let lock = InUseLock::new(&paths, &folder).unwrap();
        assert_eq!(lock, InUseLock::new(&portable, &folder).unwrap());
        let text = toml::to_string(&lock).unwrap();
        let reopened: InUseLock = toml::from_str(&text).unwrap();
        assert_eq!(reopened, lock);
        let _held = lock.try_shared("rimworld-copy").unwrap().unwrap();
        assert!(reopened.try_exclusive().unwrap().is_none());
        assert!(InUseLock::new(&paths, Path::new("relative")).is_err());
    }

    fn refusal(paths: &Paths, folder: &Path) -> String {
        lock_for_update(paths, folder, "rimworld")
            .unwrap_err()
            .to_string()
    }

    fn records(lock: &InUseLock) -> usize {
        fs::read_dir(&lock.holders).map_or(0, Iterator::count)
    }

    #[test]
    fn the_refusal_names_one_two_or_three_holders() {
        let (root, paths) = setup();
        let folder = root.path().join("g");
        let lock = InUseLock::new(&paths, &folder).unwrap();
        let _first = lock.try_shared("rimworld-copy").unwrap().unwrap();
        assert_eq!(
            refusal(&paths, &folder),
            "rimworld-copy is running from this game folder. Stop it before updating rimworld."
        );
        let _second = lock.try_shared("rimworld-2").unwrap().unwrap();
        assert_eq!(
            refusal(&paths, &folder),
            "rimworld-2 and rimworld-copy are running from this game folder. Stop them before updating rimworld."
        );
        let _third = lock.try_shared("a-third").unwrap().unwrap();
        assert_eq!(
            refusal(&paths, &folder),
            "a-third, rimworld-2, and rimworld-copy are running from this game folder. Stop them before updating rimworld."
        );
    }

    #[test]
    fn a_migration_refusal_says_migrating() {
        let (root, paths) = setup();
        let folder = root.path().join("g");
        let lock = InUseLock::new(&paths, &folder).unwrap();
        let migrate = |paths: &Paths| {
            lock_for_migration(paths, &folder, "rimworld")
                .unwrap_err()
                .to_string()
        };
        let _first = lock.try_shared("rimworld-copy").unwrap().unwrap();
        assert_eq!(
            migrate(&paths),
            "rimworld-copy is running from this game folder. Stop it before migrating rimworld."
        );
        let _second = lock.try_shared("rimworld-2").unwrap().unwrap();
        assert_eq!(
            migrate(&paths),
            "rimworld-2 and rimworld-copy are running from this game folder. Stop them before migrating rimworld."
        );
        for entry in fs::read_dir(&lock.holders).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        assert_eq!(
            migrate(&paths),
            "Something is running from rimworld's game folder. Stop it first."
        );
    }

    #[test]
    fn one_environment_holding_twice_is_named_once() {
        let (root, paths) = setup();
        let folder = root.path().join("g");
        let lock = InUseLock::new(&paths, &folder).unwrap();
        let _first = lock.try_shared("rimworld").unwrap().unwrap();
        let _second = lock.try_shared("rimworld").unwrap().unwrap();
        assert_eq!(records(&lock), 2);
        assert_eq!(lock.holders(), ["rimworld"]);
    }

    #[test]
    fn a_record_lasts_exactly_as_long_as_its_guard() {
        let (root, paths) = setup();
        let folder = root.path().join("g");
        let lock = InUseLock::new(&paths, &folder).unwrap();
        let guard = lock.try_shared("rimworld-copy").unwrap().unwrap();
        assert_eq!(records(&lock), 1);
        drop(guard);
        assert_eq!(records(&lock), 0);
        assert!(lock.holders().is_empty());
    }

    #[test]
    fn a_launch_that_loses_the_race_leaves_no_record() {
        let (root, paths) = setup();
        let folder = root.path().join("g");
        let lock = InUseLock::new(&paths, &folder).unwrap();
        let updating = lock_for_update(&paths, &folder, "rimworld").unwrap();
        assert!(lock.try_shared("rimworld-copy").unwrap().is_none());
        assert_eq!(records(&lock), 0);
        drop(updating);
    }

    #[test]
    fn a_holder_that_cant_be_named_falls_back_to_the_placeholder() {
        let (root, paths) = setup();
        let folder = root.path().join("g");
        let lock = InUseLock::new(&paths, &folder).unwrap();
        let _held = lock.try_shared("rimworld-copy").unwrap().unwrap();
        for entry in fs::read_dir(&lock.holders).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        let message = "Something is running from rimworld's game folder. Stop it first.";
        assert_eq!(refusal(&paths, &folder), message);

        // A record that names nobody doesn't count as a name either.
        fs::create_dir_all(&lock.holders).unwrap();
        let _unnamed = holders::Record::create(&lock.holders, " ").unwrap();
        assert_eq!(refusal(&paths, &folder), message);
    }

    #[test]
    fn a_stale_record_is_ignored_and_cleaned_up() {
        let (root, paths) = setup();
        let folder = root.path().join("g");
        let lock = InUseLock::new(&paths, &folder).unwrap();
        let _held = lock.try_shared("rimworld-copy").unwrap().unwrap();
        // What a killed holder leaves behind: a record nobody has locked.
        fs::write(lock.holders.join("1-2-3.holder"), "ghost").unwrap();
        fs::write(lock.holders.join(".4-5-6.tmp"), "").unwrap();
        assert_eq!(records(&lock), 3);
        assert_eq!(
            refusal(&paths, &folder),
            "rimworld-copy is running from this game folder. Stop it before updating rimworld."
        );
        assert_eq!(records(&lock), 1);
    }
}
