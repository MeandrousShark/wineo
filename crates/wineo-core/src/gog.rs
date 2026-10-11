//! GOG accounts and libraries, on top of the `wineo-gog` client: where the
//! login is kept, how it is renewed, and the cached owned list.

pub mod account;
pub mod owned;
pub mod product;

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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

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
        client_at(&server.base)
    }

    pub fn client_at(base: &str) -> super::api::Client {
        super::api::Client::with_base("wineo-test", base).unwrap()
    }

    /// A server that answers each request line through a handler, one
    /// request at a time. The handler runs while the client waits for its
    /// answer, so a test can change the saved login at exactly that point.
    pub struct Scripted {
        pub base: String,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
    }

    impl Scripted {
        pub fn start(handler: impl Fn(&str) -> Vec<u8> + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (seen, stopped) = (Arc::clone(&requests), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stopped.load(Ordering::Relaxed) {
                    let mut stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(_) => return,
                    };
                    stream.set_nonblocking(false).unwrap();
                    let mut head = Vec::new();
                    let mut chunk = [0; 1024];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => head.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let line = String::from_utf8_lossy(&head)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    seen.lock().unwrap().push(line.clone());
                    let _ = stream.write_all(&handler(&line));
                    let _ = stream.flush();
                }
            });
            Self {
                base,
                requests,
                stop,
            }
        }

        pub fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Drop for Scripted {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
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
