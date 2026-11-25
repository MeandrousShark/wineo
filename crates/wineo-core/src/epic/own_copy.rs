//! wineo's own copy of Legendary: one pinned release file, downloaded only
//! when it is exactly the size and hash this wineo names, and kept in the
//! host data dir as `epic/bin/legendary-VERSION`.

use std::fs::{self, File, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, flock};
use sha2::{Digest, Sha256};

use super::{EpicLock, Error};
use crate::cancel::Token;
use crate::download::{InstallLock, Transport};
use crate::error::IoContext;
use crate::paths::Paths;

/// One release file, named by everything that has to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin<'a> {
    pub version: &'a str,
    pub url: &'a str,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: &'a str,
}

/// The Legendary this wineo downloads: the version the probe ran, read
/// from the release file itself.
pub const PINNED: Pin<'static> = Pin {
    version: "0.21.1",
    url: "https://github.com/legendary-gl/legendary/releases/download/0.21.1/legendary_linux_x64",
    size: 3_093_622,
    sha256: "dbed33bbe96031e65858233e4badf0a1d401bb6cb0cc995d1237cf3a0c826a45",
};

/// Where wineo's copy of `version` lives. Never in a portable root.
pub fn file(paths: &Paths, version: &str) -> PathBuf {
    super::bin_dir(paths).join(format!("legendary-{version}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Downloaded(PathBuf),
    /// Someone else downloaded it first, or it was already there.
    AlreadyThere(PathBuf),
}

impl Fetched {
    pub fn path(&self) -> &Path {
        match self {
            Self::Downloaded(path) | Self::AlreadyThere(path) => path,
        }
    }
}

/// Downloads the pinned Legendary, for a user who agreed to it. Checks the
/// machine and its Python first, and removes older pinned copies once the
/// new one is in place.
pub fn get(paths: &Paths, token: &Token, progress: impl FnMut(u64, u64)) -> Result<Fetched, Error> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(Error::Unsupported);
    }
    super::legendary::python(token)?;
    #[cfg(debug_assertions)]
    if let Some(stand_in) = std::env::var_os("WINEO_TEST_LEGENDARY_FILE").filter(|v| !v.is_empty())
    {
        return get_stand_in(paths, Path::new(&stand_in), token, progress);
    }
    let transport = crate::download::Https::new().map_err(|error| Error::Download {
        version: PINNED.version.to_owned(),
        reason: crate::error::one_line(format!("{error:#}")),
    })?;
    let fetched = download(paths, &transport, &PINNED, token, progress)?;
    if matches!(fetched, Fetched::Downloaded(_)) {
        remove_older(paths, PINNED.version)?;
    }
    Ok(fetched)
}

/// A debug build's CLI tests hand over a local file to stand in for the
/// download, pinned to its own size and hash, so the real pin stays as it
/// is and a release build has no way around it.
#[cfg(debug_assertions)]
fn get_stand_in(
    paths: &Paths,
    stand_in: &Path,
    token: &Token,
    progress: impl FnMut(u64, u64),
) -> Result<Fetched, Error> {
    let bytes = fs::read(stand_in).context(|| format!("failed to read {}", stand_in.display()))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let pin = Pin {
        size: bytes.len() as u64,
        sha256: &sha256,
        ..PINNED
    };
    let fetched = download(paths, &LocalFile(bytes), &pin, token, progress)?;
    if matches!(fetched, Fetched::Downloaded(_)) {
        remove_older(paths, pin.version)?;
    }
    Ok(fetched)
}

#[cfg(debug_assertions)]
struct LocalFile(Vec<u8>);

#[cfg(debug_assertions)]
impl Transport for LocalFile {
    fn get(&self, _url: &str) -> anyhow::Result<Box<dyn Read>> {
        Ok(Box::new(std::io::Cursor::new(self.0.clone())))
    }

    fn request(
        &self,
        url: &str,
        _bearer: Option<&str>,
    ) -> anyhow::Result<crate::download::Response> {
        Ok(crate::download::Response {
            status: 200,
            body: self.get(url)?,
        })
    }
}

/// Downloads `pin` into place, keeping it only when it is exactly
/// `pin.size` bytes with `pin.sha256`. Holds the destination's lock
/// throughout, so a second caller waits and then finds the file, and
/// clears what an earlier run that died left behind.
pub fn download(
    paths: &Paths,
    transport: &dyn Transport,
    pin: &Pin<'_>,
    token: &Token,
    mut progress: impl FnMut(u64, u64),
) -> Result<Fetched, Error> {
    let fail = |reason: String| Error::Download {
        version: pin.version.to_owned(),
        reason,
    };
    let canceled = || Error::from(crate::Error::Canceled("Legendary download"));
    let dir = super::bin_dir(paths);
    super::private_dir(&dir)?;
    let destination = file(paths, pin.version);
    let _lock = InstallLock::acquire_waiting(&destination, token).map_err(|error| {
        if token.is_canceled() {
            canceled()
        } else {
            fail(crate::error::one_line(format!("{error:#}")))
        }
    })?;
    let prefix = part_prefix(pin.version);
    remove_parts(&dir, &prefix)?;
    if destination.is_file() {
        return Ok(Fetched::AlreadyThere(destination));
    }
    if token.is_canceled() {
        return Err(canceled());
    }
    let response = transport
        .request(pin.url, None)
        .map_err(|error| fail(crate::error::one_line(format!("{error:#}"))))?;
    match response.status {
        200 => {}
        404 | 410 => {
            return Err(Error::Gone {
                version: pin.version.to_owned(),
            });
        }
        status => return Err(fail(format!("the server answered {status}"))),
    }
    let mut part = tempfile::Builder::new()
        .prefix(&prefix)
        .permissions(Permissions::from_mode(0o700))
        .tempfile_in(&dir)
        .context(|| format!("failed to create a file in {}", dir.display()))?;
    let mut body = response.body;
    let mut sha256 = Sha256::new();
    let mut received = 0u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        if token.is_canceled() {
            return Err(canceled());
        }
        let count = match body.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(fail(format!("the download failed: {error}"))),
        };
        if count == 0 {
            break;
        }
        received += count as u64;
        // Longer than pinned is a different file, however it ends.
        if received > pin.size {
            return Err(Error::Mismatch {
                version: pin.version.to_owned(),
            });
        }
        part.write_all(&buffer[..count])
            .context(|| format!("failed to write {}", part.path().display()))?;
        sha256.update(&buffer[..count]);
        progress(received, pin.size);
    }
    if received < pin.size {
        return Err(Error::Short {
            version: pin.version.to_owned(),
            received,
            size: pin.size,
        });
    }
    if !format!("{:x}", sha256.finalize()).eq_ignore_ascii_case(pin.sha256) {
        return Err(Error::Mismatch {
            version: pin.version.to_owned(),
        });
    }
    part.as_file()
        .sync_all()
        .context(|| format!("failed to write {}", part.path().display()))?;
    let part = part.into_temp_path();
    crate::download::publish(&part, &destination).map_err(|error| fail(format!("{error:#}")))?;
    // Renamed away, so there is nothing left for the guard to delete.
    let _ = part.keep();
    crate::files::sync_dir(&dir).context(|| format!("failed to sync {}", dir.display()))?;
    Ok(Fetched::Downloaded(destination))
}

fn part_prefix(version: &str) -> String {
    format!(".legendary-{version}.part-")
}

/// Run under the destination's lock, so no live download owns these.
fn remove_parts(dir: &Path, prefix: &str) -> Result<(), Error> {
    let entries = fs::read_dir(dir).context(|| format!("failed to read {}", dir.display()))?;
    for entry in entries {
        let entry = entry.context(|| format!("failed to read {}", dir.display()))?;
        if entry.file_name().to_string_lossy().starts_with(prefix) {
            let path = entry.path();
            crate::files::remove_if_present(&path)
                .context(|| format!("failed to remove {}", path.display()))?;
        }
    }
    Ok(())
}

/// Removes pinned copies other than `keep`'s, under the Epic lock, each
/// only when no Legendary call is running from it. One in use is left for
/// the next time. Returns what was removed.
pub(crate) fn remove_older(paths: &Paths, keep: &str) -> Result<Vec<PathBuf>, Error> {
    let dir = super::bin_dir(paths);
    let keep = format!("legendary-{keep}");
    let _lock = EpicLock::acquire(paths)?;
    let mut removed = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(removed),
        Err(error) => {
            return Err(crate::Error::Io {
                context: format!("failed to read {}", dir.display()),
                source: error,
            }
            .into());
        }
    };
    for entry in entries {
        let entry = entry.context(|| format!("failed to read {}", dir.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("legendary-") || name == keep.as_str() {
            continue;
        }
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(opened) = File::open(&path) else {
            continue;
        };
        if flock(&opened, FlockOperation::NonBlockingLockExclusive).is_err() {
            continue;
        }
        crate::files::remove_if_present(&path)
            .context(|| format!("failed to remove {}", path.display()))?;
        removed.push(path);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests;
