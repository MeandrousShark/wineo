//! Moving one payload into the trash and back, through recorded staging.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::paths::Paths;

#[cfg(test)]
use super::BEFORE_RETAINED_DROP;
use super::{
    Identity, Kind, Manifest, Payload, Stage, Step, check_tree_mounts, contained_in, copy_verified,
    create_storage, delete_owned, digest, digest_matches, entry_dir, failpoint, identity,
    matches_identity, parent, rename_owned, require, require_same_mount_as_parent, resolved,
    same_tree, stored, sync, write_manifest,
};

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
pub(super) fn reserve_copy(source: &Path, destination: &Path) -> Result<Identity> {
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

pub(super) fn make_payload(
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

pub(super) fn move_out(paths: &Paths, manifest: &mut Manifest, index: usize) -> Result<()> {
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

pub(super) fn verify_parents(paths: &Paths, payload: &Payload) -> Result<(PathBuf, PathBuf)> {
    let original = resolved(paths, &payload.original)?;
    let retained = resolved(paths, &payload.retained)?;
    require(parent(&original)?, payload.source_parent)?;
    require(parent(&retained)?, payload.destination_parent)?;
    Ok((original, retained))
}

pub(super) fn retained_identity(payload: &Payload) -> Identity {
    payload.copied_identity.unwrap_or(payload.identity)
}

pub(super) fn verified_retained(path: &Path, payload: &Payload) -> Result<()> {
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

pub(super) fn return_payload(
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
