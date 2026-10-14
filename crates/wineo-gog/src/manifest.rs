//! Galaxy's generation 2 manifests: a build's repository manifest, each
//! depot's file list, and the redistributables GOG offers.
//!
//! Manifests come from GOG's CDN as zlib-compressed JSON, or sometimes plain
//! JSON, and need no token. Unlike product details, parsing is strict: an
//! item missing what makes it usable fails the whole manifest, since
//! skipping it would quietly install an incomplete game.

use std::fmt;
use std::io::Read;

use flate2::read::ZlibDecoder;
use reqwest::Url;
use serde_json::Value;

use crate::client::{self, Client};
use crate::error::{Endpoint, Error, ErrorKind};
use crate::json;
use crate::product::id_text;

/// Well above the largest depot manifest seen, 1.4 MB unpacked.
pub(crate) const MAX_MANIFEST: u64 = 64 * 1024 * 1024;
/// Far above any real depot (the largest seen is 14 GB), and low enough that
/// adding up every size in a manifest can't overflow.
pub(crate) const MAX_SIZE: u64 = 1 << 50;
/// Far above the largest chunk seen, 20 MB.
pub(crate) const MAX_CHUNK: u64 = 1 << 30;

/// What a build installs, from the build list's `link`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub build_id: String,
    pub base_product_id: u64,
    /// The folder name GOG suggests for the game.
    pub install_directory: Option<String>,
    /// Whether setup runs GOG's script interpreter on the game's
    /// `goggame-<id>.script`.
    pub script_interpreter: bool,
    pub depots: Vec<Depot>,
    /// Redistributable ids, like `MSVC2017`.
    pub dependencies: Vec<String>,
    pub products: Vec<RepositoryProduct>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Depot {
    pub product_id: u64,
    /// Language codes like `en-US`, or `*` for every language.
    pub languages: Vec<String>,
    /// `32` or `64` when the depot is only for one architecture. Empty
    /// for any.
    pub os_bitness: Vec<String>,
    /// The depot manifest's hash, for `Client::depot_manifest`.
    pub manifest: String,
    pub size: u64,
    pub compressed_size: u64,
    /// GOG's own files, like `goggame-<id>.info`, rather than the game's.
    pub is_gog_depot: bool,
}

impl Depot {
    /// Whether the depot is for `code`, by itself or by being for every
    /// language.
    pub fn has_language(&self, code: &str) -> bool {
        self.languages
            .iter()
            .any(|language| language == "*" || language.eq_ignore_ascii_case(code))
    }

    /// Whether the depot is for a `32` or `64` bit system.
    pub fn fits_bitness(&self, bits: &str) -> bool {
        self.os_bitness.is_empty() || self.os_bitness.iter().any(|value| value == bits)
    }
}

/// A product in a build: the base game or one of its DLC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryProduct {
    pub product_id: u64,
    pub name: Option<String>,
    /// A setup program among the product's support files, for games without
    /// the script interpreter.
    pub temp_executable: Option<String>,
    pub temp_arguments: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepotManifest {
    pub items: Vec<DepotItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepotItem {
    File(DepotFile),
    Directory(DepotDirectory),
    Link(DepotLink),
}

/// A file, built from its own chunks in order. A depot's small-file
/// container is ignored, since every file in it lists its chunks too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepotFile {
    /// As GOG wrote it, with `\` or `/` between parts. `parts` splits it.
    pub path: String,
    /// Empty for an empty file.
    pub chunks: Vec<Chunk>,
    /// The whole file's MD5, lowercase hex, when GOG gives one.
    pub md5: Option<String>,
    /// The whole file's SHA-256, lowercase hex, when GOG gives one.
    pub sha256: Option<String>,
    /// Like `executable` and `support`.
    pub flags: Vec<String>,
}

impl DepotFile {
    pub fn size(&self) -> u64 {
        self.chunks
            .iter()
            .fold(0, |total, chunk| total.saturating_add(chunk.size))
    }

    pub fn parts(&self) -> Vec<&str> {
        parts(&self.path)
    }

    /// Whether the file goes to the product's support folder rather than
    /// the game folder.
    pub fn is_support(&self) -> bool {
        self.flags.iter().any(|flag| flag == "support")
    }

    pub fn is_executable(&self) -> bool {
        self.flags.iter().any(|flag| flag == "executable")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepotDirectory {
    pub path: String,
}

impl DepotDirectory {
    pub fn parts(&self) -> Vec<&str> {
        parts(&self.path)
    }
}

/// A symbolic link. The target is kept as GOG wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepotLink {
    pub path: String,
    pub target: String,
}

impl DepotLink {
    pub fn parts(&self) -> Vec<&str> {
        parts(&self.path)
    }
}

/// One zlib-compressed piece of a file, fetched by `compressed_md5`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Of the unpacked bytes, lowercase hex.
    pub md5: String,
    pub size: u64,
    /// Of the bytes as downloaded, lowercase hex.
    pub compressed_md5: String,
    pub compressed_size: u64,
}

/// Where the current redistributables manifest is. `Debug` hides the URL,
/// like every other link the client is handed.
#[derive(Clone, PartialEq, Eq)]
pub struct DependencyRepository {
    pub build_id: String,
    pub manifest_url: String,
}

impl fmt::Debug for DependencyRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DependencyRepository")
            .field("build_id", &self.build_id)
            .field("manifest_url", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyManifest {
    pub dependencies: Vec<Dependency>,
}

/// A redistributable, like a Visual C++ runtime or the script interpreter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// Like `MSVC2017` or `ISI`.
    pub id: String,
    pub readable_name: Option<String>,
    /// What runs to install it. None for ones that are only files, like
    /// DOSBox.
    pub executable: Option<DependencyExecutable>,
    /// Used by GOG's own tools rather than listed by games.
    pub internal: bool,
    pub languages: Vec<String>,
    /// The depot manifest's hash, for `Client::dependency_depot_manifest`.
    pub manifest: String,
    pub size: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyExecutable {
    /// Relative to the dependency store, like
    /// `__redist/MSVC2017/VC_redist.x86.exe`.
    pub path: String,
    /// Passed as GOG wrote them, nested quotes and all.
    pub arguments: String,
}

impl Client {
    /// A build's repository manifest, from the `link` in its build list.
    pub fn repository(&self, link: &str) -> Result<Repository, Error> {
        let url =
            Url::parse(link).map_err(|_| Error::new(Endpoint::Repository, ErrorKind::Malformed))?;
        let json = self.fetch_manifest(Endpoint::Repository, url)?;
        parse_repository(&json)
            .ok_or_else(|| Error::new(Endpoint::Repository, ErrorKind::Malformed))
    }

    /// A depot's file list, by the hash a repository manifest gives.
    pub fn depot_manifest(&self, hash: &str) -> Result<DepotManifest, Error> {
        self.depot_manifest_at(&["content-system", "v2", "meta"], hash)
    }

    /// Where the current redistributables manifest is. The token is sent
    /// when given, as GOG's own clients do.
    pub fn dependency_repository(
        &self,
        access_token: Option<&str>,
    ) -> Result<DependencyRepository, Error> {
        let mut url = client::join(
            &self.endpoints().content_system,
            &["dependencies", "repository"],
            Endpoint::Dependencies,
        )?;
        url.query_pairs_mut().append_pair("generation", "2");
        let json = self.get_json(Endpoint::Dependencies, url, access_token)?;
        parse_dependency_repository(&json)
            .ok_or_else(|| Error::new(Endpoint::Dependencies, ErrorKind::Malformed))
    }

    /// Every redistributable GOG offers.
    pub fn dependency_manifest(
        &self,
        repository: &DependencyRepository,
    ) -> Result<DependencyManifest, Error> {
        let url = Url::parse(&repository.manifest_url)
            .map_err(|_| Error::new(Endpoint::Dependencies, ErrorKind::Malformed))?;
        let json = self.fetch_manifest(Endpoint::Dependencies, url)?;
        parse_dependency_manifest(&json)
            .ok_or_else(|| Error::new(Endpoint::Dependencies, ErrorKind::Malformed))
    }

    /// A redistributable's file list, by the hash its manifest gives.
    pub fn dependency_depot_manifest(&self, hash: &str) -> Result<DepotManifest, Error> {
        self.depot_manifest_at(&["content-system", "v2", "dependencies", "meta"], hash)
    }

    fn depot_manifest_at(&self, prefix: &[&str], hash: &str) -> Result<DepotManifest, Error> {
        let fail = |kind| Error::new(Endpoint::DepotManifest, kind);
        let hash = hex(hash, 32).ok_or_else(|| fail(ErrorKind::Malformed))?;
        let mut segments = prefix.to_vec();
        segments.extend([&hash[0..2], &hash[2..4], hash.as_str()]);
        let url = client::join(&self.endpoints().cdn, &segments, Endpoint::DepotManifest)?;
        let json = self.fetch_manifest(Endpoint::DepotManifest, url)?;
        parse_depot_manifest(&json).ok_or_else(|| fail(ErrorKind::Malformed))
    }

    /// A manifest from the CDN. No token goes there.
    fn fetch_manifest(&self, endpoint: Endpoint, url: Url) -> Result<Value, Error> {
        let fail = |kind| Error::new(endpoint, kind);
        let response = self
            .cdn()?
            .get(url)
            .send()
            .map_err(|error| fail(client::send_error(&error)))?;
        let status = response.status().as_u16();
        // No login goes to the CDN, so its refusal must not read as one and
        // get the login renewed.
        if matches!(status, 401 | 403) {
            return Err(fail(ErrorKind::Status(status)));
        }
        if status != 200 {
            return Err(fail(client::status_kind(endpoint, status, None)));
        }
        let body = client::read_limited(response, self.max_manifest).map_err(fail)?;
        decode_manifest(&body, self.max_manifest).map_err(fail)
    }
}

/// zlib-compressed JSON, or plain JSON, of at most `limit` bytes unpacked.
pub(crate) fn decode_manifest(body: &[u8], limit: u64) -> Result<Value, ErrorKind> {
    if !is_zlib(body) {
        if body.len() as u64 > limit {
            return Err(ErrorKind::TooLarge);
        }
        return serde_json::from_slice(body).map_err(|_| ErrorKind::Json);
    }
    let mut unpacked = Vec::new();
    ZlibDecoder::new(body)
        .take(limit + 1)
        .read_to_end(&mut unpacked)
        .map_err(|_| ErrorKind::Malformed)?;
    if unpacked.len() as u64 > limit {
        return Err(ErrorKind::TooLarge);
    }
    serde_json::from_slice(&unpacked).map_err(|_| ErrorKind::Json)
}

/// A zlib header: deflate with a window that fits, and a check value. JSON
/// can't start with 0x78 (`x`).
fn is_zlib(body: &[u8]) -> bool {
    match body {
        [first, second, ..] => {
            first & 0x0f == 8
                && first >> 4 <= 7
                && (u16::from(*first) << 8 | u16::from(*second)) % 31 == 0
        }
        _ => false,
    }
}

pub(crate) fn parse_repository(json: &Value) -> Option<Repository> {
    if json.get("version").and_then(json::id) != Some(2) {
        return None;
    }
    let depots = json
        .get("depots")?
        .as_array()?
        .iter()
        .map(parse_depot)
        .collect::<Option<_>>()?;
    let dependencies = match json.get("dependencies") {
        None | Some(Value::Null) => Vec::new(),
        Some(list) => list
            .as_array()?
            .iter()
            .map(|id| id.as_str().filter(|id| !id.is_empty()).map(str::to_owned))
            .collect::<Option<_>>()?,
    };
    let products = match json.get("products") {
        None | Some(Value::Null) => Vec::new(),
        Some(list) => list
            .as_array()?
            .iter()
            .map(|product| {
                Some(RepositoryProduct {
                    product_id: json::id(product.get("productId")?)?,
                    name: json::text(product, "name"),
                    temp_executable: match optional_text(product, "temp_executable")? {
                        Some(path) if !safe_path(&path) => return None,
                        path => path,
                    },
                    temp_arguments: optional_text(product, "temp_arguments")?,
                })
            })
            .collect::<Option<_>>()?,
    };
    Some(Repository {
        build_id: json.get("buildId").and_then(id_text)?,
        base_product_id: json::id(json.get("baseProductId")?)?,
        install_directory: json::text(json, "installDirectory"),
        script_interpreter: json::flag(json, "scriptInterpreter"),
        depots,
        dependencies,
        products,
    })
}

fn parse_depot(json: &Value) -> Option<Depot> {
    Some(Depot {
        product_id: json::id(json.get("productId")?)?,
        languages: strings(json.get("languages")?)?,
        os_bitness: match json.get("osBitness") {
            None | Some(Value::Null) => Vec::new(),
            Some(list) => strings(list)?,
        },
        manifest: hex(json.get("manifest")?.as_str()?, 32)?,
        size: size(json, "size")?,
        compressed_size: size(json, "compressedSize")?,
        is_gog_depot: json::flag(json, "isGogDepot"),
    })
}

pub(crate) fn parse_depot_manifest(json: &Value) -> Option<DepotManifest> {
    let items = json
        .get("depot")?
        .get("items")?
        .as_array()?
        .iter()
        .map(parse_item)
        .collect::<Option<_>>()?;
    Some(DepotManifest { items })
}

fn parse_item(json: &Value) -> Option<DepotItem> {
    let path = json.get("path")?.as_str().filter(|path| safe_path(path))?;
    match json.get("type")?.as_str()? {
        "DepotFile" => {
            let chunks: Vec<Chunk> = match json.get("chunks") {
                None | Some(Value::Null) => Vec::new(),
                Some(list) => list
                    .as_array()?
                    .iter()
                    .map(parse_chunk)
                    .collect::<Option<_>>()?,
            };
            // A file that only lives in the small-file container would be
            // written empty, so it's refused instead.
            let listed_size = |value: Option<&Value>| value.and_then(json::id).unwrap_or(0);
            if chunks.is_empty()
                && (listed_size(json.get("size")) > 0
                    || listed_size(json.get("sfcRef").and_then(|sfc| sfc.get("size"))) > 0)
            {
                return None;
            }
            let hash = |name: &str, length| match json.get(name) {
                None | Some(Value::Null) => Some(None),
                Some(value) => hex(value.as_str()?, length).map(Some),
            };
            let flags = match json.get("flags") {
                None | Some(Value::Null) => Vec::new(),
                Some(list) => strings(list)?,
            };
            Some(DepotItem::File(DepotFile {
                path: path.to_owned(),
                chunks,
                md5: hash("md5", 32)?,
                sha256: hash("sha256", 64)?,
                flags,
            }))
        }
        "DepotDirectory" => Some(DepotItem::Directory(DepotDirectory {
            path: path.to_owned(),
        })),
        "DepotLink" => Some(DepotItem::Link(DepotLink {
            path: path.to_owned(),
            target: json::text(json, "target")?,
        })),
        _ => None,
    }
}

fn parse_chunk(json: &Value) -> Option<Chunk> {
    Some(Chunk {
        md5: hex(json.get("md5")?.as_str()?, 32)?,
        size: chunk_size(json, "size")?,
        compressed_md5: hex(json.get("compressedMd5")?.as_str()?, 32)?,
        compressed_size: chunk_size(json, "compressedSize")?,
    })
}

pub(crate) fn parse_dependency_repository(json: &Value) -> Option<DependencyRepository> {
    Some(DependencyRepository {
        build_id: json.get("build_id").and_then(id_text)?,
        manifest_url: json::text(json, "repository_manifest")?,
    })
}

pub(crate) fn parse_dependency_manifest(json: &Value) -> Option<DependencyManifest> {
    let dependencies = json
        .get("depots")?
        .as_array()?
        .iter()
        .map(parse_dependency)
        .collect::<Option<_>>()?;
    Some(DependencyManifest { dependencies })
}

fn parse_dependency(json: &Value) -> Option<Dependency> {
    let executable = match json.get("executable") {
        None | Some(Value::Null) => None,
        Some(executable) => {
            let arguments = optional_text(executable, "arguments")?;
            match optional_text(executable, "path")? {
                None => None,
                Some(path) if safe_path(&path) => Some(DependencyExecutable {
                    path,
                    arguments: arguments.unwrap_or_default(),
                }),
                Some(_) => return None,
            }
        }
    };
    Some(Dependency {
        id: json::text(json, "dependencyId")?,
        readable_name: json::text(json, "readableName"),
        executable,
        internal: json::flag(json, "internal"),
        languages: match json.get("languages") {
            None | Some(Value::Null) => Vec::new(),
            Some(list) => strings(list)?,
        },
        manifest: hex(json.get("manifest")?.as_str()?, 32)?,
        size: size(json, "size")?,
        compressed_size: size(json, "compressedSize")?,
    })
}

/// A chunk is held in memory whole, and GOG cuts files into pieces of
/// 10 MiB or so, with whole small files up to a few tens of MB.
fn chunk_size(json: &Value, name: &str) -> Option<u64> {
    size(json, name).filter(|&size| size <= MAX_CHUNK)
}

fn strings(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

fn size(json: &Value, name: &str) -> Option<u64> {
    json::id(json.get(name)?).filter(|&size| size <= MAX_SIZE)
}

/// A string field that may be missing, null, or empty, which are all None.
/// Anything else that isn't a string makes the outer Option None, so a
/// malformed field fails the manifest.
fn optional_text(json: &Value, name: &str) -> Option<Option<String>> {
    match json.get(name) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some((!text.is_empty()).then(|| text.clone())),
        Some(_) => None,
    }
}

/// Lowercase hex of exactly `length` digits.
pub(crate) fn hex(text: &str, length: usize) -> Option<String> {
    (text.len() == length && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}

fn parts(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).collect()
}

/// A relative path that stays inside the folder it's joined to: no empty,
/// `.`, or `..` parts, no root or drive, and nothing Windows can't name.
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && parts(path).iter().all(|part| {
            !part.is_empty() && *part != "." && *part != ".." && !part.contains(['\0', ':'])
        })
}

#[cfg(test)]
mod tests;
