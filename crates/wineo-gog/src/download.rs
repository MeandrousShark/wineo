//! Downloading installer files into the cache, with resume and
//! verification.
//!
//! A file's `downlink` is resolved with the access token into a signed
//! download link, which is fetched without it. Links expire, so every
//! attempt resolves a fresh one and none is ever stored. A partial file is
//! resumed with a Range request, and every finished file is checked whole
//! against its size, and its MD5 when GOG publishes one, before it gets its
//! real name.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use reqwest::Url;
use reqwest::header::{CONTENT_RANGE, RANGE};

use crate::cache::{
    self, CHUNK, DownloadCache, Package, Record, disk, part_path, record_path, remove,
};
use crate::checksum;
use crate::client::{self, Client};
use crate::error::{Endpoint, Error, ErrorKind};

/// GOG's checksum documents list a hash per 10 MB chunk, so even huge
/// files have small ones.
const MAX_CHECKSUM: u64 = 4 * 1024 * 1024;

/// How a finished file was checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// Its size, and this MD5 from GOG, in lowercase hex.
    Md5(String),
    /// Only its size: GOG's checksum couldn't be had, which callers should
    /// say.
    SizeOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    pub path: PathBuf,
    pub size: u64,
    pub verification: Verification,
    /// Whether the file was already in the cache, so nothing was fetched.
    pub reused: bool,
}

/// Why one attempt stopped.
enum Stop {
    /// The download server refused the link with this status, which most
    /// likely means it expired.
    Refused(u16),
    /// The answer wasn't the range asked for. The partial file and its
    /// record are already gone, so the next attempt starts from zero.
    WrongRange,
    Failed(Error),
}

impl From<Error> for Stop {
    fn from(error: Error) -> Self {
        Self::Failed(error)
    }
}

impl Client {
    /// Downloads an installer file into the cache, or finds it there.
    ///
    /// `downlink` is the file's link from the product details, resolved
    /// with `access_token`. A partial file from an earlier attempt is
    /// resumed. `cancel` is checked between chunks, and a canceled download
    /// keeps its partial file for next time. `progress` gets the bytes
    /// there so far and the file's size.
    ///
    /// A 401 from resolving the link is `ErrorKind::Unauthorized`, so the
    /// caller can renew the token and call again to resume. A download
    /// server refusing the link is resolved again once, then fails with its
    /// status, never `Unauthorized`. A server answering with the wrong
    /// range gets one more try from zero.
    ///
    /// Holds the product's cache lock throughout, so it waits for any other
    /// process downloading or removing the same product's files. `cancel`
    /// is checked while it waits too.
    pub fn download(
        &self,
        access_token: &str,
        cache: &DownloadCache,
        package: &Package<'_>,
        downlink: &str,
        cancel: &dyn Fn() -> bool,
        progress: &dyn Fn(u64, u64),
    ) -> Result<Downloaded, Error> {
        let folder = cache.folder(package, Endpoint::Download)?;
        if package.size == 0 {
            return Err(Error::new(Endpoint::Download, ErrorKind::Malformed));
        }
        let _lock = cache.lock(package.product_id, cancel, Endpoint::Download)?;
        if let Some((path, md5)) =
            cache::reusable(&folder, package, None, cancel, Endpoint::Download)?
        {
            let name = path.file_name().and_then(|name| name.to_str());
            if let Some(name) = name {
                remove(&part_path(&folder, name), Endpoint::Download)?;
            }
            // An earlier call may have stopped between its rename and its
            // syncs, so nothing here is assumed to be on disk yet.
            File::open(&path)
                .and_then(|file| file.sync_all())
                .map_err(disk(Endpoint::Download))?;
            sync_folders(cache, &folder)?;
            progress(package.size, package.size);
            return Ok(Downloaded {
                path,
                size: package.size,
                verification: md5.map_or(Verification::SizeOnly, Verification::Md5),
                reused: true,
            });
        }
        let (mut retried, mut restarted) = (false, false);
        loop {
            match self.attempt(
                access_token,
                cache,
                &folder,
                package,
                downlink,
                cancel,
                progress,
            ) {
                Ok(downloaded) => return Ok(downloaded),
                Err(Stop::Refused(_)) if !retried => retried = true,
                Err(Stop::Refused(status)) => {
                    return Err(Error::new(Endpoint::Download, ErrorKind::Status(status)));
                }
                Err(Stop::WrongRange) if !restarted => restarted = true,
                Err(Stop::WrongRange) => {
                    return Err(Error::new(Endpoint::Download, ErrorKind::WrongRange));
                }
                Err(Stop::Failed(error)) => return Err(error),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn attempt(
        &self,
        access_token: &str,
        cache: &DownloadCache,
        folder: &Path,
        package: &Package<'_>,
        downlink: &str,
        cancel: &dyn Fn() -> bool,
        progress: &dyn Fn(u64, u64),
    ) -> Result<Downloaded, Stop> {
        let fail = |kind| Stop::Failed(Error::new(Endpoint::Download, kind));
        let disk = disk(Endpoint::Download);
        let size = package.size;
        if cancel() {
            return Err(fail(ErrorKind::Canceled));
        }

        let link = self.resolve_link(access_token, downlink)?;
        let malformed_link = || Error::new(Endpoint::Link, ErrorKind::Malformed);
        let url = Url::parse(&link.downlink).map_err(|_| malformed_link())?;
        let name = file_name(&url).ok_or_else(malformed_link)?;
        // Two files of one installer under one name would overwrite each
        // other in the folder.
        if cache::claimed_by_another(folder, &name, package.file_id) {
            return Err(fail(ErrorKind::Malformed));
        }
        let published = match &link.checksum {
            Some(url) => {
                let sum = self.checksum(url)?;
                if sum.total_size != size {
                    return Err(fail(ErrorKind::WrongSize));
                }
                Some(sum.md5)
            }
            None => None,
        };

        // A partial file is only kept for the same file, so the bytes
        // already there are known to belong to it.
        let record = match cache::read_record(folder, package.file_id) {
            Some(old)
                if old.size == size
                    && old.name == name
                    && (old.md5.is_none() || published.is_none() || old.md5 == published) =>
            {
                Record {
                    md5: published.or(old.md5),
                    ..old
                }
            }
            old => {
                if let Some(old) = old {
                    remove(&folder.join(&old.name), Endpoint::Download)?;
                    remove(&part_path(folder, &old.name), Endpoint::Download)?;
                }
                remove(&part_path(folder, &name), Endpoint::Download)?;
                Record {
                    file_id: package.file_id.to_owned(),
                    name,
                    size,
                    md5: published,
                }
            }
        };
        fs::create_dir_all(folder).map_err(disk)?;
        cache::write_record(folder, &record)?;

        let part = part_path(folder, &record.name);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&part)
            .map_err(disk)?;
        let hasher = match self.transfer(&file, url, folder, &part, package, cancel, progress) {
            Ok(hasher) => hasher,
            Err(stop) => {
                // Whatever is kept for a resume should survive a crash too.
                let _ = file.sync_all();
                return Err(stop);
            }
        };
        file.sync_all().map_err(disk)?;
        drop(file);
        if let Some(md5) = &record.md5
            && cache::hex(hasher) != *md5
        {
            remove(&part, Endpoint::Download)?;
            remove(&record_path(folder, package.file_id), Endpoint::Download)?;
            return Err(fail(ErrorKind::WrongChecksum));
        }
        let path = folder.join(&record.name);
        fs::rename(&part, &path).map_err(disk)?;
        sync_folders(cache, folder)?;
        Ok(Downloaded {
            path,
            size,
            verification: record.md5.map_or(Verification::SizeOnly, Verification::Md5),
            reused: false,
        })
    }

    /// Fills the partial file, resuming it when it has bytes already, and
    /// returns the hash of the whole file.
    #[allow(clippy::too_many_arguments)]
    fn transfer(
        &self,
        mut file: &File,
        url: Url,
        folder: &Path,
        part: &Path,
        package: &Package<'_>,
        cancel: &dyn Fn() -> bool,
        progress: &dyn Fn(u64, u64),
    ) -> Result<Md5, Stop> {
        let fail = |kind| Stop::Failed(Error::new(Endpoint::Download, kind));
        let disk = disk(Endpoint::Download);
        let size = package.size;
        let mut hasher = Md5::new();
        let mut have = file.metadata().map_err(disk)?.len();
        if have > size {
            file.set_len(0).map_err(disk)?;
            have = 0;
        } else if have > 0 {
            cache::hash(file, &mut hasher, cancel, Endpoint::Download)?;
        }
        file.seek(SeekFrom::End(0)).map_err(disk)?;
        progress(have, size);
        if have == size {
            return Ok(hasher);
        }

        let mut request = self.cdn.get(url);
        if have > 0 {
            request = request.header(RANGE, format!("bytes={have}-"));
        }
        let mut response = request
            .send()
            .map_err(|error| fail(client::send_error(&error)))?;
        let status = response.status().as_u16();
        match status {
            200 => {
                // The server sent the whole file instead of the rest.
                if have > 0 {
                    file.set_len(0).map_err(disk)?;
                    file.seek(SeekFrom::Start(0)).map_err(disk)?;
                    hasher = Md5::new();
                    have = 0;
                    progress(0, size);
                }
                if response
                    .content_length()
                    .is_some_and(|length| length != size)
                {
                    return Err(fail(ErrorKind::WrongSize));
                }
            }
            206 if have > 0 => {
                let range = response
                    .headers()
                    .get(CONTENT_RANGE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(content_range);
                if range != Some((have, size - 1, size)) {
                    return Err(discard(folder, part, package.file_id)?);
                }
            }
            // A 416 says the server won't serve the range asked for, which
            // starting over sidesteps.
            206 | 416 => return Err(discard(folder, part, package.file_id)?),
            401 | 403 => return Err(Stop::Refused(status)),
            _ => {
                return Err(fail(client::status_kind(Endpoint::Download, status, None)));
            }
        }

        let mut buffer = vec![0; CHUNK];
        loop {
            if cancel() {
                return Err(fail(ErrorKind::Canceled));
            }
            let read = match response.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(fail(client::read_error(&error))),
            };
            if have + read as u64 > size {
                remove(part, Endpoint::Download)?;
                return Err(fail(ErrorKind::WrongSize));
            }
            file.write_all(&buffer[..read]).map_err(disk)?;
            hasher.update(&buffer[..read]);
            have += read as u64;
            progress(have, size);
        }
        // Ended cleanly but short: what's there is kept for a resume.
        if have < size {
            return Err(fail(ErrorKind::Body));
        }
        Ok(hasher)
    }

    /// GOG's checksum for a file, from the link GOG gave for it. A failure
    /// is tried once more, then fails the attempt: a checksum GOG publishes
    /// is never skipped. The signed link goes without the access token.
    fn checksum(&self, url: &str) -> Result<checksum::Checksum, Stop> {
        let fail = |kind| Stop::Failed(Error::new(Endpoint::Checksum, kind));
        let url = Url::parse(url).map_err(|_| fail(ErrorKind::Malformed))?;
        let mut tried = false;
        loop {
            let result = self
                .cdn
                .get(url.clone())
                .send()
                .map_err(|error| client::send_error(&error))
                .and_then(|response| match response.status().as_u16() {
                    200 => client::read_limited(response, MAX_CHECKSUM),
                    // Signed like the file's link, so it has likely expired
                    // with it.
                    status @ (401 | 403) => Err(ErrorKind::Status(status)),
                    status => Err(client::status_kind(Endpoint::Checksum, status, None)),
                })
                .and_then(|body| checksum::parse(&body).ok_or(ErrorKind::Malformed));
            match result {
                Ok(sum) => return Ok(sum),
                Err(ErrorKind::Status(status @ (401 | 403))) => {
                    return Err(Stop::Refused(status));
                }
                Err(_) if !tried => tried = true,
                Err(kind) => return Err(fail(kind)),
            }
        }
    }
}

/// Syncs a package's folder and each one above it up to the cache, any of
/// which may be new.
fn sync_folders(cache: &DownloadCache, folder: &Path) -> Result<(), Error> {
    cache::sync_dir(folder, Endpoint::Download)?;
    if let Some(product) = folder.parent() {
        cache::sync_dir(product, Endpoint::Download)?;
    }
    cache::sync_dir(cache.dir(), Endpoint::Download)
}

/// Drops a partial file the server won't resume properly, so the next
/// attempt starts from zero instead of failing the same way forever.
fn discard(folder: &Path, part: &Path, file_id: &str) -> Result<Stop, Error> {
    remove(part, Endpoint::Download)?;
    remove(&record_path(folder, file_id), Endpoint::Download)?;
    Ok(Stop::WrongRange)
}

/// The file's name: the link's last path segment, decoded.
fn file_name(url: &Url) -> Option<String> {
    let segment = url.path_segments()?.next_back()?;
    let name = percent_decode(segment)?;
    cache::valid_name(&name).then_some(name)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            decoded.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// `bytes START-END/TOTAL`, as numbers.
fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (range, total) = value.trim().strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let number = |text: &str| {
        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
    };
    Some((number(start)?, number(end)?, number(total)?))
}

#[cfg(test)]
mod tests;
