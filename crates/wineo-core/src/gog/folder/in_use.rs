//! A folder's in-use lock. Launches hold it shared for their whole run, and
//! update and migration take it exclusively, never waiting for a game to
//! exit.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use super::{FolderError, check_folder, key};
use crate::paths::Paths;

/// A folder's in-use lock, described so a supervisor can reopen it before
/// the launch that took it lets go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InUseLock {
    path: PathBuf,
    folder: PathBuf,
}

impl InUseLock {
    pub fn new(paths: &Paths, folder: &Path) -> Result<Self> {
        check_folder(folder)?;
        Ok(Self {
            path: paths.gog_folder_in_use_lock(&key(folder)),
            folder: folder.to_owned(),
        })
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Takes the lock shared, as a launch does. None means an update or
    /// migration holds it.
    pub fn try_shared(&self) -> Result<Option<InUseGuard>> {
        self.take(FlockOperation::NonBlockingLockShared)
    }

    /// Takes the lock exclusively, as an update or migration does. None
    /// means something is running from the folder.
    pub fn try_exclusive(&self) -> Result<Option<InUseGuard>> {
        self.take(FlockOperation::NonBlockingLockExclusive)
    }

    fn take(&self, operation: FlockOperation) -> Result<Option<InUseGuard>> {
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
}

impl InUseGuard {
    pub fn lock(&self) -> &InUseLock {
        &self.lock
    }
}

impl Drop for InUseGuard {
    fn drop(&mut self) {
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

/// Takes a folder's in-use lock exclusively for an update or migration,
/// refusing while anything runs from it. `name` is how the game is named in
/// the refusal.
pub fn lock_for_update(paths: &Paths, folder: &Path, name: &str) -> Result<InUseGuard> {
    InUseLock::new(paths, folder)?
        .try_exclusive()?
        .ok_or_else(|| {
            FolderError::InUse {
                name: name.to_owned(),
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
        let first = lock.try_shared().unwrap().unwrap();
        let second = lock.try_shared().unwrap().unwrap();
        let start = std::time::Instant::now();
        let error = lock_for_update(&paths, &folder, "rimworld").unwrap_err();
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert!(matches!(
            error.downcast_ref::<FolderError>(),
            Some(FolderError::InUse { name }) if name == "rimworld"
        ));
        assert_eq!(
            error.to_string(),
            "Something is running from rimworld's game folder. Stop it first."
        );
        drop(first);
        assert!(lock.try_exclusive().unwrap().is_none());
        drop(second);
        let updating = lock_for_update(&paths, &folder, "rimworld").unwrap();
        assert!(lock.try_shared().unwrap().is_none());
        drop(updating);
        assert!(lock.try_shared().unwrap().is_some());
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
        let _orphaned = lock.try_shared().unwrap().unwrap();
        fs::remove_file(&lock.path).unwrap();
        let updating = lock.try_exclusive().unwrap().unwrap();
        assert!(lock.path.is_file());
        assert!(lock.try_shared().unwrap().is_none());
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
        let _held = lock.try_shared().unwrap().unwrap();
        assert!(reopened.try_exclusive().unwrap().is_none());
        assert!(InUseLock::new(&paths, Path::new("relative")).is_err());
    }
}
