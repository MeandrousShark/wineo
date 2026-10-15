//! A Galaxy content system for a fake GOG: build lists, zlib manifests,
//! secure links with two CDNs, chunks, and the redistributables, with ways
//! to make each of them fail.
//!
//! A test declares files, and the builder cuts them into chunks and writes
//! the manifests that list them. `ContentSystem::answer` serves what it
//! knows and leaves every other request to the test's own handler:
//!
//! ```ignore
//! let content = Arc::new(ContentSystem::new());
//! content.add_build(FakeBuild::new(1, "100").depot(
//!     FakeDepot::new(1).file(FakeFile::new("bin\\game.exe", b"MZ").flag("executable")),
//! ));
//! let served = Arc::clone(&content);
//! let server = FakeGog::start(move |request| {
//!     served.answer(request).unwrap_or_else(|| json(404, "{}"))
//! });
//! ```
//!
//! URLs in answers point back at whichever host the request was sent to,
//! so the content system needs no address before the server starts.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::write::ZlibEncoder;
use md5::{Digest, Md5};
use serde_json::{Value, json as value};
use sha2::Sha256;

use super::{Reply, Request, json, response};
use crate::manifest::Chunk;

/// A file in a fake depot.
#[derive(Debug, Clone)]
pub struct FakeFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub flags: Vec<String>,
    pub md5: bool,
    pub sha256: bool,
    /// Also listed in the depot's small-file container, with an `sfcRef`.
    pub in_container: bool,
}

impl FakeFile {
    pub fn new(path: &str, bytes: &[u8]) -> Self {
        Self {
            path: path.to_owned(),
            bytes: bytes.to_vec(),
            flags: Vec::new(),
            md5: false,
            sha256: false,
            in_container: false,
        }
    }

    pub fn flag(mut self, flag: &str) -> Self {
        self.flags.push(flag.to_owned());
        self
    }

    /// Lists the whole file's MD5.
    pub fn with_md5(mut self) -> Self {
        self.md5 = true;
        self
    }

    /// Lists the whole file's SHA-256.
    pub fn with_sha256(mut self) -> Self {
        self.sha256 = true;
        self
    }

    pub fn in_container(mut self) -> Self {
        self.in_container = true;
        self
    }
}

#[derive(Debug, Clone)]
pub enum FakeItem {
    File(FakeFile),
    Directory(String),
    Link { path: String, target: String },
}

/// A depot and its items.
#[derive(Debug, Clone)]
pub struct FakeDepot {
    pub product_id: u64,
    pub languages: Vec<String>,
    pub os_bitness: Vec<String>,
    pub is_gog_depot: bool,
    pub items: Vec<FakeItem>,
}

impl FakeDepot {
    /// A depot for every language and architecture.
    pub fn new(product_id: u64) -> Self {
        Self {
            product_id,
            languages: vec!["*".to_owned()],
            os_bitness: Vec::new(),
            is_gog_depot: false,
            items: Vec::new(),
        }
    }

    pub fn languages(mut self, languages: &[&str]) -> Self {
        self.languages = languages.iter().map(|&code| code.to_owned()).collect();
        self
    }

    pub fn bitness(mut self, bits: &[&str]) -> Self {
        self.os_bitness = bits.iter().map(|&bits| bits.to_owned()).collect();
        self
    }

    pub fn gog_depot(mut self) -> Self {
        self.is_gog_depot = true;
        self
    }

    pub fn file(mut self, file: FakeFile) -> Self {
        self.items.push(FakeItem::File(file));
        self
    }

    pub fn directory(mut self, path: &str) -> Self {
        self.items.push(FakeItem::Directory(path.to_owned()));
        self
    }

    pub fn link(mut self, path: &str, target: &str) -> Self {
        self.items.push(FakeItem::Link {
            path: path.to_owned(),
            target: target.to_owned(),
        });
        self
    }
}

/// A product listed in a build's repository manifest.
#[derive(Debug, Clone)]
pub struct FakeProduct {
    pub product_id: u64,
    pub name: String,
    pub temp_executable: String,
    pub temp_arguments: String,
}

/// A build and what it installs.
#[derive(Debug, Clone)]
pub struct FakeBuild {
    pub product_id: u64,
    pub build_id: String,
    pub version_name: String,
    /// None for the default branch.
    pub branch: Option<String>,
    pub generation: u64,
    pub date_published: String,
    pub install_directory: String,
    pub script_interpreter: bool,
    pub dependencies: Vec<String>,
    /// Defaults to the base product alone.
    pub products: Vec<FakeProduct>,
    pub depots: Vec<FakeDepot>,
}

impl FakeBuild {
    pub fn new(product_id: u64, build_id: &str) -> Self {
        Self {
            product_id,
            build_id: build_id.to_owned(),
            version_name: format!("v{build_id}"),
            branch: None,
            generation: 2,
            date_published: "2026-01-02T03:04:05+0000".to_owned(),
            install_directory: "Fake Game".to_owned(),
            script_interpreter: false,
            dependencies: Vec::new(),
            products: Vec::new(),
            depots: Vec::new(),
        }
    }

    pub fn branch(mut self, branch: &str) -> Self {
        self.branch = Some(branch.to_owned());
        self
    }

    pub fn depot(mut self, depot: FakeDepot) -> Self {
        self.depots.push(depot);
        self
    }

    pub fn dependency(mut self, id: &str) -> Self {
        self.dependencies.push(id.to_owned());
        self
    }

    pub fn product(mut self, product_id: u64, temp_executable: &str) -> Self {
        self.products.push(FakeProduct {
            product_id,
            name: format!("Product {product_id}"),
            temp_executable: temp_executable.to_owned(),
            temp_arguments: String::new(),
        });
        self
    }
}

/// A redistributable and its files.
#[derive(Debug, Clone)]
pub struct FakeDependency {
    pub id: String,
    pub readable_name: String,
    /// The program and its arguments. Empty for one with no installer.
    pub executable: (String, String),
    pub internal: bool,
    /// Its product id is ignored.
    pub depot: FakeDepot,
}

impl FakeDependency {
    pub fn new(id: &str, executable: &str, arguments: &str, depot: FakeDepot) -> Self {
        Self {
            id: id.to_owned(),
            readable_name: format!("{id} redistributable"),
            executable: (executable.to_owned(), arguments.to_owned()),
            internal: false,
            depot,
        }
    }
}

/// Which CDN of a secure link a chunk request went to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cdn {
    /// Priority 10, not fallback only.
    Primary,
    /// Priority 20, but fallback only, and listed first, so a client that
    /// ignores `fallback_only` would pick it.
    Fallback,
}

#[derive(Default)]
struct State {
    chunk_size: usize,
    /// Each product's build list items, in the order added.
    builds: BTreeMap<u64, Vec<Value>>,
    /// Build repositories and depot manifests, by hash, zlib or plain.
    meta: BTreeMap<String, Vec<u8>>,
    /// The redistributables' manifest and depot manifests, by hash.
    dependency_meta: BTreeMap<String, Vec<u8>>,
    /// The redistributables' build id and manifest hash.
    dependency_repository: Option<(String, String)>,
    /// Compressed chunks by their MD5.
    chunks: BTreeMap<String, Vec<u8>>,
    plain_json: bool,
    link_lifetime: u64,
    /// Tokens handed out and when each expires.
    tokens: BTreeMap<String, u64>,
    issued: u64,
    refusal: u16,
    /// Chunks served with a byte changed.
    corrupt: BTreeSet<String>,
    /// Chunks the primary CDN answers with a 404.
    missing_on_primary: BTreeSet<String>,
}

pub struct ContentSystem {
    state: Mutex<State>,
}

impl Default for ContentSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentSystem {
    /// Chunks of 1 MiB, links lasting a day, and refusals with a 403.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                chunk_size: 1024 * 1024,
                link_lifetime: 24 * 60 * 60,
                refusal: 403,
                ..State::default()
            }),
        }
    }

    pub fn with_chunk_size(self, chunk_size: usize) -> Self {
        assert!(chunk_size > 0);
        self.state.lock().unwrap().chunk_size = chunk_size;
        self
    }

    /// Lists a build and serves its manifests and chunks. Returns the
    /// repository manifest's hash.
    pub fn add_build(&self, build: FakeBuild) -> String {
        let mut state = self.state.lock().unwrap();
        let depots: Vec<Value> = build
            .depots
            .iter()
            .map(|depot| {
                let hash = state.add_depot(depot, false);
                let size: usize = files(depot).map(|file| file.bytes.len()).sum();
                let mut entry = value!({
                    "compressedSize": size,
                    "languages": depot.languages,
                    "manifest": hash,
                    "productId": depot.product_id.to_string(),
                    "size": size,
                });
                if !depot.os_bitness.is_empty() {
                    entry["osBitness"] = value!(depot.os_bitness);
                }
                if depot.is_gog_depot {
                    entry["isGogDepot"] = value!(true);
                }
                entry
            })
            .collect();
        let products = if build.products.is_empty() {
            vec![FakeProduct {
                product_id: build.product_id,
                name: "Fake Game".to_owned(),
                temp_executable: String::new(),
                temp_arguments: String::new(),
            }]
        } else {
            build.products.clone()
        };
        let mut repository = value!({
            "baseProductId": build.product_id.to_string(),
            "buildId": build.build_id,
            "clientId": "1",
            "clientSecret": "fake",
            "depots": depots,
            "installDirectory": build.install_directory,
            // Never read; real ones hold only a project.json.
            "offlineDepot": {
                "compressedSize": 1,
                "languages": ["*"],
                "manifest": "0".repeat(32),
                "productId": build.product_id.to_string(),
                "size": 1,
            },
            "platform": "windows",
            "products": products.iter().map(|product| value!({
                "name": product.name,
                "productId": product.product_id.to_string(),
                "temp_arguments": product.temp_arguments,
                "temp_executable": product.temp_executable,
            })).collect::<Vec<_>>(),
            "scriptInterpreter": build.script_interpreter,
            "tags": [],
            "version": 2,
        });
        if !build.dependencies.is_empty() {
            repository["dependencies"] = value!(build.dependencies);
        }
        let hash = state.add_meta(&repository, false);
        state
            .builds
            .entry(build.product_id)
            .or_default()
            .push(value!({
                "build_id": build.build_id,
                "product_id": build.product_id.to_string(),
                "os": "windows",
                "branch": build.branch,
                "version_name": build.version_name,
                "tags": [],
                "public": true,
                "date_published": build.date_published,
                "generation": build.generation,
                "meta": hash,
            }));
        hash
    }

    /// Serves the redistributables. Returns the manifest's hash.
    pub fn set_dependencies(&self, build_id: &str, dependencies: &[FakeDependency]) -> String {
        let mut state = self.state.lock().unwrap();
        let depots: Vec<Value> = dependencies
            .iter()
            .map(|dependency| {
                let hash = state.add_depot(&dependency.depot, true);
                let size: usize = files(&dependency.depot).map(|file| file.bytes.len()).sum();
                value!({
                    "compressedSize": size,
                    "dependencyId": dependency.id,
                    "executable": {
                        "arguments": dependency.executable.1,
                        "path": dependency.executable.0,
                    },
                    "internal": dependency.internal,
                    "languages": dependency.depot.languages,
                    "manifest": hash,
                    "readableName": dependency.readable_name,
                    "signature": "ignored",
                    "size": size,
                })
            })
            .collect();
        let hash = state.add_meta(&value!({ "depots": depots }), true);
        state.dependency_repository = Some((build_id.to_owned(), hash.clone()));
        hash
    }

    /// Serves `bytes` as a chunk on its own, and gives the chunk as a
    /// manifest would list it.
    pub fn add_chunk(&self, bytes: &[u8]) -> Chunk {
        self.state.lock().unwrap().add_chunk(bytes)
    }

    /// Serves manifests added from now on as plain JSON rather than zlib.
    pub fn serve_plain_json(&self, plain: bool) {
        self.state.lock().unwrap().plain_json = plain;
    }

    /// How long links handed out from now on last, in seconds.
    pub fn set_link_lifetime(&self, seconds: u64) {
        self.state.lock().unwrap().link_lifetime = seconds;
    }

    /// Makes every link handed out so far refused with `status` until a
    /// new one is fetched.
    pub fn revoke_links(&self, status: u16) {
        let mut state = self.state.lock().unwrap();
        state.tokens.clear();
        state.refusal = status;
    }

    /// Serves the chunk with a byte changed, from both CDNs.
    pub fn corrupt_chunk(&self, compressed_md5: &str) {
        let mut state = self.state.lock().unwrap();
        state.corrupt.insert(compressed_md5.to_owned());
    }

    /// Answers the chunk with a 404 on the primary CDN only.
    pub fn missing_on_primary(&self, compressed_md5: &str) {
        let mut state = self.state.lock().unwrap();
        state.missing_on_primary.insert(compressed_md5.to_owned());
    }

    /// The tokens handed out so far that still work.
    pub fn live_tokens(&self) -> Vec<String> {
        self.state.lock().unwrap().tokens.keys().cloned().collect()
    }

    /// Which CDN a recorded request went to, if it was for a chunk.
    pub fn cdn_of(request: &Request) -> Option<Cdn> {
        let path = request.path();
        if path.starts_with("/primary/") {
            Some(Cdn::Primary)
        } else if path.starts_with("/fallback/") {
            Some(Cdn::Fallback)
        } else {
            None
        }
    }

    /// The answer to a content system request, or None for any other.
    pub fn answer(&self, request: &Request) -> Option<Reply> {
        let base = format!("http://{}", request.header("host")?);
        let path = request.path().to_owned();
        let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let mut state = self.state.lock().unwrap();
        match segments.as_slice() {
            ["products", id, "os", "windows", "builds"] => {
                let items: Vec<Value> = state
                    .builds
                    .get(&id.parse().ok()?)?
                    .iter()
                    .map(|item| {
                        let mut item = item.clone();
                        let hash = item["meta"].as_str().unwrap_or_default().to_owned();
                        item.as_object_mut().unwrap().remove("meta");
                        item["link"] = value!(meta_url(&base, "meta", &hash));
                        item
                    })
                    .collect();
                Some(json(
                    200,
                    &value!({
                        "total_count": items.len(),
                        "count": items.len(),
                        "items": items,
                        "has_private_branches": false,
                    })
                    .to_string(),
                ))
            }
            ["content-system", "v2", "meta", _, _, hash] => {
                Some(Reply::Send(response(200, &[], state.meta.get(*hash)?)))
            }
            ["content-system", "v2", "dependencies", "meta", _, _, hash] => Some(Reply::Send(
                response(200, &[], state.dependency_meta.get(*hash)?),
            )),
            ["dependencies", "repository"] => {
                let (build_id, hash) = state.dependency_repository.clone()?;
                Some(json(
                    200,
                    &value!({
                        "build_id": build_id,
                        "generation": 2,
                        "repository_manifest": meta_url(&base, "dependencies/meta", &hash),
                    })
                    .to_string(),
                ))
            }
            ["products", id, "secure_link"] => {
                let link = state.issue_link(&base, &format!("/content-system/v2/store/{id}"));
                let link = value!({ "product_id": id, "type": "depot", "urls": link });
                Some(json(200, &link.to_string()))
            }
            ["open_link"] => {
                let link = open_link(&base);
                Some(json(200, &value!({ "urls": link }).to_string()))
            }
            ["primary" | "fallback", ..] => Some(state.serve_chunk(request)),
            _ => None,
        }
    }
}

impl State {
    fn add_chunk(&mut self, bytes: &[u8]) -> Chunk {
        let compressed = zlib(bytes);
        let chunk = Chunk {
            md5: md5_hex(bytes),
            size: bytes.len() as u64,
            compressed_md5: md5_hex(&compressed),
            compressed_size: compressed.len() as u64,
        };
        self.chunks.insert(chunk.compressed_md5.clone(), compressed);
        chunk
    }

    fn add_chunks(&mut self, bytes: &[u8]) -> Vec<Value> {
        bytes
            .chunks(self.chunk_size)
            .map(|piece| {
                let chunk = self.add_chunk(piece);
                value!({
                    "compressedMd5": chunk.compressed_md5,
                    "compressedSize": chunk.compressed_size,
                    "md5": chunk.md5,
                    "size": chunk.size,
                })
            })
            .collect()
    }

    /// Serves a depot's manifest and chunks, and returns the manifest's
    /// hash.
    fn add_depot(&mut self, depot: &FakeDepot, dependency: bool) -> String {
        let mut container = Vec::new();
        let mut items = Vec::new();
        for item in &depot.items {
            items.push(match item {
                FakeItem::File(file) => {
                    let mut entry = value!({
                        "chunks": self.add_chunks(&file.bytes),
                        "path": file.path,
                        "type": "DepotFile",
                    });
                    if !file.flags.is_empty() {
                        entry["flags"] = value!(file.flags);
                    }
                    if file.md5 {
                        entry["md5"] = value!(md5_hex(&file.bytes));
                    }
                    if file.sha256 {
                        entry["sha256"] = value!(hex(&Sha256::digest(&file.bytes)));
                    }
                    if file.in_container {
                        entry["sfcRef"] =
                            value!({ "offset": container.len(), "size": file.bytes.len() });
                        container.extend_from_slice(&file.bytes);
                    }
                    entry
                }
                FakeItem::Directory(path) => value!({ "path": path, "type": "DepotDirectory" }),
                FakeItem::Link { path, target } => {
                    value!({ "path": path, "target": target, "type": "DepotLink" })
                }
            });
        }
        let mut manifest = value!({ "depot": { "items": items }, "version": 2 });
        if !container.is_empty() {
            manifest["depot"]["smallFilesContainer"] =
                value!({ "chunks": self.add_chunks(&container) });
        }
        self.add_meta(&manifest, dependency)
    }

    fn add_meta(&mut self, manifest: &Value, dependency: bool) -> String {
        let text = manifest.to_string();
        let hash = md5_hex(text.as_bytes());
        let body = if self.plain_json {
            text.into_bytes()
        } else {
            zlib(text.as_bytes())
        };
        let meta = if dependency {
            &mut self.dependency_meta
        } else {
            &mut self.meta
        };
        meta.insert(hash.clone(), body);
        hash
    }

    /// A new token for both CDNs, as a secure link's `urls`.
    fn issue_link(&mut self, base: &str, path: &str) -> Value {
        self.issued += 1;
        let token = format!("FAKETOKEN{}", self.issued);
        let expires_at = now() + self.link_lifetime;
        self.tokens.insert(token.clone(), expires_at);
        value!([
            {
                "endpoint_name": "fallback",
                "url_format": "{base_url}{path}?wsSecret={token}&wsTime={time}",
                "parameters": {
                    "base_url": format!("{base}/fallback"),
                    "path": path,
                    "token": token,
                    "time": expires_at,
                },
                "priority": 20,
                "max_fails": 100,
                "supports_generation": [2],
                "fallback_only": true,
            },
            {
                "endpoint_name": "primary",
                "url_format": "{base_url}/token=nva={expires_at}~dirs={dirs}~token={token}{path}",
                "parameters": {
                    "base_url": format!("{base}/primary"),
                    "path": format!("{path}/"),
                    "token": token,
                    "dirs": 4,
                    "expires_at": expires_at,
                },
                "priority": 10,
                "max_fails": 100,
                "supports_generation": [1, 2],
                "fallback_only": false,
            },
        ])
    }

    fn serve_chunk(&self, request: &Request) -> Reply {
        let primary = ContentSystem::cdn_of(request) == Some(Cdn::Primary);
        if !request.path().contains(DEPENDENCY_STORE) && !self.token_is_live(request, primary) {
            return json(self.refusal, "{}");
        }
        self.chunk_reply(request, primary)
    }

    /// Whether the request carries a token that was handed out and hasn't
    /// expired.
    fn token_is_live(&self, request: &Request, primary: bool) -> bool {
        let token = if primary {
            request
                .path()
                .split_once("~token=")
                .and_then(|(_, rest)| rest.split('/').next())
                .map(str::to_owned)
        } else {
            request.query("wsSecret")
        };
        token
            .and_then(|token| self.tokens.get(&token).copied())
            .is_some_and(|expires_at| now() < expires_at)
    }

    fn chunk_reply(&self, request: &Request, primary: bool) -> Reply {
        let hash = request.path().rsplit('/').next().unwrap_or_default();
        if primary && self.missing_on_primary.contains(hash) {
            return json(404, "{}");
        }
        let Some(bytes) = self.chunks.get(hash) else {
            return json(404, "{}");
        };
        let mut bytes = bytes.clone();
        if self.corrupt.contains(hash) {
            let last = bytes.len() - 1;
            bytes[last] ^= 0xff;
        }
        Reply::Send(response(200, &[], &bytes))
    }
}

/// Where the dependency store lives on both CDNs. It's public, as on real
/// GOG: its links carry no token and its chunks are served without one.
const DEPENDENCY_STORE: &str = "/content-system/v2/dependencies/store";

/// The dependency store's `open_link` endpoints, shaped like GOG's: a plain
/// address on each CDN and no parameters.
fn open_link(base: &str) -> Value {
    value!([
        {
            "endpoint_name": "fallback",
            "url_format": format!("{base}/fallback{DEPENDENCY_STORE}"),
            "parameters": {},
            "priority": 20,
            "max_fails": 100,
            "supports_generation": [2],
            "fallback_only": true,
        },
        {
            "endpoint_name": "primary",
            "url_format": format!("{base}/primary{DEPENDENCY_STORE}"),
            "parameters": {},
            "priority": 10,
            "max_fails": 100,
            "supports_generation": [1, 2],
            "fallback_only": false,
        },
    ])
}

fn files(depot: &FakeDepot) -> impl Iterator<Item = &FakeFile> {
    depot.items.iter().filter_map(|item| match item {
        FakeItem::File(file) => Some(file),
        _ => None,
    })
}

fn meta_url(base: &str, kind: &str, hash: &str) -> String {
    format!(
        "{base}/content-system/v2/{kind}/{}/{}/{hash}",
        &hash[0..2],
        &hash[2..4]
    )
}

pub fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

pub fn md5_hex(bytes: &[u8]) -> String {
    hex(&Md5::digest(bytes))
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
