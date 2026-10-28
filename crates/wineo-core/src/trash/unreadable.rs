//! Listing around unreadable trash entries, and the guarded delete that
//! repairs one.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use sha2::{Digest, Sha256};

use crate::operation::{Locks, Operation};
use crate::paths::Paths;

use super::identity::{delete_owned, preflight_tree, require_same_mount_as_parent, sync};
use super::{
    Entry, MANIFEST, Unverifiable, entry_dir, info, marker_for, raw_entries, read_manifest,
    resolved,
};

/// A trash entry that can't be read, as a listing shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub id: String,
    pub dir: PathBuf,
    /// The environment it blocks, or None for every environment.
    pub blocks: Option<String>,
    pub reason: String,
    /// Ok when wineo can delete it safely, otherwise why it can't.
    pub deletable: Result<(), String>,
}

impl Unreadable {
    pub fn scope(&self) -> &str {
        self.blocks.as_deref().unwrap_or("every environment")
    }

    fn new(paths: &Paths, found: &Unverifiable) -> Self {
        Self {
            id: found.id.clone(),
            dir: found.dir.clone(),
            blocks: found.blocks.clone(),
            reason: found.reason.clone(),
            deletable: deletable(paths, found),
        }
    }
}

/// Every trash entry: the readable ones, and the ones that aren't.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Survey {
    pub entries: Vec<Entry>,
    pub unreadable: Vec<Unreadable>,
}

/// `list`, keeping going past entries that can't be read.
pub fn survey(paths: &Paths) -> Result<Survey> {
    let mut survey = Survey::default();
    for id in raw_entries(paths)? {
        match read_manifest(paths, &id) {
            Ok(manifest) => survey.entries.push(info(paths, &manifest)?),
            Err(err) => {
                let found = err.downcast::<Unverifiable>()?;
                survey.unreadable.push(Unreadable::new(paths, &found));
            }
        }
    }
    Ok(survey)
}

/// The entry `id` as unreadable, or None when it reads fine.
pub(super) fn unreadable(paths: &Paths, id: &str) -> Result<Option<Unreadable>> {
    match read_manifest(paths, id) {
        Ok(_) => Ok(None),
        Err(err) => Ok(Some(Unreadable::new(paths, &err.downcast()?))),
    }
}

/// An unreadable entry can only be deleted when everything it kept is
/// provably inside its own folder, so deleting the folder loses nothing
/// that lives anywhere else.
fn deletable(paths: &Paths, found: &Unverifiable) -> Result<(), String> {
    let Some(manifest) = &found.manifest else {
        return Err("its manifest can't be parsed".into());
    };
    if found.blocks.is_none() {
        return Err("wineo can't tell which environment it belongs to".into());
    }
    match fs::symlink_metadata(&found.dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        _ => return Err("it isn't a folder".into()),
    }
    for payload in &manifest.payloads {
        let inside = resolved(paths, &payload.retained)
            .is_ok_and(|retained| retained != found.dir && retained.starts_with(&found.dir));
        if !inside {
            return Err(format!("it keeps {} outside its folder", payload.retained));
        }
    }
    match marker_for(paths, &found.id) {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err("it has pending recovery".into()),
        Err(err) => Err(format!("its pending recovery can't be checked: {err:#}")),
    }
}

pub(super) fn fingerprint(dir: &Path) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(fs::read(dir.join(MANIFEST))?)
    ))
}

/// Deletes an unreadable entry reviewed with `fingerprint`, checking again
/// under its locks that it is still the same entry and still safe to delete.
pub(super) fn delete(paths: &Paths, id: &str, fingerprint: &str) -> Result<()> {
    let check = || -> Result<Unreadable> {
        let found = unreadable(paths, id)?
            .ok_or_else(|| anyhow!("trash entry {id} can be read now; review it again"))?;
        if let Err(why) = &found.deletable {
            bail!("trash entry {id} can't be deleted safely: {why}");
        }
        if self::fingerprint(&found.dir)? != fingerprint {
            bail!("entry changed since confirmation; review again");
        }
        Ok(found)
    };
    let found = check()?;
    let name = found
        .blocks
        .as_deref()
        .expect("a deletable entry names its environment");
    let dir = entry_dir(paths, id)?;
    let _operation = Locks::new(paths).acquire(Operation::Trash, name, &[dir.as_path()])?;
    check()?;
    require_same_mount_as_parent(&dir)?;
    preflight_tree(&dir)?;
    // The manifest goes last, so an interruption leaves an entry that still
    // reads as this one and can be deleted again.
    for child in fs::read_dir(&dir)? {
        let child = child?;
        if child.file_name() != MANIFEST {
            delete_owned(&child.path())?;
        }
    }
    fs::remove_file(dir.join(MANIFEST))?;
    sync(&dir)?;
    fs::remove_dir(&dir)?;
    sync(&paths.trash_dir())
}

#[cfg(test)]
mod tests;
