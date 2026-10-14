//! The Steam Web API key row, beside the SteamGridDB one and with the same
//! save, check, and remove behavior. The key is never shown once saved.

use std::ffi::OsString;

use iced::Task;
use wineo_core::config::GlobalConfig;
use wineo_core::key_store::{KeySource, Keyring, Saved as CoreSaved};
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steam::api::SteamApi;
use wineo_core::steam::{self, key, owned};

use super::Event;
use super::steamgriddb::{Busy, Secret, one_line, plain_text_warning};
use crate::Message;
use crate::task;

pub(crate) const KEY_PAGE: &str = "https://steamcommunity.com/dev/apikey";

#[derive(Debug, Clone, Default)]
pub(crate) struct Settings {
    pub(crate) status: Option<Status>,
    pub(crate) status_error: Option<String>,
    pub(crate) warning: Option<String>,
    pub(crate) input_open: bool,
    pub(crate) input: Secret,
    pub(crate) busy: Option<Busy>,
    pub(crate) error: Option<String>,
    pub(crate) result: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) source: Option<KeySource>,
    pub(crate) override_note: bool,
}

impl Status {
    pub(crate) fn label(&self, paths: &Paths) -> String {
        match self.source {
            Some(KeySource::Variable) => format!("From {}", key::KEY_VAR),
            Some(KeySource::Keyring) => "Saved in the keyring".to_owned(),
            Some(KeySource::File) => format!(
                "Saved in a plain text file: {}",
                key::key_file(paths).display()
            ),
            None => "No key".to_owned(),
        }
    }

    pub(crate) fn warning(&self, paths: &Paths) -> Option<String> {
        (self.source == Some(KeySource::File)).then(|| plain_text_warning(&key::key_file(paths)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Saved {
    pub(crate) status: Status,
    pub(crate) warning: Option<String>,
    pub(crate) result: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Removed {
    pub(crate) status: Status,
    pub(crate) result: String,
}

fn keyring() -> SecretServiceKeyring {
    SecretServiceKeyring::steam()
}

fn overridden(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<bool, String> {
    Ok(key::find_key(paths, lookup, keyring)
        .map_err(one_line)?
        .is_some_and(|key| key.source == KeySource::Variable))
}

fn status_with(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Status, String> {
    let source = key::find_key(paths, lookup, keyring)
        .map_err(one_line)?
        .map(|key| key.source);
    Ok(Status {
        override_note: source == Some(KeySource::Variable),
        source,
    })
}

/// Checks the key with one owned games request, like `import steam --key`,
/// and saves it only when Steam shows it the library.
fn save_with(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
    check: impl FnOnce(&str) -> Result<(), String>,
    key: &str,
) -> Result<Saved, String> {
    check(key)?;
    let override_note = overridden(paths, &lookup, keyring)?;
    let (saved_source, warning, result) =
        match key::save_key(paths, keyring, key).map_err(one_line)? {
            CoreSaved::Keyring => (
                KeySource::Keyring,
                None,
                Some("Saved the Steam Web API key to the keyring".to_owned()),
            ),
            CoreSaved::File(path) => (KeySource::File, Some(plain_text_warning(&path)), None),
        };
    Ok(Saved {
        status: Status {
            source: Some(if override_note {
                KeySource::Variable
            } else {
                saved_source
            }),
            override_note,
        },
        warning,
        result,
    })
}

fn remove_with(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Removed, String> {
    let removed = key::remove_key(paths, keyring).map_err(one_line)?;
    let override_note = overridden(paths, &lookup, keyring)?;
    Ok(Removed {
        status: Status {
            source: override_note.then_some(KeySource::Variable),
            override_note,
        },
        result: if removed.keyring || removed.file {
            "Removed the Steam Web API key".to_owned()
        } else {
            "No Steam Web API key was saved".to_owned()
        },
    })
}

/// The online check `import steam --key` makes, with its messages.
fn check_online(paths: &Paths, key: &str) -> Result<(), String> {
    let global = GlobalConfig::load(paths).map_err(one_line)?;
    let install = steam::active_for(paths, &global).ok_or("Steam wasn't found.")?;
    let api = SteamApi::from_env().map_err(|error| error.to_string())?;
    owned::check_key(&api, &install, key)
        .map_err(|error| format!("Couldn't check the key: {error}"))
}

pub(crate) fn status_task(paths: Paths, generation: u64, sequence: u64) -> Task<Message> {
    Task::perform(
        task::join(
            move || status_with(&paths, |name| std::env::var_os(name), &keyring()),
            one_line,
        ),
        move |result| Message::Settings(Event::SteamKeyStatusLoaded(generation, sequence, result)),
    )
}

pub(crate) fn save_task(
    paths: Paths,
    generation: u64,
    sequence: u64,
    input_session: Option<u64>,
    key: Secret,
) -> Task<Message> {
    Task::perform(
        task::join(
            move || {
                save_with(
                    &paths,
                    |name| std::env::var_os(name),
                    &keyring(),
                    |key| check_online(&paths, key),
                    key.as_str(),
                )
            },
            one_line,
        ),
        move |result| {
            Message::Settings(Event::SteamKeySaved(
                generation,
                sequence,
                input_session,
                result,
            ))
        },
    )
}

pub(crate) fn remove_task(
    paths: Paths,
    generation: u64,
    sequence: u64,
    input_session: Option<u64>,
) -> Task<Message> {
    Task::perform(
        task::join(
            move || remove_with(&paths, |name| std::env::var_os(name), &keyring()),
            one_line,
        ),
        move |result| {
            Message::Settings(Event::SteamKeyRemoved(
                generation,
                sequence,
                input_session,
                result,
            ))
        },
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;
    use wineo_core::key_store::{KeyringError, NoKeyring};

    pub(crate) const KEY: &str = "STEAMSECRET0123456789";

    pub(crate) struct TestKeyring(pub(crate) RefCell<Option<String>>);

    impl Keyring for TestKeyring {
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
            None,
        )
        .unwrap();
        (root, paths)
    }

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let values: HashMap<String, OsString> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
            .collect();
        move |name| values.get(name).cloned()
    }

    #[test]
    fn labels_name_the_steam_variable_and_file_never_the_key() {
        let (_root, paths) = setup();
        let variable = status_with(&paths, lookup(&[(key::KEY_VAR, KEY)]), &NoKeyring).unwrap();
        assert_eq!(variable.label(&paths), "From WINEO_STEAM_KEY");
        assert!(variable.override_note);

        let file = key::key_file(&paths);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, KEY).unwrap();
        let plain = status_with(&paths, lookup(&[]), &NoKeyring).unwrap();
        assert_eq!(
            plain.label(&paths),
            format!("Saved in a plain text file: {}", file.display())
        );
        assert!(plain.warning(&paths).unwrap().contains("plain text"));

        // The SteamGridDB variable is someone else's key.
        std::fs::remove_file(&file).unwrap();
        let other = status_with(
            &paths,
            lookup(&[(wineo_core::steamgriddb::KEY_VAR, KEY)]),
            &TestKeyring(RefCell::new(None)),
        )
        .unwrap();
        assert_eq!(other.source, None);
        assert_eq!(other.label(&paths), "No key");
        for label in [variable.label(&paths), plain.label(&paths)] {
            assert!(!label.contains(KEY));
        }
    }

    #[test]
    fn save_checks_first_and_a_refused_key_is_not_saved() {
        let (_root, paths) = setup();
        let keyring = TestKeyring(RefCell::new(None));
        let error = save_with(
            &paths,
            lookup(&[]),
            &keyring,
            |_| Err("Couldn't check the key: the key can't see this library".to_owned()),
            KEY,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Couldn't check the key: the key can't see this library"
        );
        assert_eq!(keyring.0.borrow().as_deref(), None);

        let checked = RefCell::new(None);
        let saved = save_with(
            &paths,
            lookup(&[]),
            &keyring,
            |key| {
                *checked.borrow_mut() = Some(key.to_owned());
                Ok(())
            },
            KEY,
        )
        .unwrap();
        assert_eq!(checked.borrow().as_deref(), Some(KEY));
        assert_eq!(keyring.0.borrow().as_deref(), Some(KEY));
        assert_eq!(saved.status.label(&paths), "Saved in the keyring");
        assert_eq!(
            saved.result.as_deref(),
            Some("Saved the Steam Web API key to the keyring")
        );
        assert!(!format!("{saved:?}").contains(KEY));
    }

    #[test]
    fn save_without_a_keyring_warns_about_the_plain_text_file() {
        let (_root, paths) = setup();
        let saved = save_with(&paths, lookup(&[]), &NoKeyring, |_| Ok(()), KEY).unwrap();
        let file = key::key_file(&paths);
        assert_eq!(
            saved.warning.as_deref(),
            Some(plain_text_warning(&file).as_str())
        );
        assert!(file.is_file());
        assert!(!wineo_core::steamgriddb::key_file(&paths).exists());
    }

    #[test]
    fn remove_reports_the_result_and_keeps_the_override_note() {
        let (_root, paths) = setup();
        let keyring = TestKeyring(RefCell::new(Some(KEY.to_owned())));
        let removed = remove_with(&paths, lookup(&[(key::KEY_VAR, "env")]), &keyring).unwrap();
        assert_eq!(removed.result, "Removed the Steam Web API key");
        assert!(removed.status.override_note);
        assert_eq!(keyring.0.borrow().as_deref(), None);
        let again = remove_with(&paths, lookup(&[]), &NoKeyring).unwrap();
        assert_eq!(again.result, "No Steam Web API key was saved");
        assert_eq!(again.status.label(&paths), "No key");
    }
}
