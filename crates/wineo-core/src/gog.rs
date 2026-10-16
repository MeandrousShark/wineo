//! GOG accounts and libraries, on top of the `wineo-gog` client: where the
//! login is kept, how it is renewed, the cached owned list, migrating the
//! games Heroic installed, and importing GOG's playtime and artwork for them.

pub mod account;
pub mod covers;
pub mod extras;
pub mod folder;
pub mod install_dir;
pub mod language;
pub mod migrate;
pub mod names;
pub mod owned;
pub mod package;
pub mod product;
pub mod update;

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
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::sync::Mutex;

    use crate::key_store::{Keyring, KeyringError};
    use crate::paths::Paths;
    use crate::steam::api::fixture::{Server, reply};

    /// A keyring in memory, shareable between threads. The second slot holds
    /// private-branch passwords by game and branch.
    #[derive(Default)]
    pub struct FakeKeyring(
        pub Mutex<Option<String>>,
        pub Mutex<BTreeMap<(u64, String), String>>,
    );

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

        fn get_branch(&self, game: u64, branch: &str) -> Result<Option<String>, KeyringError> {
            Ok(self
                .1
                .lock()
                .unwrap()
                .get(&(game, branch.to_owned()))
                .cloned())
        }

        fn set_branch(&self, game: u64, branch: &str, password: &str) -> Result<(), KeyringError> {
            self.1
                .lock()
                .unwrap()
                .insert((game, branch.to_owned()), password.to_owned());
            Ok(())
        }

        fn delete_branch(&self, game: u64, branch: &str) -> Result<bool, KeyringError> {
            Ok(self
                .1
                .lock()
                .unwrap()
                .remove(&(game, branch.to_owned()))
                .is_some())
        }

        fn list_branches(&self, game: u64) -> Result<Vec<(String, String)>, KeyringError> {
            Ok(self
                .1
                .lock()
                .unwrap()
                .iter()
                .filter(|((id, _), _)| *id == game)
                .map(|((_, branch), password)| (branch.clone(), password.clone()))
                .collect())
        }

        fn delete_all_branches(&self) -> Result<bool, KeyringError> {
            let mut all = self.1.lock().unwrap();
            let had = !all.is_empty();
            all.clear();
            Ok(had)
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
        client_at(&server.base)
    }

    /// A client for a fake GOG whose profile service is somewhere nothing
    /// listens, so looking up a name fails fast instead of spending the
    /// answers a test scripted for its own requests.
    pub fn client_at(base: &str) -> super::api::Client {
        let mut endpoints = super::api::Endpoints::all_at(base).unwrap();
        endpoints.users = "http://127.0.0.1:9".parse().unwrap();
        super::api::Client::with_endpoints("wineo-test", endpoints).unwrap()
    }

    /// A client for a fake GOG that serves profiles too.
    pub fn client_with_profiles(base: &str) -> super::api::Client {
        super::api::Client::with_base("wineo-test", base).unwrap()
    }

    pub use wineo_gog::fake::Scripted;

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
