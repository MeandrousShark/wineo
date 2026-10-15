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
//!
//! Outside the paths overwritten on purpose (shipped files and a migrated
//! game's), nothing is replaced or deleted by its name: a new entry is
//! renamed in refusing to replace, and whatever stands in its way is first
//! renamed to wineo's own aside name, then checked, so the check always
//! applies to the entry deleted.

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
use crate::gog::package::{self, open_dir_at, remove_at};

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

/// What wineo recorded at one path, as an entry moved aside from it is
/// checked against: file MD5s and symlink targets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Known {
    pub(super) files: Vec<String>,
    pub(super) links: Vec<String>,
}

/// What a path holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Current {
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
    /// Whether moving something aside has to refuse a filesystem that can't
    /// rename without replacing, rather than look first and rename.
    no_replace_only: bool,
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
            no_replace_only: false,
        }
    }

    /// Never falls back to looking before a rename that could replace what
    /// appeared meanwhile, which Galaxy installs rely on.
    pub(super) fn require_no_replace(mut self) -> Self {
        self.no_replace_only = true;
        self
    }

    pub(super) fn into_report(self) -> Report {
        self.report
    }

    /// Runs the three passes. `new` is the rebuilt game's winning entries,
    /// sorted by path, and `earlier` what earlier attempts of the operation
    /// wrote. A game with no committed winning entries, a migrated one's
    /// first rebuild, has nothing to tell its files from yours, so a file
    /// there of the same type is overwritten rather than saved aside, and
    /// listed when it differed from the new one.
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
        let migrated = committed.is_empty();
        self.clear_new(new, &recorded, &tracked, earlier, migrated)?;
        let total = new.len();
        for (done, entry) in new.iter().enumerate() {
            self.check_cancel()?;
            let path = entry.path.as_str();
            // A shipped entry, and a migrated game's, is overwritten
            // whatever it holds; anywhere else, what appeared or changed
            // since pass 2 is saved rather than replaced.
            let overwrite = migrated || tracked.get(path).is_some_and(|old| old.kind == entry.kind);
            if overwrite {
                self.publish_entry(entry, None)?;
            } else {
                let mut known = known_of(recorded.get(path).map_or(&[][..], Vec::as_slice));
                match entry.kind {
                    EntryType::File => known.files.extend(entry.md5.clone()),
                    EntryType::Symlink => known.links.extend(entry.target.clone()),
                    EntryType::Directory => {}
                }
                self.publish_entry(entry, Some(&known))?;
            }
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
            let known = known_of(records);
            if new_kind.is_some() {
                // Settling decides, so a check first would only hash twice.
                failpoint("update.delete.before")?;
                self.move_aside(&dir, path, &known)?;
                failpoint("update.delete.after")?;
            } else if self.ours(records, &current, &dir, leaf, path)? {
                hook("remove", path);
                failpoint("update.delete.before")?;
                if let Some(aside) = self.take_aside(&dir, path)? {
                    // Changed since it was checked: back where it was, as
                    // if it had never matched.
                    if !self.delete_if_known(&dir, &aside, path, &known)?
                        && !self.put_back(&dir, &aside, path)?
                    {
                        self.save_settled(&dir, &aside, path)?;
                    }
                }
                failpoint("update.delete.after")?;
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
                    if same || ours {
                        continue;
                    }
                    // Nothing recorded tells a migrated game's files from
                    // yours, so it's overwritten like a shipped file and
                    // listed since it differed.
                    if migrated {
                        self.report.overwritten.push(path.to_owned());
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
                    if same || ours {
                        continue;
                    }
                    if migrated {
                        self.report.overwritten.push(path.to_owned());
                        continue;
                    }
                    self.save_aside(&dir, leaf, path)?;
                }
                // Another type. Pass 1 already took away wineo's own entries
                // whose type changes, so this is yours.
                _ => {
                    if current == Current::Directory {
                        self.save_aside(&dir, leaf, path)?;
                    } else {
                        self.move_aside(&dir, path, &known_of(records))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Notes the folders of `paths` as ones an earlier, interrupted attempt
    /// may have changed, so `finish` syncs each that is still a folder. For
    /// callers placing entries themselves rather than through `publish`.
    pub(super) fn note_folders<'p>(&mut self, paths: impl IntoIterator<Item = &'p str>) {
        for path in paths {
            self.recorded_folders.insert(parent_of(path).to_owned());
        }
    }

    /// Puts the temporary file `temp`, already written and synced in `dir`,
    /// at `path` without ever replacing what is there: whatever stands there
    /// is first moved to an aside name, then deleted only when it matches
    /// what wineo recorded for the path in `known`, and moved to
    /// `<name>.wineo-saved` otherwise. So something another program put
    /// there, or changed, since the path was last looked at is never lost.
    /// Refused on a filesystem that can't rename without replacing.
    pub(super) fn place_new(
        &mut self,
        dir: &OwnedFd,
        temp: &str,
        path: &str,
        known: &Known,
    ) -> Result<()> {
        self.place(dir, temp, path, known)?;
        self.written.push(path.to_owned());
        Ok(())
    }

    /// Makes a symlink to `target` at `path` the way `place_new` puts a
    /// file there.
    pub(super) fn place_link(
        &mut self,
        dir: &OwnedFd,
        path: &str,
        target: &str,
        known: &Known,
    ) -> Result<()> {
        let temp = self.temp_name();
        rustix::fs::symlinkat(target, dir, temp.as_str())
            .with_context(|| format!("failed to create {path}"))?;
        // A symlink isn't synced itself, only its folder, which `place`
        // marks as changed.
        self.place(dir, &temp, path, known)
    }

    fn place(&mut self, dir: &OwnedFd, temp: &str, path: &str, known: &Known) -> Result<()> {
        let leaf = leaf_of(path);
        // Each attempt either places the entry or moves one out of the way,
        // so only a program racing it on purpose keeps it going.
        for _ in 0..8 {
            match rename_new(dir, temp, dir, leaf) {
                Ok(()) => {
                    self.changed.insert(parent_of(path).to_owned());
                    return Ok(());
                }
                Err(Errno::EXIST) => {}
                // What native publishing did before it could refuse.
                Err(error) if self.looks_first(error) => {
                    rustix::fs::renameat(dir, temp, dir, leaf)
                        .with_context(|| format!("failed to publish {path}"))?;
                    self.changed.insert(parent_of(path).to_owned());
                    return Ok(());
                }
                Err(error) => return Err(no_replace_error(error, path)),
            }
            self.move_aside(dir, path, known)?;
        }
        bail!("{path} kept changing while it was put in place")
    }

    /// Whether a rename that can't refuse to replace, failing with `error`,
    /// is done by looking first instead. Only Galaxy installs refuse.
    fn looks_first(&self, error: Errno) -> bool {
        !self.no_replace_only && matches!(error, Errno::INVAL | Errno::NOSYS | Errno::NOTSUP)
    }

    /// Renames `from` to `to` in `dir`, false when `to` is taken. A
    /// filesystem that can't refuse to replace is looked at first, unless
    /// `require_no_replace` says to refuse it.
    fn rename_free(&self, dir: &OwnedFd, from: &str, to: &str) -> rustix::io::Result<bool> {
        match rename_new(dir, from, dir, to) {
            Ok(()) => Ok(true),
            Err(Errno::EXIST) => Ok(false),
            Err(error) if self.looks_first(error) => {
                match rustix::fs::statat(dir, to, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(_) => Ok(false),
                    Err(Errno::NOENT) => rustix::fs::renameat(dir, from, dir, to).map(|()| true),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Clears `path` without ever deleting by its name: whatever stands
    /// there is first renamed to wineo's own aside name, so another program
    /// can't swap it between the check and the delete, then settled by
    /// `settle_aside`. Nothing there is fine.
    pub(super) fn move_aside(&mut self, dir: &OwnedFd, path: &str, known: &Known) -> Result<()> {
        match self.take_aside(dir, path)? {
            Some(aside) => self.settle_aside(dir, &aside, path, known),
            None => Ok(()),
        }
    }

    /// Renames what stands at `path` to its aside name, which it returns,
    /// or None when nothing is there.
    fn take_aside(&mut self, dir: &OwnedFd, path: &str) -> Result<Option<String>> {
        let aside = aside_name(leaf_of(path), self.operation);
        failpoint("galaxy.aside.before")?;
        match self.rename_free(dir, leaf_of(path), &aside) {
            Ok(true) => {}
            Ok(false) => return Err(no_replace_error(Errno::EXIST, path)),
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(no_replace_error(error, path)),
        }
        self.changed.insert(parent_of(path).to_owned());
        failpoint("galaxy.aside.after")?;
        Ok(Some(aside))
    }

    /// Deals with an entry moved aside from `path` as `aside`: deleted when
    /// it is a file or symlink holding what `known` records, else moved to
    /// `<name>.wineo-saved` and listed.
    pub(super) fn settle_aside(
        &mut self,
        dir: &OwnedFd,
        aside: &str,
        path: &str,
        known: &Known,
    ) -> Result<()> {
        if self.delete_if_known(dir, aside, path, known)? {
            return Ok(());
        }
        self.save_settled(dir, aside, path)
    }

    /// Deletes the entry moved aside from `path` as `aside` when it is a
    /// file or symlink holding what `known` records. True when it's gone.
    fn delete_if_known(
        &mut self,
        dir: &OwnedFd,
        aside: &str,
        path: &str,
        known: &Known,
    ) -> Result<bool> {
        let ours = match current(dir, aside, path)? {
            None => return Ok(true),
            Some(Current::File) => self
                .hash(dir, aside, path)?
                .is_some_and(|hash| known.files.contains(&hash)),
            Some(Current::Symlink(target)) => known.links.contains(&target),
            Some(_) => false,
        };
        if !ours {
            return Ok(false);
        }
        match rustix::fs::unlinkat(dir, aside, AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(error) => {
                return Err(error).with_context(|| format!("failed to remove {path}"));
            }
        }
        self.changed.insert(parent_of(path).to_owned());
        Ok(true)
    }

    /// Renames the entry moved aside from `path` back to it and lists it as
    /// kept. False when something has taken the path meanwhile.
    fn put_back(&mut self, dir: &OwnedFd, aside: &str, path: &str) -> Result<bool> {
        match self.rename_free(dir, aside, leaf_of(path)) {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(error) => return Err(no_replace_error(error, path)),
        }
        self.changed.insert(parent_of(path).to_owned());
        self.report.kept.push(path.to_owned());
        Ok(true)
    }

    /// Moves the entry moved aside from `path` as `aside` to
    /// `<name>.wineo-saved`, or with a number when that's taken, and lists
    /// it.
    fn save_settled(&mut self, dir: &OwnedFd, aside: &str, path: &str) -> Result<()> {
        let leaf = leaf_of(path);
        let mut number = 1;
        let saved = loop {
            let saved = if number == 1 {
                format!("{leaf}.wineo-saved")
            } else {
                format!("{leaf}.wineo-saved-{number}")
            };
            number += 1;
            match self.rename_free(dir, aside, &saved) {
                Ok(true) => break saved,
                Ok(false) => {}
                Err(error) => return Err(no_replace_error(error, path)),
            }
        };
        let parent = parent_of(path);
        self.changed.insert(parent.to_owned());
        self.report.saved.push(SavedAside {
            path: path.to_owned(),
            saved_as: if parent.is_empty() {
                saved
            } else {
                format!("{parent}/{saved}")
            },
        });
        Ok(())
    }

    /// Settles every entry an interrupted attempt of this operation moved
    /// aside, anywhere in the game, without following symlinks, against what
    /// `known` says was recorded at the path it came from.
    pub(super) fn recover_asides(&mut self, known: impl Fn(&str) -> Known) -> Result<()> {
        let mut pending = vec![(String::new(), self.root.try_clone()?)];
        while let Some((path, dir)) = pending.pop() {
            let mut names = Vec::new();
            let mut listing = rustix::fs::Dir::read_from(&dir)
                .with_context(|| format!("failed to read {path:?}"))?;
            while let Some(entry) = listing.read() {
                let entry = entry.with_context(|| format!("failed to read {path:?}"))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name != "." && name != ".." {
                    names.push(name);
                }
            }
            for name in names {
                let child = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}/{name}")
                };
                if let Some(origin) = aside_origin(&name, self.operation) {
                    let origin = if path.is_empty() {
                        origin.to_owned()
                    } else {
                        format!("{path}/{origin}")
                    };
                    self.settle_aside(&dir, &name, &origin, &known(&origin))?;
                    continue;
                }
                match open_dir_at(&dir, name.as_str()) {
                    Ok(fd) => pending.push((child, fd)),
                    Err(Errno::NOTDIR | Errno::LOOP | Errno::NOENT) => {}
                    Err(error) => {
                        return Err(error).with_context(|| format!("failed to open {child}"));
                    }
                }
            }
        }
        Ok(())
    }

    /// Pass 3, for one entry, never replacing anything unchecked. Directories
    /// and symlinks only need the entry itself, so callers placing files
    /// themselves publish those with it.
    pub(super) fn publish_one(&mut self, entry: &WinningEntry) -> Result<()> {
        self.publish_entry(entry, Some(&Known::default()))
    }

    /// Publishes `entry`. A file or symlink replaces whatever stands at its
    /// path when `known` is None, and is put in place as `place_new` does
    /// otherwise.
    fn publish_entry(&mut self, entry: &WinningEntry, known: Option<&Known>) -> Result<()> {
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
                hook("publish", path);
                match known {
                    Some(known) => self.place_new(&dir, &temp, path, known)?,
                    None => {
                        rustix::fs::renameat(&dir, temp.as_str(), &dir, leaf)
                            .with_context(|| format!("failed to publish {path}"))?;
                        self.changed.insert(parent_of(path).to_owned());
                        self.written.push(path.to_owned());
                    }
                }
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
                hook("publish", path);
                match known {
                    Some(known) => self.place(&dir, &temp, path, known)?,
                    None => {
                        rustix::fs::renameat(&dir, temp.as_str(), &dir, leaf)
                            .with_context(|| format!("failed to publish {path}"))?;
                        self.changed.insert(parent_of(path).to_owned());
                    }
                }
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

    pub(super) fn temp_name(&mut self) -> String {
        self.next_temp += 1;
        format!(".wineo-{}-{}", self.operation, self.next_temp)
    }

    /// The folder holding `path`, opened without following symlinks. None
    /// when a folder on the way is missing or isn't a real folder, since
    /// then nothing there can be wineo's.
    pub(super) fn open_parent(&self, path: &str) -> Result<Option<OwnedFd>> {
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
        Ok(Some(crate::units::hex(hasher.finalize())))
    }

    /// Removes an empty folder. False when something is still in it.
    pub(super) fn remove_folder(&mut self, dir: &OwnedFd, leaf: &str, path: &str) -> Result<bool> {
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
    pub(super) fn save_aside(&mut self, dir: &OwnedFd, leaf: &str, path: &str) -> Result<()> {
        failpoint("update.save.before")?;
        // Changes already made inside a folder are made durable before it
        // moves, since a recovery looks for its folders under their old
        // names: every recorded folder inside it, deepest first, then it.
        let moved_folders: Vec<String> = match open_dir_at(dir, leaf) {
            Ok(folder) => {
                let inside = format!("{path}/");
                let mut nested: Vec<String> = self
                    .recorded_folders
                    .iter()
                    .filter(|recorded| recorded.starts_with(&inside))
                    .cloned()
                    .collect();
                nested.sort_by_key(|nested| std::cmp::Reverse(depth(nested)));
                let mut synced = Vec::new();
                for nested in nested {
                    if let Some(fd) = self.open_folder_if_there(&nested)? {
                        rustix::fs::fsync(&fd)
                            .with_context(|| format!("failed to sync {nested}"))?;
                        synced.push(nested);
                    }
                }
                rustix::fs::fsync(&folder).with_context(|| format!("failed to sync {path}"))?;
                synced.push(path.to_owned());
                synced
            }
            Err(Errno::NOTDIR | Errno::LOOP | Errno::NOENT) => Vec::new(),
            Err(error) => return Err(error).with_context(|| format!("failed to open {path}")),
        };
        let mut number = 1;
        let saved = loop {
            let saved = if number == 1 {
                format!("{leaf}.wineo-saved")
            } else {
                format!("{leaf}.wineo-saved-{number}")
            };
            number += 1;
            match self.rename_free(dir, leaf, saved.as_str()) {
                Ok(true) => break saved,
                Ok(false) => {}
                Err(error @ (Errno::INVAL | Errno::NOSYS | Errno::NOTSUP))
                    if self.no_replace_only =>
                {
                    return Err(no_replace_error(error, path));
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
        for moved in moved_folders {
            record_sync(format!("dir:{saved_as}{}", &moved[path.len()..]));
        }
        self.changed.insert(parent.to_owned());
        self.report.saved.push(SavedAside {
            path: path.to_owned(),
            saved_as,
        });
        failpoint("update.save.after")
    }
}

/// What an entry moved out of the way at `leaf` is called while it waits to
/// be deleted or saved. It isn't one of the operation's temporaries, so
/// recovery never deletes it unchecked; `aside_origin` gives the name back.
pub(super) fn aside_name(leaf: &str, operation: &str) -> String {
    format!("{leaf}{}", aside_suffix(operation))
}

fn aside_suffix(operation: &str) -> String {
    format!(".wineo-aside-{operation}")
}

/// The name an aside entry of `operation` was moved from, if `name` is one.
pub(super) fn aside_origin<'a>(name: &'a str, operation: &str) -> Option<&'a str> {
    name.strip_suffix(&aside_suffix(operation))
        .filter(|origin| !origin.is_empty())
}

fn rename_new(
    from_dir: &OwnedFd,
    from: &str,
    to_dir: &OwnedFd,
    to: &str,
) -> rustix::io::Result<()> {
    #[cfg(test)]
    if NO_RENAME_NOREPLACE.with(std::cell::Cell::get) {
        return Err(Errno::INVAL);
    }
    rustix::fs::renameat_with(from_dir, from, to_dir, to, RenameFlags::NOREPLACE)
}

fn no_replace_error(error: Errno, path: &str) -> anyhow::Error {
    if error == Errno::INVAL || error == Errno::NOSYS || error == Errno::NOTSUP {
        anyhow!(
            "{path}'s filesystem can't rename a file without replacing another, which wineo needs to install Galaxy builds safely"
        )
    } else {
        anyhow::Error::from(error).context(format!("failed to put {path} in place"))
    }
}

fn content_of(kind: EntryType, md5: &Option<String>, target: &Option<String>) -> Option<String> {
    match kind {
        EntryType::File => md5.clone(),
        EntryType::Symlink => target.clone(),
        EntryType::Directory => None,
    }
}

/// The file MD5s and symlink targets in `records`.
fn known_of(records: &[Recorded]) -> Known {
    let mut known = Known::default();
    for record in records {
        match (record.kind, &record.content) {
            (EntryType::File, Some(md5)) => known.files.push(md5.clone()),
            (EntryType::Symlink, Some(target)) => known.links.push(target.clone()),
            _ => {}
        }
    }
    known
}

/// What wineo recorded at `path`, committed and attempted, as an entry
/// moved aside from it is checked against.
pub(super) fn recorded_at(
    committed: &[WinningEntry],
    attempted: &[Attempted],
    path: &str,
) -> Known {
    let committed = committed
        .iter()
        .filter(|entry| entry.path == path)
        .map(|entry| (entry.kind, &entry.md5, &entry.target));
    let attempted = attempted
        .iter()
        .filter(|entry| entry.path == path)
        .map(|entry| (entry.kind, &entry.md5, &entry.target));
    let records: Vec<Recorded> = committed
        .chain(attempted)
        .map(|(kind, md5, target)| Recorded {
            kind,
            content: content_of(kind, md5, target),
        })
        .collect();
    known_of(&records)
}

/// Runs a test's hook at `at` for `path`: "remove" right after pass 1
/// checks an old entry, and "publish" right before pass 3 puts one in
/// place.
fn hook(_at: &str, _path: &str) {
    #[cfg(test)]
    if let Some(mut hook) = HOOK.with(|cell| cell.borrow_mut().take()) {
        hook(_at, _path);
        HOOK.with(|cell| {
            cell.borrow_mut().get_or_insert(hook);
        });
    }
}

pub(super) fn current(dir: &OwnedFd, leaf: &str, path: &str) -> Result<Option<Current>> {
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

pub(super) fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

pub(super) fn leaf_of(path: &str) -> &str {
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

/// A test's hook, given where publishing is and the path.
#[cfg(test)]
pub(super) type Hook = Box<dyn FnMut(&str, &str)>;

#[cfg(test)]
thread_local! {
    static HOOK: std::cell::RefCell<Option<Hook>> = const { std::cell::RefCell::new(None) };
}

/// Sets, or with None clears, the hook `hook` runs on this thread.
#[cfg(test)]
pub(super) fn set_hook(hook: Option<Hook>) {
    HOOK.with(|cell| *cell.borrow_mut() = hook);
}

#[cfg(test)]
thread_local! {
    static NO_RENAME_NOREPLACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Makes renaming without replacing fail as on a filesystem that can't, on
/// this thread, for tests.
#[cfg(test)]
pub(super) fn set_no_rename_noreplace(on: bool) {
    NO_RENAME_NOREPLACE.with(|cell| cell.set(on));
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
