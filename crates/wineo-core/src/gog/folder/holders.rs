//! Records naming what holds a folder's in-use lock, so an update's refusal
//! can say which environments are running.
//!
//! Each holder owns one file in the folder's holders directory and keeps it
//! `flock`ed for as long as it holds the in-use lock. A reader that can lock
//! a record knows its holder is gone and deletes it, so liveness never
//! depends on a pid.
//!
//! A record is made under a hidden temporary name, locked, filled in, and
//! only then renamed, so a reader never sees a record that is unlocked
//! because it isn't finished yet.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

const RECORD: &str = ".holder";
const TEMPORARY: &str = ".tmp";

/// How many times making a record starts over when a reader deletes its
/// temporary file first.
const ATTEMPTS: usize = 8;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A holder's record, locked and present until dropped.
#[derive(Debug)]
pub(super) struct Record {
    file: File,
    path: PathBuf,
}

impl Record {
    /// Writes a record naming `holder` in `dir`, locked.
    pub(super) fn create(dir: &Path, holder: &str) -> Result<Self> {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
        for _ in 0..ATTEMPTS {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let unique = format!(
                "{}-{nanos}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let temporary = dir.join(format!(".{unique}{TEMPORARY}"));
            let path = dir.join(format!("{unique}{RECORD}"));
            let mut file = match OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)
            {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to create {}", temporary.display()));
                }
            };
            let locked = loop {
                match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                    Ok(()) => break true,
                    Err(Errno::INTR) => {}
                    // A reader looked at the file in the instant it existed
                    // unlocked and is deleting it as stale, so this record
                    // starts over under another name.
                    Err(Errno::WOULDBLOCK) => break false,
                    Err(error) => {
                        let _ = fs::remove_file(&temporary);
                        return Err(io::Error::from(error))
                            .with_context(|| format!("failed to lock {}", temporary.display()));
                    }
                }
            };
            if !locked {
                let _ = fs::remove_file(&temporary);
                continue;
            }
            match file
                .write_all(holder.as_bytes())
                .and_then(|()| fs::rename(&temporary, &path))
            {
                Ok(()) => return Ok(Self { file, path }),
                // A reader got the file in that window and deleted it.
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    let _ = fs::remove_file(&temporary);
                    return Err(error)
                        .with_context(|| format!("failed to write {}", path.display()));
                }
            }
        }
        bail!(
            "failed to record who holds the game folder in {}",
            dir.display()
        )
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

/// The environments named by the live records in `dir`, sorted and without
/// repeats. A record whose holder is gone is deleted. A live holder that
/// can't be named, with a record that is empty or isn't text, is left out.
pub(super) fn live(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names = BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let named = file_name.ends_with(RECORD);
        if !named && !(file_name.starts_with('.') && file_name.ends_with(TEMPORARY)) {
            continue;
        }
        let Ok(mut file) = File::open(&path) else {
            continue;
        };
        // Shared, so readers never keep each other from seeing a live holder.
        match flock(&file, FlockOperation::NonBlockingLockShared) {
            Ok(()) => {
                let _ = fs::remove_file(&path);
            }
            Err(Errno::WOULDBLOCK) if named => {
                let mut text = String::new();
                if file.read_to_string(&mut text).is_ok() && !text.trim().is_empty() {
                    names.insert(text.trim().to_owned());
                }
            }
            Err(_) => {}
        }
    }
    names.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_whose_lock_is_gone_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let record = Record::create(dir.path(), "rimworld").unwrap();
        assert_eq!(live(dir.path()), ["rimworld"]);
        // What the kernel does when a holder dies.
        flock(&record.file, FlockOperation::Unlock).unwrap();
        assert!(live(dir.path()).is_empty());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_record_is_never_visible_before_it_is_locked_and_named() {
        let dir = tempfile::tempdir().unwrap();
        let record = Record::create(dir.path(), "rimworld").unwrap();
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 1);
        assert!(names[0].ends_with(RECORD) && !names[0].starts_with('.'));
        assert_eq!(fs::read_to_string(&record.path).unwrap(), "rimworld");
    }

    #[test]
    fn a_child_process_does_not_inherit_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let record = Record::create(dir.path(), "rimworld").unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let inherited = fs::read_dir(format!("/proc/{}/fd", child.id()))
            .unwrap()
            .filter_map(|entry| fs::read_link(entry.ok()?.path()).ok())
            .any(|target| target == record.path);
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!inherited);
    }

    #[test]
    fn a_missing_directory_has_no_holders() {
        let dir = tempfile::tempdir().unwrap();
        assert!(live(&dir.path().join("none")).is_empty());
    }
}
