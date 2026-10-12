//! A stand-in for GOG's servers in tests, like the core and CLI tests use:
//! plain HTTP on localhost, answering each request line through a handler.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wineo_core::gog::api::Client;
use wineo_core::key_store::Keyring;

pub(crate) struct Server {
    pub(crate) base: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    pub(crate) fn start(handler: impl Fn(&str) -> Vec<u8> + Send + 'static) -> Self {
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

    pub(crate) fn client(&self) -> Client {
        Client::with_base("wineo-test", &self.base).unwrap()
    }

    pub(crate) fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn reply(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// A token endpoint answer.
pub(crate) fn tokens(refresh: &str, user_id: u64) -> Vec<u8> {
    let body = serde_json::json!({
        "access_token": "access-token-value",
        "refresh_token": refresh,
        "expires_in": 3600,
        "user_id": user_id.to_string(),
    });
    reply(200, &body.to_string())
}

/// Answers GOG's token endpoint and `/products/<id>` from `products`, and
/// serves their installers: `/downlink/<file>` resolves a file and
/// `/files/<file>` sends `FILE_SIZE` bytes.
pub(crate) fn product_server(products: Vec<serde_json::Value>) -> Server {
    product_server_slow(products, Duration::ZERO)
}

/// `product_server`, taking `delay` to answer each `/products` request, like
/// a slow GOG.
pub(crate) fn product_server_slow(products: Vec<serde_json::Value>, delay: Duration) -> Server {
    let base = Arc::new(std::sync::OnceLock::<String>::new());
    let seen = Arc::clone(&base);
    let server = Server::start(move |line| {
        let base = seen.get().cloned().unwrap_or_default();
        let path = line.split(' ').nth(1).unwrap_or_default();
        let path = path.split('?').next().unwrap_or_default();
        if path == "/token" {
            return tokens("REFRESH", USER);
        }
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        match parts.as_slice() {
            ["products", id] => {
                std::thread::sleep(delay);
                products
                    .iter()
                    .find(|product| id.parse::<u64>().ok() == product["id"].as_u64())
                    .map_or_else(
                        || reply(404, "{}"),
                        |body| reply(200, &body.to_string().replace("{BASE}", &base)),
                    )
            }
            ["downlink", name] => reply(
                200,
                &serde_json::json!({
                    "downlink": format!("{base}/files/{name}"),
                    "checksum": null,
                })
                .to_string(),
            ),
            ["files", _] => {
                let mut answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {FILE_SIZE}\r\nConnection: close\r\n\r\n"
                )
                .into_bytes();
                answer.resize(answer.len() + FILE_SIZE as usize, 0);
                answer
            }
            _ => reply(404, "{}"),
        }
    });
    let _ = base.set(server.base.clone());
    server
}

/// The size of every installer the fake GOG serves.
pub(crate) const FILE_SIZE: u64 = 7340032;

pub(crate) const USER: u64 = 1001;

/// GOG's details for a native game with one Linux installer.
pub(crate) fn native_product(id: u64, title: &str, version: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "downloads": {"installers": [{
            "id": format!("installer_linux_en_{id}"),
            "os": "linux",
            "language": "en",
            "version": version,
            "total_size": FILE_SIZE,
            "files": [{
                "id": "f",
                "size": FILE_SIZE,
                "downlink": format!("{{BASE}}/downlink/game_{id}.sh"),
            }],
        }]},
        "expanded_dlcs": [],
    })
}

/// Saves a login the fake server's token endpoint renews.
pub(crate) fn save_login(paths: &wineo_core::paths::Paths, keyring: &dyn Keyring) {
    let secret = format!(r#"{{"refresh_token":"R","user_id":"{USER}"}}"#);
    wineo_core::key_store::save_key(&wineo_core::gog::account::KEY_SPEC, paths, keyring, &secret)
        .unwrap();
}

/// A native GOG game folder under `root` owned by environment `name`, which
/// launches its `start.sh`, at installed `version`, with a journal when
/// `interrupted`.
pub(crate) fn native_game(
    paths: &wineo_core::paths::Paths,
    root: &std::path::Path,
    name: &str,
    gog_id: u64,
    version: &str,
    interrupted: bool,
) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    use wineo_core::gog::folder::{
        Committed, FolderLock, FolderState, Journal, JournalKind, Platform, Product, ProductStatus,
        Target, Track,
    };
    let folder = std::fs::canonicalize(root)
        .unwrap()
        .join("Games")
        .join(name);
    std::fs::create_dir_all(&folder).unwrap();
    let start = folder.join("start.sh");
    std::fs::write(&start, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&start, std::fs::Permissions::from_mode(0o755)).unwrap();
    native_environment(paths, name, &start, Some(gog_id));
    let products = vec![Product {
        id: gog_id,
        version: Some(version.to_owned()),
        build: None,
        status: ProductStatus::Installed,
    }];
    let state = FolderState {
        committed: Some(Committed {
            gog_id,
            platform: Platform::Linux,
            language: "en-US".to_owned(),
            track: Track::Installer,
            products: products.clone(),
            files: Vec::new(),
        }),
        journal: interrupted.then(|| Journal {
            operation: "op".to_owned(),
            kind: JournalKind::Update,
            target: Target {
                gog_id,
                platform: Platform::Linux,
                language: "en-US".to_owned(),
                track: Track::Installer,
                products,
            },
            packages: Vec::new(),
            attempted: Vec::new(),
        }),
        ..FolderState::new(folder.clone(), Some(name.to_owned()))
    };
    FolderLock::exclusive(paths, &folder, &|| false)
        .unwrap()
        .write(&state)
        .unwrap();
    folder
}

/// A native environment that runs `exe`, tracking `gog_id` when given.
pub(crate) fn native_environment(
    paths: &wineo_core::paths::Paths,
    name: &str,
    exe: &std::path::Path,
    gog_id: Option<u64>,
) {
    let config = paths.environment_config(name);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let gog = gog_id.map_or_else(String::new, |id| format!("gog_id = \"{id}\"\n"));
    std::fs::write(
        config,
        format!(
            "kind = \"native\"\ntags = [\"game\"]\n{gog}\n[targets.game]\nexe = \"{}\"\ndefault = true\n",
            exe.display()
        ),
    )
    .unwrap();
}
