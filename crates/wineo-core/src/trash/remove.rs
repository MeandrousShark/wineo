//! Removing an environment into a new trash entry.

use std::fs;
use std::io;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use sha2::{Digest, Sha256};

use crate::config::{self, GlobalConfig};
use crate::desktop;
use crate::environment::{self, Environment};
use crate::gog::folder::{self, FolderError};
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};
use crate::prefix;
use crate::runtime::Runtime;

use super::identity::{create_storage, delete_owned, identity, parent, sync};
use super::manifest::write_manifest;
use super::markers::{finish_marker, retire_marker, start_marker};
use super::payload::{make_payload, move_out};
use super::{
    Entry, Intent, Kind, Manifest, Payload, Phase, VERSION, entry_dir, failpoint, info, stored,
};

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
