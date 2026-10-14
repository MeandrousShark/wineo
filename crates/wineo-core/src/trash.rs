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

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::{RenameFlags, renameat_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::build_lock;
use crate::config::{self, EnvironmentConfig, GlobalConfig};
use crate::desktop;
use crate::environment::{self, Environment};
use crate::gog::folder::{self, FolderError, FolderLock};
use crate::name;
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};
use crate::prefix;
use crate::runtime::Runtime;

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

fn identity(path: &Path) -> Result<Option<Identity>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || (!meta.is_file() && !meta.is_dir()) {
                bail!("refusing non-regular trash payload at {}", path.display());
            }
            let (mount, birth) = match rustix::fs::statx(
                rustix::fs::CWD,
                path,
                rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
                rustix::fs::StatxFlags::MNT_ID | rustix::fs::StatxFlags::BTIME,
            ) {
                Ok(stat) => {
                    let mount = (stat.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() != 0)
                        .then_some(stat.stx_mnt_id);
                    let birth = (stat.stx_mask & rustix::fs::StatxFlags::BTIME.bits() != 0)
                        .then_some((stat.stx_btime.tv_sec, stat.stx_btime.tv_nsec));
                    (mount, birth)
                }
                Err(rustix::io::Errno::NOSYS) => (None, None),
                Err(err) => return Err(err.into()),
            };
            Ok(Some(Identity {
                device: meta.dev(),
                inode: meta.ino(),
                mount,
                birth,
            }))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("inspecting {}", path.display())),
    }
}

fn require(path: &Path, expected: Identity) -> Result<()> {
    if !matches_identity(identity(path)?, expected) {
        bail!(
            "identity changed or storage unavailable at {}",
            path.display()
        );
    }
    Ok(())
}

fn require_same_mount_as_parent(path: &Path) -> Result<()> {
    let current = identity(path)?.ok_or_else(|| anyhow!("payload unavailable"))?;
    let parent = identity(parent(path)?)?.ok_or_else(|| anyhow!("payload storage unavailable"))?;
    if current.mount != parent.mount {
        bail!("payload root is a separate mount at {}", path.display());
    }
    Ok(())
}

fn parent(path: &Path) -> Result<&Path> {
    path.parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", path.display()))
}

fn create_storage(path: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut current = path;
    while identity(current)?.is_none() {
        missing.push(current.to_owned());
        current = parent(current)?;
    }
    for dir in missing.into_iter().rev() {
        fs::create_dir(&dir)?;
        sync(parent(&dir)?)?;
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        bail!("unsafe trash storage at {}", path.display());
    }
    Ok(())
}

fn sync(path: &Path) -> Result<()> {
    File::open(path)?
        .sync_all()
        .with_context(|| format!("syncing {}", path.display()))
}

fn marker_root(paths: &Paths) -> Result<(PathBuf, Option<Identity>)> {
    if let Some(root) = paths.portable_root() {
        return Ok((root.to_owned(), identity(root)?));
    }
    Ok((parent(&paths.environments_dir())?.to_owned(), None))
}

fn marker_dir(paths: &Paths) -> PathBuf {
    paths.prefix_locks_dir().join(PENDING_DIR)
}

fn marker_filename(marker: &PendingMarker) -> String {
    let mut hash = Sha256::new();
    hash.update(marker.root.as_os_str().as_encoded_bytes());
    if let Some(id) = marker.portable_identity {
        hash.update(id.device.to_le_bytes());
        hash.update(id.inode.to_le_bytes());
    }
    let bytes = hash.finalize();
    format!(
        "trash-{}-{:016x}.toml",
        marker.manifest.id,
        u64::from_le_bytes(bytes[..8].try_into().unwrap())
    )
}

fn markers(paths: &Paths) -> Result<Vec<(PathBuf, PendingMarker)>> {
    let dir = marker_dir(paths);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        // Atomic marker writes use temporary files. Their old contents are
        // never authoritative, so only published marker names are inspected.
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("trash-") || !name.ends_with(".toml") {
            continue;
        }
        let path = entry.path();
        if !entry.file_type()?.is_file() || entry.file_type()?.is_symlink() {
            bail!("invalid pending marker at {}", path.display());
        }
        let marker: PendingMarker = toml::from_str(&fs::read_to_string(&path)?)
            .with_context(|| format!("invalid pending marker at {}", path.display()))?;
        if marker.version != VERSION
            || marker.manifest.version != VERSION
            || !id_valid(&marker.manifest.id)
            || !crate::name::is_valid(&marker.manifest.name)
            || !marker.root.is_absolute()
            || marker_filename(&marker) != name
        {
            bail!("invalid pending marker at {}", path.display());
        }
        for payload in &marker.manifest.payloads {
            for reference in [&payload.original, &payload.retained] {
                let value = Path::new(reference);
                if value
                    .components()
                    .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
                    || value.is_absolute() == marker.portable_identity.is_some()
                {
                    bail!("unsafe pending marker path at {}", path.display());
                }
            }
        }
        found.push((path, marker));
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(found)
}

fn marker_matches(paths: &Paths, marker: &PendingMarker) -> Result<bool> {
    let (root, physical) = marker_root(paths)?;
    Ok(match (marker.portable_identity, physical) {
        (Some(saved), Some(current)) => saved.matches(current) || marker.root == root,
        (Some(_), None) => marker.root == root,
        (None, None) => paths.portable_root().is_none() && marker.root == root,
        _ => false,
    })
}

fn marker_paths(paths: &Paths, marker: &PendingMarker) -> Result<Vec<PathBuf>> {
    let portable_root = if marker.portable_identity.is_some() {
        if marker_matches(paths, marker)? {
            paths.portable_root().unwrap_or(&marker.root)
        } else {
            &marker.root
        }
    } else {
        &marker.root
    };
    marker_paths_at(marker, portable_root)
}

fn marker_paths_at(marker: &PendingMarker, root: &Path) -> Result<Vec<PathBuf>> {
    marker
        .manifest
        .payloads
        .iter()
        .filter(|payload| matches!(payload.kind, Kind::Prefix | Kind::Snapshots))
        .flat_map(|payload| [&payload.original, &payload.retained])
        .map(|reference| {
            let path = Path::new(reference);
            Ok(if path.is_absolute() {
                path.to_owned()
            } else {
                root.join(path)
            })
        })
        .collect()
}

fn relocated_marker_root(marker: &PendingMarker, candidate: &Path) -> Result<Option<PathBuf>> {
    let Some(saved) = marker.portable_identity else {
        return Ok(None);
    };
    let resolved = paths::resolve(candidate);
    for ancestor in resolved.ancestors() {
        if matches_identity(identity(ancestor)?, saved) {
            return Ok(Some(ancestor.to_owned()));
        }
    }
    Ok(None)
}

fn path_overlap(candidate: &Path, location: &Path) -> bool {
    candidate.starts_with(location)
        || location.starts_with(candidate)
        || paths::resolve(candidate).starts_with(paths::resolve(location))
        || paths::resolve(location).starts_with(paths::resolve(candidate))
}

fn marker_for(paths: &Paths, id: &str) -> Result<Option<(PathBuf, PendingMarker)>> {
    let mut matches = Vec::new();
    for (path, marker) in markers(paths)? {
        if marker.manifest.id == id && marker_matches(paths, &marker)? {
            matches.push((path, marker));
        }
    }
    if matches.len() > 1 {
        bail!("several pending markers for trash entry {id}");
    }
    Ok(matches.into_iter().next())
}

fn publish_marker(path: &Path, marker: &PendingMarker) -> Result<()> {
    create_storage(parent(path)?)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent(path)?)?;
    temp.write_all(toml::to_string_pretty(marker)?.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|err| anyhow!(err.error))?;
    sync(parent(path)?)
}

fn start_marker(paths: &Paths, manifest: &Manifest, intent: Intent) -> Result<()> {
    if marker_for(paths, &manifest.id)?.is_some() {
        bail!(
            "trash entry {} has interrupted work; run `wineo validate --fix`",
            manifest.id
        );
    }
    let (root, portable_identity) = marker_root(paths)?;
    if paths.portable_root().is_some() && portable_identity.is_none() {
        bail!("portable folder is unavailable; pending trash marker cannot be created");
    }
    let marker = PendingMarker {
        version: VERSION,
        intent,
        finished: false,
        root,
        portable_identity,
        metadata_identity: identity(&paths.trash_dir())?
            .ok_or_else(|| anyhow!("trash metadata unavailable"))?,
        manifest: manifest.clone(),
    };
    failpoint("marker.before_publish")?;
    publish_marker(&marker_dir(paths).join(marker_filename(&marker)), &marker)?;
    failpoint("marker.after_publish")
}

fn finish_marker(paths: &Paths, manifest: &Manifest) -> Result<()> {
    let (path, mut marker) = marker_for(paths, &manifest.id)?
        .ok_or_else(|| anyhow!("pending marker missing for {}", manifest.id))?;
    if marker.manifest.name != manifest.name {
        bail!("pending marker owner changed");
    }
    marker.finished = true;
    failpoint("marker.before_finish")?;
    publish_marker(&path, &marker)?;
    failpoint("marker.after_finish")
}

fn retire_marker(paths: &Paths, manifest: &Manifest) -> Result<()> {
    let (path, marker) = marker_for(paths, &manifest.id)?
        .ok_or_else(|| anyhow!("pending marker missing for {}", manifest.id))?;
    require(&paths.trash_dir(), marker.metadata_identity)?;
    if !marker.finished || marker.manifest.name != manifest.name {
        bail!("pending marker for {} is not complete", manifest.id);
    }
    failpoint("marker.before_retire")?;
    fs::remove_file(&path)?;
    sync(parent(&path)?)?;
    failpoint("marker.after_retire")
}

pub fn pending_entries(paths: &Paths) -> Result<Vec<PendingEntry>> {
    let mut entries = Vec::new();
    for (_, marker) in markers(paths)? {
        if marker_matches(paths, &marker)? {
            entries.push(PendingEntry {
                id: marker.manifest.id,
                name: marker.manifest.name,
                finished: marker.finished,
            });
        }
    }
    Ok(entries)
}

fn write_manifest(paths: &Paths, manifest: &Manifest) -> Result<()> {
    let dir = entry_dir(paths, &manifest.id)?;
    let text = toml::to_string_pretty(manifest)?;
    // Keep an interrupted temporary manifest outside the entry so its
    // presence cannot obstruct final entry cleanup after recovery.
    let mut temporary = tempfile::Builder::new()
        .prefix(".wineo-manifest-")
        .tempfile_in(paths.trash_dir())?;
    temporary.write_all(text.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(dir.join(MANIFEST))
        .map_err(|err| anyhow!(err.error))?;
    sync(&dir)?;
    sync(&paths.trash_dir())
}

// Batch callers are tested by how many manifests one batch reads.
#[cfg(any(test, feature = "test-support"))]
thread_local! { static MANIFEST_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// Manifest reads on this thread since the last call.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn take_manifest_reads() -> usize {
    MANIFEST_READS.with(|reads| reads.replace(0))
}

fn read_manifest(paths: &Paths, id: &str) -> Result<Manifest> {
    #[cfg(any(test, feature = "test-support"))]
    MANIFEST_READS.with(|reads| reads.set(reads.get() + 1));
    identity(&paths.trash_dir())?.ok_or_else(|| anyhow!("trash storage unavailable"))?;
    let dir = entry_dir(paths, id)?;
    if !fs::symlink_metadata(&dir)?.file_type().is_dir() {
        bail!("trash entry is not a directory: {}", dir.display());
    }
    let file = dir.join(MANIFEST);
    if !fs::symlink_metadata(&file)?.file_type().is_file() {
        bail!("trash manifest is not a regular file: {}", file.display());
    }
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&file)?)?;
    if manifest.version != VERSION
        || manifest.id != id
        || !name::is_valid(&manifest.name)
        || !matches_identity(identity(&dir)?, manifest.entry_identity)
        || manifest.removed_at.parse::<jiff::Timestamp>().is_err()
    {
        bail!(
            "invalid or unsupported trash manifest at {}",
            file.display()
        );
    }
    let mut seen = std::collections::HashSet::new();
    for payload in &manifest.payloads {
        let original = resolved(paths, &payload.original)?;
        let retained = resolved(paths, &payload.retained)?;
        if !seen.insert(payload.kind as u8) || original == retained {
            bail!("invalid payload in {}", file.display());
        }
        if payload.copied_identity.is_some()
            && (matches!(payload.kind, Kind::Prefix | Kind::Snapshots)
                || payload.source_parent.mount == payload.destination_parent.mount)
        {
            bail!(
                "trash manifest claims an invalid copied payload in {}",
                file.display()
            );
        }
        let expected = match payload.kind {
            Kind::Config => paths.environment_config(&manifest.name),
            Kind::Art => paths.art_dir(&manifest.name),
            Kind::Stats => paths.stats_log(&manifest.name),
            Kind::Prefix => {
                let reference = manifest
                    .prefix_reference
                    .as_deref()
                    .ok_or_else(|| anyhow!("missing prefix reference"))?;
                resolved(paths, reference)?
            }
            Kind::Snapshots => {
                let reference = manifest
                    .prefix_reference
                    .as_deref()
                    .ok_or_else(|| anyhow!("missing prefix reference"))?;
                paths.snapshot_root(&resolved(paths, reference)?, &manifest.name)
            }
        };
        if original != expected
            || !retained.starts_with(paths.trash_dir())
                && !matches!(payload.kind, Kind::Prefix | Kind::Snapshots)
        {
            bail!(
                "trash manifest claims unrelated payload: {}",
                original.display()
            );
        }
        for (stage, destination) in [
            (&payload.out_stage, &retained),
            (&payload.return_stage, &original),
        ] {
            if let Some(stage) = stage {
                let location = resolved(paths, &stage.location)?;
                let staging_parent = if destination.parent() == Some(dir.as_path()) {
                    parent(&dir)?
                } else {
                    parent(destination)?
                };
                if location.parent() != Some(staging_parent)
                    || !location
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(".wineo-stage-"))
                {
                    bail!("unsafe staging location in {}", file.display());
                }
            }
        }
        if matches!(payload.kind, Kind::Prefix | Kind::Snapshots) {
            let valid = if payload.kind == Kind::Prefix {
                retained
                    == parent(&expected)?
                        .join(".wineo-trash")
                        .join(id)
                        .join("prefix")
            } else {
                let snapshots_parent = parent(&expected)?;
                let prefix = resolved(paths, manifest.prefix_reference.as_deref().unwrap())?;
                let prefix_parent = parent(&prefix)?;
                [prefix_parent, snapshots_parent]
                    .into_iter()
                    .any(|base| retained == base.join(".wineo-trash").join(id).join("snapshots"))
            };
            if !valid {
                bail!(
                    "trash manifest claims unrelated retained path: {}",
                    retained.display()
                );
            }
        } else if retained
            != entry_dir(paths, id)?.join(match payload.kind {
                Kind::Config => "config.toml",
                Kind::Art => "art",
                Kind::Stats => "stats.jsonl",
                _ => unreachable!(),
            })
        {
            bail!(
                "trash manifest claims unrelated retained path: {}",
                retained.display()
            );
        }
    }
    if !seen.contains(&(Kind::Config as u8)) {
        bail!("trash entry has no config payload");
    }
    if seen.contains(&(Kind::Prefix as u8)) && !manifest.include_prefix
        || seen.contains(&(Kind::Snapshots as u8)) && !manifest.include_prefix
        || seen.contains(&(Kind::Prefix as u8)) && manifest.prefix_present != Some(true)
    {
        bail!("trash manifest has inconsistent prefix ownership");
    }
    if let Some(reference) = &manifest.prefix_reference {
        let config = manifest
            .payloads
            .iter()
            .find(|payload| payload.kind == Kind::Config)
            .unwrap();
        let source = resolved(paths, &config.original)?;
        let retained = resolved(paths, &config.retained)?;
        let current = identity(&source)?;
        let saved = identity(&retained)?;
        let bytes = if matches_identity(current, config.identity) {
            Some(fs::read(&source)?)
        } else if matches_identity(saved, retained_identity(config))
            && digest_matches(&retained, &config.digest)?
        {
            Some(fs::read(&retained)?)
        } else if config
            .returned_identity
            .is_some_and(|returned| matches_identity(current, returned))
            || saved.is_none() && matches_identity(current, retained_identity(config))
        {
            // A returned copy may still be partial. Recovery validates it
            // before changing either side; the config is only parsed once it
            // is complete.
            if digest_matches(&source, &config.digest)? {
                Some(fs::read(&source)?)
            } else {
                None
            }
        } else if matches!(manifest.phase, Phase::Pruning | Phase::Removing)
            && matches!(config.step, Step::Deleting | Step::Deleted)
        {
            None
        } else {
            bail!("config payload unavailable in trash entry {id}");
        };
        if let Some(bytes) = bytes {
            let text = String::from_utf8(bytes)?;
            let parsed = EnvironmentConfig::parse(&source, &text)?;
            let actual = match &parsed.kind {
                config::Kind::Wine(wine) => wine,
                config::Kind::Native => bail!("prefix payload belongs to a native config"),
                config::Kind::Proxied(_) => bail!("prefix payload belongs to a proxied config"),
            };
            if paths::resolve(&paths.config_path(&actual.prefix)?) != resolved(paths, reference)? {
                bail!("trash entry {id} prefix differs from its saved config");
            }
        }
    }
    Ok(manifest)
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

/// Marks digests written in the unambiguous encoding. Older manifests hold
/// bare v1 digests, which `digest_matches` still checks the old way.
const DIGEST_V2: &str = "v2:";

/// A digest of a payload tree. Every name and symlink target is length
/// prefixed and every file is hashed on its own, so no two different trees
/// produce the same input, as v1's plain concatenation could.
fn digest(path: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    fn visit(path: &Path, hash: &mut Sha256) -> Result<()> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(path)?;
            let target = target.as_os_str().as_encoded_bytes();
            hash.update(b"l");
            hash.update((target.len() as u64).to_le_bytes());
            hash.update(target);
        } else if meta.is_file() {
            let mut file_hash = Sha256::new();
            let mut file = File::open(path)?;
            let mut buffer = [0u8; 65536];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                file_hash.update(&buffer[..count]);
            }
            hash.update(b"f");
            hash.update(file_hash.finalize());
        } else if meta.is_dir() {
            let mut children = fs::read_dir(path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<io::Result<Vec<_>>>()?;
            children.sort();
            hash.update(b"d");
            hash.update((children.len() as u64).to_le_bytes());
            for child in children {
                let name = child.file_name().unwrap().as_encoded_bytes();
                hash.update((name.len() as u64).to_le_bytes());
                hash.update(name);
                visit(&child, hash)?;
            }
        } else {
            bail!("unsupported payload at {}", path.display());
        }
        Ok(())
    }
    visit(path, &mut hash)?;
    Ok(format!("{DIGEST_V2}{:x}", hash.finalize()))
}

/// Whether `path` still has the recorded digest, in whichever encoding it
/// was recorded.
fn digest_matches(path: &Path, recorded: &str) -> Result<bool> {
    if recorded.starts_with(DIGEST_V2) {
        Ok(digest(path)? == recorded)
    } else {
        Ok(digest_v1(path)? == recorded)
    }
}

#[cfg(test)]
type DropHook = fn(&Path, &Path);

#[cfg(test)]
thread_local! {
    /// Runs once just before a restored copy's retained twin is dropped.
    static BEFORE_RETAINED_DROP: std::cell::Cell<Option<DropHook>> =
        const { std::cell::Cell::new(None) };
}

/// Whether two trees hold exactly the same names, contents, and links. A
/// digest match decides nothing destructive on its own, since a v1 digest
/// from an older manifest can collide.
fn same_tree(a: &Path, b: &Path) -> Result<bool> {
    Ok(contained_in(a, b)? && contained_in(b, a)?)
}

fn digest_v1(path: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    fn visit(path: &Path, hash: &mut Sha256) -> Result<()> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            hash.update(b"link");
            hash.update(fs::read_link(path)?.as_os_str().as_encoded_bytes());
        } else if meta.is_file() {
            hash.update(b"file");
            let mut file = File::open(path)?;
            let mut buffer = [0u8; 65536];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
        } else if meta.is_dir() {
            hash.update(b"dir");
            let mut children = fs::read_dir(path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<io::Result<Vec<_>>>()?;
            children.sort();
            for child in children {
                hash.update(child.file_name().unwrap().as_encoded_bytes());
                visit(&child, hash)?;
            }
        } else {
            bail!("unsupported payload at {}", path.display());
        }
        Ok(())
    }
    visit(path, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

/// Whether everything in `part` is also in `whole` with the same contents,
/// as a partly deleted copy of `whole` would be.
fn contained_in(part: &Path, whole: &Path) -> Result<bool> {
    let (part_meta, whole_meta) = (fs::symlink_metadata(part)?, fs::symlink_metadata(whole)?);
    if part_meta.file_type().is_symlink() {
        return Ok(
            whole_meta.file_type().is_symlink() && fs::read_link(part)? == fs::read_link(whole)?
        );
    }
    if part_meta.is_file() {
        return Ok(whole_meta.is_file() && fs::read(part)? == fs::read(whole)?);
    }
    if !part_meta.is_dir() || !whole_meta.is_dir() {
        return Ok(false);
    }
    for entry in fs::read_dir(part)? {
        let name = entry?.file_name();
        let other = whole.join(&name);
        if fs::symlink_metadata(&other).is_err() || !contained_in(&part.join(&name), &other)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn copy_verified(source: &Path, destination: &Path, expected: &str) -> Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.is_file() {
        // An existing reservation must be a regular file, never a substituted
        // link. The normal stage path is new and is opened exclusively.
        let existing = identity(destination)?;
        if existing.is_some() && !fs::symlink_metadata(destination)?.is_file() {
            bail!("staging collision at {}", destination.display());
        }
        use std::os::unix::fs::OpenOptionsExt;
        let mut to = fs::OpenOptions::new()
            .write(true)
            .create_new(existing.is_none())
            .truncate(existing.is_some())
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(destination)?;
        io::copy(&mut File::open(source)?, &mut to)?;
        to.sync_all()?;
    } else if meta.is_dir() {
        if identity(destination)?.is_none() {
            fs::create_dir(destination)?;
        }
        fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
            for entry in fs::read_dir(source)? {
                let entry = entry?;
                let from = entry.path();
                let to = destination.join(entry.file_name());
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    fs::create_dir(&to)?;
                    copy_tree(&from, &to)?;
                    sync(&to)?;
                } else if kind.is_symlink() {
                    std::os::unix::fs::symlink(fs::read_link(&from)?, &to)?;
                } else if kind.is_file() {
                    let mut file = fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&to)?;
                    io::copy(&mut File::open(&from)?, &mut file)?;
                    file.sync_all()?;
                } else {
                    bail!("unsupported payload at {}", from.display());
                }
            }
            Ok(())
        }
        copy_tree(source, destination)?;
        sync(destination)?;
    } else {
        bail!("unsafe payload at {}", source.display());
    }
    sync(parent(destination)?)?;
    if !digest_matches(destination, expected)? {
        bail!(
            "metadata copy verification failed at {}",
            destination.display()
        );
    }
    Ok(())
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

fn stage_location(paths: &Paths, stage: &Stage) -> Result<PathBuf> {
    resolved(paths, &stage.location)
}

fn discard_stage(paths: &Paths, stage: &Stage) -> Result<()> {
    let dir = stage_location(paths, stage)?;
    if identity(&dir)?.is_some() {
        require(&dir, stage.identity)?;
        require_same_mount_as_parent(&dir)?;
        delete_owned(&dir)?;
    }
    Ok(())
}

fn prepare_stage(
    paths: &Paths,
    manifest: &mut Manifest,
    index: usize,
    destination: &Path,
    returning: bool,
) -> Result<Stage> {
    let dir = entry_dir(paths, &manifest.id)?;
    let staging_parent = if destination.parent() == Some(dir.as_path()) {
        parent(&dir)?
    } else {
        parent(destination)?
    };
    failpoint(if returning {
        "return.before_reservation"
    } else {
        "out.before_reservation"
    })?;
    let reserved = tempfile::Builder::new()
        .prefix(".wineo-stage-")
        .tempdir_in(staging_parent)?;
    sync(staging_parent)?;
    let stage = Stage {
        location: stored(paths, reserved.path())?,
        identity: identity(reserved.path())?.ok_or_else(|| anyhow!("staging disappeared"))?,
    };
    // The final payload path remains free until this stage identity has been
    // durably recorded. An orphan before that write cannot be mistaken for a
    // copy owned by this manifest.
    let _path = reserved.keep();
    failpoint(if returning {
        "return.after_reservation"
    } else {
        "out.after_reservation"
    })?;
    if returning {
        manifest.payloads[index].return_stage = Some(stage.clone());
    } else {
        manifest.payloads[index].out_stage = Some(stage.clone());
    }
    failpoint(if returning {
        "return.before_stage_write"
    } else {
        "out.before_stage_write"
    })?;
    write_manifest(paths, manifest)?;
    failpoint(if returning {
        "return.after_stage_write"
    } else {
        "out.after_stage_write"
    })?;
    Ok(stage)
}

#[cfg(test)]
fn reserve_copy(source: &Path, destination: &Path) -> Result<Identity> {
    if fs::symlink_metadata(source)?.is_dir() {
        fs::create_dir(destination)?;
    } else {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?
            .sync_all()?;
    }
    sync(parent(destination)?)?;
    identity(destination)?
        .ok_or_else(|| anyhow!("staged copy disappeared at {}", destination.display()))
}

fn delete_owned(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        preflight_tree(path)?;
        delete_subvolumes(path)?;
        if identity(path)?.is_some() {
            fs::remove_dir_all(path)?;
        }
    } else {
        fs::remove_file(path)?;
    }
    sync(parent(path)?)
}

fn is_subvolume(path: &Path) -> Result<bool> {
    let meta = fs::symlink_metadata(path)?;
    Ok(
        meta.is_dir()
            && meta.ino() == 256
            && rustix::fs::statfs(path)?.f_type as u32 == 0x9123_683e,
    )
}

fn subvolume_property(path: &Path) -> Result<Option<bool>> {
    if !is_subvolume(path)? {
        return Ok(None);
    }
    let btrfs = crate::runner::find_on_path("btrfs")
        .ok_or_else(|| anyhow!("Btrfs subvolume at {} needs btrfs-progs", path.display()))?;
    let output = Command::new(btrfs)
        .args(["property", "get", "-ts"])
        .arg(path)
        .arg("ro")
        .output()?;
    if !output.status.success() {
        bail!(
            "cannot verify Btrfs subvolume at {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    match String::from_utf8(output.stdout)?.trim() {
        "ro=true" => Ok(Some(true)),
        "ro=false" => Ok(Some(false)),
        other => bail!("unexpected Btrfs property at {}: {other}", path.display()),
    }
}

#[cfg(test)]
thread_local! {
    static FAKE_MOUNTS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Makes the mount checks treat `paths` as mount points, since a real bind
/// mount needs root.
#[cfg(test)]
pub(crate) fn set_fake_mounts(paths: Vec<PathBuf>) {
    FAKE_MOUNTS.with(|mounts| *mounts.borrow_mut() = paths);
}

/// `path`'s mount ID. A test's fake mount gives everything under it an ID
/// of its own, like a real bind mount would.
fn mount_of(path: &Path, current: Identity) -> Option<u64> {
    #[cfg(test)]
    if let Some(fake) = FAKE_MOUNTS.with(|mounts| {
        mounts
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, mount)| path.starts_with(mount))
            .max_by_key(|(_, mount)| mount.as_os_str().len())
            .map(|(index, _)| u64::MAX - index as u64)
    }) {
        return Some(fake);
    }
    let _ = path;
    current.mount
}

fn same_tree_mount(root: Identity, path: &Path) -> Result<bool> {
    let current =
        identity(path)?.ok_or_else(|| anyhow!("storage unavailable at {}", path.display()))?;
    let mount = mount_of(path, current);
    if root.mount.is_none() || mount != root.mount {
        return Ok(false);
    }
    Ok(current.device == root.device || subvolume_property(path)?.is_some())
}

/// The first directory at or under `path`, walking without following
/// symlinks, that is a mount point: `path` itself when it is mounted apart
/// from its parent, or a directory on another mount below it. Bind mounts of
/// the same filesystem keep st_dev, so the mount ID is what tells them apart.
pub(crate) fn foreign_mount(path: &Path) -> Result<Option<PathBuf>> {
    let mut root = identity(path)?.ok_or_else(|| anyhow!("missing tree at {}", path.display()))?;
    root.mount = mount_of(path, root);
    let parent = parent(path)?;
    let parent_identity =
        identity(parent)?.ok_or_else(|| anyhow!("storage unavailable at {}", parent.display()))?;
    if root.mount.is_none() || mount_of(parent, parent_identity) != root.mount {
        return Ok(Some(path.to_owned()));
    }
    fn walk(path: &Path, root: Identity) -> Result<Option<PathBuf>> {
        if !fs::symlink_metadata(path)?.is_dir() {
            return Ok(None);
        }
        if !same_tree_mount(root, path)? {
            return Ok(Some(path.to_owned()));
        }
        subvolume_property(path)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && let Some(found) = walk(&entry.path(), root)?
            {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }
    walk(path, root)
}

fn preflight_tree(path: &Path) -> Result<()> {
    if let Some(mount) = foreign_mount(path)? {
        bail!(
            "nested mount at {} is not owned by this trash entry",
            mount.display()
        );
    }
    Ok(())
}

fn check_tree_mounts(path: &Path) -> Result<()> {
    if let Some(mount) = foreign_mount(path)? {
        bail!(
            "nested mount at {} is not owned by this environment",
            mount.display()
        );
    }
    Ok(())
}

fn delete_subvolumes(path: &Path) -> Result<()> {
    if subvolume_property(path)? == Some(true) {
        let btrfs = crate::runner::find_on_path("btrfs")
            .ok_or_else(|| anyhow!("btrfs-progs unavailable"))?;
        let result = Command::new(btrfs)
            .args(["property", "set", "-ts"])
            .arg(path)
            .args(["ro", "false"])
            .output()?;
        if !result.status.success() {
            bail!(
                "cannot make Btrfs subvolume writable for deletion at {}: {}",
                path.display(),
                String::from_utf8_lossy(&result.stderr).trim()
            );
        }
        sync(path)?;
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            delete_subvolumes(&entry.path())?;
        }
    }
    if is_subvolume(path)? {
        fs::remove_dir_all(path)
            .with_context(|| format!("deleting Btrfs subvolume at {}", path.display()))?;
        sync(parent(path)?)?;
    }
    Ok(())
}

fn rename_owned(source: &Path, destination: &Path) -> Result<()> {
    renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        RenameFlags::NOREPLACE,
    )?;
    sync(parent(source)?)?;
    sync(parent(destination)?)
}

fn make_payload(
    paths: &Paths,
    kind: Kind,
    source: &Path,
    destination: &Path,
) -> Result<Option<Payload>> {
    let Some(owned) = identity(source)? else {
        return Ok(None);
    };
    create_storage(parent(destination)?)?;
    if identity(destination)?.is_some() {
        bail!("trash destination occupied: {}", destination.display());
    }
    let source_parent =
        identity(parent(source)?)?.ok_or_else(|| anyhow!("source storage unavailable"))?;
    let destination_parent =
        identity(parent(destination)?)?.ok_or_else(|| anyhow!("trash storage unavailable"))?;
    if owned.mount != source_parent.mount {
        bail!(
            "payload root is a separate mount at {}; use storage within its filesystem",
            source.display()
        );
    }
    if matches!(kind, Kind::Prefix | Kind::Snapshots)
        && (source_parent.mount != destination_parent.mount || source_parent.mount.is_none())
    {
        bail!(
            "large payload cannot be moved on its filesystem: {}",
            source.display()
        );
    }
    if fs::symlink_metadata(source)?.is_dir() {
        check_tree_mounts(source)?;
    }
    Ok(Some(Payload {
        kind,
        original: stored(paths, source)?,
        retained: stored(paths, destination)?,
        identity: owned,
        source_parent,
        destination_parent,
        copied_identity: None,
        returned_identity: None,
        out_stage: None,
        return_stage: None,
        digest: digest(source)?,
        step: Step::Ready,
        kept: false,
    }))
}

fn move_out(paths: &Paths, manifest: &mut Manifest, index: usize) -> Result<()> {
    let payload = manifest.payloads[index].clone();
    let source = resolved(paths, &payload.original)?;
    let destination = resolved(paths, &payload.retained)?;
    require(parent(&source)?, payload.source_parent)?;
    require(parent(&destination)?, payload.destination_parent)?;
    require(&source, payload.identity)?;
    let source_parent =
        identity(parent(&source)?)?.ok_or_else(|| anyhow!("source storage unavailable"))?;
    let destination_parent =
        identity(parent(&destination)?)?.ok_or_else(|| anyhow!("trash storage unavailable"))?;
    if !payload.source_parent.matches(source_parent)
        || !payload.destination_parent.matches(destination_parent)
    {
        bail!("payload storage changed while moving {}", source.display());
    }
    require_same_mount_as_parent(&source)?;
    if identity(&destination)?.is_some() {
        bail!("trash collision at {}", destination.display());
    }
    let source_mount = source_parent.mount;
    let destination_mount = destination_parent.mount;
    if matches!(payload.kind, Kind::Prefix | Kind::Snapshots)
        && (source_mount.is_none() || source_mount != destination_mount)
    {
        bail!(
            "large payload cannot be moved on its filesystem: {}",
            source.display()
        );
    }
    let copy = source_mount != destination_mount;
    manifest.payloads[index].source_parent.mount = source_mount;
    manifest.payloads[index].destination_parent.mount = destination_mount;
    manifest.payloads[index].step = if copy { Step::Copying } else { Step::Moving };
    write_manifest(paths, manifest)?;
    if copy {
        let stage = prepare_stage(paths, manifest, index, &destination, false)?;
        let stage_dir = stage_location(paths, &stage)?;
        let staged = stage_dir.join("payload");
        copy_verified(&source, &staged, &payload.digest)?;
        manifest.payloads[index].copied_identity = identity(&staged)?;
        failpoint("out.before_identity_write")?;
        write_manifest(paths, manifest)?;
        failpoint("out.after_identity_write")?;
        require(&stage_dir, stage.identity)?;
        rename_owned(&staged, &destination)?;
        failpoint("out.after_publish")?;
        discard_stage(paths, &stage)?;
        manifest.payloads[index].step = Step::Copied;
        manifest.payloads[index].out_stage = None;
        write_manifest(paths, manifest)?;
        manifest.payloads[index].step = Step::DeletingSource;
        write_manifest(paths, manifest)?;
        require(&source, payload.identity)?;
        if !digest_matches(&source, &payload.digest)? || !same_tree(&source, &destination)? {
            bail!(
                "source changed while copying metadata at {}",
                source.display()
            );
        }
        delete_owned(&source)?;
    } else {
        rename_owned(&source, &destination)?;
    }
    manifest.payloads[index].step = Step::Retained;
    write_manifest(paths, manifest)
}

/// Moves owned data into a unique entry. A failure after publication leaves a
/// `removing` record for explicit rollback; it never falls back to deletion.
pub fn remove(paths: &Paths, name: &str, include_prefix: bool) -> Result<Entry> {
    remove_with(
        paths,
        name,
        include_prefix,
        &crate::cancel::Token::new(),
        |_| {},
    )
}

/// What a removal is doing, for a frontend's status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveEvent {
    /// Wine is still up after a moment. The token can still cancel.
    Waiting,
    /// Data starts moving. The token is no longer checked.
    Moving,
}

pub fn remove_with(
    paths: &Paths,
    name: &str,
    include_prefix: bool,
    token: &crate::cancel::Token,
    events: impl FnMut(RemoveEvent),
) -> Result<Entry> {
    remove_with_force(paths, name, include_prefix, false, token, events)
}

/// `remove_with`, but with `force` an environment whose GOG game is
/// mid-update is removed too, leaving the folder's journal in place.
pub fn remove_with_force(
    paths: &Paths,
    name: &str,
    include_prefix: bool,
    force: bool,
    token: &crate::cancel::Token,
    mut events: impl FnMut(RemoveEvent),
) -> Result<Entry> {
    Environment::ensure_exists(paths, name)?;
    let initial = Environment::load(paths, name).ok();
    let initial_prefix = initial
        .as_ref()
        .and_then(|env| env.prefix(paths).ok())
        .filter(|prefix| paths.check_prefix(prefix).is_ok());
    let initial_resolved = initial_prefix.as_deref().map(paths::resolve);
    let locks = Locks::new(paths);
    let _operation = locks.acquire(
        Operation::Remove,
        name,
        &initial_prefix.as_deref().into_iter().collect::<Vec<_>>(),
    )?;
    // GOG games live in the host library, so a portable environment of the
    // same name never owns one.
    let gog = if paths.portable_root().is_none() {
        let strict = folder::may_track_game(paths, name);
        folder::lock_owned(paths, name, strict, &|| token.is_canceled())?
    } else {
        Vec::new()
    };
    let _config = locks.config(&paths.environment_config(name))?;
    let _art_config = locks.config(&paths.art_dir(name).join("art.toml"))?;
    Environment::ensure_exists(paths, name)?;
    let current = Environment::load(paths, name).ok();
    let prefix_path = current
        .as_ref()
        .and_then(|env| env.prefix(paths).ok())
        .filter(|prefix| paths.check_prefix(prefix).is_ok());
    if prefix_path.as_deref().map(paths::resolve) != initial_resolved {
        bail!("environment prefix changed while acquiring locks; retry");
    }
    let runtime = Runtime::new(paths, name);
    if runtime.is_running()? {
        bail!("{name} is running, stop it first");
    }
    match runtime.status() {
        Ok(status) if status.lost => {
            bail!("{name}'s last run was lost; clean it up with `wineo stop {name}` first")
        }
        Err(err) if include_prefix => {
            bail!(
                "{name}'s run state cannot be read; clean it up with `wineo stop {name}` first: {err}"
            )
        }
        _ => {}
    }
    runtime.check_resolved(name)?;
    if !force && gog.iter().any(|(_, state)| state.journal.is_some()) {
        return Err(FolderError::RemoveMidUpdate {
            name: name.to_owned(),
        }
        .into());
    }
    if include_prefix && let Some(env) = &current {
        match &env.config.kind {
            config::Kind::Wine(_) => {}
            config::Kind::Native => bail!("{name} has no prefix. Run wineo rm {name}"),
            config::Kind::Proxied(_) => return Err(env.config.no_prefix(name).into()),
        }
    }
    if include_prefix && prefix_path.is_none() {
        bail!("{name}'s config cannot identify its prefix; remove without --delete-prefix instead");
    }
    if include_prefix
        && let Some(raw) = &prefix_path
        && fs::symlink_metadata(raw).is_ok_and(|meta| meta.file_type().is_symlink())
    {
        bail!(
            "{} is a symlink, so its prefix was not moved. Run `wineo rm {name}` without --delete-prefix instead",
            raw.display()
        );
    }
    let prefix_path = prefix_path.map(|path| paths::resolve(&path));
    if include_prefix && let Some(prefix) = &prefix_path {
        for other in [
            paths.trash_dir(),
            paths.environment_config(name),
            paths.art_dir(name),
            paths.stats_log(name),
        ] {
            if prefix.starts_with(&other) || other.starts_with(prefix) {
                bail!(
                    "prefix overlaps Wineo trash or metadata storage: {}",
                    prefix.display()
                );
            }
        }
    }
    let prefix_present = prefix_path.as_deref().map(identity).transpose()?.flatten();
    if include_prefix && let (Some(prefix_path), Some(_)) = (&prefix_path, prefix_present) {
        if !prefix::holds_prefix(prefix_path) {
            bail!(
                "{} is not a wine prefix. Run `wineo rm {name}` without --delete-prefix after checking its contents",
                prefix_path.display()
            );
        }
        let sharing = environment::sharing_prefix(paths, name, prefix_path)?;
        // Registrations in other roots are not globally indexed. Host-wide
        // path locks coordinate active work there, not idle registrations.
        if !sharing.names.is_empty() {
            bail!(
                "prefix {} is also used by {}",
                prefix_path.display(),
                sharing.names.join(", ")
            );
        }
        if !sharing.unreadable.is_empty() {
            bail!(
                "{} can't be read, so it's unknown whether {} is in use",
                sharing.unreadable.join(", "),
                prefix_path.display()
            );
        }
        environment::check_nesting(paths, name, prefix_path)?;
        if let Ok(global) = GlobalConfig::load(paths) {
            current
                .as_ref()
                .unwrap()
                .wait_idle_for_remove(paths, &global, token, || events(RemoveEvent::Waiting))?;
        }
    }
    if token.is_canceled() {
        return Err(crate::error::Error::Canceled("removing environment").into());
    }
    events(RemoveEvent::Moving);
    // Released before the config goes, so a crash in between leaves an
    // environment whose folder has no owner, which claiming heals.
    for (lock, _) in &gog {
        lock.release()?;
    }
    failpoint("gog.after_release")?;
    create_storage(&paths.trash_dir())?;
    let stamp = jiff::Timestamp::now()
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string();
    let (id, dir) = loop {
        let random = tempfile::Builder::new()
            .prefix(".id-")
            .tempfile_in(paths.locks_dir())?;
        let token = Sha256::digest(random.path().as_os_str().as_encoded_bytes());
        let suffix = format!("{:08x}", u32::from_le_bytes(token[..4].try_into()?));
        let id = format!("{stamp}-{suffix}");
        let dir = entry_dir(paths, &id)?;
        match fs::create_dir(&dir) {
            Ok(()) => break (id, dir),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    };
    sync(&paths.trash_dir())?;
    let mut payloads = Vec::new();
    let config = paths.environment_config(name);
    let add = |payloads: &mut Vec<Payload>, kind, source: &Path, target: &Path| -> Result<()> {
        if let Some(payload) = make_payload(paths, kind, source, target)? {
            payloads.push(payload);
        }
        Ok(())
    };
    add(
        &mut payloads,
        Kind::Config,
        &config,
        &dir.join("config.toml"),
    )?;
    add(
        &mut payloads,
        Kind::Art,
        &paths.art_dir(name),
        &dir.join("art"),
    )?;
    add(
        &mut payloads,
        Kind::Stats,
        &paths.stats_log(name),
        &dir.join("stats.jsonl"),
    )?;
    let snapshots = prefix_path.as_ref().map(|p| paths.snapshot_root(p, name));
    if include_prefix && let Some(prefix_path) = &prefix_path {
        let container = parent(prefix_path)?.join(".wineo-trash").join(&id);
        add(
            &mut payloads,
            Kind::Prefix,
            prefix_path,
            &container.join("prefix"),
        )?;
        if let Some(snapshots) = &snapshots
            && let Some(snapshot_meta) = identity(snapshots)?
        {
            let snapshot_parent = parent(snapshots)?;
            let prefix_parent = parent(prefix_path)?;
            let prefix_storage =
                identity(prefix_parent)?.ok_or_else(|| anyhow!("prefix mount unavailable"))?;
            let base =
                if snapshot_meta.mount.is_some() && snapshot_meta.mount == prefix_storage.mount {
                    prefix_parent
                } else {
                    snapshot_parent
                };
            let target = base.join(".wineo-trash").join(&id).join("snapshots");
            add(&mut payloads, Kind::Snapshots, snapshots, &target)?;
        }
    }
    if !payloads.iter().any(|p| p.kind == Kind::Config) {
        bail!("config disappeared; nothing was removed");
    }
    let mut manifest = Manifest {
        version: VERSION,
        id,
        entry_identity: identity(&dir)?.ok_or_else(|| anyhow!("trash entry disappeared"))?,
        name: name.to_owned(),
        removed_at: jiff::Timestamp::now().to_string(),
        include_prefix,
        prefix_present: prefix_path.as_ref().map(|_| prefix_present.is_some()),
        prefix_reference: prefix_path
            .as_deref()
            .map(|p| stored(paths, p))
            .transpose()?,
        prefix_identity: prefix_present,
        phase: Phase::Removing,
        payloads,
        gog_folders: gog
            .iter()
            .filter(|(_, state)| {
                current
                    .as_ref()
                    .and_then(|env| env.config.gog_id.as_deref())
                    .is_some_and(|gog_id| state.is_game(gog_id))
            })
            .map(|(_, state)| state.folder.clone())
            .collect(),
    };
    start_marker(paths, &manifest, Intent::Remove)?;
    write_manifest(paths, &manifest)?;
    failpoint("remove.after_manifest")?;
    for index in 0..manifest.payloads.len() {
        move_out(paths, &mut manifest, index)?;
    }
    let desktop_removed = desktop::remove_all(paths, name)?.len();
    if runtime.dir().exists() {
        delete_owned(runtime.dir())?;
    }
    let import_log = paths.import_log(name);
    if fs::symlink_metadata(&import_log).is_ok() {
        fs::remove_file(&import_log)?;
        sync(parent(&import_log)?)?;
    }
    manifest.phase = Phase::Trashed;
    write_manifest(paths, &manifest)?;
    finish_marker(paths, &manifest)?;
    retire_marker(paths, &manifest)?;
    let mut entry = info(paths, &manifest)?;
    entry.desktop_removed = desktop_removed;
    Ok(entry)
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

fn verify_parents(paths: &Paths, payload: &Payload) -> Result<(PathBuf, PathBuf)> {
    let original = resolved(paths, &payload.original)?;
    let retained = resolved(paths, &payload.retained)?;
    require(parent(&original)?, payload.source_parent)?;
    require(parent(&retained)?, payload.destination_parent)?;
    Ok((original, retained))
}

fn retained_identity(payload: &Payload) -> Identity {
    payload.copied_identity.unwrap_or(payload.identity)
}

fn verified_retained(path: &Path, payload: &Payload) -> Result<()> {
    // A Btrfs subvolume can change st_dev relative to its parent while a
    // same-mount rename keeps the subvolume inode. Only an actual metadata
    // copy receives a distinct recorded identity.
    require(path, retained_identity(payload))?;
    require_same_mount_as_parent(path)?;
    if !digest_matches(path, &payload.digest)? {
        bail!("retained payload changed at {}", path.display());
    }
    Ok(())
}

fn return_payload(
    paths: &Paths,
    manifest: &mut Manifest,
    index: usize,
    rollback: bool,
) -> Result<()> {
    let payload = manifest.payloads[index].clone();
    let (original, retained) = verify_parents(paths, &payload)?;
    let source = identity(&original)?;
    let saved = identity(&retained)?;
    if let Some(stage) = &payload.out_stage {
        discard_stage(paths, stage)?;
        manifest.payloads[index].out_stage = None;
        write_manifest(paths, manifest)?;
    }
    if let Some(stage) = &payload.return_stage {
        discard_stage(paths, stage)?;
        manifest.payloads[index].return_stage = None;
        write_manifest(paths, manifest)?;
    }
    if rollback && matches_identity(source, payload.identity) {
        require_same_mount_as_parent(&original)?;
        // Removal may have stopped before its move, or a copied metadata
        // payload may still exist on both sides. A whole source is
        // authoritative.
        if let Some(remaining) = saved {
            if !payload
                .copied_identity
                .is_some_and(|copied| copied.matches(remaining))
            {
                bail!("retained copy changed at {}", retained.display());
            }
            verified_retained(&retained, &payload)?;
            if !digest_matches(&original, &payload.digest)? || !same_tree(&original, &retained)? {
                // Deleting a copied source can stop part way, leaving its
                // root behind. Only the retained copy is whole then.
                if payload.step != Step::DeletingSource || !contained_in(&original, &retained)? {
                    // Changed before its deletion began, so neither copy
                    // is partial and both are kept.
                    manifest.payloads[index].kept = true;
                    return write_manifest(paths, manifest);
                }
                delete_owned(&original)?;
                return return_payload(paths, manifest, index, rollback);
            }
            delete_owned(&retained)?;
        }
        manifest.payloads[index].step = Step::Returned;
        return write_manifest(paths, manifest);
    }
    if rollback && source.is_some() && payload.step == Step::Deleting {
        // Dropping the copy of an identical replacement was recorded before
        // an interruption. The replacement stays; only the copy goes.
        require_same_mount_as_parent(&original)?;
        if saved.is_some() {
            require(&retained, retained_identity(&payload))?;
            delete_owned(&retained)?;
        }
        manifest.payloads[index].step = Step::Returned;
        return write_manifest(paths, manifest);
    }
    if rollback
        && source.is_some()
        && let Some(remaining) = saved
        && payload
            .copied_identity
            .is_some_and(|copied| copied.matches(remaining))
    {
        // A copied source replaced since, like an editor's save through a
        // rename, is the user's newer version. It is never deleted.
        require_same_mount_as_parent(&original)?;
        verified_retained(&retained, &payload)?;
        if digest_matches(&original, &payload.digest)? && same_tree(&original, &retained)? {
            manifest.payloads[index].step = Step::Deleting;
            write_manifest(paths, manifest)?;
            delete_owned(&retained)?;
            failpoint("rollback.after_dropping_copy")?;
            manifest.payloads[index].step = Step::Returned;
        } else {
            manifest.payloads[index].kept = true;
        }
        return write_manifest(paths, manifest);
    }
    if let Some(returned) = payload.returned_identity
        && matches_identity(source, returned)
    {
        require(&original, returned)?;
        require_same_mount_as_parent(&original)?;
        let whole = digest_matches(&original, &payload.digest)?
            && (saved.is_none() || same_tree(&original, &retained)?);
        if !whole {
            // A crash during the reserved copy left a partial result. Its
            // identity proves it is our staging, not a new collision.
            if saved.is_none() {
                bail!("retained copy missing at {}", retained.display());
            }
            verified_retained(&retained, &payload)?;
            delete_owned(&original)?;
            manifest.payloads[index].returned_identity = None;
            write_manifest(paths, manifest)?;
            return return_payload(paths, manifest, index, rollback);
        }
        if let Some(found) = saved {
            require(&retained, retained_identity(&payload))?;
            require(&retained, found)?;
            verified_retained(&retained, &payload)?;
            delete_owned(&retained)?;
        }
        manifest.payloads[index].step = Step::Returned;
        return write_manifest(paths, manifest);
    }
    if !rollback && matches_identity(source, retained_identity(&payload)) && saved.is_none() {
        require_same_mount_as_parent(&original)?;
        manifest.payloads[index].step = Step::Returned;
        return write_manifest(paths, manifest);
    }
    if source.is_some() {
        // During restoration a cross-device copy may have reached the source
        // before its completion record. A matching digest is not an ownership
        // proof; stop rather than remove the last authoritative retained copy.
        bail!(
            "restore collision at {}; retained copy stays at {}",
            original.display(),
            retained.display()
        );
    }
    if saved.is_none() {
        bail!(
            "payload missing at both {} and {}",
            original.display(),
            retained.display()
        );
    }
    verified_retained(&retained, &payload)?;
    let source_mount = identity(parent(&original)?)?.and_then(|id| id.mount);
    let destination_mount = identity(parent(&retained)?)?.and_then(|id| id.mount);
    if matches!(payload.kind, Kind::Prefix | Kind::Snapshots)
        && (source_mount.is_none() || source_mount != destination_mount)
    {
        bail!(
            "large payload cannot be moved on its filesystem: {}",
            retained.display()
        );
    }
    manifest.payloads[index].step = Step::Returning;
    write_manifest(paths, manifest)?;
    if destination_mount == source_mount {
        rename_owned(&retained, &original)?;
        require(&original, retained_identity(&payload))?;
    } else {
        let stage = prepare_stage(paths, manifest, index, &original, true)?;
        let stage_dir = stage_location(paths, &stage)?;
        let staged = stage_dir.join("payload");
        copy_verified(&retained, &staged, &payload.digest)?;
        manifest.payloads[index].returned_identity = identity(&staged)?;
        failpoint("return.before_identity_write")?;
        write_manifest(paths, manifest)?;
        failpoint("return.after_identity_write")?;
        require(&stage_dir, stage.identity)?;
        rename_owned(&staged, &original)?;
        failpoint("return.after_publish")?;
        discard_stage(paths, &stage)?;
        manifest.payloads[index].return_stage = None;
        manifest.payloads[index].step = Step::Copied;
        write_manifest(paths, manifest)?;
        // Keep the retained copy until the returned one has durable identity
        // and holds exactly what the retained copy does.
        #[cfg(test)]
        BEFORE_RETAINED_DROP.with(|hook| {
            if let Some(hook) = hook.take() {
                hook(&original, &retained);
            }
        });
        verified_retained(&retained, &payload)?;
        let returned = manifest.payloads[index]
            .returned_identity
            .ok_or_else(|| anyhow!("returned copy has no recorded identity"))?;
        require(&original, returned)?;
        if !same_tree(&original, &retained)? {
            bail!(
                "{} differs from its retained copy at {}; both were kept",
                original.display(),
                retained.display()
            );
        }
        delete_owned(&retained)?;
    }
    manifest.payloads[index].step = Step::Returned;
    write_manifest(paths, manifest)
}

fn reconcile(paths: &Paths, name: &str) -> Result<()> {
    let path = paths.environment_config(name);
    let config = match fs::read_to_string(&path)
        .ok()
        .and_then(|text| EnvironmentConfig::parse(&path, &text).ok())
    {
        Some(config) => config,
        None => return Ok(()), // Broken raw configs stay registered without shortcuts.
    };
    let environment = Environment {
        name: name.to_owned(),
        path,
        config,
    };
    if desktop::supported(paths).is_ok() {
        let owned = desktop::list(paths)?;
        for target in desktop::requested_targets(&environment) {
            let shortcut = paths
                .applications_dir()
                .join(format!("wineo-{name}-{target}.desktop"));
            if fs::symlink_metadata(&shortcut).is_ok()
                && !owned.iter().any(|entry| {
                    entry.path == shortcut && entry.environment == name && entry.target == *target
                })
            {
                bail!(
                    "unrelated desktop shortcut at {} was not overwritten",
                    shortcut.display()
                );
            }
        }
    }
    let global = GlobalConfig::load_or_default(paths)?;
    let changes = match &environment.config.kind {
        config::Kind::Native => desktop::reconcile_native(paths, &global, &environment)?,
        config::Kind::Proxied(_) => desktop::reconcile_proxied(paths, &global, &environment)?,
        config::Kind::Wine(_) => desktop::reconcile(
            paths,
            &global,
            &environment,
            None,
            &environment.prefix(paths)?,
        )?,
    };
    for change in changes {
        if let desktop::Change::Skipped { target, reason } = change {
            bail!("environment restored, but desktop entry {target} was not created: {reason}");
        }
    }
    Ok(())
}

fn finish_entry(paths: &Paths, manifest: &Manifest) -> Result<()> {
    for payload in &manifest.payloads {
        if matches!(payload.kind, Kind::Prefix | Kind::Snapshots) {
            let retained = resolved(paths, &payload.retained)?;
            let container = parent(&retained)?;
            match fs::remove_dir(container) {
                Ok(()) => sync(parent(container)?)?,
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) if err.kind() == io::ErrorKind::DirectoryNotEmpty => {
                    // Another payload may share this container. It must be
                    // empty by the time the last payload has been processed.
                    if manifest
                        .payloads
                        .iter()
                        .all(|item| item.step == Step::Deleted || item.step == Step::Returned)
                    {
                        bail!(
                            "trash container contains unexpected data: {}",
                            container.display()
                        );
                    }
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
    let dir = entry_dir(paths, &manifest.id)?;
    let file = dir.join(MANIFEST);
    if fs::read_dir(&dir)?.count() != 1 {
        bail!("trash entry contains unexpected data at {}", dir.display());
    }
    fs::remove_file(&file)?;
    sync(&dir)?;
    fs::remove_dir(&dir)?;
    sync(&paths.trash_dir())
}

fn complete_entry(paths: &Paths, manifest: &Manifest) -> Result<()> {
    let marked = marker_for(paths, &manifest.id)?.is_some();
    if marked {
        finish_marker(paths, manifest)?;
    }
    finish_entry(paths, manifest)?;
    if marked {
        retire_marker(paths, manifest)?;
    }
    Ok(())
}

fn restore_locked(
    paths: &Paths,
    manifest: &mut Manifest,
    rollback: bool,
    gog: &[FolderLock],
    notes: &mut Vec<String>,
) -> Result<()> {
    restore_locked_noting(paths, manifest, rollback, gog, || Ok(()), notes)
}

#[cfg(test)]
fn restore_locked_with(
    paths: &Paths,
    manifest: &mut Manifest,
    rollback: bool,
    gog: &[FolderLock],
    published: impl FnOnce() -> Result<()>,
) -> Result<()> {
    restore_locked_noting(paths, manifest, rollback, gog, published, &mut Vec::new())
}

fn restore_locked_noting(
    paths: &Paths,
    manifest: &mut Manifest,
    rollback: bool,
    gog: &[FolderLock],
    published: impl FnOnce() -> Result<()>,
    notes: &mut Vec<String>,
) -> Result<()> {
    let config = paths.environment_config(&manifest.name);
    if !rollback && manifest.phase == Phase::Trashed {
        if identity(&config)?.is_some() {
            bail!("environment `{}` already exists", manifest.name);
        }
        for lock in gog {
            if let Some(state) = lock.read()?
                && let Some(owner) = state.owner
                && owner != manifest.name
            {
                return Err(FolderError::OwnedByOther {
                    folder: state.folder,
                    owner,
                }
                .into());
            }
        }
        if manifest.prefix_present == Some(true) && !manifest.include_prefix {
            let original = resolved(
                paths,
                manifest
                    .prefix_reference
                    .as_deref()
                    .ok_or_else(|| anyhow!("missing prefix reference"))?,
            )?;
            let expected = manifest
                .prefix_identity
                .ok_or_else(|| anyhow!("missing prefix identity"))?;
            if expected.birth.is_none() {
                bail!(
                    "retained prefix identity cannot be established at {}",
                    original.display()
                );
            }
            require(&original, expected)?;
        }
        for payload in &manifest.payloads {
            let (original, retained) = verify_parents(paths, payload)?;
            if identity(&original)?.is_some() {
                bail!("restore collision at {}", original.display());
            }
            verified_retained(&retained, payload)?;
        }
        check_restored_prefix(paths, manifest)?;
        start_marker(paths, manifest, Intent::Restore)?;
        manifest.phase = Phase::Restoring;
        write_manifest(paths, manifest)?;
    }
    if !rollback && manifest.phase != Phase::Restoring {
        bail!("trash entry is not restorable");
    }
    if rollback && manifest.phase != Phase::Removing {
        bail!("trash entry is not an interrupted removal");
    }
    if !rollback && manifest.prefix_present == Some(true) && !manifest.include_prefix {
        let prefix = resolved(paths, manifest.prefix_reference.as_deref().unwrap())?;
        let expected = manifest.prefix_identity.unwrap();
        if expected.birth.is_none() {
            bail!(
                "retained prefix identity cannot be established at {}",
                prefix.display()
            );
        }
        require(&prefix, expected)?;
    }
    // Recovery must finish even when build discovery is unavailable.
    let restored_config = return_config(paths, manifest).ok().flatten();
    let global = restored_config
        .as_ref()
        .and_then(EnvironmentConfig::wine)
        .and_then(|_| GlobalConfig::load(paths).ok());
    with_environment_build_locks(paths, global.as_ref(), restored_config.as_ref(), || {
        for index in (0..manifest.payloads.len()).rev() {
            if manifest.payloads[index].step == Step::Returned || manifest.payloads[index].kept {
                continue;
            }
            return_payload(paths, manifest, index, rollback)?;
        }
        published()
    })?;
    // The config is back first, so a crash before this leaves an environment
    // whose folder has no owner, which claiming heals.
    failpoint("gog.before_reclaim")?;
    reclaim_gog(paths, manifest, gog)?;
    // Registration and payload restoration have completed. A shortcut failure
    // is reported, but it must not undo or block the returned environment.
    let desktop_error = reconcile(paths, &manifest.name).err();
    if manifest.payloads.iter().any(|payload| payload.kept) {
        keep_changed(paths, manifest, notes)?;
    } else {
        complete_entry(paths, manifest)?;
    }
    if let Some(err) = desktop_error {
        bail!(
            "environment {} was restored, but its desktop entries need attention: {err:#}",
            manifest.name
        );
    }
    Ok(())
}

/// Ends a rollback that kept a changed source live: the retained copies from
/// before the change become an ordinary trash entry, with a copy of the live
/// config so the entry can be restored once the live environment is gone.
fn keep_changed(paths: &Paths, manifest: &mut Manifest, notes: &mut Vec<String>) -> Result<()> {
    let dir = entry_dir(paths, &manifest.id)?;
    let mut kept: Vec<Payload> = manifest
        .payloads
        .iter()
        .filter(|payload| payload.kept)
        .cloned()
        .collect();
    for payload in &kept {
        notes.push(format!(
            "{} changed after it was copied to the trash, so it stays as it is; the copy from before is in trash entry {}",
            resolved(paths, &payload.original)?.display(),
            manifest.id
        ));
    }
    if !kept.iter().any(|payload| payload.kind == Kind::Config) {
        let source = paths.environment_config(&manifest.name);
        let retained = dir.join("config.toml");
        // The config payload has already gone back, so only a copy this step
        // started before a crash can be here.
        if identity(&retained)?.is_some() {
            fs::remove_file(&retained)?;
        }
        let source_parent =
            identity(parent(&source)?)?.ok_or_else(|| anyhow!("config storage unavailable"))?;
        let destination_parent =
            identity(&dir)?.ok_or_else(|| anyhow!("trash storage unavailable"))?;
        let digest = digest(&source)?;
        copy_verified(&source, &retained, &digest)?;
        let copied = identity(&retained)?.ok_or_else(|| anyhow!("config copy disappeared"))?;
        let cross = source_parent.mount != destination_parent.mount;
        let identity = if cross {
            identity(&source)?.ok_or_else(|| anyhow!("config disappeared"))?
        } else {
            copied
        };
        kept.insert(
            0,
            Payload {
                kind: Kind::Config,
                original: stored(paths, &source)?,
                retained: stored(paths, &retained)?,
                identity,
                source_parent,
                destination_parent,
                copied_identity: cross.then_some(copied),
                returned_identity: None,
                out_stage: None,
                return_stage: None,
                digest,
                step: Step::Retained,
                kept: false,
            },
        );
    }
    for payload in &mut kept {
        payload.kept = false;
        payload.step = Step::Retained;
        payload.returned_identity = None;
        payload.out_stage = None;
        payload.return_stage = None;
    }
    manifest.payloads = kept;
    manifest.include_prefix = false;
    manifest.prefix_present = None;
    manifest.prefix_reference = None;
    manifest.prefix_identity = None;
    manifest.gog_folders.clear();
    manifest.phase = Phase::Trashed;
    write_manifest(paths, manifest)?;
    if marker_for(paths, &manifest.id)?.is_some() {
        finish_marker(paths, manifest)?;
        retire_marker(paths, manifest)?;
    }
    Ok(())
}

/// Claims the released folders again for the returned config's game. A
/// folder another environment took meanwhile is left to `validate`, so
/// recovery still finishes.
fn reclaim_gog(paths: &Paths, manifest: &Manifest, gog: &[FolderLock]) -> Result<()> {
    if gog.is_empty() {
        return Ok(());
    }
    let path = paths.environment_config(&manifest.name);
    let Some(gog_id) = fs::read_to_string(&path)
        .ok()
        .and_then(|text| EnvironmentConfig::parse(&path, &text).ok())
        .and_then(|config| config.gog_id)
    else {
        return Ok(());
    };
    for lock in gog {
        match lock.read()? {
            Some(state) if state.is_game(&gog_id) => {}
            _ => continue,
        }
        if let Err(err) = lock.reclaim(&manifest.name)
            && !matches!(
                err.downcast_ref::<FolderError>(),
                Some(FolderError::OwnedByOther { .. })
            )
        {
            return Err(err);
        }
    }
    Ok(())
}

/// Applies the rules that registering this prefix anew would: it may not
/// nest with another environment's, and a prefix coming back from the trash
/// may not land where another environment already points.
fn check_restored_prefix(paths: &Paths, manifest: &Manifest) -> Result<()> {
    // A hand-edited config that can't be parsed stays restorable, as before,
    // so there is no prefix to check.
    let Some(config) = return_config(paths, manifest)? else {
        return Ok(());
    };
    let Some(wine) = config.wine() else {
        return Ok(());
    };
    let prefix = paths.config_path(&wine.prefix)?;
    let returning = manifest
        .payloads
        .iter()
        .any(|payload| payload.kind == Kind::Prefix);
    if returning {
        environment::check_destination(paths, &manifest.name, &prefix)?;
    } else {
        environment::check_nesting(paths, &manifest.name, &prefix)?;
    }
    Ok(())
}

fn return_config(paths: &Paths, manifest: &Manifest) -> Result<Option<EnvironmentConfig>> {
    let payload = manifest
        .payloads
        .iter()
        .find(|payload| payload.kind == Kind::Config)
        .ok_or_else(|| anyhow!("trash entry has no config payload"))?;
    let original = resolved(paths, &payload.original)?;
    let retained = resolved(paths, &payload.retained)?;
    let mut original_identities = vec![payload.identity, retained_identity(payload)];
    if let Some(returned) = payload.returned_identity {
        original_identities.push(returned);
    }
    let candidates = [
        (&original, original_identities),
        (&retained, vec![retained_identity(payload)]),
    ];
    for (path, identities) in candidates {
        let Some(found) = identity(path)? else {
            continue;
        };
        if !identities.iter().any(|saved| saved.matches(found))
            || !digest_matches(path, &payload.digest)?
        {
            continue;
        }
        // Hand-edited configs must remain restorable even when they cannot be parsed.
        return Ok(String::from_utf8(fs::read(path)?)
            .ok()
            .and_then(|text| EnvironmentConfig::parse(&original, &text).ok()));
    }
    bail!("config payload unavailable in trash entry {}", manifest.id)
}

fn with_environment_build_locks<T>(
    paths: &Paths,
    global: Option<&GlobalConfig>,
    config: Option<&EnvironmentConfig>,
    publish: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let _guards = match (global, config.and_then(EnvironmentConfig::wine)) {
        (Some(global), Some(wine)) => build_lock::wine(paths, global, wine).unwrap_or_default(),
        _ => Vec::new(),
    };
    publish()
}

pub fn restore(paths: &Paths, id: &str) -> Result<()> {
    locked(paths, id, true, |manifest, gog| {
        if manifest.phase != Phase::Trashed {
            bail!("entry {id} needs `wineo validate --fix` first");
        }
        restore_locked(paths, manifest, false, gog, &mut Vec::new())
    })
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
