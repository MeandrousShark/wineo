//! Unpacking one product into its staging folder. Every write goes through
//! directory fds opened without following symlinks, so nothing the package
//! or anyone else puts in the folder can redirect a write outside it.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;

use anyhow::{Context, Result};
use md5::{Digest, Md5};
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;

use super::{Package, PackageError, Split, invalid};
use crate::cancel;
use crate::gog::folder::{EntryType, WinningEntry};

const BUFFER: usize = 256 * 1024;
/// How often a large file checks for cancellation.
const CANCEL_EVERY: u64 = 8 * 1024 * 1024;

/// Bytes written to staging so far, out of every file it will write,
/// assembled split files included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageProgress {
    pub done: u64,
    pub total: u64,
}

/// The staging folder's name inside the game folder. Recovery removes it
/// with the rest of the operation's `.wineo-<operation>-*` files.
pub fn staging_name(operation: &str, product: u64) -> String {
    format!(".wineo-{operation}-stage-{product}")
}

fn check_operation(operation: &str) -> Result<()> {
    if operation.is_empty() || operation.contains(['/', '\0']) {
        anyhow::bail!("{operation:?} isn't a valid operation id");
    }
    Ok(())
}

fn open_folder(folder: &Path) -> Result<OwnedFd> {
    rustix::fs::open(
        folder,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .with_context(|| format!("failed to open {}", folder.display()))
}

fn open_dir_at(dir: impl AsFd, name: impl rustix::path::Arg) -> rustix::io::Result<OwnedFd> {
    rustix::fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
}

/// Opens the folder holding `path` inside the staging root, one component
/// at a time so no symlink is ever followed.
fn open_parent<'a>(root: &OwnedFd, path: &'a str) -> Result<(OwnedFd, &'a str)> {
    let (parent, leaf) = match path.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent), leaf),
        None => (None, path),
    };
    let mut dir = root
        .try_clone()
        .context("failed to open the staging folder")?;
    if let Some(parent) = parent {
        for part in parent.split('/') {
            dir = open_dir_at(&dir, part)
                .with_context(|| format!("failed to open {parent} in staging"))?;
        }
    }
    Ok((dir, leaf))
}

fn create_file(dir: &OwnedFd, name: &str, mode: u32) -> rustix::io::Result<File> {
    let fd = rustix::fs::openat(
        dir,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(mode),
    )?;
    // The umask may have narrowed the mode, and the mode is what's recorded.
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(mode))?;
    Ok(File::from(fd))
}

fn lowercase_hex(hasher: Md5) -> String {
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Copier<'a, F> {
    cancel: &'a cancel::Token,
    progress: F,
    done: u64,
    total: u64,
    buffer: Vec<u8>,
    /// Bytes copied since the last cancel check, across files and pieces.
    since_check: u64,
}

impl<F: FnMut(StageProgress)> Copier<'_, F> {
    /// Copies everything `from` holds into `to`, returning its size and MD5.
    /// Stops once more than `limit` bytes arrive.
    fn copy(
        &mut self,
        path: &str,
        from: &mut impl Read,
        to: &mut File,
        limit: u64,
    ) -> Result<(u64, String)> {
        let mut hasher = Md5::new();
        let mut written = 0u64;
        loop {
            let read = match from.read(&mut self.buffer) {
                Ok(read) => read,
                // How the zip reader reports a wrong size or checksum.
                Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                    return Err(
                        invalid(format!("{path} doesn't match its size or checksum")).into(),
                    );
                }
                Err(error) => return Err(error.into()),
            };
            if read == 0 {
                break;
            }
            written += read as u64;
            if written > limit {
                break;
            }
            hasher.update(&self.buffer[..read]);
            to.write_all(&self.buffer[..read])?;
            self.done += read as u64;
            (self.progress)(StageProgress {
                done: self.done,
                total: self.total,
            });
            self.since_check += read as u64;
            if self.since_check >= CANCEL_EVERY {
                self.since_check = 0;
                self.check_cancel()?;
            }
        }
        Ok((written, lowercase_hex(hasher)))
    }

    fn check_cancel(&self) -> Result<()> {
        if self.cancel.is_canceled() {
            return Err(PackageError::Canceled.into());
        }
        Ok(())
    }
}

/// Unpacks `package`'s game files into its staging folder (`staging_name`)
/// inside `folder`, assembling split files, and returns what was staged with
/// paths relative to the staging folder. Everything is synced before it
/// returns. On error or cancellation the staging folder is left for the
/// caller to remove.
pub fn stage(
    package: &mut Package,
    folder: &Path,
    operation: &str,
    product: u64,
    cancel: &cancel::Token,
    progress: impl FnMut(StageProgress),
) -> Result<Vec<WinningEntry>> {
    check_operation(operation)?;
    let name = staging_name(operation, product);
    let folder_fd = open_folder(folder)?;
    let shown = folder.join(&name);
    rustix::fs::mkdirat(&folder_fd, name.as_str(), Mode::from_raw_mode(0o755))
        .with_context(|| format!("failed to create {}", shown.display()))?;
    let root = open_dir_at(&folder_fd, &name)
        .with_context(|| format!("failed to open {}", shown.display()))?;
    rustix::fs::fchmod(&root, Mode::from_raw_mode(0o755))
        .with_context(|| format!("failed to set {}'s mode", shown.display()))?;

    let mut copier = Copier {
        cancel,
        progress,
        done: 0,
        // Opening checked that this sum fits.
        total: package.unpacked_size() + package.assembled_size(),
        buffer: vec![0; BUFFER],
        since_check: 0,
    };
    let mut staged = Vec::new();
    // Each written file's (device, inode), so assembly reads only the files
    // staging wrote.
    let mut written = HashMap::new();

    let entries = package.entries.clone();
    for entry in entries.iter().filter(|e| e.kind == EntryType::Directory) {
        copier.check_cancel()?;
        let (parent, leaf) = open_parent(&root, &entry.path)?;
        rustix::fs::mkdirat(&parent, leaf, Mode::from_raw_mode(entry.mode))
            .with_context(|| format!("failed to create {} in staging", entry.path))?;
        let dir = open_dir_at(&parent, leaf)
            .with_context(|| format!("failed to open {} in staging", entry.path))?;
        rustix::fs::fchmod(&dir, Mode::from_raw_mode(entry.mode))
            .with_context(|| format!("failed to set {}'s mode", entry.path))?;
        staged.push(winning(
            &entry.path,
            EntryType::Directory,
            entry.mode,
            product,
        ));
    }

    // In archive order, so the package is read front to back.
    let mut files: Vec<_> = entries
        .iter()
        .filter(|e| e.kind == EntryType::File)
        .collect();
    files.sort_by_key(|entry| entry.index);
    for entry in files {
        copier.check_cancel()?;
        let (parent, leaf) = open_parent(&root, &entry.path)?;
        let mut file = create_file(&parent, leaf, entry.mode)
            .with_context(|| format!("failed to create {} in staging", entry.path))?;
        let mut zipped = package
            .archive
            .by_index(entry.index.expect("files are zip entries"))
            .map_err(|error| invalid(error.to_string()))?;
        let (size, md5) = copier
            .copy(&entry.path, &mut zipped, &mut file, entry.size)
            .with_context(|| format!("failed to unpack {}", entry.path))?;
        if size != entry.size {
            return Err(invalid(format!("{}'s size doesn't match", entry.path)).into());
        }
        file.sync_all()
            .with_context(|| format!("failed to sync {} in staging", entry.path))?;
        let stat = rustix::fs::fstat(&file)
            .with_context(|| format!("failed to check {} in staging", entry.path))?;
        written.insert(entry.path.as_str(), (stat.st_dev, stat.st_ino));
        staged.push(WinningEntry {
            size: Some(size),
            md5: Some(md5),
            ..winning(&entry.path, EntryType::File, entry.mode, product)
        });
    }

    for entry in entries.iter().filter(|e| e.kind == EntryType::Symlink) {
        copier.check_cancel()?;
        let target = entry.target.as_deref().expect("symlinks have targets");
        let (parent, leaf) = open_parent(&root, &entry.path)?;
        rustix::fs::symlinkat(target, &parent, leaf)
            .with_context(|| format!("failed to create {} in staging", entry.path))?;
        staged.push(WinningEntry {
            target: Some(target.to_owned()),
            ..winning(&entry.path, EntryType::Symlink, entry.mode, product)
        });
    }

    for (number, split) in package.splits.clone().iter().enumerate() {
        copier.check_cancel()?;
        let mode = staged
            .iter()
            .find(|staged| staged.path == split.path)
            .map_or(0o644, |staged| staged.mode);
        let pieces = split
            .pieces
            .iter()
            .map(|piece| Piece {
                path: piece.as_str(),
                size: package.file_size(piece).expect("pieces are files"),
                identity: written[piece.as_str()],
            })
            .collect::<Vec<_>>();
        let (size, md5) = assemble(&root, split, &pieces, operation, number, mode, &mut copier)?;
        staged.retain(|staged| staged.path != split.path && !split.pieces.contains(&staged.path));
        staged.push(WinningEntry {
            size: Some(size),
            md5: Some(md5),
            ..winning(&split.path, EntryType::File, mode, product)
        });
    }

    sync_tree(&root, &entries)?;
    folder_fd
        .try_clone()
        .map(File::from)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("failed to sync {}", folder.display()))?;

    // A cancel during the last copy or the syncs still counts.
    copier.check_cancel()?;
    staged.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(staged)
}

fn winning(path: &str, kind: EntryType, mode: u32, product: u64) -> WinningEntry {
    WinningEntry {
        path: path.to_owned(),
        kind,
        size: None,
        md5: None,
        target: None,
        mode,
        product,
    }
}

/// Joins the pieces into a temporary file beside them, then renames it over
/// the output and deletes the pieces, like GOG's script.
fn assemble<F: FnMut(StageProgress)>(
    root: &OwnedFd,
    split: &Split,
    pieces: &[Piece<'_>],
    operation: &str,
    number: usize,
    mode: u32,
    copier: &mut Copier<'_, F>,
) -> Result<(u64, String)> {
    let (parent, leaf) = open_parent(root, &split.path)?;
    let temp = format!(".wineo-{operation}-split-{number}");
    let mut output = create_file(&parent, &temp, mode)
        .with_context(|| format!("failed to assemble {} in staging", split.path))?;
    let mut hasher = Md5::new();
    let mut size = 0;
    for &Piece {
        path: piece,
        size: declared,
        identity,
    } in pieces
    {
        let file = open_piece(root, piece, declared, identity)?;
        let mut tee = Tee {
            inner: file.take(declared),
            hasher: &mut hasher,
        };
        let (written, _) = copier
            .copy(piece, &mut tee, &mut output, declared)
            .with_context(|| format!("failed to assemble {}", split.path))?;
        if written != declared {
            return Err(invalid(format!("{piece} changed while staging")).into());
        }
        size += written;
    }
    output
        .sync_all()
        .with_context(|| format!("failed to sync {} in staging", split.path))?;
    rustix::fs::renameat(&parent, temp.as_str(), &parent, leaf)
        .with_context(|| format!("failed to assemble {} in staging", split.path))?;
    for piece in &split.pieces {
        let (piece_dir, piece_leaf) = open_parent(root, piece)?;
        rustix::fs::unlinkat(&piece_dir, piece_leaf, AtFlags::empty())
            .with_context(|| format!("failed to remove {piece} in staging"))?;
    }
    Ok((size, lowercase_hex(hasher)))
}

/// A split piece staging wrote, with its declared size and the (device,
/// inode) it was written to.
#[derive(Clone, Copy)]
struct Piece<'a> {
    path: &'a str,
    size: u64,
    identity: (u64, u64),
}

/// Opens a staged piece, refusing it unless it's still the regular file
/// staging wrote (same device and inode) at its declared size. That catches
/// a piece replaced by anything else, including a hard link elsewhere, but
/// not one rewritten in place at the same size. Nonblocking, so a FIFO put
/// there can't hang the open.
fn open_piece(root: &OwnedFd, piece: &str, declared: u64, identity: (u64, u64)) -> Result<File> {
    let changed = || invalid(format!("{piece} changed while staging"));
    let (dir, leaf) = open_parent(root, piece)?;
    let fd = match rustix::fs::openat(
        &dir,
        leaf,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::LOOP | Errno::NOENT) => return Err(changed().into()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to open {piece} in staging"));
        }
    };
    let stat =
        rustix::fs::fstat(&fd).with_context(|| format!("failed to check {piece} in staging"))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || u64::try_from(stat.st_size).ok() != Some(declared)
        || (stat.st_dev, stat.st_ino) != identity
    {
        return Err(changed().into());
    }
    Ok(File::from(fd))
}

/// Hashes what passes through, so an assembled file's MD5 covers every
/// piece in order.
struct Tee<'a> {
    inner: std::io::Take<File>,
    hasher: &'a mut Md5,
}

impl Read for Tee<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.hasher.update(&buf[..read]);
        Ok(read)
    }
}

/// Syncs every directory in the staging tree, deepest first, then the root.
fn sync_tree(root: &OwnedFd, entries: &[super::Entry]) -> Result<()> {
    let mut dirs: Vec<&str> = entries
        .iter()
        .filter(|entry| entry.kind == EntryType::Directory)
        .map(|entry| entry.path.as_str())
        .collect();
    dirs.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));
    for path in dirs {
        let (parent, leaf) = open_parent(root, path)?;
        let dir = open_dir_at(&parent, leaf)
            .with_context(|| format!("failed to open {path} in staging"))?;
        File::from(dir)
            .sync_all()
            .with_context(|| format!("failed to sync {path} in staging"))?;
    }
    root.try_clone()
        .map(File::from)
        .and_then(|dir| dir.sync_all())
        .context("failed to sync the staging folder")
}

/// Removes a staging folder without following any symlink in it, or the
/// folder itself if it is one. A missing folder is fine.
pub fn remove_staging(folder: &Path, operation: &str, product: u64) -> Result<()> {
    check_operation(operation)?;
    let folder_fd = open_folder(folder)?;
    let name = staging_name(operation, product);
    let c_name = CString::new(name.as_str()).expect("checked for NUL");
    remove_at(&folder_fd, &c_name)
        .with_context(|| format!("failed to remove {}", folder.join(&name).display()))?;
    folder_fd
        .try_clone()
        .map(File::from)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("failed to sync {}", folder.display()))
}

fn remove_at(dir: &OwnedFd, name: &CStr) -> Result<()> {
    let stat = match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::NOENT) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        rustix::fs::unlinkat(dir, name, AtFlags::empty())?;
        return Ok(());
    }
    let child = open_dir_at(dir, name)?;
    let mut names = Vec::new();
    let mut listing = rustix::fs::Dir::read_from(&child)?;
    while let Some(entry) = listing.read() {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if bytes != b"." && bytes != b".." {
            names.push(entry.file_name().to_owned());
        }
    }
    for child_name in names {
        remove_at(&child, &child_name)?;
    }
    rustix::fs::unlinkat(dir, name, AtFlags::REMOVEDIR)?;
    Ok(())
}
