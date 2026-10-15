//! A local HTTP server standing in for GOG. Debug builds of wineo send every
//! GOG request to the server named by `WINEO_TEST_GOG_API`, where all the
//! services share one base, so requests are routed by path.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use wineo_gog::fake::content::ContentSystem;
use wineo_gog::fake::{Reply, Request};

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
    /// Every id the account owns, DLC included. Unset, the list is a 404.
    pub owned_ids: Option<Vec<u64>>,
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
    /// Package files to download, by file name.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Total playtime in minutes by game id. A game not listed has none (404).
    pub playtime: BTreeMap<u64, u64>,
    /// Artwork links by game id, as `_links` holds them. A game not listed
    /// is a 404.
    pub art: BTreeMap<u64, Value>,
    /// The account's public name. Without one the profile is a 404.
    pub username: Option<String>,
    /// The profile fails with this status instead of answering.
    pub profile_status: Option<u16>,
    /// File names whose downloads GOG publishes no checksum for.
    pub no_checksum: BTreeSet<String>,
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
            owned_ids: None,
            page_size: None,
            owned_page_status: None,
            products: BTreeMap::new(),
            product_status: BTreeMap::new(),
            builds: BTreeMap::new(),
            files: BTreeMap::new(),
            no_checksum: BTreeSet::new(),
            playtime: BTreeMap::new(),
            art: BTreeMap::new(),
            username: None,
            profile_status: None,
            renewals: 0,
        }
    }
}

pub struct GogApi {
    pub base: String,
    /// Galaxy builds, manifests, and chunks. Asked before the rest.
    pub content: Arc<ContentSystem>,
    /// Lists the builds the other way round, the last one added first, as
    /// GOG lists a newer build before an older one.
    pub newest_first: Arc<AtomicBool>,
    /// Builds GOG no longer lists, by id.
    pub hidden_builds: Arc<Mutex<BTreeSet<String>>>,
    /// Build lists fail with this status while it isn't 0.
    pub builds_status: Arc<AtomicU16>,
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

/// A Windows build list, newest first, each as `(build id, branch)`. Every
/// build is generation 2 and named `v` and its id.
pub fn builds(items: &[(&str, Option<&str>)]) -> Value {
    let items: Vec<_> = items.iter().map(|&(id, branch)| (id, branch, 2)).collect();
    builds_of(&items)
}

/// `builds`, each with its generation.
pub fn builds_of(items: &[(&str, Option<&str>, u8)]) -> Value {
    json!({
        "items": items.iter().map(|(id, branch, generation)| json!({
            "build_id": id,
            "version_name": format!("v{id}"),
            "branch": branch,
            "date_published": "2026-01-01T00:00:00+0000",
            "generation": generation,
            "link": "http://unused.example/manifest",
        })).collect::<Vec<_>>(),
        "has_private_branches": false,
    })
}

fn md5_hex(bytes: &[u8]) -> String {
    use md5::Digest as _;
    wineo_core::units::hex(md5::Md5::digest(bytes))
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

/// The request line and headers of `head`, as the content system reads them.
fn content_request(head: &str) -> Request {
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split(' ');
    Request {
        method: first.next().unwrap_or_default().to_owned(),
        target: first.next().unwrap_or("/").to_owned(),
        headers: lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
            .collect(),
    }
}

/// A build list's reply without the `hidden` builds, and with the rest the
/// other way round when `reverse`.
fn shaped(reply: &[u8], reverse: bool, hidden: &BTreeSet<String>) -> Vec<u8> {
    let text = String::from_utf8_lossy(reply);
    let body = &text[text.find("\r\n\r\n").unwrap() + 4..];
    let mut list: Value = serde_json::from_str(body).unwrap();
    let items = list["items"].as_array_mut().unwrap();
    items.retain(|item| !hidden.contains(item["build_id"].as_str().unwrap_or_default()));
    if reverse {
        items.reverse();
    }
    reply_bytes(200, &list.to_string())
}

fn reply_bytes(status: u16, body: &str) -> Vec<u8> {
    reply(status, body).into_bytes()
}

/// Answers a request with the bytes to send back. File downloads aren't
/// text, so they are served here and everything else by `answer`.
fn respond(config: &mut Config, base: &str, target: &str, bearer: Option<&str>) -> Vec<u8> {
    let path = target.split('?').next().unwrap_or_default();
    if let Some(name) = path.strip_prefix("/files/") {
        return match config.files.get(name) {
            Some(bytes) => {
                let mut response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .into_bytes();
                response.extend(bytes);
                response
            }
            None => reply(404, "{}").into_bytes(),
        };
    }
    answer(config, base, target, bearer).into_bytes()
}

fn answer(config: &mut Config, base: &str, target: &str, bearer: Option<&str>) -> String {
    let path = target.split('?').next().unwrap_or_default();
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let signed_in = bearer.is_some_and(|token| config.access.contains(token));
    match segments.as_slice() {
        ["downlink", name] => {
            if !signed_in {
                return unauthorized();
            }
            if !config.files.contains_key(*name) {
                return reply(404, "{}");
            }
            let checksum =
                (!config.no_checksum.contains(*name)).then(|| format!("{base}/sums/{name}"));
            reply(
                200,
                &json!({"downlink": format!("{base}/files/{name}"), "checksum": checksum})
                    .to_string(),
            )
        }
        ["games", id, "users", _user, "sessions"] => {
            if !signed_in {
                return unauthorized();
            }
            match id
                .parse::<u64>()
                .ok()
                .and_then(|id| config.playtime.get(&id))
            {
                Some(minutes) => reply(200, &json!({"time_sum": minutes}).to_string()),
                None => reply(404, "{}"),
            }
        }
        ["v2", "games", id] => {
            let Ok(id) = id.parse::<u64>() else {
                return reply(404, "{}");
            };
            match config.art.get(&id) {
                Some(links) => reply(
                    200,
                    &json!({"_embedded": {"product": {"id": id}}, "_links": links}).to_string(),
                ),
                None => reply(404, "{}"),
            }
        }
        ["users", id] => {
            if let Some(status) = config.profile_status {
                return reply(status, "{}");
            }
            match &config.username {
                Some(name) => reply(200, &json!({"id": id, "username": name}).to_string()),
                None => reply(404, "{}"),
            }
        }
        ["sums", name] => match config.files.get(*name) {
            Some(bytes) => reply(
                200,
                &format!(
                    r#"<file name="{name}" md5="{}" total_size="{}"/>"#,
                    md5_hex(bytes),
                    bytes.len()
                ),
            ),
            None => reply(404, "{}"),
        },
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
        ["user", "data", "games"] => {
            if !signed_in {
                return unauthorized();
            }
            match &config.owned_ids {
                Some(ids) => reply(200, &json!({ "owned": ids }).to_string()),
                None => reply(404, "{}"),
            }
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
        let content = Arc::new(ContentSystem::new().with_chunk_size(8));
        let newest_first = Arc::new(AtomicBool::new(false));
        let builds_status = Arc::new(AtomicU16::new(0));
        let hidden_builds = Arc::new(Mutex::new(BTreeSet::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (config, requests, stop) = (config.clone(), requests.clone(), stop.clone());
            let content = content.clone();
            let newest_first = newest_first.clone();
            let builds_status = builds_status.clone();
            let hidden = hidden_builds.clone();
            let base = base.clone();
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
                    let failing = builds_status.load(Ordering::Relaxed);
                    let lists_builds = target
                        .split('?')
                        .next()
                        .is_some_and(|path| path.ends_with("/builds"));
                    let response = match content
                        .answer(&content_request(&text))
                        .filter(|_| failing == 0 || !lists_builds)
                    {
                        Some(Reply::Send(bytes) | Reply::Stall(bytes, _)) => {
                            let hidden = hidden.lock().unwrap();
                            if lists_builds
                                && (newest_first.load(Ordering::Relaxed) || !hidden.is_empty())
                            {
                                shaped(&bytes, newest_first.load(Ordering::Relaxed), &hidden)
                            } else {
                                bytes
                            }
                        }
                        None if failing != 0 && lists_builds => reply_bytes(failing, "{}"),
                        None => {
                            respond(&mut config.lock().unwrap(), &base, target, token.as_deref())
                        }
                    };
                    let _ = stream.write_all(&response);
                }
            })
        };
        Self {
            base,
            content,
            newest_first,
            builds_status,
            hidden_builds,
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

    /// Puts a Linux installer for `id` on offer, serving `bytes` as its one
    /// file, and returns its entry for a product's details.
    pub fn linux_package(&self, id: u64, language: &str, version: &str, bytes: &[u8]) -> Value {
        let name = format!(
            "game_{id}_{}.sh",
            version.replace(|char: char| !char.is_ascii_alphanumeric(), "_")
        );
        self.with(|config| {
            config.files.insert(name.clone(), bytes.to_vec());
        });
        json!({
            "id": format!("installer_linux_{language}"),
            "name": "Installer",
            "os": "linux",
            "language": language,
            "language_full": language,
            "version": version,
            "total_size": bytes.len(),
            "files": [{
                "id": format!("{language}3installer0"),
                "size": bytes.len(),
                "downlink": format!("{}/downlink/{name}", self.base),
            }],
        })
    }

    /// Puts a Windows installer for `id` on offer: a setup program, `script`,
    /// which the fake Inno runner sources, and one part beside it. Returns
    /// its entry for a product's details.
    pub fn windows_package(&self, id: u64, language: &str, version: &str, script: &str) -> Value {
        let stem = format!(
            "setup_{id}_{}",
            version.replace(|char: char| !char.is_ascii_alphanumeric(), "_")
        );
        let exe = format!("{stem}.exe");
        let part = format!("{stem}-1.bin");
        let script = format!("[ -f \"$(dirname \"$exe\")/{part}\" ] || exit 99\n{script}");
        let part_bytes = b"the rest of the installer".to_vec();
        let sizes = [script.len(), part_bytes.len()];
        self.with(|config| {
            config.files.insert(exe.clone(), script.into_bytes());
            config.files.insert(part.clone(), part_bytes);
        });
        json!({
            "id": format!("installer_windows_{language}"),
            "name": "Installer",
            "os": "windows",
            "language": language,
            "language_full": language,
            "version": version,
            "total_size": sizes[0] + sizes[1],
            "files": [
                {
                    "id": format!("{language}1installer0"),
                    "size": sizes[0],
                    "downlink": format!("{}/downlink/{exe}", self.base),
                },
                {
                    "id": format!("{language}1installer1"),
                    "size": sizes[1],
                    "downlink": format!("{}/downlink/{part}", self.base),
                },
            ],
        })
    }

    /// How many package files were downloaded.
    pub fn downloads(&self) -> usize {
        self.requests()
            .iter()
            .filter(|line| line.starts_with("GET /files/"))
            .count()
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
