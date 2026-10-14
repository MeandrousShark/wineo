//! Payload digests, tree comparison, and verified metadata copies.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

use super::identity::{identity, parent, sync};

/// Marks digests written in the unambiguous encoding. Older manifests hold
/// bare v1 digests, which `digest_matches` still checks the old way.
const DIGEST_V2: &str = "v2:";

/// A digest of a payload tree. Every name and symlink target is length
/// prefixed and every file is hashed on its own, so no two different trees
/// produce the same input, as v1's plain concatenation could.
pub(super) fn digest(path: &Path) -> Result<String> {
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
pub(super) fn digest_matches(path: &Path, recorded: &str) -> Result<bool> {
    if recorded.starts_with(DIGEST_V2) {
        Ok(digest(path)? == recorded)
    } else {
        Ok(digest_v1(path)? == recorded)
    }
}

/// Whether two trees hold exactly the same names, contents, and links. A
/// digest match decides nothing destructive on its own, since a v1 digest
/// from an older manifest can collide.
pub(super) fn same_tree(a: &Path, b: &Path) -> Result<bool> {
    Ok(contained_in(a, b)? && contained_in(b, a)?)
}

pub(super) fn digest_v1(path: &Path) -> Result<String> {
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
pub(super) fn contained_in(part: &Path, whole: &Path) -> Result<bool> {
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

pub(super) fn copy_verified(source: &Path, destination: &Path, expected: &str) -> Result<()> {
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
