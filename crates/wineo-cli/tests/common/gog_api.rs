//! A local HTTP server standing in for GOG. Debug builds of wineo send every
//! GOG request to the server named by `WINEO_TEST_GOG_API`, where all the
//! services share one base, so requests are routed by path.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

/// What the server answers with. Tests change it between runs.
pub struct Config {
    /// The only authorization code the token endpoint accepts.
    pub code: String,
    pub user_id: u64,
    /// Refresh tokens are refused with `invalid_grant`.
    pub revoked: bool,
    /// The one refresh token that works: the last one handed out. Setting
    /// it to anything else makes the saved login stale.
    pub refresh: Option<String>,
    /// The access tokens handed out, which are the ones that work.
    access: BTreeSet<String>,
    /// The token endpoint fails with this status instead of answering.
    pub token_status: Option<u16>,
    /// Owned games as `getFilteredProducts` lists them.
    pub owned: Vec<Value>,
    /// The owned list fails with this status instead of answering.
    pub owned_status: Option<u16>,
    /// Owned games per page. All on one page when unset.
    pub page_size: Option<usize>,
    /// This page of the owned list fails with this status.
    pub owned_page_status: Option<(u32, u16)>,
    /// Product details by id. A product not listed is a 404.
    pub products: BTreeMap<u64, Value>,
    /// Products whose details fail with this status.
    pub product_status: BTreeMap<u64, u16>,
    /// Windows build lists by product id.
    pub builds: BTreeMap<u64, Value>,
    renewals: u32,
}

pub const CODE: &str = "GoodCode-SECRET-1234";
pub const USER: u64 = 4242;

impl Default for Config {
    fn default() -> Self {
        Self {
            code: CODE.to_owned(),
            user_id: USER,
            revoked: false,
            refresh: None,
            access: BTreeSet::new(),
            token_status: None,
            owned: Vec::new(),
            owned_status: None,
            page_size: None,
            owned_page_status: None,
            products: BTreeMap::new(),
            product_status: BTreeMap::new(),
            builds: BTreeMap::new(),
            renewals: 0,
        }
    }
}

pub struct GogApi {
    pub base: String,
    config: Arc<Mutex<Config>>,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Fails, naming only the stream, when a command's output holds one of the
/// fake GOG's secrets. Run before anything prints the output.
#[track_caller]
pub fn checked(output: std::process::Output) -> std::process::Output {
    for (stream, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let text = String::from_utf8_lossy(bytes);
        assert!(
            !["SECRET", "GoodCode"]
                .iter()
                .any(|secret| text.contains(secret)),
            "a secret showed up on {stream}"
        );
    }
    output
}

/// The address GOG's login ends on, with `code` in it.
pub fn success_address(code: &str) -> String {
    format!("https://embed.gog.com/on_login_success?origin=client&code={code}")
}

/// An owned game as the list shows it.
pub fn owned_game(id: u64, title: &str, linux: bool, windows: bool) -> Value {
    json!({
        "id": id,
        "title": title,
        "image": format!("//images.example/{id}"),
        "worksOn": {"Windows": windows, "Mac": false, "Linux": linux},
        "slug": title.to_lowercase().replace(' ', "_"),
        "dlcCount": 0,
    })
}

/// One installer or patch in a product's details.
pub fn installer(os: &str, language: &str, version: &str) -> Value {
    json!({
        "id": format!("installer_{os}_{language}"),
        "name": "Installer",
        "os": os,
        "language": language,
        "language_full": language,
        "version": version,
        "total_size": 1,
        "files": [{"id": format!("{language}1installer0"), "size": 1, "downlink": "http://unused.example/link"}],
    })
}

/// A product's details with the given installers and expanded DLC.
pub fn product(id: u64, title: &str, installers: &[Value], dlcs: &[Value]) -> Value {
    json!({
        "id": id,
        "title": title,
        "slug": title.to_lowercase().replace(' ', "_"),
        "game_type": "game",
        "content_system_compatibility": {"windows": true, "osx": false, "linux": true},
        "languages": {"en": "English"},
        "images": {},
        "downloads": {"installers": installers, "patches": []},
        "dlcs": {"products": dlcs.iter().map(|dlc| json!({"id": dlc["id"]})).collect::<Vec<_>>()},
        "expanded_dlcs": dlcs,
    })
}

/// A Windows build list, newest first, each as `(build id, branch)`.
pub fn builds(items: &[(&str, Option<&str>)]) -> Value {
    json!({
        "items": items.iter().map(|(id, branch)| json!({
            "build_id": id,
            "version_name": "1.0",
            "branch": branch,
            "date_published": "2026-01-01T00:00:00+0000",
            "generation": 2,
            "link": "http://unused.example/manifest",
        })).collect::<Vec<_>>(),
        "has_private_branches": false,
    })
}

fn reply(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn query(target: &str, name: &str) -> Option<String> {
    target
        .split_once('?')?
        .1
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .map(str::to_owned)
}

/// The token a request sent as its bearer, if any.
fn bearer(head: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then(|| value.trim().strip_prefix("Bearer ").map(str::to_owned))
            .flatten()
    })
}

fn unauthorized() -> String {
    reply(401, "{}")
}

fn answer(config: &mut Config, target: &str, bearer: Option<&str>) -> String {
    let path = target.split('?').next().unwrap_or_default();
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let signed_in = bearer.is_some_and(|token| config.access.contains(token));
    match segments.as_slice() {
        ["token"] => {
            if let Some(status) = config.token_status {
                return reply(status, "{}");
            }
            let granted = match query(target, "grant_type").as_deref() {
                Some("authorization_code") => {
                    query(target, "code").as_deref() == Some(&config.code)
                }
                Some("refresh_token") => {
                    !config.revoked
                        && query(target, "refresh_token")
                            .is_some_and(|token| config.refresh.as_deref() == Some(token.as_str()))
                }
                _ => false,
            };
            if !granted {
                return reply(400, r#"{"error":"invalid_grant"}"#);
            }
            config.renewals += 1;
            let access = format!("ACCESS-SECRET-{}", config.renewals);
            let refresh = format!("REFRESH-SECRET-{}", config.renewals);
            config.access.insert(access.clone());
            config.refresh = Some(refresh.clone());
            reply(
                200,
                &json!({
                    "access_token": access,
                    "refresh_token": refresh,
                    "expires_in": 3600,
                    "user_id": config.user_id.to_string(),
                })
                .to_string(),
            )
        }
        ["account", "getFilteredProducts"] => {
            if !signed_in {
                return unauthorized();
            }
            if let Some(status) = config.owned_status {
                return reply(status, "{}");
            }
            let page: u32 = query(target, "page")
                .and_then(|page| page.parse().ok())
                .unwrap_or(1);
            if let Some((failing, status)) = config.owned_page_status
                && failing == page
            {
                return reply(status, "{}");
            }
            let size = config.page_size.unwrap_or(config.owned.len().max(1));
            let total = config.owned.len().div_ceil(size).max(1) as u32;
            let products: Vec<&Value> = config
                .owned
                .iter()
                .skip(size * (page as usize - 1))
                .take(size)
                .collect();
            reply(
                200,
                &json!({"page": page, "totalPages": total, "products": products}).to_string(),
            )
        }
        ["products", id] => {
            let Ok(id) = id.parse::<u64>() else {
                return reply(404, "{}");
            };
            if let Some(status) = config.product_status.get(&id) {
                return reply(*status, "{}");
            }
            match config.products.get(&id) {
                Some(product) => reply(200, &product.to_string()),
                None => reply(404, "{}"),
            }
        }
        ["products", id, "os", "windows", "builds"] => {
            if !signed_in {
                return unauthorized();
            }
            match id.parse::<u64>().ok().and_then(|id| config.builds.get(&id)) {
                Some(builds) => reply(200, &builds.to_string()),
                None => reply(404, "{}"),
            }
        }
        _ => reply(404, "{}"),
    }
}

impl GogApi {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let config = Arc::new(Mutex::new(Config::default()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (config, requests, stop) = (config.clone(), requests.clone(), stop.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut chunk = [0_u8; 1024];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => request.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let text = String::from_utf8_lossy(&request);
                    let line = text.lines().next().unwrap_or_default().to_owned();
                    requests.lock().unwrap().push(line.clone());
                    let target = line.split(' ').nth(1).unwrap_or("/");
                    let token = bearer(&text);
                    let response = answer(&mut config.lock().unwrap(), target, token.as_deref());
                    let _ = stream.write_all(response.as_bytes());
                }
            })
        };
        Self {
            base,
            config,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    /// A server with `owned` games and their `products` already set up.
    pub fn with_library(owned: Vec<Value>, products: Vec<Value>) -> Self {
        let api = Self::start();
        api.with(|config| {
            config.owned = owned;
            for product in products {
                config
                    .products
                    .insert(product["id"].as_u64().unwrap(), product);
            }
        });
        api
    }

    pub fn with(&self, change: impl FnOnce(&mut Config)) {
        change(&mut self.config.lock().unwrap());
    }

    /// Asks the server directly, the way a client would. The status and body.
    pub fn get(&self, target: &str, bearer: Option<&str>) -> (u16, String) {
        use std::io::{Read, Write};
        let mut stream =
            std::net::TcpStream::connect(self.base.trim_start_matches("http://")).unwrap();
        let auth = bearer
            .map(|token| format!("Authorization: Bearer {token}\r\n"))
            .unwrap_or_default();
        write!(
            stream,
            "GET {target} HTTP/1.1\r\nHost: fake\r\n{auth}Connection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let status = response.split(' ').nth(1).unwrap().parse().unwrap();
        let body = response
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or_default()
            .to_owned();
        (status, body)
    }

    /// The request lines seen so far, like `GET /path?query HTTP/1.1`.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// How many requests went to the owned list.
    pub fn owned_requests(&self) -> usize {
        self.requests()
            .iter()
            .filter(|line| line.contains("/account/getFilteredProducts"))
            .count()
    }
}

impl Drop for GogApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
