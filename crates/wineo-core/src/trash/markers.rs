//! Pending markers, which outlive a lost trash entry so its work can be recovered.

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

use crate::paths::{self, Paths};

use super::identity::{create_storage, identity, parent, require, sync};
use super::{
    Identity, Intent, Kind, Manifest, PENDING_DIR, PendingEntry, PendingMarker, VERSION, failpoint,
    id_valid, matches_identity,
};

fn marker_root(paths: &Paths) -> Result<(PathBuf, Option<Identity>)> {
    if let Some(root) = paths.portable_root() {
        return Ok((root.to_owned(), identity(root)?));
    }
    Ok((parent(&paths.environments_dir())?.to_owned(), None))
}

pub(super) fn marker_dir(paths: &Paths) -> PathBuf {
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

pub(super) fn markers(paths: &Paths) -> Result<Vec<(PathBuf, PendingMarker)>> {
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

pub(super) fn marker_matches(paths: &Paths, marker: &PendingMarker) -> Result<bool> {
    let (root, physical) = marker_root(paths)?;
    Ok(match (marker.portable_identity, physical) {
        (Some(saved), Some(current)) => saved.matches(current) || marker.root == root,
        (Some(_), None) => marker.root == root,
        (None, None) => paths.portable_root().is_none() && marker.root == root,
        _ => false,
    })
}

pub(super) fn marker_paths(paths: &Paths, marker: &PendingMarker) -> Result<Vec<PathBuf>> {
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

pub(super) fn marker_paths_at(marker: &PendingMarker, root: &Path) -> Result<Vec<PathBuf>> {
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

pub(super) fn relocated_marker_root(
    marker: &PendingMarker,
    candidate: &Path,
) -> Result<Option<PathBuf>> {
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

pub(super) fn path_overlap(candidate: &Path, location: &Path) -> bool {
    candidate.starts_with(location)
        || location.starts_with(candidate)
        || paths::resolve(candidate).starts_with(paths::resolve(location))
        || paths::resolve(location).starts_with(paths::resolve(candidate))
}

pub(super) fn marker_for(paths: &Paths, id: &str) -> Result<Option<(PathBuf, PendingMarker)>> {
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

pub(super) fn publish_marker(path: &Path, marker: &PendingMarker) -> Result<()> {
    create_storage(parent(path)?)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent(path)?)?;
    temp.write_all(toml::to_string_pretty(marker)?.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|err| anyhow!(err.error))?;
    sync(parent(path)?)
}

pub(super) fn start_marker(paths: &Paths, manifest: &Manifest, intent: Intent) -> Result<()> {
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

pub(super) fn finish_marker(paths: &Paths, manifest: &Manifest) -> Result<()> {
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

pub(super) fn retire_marker(paths: &Paths, manifest: &Manifest) -> Result<()> {
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
