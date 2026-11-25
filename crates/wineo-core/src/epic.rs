//! Epic Games through Legendary, run as an outside program. This module is
//! the only code that starts Legendary or reads what it prints. Legendary
//! exits 0 after many failures, so nothing here trusts its exit code alone:
//! a code that isn't 0 is always a failure, and every call is checked
//! afterwards from what it printed or left behind.

pub mod account;
pub mod legendary;
pub mod own_copy;
pub mod owned;
mod run;

#[cfg(test)]
pub(crate) mod fake;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, flock};

use crate::error::IoContext;
use crate::paths::Paths;

/// The oldest Legendary wineo runs: the version whose source and output
/// this module was written against.
pub const FLOOR: Version = Version::new(0, 21, 1);

/// Legendary's own floor for the Python its release file runs on.
pub const PYTHON_FLOOR: (u32, u32) = (3, 10);

/// Under every message that gives up on wineo's own copy.
pub const INSTALL_HINT: &str =
    "Install Legendary yourself and set [epic] legendary, or update wineo.";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("[epic] legendary is set to {shown}, which can't be used: {reason}.")]
    Setting { shown: String, reason: String },

    #[error(
        "[epic] legendary is set to {shown}, which is Legendary {found}. wineo needs {FLOOR} or newer."
    )]
    SettingTooOld { shown: String, found: Version },

    #[error("Epic needs Python 3.10 or newer to run Legendary.")]
    Python,

    #[error(
        "wineo can't download Legendary for this machine. Install Legendary yourself and set [epic] legendary."
    )]
    Unsupported,

    #[error(
        "Couldn't get Legendary {version}: the download doesn't match what this wineo expects.\n{INSTALL_HINT}"
    )]
    Mismatch { version: String },

    #[error("Couldn't get Legendary {version}: the download is no longer there.\n{INSTALL_HINT}")]
    Gone { version: String },

    #[error(
        "Couldn't get Legendary {version}: the download ended after {received} of {size} bytes."
    )]
    Short {
        version: String,
        received: u64,
        size: u64,
    },

    #[error("Couldn't get Legendary {version}: {reason}")]
    Download { version: String, reason: String },

    /// Exited with a code that isn't 0, or couldn't be run at all.
    #[error("`legendary {command}` failed: {detail}")]
    Failed { command: String, detail: String },

    #[error("`legendary {command}` didn't finish in {seconds} seconds.")]
    TimedOut { command: String, seconds: u64 },

    /// Exited 0, but printed something wineo can't read.
    #[error("Couldn't read what `legendary {command}` printed: {problem}.")]
    Output { command: String, problem: String },

    #[error("Epic didn't accept that login{}. Log in again and paste the new code.", .detail.as_ref().map(|detail| format!(" ({detail})")).unwrap_or_default())]
    LoginRejected { detail: Option<String> },

    #[error("Couldn't log out of Epic: Legendary still has {account} logged in.")]
    StillLoggedIn { account: String },

    #[error("That isn't an Epic login code. Paste the code, or the whole page Epic showed.")]
    NotACode,

    #[error("Not logged in to Epic. Log in with wineo import epic --login.")]
    LoggedOut,

    #[error("Couldn't get owned Epic games: {0}")]
    Owned(Box<Error>),

    #[error(transparent)]
    Core(#[from] crate::Error),
}

impl Error {
    /// Whether this is a canceled call rather than a failed one.
    pub fn is_canceled(&self) -> bool {
        matches!(self, Self::Core(crate::Error::Canceled(_)))
    }
}

/// A Legendary or Python version, compared by its numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// `X.Y` or `X.Y.Z`, with anything after the numbers, like `rc1` or
    /// `+local`, ignored.
    pub fn parse(text: &str) -> Option<Self> {
        let mut numbers = text.trim().split('.').map(|part| {
            let digits = part.bytes().take_while(u8::is_ascii_digit).count();
            (digits > 0).then(|| part[..digits].parse().ok()).flatten()
        });
        let major = numbers.next()??;
        let minor = numbers.next()??;
        let patch = numbers.next().flatten().unwrap_or(0);
        Some(Self::new(major, minor, patch))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The environment Legendary's process is given, taken from wineo's own
/// once and never written back to it.
#[derive(Clone)]
pub struct Env(Vec<(OsString, OsString)>);

impl Env {
    /// wineo's own environment.
    pub fn current() -> Self {
        Self(std::env::vars_os().collect())
    }

    #[cfg(test)]
    pub(crate) fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Self(
            pairs
                .iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value)))
                .collect(),
        )
    }

    fn get(&self, key: &str) -> Option<&OsStr> {
        self.0
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_os_str())
    }

    /// `name` on this environment's `PATH`, the way a shell would find it.
    fn find_program(&self, name: &str) -> Option<PathBuf> {
        crate::runner::find_in_path(self.get("PATH").unwrap_or_default(), name)
    }

    /// The variables for Legendary's process: Legendary's and Python's own
    /// cleared, since they change what Legendary does and prints, its state
    /// folder set, and its cache moved into wineo's Epic folder so the
    /// release file unpacks there. Locale, proxy, and certificate variables
    /// pass through.
    fn for_legendary(&self, paths: &Paths) -> Vec<(OsString, OsString)> {
        let cleared = |name: &OsStr| {
            let name = name.as_encoded_bytes();
            [
                b"LGDRY_".as_slice(),
                b"LEGENDARY_".as_slice(),
                b"PYTHON".as_slice(),
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
                || name == b"XDG_CACHE_HOME"
        };
        let mut vars: Vec<_> = self
            .0
            .iter()
            .filter(|(name, _)| !cleared(name))
            .cloned()
            .collect();
        vars.push((
            "LEGENDARY_CONFIG_PATH".into(),
            state_dir(paths).into_os_string(),
        ));
        vars.push(("XDG_CACHE_HOME".into(), cache_dir(paths).into_os_string()));
        vars
    }
}

/// Only the variable names: values can hold anything.
impl fmt::Debug for Env {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(name, _)| name))
            .finish()
    }
}

/// Legendary's state folder: its login, its records, its `config.ini`.
pub fn state_dir(paths: &Paths) -> PathBuf {
    paths.epic_dir().join("legendary")
}

/// What Legendary's process gets as `XDG_CACHE_HOME`.
pub fn cache_dir(paths: &Paths) -> PathBuf {
    paths.epic_dir().join("cache")
}

/// Where wineo keeps its own copies of Legendary.
pub fn bin_dir(paths: &Paths) -> PathBuf {
    paths.epic_dir().join("bin")
}

/// Creates `dir` and any missing parents as `0700`. Folders that already
/// exist keep their mode.
fn private_dir(dir: &Path) -> crate::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .context(|| format!("failed to create {}", dir.display()))
}

/// Holds the host-wide Epic lock until dropped.
pub struct EpicLock {
    file: File,
}

impl EpicLock {
    /// Waits for whoever holds it now.
    pub fn acquire(paths: &Paths) -> crate::Result<Self> {
        let path = paths.epic_lock();
        let dir = path.parent().expect("the lock has a parent");
        fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .context(|| format!("failed to open {}", path.display()))?;
        flock(&file, FlockOperation::LockExclusive)
            .map_err(std::io::Error::from)
            .context(|| format!("failed to lock {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for EpicLock {
    fn drop(&mut self) {
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

#[cfg(test)]
mod tests;
