//! Publishing a rebuilt native game into its folder.
//!
//! Every write goes through directory fds opened one component at a time
//! without following symlinks, so nothing in the folder can redirect a write
//! or a delete outside it. A file is renamed from its staging folder to a
//! temporary `.wineo-<operation>-<n>` beside its final path, then over that
//! path, so a path only ever holds a whole file. Symlinks are made the same
//! way, replacing the directory entry rather than writing through it.
//!
//! Three passes, each safe to repeat after a crash:
//! 1. Entries wineo recorded (committed winning entries and the journal's
//!    `attempted`) that the new version doesn't ship, or ships as another
//!    type, are deleted when they still hold what was recorded. Anything
//!    else there is kept, or saved aside when a new entry needs the path.
//! 2. Whatever stands where a new entry goes and isn't wineo's is saved
//!    aside as `<name>.wineo-saved`.
//! 3. The new entries are published, parents first.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;

use anyhow::{Context, Result, anyhow, bail};
use md5::{Digest, Md5};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, RenameFlags};
use rustix::io::Errno;

use super::UpdateError;
use crate::cancel;
use crate::gog::folder::{Attempted, EntryType, WinningEntry, failpoint};
use crate::gog::package::{self, lowercase_hex, open_dir_at, remove_at};

const BUFFER: usize = 256 * 1024;

/// Something of yours that stood where a new game entry goes, moved aside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedAside {
    /// Relative to the game folder.
    pub path: String,
    pub saved_as: String,
}

/// What publishing did with entries that weren't simply wineo's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub saved: Vec<SavedAside>,
    /// Old entries the new version doesn't ship, kept because you changed
    /// them.
    pub kept: Vec<String>,
    /// Shipped files you changed, overwritten by the new version.
    pub overwritten: Vec<String>,
}

/// What wineo recorded at a path: its type, and an MD5 for a file or a
/// target for a symlink.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Recorded {
    kind: EntryType,
    content: Option<String>,
}

/// What a path holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Current {
    File,
    Symlink(String),
    Directory,
    /// A FIFO, socket, or device, which is never wineo's.
    Other,
}

/// How far publishing has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishProgress {
    pub done: usize,
    pub total: usize,
}

pub(super) struct Publisher<'a> {
    root: OwnedFd,
    operation: &'a str,
    cancel: &'a cancel::Token,
    next_temp: usize,
    /// Folders, relative to the game folder ("" for itself), whose entries
    /// changed and need syncing.
    changed: BTreeSet<String>,
    /// Files published, which are synced before the commit.
    written: Vec<String>,
    /// The folders of every entry wineo recorded or publishes. An earlier,
    /// interrupted attempt may have changed any of them without syncing,
    /// and its changes can't be told from what is on disk now, so each one
    /// that is still a folder is synced too.
    recorded_folders: BTreeSet<String>,
    report: Report,
}

impl<'a> Publisher<'a> {
    pub(super) fn new(root: OwnedFd, operation: &'a str, cancel: &'a cancel::Token) -> Self {
        Self {
            root,
            operation,
            cancel,
            next_temp: 0,
            changed: BTreeSet::new(),
            written: Vec::new(),
            recorded_folders: BTreeSet::new(),
            report: Report::default(),
        }
    }

    pub(super) fn into_report(self) -> Report {
        self.report
    }

    /// Runs the three passes. `new` is the rebuilt game's winning entries,
    /// sorted by path. A game with no committed winning entries, a migrated
    /// one's first rebuild, has nothing to tell its files from yours, so a
    /// file there of the same type is overwritten rather than saved aside.
    pub(super) fn publish(
        &mut self,
        new: &[WinningEntry],
        committed: &[WinningEntry],
        attempted: &[Attempted],
        earlier: &[Attempted],
        mut progress: impl FnMut(PublishProgress),
    ) -> Result<()> {
        let mut recorded: BTreeMap<&str, Vec<Recorded>> = BTreeMap::new();
        for entry in committed {
            recorded.entry(&entry.path).or_default().push(Recorded {
                kind: entry.kind,
                content: content_of(entry.kind, &entry.md5, &entry.target),
            });
        }
        for entry in attempted {
            recorded.entry(&entry.path).or_default().push(Recorded {
                kind: entry.kind,
                content: content_of(entry.kind, &entry.md5, &entry.target),
            });
        }
        let shipped: BTreeMap<&str, &WinningEntry> = new
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        self.recorded_folders = recorded
            .keys()
            .copied()
            .chain(new.iter().map(|entry| entry.path.as_str()))
            .map(|path| parent_of(path).to_owned())
            .collect();
        let tracked: BTreeMap<&str, &WinningEntry> = committed
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();

        self.remove_old(&recorded, &shipped)?;
        self.clear_new(new, &recorded, &tracked, earlier, committed.is_empty())?;
        let total = new.len();
        for (done, entry) in new.iter().enumerate() {
            self.check_cancel()?;
            self.publish_one(entry)?;
            progress(PublishProgress {
                done: done + 1,
                total,
            });
        }
        Ok(())
    }

    /// Pass 1: recorded entries the new version doesn't ship as the same
    /// type, children before their folders.
    fn remove_old(
        &mut self,
        recorded: &BTreeMap<&str, Vec<Recorded>>,
        shipped: &BTreeMap<&str, &WinningEntry>,
    ) -> Result<()> {
        for (&path, records) in recorded.iter().rev() {
            self.check_cancel()?;
            let new_kind = shipped.get(path).map(|entry| entry.kind);
            let Some(dir) = self.open_parent(path)? else {
                continue;
            };
            let leaf = leaf_of(path);
            let Some(current) = current(&dir, leaf, path)? else {
                continue;
            };
            match (&current, new_kind) {
                (Current::Directory, Some(EntryType::Directory))
                | (Current::File, Some(EntryType::File))
                | (Current::Symlink(_), Some(EntryType::Symlink)) => continue,
                _ => {}
            }
            if current == Current::Directory {
                let was_folder = records.iter().any(|r| r.kind == EntryType::Directory);
                if was_folder && self.remove_folder(&dir, leaf, path)? {
                    continue;
                }
                // Still holding something of yours, or never wineo's folder.
                if new_kind.is_some() {
                    self.save_aside(&dir, leaf, path)?;
                } else if !was_folder {
                    self.report.kept.push(path.to_owned());
                }
                continue;
            }
            if self.ours(records, &current, &dir, leaf, path)? {
                self.delete(&dir, leaf, path)?;
            } else if new_kind.is_some() {
                self.save_aside(&dir, leaf, path)?;
            } else {
                self.report.kept.push(path.to_owned());
            }
        }
        Ok(())
    }

    /// Pass 2: whatever stands where a new entry goes, parents first.
    fn clear_new(
        &mut self,
        new: &[WinningEntry],
        recorded: &BTreeMap<&str, Vec<Recorded>>,
        tracked: &BTreeMap<&str, &WinningEntry>,
        earlier: &[Attempted],
        migrated: bool,
    ) -> Result<()> {
        let none = Vec::new();
        // What an earlier attempt of this operation wrote there, which isn't
        // a change of yours.
        let wrote_earlier = |path: &str, kind: EntryType, content: Option<&str>| {
            earlier.iter().any(|entry| {
                entry.path == path
                    && entry.kind == kind
                    && content_of(entry.kind, &entry.md5, &entry.target).as_deref() == content
            })
        };
        for entry in new {
            self.check_cancel()?;
            let path = entry.path.as_str();
            let records = recorded.get(path).unwrap_or(&none);
            // A parent that isn't there yet is created when publishing.
            let Some(dir) = self.open_parent(path)? else {
                continue;
            };
            let leaf = leaf_of(path);
            let Some(current) = current(&dir, leaf, path)? else {
                continue;
            };
            match (&current, entry.kind) {
                (Current::Directory, EntryType::Directory) => {}
                (Current::File, EntryType::File) => {
                    let hash = self.hash(&dir, leaf, path)?;
                    // A shipped file is overwritten, and listed when it isn't
                    // what was committed, whatever the new one holds.
                    if let Some(old) = tracked.get(path).filter(|old| old.kind == EntryType::File) {
                        if hash.is_some()
                            && hash != old.md5
                            && !wrote_earlier(path, EntryType::File, hash.as_deref())
                        {
                            self.report.overwritten.push(path.to_owned());
                        }
                        continue;
                    }
                    let same = hash.is_some() && hash == entry.md5;
                    let ours = records
                        .iter()
                        .any(|r| r.kind == EntryType::File && hash.is_some() && r.content == hash);
                    if same || ours || migrated {
                        continue;
                    }
                    self.save_aside(&dir, leaf, path)?;
                }
                (Current::Symlink(target), EntryType::Symlink) => {
                    if let Some(old) = tracked
                        .get(path)
                        .filter(|old| old.kind == EntryType::Symlink)
                    {
                        if old.target.as_deref() != Some(target.as_str())
                            && !wrote_earlier(path, EntryType::Symlink, Some(target))
                        {
                            self.report.overwritten.push(path.to_owned());
                        }
                        continue;
                    }
                    let same = entry.target.as_deref() == Some(target.as_str());
                    let ours = records.iter().any(|r| {
                        r.kind == EntryType::Symlink && r.content.as_deref() == Some(target)
                    });
                    if same || ours || migrated {
                        continue;
                    }
                    self.save_aside(&dir, leaf, path)?;
                }
                // Another type. Pass 1 already took away wineo's own entries
                // whose type changes, so this is yours.
                _ => {
                    if current != Current::Directory
                        && self.ours(records, &current, &dir, leaf, path)?
                    {
                        self.delete(&dir, leaf, path)?;
                    } else {
                        self.save_aside(&dir, leaf, path)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Pass 3, for one entry.
    fn publish_one(&mut self, entry: &WinningEntry) -> Result<()> {
        let path = entry.path.as_str();
        let dir = self
            .open_parent(path)?
            .ok_or_else(|| anyhow!("{path}'s folder is missing while publishing"))?;
        let leaf = leaf_of(path);
        match entry.kind {
            EntryType::Directory => match current(&dir, leaf, path)? {
                None => {
                    failpoint("update.publish.before")?;
                    rustix::fs::mkdirat(&dir, leaf, Mode::from_raw_mode(entry.mode))
                        .with_context(|| format!("failed to create {path}"))?;
                    let made = open_dir_at(&dir, leaf)
                        .with_context(|| format!("failed to open {path}"))?;
                    rustix::fs::fchmod(&made, Mode::from_raw_mode(entry.mode))
                        .with_context(|| format!("failed to set {path}'s mode"))?;
                    self.changed.insert(parent_of(path).to_owned());
                    failpoint("update.publish.after")?;
                }
                Some(Current::Directory) => {}
                Some(_) => bail!("{path} changed while publishing"),
            },
            EntryType::File => {
                let staging = self.open_staged_parent(entry.product, path)?;
                let temp = self.temp_name();
                failpoint("update.publish.before")?;
                rustix::fs::renameat(&staging, leaf, &dir, temp.as_str())
                    .with_context(|| format!("failed to move {path} out of staging"))?;
                failpoint("update.publish.temp")?;
                rustix::fs::renameat(&dir, temp.as_str(), &dir, leaf)
                    .with_context(|| format!("failed to publish {path}"))?;
                self.changed.insert(parent_of(path).to_owned());
                self.written.push(path.to_owned());
                failpoint("update.publish.after")?;
            }
            EntryType::Symlink => {
                let target = entry
                    .target
                    .as_deref()
                    .ok_or_else(|| anyhow!("{path} has no target"))?;
                let temp = self.temp_name();
                failpoint("update.publish.before")?;
                rustix::fs::symlinkat(target, &dir, temp.as_str())
                    .with_context(|| format!("failed to create {path}"))?;
                failpoint("update.publish.temp")?;
                rustix::fs::renameat(&dir, temp.as_str(), &dir, leaf)
                    .with_context(|| format!("failed to publish {path}"))?;
                self.changed.insert(parent_of(path).to_owned());
                failpoint("update.publish.after")?;
            }
        }
        Ok(())
    }

    /// Removes the staging folders and any temporary file of this operation
    /// left behind, then syncs every published file and every folder whose
    /// entries changed, deepest first.
    pub(super) fn finish(&mut self, products: &[u64]) -> Result<()> {
        for &product in products {
            let name = package::staging_name(self.operation, product);
            remove_named(&self.root, &name).with_context(|| format!("failed to remove {name}"))?;
            self.changed.insert(String::new());
        }
        for dir in remove_temporaries(&self.root, self.operation)? {
            self.changed.insert(dir);
        }
        for path in std::mem::take(&mut self.written) {
            let dir = self
                .open_parent(&path)?
                .ok_or_else(|| anyhow!("{path}'s folder is missing while syncing"))?;
            let file = rustix::fs::openat(
                &dir,
                leaf_of(&path),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .with_context(|| format!("failed to open {path} to sync it"))?;
            rustix::fs::fsync(&file).with_context(|| format!("failed to sync {path}"))?;
            record_sync(format!("file:{path}"));
        }
        let changed = std::mem::take(&mut self.changed);
        let recorded = std::mem::take(&mut self.recorded_folders);
        let mut dirs: Vec<&String> = changed.union(&recorded).collect();
        dirs.sort_by_key(|dir| std::cmp::Reverse(depth(dir)));
        for dir in dirs {
            // A recorded folder that is gone, or isn't a folder any more,
            // was changed in its parent, which is synced too.
            let fd = if changed.contains(dir) {
                self.open_folder(dir)?
            } else {
                match self.open_folder_if_there(dir)? {
                    Some(fd) => fd,
                    None => continue,
                }
            };
            rustix::fs::fsync(&fd).with_context(|| format!("failed to sync {dir:?}"))?;
            record_sync(format!("dir:{dir}"));
        }
        Ok(())
    }

    fn check_cancel(&self) -> Result<()> {
        if self.cancel.is_canceled() {
            return Err(UpdateError::Canceled { journal: true }.into());
        }
        Ok(())
    }

    fn temp_name(&mut self) -> String {
        self.next_temp += 1;
        format!(".wineo-{}-{}", self.operation, self.next_temp)
    }

    /// The folder holding `path`, opened without following symlinks. None
    /// when a folder on the way is missing or isn't a real folder, since
    /// then nothing there can be wineo's.
    fn open_parent(&self, path: &str) -> Result<Option<OwnedFd>> {
        let parent = parent_of(path);
        let mut dir = self.root.try_clone().context("failed to open the game")?;
        if parent.is_empty() {
            return Ok(Some(dir));
        }
        for part in parent.split('/') {
            dir = match open_dir_at(&dir, part) {
                Ok(next) => next,
                Err(Errno::NOENT | Errno::NOTDIR | Errno::LOOP) => return Ok(None),
                Err(error) => {
                    return Err(error).with_context(|| format!("failed to open {parent}"));
                }
            };
        }
        Ok(Some(dir))
    }

    /// A folder in the game, which has to be a real one.
    fn open_folder(&self, path: &str) -> Result<OwnedFd> {
        if path.is_empty() {
            return self.root.try_clone().context("failed to open the game");
        }
        let parent = self
            .open_parent(path)?
            .ok_or_else(|| anyhow!("{path} is missing"))?;
        open_dir_at(&parent, leaf_of(path)).with_context(|| format!("failed to open {path}"))
    }

    /// `open_folder`, or None when the path isn't a real folder.
    fn open_folder_if_there(&self, path: &str) -> Result<Option<OwnedFd>> {
        let Some(parent) = self.open_parent(path)? else {
            return Ok(None);
        };
        match open_dir_at(&parent, leaf_of(path)) {
            Ok(fd) => Ok(Some(fd)),
            Err(Errno::NOENT | Errno::NOTDIR | Errno::LOOP) => Ok(None),
            Err(error) => Err(error).with_context(|| format!("failed to open {path}")),
        }
    }

    fn open_staged_parent(&self, product: u64, path: &str) -> Result<OwnedFd> {
        let name = package::staging_name(self.operation, product);
        let staging = open_dir_at(&self.root, name.as_str())
            .with_context(|| format!("failed to open {name}"))?;
        Ok(package::open_parent(&staging, path)?.0)
    }

    /// Whether `current` still holds what wineo recorded at the path.
    fn ours(
        &self,
        records: &[Recorded],
        current: &Current,
        dir: &OwnedFd,
        leaf: &str,
        path: &str,
    ) -> Result<bool> {
        Ok(match current {
            Current::Directory => records.iter().any(|r| r.kind == EntryType::Directory),
            Current::Symlink(target) => records
                .iter()
                .any(|r| r.kind == EntryType::Symlink && r.content.as_deref() == Some(target)),
            Current::File => {
                if !records.iter().any(|r| r.kind == EntryType::File) {
                    return Ok(false);
                }
                let hash = self.hash(dir, leaf, path)?;
                hash.is_some()
                    && records
                        .iter()
                        .any(|r| r.kind == EntryType::File && r.content == hash)
            }
            Current::Other => false,
        })
    }

    /// The MD5 of a regular file, or None when it isn't one any more.
    fn hash(&self, dir: &OwnedFd, leaf: &str, path: &str) -> Result<Option<String>> {
        let fd = match rustix::fs::openat(
            dir,
            leaf,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::NOENT | Errno::LOOP) => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("failed to open {path}")),
        };
        let stat = rustix::fs::fstat(&fd).with_context(|| format!("failed to check {path}"))?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Ok(None);
        }
        let mut file = File::from(fd);
        let mut hasher = Md5::new();
        let mut buffer = vec![0; BUFFER];
        loop {
            self.check_cancel()?;
            let read = match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).with_context(|| format!("failed to read {path}")),
            };
            hasher.update(&buffer[..read]);
        }
        Ok(Some(lowercase_hex(hasher)))
    }

    fn delete(&mut self, dir: &OwnedFd, leaf: &str, path: &str) -> Result<()> {
        failpoint("update.delete.before")?;
        match rustix::fs::unlinkat(dir, leaf, AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(error) => return Err(error).with_context(|| format!("failed to remove {path}")),
        }
        self.changed.insert(parent_of(path).to_owned());
        failpoint("update.delete.after")
    }

    /// Removes an empty folder. False when something is still in it.
    fn remove_folder(&mut self, dir: &OwnedFd, leaf: &str, path: &str) -> Result<bool> {
        failpoint("update.delete.before")?;
        match rustix::fs::unlinkat(dir, leaf, AtFlags::REMOVEDIR) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(Errno::NOTEMPTY | Errno::EXIST) => return Ok(false),
            Err(error) => return Err(error).with_context(|| format!("failed to remove {path}")),
        }
        // Nothing is left of it to sync; its parent holds the change now.
        let inside = format!("{path}/");
        self.changed
            .retain(|dir| dir != path && !dir.starts_with(&inside));
        self.changed.insert(parent_of(path).to_owned());
        failpoint("update.delete.after")?;
        Ok(true)
    }

    /// Moves what stands at `path` to `<name>.wineo-saved`, or with a number
    /// when that's taken, never replacing anything.
    fn save_aside(&mut self, dir: &OwnedFd, leaf: &str, path: &str) -> Result<()> {
        failpoint("update.save.before")?;
        let mut number = 1;
        let saved = loop {
            let saved = if number == 1 {
                format!("{leaf}.wineo-saved")
            } else {
                format!("{leaf}.wineo-saved-{number}")
            };
            number += 1;
            match rustix::fs::renameat_with(dir, leaf, dir, saved.as_str(), RenameFlags::NOREPLACE)
            {
                Ok(()) => break saved,
                Err(Errno::EXIST) => {}
                // A filesystem that can't refuse to replace: look first.
                Err(Errno::INVAL) => {
                    match rustix::fs::statat(dir, saved.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
                        Ok(_) => {}
                        Err(Errno::NOENT) => {
                            rustix::fs::renameat(dir, leaf, dir, saved.as_str())
                                .with_context(|| format!("failed to move {path} aside"))?;
                            break saved;
                        }
                        Err(error) => {
                            return Err(error)
                                .with_context(|| format!("failed to move {path} aside"));
                        }
                    }
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("failed to move {path} aside"));
                }
            }
        };
        let parent = parent_of(path);
        let saved_as = if parent.is_empty() {
            saved
        } else {
            format!("{parent}/{saved}")
        };
        // Folders inside what moved still need their changes synced, under
        // their new name.
        let inside = format!("{path}/");
        self.changed = std::mem::take(&mut self.changed)
            .into_iter()
            .map(|dir| {
                if dir == path {
                    saved_as.clone()
                } else if let Some(rest) = dir.strip_prefix(&inside) {
                    format!("{saved_as}/{rest}")
                } else {
                    dir
                }
            })
            .collect();
        self.changed.insert(parent.to_owned());
        self.report.saved.push(SavedAside {
            path: path.to_owned(),
            saved_as,
        });
        failpoint("update.save.after")
    }
}

fn content_of(kind: EntryType, md5: &Option<String>, target: &Option<String>) -> Option<String> {
    match kind {
        EntryType::File => md5.clone(),
        EntryType::Symlink => target.clone(),
        EntryType::Directory => None,
    }
}

fn current(dir: &OwnedFd, leaf: &str, path: &str) -> Result<Option<Current>> {
    let stat = match rustix::fs::statat(dir, leaf, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::NOENT) => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("failed to inspect {path}")),
    };
    Ok(Some(match FileType::from_raw_mode(stat.st_mode) {
        FileType::RegularFile => Current::File,
        FileType::Directory => Current::Directory,
        FileType::Symlink => {
            let target = rustix::fs::readlinkat(dir, leaf, Vec::new())
                .with_context(|| format!("failed to read {path}"))?;
            Current::Symlink(target.to_string_lossy().into_owned())
        }
        _ => Current::Other,
    }))
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn leaf_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, leaf)| leaf)
}

fn depth(dir: &str) -> usize {
    if dir.is_empty() {
        0
    } else {
        dir.matches('/').count() + 1
    }
}

/// Whether `name` is one of `operation`'s temporary files or staging
/// folders: `.wineo-<operation>-<n>` or `.wineo-<operation>-stage-<n>`.
fn is_temporary(name: &[u8], operation: &str) -> bool {
    let prefix = format!(".wineo-{operation}-");
    let Some(rest) = name.strip_prefix(prefix.as_bytes()) else {
        return false;
    };
    let rest = rest.strip_prefix(b"stage-").unwrap_or(rest);
    !rest.is_empty() && rest.iter().all(u8::is_ascii_digit)
}

fn remove_named(dir: &OwnedFd, name: &str) -> Result<()> {
    let name = std::ffi::CString::new(name).context("a name has a NUL")?;
    remove_at(dir, &name)
}

/// Deletes every temporary file and staging folder `operation` left anywhere
/// in the game, without following symlinks. Returns the folders, relative
/// to the game, it removed something from.
pub(super) fn remove_temporaries(root: &OwnedFd, operation: &str) -> Result<BTreeSet<String>> {
    let mut changed = BTreeSet::new();
    let mut pending = vec![(String::new(), root.try_clone()?)];
    while let Some((path, dir)) = pending.pop() {
        let mut names = Vec::new();
        let mut listing =
            rustix::fs::Dir::read_from(&dir).with_context(|| format!("failed to read {path:?}"))?;
        while let Some(entry) = listing.read() {
            let entry = entry.with_context(|| format!("failed to read {path:?}"))?;
            let name = entry.file_name().to_owned();
            if name.as_bytes() != b"." && name.as_bytes() != b".." {
                names.push(name);
            }
        }
        for name in names {
            let shown = name.to_string_lossy();
            let child = if path.is_empty() {
                shown.into_owned()
            } else {
                format!("{path}/{shown}")
            };
            if is_temporary(name.as_bytes(), operation) {
                remove_at(&dir, &name).with_context(|| format!("failed to remove {child}"))?;
                changed.insert(path.clone());
                continue;
            }
            match open_dir_at(&dir, name.as_c_str()) {
                Ok(fd) => pending.push((child, fd)),
                Err(Errno::NOTDIR | Errno::LOOP | Errno::NOENT) => {}
                Err(error) => return Err(error).with_context(|| format!("failed to open {child}")),
            }
        }
    }
    Ok(changed)
}

/// Syncs the folders `remove_temporaries` changed, deepest first.
pub(super) fn sync_folders(
    root: &OwnedFd,
    dirs: BTreeSet<String>,
    cancel: &cancel::Token,
) -> Result<()> {
    let publisher = Publisher::new(root.try_clone()?, "", cancel);
    let mut dirs: Vec<String> = dirs.into_iter().collect();
    dirs.sort_by_key(|dir| std::cmp::Reverse(depth(dir)));
    for dir in dirs {
        let fd = publisher.open_folder(&dir)?;
        rustix::fs::fsync(&fd).with_context(|| format!("failed to sync {dir:?}"))?;
        record_sync(format!("dir:{dir}"));
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static SYNCS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Notes a sync or commit for tests checking that every sync comes first.
pub(super) fn record_sync(_event: String) {
    #[cfg(test)]
    SYNCS.with(|syncs| syncs.borrow_mut().push(_event));
}

/// Every sync and commit recorded on this thread since the last call.
#[cfg(test)]
pub(super) fn take_syncs() -> Vec<String> {
    SYNCS.with(|syncs| std::mem::take(&mut *syncs.borrow_mut()))
}

/// Refuses packages whose entries can't be layered into one tree: an entry
/// whose folder a later product ships as a file or symlink. `products` are
/// each product's entries, in order.
pub(super) fn check_layers(products: &[(u64, Vec<(String, EntryType)>)]) -> Result<()> {
    let mut layered: BTreeMap<&str, (u64, EntryType)> = BTreeMap::new();
    for (product, entries) in products {
        for (path, kind) in entries {
            layered.insert(path, (*product, *kind));
        }
    }
    for (path, (product, _)) in &layered {
        let mut parent = parent_of(path);
        while !parent.is_empty() {
            match layered.get(parent) {
                Some((_, EntryType::Directory)) | None => {}
                Some((other, _)) => {
                    return Err(UpdateError::Layers {
                        path: (*path).to_owned(),
                        product: *product,
                        parent: parent.to_owned(),
                        other: *other,
                    }
                    .into());
                }
            }
            parent = parent_of(parent);
        }
    }
    Ok(())
}

/// The new winning entries: every product's staged entries in order, a path
/// shipped by several coming from the latest. Sorted by path.
pub(super) fn layer(products: Vec<Vec<WinningEntry>>) -> Vec<WinningEntry> {
    let mut layered: BTreeMap<String, WinningEntry> = BTreeMap::new();
    for entries in products {
        for entry in entries {
            layered.insert(entry.path.clone(), entry);
        }
    }
    layered.into_values().collect()
}
