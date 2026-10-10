use std::ffi::OsString;
use std::fmt;
use std::path::Path;

use iced::Task;
use wineo_core::download::{Https, Transport};
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb::{self, KeySource, Keyring, Saved as CoreSaved};

use super::Event;
use crate::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Busy {
    Saving,
    Removing,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

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
    pub(crate) fn available(&self) -> bool {
        self.source.is_some()
    }

    pub(crate) fn label(&self, paths: &Paths) -> String {
        match self.source {
            Some(KeySource::Variable) => "From WINEO_STEAMGRIDDB_KEY".to_owned(),
            Some(KeySource::Keyring) => "Saved in the keyring".to_owned(),
            Some(KeySource::File) => format!(
                "Saved in a plain text file: {}",
                steamgriddb::key_file(paths).display()
            ),
            None => "No key".to_owned(),
        }
    }

    pub(crate) fn warning(&self, paths: &Paths) -> Option<String> {
        (self.source == Some(KeySource::File))
            .then(|| plain_text_warning(&steamgriddb::key_file(paths)))
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

pub(crate) fn status(paths: &Paths) -> Result<Status, String> {
    status_with(
        paths,
        |key| std::env::var_os(key),
        &SecretServiceKeyring::steamgriddb(),
    )
}

fn status_with(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
) -> Result<Status, String> {
    let source = steamgriddb::find_key(paths, &lookup, keyring)
        .map_err(one_line)?
        .map(|key| key.source);
    Ok(Status {
        override_note: source == Some(KeySource::Variable),
        source,
    })
}

fn save_with(
    paths: &Paths,
    lookup: impl Fn(&str) -> Option<OsString>,
    keyring: &dyn Keyring,
    transport: &dyn Transport,
    key: &str,
) -> Result<Saved, String> {
    if !steamgriddb::check_key(transport, key).map_err(one_line)? {
        return Err("SteamGridDB rejected the key".to_owned());
    }
    let override_note = steamgriddb::find_key(paths, &lookup, keyring)
        .map_err(one_line)?
        .is_some_and(|key| key.source == KeySource::Variable);
    let saved = steamgriddb::save_key(paths, keyring, key).map_err(one_line)?;
    let (saved_source, warning, result) = match saved {
        CoreSaved::Keyring => (
            KeySource::Keyring,
            None,
            Some("Saved the SteamGridDB key to the keyring".to_owned()),
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
    let removed = steamgriddb::remove_key(paths, keyring).map_err(one_line)?;
    let override_note = steamgriddb::find_key(paths, &lookup, keyring)
        .map_err(one_line)?
        .is_some_and(|key| key.source == KeySource::Variable);
    Ok(Removed {
        status: Status {
            source: override_note.then_some(KeySource::Variable),
            override_note,
        },
        result: if removed.keyring || removed.file {
            "Removed the SteamGridDB key".to_owned()
        } else {
            "No SteamGridDB key was saved".to_owned()
        },
    })
}

pub(super) fn plain_text_warning(path: &Path) -> String {
    format!(
        "warning: no keyring, so the key is stored in plain text in {}",
        path.display()
    )
}

pub(super) fn one_line(error: impl std::fmt::Display) -> String {
    error
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn status_task(paths: Paths, generation: u64, sequence: u64) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || status(&paths))
                .await
                .unwrap_or_else(|error| Err(one_line(error)))
        },
        move |result| {
            Message::Settings(Event::SteamGridDbKeyStatusLoaded(
                generation, sequence, result,
            ))
        },
    )
}

pub(crate) fn key_presence_task(paths: Paths, sequence: u64) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || status(&paths))
                .await
                .ok()
                .and_then(Result::ok)
                .is_some_and(|status| status.available())
        },
        move |available| Message::SteamGridDbKeyPresenceLoaded(sequence, available),
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
        async move {
            tokio::task::spawn_blocking(move || {
                let keyring = SecretServiceKeyring::steamgriddb();
                let transport = Https::new().map_err(one_line)?;
                save_with(
                    &paths,
                    |name| std::env::var_os(name),
                    &keyring,
                    &transport,
                    key.as_str(),
                )
            })
            .await
            .unwrap_or_else(|error| Err(one_line(error)))
        },
        move |result| {
            Message::Settings(Event::SteamGridDbKeySaved(
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
        async move {
            tokio::task::spawn_blocking(move || {
                let keyring = SecretServiceKeyring::steamgriddb();
                remove_with(&paths, |name| std::env::var_os(name), &keyring)
            })
            .await
            .unwrap_or_else(|error| Err(one_line(error)))
        },
        move |result| {
            Message::Settings(Event::SteamGridDbKeyRemoved(
                generation,
                sequence,
                input_session,
                result,
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::{Cursor, Read};

    use super::*;
    use wineo_core::download::{Response, Transport};
    use wineo_core::steamgriddb::{KeyringError, NoKeyring};

    struct TestKeyring {
        key: RefCell<Option<String>>,
        error: Option<KeyringError>,
    }

    impl TestKeyring {
        fn with(key: Option<&str>) -> Self {
            Self {
                key: RefCell::new(key.map(str::to_owned)),
                error: None,
            }
        }

        fn failing(reason: &str) -> Self {
            Self {
                key: RefCell::new(None),
                error: Some(KeyringError::Failed(reason.to_owned())),
            }
        }

        fn check(&self) -> Result<(), KeyringError> {
            self.error.clone().map_or(Ok(()), Err)
        }
    }

    impl Keyring for TestKeyring {
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

    struct TestTransport(u16);

    impl Transport for TestTransport {
        fn get(&self, _url: &str) -> anyhow::Result<Box<dyn Read>> {
            Ok(Box::new(Cursor::new(Vec::<u8>::new())))
        }

        fn request(&self, _url: &str, _bearer: Option<&str>) -> anyhow::Result<Response> {
            Ok(Response {
                status: self.0,
                body: Box::new(Cursor::new(Vec::<u8>::new())),
            })
        }
    }

    fn setup() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            Some(&root.path().join("portable")),
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
    fn source_labels_never_include_the_key() {
        let (_root, paths) = setup();
        let variable = status_with(
            &paths,
            lookup(&[(steamgriddb::KEY_VAR, "secret-value")]),
            &NoKeyring,
        )
        .unwrap();
        assert_eq!(variable.label(&paths), "From WINEO_STEAMGRIDDB_KEY");
        assert!(variable.override_note);

        let keyring = status_with(
            &paths,
            lookup(&[]),
            &TestKeyring::with(Some("secret-value")),
        )
        .unwrap();
        assert_eq!(keyring.label(&paths), "Saved in the keyring");

        let file = steamgriddb::key_file(&paths);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "secret-value").unwrap();
        let plain_text = status_with(&paths, lookup(&[]), &NoKeyring).unwrap();
        assert_eq!(
            plain_text.label(&paths),
            format!("Saved in a plain text file: {}", file.display())
        );
        assert_eq!(
            plain_text.warning(&paths).as_deref(),
            Some(plain_text_warning(&file).as_str())
        );

        std::fs::remove_file(file).unwrap();
        let missing = status_with(&paths, lookup(&[]), &NoKeyring).unwrap();
        assert_eq!(missing.label(&paths), "No key");
        for label in [
            variable.label(&paths),
            keyring.label(&paths),
            plain_text.label(&paths),
            missing.label(&paths),
        ] {
            assert!(!label.contains("secret-value"));
        }
    }

    #[test]
    fn whitespace_environment_value_uses_the_core_fallback_rules() {
        let (_root, paths) = setup();
        let whitespace = lookup(&[(steamgriddb::KEY_VAR, " \t\n")]);
        let status =
            status_with(&paths, &whitespace, &TestKeyring::with(Some("saved-key"))).unwrap();
        assert_eq!(status.source, Some(KeySource::Keyring));
        assert!(!status.override_note);

        let keyring = TestKeyring::with(None);
        let saved = save_with(
            &paths,
            &whitespace,
            &keyring,
            &TestTransport(200),
            "new-key",
        )
        .unwrap();
        assert_eq!(saved.status.source, Some(KeySource::Keyring));
        assert!(!saved.status.override_note);

        let removed = remove_with(&paths, &whitespace, &keyring).unwrap();
        assert_eq!(removed.status.source, None);
        assert!(!removed.status.override_note);
    }

    #[test]
    fn debug_output_redacts_the_key_from_settings_events_and_messages() {
        let key = "private-api-key";
        let settings = Settings {
            input: Secret::new(key.to_owned()),
            ..Settings::default()
        };
        let event = Event::SteamGridDbKeyText(Secret::new(key.to_owned()));
        let message = Message::Settings(event.clone());

        for formatted in [
            format!("{settings:?}"),
            format!("{event:?}"),
            format!("{message:?}"),
        ] {
            assert!(!formatted.contains(key));
            assert!(formatted.contains("<redacted>"));
        }
    }

    #[test]
    fn valid_key_is_checked_then_saved_even_when_the_environment_overrides_it() {
        let (_root, paths) = setup();
        let keyring = TestKeyring::with(None);
        let saved = save_with(
            &paths,
            lookup(&[(steamgriddb::KEY_VAR, "environment-key")]),
            &keyring,
            &TestTransport(200),
            "saved-key",
        )
        .unwrap();
        assert_eq!(keyring.key.borrow().as_deref(), Some("saved-key"));
        assert_eq!(saved.status.label(&paths), "From WINEO_STEAMGRIDDB_KEY");
        assert!(saved.status.override_note);
        assert_eq!(
            saved.result.as_deref(),
            Some("Saved the SteamGridDB key to the keyring")
        );
    }

    #[test]
    fn invalid_key_is_not_saved() {
        let (_root, paths) = setup();
        let keyring = TestKeyring::with(None);
        let error = save_with(
            &paths,
            lookup(&[]),
            &keyring,
            &TestTransport(401),
            "invalid",
        )
        .unwrap_err();
        assert_eq!(error, "SteamGridDB rejected the key");
        assert_eq!(keyring.key.borrow().as_deref(), None);
    }

    #[test]
    fn keyring_failure_is_an_error_without_a_file_fallback() {
        let (_root, paths) = setup();
        let error = save_with(
            &paths,
            lookup(&[]),
            &TestKeyring::failing("locked"),
            &TestTransport(200),
            "valid",
        )
        .unwrap_err();
        assert!(error.contains("locked"));
        assert!(!steamgriddb::key_file(&paths).exists());
    }

    #[test]
    fn unavailable_keyring_reports_the_plain_text_warning() {
        let (_root, paths) = setup();
        let saved = save_with(
            &paths,
            lookup(&[]),
            &NoKeyring,
            &TestTransport(200),
            "valid",
        )
        .unwrap();
        let file = steamgriddb::key_file(&paths);
        assert_eq!(
            saved.status.label(&paths),
            format!("Saved in a plain text file: {}", file.display())
        );
        assert_eq!(
            saved.warning.as_deref(),
            Some(plain_text_warning(&file).as_str())
        );
        assert!(file.exists());
    }

    #[test]
    fn remove_reports_the_result_and_keeps_the_environment_override_note() {
        let (_root, paths) = setup();
        let keyring = TestKeyring::with(Some("saved-key"));
        let removed = remove_with(
            &paths,
            lookup(&[(steamgriddb::KEY_VAR, "environment-key")]),
            &keyring,
        )
        .unwrap();
        assert_eq!(removed.result, "Removed the SteamGridDB key");
        assert_eq!(removed.status.label(&paths), "From WINEO_STEAMGRIDDB_KEY");
        assert!(removed.status.override_note);
        assert_eq!(keyring.key.borrow().as_deref(), None);

        let file = steamgriddb::key_file(&paths);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "file-key").unwrap();
        let removed_file = remove_with(&paths, lookup(&[]), &NoKeyring).unwrap();
        assert_eq!(removed_file.result, "Removed the SteamGridDB key");
        assert_eq!(removed_file.status.label(&paths), "No key");
        assert!(!file.exists());

        let again = remove_with(&paths, lookup(&[]), &NoKeyring).unwrap();
        assert_eq!(again.result, "No SteamGridDB key was saved");
    }
}
