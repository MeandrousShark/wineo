//! The SteamGridDB API key: where it is kept, how it is found, and checking
//! it against the API.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::bail;

use crate::download::Transport;
use crate::error::{Error, IoContext, Result};
use crate::paths::Paths;

pub const API: &str = "https://www.steamgriddb.com/api/v2";

/// Overrides every saved key, for scripts and CI.
pub const KEY_VAR: &str = "WINEO_STEAMGRIDDB_KEY";

const KEY_FILE: &str = "steamgriddb.key";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub value: String,
    pub source: KeySource,
}

/// The plain text key file, always on this machine, never in a portable folder.
pub fn key_file(paths: &Paths) -> PathBuf {
    paths.host_data().join(KEY_FILE)
}

/// Looks for the key in `WINEO_STEAMGRIDDB_KEY`, then the keyring, then the
/// key file. An empty value counts as unset.
pub fn find_key(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Option<Key>> {
    if let Some(value) = lookup(KEY_VAR)
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
    let path = key_file(paths);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(clean(text).map(|value| Key {
            value,
            source: KeySource::File,
        })),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).context(|| format!("failed to read {}", path.display())),
    }
}

/// Like `find_key`, but a missing key is an error that says how to add one.
pub fn require_key(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Key> {
    find_key(paths, lookup, keyring)?.ok_or(Error::NoSteamGridDbKey)
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
pub fn save_key(paths: &Paths, keyring: &dyn Keyring, key: &str) -> Result<Saved> {
    let path = key_file(paths);
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
pub fn remove_key(paths: &Paths, keyring: &dyn Keyring) -> Result<Removed> {
    let from_keyring = match keyring.delete() {
        Ok(deleted) => deleted,
        Err(KeyringError::Unavailable) => false,
        Err(KeyringError::Failed(reason)) => return Err(Error::Keyring(reason)),
    };
    Ok(Removed {
        keyring: from_keyring,
        file: remove_file(&key_file(paths))?,
    })
}

fn remove_file(path: &std::path::Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).context(|| format!("failed to remove {}", path.display())),
    }
}

/// Writes through a temp file created `0600`, so the key is never readable by
/// anyone else, not even for a moment.
fn write_private(path: &std::path::Path, key: &str) -> Result<()> {
    let dir = path.parent().expect("the key file has a parent");
    fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
    let tmp = dir.join(format!(".{KEY_FILE}.tmp"));
    remove_file(&tmp)?;
    let context = || format!("failed to write {}", path.display());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .context(context)?;
    file.write_all(format!("{key}\n").as_bytes())
        .and_then(|()| file.sync_all())
        .context(context)?;
    fs::rename(&tmp, path).context(context)
}

/// Whether SteamGridDB accepts `key`, from one authenticated request.
pub fn check_key(transport: &dyn Transport, key: &str) -> anyhow::Result<bool> {
    let response = transport.get_bearer(&format!("{API}/search/autocomplete/wineo"), key)?;
    match response.status {
        200 => Ok(true),
        401 | 403 => Ok(false),
        status => bail!("SteamGridDB answered with status {status}"),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::download::Response;

    /// A keyring in memory, or one that is unavailable or broken.
    struct FakeKeyring {
        key: RefCell<Option<String>>,
        error: Option<KeyringError>,
    }

    impl FakeKeyring {
        fn with(key: Option<&str>) -> Self {
            Self {
                key: RefCell::new(key.map(str::to_owned)),
                error: None,
            }
        }

        fn failing(error: KeyringError) -> Self {
            Self {
                key: RefCell::new(None),
                error: Some(error),
            }
        }

        fn check(&self) -> Result<(), KeyringError> {
            self.error.clone().map_or(Ok(()), Err)
        }
    }

    impl Keyring for FakeKeyring {
        fn get(&self) -> Result<Option<String>, KeyringError> {
            self.check()?;
            Ok(self.key.borrow().clone())
        }

        fn set(&self, key: &str) -> Result<(), KeyringError> {
            self.check()?;
            *self.key.borrow_mut() = Some(key.to_owned());
            Ok(())
        }

        fn delete(&self) -> Result<bool, KeyringError> {
            self.check()?;
            Ok(self.key.borrow_mut().take().is_some())
        }
    }

    fn setup() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            Some(&root.path().join("usb")),
        )
        .unwrap();
        (root, paths)
    }

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), OsString::from(value)))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn found(paths: &Paths, env: &[(&str, &str)], keyring: &dyn Keyring) -> Option<Key> {
        find_key(paths, vars(env), keyring).unwrap()
    }

    #[test]
    fn the_key_file_is_on_the_host_even_in_a_portable_folder() {
        let (root, paths) = setup();
        assert!(paths.portable_root().is_some());
        assert_eq!(
            key_file(&paths),
            root.path().join(".local/share/wineo/steamgriddb.key")
        );
    }

    #[test]
    fn lookup_tries_the_variable_then_the_keyring_then_the_file() {
        let (_root, paths) = setup();
        let file = key_file(&paths);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "from-file\n").unwrap();
        let keyring = FakeKeyring::with(Some("from-keyring"));
        let key = |value: &str, source| {
            Some(Key {
                value: value.to_owned(),
                source,
            })
        };

        assert_eq!(
            found(&paths, &[(KEY_VAR, " from-var ")], &keyring),
            key("from-var", KeySource::Variable)
        );
        assert_eq!(
            found(&paths, &[(KEY_VAR, "")], &keyring),
            key("from-keyring", KeySource::Keyring)
        );
        assert_eq!(
            found(&paths, &[], &FakeKeyring::with(None)),
            key("from-file", KeySource::File)
        );
        assert_eq!(
            found(
                &paths,
                &[],
                &FakeKeyring::failing(KeyringError::Unavailable)
            ),
            key("from-file", KeySource::File)
        );
        let err = find_key(
            &paths,
            vars(&[]),
            &FakeKeyring::failing(KeyringError::Failed("locked".into())),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Keyring(_)), "{err}");
    }

    #[test]
    fn a_missing_key_says_how_to_add_one() {
        let (_root, paths) = setup();
        let file = key_file(&paths);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "\n").unwrap();
        assert_eq!(found(&paths, &[], &FakeKeyring::with(Some(" "))), None);
        let err = require_key(&paths, vars(&[]), &NoKeyring).unwrap_err();
        assert_eq!(
            err.to_string(),
            "no SteamGridDB key, run `wineo art key` or set WINEO_STEAMGRIDDB_KEY"
        );
    }

    #[test]
    fn saving_prefers_the_keyring_and_drops_a_stale_file() {
        let (_root, paths) = setup();
        let file = key_file(&paths);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "old\n").unwrap();
        let keyring = FakeKeyring::with(None);
        assert_eq!(save_key(&paths, &keyring, "new").unwrap(), Saved::Keyring);
        assert_eq!(keyring.key.borrow().as_deref(), Some("new"));
        assert!(!file.exists());

        let err = save_key(
            &paths,
            &FakeKeyring::failing(KeyringError::Failed("denied".into())),
            "new",
        )
        .unwrap_err();
        assert!(matches!(err, Error::Keyring(_)), "{err}");
        assert!(!file.exists());
    }

    #[test]
    fn without_a_keyring_the_key_file_is_private() {
        let (_root, paths) = setup();
        let file = key_file(&paths);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "old\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            save_key(&paths, &NoKeyring, "abc123").unwrap(),
            Saved::File(file.clone())
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "abc123\n");
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
        assert_eq!(
            found(&paths, &[], &NoKeyring).unwrap().value,
            "abc123".to_owned()
        );
    }

    #[test]
    fn removal_deletes_from_both() {
        let (_root, paths) = setup();
        save_key(&paths, &NoKeyring, "abc").unwrap();
        let keyring = FakeKeyring::with(Some("abc"));
        assert_eq!(
            remove_key(&paths, &keyring).unwrap(),
            Removed {
                keyring: true,
                file: true
            }
        );
        assert_eq!(
            remove_key(&paths, &keyring).unwrap(),
            Removed {
                keyring: false,
                file: false
            }
        );
        assert_eq!(
            remove_key(&paths, &NoKeyring).unwrap(),
            Removed {
                keyring: false,
                file: false
            }
        );
    }

    struct Answer {
        status: u16,
        seen: RefCell<Vec<(String, String)>>,
    }

    impl Transport for Answer {
        fn get(&self, url: &str) -> anyhow::Result<Box<dyn Read>> {
            bail!("unexpected unauthenticated request to {url}")
        }

        fn get_bearer(&self, url: &str, token: &str) -> anyhow::Result<Response> {
            self.seen
                .borrow_mut()
                .push((url.to_owned(), token.to_owned()));
            Ok(Response {
                status: self.status,
                body: Box::new(io::empty()),
            })
        }
    }

    #[test]
    fn a_key_is_checked_with_one_authenticated_request() {
        let answer = |status| Answer {
            status,
            seen: RefCell::new(Vec::new()),
        };
        let ok = answer(200);
        assert!(check_key(&ok, "abc").unwrap());
        assert_eq!(
            ok.seen.into_inner(),
            [(format!("{API}/search/autocomplete/wineo"), "abc".to_owned())]
        );
        assert!(!check_key(&answer(401), "abc").unwrap());
        assert!(!check_key(&answer(403), "abc").unwrap());
        let err = check_key(&answer(500), "abc").unwrap_err();
        assert_eq!(err.to_string(), "SteamGridDB answered with status 500");
    }
}
