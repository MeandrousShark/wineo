//! GOG accounts and libraries, on top of the `wineo-gog` client: where the
//! login is kept, how it is renewed, and the cached owned list.

pub mod account;
pub mod owned;

pub use wineo_gog as api;

/// What wineo calls itself to GOG. Nothing personal goes in it.
pub const USER_AGENT: &str = concat!("wineo/", env!("CARGO_PKG_VERSION"));

/// GOG itself, except that debug builds send requests to the plain HTTP
/// server named by `WINEO_TEST_GOG_API`, so CLI tests can stand in for GOG.
/// Release builds never look at the variable.
pub fn client_from_env() -> Result<api::Client, api::Error> {
    #[cfg(debug_assertions)]
    if let Some(base) = std::env::var("WINEO_TEST_GOG_API")
        .ok()
        .filter(|base| !base.is_empty())
    {
        return api::Client::with_base(USER_AGENT, &base);
    }
    api::Client::new(USER_AGENT)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::ffi::OsString;
    use std::sync::Mutex;

    use crate::key_store::{Keyring, KeyringError};
    use crate::paths::Paths;
    use crate::steam::api::fixture::{Server, reply};

    /// A keyring in memory, shareable between threads.
    #[derive(Default)]
    pub struct FakeKeyring(pub Mutex<Option<String>>);

    impl Keyring for FakeKeyring {
        fn get(&self) -> Result<Option<String>, KeyringError> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn set(&self, key: &str) -> Result<(), KeyringError> {
            *self.0.lock().unwrap() = Some(key.to_owned());
            Ok(())
        }

        fn delete(&self) -> Result<bool, KeyringError> {
            Ok(self.0.lock().unwrap().take().is_some())
        }
    }

    pub fn setup() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            None,
        )
        .unwrap();
        (root, paths)
    }

    pub fn client(server: &Server) -> super::api::Client {
        super::api::Client::with_base("wineo-test", &server.base).unwrap()
    }

    /// A token endpoint answer.
    pub fn tokens(access: &str, refresh: &str, user_id: u64, expires_in: u64) -> Vec<u8> {
        let body = serde_json::json!({
            "access_token": access,
            "refresh_token": refresh,
            "expires_in": expires_in,
            "user_id": user_id.to_string(),
        });
        reply(200, &body.to_string())
    }
}
