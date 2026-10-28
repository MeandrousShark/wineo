//! On-disk identity, mount checks, and the synced filesystem steps trash builds on.

use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::{RenameFlags, renameat_with};

use super::{Identity, matches_identity};

pub(super) fn identity(path: &Path) -> Result<Option<Identity>> {
    use rustix::fs::FileType;

    let Some(seen) =
        crate::files::observe(path).with_context(|| format!("inspecting {}", path.display()))?
    else {
        return Ok(None);
    };
    if !matches!(seen.kind, FileType::RegularFile | FileType::Directory) {
        bail!("refusing non-regular trash payload at {}", path.display());
    }
    Ok(Some(Identity {
        device: seen.device,
        inode: seen.inode,
        mount: seen.mount,
        birth: seen.birth,
        fsid: seen.fsid,
    }))
}

pub(super) fn require(path: &Path, expected: Identity) -> Result<()> {
    if !matches_identity(identity(path)?, expected) {
        bail!(
            "identity changed or storage unavailable at {}",
            path.display()
        );
    }
    Ok(())
}

pub(super) fn require_same_mount_as_parent(path: &Path) -> Result<()> {
    let current = identity(path)?.ok_or_else(|| anyhow!("payload unavailable"))?;
    let parent = identity(parent(path)?)?.ok_or_else(|| anyhow!("payload storage unavailable"))?;
    if current.mount != parent.mount {
        bail!("payload root is a separate mount at {}", path.display());
    }
    Ok(())
}

pub(super) fn parent(path: &Path) -> Result<&Path> {
    path.parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", path.display()))
}

pub(super) fn create_storage(path: &Path) -> Result<()> {
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

pub(super) fn sync(path: &Path) -> Result<()> {
    File::open(path)?
        .sync_all()
        .with_context(|| format!("syncing {}", path.display()))
}

pub(super) fn delete_owned(path: &Path) -> Result<()> {
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

pub(super) fn subvolume_property(path: &Path) -> Result<Option<bool>> {
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

pub(super) fn preflight_tree(path: &Path) -> Result<()> {
    if let Some(mount) = foreign_mount(path)? {
        bail!(
            "nested mount at {} is not owned by this trash entry",
            mount.display()
        );
    }
    Ok(())
}

pub(super) fn check_tree_mounts(path: &Path) -> Result<()> {
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

pub(super) fn rename_owned(source: &Path, destination: &Path) -> Result<()> {
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
