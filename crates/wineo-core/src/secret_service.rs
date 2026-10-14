//! The system keyring, through the freedesktop Secret Service API.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use secret_service::{EncryptionType, Error, Item, SecretService};

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
        Error::Crypto(err) => format!("Crypto error: {err}"),
        Error::Zbus(zbus::Error::MethodError(name, description, _)) => {
            format!(
                "DBus error: {}",
                description.as_deref().unwrap_or(name.as_str())
            )
        }
        Error::Zbus(err) => format!("DBus error: {err}"),
        Error::ZbusFdo(err) => format!("DBus error: {err}"),
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

impl Keyring for SecretServiceKeyring {
    fn get(&self) -> Result<Option<String>, KeyringError> {
        block_on(async {
            let service = connect().await?;
            let found = service
                .search_items(self.attributes())
                .await
                .map_err(failed)?;
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

    fn set(&self, key: &str) -> Result<(), KeyringError> {
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
                .create_item(
                    &self.label,
                    self.attributes(),
                    key.as_bytes(),
                    true,
                    "text/plain",
                )
                .await
                .map_err(failed)?;
            Ok(())
        })
    }

    fn delete(&self) -> Result<bool, KeyringError> {
        block_on(async {
            let service = connect().await?;
            let found = service
                .search_items(self.attributes())
                .await
                .map_err(failed)?;
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
