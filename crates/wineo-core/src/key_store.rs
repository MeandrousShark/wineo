//! Where an API key is kept and how it is found: an environment variable,
//! then the keyring, then a private file. Each service describes itself with
//! a `KeySpec` and shares the rest.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::{fmt, fmt::Formatter};

use crate::error::{Error, IoContext, Result};
use crate::files::write_unique_private;
use crate::paths::Paths;

/// Where one service's key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    /// Overrides every saved key, for scripts and CI.
    pub env_var: &'static str,
    /// The `account` attribute of the keyring item.
    pub account: &'static str,
    /// The plain text fallback, under the host data dir.
    pub file_name: &'static str,
    /// The keyring item's human readable label.
    pub label: &'static str,
}

/// Why the keyring could not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyringError {
    /// There is no keyring service to ask, so the key file stands in.
    Unavailable,
    /// The keyring is there but failed, which is reported rather than
    /// quietly falling back to plain text.
    Failed(String),
}

/// Where the key is kept when a keyring service is running. Behind a trait so
/// tests never touch a real keyring.
pub trait Keyring {
    fn get(&self) -> Result<Option<String>, KeyringError>;
    fn set(&self, key: &str) -> Result<(), KeyringError>;
    /// Returns whether there was a key to delete.
    fn delete(&self) -> Result<bool, KeyringError>;
}

/// A keyring that is never available, so the key file is always used.
pub struct NoKeyring;

impl Keyring for NoKeyring {
    fn get(&self) -> Result<Option<String>, KeyringError> {
        Err(KeyringError::Unavailable)
    }

    fn set(&self, _key: &str) -> Result<(), KeyringError> {
        Err(KeyringError::Unavailable)
    }

    fn delete(&self) -> Result<bool, KeyringError> {
        Err(KeyringError::Unavailable)
    }
}

/// Where a found key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Variable,
    Keyring,
    File,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Key {
    pub value: String,
    pub source: KeySource,
}

impl fmt::Debug for Key {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Key")
            .field("value", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

/// The plain text key file, always on this machine, never in a portable folder.
pub fn key_file(spec: &KeySpec, paths: &Paths) -> PathBuf {
    paths.host_data().join(spec.file_name)
}

/// Looks for the key in the spec's variable, then the keyring, then the key
/// file. An empty value counts as unset.
pub fn find_key(
    spec: &KeySpec,
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Option<Key>> {
    if let Some(value) = lookup(spec.env_var)
        .and_then(|value| value.into_string().ok())
        .and_then(clean)
    {
        return Ok(Some(Key {
            value,
            source: KeySource::Variable,
        }));
    }
    match keyring.get() {
        Ok(Some(value)) => {
            if let Some(value) = clean(value) {
                return Ok(Some(Key {
                    value,
                    source: KeySource::Keyring,
                }));
            }
        }
        Ok(None) | Err(KeyringError::Unavailable) => {}
        Err(KeyringError::Failed(reason)) => return Err(Error::Keyring(reason)),
    }
    let path = key_file(spec, paths);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(clean(text).map(|value| Key {
            value,
            source: KeySource::File,
        })),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).context(|| format!("failed to read {}", path.display())),
    }
}

fn clean(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Where `save_key` put the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    Keyring,
    /// No keyring was available, so it is in this plain text file.
    File(PathBuf),
}

/// Saves the key to the keyring, or to the `0600` key file when there is no
/// keyring. A key file left from before is removed once the keyring has the
/// key, so no stale copy stays in plain text.
pub fn save_key(spec: &KeySpec, paths: &Paths, keyring: &dyn Keyring, key: &str) -> Result<Saved> {
    let path = key_file(spec, paths);
    match keyring.set(key) {
        Ok(()) => {
            remove_file(&path)?;
            Ok(Saved::Keyring)
        }
        Err(KeyringError::Unavailable) => {
            write_private(&path, key)?;
            Ok(Saved::File(path))
        }
        Err(KeyringError::Failed(reason)) => Err(Error::Keyring(reason)),
    }
}

/// What `remove_key` deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Removed {
    pub keyring: bool,
    pub file: bool,
}

/// Deletes the key from the keyring and the key file.
pub fn remove_key(spec: &KeySpec, paths: &Paths, keyring: &dyn Keyring) -> Result<Removed> {
    let from_keyring = match keyring.delete() {
        Ok(deleted) => deleted,
        Err(KeyringError::Unavailable) => false,
        Err(KeyringError::Failed(reason)) => return Err(Error::Keyring(reason)),
    };
    Ok(Removed {
        keyring: from_keyring,
        file: remove_file(&key_file(spec, paths))?,
    })
}

fn remove_file(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).context(|| format!("failed to remove {}", path.display())),
    }
}

/// Writes through a temp file created `0600`, so the key is never readable by
/// anyone else, not even for a moment. Each save has its own temp file, so
/// concurrent saves can't publish or delete each other's.
fn write_private(path: &Path, key: &str) -> Result<()> {
    let dir = path.parent().expect("the key file has a parent");
    fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
    // Older versions wrote to this one fixed name and could leave it behind
    // after a crash. Unique temp names are never reused, so nothing else
    // would ever clear it.
    let name = path.file_name().expect("the key file has a name");
    let mut legacy = OsString::from(".");
    legacy.push(name);
    legacy.push(".tmp");
    remove_file(&dir.join(legacy))?;
    write_unique_private(path, format!("{key}\n").as_bytes())
        .context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn concurrent_saves_leave_one_complete_private_file_and_no_temp_files() {
        const THREADS: usize = 8;
        const ROUNDS: usize = 50;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("dir/key");
        let keys: Vec<String> = (0..THREADS)
            .map(|n| format!("key-{n}-").repeat(5_000))
            .collect();
        let barrier = std::sync::Barrier::new(THREADS);
        std::thread::scope(|scope| {
            for key in &keys {
                let (path, barrier) = (&path, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    for _ in 0..ROUNDS {
                        write_private(path, key).unwrap();
                    }
                });
            }
        });
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            keys.iter().any(|key| text == format!("{key}\n")),
            "the file is not any one save"
        );
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn a_save_clears_the_legacy_fixed_temp_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("key");
        fs::write(root.path().join(".key.tmp"), "stale").unwrap();
        write_private(&path, "secret").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "secret\n");
        assert!(!root.path().join(".key.tmp").exists());
    }
}
