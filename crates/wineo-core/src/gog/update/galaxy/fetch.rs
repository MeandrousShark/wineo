//! Downloading Galaxy files a chunk at a time, up to 4 chunks at once
//! across all files, each file into a new file of its own that the caller
//! then renames into place.
//!
//! Each chunk is written at its offset in its file, so a file's chunks can
//! finish in any order and one big file keeps every worker busy. Whoever
//! writes a file's last chunk syncs the file and reads it back to check its
//! whole-file hashes, so nothing half written is ever handed on.
//!
//! Chunks go in rounds of a bounded number of bytes. Each round takes a
//! fresh access token on this thread, since the session can't be shared,
//! and only a secure link fetched during the round uses it; between rounds
//! the login is checked again. A file can span rounds, its new file kept
//! open between them, so a round's end waits only for the chunks in
//! flight, never for a whole file. Every write to wineo's state stays on
//! this thread too: workers only fetch, write, and sync, and the caller's
//! `done` runs here as each file finishes.

use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use anyhow::{Context, Result, anyhow};
use md5::{Digest, Md5};
use rustix::fs::{Mode, OFlags};
use sha2::Sha256;
use wineo_gog::{ChunkLinks, Client};

use super::build::gog_chunk;
use super::record::FileEntry;
use crate::cancel::Token;
use crate::gog::account::Session;
use crate::gog::update::UpdateError;

/// How many chunks download at once.
pub(super) const WORKERS: usize = 4;
/// A round ends after this many chunk bytes or chunks, whichever comes
/// first, so its access token is still good for a secure link at its end.
const ROUND_BYTES: u64 = 1 << 30;
const ROUND_CHUNKS: usize = 1024;
/// How much of a finished file is read back at a time to check it.
const READ_BUFFER: usize = 1 << 20;

/// One file to download.
pub(super) struct Job<'a> {
    /// The folder it's written in, as a path relative to `root` ("" for
    /// `root` itself) opened without following symlinks, and the new name
    /// it's written under. Chunks are taken in order, and a file is opened
    /// with its first and closed once its last is written, so only the few
    /// files in flight hold descriptors however long the queue.
    pub(super) root: &'a OwnedFd,
    pub(super) parent: String,
    pub(super) name: String,
    pub(super) file: &'a FileEntry,
    /// The secure links of the file's product, or of the redistributables.
    pub(super) links: &'a ChunkLinks,
}

/// One piece of work: a chunk of a job's file at its offset, or, for an
/// empty file, the file itself.
struct Unit {
    job: usize,
    chunk: Option<usize>,
    offset: u64,
}

/// A job's new file, shared by the workers writing its chunks.
struct Output {
    file: Mutex<Option<Arc<File>>>,
    /// Chunks not yet written. Only a written chunk counts, so a failed one
    /// can never let the file pass as whole.
    remaining: AtomicUsize,
}

struct Downloads<'r, 'a> {
    jobs: &'r [Job<'a>],
    outputs: Vec<Output>,
    token: &'r Token,
    written: AtomicU64,
}

/// Downloads every job's file under its new name, checking each chunk and
/// the whole file, and syncs it. `done` gets each job and its MD5 as it
/// finishes; `progress` the bytes written so far, after each chunk. The
/// first failure stops the rest, and a job's half-written file is left for
/// the caller's cleanup of its temporary files.
pub(super) fn run(
    session: &mut Session,
    jobs: &[Job],
    token: &Token,
    progress: &mut dyn FnMut(u64),
    done: &mut dyn FnMut(&Job, String) -> Result<()>,
) -> Result<()> {
    let mut units = Vec::new();
    for (job, entry) in jobs.iter().enumerate() {
        if entry.file.chunks.is_empty() {
            units.push(Unit {
                job,
                chunk: None,
                offset: 0,
            });
        }
        let mut offset = 0u64;
        for (index, chunk) in entry.file.chunks.iter().enumerate() {
            units.push(Unit {
                job,
                chunk: Some(index),
                offset,
            });
            offset = offset.saturating_add(chunk.size);
        }
    }
    let downloads = Downloads {
        jobs,
        outputs: jobs
            .iter()
            .map(|job| Output {
                file: Mutex::new(None),
                remaining: AtomicUsize::new(job.file.chunks.len()),
            })
            .collect(),
        token,
        written: AtomicU64::new(0),
    };
    #[cfg(test)]
    let (round_bytes, round_chunks) =
        super::tests::round_limits().unwrap_or((ROUND_BYTES, ROUND_CHUNKS));
    #[cfg(not(test))]
    let (round_bytes, round_chunks) = (ROUND_BYTES, ROUND_CHUNKS);
    let mut start = 0;
    while start < units.len() {
        let mut end = start;
        let mut bytes = 0u64;
        while end < units.len()
            && end - start < round_chunks
            && (end == start || bytes < round_bytes)
        {
            bytes = bytes.saturating_add(downloads.size_of(&units[end]));
            end += 1;
        }
        if token.is_canceled() {
            return Err(UpdateError::Canceled { journal: true }.into());
        }
        let access = session.access_token()?;
        downloads.round(
            session.client(),
            access.expose(),
            &units[start..end],
            progress,
            done,
        )?;
        start = end;
    }
    Ok(())
}

impl Downloads<'_, '_> {
    fn size_of(&self, unit: &Unit) -> u64 {
        unit.chunk
            .map_or(0, |chunk| self.jobs[unit.job].file.chunks[chunk].size)
    }

    fn round(
        &self,
        client: &Client,
        access: &str,
        units: &[Unit],
        progress: &mut dyn FnMut(u64),
        done: &mut dyn FnMut(&Job, String) -> Result<()>,
    ) -> Result<()> {
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        // One message per unit: its file's MD5 once the file is whole.
        let (send, receive) = mpsc::channel::<(usize, Result<Option<String>>)>();
        std::thread::scope(|scope| {
            for _ in 0..WORKERS.min(units.len()) {
                let send = send.clone();
                let (next, stop) = (&next, &stop);
                scope.spawn(move || {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= units.len()
                            || self.token.is_canceled()
                            || stop.load(Ordering::Relaxed)
                        {
                            return;
                        }
                        let unit = &units[index];
                        let result = self.write_unit(client, access, unit);
                        if send.send((unit.job, result)).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(send);
            let mut first = None;
            let mut finished = 0;
            for (job, result) in receive {
                if first.is_some() {
                    continue;
                }
                let job = &self.jobs[job];
                let outcome = result.and_then(|whole| match whole {
                    Some(md5) => done(job, md5),
                    None => Ok(()),
                });
                match outcome {
                    Ok(()) => {
                        finished += 1;
                        progress(self.written.load(Ordering::Relaxed));
                    }
                    Err(error) => {
                        stop.store(true, Ordering::Relaxed);
                        first = Some(error);
                    }
                }
            }
            match first {
                Some(error) => Err(error),
                // Workers stop early only when canceled.
                None if finished < units.len() => {
                    Err(UpdateError::Canceled { journal: true }.into())
                }
                None => Ok(()),
            }
        })
    }

    /// Writes one chunk at its offset, and checks the file when it was the
    /// last. The file's MD5 once it's whole.
    fn write_unit(&self, client: &Client, access: &str, unit: &Unit) -> Result<Option<String>> {
        let job = &self.jobs[unit.job];
        let output = &self.outputs[unit.job];
        let path = &job.file.path;
        let file = output.open(job)?;
        if let Some(index) = unit.chunk {
            let bytes = client
                .chunk(job.links, access, &gog_chunk(&job.file.chunks[index]))
                .map_err(|error| -> anyhow::Error {
                    if error.kind() == wineo_gog::ErrorKind::Canceled {
                        UpdateError::Canceled { journal: true }.into()
                    } else {
                        anyhow::Error::from(error).context(format!("Couldn't download {path}"))
                    }
                })?;
            file.write_all_at(&bytes, unit.offset)
                .with_context(|| format!("failed to write {path}"))?;
            self.written
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
            if output.remaining.fetch_sub(1, Ordering::AcqRel) != 1 {
                return Ok(None);
            }
        }
        // Every chunk is written, so no one else needs the file open.
        output.close();
        check(job.file, &file).map(Some)
    }
}

impl Output {
    /// The job's new file, created by whichever of its chunks comes first.
    fn open(&self, job: &Job) -> Result<Arc<File>> {
        let mut slot = self
            .file
            .lock()
            .map_err(|_| anyhow!("a download worker failed"))?;
        if let Some(file) = &*slot {
            return Ok(Arc::clone(file));
        }
        let path = &job.file.path;
        let dir = open_dir(job.root, &job.parent)?;
        let fd = rustix::fs::openat(
            &dir,
            job.name.as_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o644),
        )
        .with_context(|| format!("failed to create a file for {path}"))?;
        // The umask may have narrowed it; Wine ignores executable bits anyway.
        rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o644))
            .with_context(|| format!("failed to set {path}'s mode"))?;
        let file = Arc::new(File::from(fd));
        *slot = Some(Arc::clone(&file));
        Ok(file)
    }

    fn close(&self) {
        if let Ok(mut slot) = self.file.lock() {
            slot.take();
        }
    }
}

/// Syncs a whole file and reads it back, checking its size and the hashes
/// GOG lists for it. Its MD5.
fn check(entry: &FileEntry, file: &File) -> Result<String> {
    let path = &entry.path;
    file.sync_all()
        .with_context(|| format!("failed to sync {path}"))?;
    let mut md5 = Md5::new();
    let mut sha256 = entry.sha256.as_ref().map(|_| Sha256::new());
    // One byte over the listed size, so a longer file still shows.
    let length = usize::try_from(entry.size)
        .map_or(READ_BUFFER, |size| size.saturating_add(1).min(READ_BUFFER));
    let mut buffer = vec![0; length];
    let mut size = 0u64;
    loop {
        let read = match file.read_at(&mut buffer, size) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {path} back"));
            }
        };
        md5.update(&buffer[..read]);
        if let Some(sha256) = &mut sha256 {
            sha256.update(&buffer[..read]);
        }
        size += read as u64;
    }
    if size != entry.size {
        anyhow::bail!("Couldn't download {path}: it isn't the size GOG lists");
    }
    let md5 = crate::units::hex(md5.finalize());
    let sha256 = sha256.map(|sha256| crate::units::hex(sha256.finalize()));
    if entry.md5.as_ref().is_some_and(|expected| *expected != md5)
        || entry.sha256.as_ref() != sha256.as_ref()
    {
        anyhow::bail!("Couldn't download {path}: the file doesn't match GOG's checksum");
    }
    Ok(md5)
}

/// The folder `relative` to `root`, one component at a time without
/// following symlinks.
pub(super) fn open_dir(root: &OwnedFd, relative: &str) -> Result<OwnedFd> {
    let mut dir = root.try_clone().context("failed to open a folder")?;
    if relative.is_empty() {
        return Ok(dir);
    }
    for part in relative.split('/') {
        dir = crate::gog::package::open_dir_at(&dir, part)
            .with_context(|| format!("failed to open {relative}"))?;
    }
    Ok(dir)
}
