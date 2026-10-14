//! Durable manifest writes and the checks every manifest read passes.

use std::fs;
use std::io::Write;

use anyhow::{Result, anyhow, bail};

use crate::config::{self, EnvironmentConfig};
use crate::name;
use crate::paths::{self, Paths};

use super::digest::digest_matches;
use super::identity::{identity, parent, sync};
use super::payload::retained_identity;
use super::{
    Kind, MANIFEST, Manifest, Phase, Step, VERSION, entry_dir, matches_identity, resolved,
};

pub(super) fn write_manifest(paths: &Paths, manifest: &Manifest) -> Result<()> {
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

pub(super) fn read_manifest(paths: &Paths, id: &str) -> Result<Manifest> {
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
