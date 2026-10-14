//! Galaxy's chunks: the signed links they're fetched with, and fetching and
//! checking them.
//!
//! A secure link is per product, or for the dependency store, and lists a
//! few CDNs, each a URL template with a token that lasts about a day. The
//! templates and their parameters are secrets while they last, so no
//! `Debug` output shows them and no error is built from them.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::ZlibDecoder;
use md5::{Digest, Md5};
use reqwest::Url;
use serde_json::Value;
use sha2::Sha256;

use crate::client::{self, Client};
use crate::error::{Endpoint, Error, ErrorKind};
use crate::json;
use crate::manifest::{Chunk, DepotFile};

/// A link is fetched again this long before it expires, so one doesn't run
/// out partway through a chunk.
const EXPIRY_MARGIN: u64 = 60;

/// Whose chunks a secure link reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkScope {
    /// A base game or DLC.
    Product(u64),
    /// The redistributables.
    Dependencies,
}

/// One CDN in a secure link.
#[derive(Clone)]
struct LinkEndpoint {
    name: String,
    url_format: String,
    parameters: BTreeMap<String, String>,
    priority: i64,
    max_fails: Option<u64>,
    fallback_only: bool,
}

impl fmt::Debug for LinkEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LinkEndpoint")
            .field("name", &self.name)
            .field("url_format", &"<redacted>")
            .field("parameters", &"<redacted>")
            .field("priority", &self.priority)
            .field("max_fails", &self.max_fails)
            .field("fallback_only", &self.fallback_only)
            .finish()
    }
}

impl LinkEndpoint {
    /// The chunk's address on this CDN: `/ab/cd/<hash>` appended to the
    /// `path` parameter, then every `{key}` in the template filled in.
    fn chunk_url(&self, compressed_md5: &str) -> Option<Url> {
        let hash = compressed_md5;
        if hash.len() < 4 || !hash.is_ascii() {
            return None;
        }
        let mut parameters = self.parameters.clone();
        let path = parameters.get_mut("path")?;
        let joined = format!(
            "{}/{}/{}/{hash}",
            path.trim_end_matches('/'),
            &hash[0..2],
            &hash[2..4]
        );
        *path = joined;
        Url::parse(&fill(&self.url_format, &parameters)).ok()
    }

    fn expires_at(&self) -> Option<u64> {
        self.parameters.get("expires_at")?.parse().ok()
    }
}

/// Fills each `{key}` in one pass, so a value that looks like a key is
/// left alone. Unknown keys stay as written.
fn fill(format: &str, parameters: &BTreeMap<String, String>) -> String {
    let mut filled = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(start) = rest.find('{') {
        filled.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after
            .find('}')
            .and_then(|end| Some((parameters.get(&after[..end])?, end)))
        {
            Some((value, end)) => {
                filled.push_str(value);
                rest = &after[end + 1..];
            }
            None => {
                filled.push('{');
                rest = after;
            }
        }
    }
    filled.push_str(rest);
    filled
}

/// A signed link to chunks, with its CDNs in the order they're tried.
#[derive(Clone)]
pub struct SecureLink {
    endpoints: Vec<LinkEndpoint>,
}

impl fmt::Debug for SecureLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecureLink")
            .field("endpoints", &self.endpoints)
            .field("expires_at", &self.expires_at())
            .finish()
    }
}

impl SecureLink {
    /// The earliest expiry any CDN gives, as Unix seconds. Not every CDN
    /// gives one.
    pub fn expires_at(&self) -> Option<u64> {
        self.endpoints
            .iter()
            .filter_map(LinkEndpoint::expires_at)
            .min()
    }

    /// The CDNs' names, in the order they're tried.
    pub fn endpoint_names(&self) -> Vec<&str> {
        self.endpoints
            .iter()
            .map(|endpoint| endpoint.name.as_str())
            .collect()
    }

    /// Whether the link is still good at `now`, with a margin.
    fn fresh(&self, now: u64) -> bool {
        self.expires_at()
            .is_none_or(|expires| now.saturating_add(EXPIRY_MARGIN) < expires)
    }
}

pub(crate) fn parse_secure_link(json: &Value) -> Option<SecureLink> {
    let mut endpoints = json
        .get("urls")?
        .as_array()?
        .iter()
        .map(|endpoint| {
            let parameters = endpoint
                .get("parameters")?
                .as_object()?
                .iter()
                .map(|(key, value)| {
                    let value = match value {
                        Value::String(text) => text.clone(),
                        Value::Number(number) => number.to_string(),
                        _ => return None,
                    };
                    Some((key.clone(), value))
                })
                .collect::<Option<_>>()?;
            Some(LinkEndpoint {
                name: json::text(endpoint, "endpoint_name")?,
                url_format: json::text(endpoint, "url_format")?,
                parameters,
                priority: endpoint
                    .get("priority")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                max_fails: endpoint.get("max_fails").and_then(Value::as_u64),
                fallback_only: json::flag(endpoint, "fallback_only"),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    if endpoints.is_empty() {
        return None;
    }
    // Stable, so equal priorities keep GOG's order.
    endpoints
        .sort_by_key(|endpoint| (endpoint.fallback_only, std::cmp::Reverse(endpoint.priority)));
    Some(SecureLink { endpoints })
}

/// Keeps one scope's secure link while it lasts, and fetches a new one
/// when it's about to expire or a CDN refuses it. Shared between threads
/// downloading the same scope's chunks.
pub struct ChunkLinks {
    scope: LinkScope,
    /// Each new link gets the next generation, so a refusal of an old link
    /// doesn't throw away one another thread just fetched.
    current: Mutex<(u64, Option<Arc<SecureLink>>)>,
}

impl fmt::Debug for ChunkLinks {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let held = self
            .current
            .lock()
            .map(|current| current.1.is_some())
            .unwrap_or(false);
        formatter
            .debug_struct("ChunkLinks")
            .field("scope", &self.scope)
            .field("held", &held)
            .finish()
    }
}

impl ChunkLinks {
    pub fn new(scope: LinkScope) -> Self {
        Self {
            scope,
            current: Mutex::new((0, None)),
        }
    }

    pub fn scope(&self) -> LinkScope {
        self.scope
    }

    /// The link to use now, fetched if there's none or it's about to
    /// expire. The lock is held while fetching, so threads wanting a new
    /// link fetch it once between them.
    fn get(&self, client: &Client, access_token: &str) -> Result<(u64, Arc<SecureLink>), Error> {
        let mut current = self
            .current
            .lock()
            .map_err(|_| Error::new(Endpoint::SecureLink, ErrorKind::Request))?;
        if let Some(link) = &current.1
            && link.fresh(now())
        {
            return Ok((current.0, Arc::clone(link)));
        }
        let link = Arc::new(client.secure_link(access_token, self.scope)?);
        current.0 += 1;
        current.1 = Some(Arc::clone(&link));
        Ok((current.0, link))
    }

    /// Drops the link of `generation`, unless a newer one replaced it.
    fn invalidate(&self, generation: u64) {
        if let Ok(mut current) = self.current.lock()
            && current.0 == generation
        {
            current.1 = None;
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// How fetching a chunk from one CDN went wrong.
enum Attempt {
    /// A 401 or 403, which most likely means the link expired.
    Refused(u16),
    Failed(Error),
}

impl Client {
    /// A new secure link for a product's chunks or the dependency store's.
    pub fn secure_link(&self, access_token: &str, scope: LinkScope) -> Result<SecureLink, Error> {
        let fail = |kind| Error::new(Endpoint::SecureLink, kind);
        let base = &self.endpoints().content_system;
        // Set verbatim, since GOG's clients send `path` unencoded.
        let (mut url, query) = match scope {
            LinkScope::Product(id) => (
                client::join(
                    base,
                    &["products", &id.to_string(), "secure_link"],
                    Endpoint::SecureLink,
                )?,
                "_version=2&generation=2&path=/",
            ),
            LinkScope::Dependencies => (
                client::join(base, &["open_link"], Endpoint::SecureLink)?,
                "generation=2&_version=2&path=/dependencies/store/",
            ),
        };
        url.set_query(Some(query));
        let json = self.get_json(Endpoint::SecureLink, url, Some(access_token))?;
        if let (LinkScope::Product(id), Some(answered)) = (scope, json.get("product_id"))
            && json::id(answered) != Some(id)
        {
            return Err(fail(ErrorKind::Malformed));
        }
        parse_secure_link(&json).ok_or_else(|| fail(ErrorKind::Malformed))
    }

    /// Fetches a chunk and hands back its unpacked bytes, after checking
    /// the bytes as downloaded against `compressed_md5` and the unpacked
    /// ones against `md5` and `size`.
    ///
    /// CDNs are tried in the link's order. A CDN refusing the link gets the
    /// link fetched again once, and the CDNs tried again from the first; any
    /// other failure, a wrong checksum included, moves on to the next CDN.
    /// When every CDN fails, the last failure is returned. A refusal after
    /// the link was fetched again is `ErrorKind::Status`, never
    /// `Unauthorized`, which is kept for the secure link request itself.
    pub fn chunk(
        &self,
        links: &ChunkLinks,
        access_token: &str,
        chunk: &Chunk,
    ) -> Result<Vec<u8>, Error> {
        let fail = |kind| Error::new(Endpoint::Chunk, kind);
        let mut refetched = false;
        'link: loop {
            let (generation, link) = links.get(self, access_token)?;
            let mut last = fail(ErrorKind::Malformed);
            for endpoint in &link.endpoints {
                let Some(url) = endpoint.chunk_url(&chunk.compressed_md5) else {
                    last = fail(ErrorKind::Malformed);
                    continue;
                };
                match self.fetch_chunk(url, chunk) {
                    Ok(bytes) => return Ok(bytes),
                    Err(Attempt::Refused(_)) if !refetched => {
                        refetched = true;
                        links.invalidate(generation);
                        continue 'link;
                    }
                    Err(Attempt::Refused(status)) => last = fail(ErrorKind::Status(status)),
                    Err(Attempt::Failed(error)) => last = error,
                }
            }
            return Err(last);
        }
    }

    fn fetch_chunk(&self, url: Url, chunk: &Chunk) -> Result<Vec<u8>, Attempt> {
        let fail = |kind| Attempt::Failed(Error::new(Endpoint::Chunk, kind));
        let response = self
            .cdn()
            .map_err(Attempt::Failed)?
            .get(url)
            .send()
            .map_err(|error| fail(client::send_error(&error)))?;
        let status = response.status().as_u16();
        if matches!(status, 401 | 403) {
            return Err(Attempt::Refused(status));
        }
        if status != 200 {
            return Err(fail(client::status_kind(Endpoint::Chunk, status, None)));
        }
        let compressed = client::read_limited(response, chunk.compressed_size).map_err(|kind| {
            fail(match kind {
                ErrorKind::TooLarge => ErrorKind::WrongSize,
                other => other,
            })
        })?;
        if compressed.len() as u64 != chunk.compressed_size {
            return Err(fail(ErrorKind::WrongSize));
        }
        if hex_md5(&compressed) != chunk.compressed_md5 {
            return Err(fail(ErrorKind::WrongChecksum));
        }
        let mut unpacked = Vec::new();
        ZlibDecoder::new(compressed.as_slice())
            .take(chunk.size.saturating_add(1))
            .read_to_end(&mut unpacked)
            .map_err(|_| fail(ErrorKind::Malformed))?;
        if unpacked.len() as u64 != chunk.size {
            return Err(fail(ErrorKind::WrongSize));
        }
        if hex_md5(&unpacked) != chunk.md5 {
            return Err(fail(ErrorKind::WrongChecksum));
        }
        Ok(unpacked)
    }

    /// Writes a whole file's chunks to `out` in order, then checks the
    /// file's MD5 and SHA-256 where the manifest gives them. Each chunk is
    /// checked before it's written, but a wrong whole-file checksum is only
    /// known at the end, so on any error the caller throws `out` away.
    /// `cancel` is checked before each chunk.
    pub fn write_file(
        &self,
        links: &ChunkLinks,
        access_token: &str,
        file: &DepotFile,
        out: &mut dyn Write,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        let fail = |kind| Error::new(Endpoint::Chunk, kind);
        let (mut md5, mut sha256) = (Md5::new(), Sha256::new());
        for chunk in &file.chunks {
            if cancel() {
                return Err(fail(ErrorKind::Canceled));
            }
            let bytes = self.chunk(links, access_token, chunk)?;
            md5.update(&bytes);
            sha256.update(&bytes);
            out.write_all(&bytes)
                .map_err(|error| fail(ErrorKind::Disk(error.kind())))?;
        }
        if file
            .md5
            .as_ref()
            .is_some_and(|expected| *expected != hex(&md5.finalize()))
            || file
                .sha256
                .as_ref()
                .is_some_and(|expected| *expected != hex(&sha256.finalize()))
        {
            return Err(fail(ErrorKind::WrongChecksum));
        }
        Ok(())
    }
}

fn hex_md5(bytes: &[u8]) -> String {
    hex(&Md5::digest(bytes))
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
