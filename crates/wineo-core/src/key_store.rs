//! Where an API key is kept and how it is found: an environment variable,
//! then the keyring, then a private file. Each service describes itself with
//! a `KeySpec` and shares the rest.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::{fmt, fmt::Formatter};

use crate::error::{Error, IoContext, Result};
use crate::files::{self, write_unique_private};
use crate::paths::Paths;

/// Where one service's key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    /// Overrides every saved key, for scripts and CI. None for secrets that
    /// change as they are used, which a variable can't keep up with.
    pub env_var: Option<&'static str>,
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

/// Looks for the key in the spec's variable, if it has one, then the keyring,
/// then the key file. An empty value counts as unset.
pub fn find_key(
    spec: &KeySpec,
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Option<Key>> {
    if let Some(value) = spec
        .env_var
        .and_then(&lookup)
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
            remove_key_file(&path)?;
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
        file: remove_key_file(&key_file(spec, paths))?,
    })
}

fn remove_file(path: &Path) -> Result<bool> {
    files::remove_if_present(path).context(|| format!("failed to remove {}", path.display()))
}

/// Deletes the key file and any temp file a crashed save left beside it,
/// since those hold the key too. Returns whether the key file itself existed.
/// A leftover that can't be deleted is an error, because the point is to get
/// the secret off disk. Only names that exactly match a temp name are touched.
fn remove_key_file(path: &Path) -> Result<bool> {
    let existed = remove_file(path)?;
    let (Some(dir), Some(name)) = (
        path.parent(),
        path.file_name().and_then(|name| name.to_str()),
    ) else {
        return Ok(existed);
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(existed),
        Err(err) => return Err(err).context(|| format!("failed to read {}", dir.display())),
    };
    for entry in entries {
        let entry = entry.context(|| format!("failed to read {}", dir.display()))?;
        let is_leftover = entry
            .file_name()
            .to_str()
            .is_some_and(|candidate| is_temp_name(name, candidate));
        if is_leftover && !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            remove_file(&entry.path())?;
        }
    }
    Ok(existed)
}

/// `<name>.<pid>-<n>.tmp` from `write_unique_private`, or the fixed
/// `.<name>.tmp` older versions used.
fn is_temp_name(name: &str, candidate: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if candidate
        .strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(".tmp"))
        == Some(name)
    {
        return true;
    }
    candidate
        .strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .and_then(|ids| ids.split_once('-'))
        .is_some_and(|(pid, counter)| digits(pid) && digits(counter))
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
        // A umask may narrow the mode further, but never open it to others.
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
        assert_eq!(mode & 0o600, 0o600, "{mode:o}");
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

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn removing_the_key_file_sweeps_crash_leftovers_and_the_legacy_temp() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("key");
        for name in ["key", "key.123-0.tmp", "key.4-56.tmp", ".key.tmp"] {
            fs::write(root.path().join(name), "secret").unwrap();
        }
        assert!(remove_key_file(&path).unwrap());
        assert!(names(root.path()).is_empty());
    }

    #[test]
    fn removing_the_key_file_keeps_names_that_are_not_temp_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("key");
        let kept = [
            "key.tmp",
            "key.1-2.tmp.bak",
            "key.1-2.TMP",
            "key.-2.tmp",
            "key.1-.tmp",
            "key.1-2-3.tmp",
            "key.a-2.tmp",
            "key.+1-2.tmp",
            "key2.1-2.tmp",
            "other.1-2.tmp",
            "..key.tmp",
            ".other.tmp",
            "x.key.1-2.tmp",
        ];
        for name in kept {
            fs::write(root.path().join(name), "x").unwrap();
        }
        fs::create_dir(root.path().join("key.9-9.tmp")).unwrap();
        // No key file, so nothing was removed.
        assert!(!remove_key_file(&path).unwrap());
        let mut expected: Vec<_> = kept.iter().map(|name| (*name).to_owned()).collect();
        expected.push("key.9-9.tmp".to_owned());
        expected.sort();
        assert_eq!(names(root.path()), expected);
    }

    #[test]
    fn remove_key_sweeps_leftovers_beside_the_key_file() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            None,
        )
        .unwrap();
        let spec = KeySpec {
            env_var: Some("WINEO_TEST_NO_SUCH_VAR"),
            account: "test",
            file_name: "test-key",
            label: "Test",
        };
        let path = key_file(&spec, &paths);
        write_private(&path, "secret").unwrap();
        let dir = path.parent().unwrap();
        fs::write(dir.join("test-key.77-1.tmp"), "secret").unwrap();
        let removed = remove_key(&spec, &paths, &NoKeyring).unwrap();
        assert!(removed.file);
        assert!(!names(dir).iter().any(|name| name.starts_with("test-key")));
    }

    fn test_paths(root: &Path) -> Paths {
        let home = root.to_owned();
        Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            None,
        )
        .unwrap()
    }

    const SPEC: KeySpec = KeySpec {
        env_var: Some("WINEO_TEST_NO_SUCH_VAR"),
        account: "test",
        file_name: "test-key",
        label: "Test",
    };

    #[test]
    fn a_symlink_named_like_a_leftover_is_unlinked_and_its_target_kept() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("key");
        let target = root.path().join("elsewhere");
        fs::write(&target, "precious").unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("key.5-6.tmp")).unwrap();
        assert!(!remove_key_file(&path).unwrap());
        assert_eq!(names(root.path()), ["elsewhere"]);
        assert_eq!(fs::read_to_string(&target).unwrap(), "precious");
    }

    #[test]
    fn leftovers_are_swept_when_the_key_file_is_already_gone() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let path = key_file(&SPEC, &paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path.with_file_name("test-key.8-3.tmp"), "secret").unwrap();
        fs::write(path.with_file_name(".test-key.tmp"), "secret").unwrap();
        let removed = remove_key(&SPEC, &paths, &NoKeyring).unwrap();
        assert!(!removed.file);
        assert!(
            !names(path.parent().unwrap())
                .iter()
                .any(|n| n.contains("test-key"))
        );
    }

    struct AcceptingKeyring;

    impl Keyring for AcceptingKeyring {
        fn get(&self) -> Result<Option<String>, KeyringError> {
            Ok(None)
        }
        fn set(&self, _key: &str) -> Result<(), KeyringError> {
            Ok(())
        }
        fn delete(&self) -> Result<bool, KeyringError> {
            Ok(false)
        }
    }

    #[test]
    fn migrating_to_the_keyring_sweeps_the_key_file_and_its_leftovers() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let path = key_file(&SPEC, &paths);
        write_private(&path, "old").unwrap();
        fs::write(path.with_file_name("test-key.8-3.tmp"), "old").unwrap();
        fs::write(path.with_file_name(".test-key.tmp"), "old").unwrap();
        let saved = save_key(&SPEC, &paths, &AcceptingKeyring, "new").unwrap();
        assert_eq!(saved, Saved::Keyring);
        assert!(
            !names(path.parent().unwrap())
                .iter()
                .any(|n| n.contains("test-key"))
        );
    }

    #[test]
    fn a_leftover_that_cannot_be_deleted_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("keys");
        fs::create_dir(&dir).unwrap();
        let leftover = dir.join("key.5-6.tmp");
        fs::write(&leftover, "secret").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        // Root ignores directory permissions, so there is nothing to test.
        let can_delete_anyway = fs::write(dir.join("probe"), "").is_ok();
        let result = remove_key_file(&dir.join("key"));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        if can_delete_anyway {
            return;
        }
        let error = result.unwrap_err().to_string();
        assert!(
            error.starts_with(&format!("failed to remove {}", leftover.display())),
            "{error}"
        );
        assert!(leftover.exists());
    }
}
