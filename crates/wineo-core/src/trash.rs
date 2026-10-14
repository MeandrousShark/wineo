//! Durable, explicitly recoverable environment removal.
//!
//! A manifest is the authority for a trash entry. Before a move, copy, source
//! unlink, restore, or prune, its intent is written through a synced temporary
//! file and the entry directory is synced. After the filesystem mutation both
//! affected parents are synced, then progress is written and synced. Recovery
//! accepts either side of an interrupted rename only if its recorded identity
//! matches. A metadata copy is verified and synced before the original is
//! unlinked; if both copies survive a crash, the original remains authoritative.
//! `removing` rolls back, `restoring` completes, and `pruning` only resumes
//! deletion. None of these phases can be used as an ordinary environment.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::gog::folder::{self, FolderLock};
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};

mod digest;
mod identity;
mod manifest;
mod markers;
mod payload;
mod remove;
mod restore;

pub(crate) use identity::foreign_mount;
#[cfg(test)]
pub(crate) use identity::set_fake_mounts;
use identity::{
    delete_owned, identity, parent, preflight_tree, require, require_same_mount_as_parent, sync,
};
#[cfg(any(test, feature = "test-support"))]
pub use manifest::take_manifest_reads;
use manifest::{read_manifest, write_manifest};
pub use markers::pending_entries;
use markers::{
    finish_marker, marker_for, marker_matches, marker_paths, marker_paths_at, markers,
    path_overlap, relocated_marker_root, retire_marker, start_marker,
};
use payload::{retained_identity, verified_retained, verify_parents};
pub use remove::{RemoveEvent, remove, remove_with, remove_with_force};
pub use restore::restore;
use restore::{complete_entry, restore_locked};

const VERSION: u32 = 1;
const MANIFEST: &str = "manifest.toml";
const PENDING_DIR: &str = ".trash-pending";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Intent {
    Remove,
    Restore,
    Prune,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingMarker {
    version: u32,
    intent: Intent,
    finished: bool,
    /// An XDG config root or the portable folder's path when published.
    root: PathBuf,
    /// The physical portable folder survives a same-filesystem rename.
    portable_identity: Option<Identity>,
    metadata_identity: Identity,
    manifest: Manifest,
}

#[derive(Debug, Clone)]
pub struct PendingEntry {
    pub id: String,
    pub name: String,
    pub finished: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Removing,
    Trashed,
    Restoring,
    Pruning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    Ready,
    Moving,
    Copying,
    Copied,
    DeletingSource,
    Retained,
    Returning,
    Returned,
    Deleting,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Identity {
    device: u64,
    inode: u64,
    #[serde(default)]
    mount: Option<u64>,
    #[serde(default)]
    birth: Option<(i64, u32)>,
}

impl Identity {
    fn matches(self, current: Self) -> bool {
        self.device == current.device
            && self.inode == current.inode
            && self.birth == current.birth
            // Without birth time, a changed mount ID could hide inode reuse.
            && (self.birth.is_some() || self.mount == current.mount)
    }
}

fn matches_identity(current: Option<Identity>, saved: Identity) -> bool {
    current.is_some_and(|current| saved.matches(current))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Config,
    Art,
    Stats,
    Prefix,
    Snapshots,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Payload {
    kind: Kind,
    original: String,
    retained: String,
    identity: Identity,
    /// The original parent and retained parent identify mounted storage. An
    /// empty mount point is not evidence that its contents were deleted.
    source_parent: Identity,
    destination_parent: Identity,
    copied_identity: Option<Identity>,
    #[serde(default)]
    returned_identity: Option<Identity>,
    #[serde(default)]
    out_stage: Option<Stage>,
    #[serde(default)]
    return_stage: Option<Stage>,
    digest: String,
    step: Step,
    /// Rollback found the source changed before its deletion began, so the
    /// source stays live and this retained copy stays in the trash.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    kept: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stage {
    location: String,
    identity: Identity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    id: String,
    entry_identity: Identity,
    name: String,
    removed_at: String,
    include_prefix: bool,
    /// None means the raw config did not identify a usable prefix path.
    prefix_present: Option<bool>,
    prefix_reference: Option<String>,
    prefix_identity: Option<Identity>,
    phase: Phase,
    payloads: Vec<Payload>,
    /// GOG game folders `rm` released from this environment, which a
    /// restore claims again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    gog_folders: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub removed_at: String,
    pub include_prefix: bool,
    pub prefix_present: Option<bool>,
    pub phase: Phase,
    pub prefix_left: Option<PathBuf>,
    pub snapshots_left: Option<PathBuf>,
    pub desktop_removed: usize,
}

fn id_valid(id: &str) -> bool {
    let Some((date, random)) = id.split_once('-') else {
        return false;
    };
    date.len() == 16
        && date.ends_with('Z')
        && date.as_bytes()[8] == b'T'
        && date[..8]
            .bytes()
            .chain(date[9..15].bytes())
            .all(|c| c.is_ascii_digit())
        && random.len() == 8
        && random.chars().all(|c| c.is_ascii_hexdigit())
}

fn entry_dir(paths: &Paths, id: &str) -> Result<PathBuf> {
    if !id_valid(id) {
        bail!("invalid trash ID `{id}`");
    }
    Ok(paths.trash_dir().join(id))
}

fn raw_entries(paths: &Paths) -> Result<Vec<String>> {
    let root = paths.trash_dir();
    if identity(&root)?.is_none() && !pending_entries(paths)?.is_empty() {
        bail!(
            "trash metadata storage at {} is unavailable; pending recovery remains",
            root.display()
        );
    }
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", root.display())),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if id.starts_with(".wineo-stage-") && entry.file_type()?.is_dir() {
            continue; // A crash before stage ownership was published can leave an orphan.
        }
        if id.starts_with(".wineo-manifest-") && entry.file_type()?.is_file() {
            continue;
        }
        if !id_valid(&id) || !entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
            bail!("unexpected trash entry at {}", entry.path().display());
        }
        // A crash before the first manifest publication, or after its final
        // unlink, can leave an empty ID directory but no owned payload.
        if fs::symlink_metadata(entry.path().join(MANIFEST)).is_err()
            && fs::read_dir(entry.path())?.next().is_none()
        {
            continue;
        }
        ids.push(id);
    }
    ids.sort();
    Ok(ids)
}

fn stored(paths: &Paths, path: &Path) -> Result<String> {
    if let Some(root) = paths.portable_root() {
        let relative = path.strip_prefix(root).with_context(|| {
            format!("trash payload outside portable folder: {}", path.display())
        })?;
        return Ok(relative
            .to_str()
            .ok_or_else(|| anyhow!("non-UTF8 trash path"))?
            .to_owned());
    }
    Ok(path
        .to_str()
        .ok_or_else(|| anyhow!("non-UTF8 trash path"))?
        .to_owned())
}

fn resolved(paths: &Paths, value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    let joined = match paths.portable_root() {
        Some(root) if !path.is_absolute() => root.join(path),
        None if path.is_absolute() => path.to_owned(),
        _ => bail!("trash path has wrong root: {value}"),
    };
    if joined
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        bail!("unsafe trash path: {value}");
    }
    Ok(joined)
}

fn info(paths: &Paths, manifest: &Manifest) -> Result<Entry> {
    let prefix_left = if !manifest.include_prefix {
        manifest
            .prefix_reference
            .as_deref()
            .map(|reference| resolved(paths, reference))
            .transpose()?
    } else {
        None
    };
    let snapshots_left = prefix_left
        .as_ref()
        .map(|prefix| paths.snapshot_root(prefix, &manifest.name));
    Ok(Entry {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        removed_at: manifest.removed_at.clone(),
        include_prefix: manifest.include_prefix,
        prefix_present: manifest.prefix_present,
        phase: manifest.phase,
        prefix_left,
        snapshots_left,
        desktop_removed: 0,
    })
}

pub fn list(paths: &Paths) -> Result<Vec<Entry>> {
    raw_entries(paths)?
        .iter()
        .map(|id| info(paths, &read_manifest(paths, id)?))
        .collect()
}

pub fn inspect(paths: &Paths, id: &str) -> Result<Entry> {
    info(paths, &read_manifest(paths, id)?)
}

/// Called under ordinary operation/config locks as well as before config
/// lookup. A missing config cannot hide an unfinished operation.
pub fn check_pending(paths: &Paths, name: &str, affected: &[&Path]) -> Result<()> {
    let scan = PendingScan::new(paths);
    match scan.find(paths, name, affected) {
        Ok(()) => Ok(()),
        Err(Blocked::Found(err)) => Err(err),
        Err(Blocked::Stored(slot)) => Err(scan.take(slot)),
    }
}

/// One read of the pending markers and trash manifests, so a command that
/// loads every environment reads the trash once instead of once per load.
/// It is a snapshot: keep it only for a batch of reads that cannot itself
/// trash, restore, or prune anything, and never hold it across commands.
pub struct PendingScan {
    markers: Result<Vec<PendingMarker>>,
    /// Read only once the markers allow it, matching the order a single
    /// check has always read them in.
    manifests: std::cell::OnceCell<Result<Vec<Result<Manifest>>>>,
}

enum Blocked {
    Found(anyhow::Error),
    Stored(Slot),
}

#[derive(Clone, Copy)]
enum Slot {
    Markers,
    Manifests,
    Manifest(usize),
}

impl PendingScan {
    pub fn new(paths: &Paths) -> Self {
        Self {
            markers: markers(paths)
                .map(|found| found.into_iter().map(|(_, marker)| marker).collect()),
            manifests: std::cell::OnceCell::new(),
        }
    }

    /// The same answer `check_pending` gives for this snapshot. A stored
    /// read error keeps only its top-level message, so callers that need the
    /// full context chain should use `check_pending`.
    pub fn check(&self, paths: &Paths, name: &str, affected: &[&Path]) -> Result<()> {
        match self.find(paths, name, affected) {
            Ok(()) => Ok(()),
            Err(Blocked::Found(err)) => Err(err),
            // Errors can't be cloned, and every load that hits a stored one
            // must still fail with its message.
            Err(Blocked::Stored(slot)) => Err(anyhow!("{}", self.stored(slot))),
        }
    }

    fn stored(&self, slot: Slot) -> &anyhow::Error {
        let manifests = self.manifests.get();
        match (slot, manifests) {
            (Slot::Markers, _) => self.markers.as_ref().err(),
            (Slot::Manifests, Some(manifests)) => manifests.as_ref().err(),
            (Slot::Manifest(index), Some(Ok(manifests))) => manifests[index].as_ref().err(),
            _ => None,
        }
        .expect("slot names a stored error")
    }

    fn take(self, slot: Slot) -> anyhow::Error {
        match slot {
            Slot::Markers => self.markers.err(),
            Slot::Manifests => self.manifests.into_inner().and_then(Result::err),
            Slot::Manifest(index) => self
                .manifests
                .into_inner()
                .and_then(Result::ok)
                .and_then(|mut manifests| manifests.swap_remove(index).err()),
        }
        .expect("slot names a stored error")
    }

    fn find(&self, paths: &Paths, name: &str, affected: &[&Path]) -> Result<(), Blocked> {
        let markers = self
            .markers
            .as_ref()
            .map_err(|_| Blocked::Stored(Slot::Markers))?;
        for marker in markers {
            Self::check_marker(paths, marker, name, affected).map_err(Blocked::Found)?;
        }
        let manifests = self
            .manifests
            .get_or_init(|| scan_manifests(paths))
            .as_ref()
            .map_err(|_| Blocked::Stored(Slot::Manifests))?;
        for (index, manifest) in manifests.iter().enumerate() {
            let manifest = manifest
                .as_ref()
                .map_err(|_| Blocked::Stored(Slot::Manifest(index)))?;
            Self::check_manifest(paths, manifest, name, affected).map_err(Blocked::Found)?;
        }
        Ok(())
    }

    fn check_marker(
        paths: &Paths,
        marker: &PendingMarker,
        name: &str,
        affected: &[&Path],
    ) -> Result<()> {
        let locations = marker_paths(paths, marker)?;
        let mut overlap = affected.iter().any(|candidate| {
            locations
                .iter()
                .any(|location| path_overlap(candidate, location))
        });
        if !overlap {
            for candidate in affected {
                if let Some(root) = relocated_marker_root(marker, candidate)? {
                    overlap = marker_paths_at(marker, &root)?
                        .iter()
                        .any(|location| path_overlap(candidate, location));
                    if overlap {
                        break;
                    }
                }
            }
        }
        if (marker_matches(paths, marker)? && marker.manifest.name == name) || overlap {
            bail!(
                "trash entry {} has pending {:?} for {}; recover with `wineo validate --fix`",
                marker.manifest.id,
                marker.intent,
                marker.manifest.name
            );
        }
        Ok(())
    }

    fn check_manifest(
        paths: &Paths,
        manifest: &Manifest,
        name: &str,
        affected: &[&Path],
    ) -> Result<()> {
        let id = &manifest.id;
        let overlaps = (manifest.phase != Phase::Trashed && manifest.name == name)
            || manifest
                .payloads
                .iter()
                .filter(|p| matches!(p.kind, Kind::Prefix | Kind::Snapshots))
                .any(|p| {
                    let values: &[&str] = if manifest.phase == Phase::Trashed {
                        &[p.retained.as_str()]
                    } else {
                        &[p.original.as_str(), p.retained.as_str()]
                    };
                    values.iter().any(|value| {
                        resolved(paths, value).is_ok_and(|path| {
                            affected.iter().any(|candidate| {
                                if candidate.starts_with(&path) || path.starts_with(candidate) {
                                    return true;
                                }
                                let candidate = paths::resolve(candidate);
                                let owned = paths::resolve(&path);
                                candidate.starts_with(&owned) || owned.starts_with(&candidate)
                            })
                        })
                    })
                });
        if overlaps {
            if manifest.phase == Phase::Trashed {
                bail!("prefix location belongs to trash entry {id}; restore or prune it first");
            }
            bail!(
                "trash entry {id} has interrupted {:?} for {}; recover with `wineo validate --fix`",
                manifest.phase,
                manifest.name
            );
        }
        Ok(())
    }
}

fn scan_manifests(paths: &Paths) -> Result<Vec<Result<Manifest>>> {
    if identity(&paths.trash_dir())?.is_none() {
        return Ok(Vec::new());
    }
    Ok(raw_entries(paths)?
        .iter()
        .map(|id| read_manifest(paths, id))
        .collect())
}

#[cfg(test)]
type DropHook = fn(&Path, &Path);

#[cfg(test)]
thread_local! {
    /// Runs once just before a restored copy's retained twin is dropped.
    static BEFORE_RETAINED_DROP: std::cell::Cell<Option<DropHook>> =
        const { std::cell::Cell::new(None) };
}

// Frontend tests need real interrupted operations without duplicating private markers.
#[cfg(any(test, feature = "test-support"))]
thread_local! { static TRASH_FAILPOINT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn set_trash_failpoint(at: Option<&'static str>) {
    TRASH_FAILPOINT.with(|point| point.set(at));
}

fn failpoint(_at: &str) -> Result<()> {
    #[cfg(any(test, feature = "test-support"))]
    TRASH_FAILPOINT.with(|point| {
        if point.get() == Some(_at) {
            point.set(None);
            bail!("injected trash interruption at {_at}");
        }
        Ok(())
    })?;
    Ok(())
}

fn lock_paths(paths: &Paths, manifest: &Manifest) -> Result<Vec<PathBuf>> {
    let mut locations = vec![entry_dir(paths, &manifest.id)?];
    for payload in &manifest.payloads {
        if matches!(payload.kind, Kind::Prefix | Kind::Snapshots) {
            locations.push(resolved(paths, &payload.original)?);
            locations.push(resolved(paths, &payload.retained)?);
        }
    }
    if let Some(reference) = &manifest.prefix_reference {
        locations.push(resolved(paths, reference)?);
    }
    Ok(locations)
}

/// Holds the entry's locks while `action` runs. With `gog`, the GOG folders
/// the removal released are locked exclusively too and passed along.
fn locked(
    paths: &Paths,
    id: &str,
    gog: bool,
    action: impl FnOnce(&mut Manifest, &[FolderLock]) -> Result<()>,
) -> Result<()> {
    let manifest = read_manifest(paths, id)?;
    let locations = lock_paths(paths, &manifest)?;
    let locks = Locks::new(paths);
    let _operation = locks.acquire(
        Operation::Trash,
        &manifest.name,
        &locations.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
    )?;
    let mut folders = if gog && paths.portable_root().is_none() {
        manifest.gog_folders.clone()
    } else {
        Vec::new()
    };
    folders.sort_by_key(|path| folder::key(path));
    folders.dedup();
    let gog_locks = folders
        .iter()
        .map(|path| FolderLock::exclusive(paths, path, &|| false))
        .collect::<Result<Vec<_>>>()?;
    let _config = locks.config_recovery(&paths.environment_config(&manifest.name))?;
    let _art_config = locks.config_recovery(&paths.art_dir(&manifest.name).join("art.toml"))?;
    let mut current = read_manifest(paths, id)?;
    if current.name != manifest.name
        || lock_paths(paths, &current)? != locations
        || current.gog_folders != manifest.gog_folders
    {
        bail!("trash entry changed while acquiring locks; retry");
    }
    action(&mut current, &gog_locks)
}

/// Validate once before confirmation and again while holding the entry,
/// environment, config, and both prefix location locks.
#[derive(Debug, Clone)]
pub struct PruneReview {
    pub entries: Vec<Entry>,
    pub cutoff: Option<jiff::Timestamp>,
    pub errors: Vec<(String, String)>,
    fingerprints: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Selector<'a> {
    Id(&'a str),
    All,
    OlderThan(u32),
}

pub fn review_prune(paths: &Paths, selector: Selector<'_>) -> Result<PruneReview> {
    review_prune_at(paths, selector, jiff::Timestamp::now())
}

fn review_prune_at(
    paths: &Paths,
    selector: Selector<'_>,
    now: jiff::Timestamp,
) -> Result<PruneReview> {
    let cutoff = match selector {
        Selector::OlderThan(days) if days > 0 => {
            Some(now.checked_sub(jiff::Span::new().hours(i64::from(days) * 24))?)
        }
        Selector::OlderThan(_) => bail!("age must be positive whole days"),
        _ => None,
    };
    let ids = match selector {
        Selector::Id(id) => vec![id.to_owned()],
        _ => raw_entries(paths)?,
    };
    let mut selected = Vec::new();
    let mut fingerprints = Vec::new();
    let mut errors = Vec::new();
    for id in ids {
        let entry = match inspect(paths, &id) {
            Ok(entry) => entry,
            Err(err) if matches!(selector, Selector::Id(_)) => return Err(err),
            Err(err) => {
                errors.push((id, format!("{err:#}")));
                continue;
            }
        };
        if entry.phase != Phase::Trashed {
            continue;
        }
        if let Some(cutoff) = cutoff {
            let removed: jiff::Timestamp = entry.removed_at.parse()?;
            if removed >= cutoff {
                continue;
            }
        }
        let bytes = fs::read(entry_dir(paths, &entry.id)?.join(MANIFEST))?;
        fingerprints.push(format!("{:x}", Sha256::digest(bytes)));
        selected.push(entry);
    }
    Ok(PruneReview {
        entries: selected,
        cutoff,
        errors,
        fingerprints,
    })
}

fn preflight_prune(paths: &Paths, manifest: &Manifest) -> Result<()> {
    for payload in &manifest.payloads {
        let (_, retained) = verify_parents(paths, payload)?;
        verified_retained(&retained, payload)?;
        if retained.is_dir() {
            preflight_tree(&retained)?;
        }
    }
    Ok(())
}

fn prune_locked(paths: &Paths, manifest: &mut Manifest) -> Result<()> {
    if manifest.phase == Phase::Trashed {
        preflight_prune(paths, manifest)?;
        start_marker(paths, manifest, Intent::Prune)?;
        manifest.phase = Phase::Pruning;
        write_manifest(paths, manifest)?;
    }
    if manifest.phase != Phase::Pruning {
        bail!("entry is not eligible for pruning");
    }
    for index in 0..manifest.payloads.len() {
        let payload = manifest.payloads[index].clone();
        if payload.step == Step::Deleted {
            continue;
        }
        let (_, retained) = verify_parents(paths, &payload)?;
        match identity(&retained)? {
            Some(found) if retained_identity(&payload).matches(found) => {
                require_same_mount_as_parent(&retained)?;
                manifest.payloads[index].step = Step::Deleting;
                write_manifest(paths, manifest)?;
                delete_owned(&retained)?;
                require(parent(&retained)?, payload.destination_parent)?;
            }
            Some(_) => bail!("retained payload changed at {}", retained.display()),
            None if payload.step == Step::Deleting => {}
            None => bail!("retained storage unavailable at {}", retained.display()),
        }
        manifest.payloads[index].step = Step::Deleted;
        write_manifest(paths, manifest)?;
    }
    complete_entry(paths, manifest)
}

/// Keeps an individual failed entry's record; callers continue independent
/// entries and report all failures rather than stopping a batch early.
pub fn prune_reviewed(paths: &Paths, review: &PruneReview) -> Vec<(String, Result<()>)> {
    review
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let result = locked(paths, &entry.id, false, |manifest, _| {
                let raw = fs::read(entry_dir(paths, &entry.id)?.join(MANIFEST))?;
                if format!("{:x}", Sha256::digest(raw)) != review.fingerprints[index]
                    || info(paths, manifest)? != *entry
                {
                    bail!("entry changed since confirmation; review again");
                }
                prune_locked(paths, manifest)
            });
            (entry.id.clone(), result)
        })
        .collect()
}

pub fn recover(paths: &Paths, id: &str) -> Result<()> {
    recover_noting(paths, id).map(|_| ())
}

/// `recover`, returning what a rollback kept in the trash instead of
/// returning, for the caller to report.
pub fn recover_noting(paths: &Paths, id: &str) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    if let Some((_, marker)) = marker_for(paths, id)? {
        require(&paths.trash_dir(), marker.metadata_identity)?;
        if let Some(saved) = marker.portable_identity {
            let root = paths
                .portable_root()
                .ok_or_else(|| anyhow!("portable folder unavailable"))?;
            require(root, saved)?;
        }
        let dir = entry_dir(paths, id)?;
        if identity(&dir)?.is_none() || !dir.join(MANIFEST).exists() {
            let locations = marker_paths(paths, &marker)?;
            let locks = Locks::new(paths);
            let _guard = locks.acquire(
                Operation::Trash,
                &marker.manifest.name,
                &locations.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
            )?;
            let _config =
                locks.config_recovery(&paths.environment_config(&marker.manifest.name))?;
            if dir.join(MANIFEST).exists() {
                bail!("trash manifest appeared during recovery; retry");
            }
            if marker.finished || marker.intent == Intent::Remove {
                if !marker.finished {
                    for payload in &marker.manifest.payloads {
                        let source = resolved(paths, &payload.original)?;
                        let retained = resolved(paths, &payload.retained)?;
                        require(&source, payload.identity)?;
                        if identity(&retained)?.is_some() {
                            bail!(
                                "trash entry {id} has a payload without its manifest at {}; marker retained",
                                retained.display()
                            );
                        }
                    }
                }
                if identity(&dir)?.is_some() {
                    if fs::read_dir(&dir)?.next().is_some() {
                        bail!(
                            "trash entry {id} contains data without its manifest; marker retained"
                        );
                    }
                    fs::remove_dir(&dir)?;
                    sync(&paths.trash_dir())?;
                }
                if !marker.finished {
                    finish_marker(paths, &marker.manifest)?;
                }
                retire_marker(paths, &marker.manifest)?;
                return Ok(notes);
            }
            bail!(
                "trash entry {id} lost its manifest during {:?}; marker retained",
                marker.intent
            );
        }
    }
    locked(paths, id, true, |manifest, gog| match manifest.phase {
        Phase::Removing => restore_locked(paths, manifest, true, gog, &mut notes),
        Phase::Restoring => restore_locked(paths, manifest, false, gog, &mut notes),
        Phase::Pruning => prune_locked(paths, manifest),
        Phase::Trashed if marker_for(paths, id)?.is_some() => {
            finish_marker(paths, manifest)?;
            retire_marker(paths, manifest)
        }
        Phase::Trashed => bail!("entry {id} is not interrupted"),
    })?;
    Ok(notes)
}

#[cfg(test)]
mod tests;
