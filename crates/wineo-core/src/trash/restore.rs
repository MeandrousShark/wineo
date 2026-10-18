//! Returning a trash entry's payloads: restore, rollback, and finishing an entry.

use std::fs;
use std::io;

use anyhow::{Result, anyhow, bail};

use crate::build_lock;
use crate::config::{self, EnvironmentConfig, GlobalConfig};
use crate::desktop;
use crate::environment::{self, Environment};
use crate::gog::folder::{FolderError, FolderLock};
use crate::paths::Paths;

use super::digest::{copy_verified, digest, digest_matches};
use super::identity::{identity, parent, require, sync};
use super::manifest::write_manifest;
use super::markers::{finish_marker, marker_for, retire_marker, start_marker};
use super::payload::{retained_identity, return_payload, verified_retained, verify_parents};
use super::{
    Intent, Kind, MANIFEST, Manifest, Payload, Phase, Step, entry_dir, failpoint, locked, resolved,
    stored,
};

/// Rebuilds the desktop entries. The warning says the global config was
/// passed over for defaults, which the entries were built from.
fn reconcile(paths: &Paths, name: &str) -> Result<Option<String>> {
    let path = paths.environment_config(name);
    let config = match fs::read_to_string(&path)
        .ok()
        .and_then(|text| EnvironmentConfig::parse(&path, &text).ok())
    {
        Some(config) => config,
        None => return Ok(None), // Broken raw configs stay registered without shortcuts.
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
    let loaded = GlobalConfig::load_or_default(paths)?;
    let global = loaded.config;
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
    Ok(loaded.warning)
}

pub(super) fn finish_entry(paths: &Paths, manifest: &Manifest) -> Result<()> {
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

pub(super) fn complete_entry(paths: &Paths, manifest: &Manifest) -> Result<()> {
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

pub(super) fn restore_locked(
    paths: &Paths,
    manifest: &mut Manifest,
    rollback: bool,
    gog: &[FolderLock],
    notes: &mut Vec<String>,
) -> Result<()> {
    restore_locked_noting(paths, manifest, rollback, gog, || Ok(()), notes)
}

#[cfg(test)]
pub(super) fn restore_locked_with(
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
    // Recovery must finish even when build discovery is unavailable. The
    // global config only picks which build locks to take, so one that can't
    // be read skips them instead of refusing the recovery.
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
    let (desktop_error, warning) = match reconcile(paths, &manifest.name) {
        Ok(warning) => (None, warning),
        Err(err) => (Some(err), None),
    };
    // A manifest without an entry also covers a library.toml that was
    // unreadable at removal, when the real entry stayed in the file. Writing
    // None then would delete it once the file is repaired.
    let library_warning = match (rollback, manifest.library.as_ref()) {
        (false, Some(entry)) => crate::library::put(paths, &manifest.name, Some(entry))
            .err()
            .map(|err| {
                let mut values = Vec::new();
                if entry.favorite {
                    values.push("favorite".to_owned());
                }
                if entry.hidden {
                    values.push("hidden".to_owned());
                }
                if let Some(added) = entry.added {
                    values.push(format!("added {}", added.strftime("%Y-%m-%d")));
                }
                let lost = if values.is_empty() {
                    String::new()
                } else {
                    format!(" Values not restored: {}.", values.join(", "))
                };
                format!(
                    "Can't restore {} in library.toml: {}.{lost}",
                    manifest.name,
                    crate::error::chain(&err),
                )
            }),
        _ => None,
    };
    let completed = if manifest.payloads.iter().any(|payload| payload.kept) {
        keep_changed(paths, manifest, notes)
    } else {
        complete_entry(paths, manifest)
    };
    completed.map_err(|err| match &library_warning {
        Some(warning) => err.context(warning.clone()),
        None => err,
    })?;
    notes.extend(warning);
    if let Some(err) = desktop_error {
        let err = anyhow!(
            "environment {} was restored, but its desktop entries need attention: {err:#}",
            manifest.name
        );
        return Err(match library_warning {
            Some(warning) => err.context(warning),
            None => err,
        });
    }
    notes.extend(library_warning);
    Ok(())
}

/// Ends a rollback that kept a changed source live: the retained copies from
/// before the change become an ordinary trash entry, with a copy of the live
/// config so the entry can be restored once the live environment is gone.
pub(super) fn keep_changed(
    paths: &Paths,
    manifest: &mut Manifest,
    notes: &mut Vec<String>,
) -> Result<()> {
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

pub(super) fn with_environment_build_locks<T>(
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

/// Restores a trashed environment. The notes are warnings to show, such as
/// the global config having been passed over for defaults.
pub fn restore(paths: &Paths, id: &str) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    locked(paths, id, true, |manifest, gog| {
        if manifest.phase != Phase::Trashed {
            bail!("entry {id} needs `wineo validate --fix` first");
        }
        restore_locked(paths, manifest, false, gog, &mut notes)
    })?;
    Ok(notes)
}
