//! The shared store of GOG's redistributables, `gog/redist/<dependencyId>/
//! <manifest hash>/` in the data directory, so a newer version never
//! replaces files a running setup uses.
//!
//! An operation names every version it uses in its record's `pending`
//! before downloading any, then holds each version's lock shared from
//! before it looks for the version until the operation ends. A missing
//! version is downloaded into a temporary folder, verified, and renamed into
//! place under its dependency's lock, which is held only for that. Cleanup
//! takes a dependency's lock and then each version's exclusively without
//! waiting, rereads every record under them, and removes a version only
//! when none names it.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use wineo_gog::ChunkLinks;

use super::fetch::{self, Job};
use super::record::{self, FileEntry, StoreVersion};
use crate::cancel::Token;
use crate::files;
use crate::gog::account::Session;
use crate::gog::folder::{create_dir_durable, lock_file};
use crate::gog::package;
use crate::paths::Paths;

/// Whether a dependency id can name a folder and a lock file: letters,
/// digits, `_`, `-`, and `.`, not starting with a dot.
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// Where a version's files are.
pub(super) fn version_dir(paths: &Paths, version: &StoreVersion) -> PathBuf {
    paths
        .gog_redist_dir()
        .join(&version.dependency)
        .join(&version.manifest)
}

/// A version an operation uses, its lock held shared until dropped.
pub(super) struct Held {
    file: File,
    pub(super) version: StoreVersion,
    pub(super) dir: PathBuf,
}

impl Drop for Held {
    fn drop(&mut self) {
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

impl Held {
    /// Whether the version is in the store.
    pub(super) fn present(&self) -> bool {
        fs::symlink_metadata(&self.dir).is_ok_and(|meta| meta.is_dir())
    }
}

/// Takes `version`'s lock shared, waiting while cleanup holds it, and keeps
/// it until the returned value is dropped.
pub(super) fn hold(paths: &Paths, version: &StoreVersion, token: &Token) -> Result<Held> {
    check(version)?;
    let file = lock_file(
        &paths.gog_redist_version_lock(&version.dependency, &version.manifest),
        false,
        &|| token.is_canceled(),
    )?;
    Ok(Held {
        file,
        version: version.clone(),
        dir: version_dir(paths, version),
    })
}

fn check(version: &StoreVersion) -> Result<()> {
    if !valid_id(&version.dependency)
        || version.manifest.len() != 32
        || !version
            .manifest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        anyhow::bail!("{} isn't a usable redistributable", version.dependency);
    }
    Ok(())
}

/// Downloads a held version that isn't in the store yet, under its
/// dependency's lock: into a temporary folder beside it, every file checked
/// and synced, then renamed into place. Another operation may have put it
/// there meanwhile, which is as good.
#[allow(clippy::too_many_arguments)]
pub(super) fn publish(
    paths: &Paths,
    session: &mut Session,
    held: &Held,
    files: &[FileEntry],
    links: &ChunkLinks,
    operation: &str,
    token: &Token,
    progress: &mut dyn FnMut(u64),
) -> Result<()> {
    let version = &held.version;
    let _lock = lock_file(
        &paths.gog_redist_dependency_lock(&version.dependency),
        true,
        &|| token.is_canceled(),
    )?;
    if held.present() {
        return Ok(());
    }
    let parent = held
        .dir
        .parent()
        .expect("a version has a dependency folder");
    create_dir_durable(parent)?;
    remove_leftovers(parent)?;
    let temp = parent.join(format!(".wineo-{operation}-{}", version.manifest));
    create_dir_durable(&temp)?;
    let root = package::open_folder(&temp)?;
    let mut jobs = Vec::new();
    let mut folders = vec![temp.clone()];
    for file in files {
        let (parent, name) = file.path.rsplit_once('/').unwrap_or(("", &file.path));
        let dir = temp.join(parent);
        fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let mut up = dir.as_path();
        while up != temp && !folders.iter().any(|known| known == up) {
            folders.push(up.to_owned());
            up = up.parent().expect("inside the temporary folder");
        }
        jobs.push(Job {
            root: &root,
            parent: parent.to_owned(),
            name: name.to_owned(),
            file,
            links,
        });
    }
    fetch::run(session, &jobs, token, progress, &mut |_, _| Ok(()))?;
    // Deepest first, so each folder's entries are durable before its
    // parent's are.
    folders.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in &folders {
        files::sync_dir(dir).with_context(|| format!("failed to sync {}", dir.display()))?;
    }
    fs::rename(&temp, &held.dir)
        .with_context(|| format!("failed to put {} in place", held.dir.display()))?;
    files::sync_dir(parent).with_context(|| format!("failed to sync {}", parent.display()))
}

/// Removes what an interrupted download or removal left in a dependency's
/// folder. Only called holding the dependency's lock.
fn remove_leftovers(dir: &Path) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", dir.display()));
        }
    };
    let mut removed = false;
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(".wineo-") {
            remove_any(&entry.path())?;
            removed = true;
        }
    }
    if removed {
        files::sync_dir(dir).with_context(|| format!("failed to sync {}", dir.display()))?;
    }
    Ok(())
}

fn remove_any(path: &Path) -> Result<()> {
    let result = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
    };
    result.with_context(|| format!("failed to remove {}", path.display()))
}

/// Takes a lock exclusively without waiting. None when anything holds it.
fn try_exclusive(path: &Path) -> Result<Option<OwnedFd>> {
    let dir = path.parent().expect("a lock has a parent");
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    loop {
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => break,
            Err(Errno::WOULDBLOCK) => return Ok(None),
            Err(Errno::INTR) => {}
            Err(error) => {
                return Err(io::Error::from(error))
                    .with_context(|| format!("failed to lock {}", path.display()));
            }
        }
    }
    // A lock file removed by hand would leave this lock on a file nobody
    // else sees, so that counts as held.
    let held = file.metadata()?;
    match fs::metadata(path) {
        Ok(now) if now.dev() == held.dev() && now.ino() == held.ino() => Ok(Some(file.into())),
        _ => Ok(None),
    }
}

/// Removes every version no Galaxy game's record names and no operation
/// holds, and what interrupted downloads left. A version or dependency
/// whose lock is held is skipped. Returns what was removed.
pub(super) fn clean(paths: &Paths) -> Result<Vec<StoreVersion>> {
    let root = paths.gog_redist_dir();
    let dependencies = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", root.display()));
        }
    };
    let mut removed = Vec::new();
    for entry in dependencies {
        let entry = entry?;
        let dependency = entry.file_name().to_string_lossy().into_owned();
        if !valid_id(&dependency) || !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(_dependency_lock) = try_exclusive(&paths.gog_redist_dependency_lock(&dependency))?
        else {
            continue;
        };
        let dir = entry.path();
        remove_leftovers(&dir)?;
        for version in
            fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))?
        {
            let version = version?;
            let hash = version.file_name().to_string_lossy().into_owned();
            let named = StoreVersion {
                dependency: dependency.clone(),
                manifest: hash.clone(),
            };
            if check(&named).is_err() || !version.file_type()?.is_dir() {
                continue;
            }
            let Some(_version_lock) =
                try_exclusive(&paths.gog_redist_version_lock(&dependency, &hash))?
            else {
                continue;
            };
            #[cfg(test)]
            super::tests::store_step("locked");
            // Read under the lock: a record naming it from here on is one
            // whose operation will wait for the lock and find it gone.
            if record::all(&paths.gog_folders_dir())?
                .iter()
                .any(|record| record.names(&named))
            {
                continue;
            }
            // Out of the way in one rename first, so a half-removed folder
            // never looks like a complete version.
            let doomed = dir.join(format!(".wineo-removing-{hash}"));
            fs::rename(version.path(), &doomed)
                .with_context(|| format!("failed to remove {}", version.path().display()))?;
            files::sync_dir(&dir).with_context(|| format!("failed to sync {}", dir.display()))?;
            remove_any(&doomed)?;
            removed.push(named);
        }
    }
    Ok(removed)
}
