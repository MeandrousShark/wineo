//! Source tree walks: fingerprints, external links, and the progress they report.

use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::cancel::Token;

use super::copy::resolve_link;
use super::{ExternalDependency, ImportProgress, SourceFingerprint, check_cancelled};

fn external_dependency(
    root: &Path,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<Option<ExternalDependency>> {
    if !metadata.file_type().is_symlink() {
        return Ok(None);
    }
    let target =
        fs::read_link(path).with_context(|| format!("failed to read link {}", path.display()))?;
    let resolved = resolve_link(path, &target);
    Ok((!resolved.starts_with(root)).then(|| ExternalDependency {
        source: path.to_owned(),
        target: resolved,
    }))
}

const CHECK_INTERVAL: u64 = 1000;

/// Counts entries during a pre-copy walk so a large source reports progress
/// and can be canceled instead of looking stuck.
pub(super) struct Check<'a> {
    cancel: &'a Token,
    progress: &'a mut dyn FnMut(ImportProgress),
    entries: u64,
}

impl<'a> Check<'a> {
    pub(super) fn new(cancel: &'a Token, progress: &'a mut dyn FnMut(ImportProgress)) -> Self {
        Self {
            cancel,
            progress,
            entries: 0,
        }
    }

    fn tick(&mut self) -> Result<()> {
        check_cancelled(self.cancel)?;
        self.entries += 1;
        if self.entries.is_multiple_of(CHECK_INTERVAL) {
            self.report()?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.report()
    }

    // A cancel can arrive during the callback itself, and the last one of a
    // walk has no later entry to notice it before the caller publishes.
    fn report(&mut self) -> Result<()> {
        (self.progress)(ImportProgress::Checking {
            entries: self.entries,
        });
        check_cancelled(self.cancel)
    }
}

/// Which fingerprint a caller compares; see `SourceFingerprint`.
#[derive(Debug, Clone, Copy)]
pub(super) enum FingerprintKind {
    Content,
    Metadata,
}

/// External links and the fingerprint of `root`, found in one walk.
pub(super) fn scan(
    root: &Path,
    kind: FingerprintKind,
    check: &mut Check<'_>,
) -> Result<(Vec<ExternalDependency>, SourceFingerprint)> {
    let mut found = Vec::new();
    let fingerprint = fingerprint_walk(root, kind, check, Some(&mut found))?;
    found.sort_by(|a, b| a.source.cmp(&b.source));
    Ok((found, fingerprint))
}

/// The content fingerprint snapshot, clone, and portable compare against.
pub(crate) fn fingerprint(root: &Path) -> Result<SourceFingerprint> {
    fingerprint_with(root, &Token::new(), |_| {})
}

pub(crate) fn fingerprint_with(
    root: &Path,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<SourceFingerprint> {
    check_cancelled(cancel)?;
    fingerprint_walk(
        root,
        FingerprintKind::Content,
        &mut Check::new(cancel, &mut progress),
        None,
    )
}

pub(super) fn import_fingerprint(root: &Path, check: &mut Check<'_>) -> Result<SourceFingerprint> {
    fingerprint_walk(root, FingerprintKind::Metadata, check, None)
}

fn fingerprint_walk(
    root: &Path,
    kind: FingerprintKind,
    check: &mut Check<'_>,
    external: Option<&mut Vec<ExternalDependency>>,
) -> Result<SourceFingerprint> {
    match kind {
        FingerprintKind::Content => {
            let cancel = check.cancel;
            hash_records(root, check, external, |relative, path, metadata| {
                content_record(relative, path, metadata, cancel)
            })
        }
        FingerprintKind::Metadata => hash_records(root, check, external, metadata_record),
    }
}

type ContentRecord = (
    String,
    u8,
    u64,
    u32,
    u64,
    Option<(u64, u32)>,
    Option<u64>,
    Option<String>,
);

// This tuple must hash exactly as before, since restore markers persist it.
fn content_record(
    relative: String,
    path: &Path,
    metadata: &fs::Metadata,
    cancel: &Token,
) -> Result<ContentRecord> {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| (duration.as_secs(), duration.subsec_nanos()));
    let content = if metadata.file_type().is_file() {
        Some(file_digest(path, cancel)?)
    } else {
        None
    };
    Ok((
        relative,
        file_kind(metadata),
        metadata.len(),
        metadata.mode(),
        metadata.nlink(),
        modified,
        content,
        link_target(path, metadata)?,
    ))
}

type MetadataRecord = (
    String,
    u8,
    u64,
    u32,
    u64,
    (i64, i64),
    (i64, i64),
    u64,
    Option<String>,
);

// ctime moves on every write, so a same-size rewrite still changes this.
fn metadata_record(
    relative: String,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<MetadataRecord> {
    Ok((
        relative,
        file_kind(metadata),
        metadata.len(),
        metadata.mode(),
        metadata.nlink(),
        (metadata.mtime(), metadata.mtime_nsec()),
        (metadata.ctime(), metadata.ctime_nsec()),
        metadata.ino(),
        link_target(path, metadata)?,
    ))
}

fn link_target(path: &Path, metadata: &fs::Metadata) -> Result<Option<String>> {
    if !metadata.file_type().is_symlink() {
        return Ok(None);
    }
    let target =
        fs::read_link(path).with_context(|| format!("failed to read link {}", path.display()))?;
    Ok(Some(target.to_string_lossy().into_owned()))
}

fn hash_records<R: Hash>(
    root: &Path,
    check: &mut Check<'_>,
    mut external: Option<&mut Vec<ExternalDependency>>,
    record: impl Fn(String, &Path, &fs::Metadata) -> Result<R>,
) -> Result<SourceFingerprint> {
    let mut records: Vec<(String, R)> = Vec::new();
    let mut entries = 0;
    walk_no_follow(root, &mut |path, metadata| {
        check.tick()?;
        if let Some(found) = external.as_deref_mut()
            && let Some(dependency) = external_dependency(root, path, metadata)?
        {
            found.push(dependency);
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        records.push((relative.clone(), record(relative, path, metadata)?));
        entries += 1;
        Ok(())
    })?;
    check.finish()?;
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let records: Vec<R> = records.into_iter().map(|(_, record)| record).collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    records.hash(&mut hasher);
    Ok(SourceFingerprint {
        hash: hasher.finish(),
        entries,
    })
}

fn file_digest(path: &Path, cancel: &Token) -> Result<u64> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to read {} for fingerprinting", path.display()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut buffer = [0; 128 * 1024];
    loop {
        check_cancelled(cancel)?;
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read {} for fingerprinting", path.display()))?;
        if count == 0 {
            break;
        }
        buffer[..count].hash(&mut hasher);
    }
    Ok(hasher.finish())
}

fn file_kind(metadata: &fs::Metadata) -> u8 {
    let kind = metadata.file_type();
    if kind.is_dir() {
        1
    } else if kind.is_file() {
        2
    } else if kind.is_symlink() {
        3
    } else {
        4
    }
}

pub(super) fn walk_no_follow(
    root: &Path,
    callback: &mut impl FnMut(&Path, &fs::Metadata) -> Result<()>,
) -> Result<()> {
    let metadata = fs::symlink_metadata(root)
        .with_context(|| format!("failed to inspect {}", root.display()))?;
    callback(root, &metadata)?;
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut children: Vec<PathBuf> = fs::read_dir(root)
        .with_context(|| format!("failed to read {}", root.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<_>>()
        .with_context(|| format!("failed to read {}", root.display()))?;
    children.sort();
    for child in children {
        walk_no_follow(&child, callback)?;
    }
    Ok(())
}

/// Links under `root` that point outside it and outside every tree in `moved`.
pub(crate) fn external_links_with(
    root: &Path,
    moved: &[&Path],
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<Vec<ExternalDependency>> {
    let mut check = Check::new(cancel, progress);
    let mut found = Vec::new();
    walk_no_follow(root, &mut |path, metadata| {
        check.tick()?;
        if let Some(link) = external_dependency(root, path, metadata)?
            && !moved.iter().any(|tree| link.target.starts_with(tree))
        {
            found.push(link);
        }
        Ok(())
    })?;
    check.finish()?;
    found.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(found)
}
