//! The download cache for installer files.
//!
//! Layout, under the cache directory:
//!
//! ```text
//! <product id>/<download id>/<file name>         a finished file
//! <product id>/<download id>/<file name>.part    a partial one
//! <product id>/<download id>/.<file id>.json     the file's record
//! ```
//!
//! The download id is the installer's or patch's, like
//! `installer_windows_en`, so every part of one installer (`setup_*.exe`
//! and its `.bin` files) lands in one folder under GOG's own names, which
//! is how the setup program finds them. The record names the file and its
//! expected size and MD5, so a file can be found by its GOG file id without
//! the network, and a partial download is only resumed for the same file.
//! One file id holds one file at a time: a different one replaces it.
//!
//! `<product id>/.lock` is held exclusively, across processes, for as long
//! as anything writes or removes the product's files. Reading the cache
//! takes no lock: a finished file is only ever replaced by a rename, so a
//! reader sees the old file or the new one, never half of either.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use md5::{Digest, Md5};
use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use crate::error::{Endpoint, Error, ErrorKind};
use crate::product::{Download, DownloadFile};

/// How long a wait for a product's lock sleeps between tries.
const LOCK_RETRY: Duration = Duration::from_millis(100);

/// How much of a file is read or written at once.
pub(crate) const CHUNK: usize = 256 * 1024;

/// One installer file, by where it belongs in the cache and its size from
/// the product details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Package<'a> {
    pub product_id: u64,
    /// The installer's or patch's id, like `installer_windows_en`.
    pub download_id: &'a str,
    /// GOG's file id, like `en1installer0`.
    pub file_id: &'a str,
    pub size: u64,
}

impl<'a> Package<'a> {
    pub fn of(product_id: u64, download: &'a Download, file: &'a DownloadFile) -> Self {
        Self {
            product_id,
            download_id: &download.id,
            file_id: &file.id,
            size: file.size,
        }
    }
}

/// A file the cache holds, finished or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedFile {
    pub download_id: String,
    pub file_id: String,
    /// GOG's file name.
    pub name: String,
    /// Where the finished file is, or will be.
    pub path: PathBuf,
    pub size: u64,
    /// GOG's MD5 in lowercase hex, when it published one.
    pub md5: Option<String>,
    /// Whether the finished file is there with its full size. Its contents
    /// aren't checked; `DownloadCache::complete` does that.
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadCache {
    dir: PathBuf,
}

/// What the cache knows about one file id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Record {
    pub file_id: String,
    pub name: String,
    pub size: u64,
    pub md5: Option<String>,
}

impl DownloadCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The folder a package's files go in, once its ids are known to be
    /// safe as file names.
    pub(crate) fn folder(
        &self,
        package: &Package<'_>,
        endpoint: Endpoint,
    ) -> Result<PathBuf, Error> {
        if !valid_id(package.download_id) || !valid_id(package.file_id) {
            return Err(Error::new(endpoint, ErrorKind::Malformed));
        }
        Ok(self
            .product_dir(package.product_id)
            .join(package.download_id))
    }

    fn product_dir(&self, product_id: u64) -> PathBuf {
        self.dir.join(product_id.to_string())
    }

    /// Takes the product's lock, waiting for whoever holds it. The wait is
    /// polled rather than blocking, so `cancel` still works while another
    /// process downloads a large file.
    pub(crate) fn lock(
        &self,
        product_id: u64,
        cancel: &dyn Fn() -> bool,
        endpoint: Endpoint,
    ) -> Result<ProductLock, Error> {
        let disk = disk(endpoint);
        let dir = self.product_dir(product_id);
        let path = dir.join(".lock");
        loop {
            fs::create_dir_all(&dir).map_err(disk)?;
            let file = match OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
            {
                Ok(file) => file,
                // Removed with its product between the two calls.
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(disk(error)),
            };
            loop {
                match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                    Ok(()) => break,
                    Err(Errno::WOULDBLOCK) => {
                        if cancel() {
                            return Err(Error::new(endpoint, ErrorKind::Canceled));
                        }
                        thread::sleep(LOCK_RETRY);
                    }
                    Err(Errno::INTR) => {}
                    Err(error) => return Err(disk(error.into())),
                }
            }
            let lock = ProductLock { file };
            // The holder before may have removed the product, lock file
            // and all, which leaves this lock on a file nobody else sees.
            let held = lock.file.metadata().map_err(disk)?;
            match fs::metadata(&path) {
                Ok(now) if now.dev() == held.dev() && now.ino() == held.ino() => return Ok(lock),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(disk(error)),
            }
        }
    }

    /// The finished file for a package, when it's in the cache with its
    /// size and an MD5 that matches. `md5`, like one a journal recorded,
    /// must match the cache's own when both are known; with neither, only
    /// the size is checked. Reads the whole file to hash it, checking
    /// `cancel` between chunks.
    pub fn complete(
        &self,
        package: &Package<'_>,
        md5: Option<&str>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Option<PathBuf>, Error> {
        let folder = self.folder(package, Endpoint::Cache)?;
        Ok(reusable(&folder, package, md5, cancel, Endpoint::Cache)?.map(|(path, _)| path))
    }

    /// Every file cached for a product, by download id and then file id.
    pub fn files(&self, product_id: u64) -> Result<Vec<CachedFile>, Error> {
        let disk = disk(Endpoint::Cache);
        let mut files = Vec::new();
        let downloads = match fs::read_dir(self.product_dir(product_id)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(files),
            Err(error) => return Err(disk(error)),
        };
        for download in downloads {
            let download = download.map_err(disk)?;
            let Some(download_id) = download.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !valid_id(&download_id) || !download.file_type().map_err(disk)?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(download.path()).map_err(disk)? {
                let entry = entry.map_err(disk)?;
                let Some(file_id) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix('.')?.strip_suffix(".json"))
                    .map(str::to_owned)
                else {
                    continue;
                };
                let Some(record) = read_record(&download.path(), &file_id) else {
                    continue;
                };
                let path = download.path().join(&record.name);
                let complete = fs::symlink_metadata(&path)
                    .is_ok_and(|meta| meta.is_file() && meta.len() == record.size);
                files.push(CachedFile {
                    download_id: download_id.clone(),
                    file_id,
                    name: record.name,
                    path,
                    size: record.size,
                    md5: record.md5,
                    complete,
                });
            }
        }
        files.sort_by(|a, b| (&a.download_id, &a.file_id).cmp(&(&b.download_id, &b.file_id)));
        Ok(files)
    }

    /// Removes everything cached for a product, finished or partial.
    /// Waits for a download of the product in another process to finish,
    /// checking `cancel` while it waits.
    pub fn remove_product(&self, product_id: u64, cancel: &dyn Fn() -> bool) -> Result<(), Error> {
        let _lock = self.lock(product_id, cancel, Endpoint::Cache)?;
        // The lock file goes too. Anyone waiting on it notices and locks
        // a new one.
        match fs::remove_dir_all(self.product_dir(product_id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                Err(disk(Endpoint::Cache)(error))
            }
            _ => Ok(()),
        }
    }
}

/// Holds a product's lock until dropped.
pub(crate) struct ProductLock {
    file: File,
}

impl Drop for ProductLock {
    fn drop(&mut self) {
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

/// The finished file and its MD5, when it can be used as it is.
pub(crate) fn reusable(
    folder: &Path,
    package: &Package<'_>,
    md5: Option<&str>,
    cancel: &dyn Fn() -> bool,
    endpoint: Endpoint,
) -> Result<Option<(PathBuf, Option<String>)>, Error> {
    let Some(record) = read_record(folder, package.file_id) else {
        return Ok(None);
    };
    if record.size != package.size {
        return Ok(None);
    }
    let md5 = match (md5.map(str::to_ascii_lowercase), record.md5) {
        (Some(given), Some(known)) if given != known => return Ok(None),
        (given, known) => given.or(known),
    };
    let path = folder.join(&record.name);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(disk(endpoint)(error)),
    };
    let meta = file.metadata().map_err(disk(endpoint))?;
    if !meta.is_file() || meta.len() != package.size {
        return Ok(None);
    }
    if let Some(md5) = &md5 {
        let mut hasher = Md5::new();
        hash(&mut file, &mut hasher, cancel, endpoint)?;
        if hex(hasher) != *md5 {
            return Ok(None);
        }
    }
    Ok(Some((path, md5)))
}

/// Feeds the rest of a file to `hasher`.
pub(crate) fn hash(
    file: &mut File,
    hasher: &mut Md5,
    cancel: &dyn Fn() -> bool,
    endpoint: Endpoint,
) -> Result<u64, Error> {
    let mut buffer = vec![0; CHUNK];
    let mut total = 0;
    loop {
        if cancel() {
            return Err(Error::new(endpoint, ErrorKind::Canceled));
        }
        let read = match file.read(&mut buffer) {
            Ok(0) => return Ok(total),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(disk(endpoint)(error)),
        };
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
}

pub(crate) fn hex(hasher: Md5) -> String {
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn record_path(folder: &Path, file_id: &str) -> PathBuf {
    folder.join(format!(".{file_id}.json"))
}

pub(crate) fn part_path(folder: &Path, name: &str) -> PathBuf {
    folder.join(format!("{name}.part"))
}

/// A record that can be trusted to name a file in its folder, or none.
pub(crate) fn read_record(folder: &Path, file_id: &str) -> Option<Record> {
    let text = fs::read(record_path(folder, file_id)).ok()?;
    let record: Record = serde_json::from_slice(&text).ok()?;
    let md5_ok = record.md5.as_ref().is_none_or(|md5| {
        md5.len() == 32
            && md5
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    (record.file_id == file_id && valid_name(&record.name) && record.size > 0 && md5_ok)
        .then_some(record)
}

/// Writes a record durably enough that it never names a file half-way: a
/// temporary file, synced, then renamed over the old one.
pub(crate) fn write_record(folder: &Path, record: &Record) -> Result<(), Error> {
    let disk = disk(Endpoint::Download);
    let path = record_path(folder, &record.file_id);
    let temporary = folder.join(format!(".{}.json.tmp", record.file_id));
    let text = serde_json::to_vec(record).expect("a record serializes");
    fs::write(&temporary, text).map_err(disk)?;
    File::open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(disk)?;
    fs::rename(&temporary, &path).map_err(disk)
}

/// Removes a file, which may already be gone.
pub(crate) fn remove(path: &Path, endpoint: Endpoint) -> Result<(), Error> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(disk(endpoint)(error)),
        _ => Ok(()),
    }
}

pub(crate) fn sync_dir(dir: &Path, endpoint: Endpoint) -> Result<(), Error> {
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(disk(endpoint))
}

pub(crate) fn disk(endpoint: Endpoint) -> impl Fn(io::Error) -> Error + Copy {
    move |error| Error::new(endpoint, ErrorKind::Disk(error.kind()))
}

/// GOG's download and file ids, like `installer_windows_en` and
/// `en1installer0`, used as folder and file names.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// A file name from GOG, kept as it is since setup programs find their
/// parts by name. Names that could be hidden, a partial file, or a path
/// are refused.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.ends_with(".part")
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}
