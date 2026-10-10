//! The system keyring, through the freedesktop Secret Service API.

use std::collections::HashMap;

use dbus_secret_service::{EncryptionType, Error, SecretService};

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

    pub fn for_account(account: &str) -> Self {
        Self {
            account: account.to_owned(),
            label: format!("{SERVICE} {account} key"),
        }
    }

    fn attributes(&self) -> HashMap<&str, &str> {
        HashMap::from([("service", SERVICE), ("account", self.account.as_str())])
    }
}

/// Connects, treating any failure as no keyring: nothing has been asked of
/// one yet, so there is no request of the user's to have failed.
fn connect() -> Result<SecretService, KeyringError> {
    SecretService::connect(EncryptionType::Dh).map_err(|_| KeyringError::Unavailable)
}

fn failed(err: Error) -> KeyringError {
    KeyringError::Failed(err.to_string())
}

impl Keyring for SecretServiceKeyring {
    fn get(&self) -> Result<Option<String>, KeyringError> {
        let service = connect()?;
        let found = service.search_items(self.attributes()).map_err(failed)?;
        let Some(item) = found.unlocked.first().or(found.locked.first()) else {
            return Ok(None);
        };
        item.ensure_unlocked().map_err(failed)?;
        let secret = item.get_secret().map_err(failed)?;
        String::from_utf8(secret)
            .map(Some)
            .map_err(|_| KeyringError::Failed("the saved key is not text".to_owned()))
    }

    fn set(&self, key: &str) -> Result<(), KeyringError> {
        let service = connect()?;
        let collection = match service.get_default_collection() {
            Ok(collection) => collection,
            Err(Error::NoResult) => return Err(KeyringError::Unavailable),
            Err(err) => return Err(failed(err)),
        };
        collection.ensure_unlocked().map_err(failed)?;
        collection
            .create_item(
                &self.label,
                self.attributes(),
                key.as_bytes(),
                true,
                "text/plain",
            )
            .map_err(failed)?;
        Ok(())
    }

    fn delete(&self) -> Result<bool, KeyringError> {
        let service = connect()?;
        let found = service.search_items(self.attributes()).map_err(failed)?;
        let items: Vec<_> = found.unlocked.iter().chain(&found.locked).collect();
        for item in &items {
            item.delete().map_err(failed)?;
        }
        Ok(!items.is_empty())
    }
}
