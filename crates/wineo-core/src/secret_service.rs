//! The system keyring, through the freedesktop Secret Service API.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use secret_service::{EncryptionType, Error, Item, SecretService};
use zbus::DBusError;

use crate::key_store::{KeySpec, Keyring, KeyringError};

/// One item in the Secret Service, found by its `service` and `account`
/// attributes, the same pair tools like `secret-tool` search by.
pub struct SecretServiceKeyring {
    account: String,
    label: String,
}

const SERVICE: &str = "wineo";

impl SecretServiceKeyring {
    /// The item holding the SteamGridDB key.
    pub fn steamgriddb() -> Self {
        Self::for_spec(&crate::steamgriddb::KEY_SPEC)
    }

    /// The item holding the GOG login.
    pub fn gog() -> Self {
        Self::for_spec(&crate::gog::account::KEY_SPEC)
    }

    /// The item holding the Steam key.
    pub fn steam() -> Self {
        Self::for_spec(&crate::steam::key::KEY_SPEC)
    }

    /// The item holding the key a spec describes.
    pub fn for_spec(spec: &KeySpec) -> Self {
        Self {
            account: spec.account.to_owned(),
            label: spec.label.to_owned(),
        }
    }

    fn attributes(&self) -> HashMap<&str, &str> {
        HashMap::from([("service", SERVICE), ("account", self.account.as_str())])
    }
}

/// Connects, treating any failure as no keyring: nothing has been asked of
/// one yet, so there is no request of the user's to have failed.
///
/// Each call gets its own connection: secret-service never closes the
/// encrypted session it opens, and the daemon only drops it on disconnect.
///
/// Method calls time out like the libdbus client's did, so a daemon that
/// stops answering can't hang a command. Waiting on an unlock prompt is a
/// signal, not a method call, so the user still gets as long as they need.
async fn connect<'a>() -> Result<SecretService<'a>, KeyringError> {
    let bus = zbus::connection::Builder::session()
        .map_err(|_| KeyringError::Unavailable)?
        .method_timeout(METHOD_TIMEOUT)
        .build()
        .await
        .map_err(|_| KeyringError::Unavailable)?;
    SecretService::connect_with_existing(EncryptionType::Dh, bus)
        .await
        .map_err(|_| KeyringError::Unavailable)
}

const METHOD_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs one keyring operation on a runtime of its own on this thread, so no
/// worker threads outlive it. zbus's blocking API would instead start a
/// multi-thread runtime that lives as long as the process. Like any
/// `block_on`, it must not be called from async code.
fn block_on<T>(
    operation: impl Future<Output = Result<T, KeyringError>>,
) -> Result<T, KeyringError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| KeyringError::Unavailable)?;
    runtime.block_on(operation)
}

/// Keeps the messages the libdbus client gave, which users may have seen.
fn failed(err: Error) -> KeyringError {
    let message = match err {
        // The only decryption failure secret-service reports: bad padding.
        Error::Crypto("message decryption failed") => "Crypto error: Unpad Error".to_owned(),
        Error::Crypto(err) => format!("Crypto error: {err}"),
        Error::Zbus(err) => format!("DBus error: {}", zbus_message(&err)),
        Error::ZbusFdo(err) => format!("DBus error: {}", fdo_message(&err)),
        Error::Locked => "Secret Service: object locked".to_owned(),
        Error::NoResult => "Secret Service: no result found".to_owned(),
        Error::Prompt => "Secret Service: unlock prompt was dismissed".to_owned(),
        Error::PromptDisconnected => {
            "Secret Service: connection closed before the prompt completed".to_owned()
        }
        Error::Unavailable => "No DBus session or Secret Service provider found".to_owned(),
        err => format!("Unexpected Error: {err}"),
    };
    KeyringError::Failed(message)
}

/// libdbus showed only an error reply's description, never its name.
fn zbus_message(err: &zbus::Error) -> String {
    match err {
        zbus::Error::MethodError(_, description, _) => description.clone().unwrap_or_default(),
        zbus::Error::FDO(err) => fdo_message(err),
        err => err.to_string(),
    }
}

fn fdo_message(err: &zbus::fdo::Error) -> String {
    match err {
        zbus::fdo::Error::ZBus(err) => zbus_message(err),
        err => err.description().unwrap_or_default().to_owned(),
    }
}

impl SecretServiceKeyring {
    fn read(&self, attributes: HashMap<&str, &str>) -> Result<Option<String>, KeyringError> {
        block_on(async {
            let service = connect().await?;
            let found = service.search_items(attributes).await.map_err(failed)?;
            let Some(item) = found.unlocked.first().or(found.locked.first()) else {
                return Ok(None);
            };
            unlock_item(item).await?;
            let secret = item.get_secret().await.map_err(failed)?;
            String::from_utf8(secret)
                .map(Some)
                .map_err(|_| KeyringError::Failed("the saved key is not text".to_owned()))
        })
    }

    fn write(
        &self,
        label: &str,
        attributes: HashMap<&str, &str>,
        key: &str,
    ) -> Result<(), KeyringError> {
        block_on(async {
            let service = connect().await?;
            let collection = match service.get_default_collection().await {
                Ok(collection) => collection,
                Err(Error::NoResult) => return Err(KeyringError::Unavailable),
                Err(err) => return Err(failed(err)),
            };
            // This crate's ensure_unlocked only reports a lock; asking to unlock
            // shows the keyring's prompt, as before.
            if collection.is_locked().await.map_err(failed)? {
                collection.unlock().await.map_err(failed)?;
            }
            collection
                .create_item(label, attributes, key.as_bytes(), true, "text/plain")
                .await
                .map_err(failed)?;
            Ok(())
        })
    }

    /// Deletes every item that has all of `attributes`.
    fn remove(&self, attributes: HashMap<&str, &str>) -> Result<bool, KeyringError> {
        block_on(async {
            let service = connect().await?;
            let found = service.search_items(attributes).await.map_err(failed)?;
            let items: Vec<_> = found.unlocked.iter().chain(&found.locked).collect();
            for item in &items {
                // This crate refuses to delete a locked item rather than letting
                // the keyring decide, so unlock it first.
                unlock_item(item).await?;
                item.delete().await.map_err(failed)?;
            }
            Ok(!items.is_empty())
        })
    }
}

/// The `account` of the items holding private-branch passwords. Each also
/// has the `user` who saved it, the `game`, and the `branch`.
const BRANCH_ACCOUNT: &str = "gog-branch";
const BRANCH_LABEL: &str = "wineo GOG branch password";

fn branch_attributes<'a>(
    user: &'a str,
    game: &'a str,
    branch: &'a str,
) -> HashMap<&'a str, &'a str> {
    HashMap::from([
        ("service", SERVICE),
        ("account", BRANCH_ACCOUNT),
        ("user", user),
        ("game", game),
        ("branch", branch),
    ])
}

impl Keyring for SecretServiceKeyring {
    fn get(&self) -> Result<Option<String>, KeyringError> {
        self.read(self.attributes())
    }

    fn set(&self, key: &str) -> Result<(), KeyringError> {
        self.write(&self.label, self.attributes(), key)
    }

    fn delete(&self) -> Result<bool, KeyringError> {
        self.remove(self.attributes())
    }

    fn get_branch(
        &self,
        user: &str,
        game: u64,
        branch: &str,
    ) -> Result<Option<String>, KeyringError> {
        self.read(branch_attributes(user, &game.to_string(), branch))
    }

    fn set_branch(
        &self,
        user: &str,
        game: u64,
        branch: &str,
        password: &str,
    ) -> Result<(), KeyringError> {
        self.write(
            BRANCH_LABEL,
            branch_attributes(user, &game.to_string(), branch),
            password,
        )
    }

    fn delete_branch(&self, user: &str, game: u64, branch: &str) -> Result<bool, KeyringError> {
        self.remove(branch_attributes(user, &game.to_string(), branch))
    }

    fn list_branches(&self, user: &str, game: u64) -> Result<Vec<(String, String)>, KeyringError> {
        let game = game.to_string();
        block_on(async {
            let service = connect().await?;
            let found = service
                .search_items(HashMap::from([
                    ("service", SERVICE),
                    ("account", BRANCH_ACCOUNT),
                    ("user", user),
                    ("game", game.as_str()),
                ]))
                .await
                .map_err(failed)?;
            let mut listed = Vec::new();
            for item in found.unlocked.iter().chain(&found.locked) {
                unlock_item(item).await?;
                let attributes = item.get_attributes().await.map_err(failed)?;
                let Some(branch) = attributes.get("branch") else {
                    continue;
                };
                let secret = item.get_secret().await.map_err(failed)?;
                let password = String::from_utf8(secret)
                    .map_err(|_| KeyringError::Failed("the saved key is not text".to_owned()))?;
                listed.push((branch.clone(), password));
            }
            Ok(listed)
        })
    }

    fn delete_all_branches(&self) -> Result<bool, KeyringError> {
        self.remove(HashMap::from([
            ("service", SERVICE),
            ("account", BRANCH_ACCOUNT),
        ]))
    }

    fn delete_branches_not_of(&self, user: &str) -> Result<bool, KeyringError> {
        block_on(async {
            let service = connect().await?;
            let found = service
                .search_items(HashMap::from([
                    ("service", SERVICE),
                    ("account", BRANCH_ACCOUNT),
                ]))
                .await
                .map_err(failed)?;
            let mut deleted = false;
            for item in found.unlocked.iter().chain(&found.locked) {
                unlock_item(item).await?;
                let attributes = item.get_attributes().await.map_err(failed)?;
                // An item with no owner is from before items had one.
                if attributes.get("user").map(String::as_str) != Some(user) {
                    item.delete().await.map_err(failed)?;
                    deleted = true;
                }
            }
            Ok(deleted)
        })
    }
}

/// Unlocks the item if it is locked, which may show the keyring's prompt.
async fn unlock_item(item: &Item<'_>) -> Result<(), KeyringError> {
    if item.is_locked().await.map_err(failed)? {
        item.unlock().await.map_err(failed)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(err: Error) -> String {
        match failed(err) {
            KeyringError::Failed(message) => message,
            KeyringError::Unavailable => panic!("expected a failure"),
        }
    }

    fn denied() -> zbus::fdo::Error {
        zbus::fdo::Error::AccessDenied("denied".to_owned())
    }

    #[test]
    fn a_bad_padding_reads_as_libdbus_did() {
        assert_eq!(
            message(Error::Crypto("message decryption failed")),
            "Crypto error: Unpad Error"
        );
        assert_eq!(
            message(Error::Crypto("secret has an invalid initialization vector")),
            "Crypto error: secret has an invalid initialization vector"
        );
    }

    #[test]
    fn dbus_errors_show_only_their_description() {
        assert_eq!(message(Error::ZbusFdo(denied())), "DBus error: denied");
        assert_eq!(
            message(Error::Zbus(zbus::Error::FDO(Box::new(denied())))),
            "DBus error: denied"
        );
        assert_eq!(
            message(Error::ZbusFdo(zbus::fdo::Error::ZBus(zbus::Error::FDO(
                Box::new(denied())
            )))),
            "DBus error: denied"
        );

        let call = zbus::message::Message::method_call("/", "Unlock")
            .unwrap()
            .build(&())
            .unwrap();
        let name = "org.freedesktop.Secret.Error.IsLocked";
        let locked = |description: Option<&str>| {
            Error::Zbus(zbus::Error::MethodError(
                name.try_into().unwrap(),
                description.map(str::to_owned),
                call.clone(),
            ))
        };
        assert_eq!(message(locked(Some("locked"))), "DBus error: locked");
        assert_eq!(message(locked(None)), "DBus error: ");
    }

    #[test]
    fn secret_service_states_keep_their_messages() {
        assert_eq!(message(Error::Locked), "Secret Service: object locked");
        assert_eq!(message(Error::NoResult), "Secret Service: no result found");
        assert_eq!(
            message(Error::Prompt),
            "Secret Service: unlock prompt was dismissed"
        );
        assert_eq!(
            message(Error::Unavailable),
            "No DBus session or Secret Service provider found"
        );
    }

    /// Writes to whatever Secret Service the session bus reaches, so it only
    /// runs when `WINEO_TEST_SECRET_SERVICE` says that bus is a throwaway
    /// one, such as gnome-keyring under `dbus-run-session`.
    #[test]
    #[ignore = "writes to a real Secret Service"]
    fn round_trips_through_a_real_secret_service() {
        if std::env::var_os("WINEO_TEST_SECRET_SERVICE").is_none() {
            return;
        }
        let keyring = SecretServiceKeyring::for_spec(&KeySpec {
            env_var: None,
            account: "test-round-trip",
            file_name: "test-round-trip",
            label: "wineo test key",
        });
        assert_eq!(
            keyring.get(),
            Ok(None),
            "refusing to replace an existing item"
        );

        keyring.set("first").unwrap();
        assert_eq!(keyring.get(), Ok(Some("first".to_owned())));
        keyring.set("second").unwrap();
        assert_eq!(keyring.get(), Ok(Some("second".to_owned())));

        assert_eq!(keyring.delete(), Ok(true));
        assert_eq!(keyring.get(), Ok(None));
        assert_eq!(keyring.delete(), Ok(false));
    }
}
