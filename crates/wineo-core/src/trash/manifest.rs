//! Durable manifest writes and the checks every manifest read passes.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::config::{self, EnvironmentConfig};
use crate::name;
use crate::paths::{self, Paths};

use super::digest::digest_matches;
use super::identity::{identity, parent, sync};
use super::markers::path_overlap;
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

/// A trash entry whose manifest fails its checks. When the manifest parses
/// and names a valid environment, only that environment is blocked; otherwise
/// nothing says whose it is, so it blocks every environment.
#[derive(Debug)]
pub struct Unverifiable {
    pub id: String,
    pub dir: PathBuf,
    /// The environment it blocks, or None for every environment.
    pub blocks: Option<String>,
    pub reason: String,
    pub(super) manifest: Option<Box<Manifest>>,
}

impl fmt::Display for Unverifiable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "trash entry {} is unreadable ({}) and blocks {}; see `wineo validate`",
            self.dir.display(),
            self.reason,
            self.scope()
        )
    }
}

impl std::error::Error for Unverifiable {}

impl Unverifiable {
    /// Who this entry blocks, as a message would say it.
    pub fn scope(&self) -> &str {
        self.blocks.as_deref().unwrap_or("every environment")
    }

    /// Whether this entry stops `name`, or work on `affected`, from going
    /// ahead. Its phase isn't trusted, so every path it records counts.
    pub(super) fn applies(&self, paths: &Paths, name: &str, affected: &[&Path]) -> bool {
        let (Some(blocked), Some(manifest)) = (&self.blocks, &self.manifest) else {
            return true;
        };
        blocked == name
            || manifest
                .payloads
                .iter()
                .flat_map(|payload| [&payload.original, &payload.retained])
                .filter_map(|value| resolved(paths, value).ok())
                .any(|location| {
                    affected
                        .iter()
                        .any(|candidate| path_overlap(candidate, &location))
                })
    }
}

pub(super) fn read_manifest(paths: &Paths, id: &str) -> Result<Manifest> {
    #[cfg(any(test, feature = "test-support"))]
    MANIFEST_READS.with(|reads| reads.set(reads.get() + 1));
    identity(&paths.trash_dir())?.ok_or_else(|| anyhow!("trash storage unavailable"))?;
    let dir = entry_dir(paths, id)?;
    // A missing entry is a wrong ID, not a damaged one.
    let meta = fs::symlink_metadata(&dir)?;
    let unverifiable = |blocks, reason, manifest| Unverifiable {
        id: id.to_owned(),
        dir: dir.clone(),
        blocks,
        reason,
        manifest,
    };
    let manifest = match parse_manifest(&meta, &dir) {
        Ok(manifest) => manifest,
        Err(reason) => return Err(unverifiable(None, reason, None).into()),
    };
    if !name::is_valid(&manifest.name) {
        let reason = format!("invalid environment name `{}`", manifest.name);
        return Err(unverifiable(None, reason, Some(Box::new(manifest))).into());
    }
    match verify_manifest(paths, id, &dir, &manifest) {
        Ok(()) => Ok(manifest),
        Err(err) => Err(unverifiable(
            Some(manifest.name.clone()),
            format!("{err:#}"),
            Some(Box::new(manifest)),
        )
        .into()),
    }
}

fn parse_manifest(meta: &fs::Metadata, dir: &Path) -> Result<Manifest, String> {
    if !meta.file_type().is_dir() {
        return Err("it isn't a folder".into());
    }
    let file = dir.join(MANIFEST);
    match fs::symlink_metadata(&file) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => return Err(format!("{MANIFEST} isn't a regular file")),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(format!("{MANIFEST} is missing"));
        }
        Err(err) => return Err(format!("{MANIFEST} can't be read: {err}")),
    }
    let text =
        fs::read_to_string(&file).map_err(|err| format!("{MANIFEST} can't be read: {err}"))?;
    toml::from_str(&text).map_err(|err| format!("{MANIFEST} can't be parsed: {}", err.message()))
}

/// Everything a parsed manifest must satisfy. Errors are short reasons; the
/// caller says which entry they belong to.
fn verify_manifest(paths: &Paths, id: &str, dir: &Path, manifest: &Manifest) -> Result<()> {
    if manifest.version != VERSION {
        bail!("unsupported manifest version {}", manifest.version);
    }
    if manifest.id != id {
        bail!("its manifest is for entry {}", manifest.id);
    }
    if !matches_identity(identity(dir)?, manifest.entry_identity) {
        bail!("its folder's identity changed");
    }
    if manifest.removed_at.parse::<jiff::Timestamp>().is_err() {
        bail!("unreadable removal time `{}`", manifest.removed_at);
    }
    let mut seen = std::collections::HashSet::new();
    for payload in &manifest.payloads {
        let original = resolved(paths, &payload.original)?;
        let retained = resolved(paths, &payload.retained)?;
        if !seen.insert(payload.kind as u8) || original == retained {
            bail!("its {} payload is invalid", kind_name(payload.kind));
        }
        if payload.copied_identity.is_some()
            && (matches!(payload.kind, Kind::Prefix | Kind::Snapshots)
                || payload.source_parent.mount == payload.destination_parent.mount)
        {
            bail!(
                "it claims an invalid copied {} payload",
                kind_name(payload.kind)
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
            bail!("it claims an unrelated payload at {}", original.display());
        }
        for (stage, destination) in [
            (&payload.out_stage, &retained),
            (&payload.return_stage, &original),
        ] {
            if let Some(stage) = stage {
                let location = resolved(paths, &stage.location)?;
                let staging_parent = if destination.parent() == Some(dir) {
                    parent(dir)?
                } else {
                    parent(destination)?
                };
                if location.parent() != Some(staging_parent)
                    || !location
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(".wineo-stage-"))
                {
                    bail!("unsafe staging location {}", location.display());
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
                    "it claims an unrelated retained path {}",
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
                "it claims an unrelated retained path {}",
                retained.display()
            );
        }
    }
    if !seen.contains(&(Kind::Config as u8)) {
        bail!("it has no config payload");
    }
    if seen.contains(&(Kind::Prefix as u8)) && !manifest.include_prefix
        || seen.contains(&(Kind::Snapshots as u8)) && !manifest.include_prefix
        || seen.contains(&(Kind::Prefix as u8)) && manifest.prefix_present != Some(true)
    {
        bail!("its prefix ownership is inconsistent");
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
            bail!("its config payload is unavailable");
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
                bail!("its prefix differs from its saved config");
            }
        }
    }
    Ok(())
}

fn kind_name(kind: Kind) -> String {
    format!("{kind:?}").to_lowercase()
}
