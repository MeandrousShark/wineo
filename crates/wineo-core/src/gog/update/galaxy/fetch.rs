//! Downloading Galaxy files, 4 at a time, each into a new file of its own
//! that the caller then renames into place.
//!
//! Files go in rounds. Each round takes a fresh access token on this
//! thread, since the session can't be shared, and only a secure link
//! fetched during the round uses it; between rounds the login is checked
//! again. Every write to wineo's state stays on this thread too: workers
//! only fetch, write, and sync their files, and the caller's `done` runs
//! here as each one finishes.

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;

use anyhow::{Context, Result};
use md5::{Digest, Md5};
use rustix::fs::{Mode, OFlags};
use wineo_gog::{ChunkLinks, Client};

use super::build::depot_file;
use super::record::FileEntry;
use crate::cancel::Token;
use crate::gog::account::Session;
use crate::gog::update::UpdateError;

/// How many files download at once.
pub(super) const WORKERS: usize = 4;
/// A round ends after this many files or bytes, whichever comes first, so
/// its access token is still good for a secure link at its end.
const ROUND_FILES: usize = 64;
const ROUND_BYTES: u64 = 1 << 30;

/// One file to download.
pub(super) struct Job<'a> {
    /// The folder it's written in, as a path relative to `root` ("" for
    /// `root` itself) opened without following symlinks, and the new name
    /// it's written under. Opened only while the file is written, so a long
    /// queue holds no descriptors.
    pub(super) root: &'a OwnedFd,
    pub(super) parent: String,
    pub(super) name: String,
    pub(super) file: &'a FileEntry,
    /// The secure links of the file's product, or of the redistributables.
    pub(super) links: &'a ChunkLinks,
}

/// Downloads every job's file under its new name, checking each chunk and
/// the whole file, and syncs it. `done` gets each job and its MD5 as it
/// finishes; `progress` the bytes written so far. The first failure stops
/// the rest, and a job's half-written file is left for the caller's
/// cleanup of its temporary files.
pub(super) fn run(
    session: &mut Session,
    jobs: &[Job],
    token: &Token,
    progress: &mut dyn FnMut(u64),
    done: &mut dyn FnMut(&Job, String) -> Result<()>,
) -> Result<()> {
    let written = AtomicU64::new(0);
    let mut start = 0;
    while start < jobs.len() {
        let mut end = start;
        let mut bytes = 0u64;
        while end < jobs.len() && end - start < ROUND_FILES && (end == start || bytes < ROUND_BYTES)
        {
            bytes = bytes.saturating_add(jobs[end].file.size);
            end += 1;
        }
        if token.is_canceled() {
            return Err(UpdateError::Canceled { journal: true }.into());
        }
        let access = session.access_token()?;
        round(
            session.client(),
            access.expose(),
            &jobs[start..end],
            token,
            &written,
            progress,
            done,
        )?;
        start = end;
    }
    Ok(())
}

fn round(
    client: &Client,
    access: &str,
    jobs: &[Job],
    token: &Token,
    written: &AtomicU64,
    progress: &mut dyn FnMut(u64),
    done: &mut dyn FnMut(&Job, String) -> Result<()>,
) -> Result<()> {
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (send, receive) = mpsc::channel::<(usize, Result<String>)>();
    std::thread::scope(|scope| {
        for _ in 0..WORKERS.min(jobs.len()) {
            let send = send.clone();
            let (next, stop) = (&next, &stop);
            scope.spawn(move || {
                let cancel = || token.is_canceled() || stop.load(Ordering::Relaxed);
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= jobs.len() || cancel() {
                        return;
                    }
                    let result = write_one(client, access, &jobs[index], &cancel, written);
                    if send.send((index, result)).is_err() {
                        return;
                    }
                }
            });
        }
        drop(send);
        let mut first = None;
        let mut finished = 0;
        for (index, result) in receive {
            if first.is_some() {
                continue;
            }
            let outcome = result.and_then(|md5| done(&jobs[index], md5));
            match outcome {
                Ok(()) => {
                    finished += 1;
                    progress(written.load(Ordering::Relaxed));
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
            None if finished < jobs.len() => Err(UpdateError::Canceled { journal: true }.into()),
            None => Ok(()),
        }
    })
}

/// A file being written, hashed as it goes.
struct Hashing<'a> {
    file: File,
    md5: Md5,
    size: u64,
    written: &'a AtomicU64,
}

impl Write for Hashing<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.file.write(bytes)?;
        self.md5.update(&bytes[..count]);
        self.size += count as u64;
        self.written.fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn write_one(
    client: &Client,
    access: &str,
    job: &Job,
    cancel: &dyn Fn() -> bool,
    written: &AtomicU64,
) -> Result<String> {
    let path = &job.file.path;
    let dir = open_dir(job.root, &job.parent)?;
    let fd = rustix::fs::openat(
        &dir,
        job.name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o644),
    )
    .with_context(|| format!("failed to create a file for {path}"))?;
    // The umask may have narrowed it; Wine ignores executable bits anyway.
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o644))
        .with_context(|| format!("failed to set {path}'s mode"))?;
    let mut out = Hashing {
        file: File::from(fd),
        md5: Md5::new(),
        size: 0,
        written,
    };
    client
        .write_file(job.links, access, &depot_file(job.file), &mut out, cancel)
        .map_err(|error| -> anyhow::Error {
            if error.kind() == wineo_gog::ErrorKind::Canceled {
                UpdateError::Canceled { journal: true }.into()
            } else {
                anyhow::Error::from(error).context(format!("Couldn't download {path}"))
            }
        })?;
    if out.size != job.file.size {
        anyhow::bail!("Couldn't download {path}: it isn't the size GOG lists");
    }
    out.file
        .sync_all()
        .with_context(|| format!("failed to sync {path}"))?;
    Ok(crate::units::hex(out.md5.finalize()))
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
