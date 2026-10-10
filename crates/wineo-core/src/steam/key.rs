//! The Steam Web API key, kept beside the SteamGridDB one with the same read
//! order: `WINEO_STEAM_KEY`, then the keyring, then a private file.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::error::Result;
use crate::key_store::{self, Key, KeySpec, Keyring, Removed, Saved};
use crate::paths::Paths;

/// Overrides every saved key, for scripts and CI.
pub const KEY_VAR: &str = "WINEO_STEAM_KEY";

pub const KEY_SPEC: KeySpec = KeySpec {
    env_var: KEY_VAR,
    account: "steam",
    file_name: "steam.key",
    label: "wineo steam key",
};

/// The plain text key file, always on this machine, never in a portable folder.
pub fn key_file(paths: &Paths) -> PathBuf {
    key_store::key_file(&KEY_SPEC, paths)
}

/// Looks for the key in `WINEO_STEAM_KEY`, then the keyring, then the key
/// file. An empty value counts as unset.
pub fn find_key(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Option<Key>> {
    key_store::find_key(&KEY_SPEC, paths, lookup, keyring)
}

/// Saves the key to the keyring, or to the `0600` key file when there is no
/// keyring.
pub fn save_key(paths: &Paths, keyring: &dyn Keyring, key: &str) -> Result<Saved> {
    key_store::save_key(&KEY_SPEC, paths, keyring, key)
}

/// Deletes the key from the keyring and the key file.
pub fn remove_key(paths: &Paths, keyring: &dyn Keyring) -> Result<Removed> {
    key_store::remove_key(&KEY_SPEC, paths, keyring)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::key_store::{KeySource, KeyringError, NoKeyring};

    struct FakeKeyring(RefCell<Option<String>>);

    impl Keyring for FakeKeyring {
        fn get(&self) -> Result<Option<String>, KeyringError> {
            Ok(self.0.borrow().clone())
        }

        fn set(&self, key: &str) -> Result<(), KeyringError> {
            *self.0.borrow_mut() = Some(key.to_owned());
            Ok(())
        }

        fn delete(&self) -> Result<bool, KeyringError> {
            Ok(self.0.borrow_mut().take().is_some())
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

    fn env(value: &'static str) -> impl Fn(&str) -> Option<OsString> {
        move |name| (name == KEY_VAR).then(|| OsString::from(value))
    }

    #[test]
    fn the_variable_wins_then_the_keyring_then_the_file() {
        let (_root, paths) = setup();
        let keyring = FakeKeyring(RefCell::new(Some("from-keyring".to_owned())));
        fs::create_dir_all(key_file(&paths).parent().unwrap()).unwrap();
        fs::write(key_file(&paths), "from-file\n").unwrap();

        let key = find_key(&paths, env(" from-env "), &keyring)
            .unwrap()
            .unwrap();
        assert_eq!(
            (key.value.as_str(), key.source),
            ("from-env", KeySource::Variable)
        );

        let key = find_key(&paths, |_| None, &keyring).unwrap().unwrap();
        assert_eq!(
            (key.value.as_str(), key.source),
            ("from-keyring", KeySource::Keyring)
        );

        let key = find_key(&paths, |_| None, &NoKeyring).unwrap().unwrap();
        assert_eq!(
            (key.value.as_str(), key.source),
            ("from-file", KeySource::File)
        );

        // An empty variable counts as unset.
        let key = find_key(&paths, env("  "), &NoKeyring).unwrap().unwrap();
        assert_eq!(key.source, KeySource::File);
    }

    #[test]
    fn the_steamgriddb_variable_and_file_are_not_the_steam_ones() {
        let (_root, paths) = setup();
        let other = crate::steamgriddb::key_file(&paths);
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&other, "grid\n").unwrap();
        let lookup =
            |name: &str| (name == crate::steamgriddb::KEY_VAR).then(|| OsString::from("grid"));
        assert_eq!(find_key(&paths, lookup, &NoKeyring).unwrap(), None);
        assert_eq!(key_file(&paths).file_name().unwrap(), "steam.key");
        assert_eq!(key_file(&paths).parent(), other.parent());
    }

    #[test]
    fn saving_without_a_keyring_writes_a_private_file_and_removing_deletes_it() {
        let (_root, paths) = setup();
        let saved = save_key(&paths, &NoKeyring, "abc123").unwrap();
        assert_eq!(saved, Saved::File(key_file(&paths)));
        let mode = fs::metadata(key_file(&paths)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(fs::read_to_string(key_file(&paths)).unwrap(), "abc123\n");

        let removed = remove_key(&paths, &NoKeyring).unwrap();
        assert_eq!(
            removed,
            Removed {
                keyring: false,
                file: true
            }
        );
        assert!(!key_file(&paths).exists());
    }

    #[test]
    fn saving_to_the_keyring_drops_a_stale_file() {
        let (_root, paths) = setup();
        save_key(&paths, &NoKeyring, "old").unwrap();
        let keyring = FakeKeyring(RefCell::new(None));
        assert_eq!(save_key(&paths, &keyring, "new").unwrap(), Saved::Keyring);
        assert!(!key_file(&paths).exists());
        let key = find_key(&paths, |_| None, &keyring).unwrap().unwrap();
        assert_eq!(key.value, "new");
    }
}
