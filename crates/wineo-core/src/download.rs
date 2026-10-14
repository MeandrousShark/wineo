//! Verified upstream downloads, staged on the destination filesystem.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};

use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::error::Error;
use crate::paths::{Paths, resolve};

pub const RECEIPT: &str = ".wineo-install.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    GeProton,
    ProtonCachyos,
    Dxvk,
    Vkd3d,
}

impl Provider {
    pub const ALL: [Self; 4] = [Self::GeProton, Self::ProtonCachyos, Self::Dxvk, Self::Vkd3d];
    pub fn id(self) -> &'static str {
        match self {
            Self::GeProton => "ge-proton",
            Self::ProtonCachyos => "proton-cachyos",
            Self::Dxvk => "dxvk",
            Self::Vkd3d => "vkd3d",
        }
    }
    fn repo(self) -> &'static str {
        match self {
            Self::GeProton => "GloriousEggroll/proton-ge-custom",
            Self::ProtonCachyos => "CachyOS/proton-cachyos",
            Self::Dxvk => "doitsujin/dxvk",
            Self::Vkd3d => "HansKristian-Work/vkd3d-proton",
        }
    }
    fn variant(self) -> &'static str {
        if self == Self::ProtonCachyos {
            "slr"
        } else {
            "baseline"
        }
    }
    fn root(self, paths: &Paths, global: &GlobalConfig) -> Result<PathBuf> {
        Ok(match self {
            Self::GeProton | Self::ProtonCachyos => global.runner_root(paths)?,
            Self::Dxvk => match &global.defaults.dxvk_root {
                Some(p) => paths.config_path(p)?,
                None => paths.default_dxvk_root(),
            },
            Self::Vkd3d => match &global.defaults.vkd3d_root {
                Some(p) => paths.config_path(p)?,
                None => paths.default_vkd3d_root(),
            },
        })
    }
}
impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}
impl FromStr for Provider {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|p| p.id() == s)
            .ok_or_else(|| format!("unknown download provider `{s}`"))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamRelease {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}
#[derive(Debug, Clone)]
pub struct Release {
    pub provider: Provider,
    pub tag: String,
    pub installed_name: String,
    pub asset_name: String,
    pub size: u64,
    asset: Asset,
    checksums: Vec<Asset>,
    stable: bool,
}
impl Release {
    pub fn destination(&self, paths: &Paths, global: &GlobalConfig) -> Result<PathBuf> {
        Ok(self
            .provider
            .root(paths, global)?
            .join(&self.installed_name))
    }
    pub fn verification(&self) -> String {
        let mut methods: Vec<String> = self.checksums.iter().map(|a| a.name.clone()).collect();
        if self.asset.digest.is_some() {
            methods.push("GitHub asset SHA-256".into());
        }
        if methods.is_empty() {
            "unavailable (installation will be refused)".into()
        } else {
            methods.join(" + ")
        }
    }
}
#[derive(Debug)]
pub struct Catalog {
    pub releases: Vec<Release>,
    pub has_more: bool,
}
#[derive(Debug)]
pub enum Progress {
    Downloading { received: u64, total: u64 },
    Verifying,
    Extracting,
    Installed(PathBuf),
    AlreadyInstalled(PathBuf),
}

/// A response whose status the caller reads for itself.
pub struct Response {
    pub status: u16,
    pub body: Box<dyn Read>,
}

/// An injectable streaming transport. Production requests and redirects are HTTPS only.
pub trait Transport {
    fn get(&self, url: &str) -> Result<Box<dyn Read>>;

    /// A GET whose status the caller reads, sending `bearer` as a bearer token
    /// when given, for APIs. Transports that never make one keep this refusal.
    fn request(&self, url: &str, bearer: Option<&str>) -> Result<Response> {
        let _ = bearer;
        bail!("this transport can't make an API request to {url}")
    }

    /// Keeps header metadata available without changing the response shape.
    fn request_with_length(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> Result<(Response, Option<u64>)> {
        self.request(url, bearer).map(|response| (response, None))
    }
}
/// Clones share one connection pool and its worker thread.
#[derive(Clone)]
pub struct Https(reqwest::blocking::Client);
impl Https {
    /// An HTTPS client for API requests.
    pub fn new() -> Result<Self> {
        Ok(Self(http_client(true, Duration::from_secs(10))?))
    }
}

/// Something a batch builds once, when its first item needs it, instead of
/// once per item. A blocking HTTP client costs a thread and a TLS setup. A
/// failed build isn't kept, so each item that asks after a failure gets the
/// error it would have built for itself.
pub struct Lazy<T>(std::cell::OnceCell<T>);

impl<T> Default for Lazy<T> {
    fn default() -> Self {
        Self(std::cell::OnceCell::new())
    }
}

impl<T> Lazy<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_try_init<E>(&self, build: impl FnOnce() -> Result<T, E>) -> Result<&T, E> {
        if let Some(value) = self.0.get() {
            return Ok(value);
        }
        let value = build()?;
        Ok(self.0.get_or_init(|| value))
    }
}

impl Lazy<Https> {
    /// The batch's client, built on first use.
    pub fn https(&self) -> Result<&Https> {
        self.get_or_try_init(Https::new)
    }
}
impl Transport for Https {
    fn request(&self, url: &str, bearer: Option<&str>) -> Result<Response> {
        self.request_with_length(url, bearer)
            .map(|(response, _)| response)
    }

    fn request_with_length(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> Result<(Response, Option<u64>)> {
        ensure!(
            reqwest::Url::parse(url)?.scheme() == "https",
            "API URL must use HTTPS"
        );
        let mut request = self.0.get(url);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .map_err(|err| Error::Network(err.into()))
            .with_context(|| format!("request failed: {url}"))?;
        let content_length = response.content_length();
        Ok((
            Response {
                status: response.status().as_u16(),
                body: Box::new(response),
            },
            content_length,
        ))
    }

    fn get(&self, url: &str) -> Result<Box<dyn Read>> {
        ensure!(
            reqwest::Url::parse(url)?.scheme() == "https",
            "download URL must use HTTPS"
        );
        let response = self
            .0
            .get(url)
            .send()
            .map_err(|err| Error::Network(err.into()))
            .with_context(|| format!("request failed: {url}"))?;
        let status = response.status();
        if status.as_u16() == 403 || status.as_u16() == 429 {
            return Err(Error::Network(
                format!("GitHub denied or rate-limited the request ({status}); try again later")
                    .into(),
            )
            .into());
        }
        Ok(Box::new(
            response
                .error_for_status()
                .map_err(|err| Error::Network(err.into()))
                .with_context(|| format!("request failed: {url}"))?,
        ))
    }
}
pub struct Client<T = Https> {
    transport: T,
}
impl Client<Https> {
    pub fn new() -> Result<Self> {
        ensure!(
            cfg!(all(target_os = "linux", target_arch = "x86_64")),
            "downloads currently support Linux x86_64 hosts only"
        );
        Ok(Self {
            transport: Https(http_client(true, Duration::from_secs(5))?),
        })
    }
}
fn http_client(https_only: bool, timeout: Duration) -> Result<reqwest::blocking::Client> {
    http_client_following(https_only, timeout, reqwest::redirect::Policy::limited(10))
}

/// The same client with its own redirect policy, for requests that carry a
/// secret in the URL and must not follow it anywhere.
pub(crate) fn http_client_following(
    https_only: bool,
    timeout: Duration,
    redirects: reqwest::redirect::Policy,
) -> Result<reqwest::blocking::Client> {
    // Blocking responses apply this timeout to each Read, not the entire archive.
    Ok(reqwest::blocking::Client::builder()
        .https_only(https_only)
        .connect_timeout(timeout)
        .timeout(timeout)
        .user_agent(concat!("wineo/", env!("CARGO_PKG_VERSION")))
        .redirect(redirects)
        .build()
        .map_err(|err| Error::Network(err.into()))?)
}

impl<T: Transport> Client<T> {
    pub fn with_transport(transport: T) -> Self {
        Self { transport }
    }
    fn metadata<U: serde::de::DeserializeOwned>(&self, url: &str) -> Result<U> {
        let mut data = Vec::new();
        self.transport
            .get(url)?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut data)
            .map_err(|err| Error::Network(err.into()))?;
        ensure!(
            data.len() <= 8 * 1024 * 1024,
            "release metadata exceeds size limit"
        );
        serde_json::from_slice(&data).context("invalid GitHub release metadata")
    }
    pub fn list(&self, provider: Provider, page: u32) -> Result<Catalog> {
        ensure!(page > 0, "release page starts at 1");
        let releases: Vec<UpstreamRelease> = self.metadata(&format!(
            "https://api.github.com/repos/{}/releases?per_page=30&page={page}",
            provider.repo()
        ))?;
        let has_more = releases.len() == 30;
        let mut selected = Vec::new();
        for release in releases {
            if !release.draft
                && !release.prerelease
                && let Some(release) = select_asset(provider, release)?
            {
                selected.push(release);
            }
        }
        Ok(Catalog {
            releases: selected,
            has_more,
        })
    }
    pub fn select(&self, provider: Provider, tag: Option<&str>) -> Result<Release> {
        if let Some(tag) = tag {
            let mut url = reqwest::Url::parse(&format!(
                "https://api.github.com/repos/{}/releases/tags/",
                provider.repo()
            ))?;
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("invalid release URL"))?
                .pop_if_empty()
                .push(tag);
            let upstream: UpstreamRelease = self.metadata(url.as_str())?;
            ensure!(
                upstream.tag_name == tag,
                "upstream returned a different release tag"
            );
            ensure!(!upstream.draft, "draft releases are not supported");
            return select_asset(provider, upstream)?
                .context("release has no supported Linux x86_64 archive");
        }
        for page in 1..=1000 {
            let catalog = self.list(provider, page)?;
            if let Some(release) = catalog.releases.into_iter().next() {
                return Ok(release);
            }
            if !catalog.has_more {
                break;
            }
        }
        bail!("no supported stable {provider} release found")
    }

    pub fn select_pinned(&self, provider: Provider, installed_name: &str) -> Result<Release> {
        let tag = pinned_tag(provider, installed_name)?;
        let release = self.select(provider, Some(&tag))?;
        ensure!(
            release.installed_name == installed_name,
            "release {} installs `{}`, not requested build `{installed_name}`",
            release.tag,
            release.installed_name
        );
        Ok(release)
    }
    pub fn install(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
        release: &Release,
        cancelled: &Token,
        progress: impl FnMut(Progress),
    ) -> Result<PathBuf> {
        self.install_with_phase(paths, global, release, cancelled, progress, |_| {})
    }
    fn install_with_phase(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
        release: &Release,
        cancelled: &Token,
        mut progress: impl FnMut(Progress),
        mut phase: impl FnMut(InstallPhase),
    ) -> Result<PathBuf> {
        check_cancel(cancelled)?;
        let destination = destination_identity(&release.destination(paths, global)?)?;
        let _lock = InstallLock::acquire(&destination)?;
        let kind = match release.provider {
            Provider::GeProton | Provider::ProtonCachyos => crate::build_lock::Kind::Runner,
            Provider::Dxvk => crate::build_lock::Kind::Dxvk,
            Provider::Vkd3d => crate::build_lock::Kind::Vkd3d,
        };
        let name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid build destination"))?;
        let _build_guard = crate::build_lock::shared(paths, kind, name)?;
        remove_abandoned_stages(&destination)?;
        let expected = self.expected_digests(release)?;
        check_cancel(cancelled)?;
        let receipt_base = Receipt {
            provider: release.provider,
            version: release.tag.clone(),
            architecture: "x86_64".into(),
            variant: release.provider.variant().into(),
            stable: release.stable,
            asset: release.asset_name.clone(),
            digest: String::new(),
            verified_digests: Vec::new(),
        };
        if destination.symlink_metadata().is_ok() {
            ensure!(
                !destination.symlink_metadata()?.file_type().is_symlink(),
                "destination is a symlink: {}",
                destination.display()
            );
            let receipt: Receipt = serde_json::from_slice(
                &fs::read(destination.join(RECEIPT))
                    .context("existing destination is not a tracked download")?,
            )?;
            let mut matching = receipt.clone();
            matching.digest.clear();
            matching.verified_digests.clear();
            ensure!(
                matching == receipt_base
                    && expected
                        .iter()
                        .all(|d| receipt.verified_digests.contains(d)),
                "existing destination has different download provenance: {}",
                destination.display()
            );
            validate_layout(release.provider, &destination)?;
            progress(Progress::AlreadyInstalled(destination.clone()));
            return Ok(destination);
        }
        let parent = destination.parent().context("destination has no parent")?;
        let stage = tempfile::Builder::new()
            .prefix(&stage_prefix(&destination))
            .tempdir_in(parent)?;
        let archive_path = stage.path().join("archive");
        let mut output = File::create(&archive_path)?;
        let mut response = self.transport.get(&release.asset.browser_download_url)?;
        let mut sha256 = Sha256::new();
        let mut sha512 = Sha512::new();
        let mut received = 0u64;
        let mut buffer = [0; 128 * 1024];
        loop {
            check_cancel(cancelled)?;
            let count = response
                .read(&mut buffer)
                .map_err(|err| Error::Network(err.into()))
                .context("interrupted archive download")?;
            if count == 0 {
                break;
            }
            received += count as u64;
            ensure!(received <= release.size, "archive exceeds advertised size");
            output.write_all(&buffer[..count])?;
            sha256.update(&buffer[..count]);
            sha512.update(&buffer[..count]);
            progress(Progress::Downloading {
                received,
                total: release.size,
            });
        }
        ensure!(
            received == release.size,
            "incomplete archive: expected {} bytes, received {received}",
            release.size
        );
        output.sync_all()?;
        drop(output);
        progress(Progress::Verifying);
        let digests = [
            format!("sha256:{:x}", sha256.finalize()),
            format!("sha512:{:x}", sha512.finalize()),
        ];
        for digest in &expected {
            ensure!(
                digests.contains(digest),
                "archive checksum mismatch ({digest})"
            );
        }
        progress(Progress::Extracting);
        let unpacked = stage.path().join("unpacked");
        fs::create_dir(&unpacked)?;
        extract(&archive_path, &release.asset_name, &unpacked, cancelled)?;
        let build = locate_build(release.provider, &unpacked)?;
        ensure!(
            !build.join(RECEIPT).try_exists()?,
            "archive contains a reserved install receipt"
        );
        let receipt = Receipt {
            digest: digests[0].clone(),
            verified_digests: expected,
            ..receipt_base
        };
        fs::write(build.join(RECEIPT), serde_json::to_vec_pretty(&receipt)?)?;
        phase(InstallPhase::BeforePublication);
        check_cancel(cancelled)?;
        publish(&build, &destination)?;
        phase(InstallPhase::AfterPublication);
        progress(Progress::Installed(destination.clone()));
        Ok(destination)
    }
    fn expected_digests(&self, release: &Release) -> Result<Vec<String>> {
        let mut digests = Vec::new();
        if let Some(digest) = &release.asset.digest {
            validate_digest(digest, "sha256")?;
            digests.push(digest.to_lowercase());
        }
        for asset in &release.checksums {
            let mut data = String::new();
            self.transport
                .get(&asset.browser_download_url)?
                .take(1024 * 1024 + 1)
                .read_to_string(&mut data)
                .map_err(|err| Error::Network(err.into()))?;
            ensure!(
                data.len() <= 1024 * 1024,
                "checksum document exceeds size limit"
            );
            digests.push(parse_checksum(&data, &release.asset_name, &asset.name)?);
        }
        ensure!(
            !digests.is_empty(),
            "{} has no supported upstream checksum or GitHub SHA-256 digest; refusing unchecked installation",
            release.asset_name
        );
        Ok(digests)
    }
}

enum InstallPhase {
    BeforePublication,
    AfterPublication,
}

fn stage_prefix(destination: &Path) -> String {
    format!(
        ".wineo-stage-{:x}-",
        Sha256::digest(destination.as_os_str().as_encoded_bytes())
    )
}

fn remove_abandoned_stages(destination: &Path) -> Result<()> {
    let parent = destination.parent().context("destination has no parent")?;
    let prefix = stage_prefix(destination);
    // The destination lock excludes another install from actively using this prefix.
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) && entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub provider: Provider,
    pub version: String,
    pub architecture: String,
    pub variant: String,
    pub stable: bool,
    pub asset: String,
    pub digest: String,
    #[serde(default)]
    pub verified_digests: Vec<String>,
}

fn archive_stem(name: &str) -> Option<&str> {
    [".tar.gz", ".tar.xz", ".tar.zst"]
        .into_iter()
        .find_map(|ext| name.strip_suffix(ext))
}
fn supported(provider: Provider, name: &str, tag: &str) -> bool {
    let Some(stem) = archive_stem(name) else {
        return false;
    };
    match provider {
        Provider::GeProton => {
            stem == tag || stem == format!("{tag}-x86_64") || stem == format!("{tag}-x86-64")
        }
        Provider::Dxvk => [
            format!("dxvk-{}", tag.trim_start_matches('v')),
            format!("dxvk-{}-x86_64", tag.trim_start_matches('v')),
        ]
        .contains(&stem.to_string()),
        Provider::Vkd3d => [
            format!("vkd3d-proton-{}", tag.trim_start_matches('v')),
            format!("vkd3d-proton-{}-x86_64", tag.trim_start_matches('v')),
        ]
        .contains(&stem.to_string()),
        Provider::ProtonCachyos => {
            let stem = stem.to_ascii_lowercase();
            stem.starts_with("proton-cachyos-")
                && stem.contains("-slr-")
                && (stem.ends_with("-x86_64") || stem.ends_with("-x86-64"))
                && !["-v3", "-v4", "-avx", "aarch64", "arm64", "-native"]
                    .iter()
                    .any(|s| stem.contains(s))
        }
    }
}
/// Select by provider conventions, never by the first filename in the response.
pub fn select_asset(provider: Provider, upstream: UpstreamRelease) -> Result<Option<Release>> {
    let mut matches: Vec<_> = upstream
        .assets
        .iter()
        .filter(|a| supported(provider, &a.name, &upstream.tag_name))
        .collect();
    // GE's transition releases publish gzip and zstd for the same build, but
    // the upstream SHA512 manifest covers its conventional gzip package only.
    if provider == Provider::GeProton && matches.len() > 1 {
        let gzip: Vec<_> = matches
            .iter()
            .filter(|asset| asset.name.ends_with(".tar.gz"))
            .copied()
            .collect();
        if gzip.len() == 1
            && matches
                .iter()
                .all(|asset| archive_stem(&asset.name) == archive_stem(&gzip[0].name))
        {
            matches = gzip;
        }
    }
    ensure!(
        matches.len() <= 1,
        "ambiguous supported archives in {} release {}",
        provider,
        upstream.tag_name
    );
    let Some(asset) = matches.first() else {
        return Ok(None);
    };
    ensure!(
        crate::name::is_valid_runner(&upstream.tag_name),
        "release tag cannot be represented safely as an installed name"
    );
    let installed_name = match provider {
        Provider::GeProton => upstream.tag_name.clone(),
        Provider::ProtonCachyos => archive_stem(&asset.name).unwrap().to_owned(),
        Provider::Dxvk => format!("dxvk-{}", upstream.tag_name.trim_start_matches('v')),
        Provider::Vkd3d => format!("vkd3d-proton-{}", upstream.tag_name.trim_start_matches('v')),
    };
    ensure!(
        crate::name::is_valid_runner(&installed_name),
        "invalid installed build name"
    );
    let stem = archive_stem(&asset.name).unwrap();
    let checksums = upstream
        .assets
        .iter()
        .filter(|a| {
            let lower = a.name.to_ascii_lowercase();
            ["sha256sum", "sha512sum", "sha256", "sha512"]
                .into_iter()
                .any(|suffix| {
                    a.name == format!("{stem}.{suffix}")
                        || a.name == format!("{}.{suffix}", asset.name)
                })
                || [
                    "sha256sums",
                    "sha512sums",
                    "sha256sums.txt",
                    "sha512sums.txt",
                    "checksums.txt",
                ]
                .contains(&lower.as_str())
        })
        .cloned()
        .collect();
    Ok(Some(Release {
        provider,
        tag: upstream.tag_name,
        installed_name,
        asset_name: asset.name.clone(),
        size: asset.size,
        asset: (*asset).clone(),
        checksums,
        stable: !upstream.draft && !upstream.prerelease,
    }))
}
fn validate_digest(digest: &str, algorithm: &str) -> Result<()> {
    let (kind, hex) = digest.split_once(':').context("malformed checksum")?;
    ensure!(
        kind == algorithm
            && hex.len() == if kind == "sha256" { 64 } else { 128 }
            && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "malformed or unsupported {algorithm} checksum"
    );
    Ok(())
}
fn parse_checksum(text: &str, asset: &str, document: &str) -> Result<String> {
    let mut matches = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        if name
            .trim_start_matches('*')
            .strip_prefix("./")
            .unwrap_or(name.trim_start_matches('*'))
            != asset
        {
            continue;
        }
        ensure!(
            parts.next().is_none(),
            "malformed checksum entry for {asset}"
        );
        let algorithm = match hash.len() {
            64 => "sha256",
            128 => "sha512",
            _ => bail!("unsupported checksum in {document}"),
        };
        let digest = format!("{algorithm}:{}", hash.to_ascii_lowercase());
        validate_digest(&digest, algorithm)?;
        matches.push(digest);
    }
    ensure!(
        matches.len() == 1,
        "{document} must contain exactly one checksum for {asset}"
    );
    Ok(matches.remove(0))
}
fn check_cancel(cancelled: &Token) -> Result<()> {
    if cancelled.is_canceled() {
        return Err(Error::Canceled("download").into());
    }
    Ok(())
}

pub fn pinned_tag(provider: Provider, installed_name: &str) -> Result<String> {
    let prefix = match provider {
        Provider::Dxvk => "dxvk-",
        Provider::Vkd3d => "vkd3d-proton-",
        _ => bail!("only components can use a pinned build download"),
    };
    let version = installed_name
        .strip_prefix(prefix)
        .filter(|v| !v.is_empty());
    ensure!(
        version.is_some(),
        "`{installed_name}` is not an upstream {provider} build name"
    );
    Ok(format!("v{}", version.unwrap()))
}

fn destination_identity(destination: &Path) -> Result<PathBuf> {
    let parent = destination.parent().context("destination has no parent")?;
    let leaf = destination
        .file_name()
        .context("destination has no filename")?;
    Ok(resolve(parent).join(leaf))
}

/// The lock file stays in place so another process cannot acquire a new inode.
pub struct InstallLock {
    file: File,
}
impl InstallLock {
    pub fn acquire(destination: &Path) -> Result<Self> {
        let destination = destination_identity(destination)?;
        let parent = destination.parent().context("destination has no parent")?;
        fs::create_dir_all(parent)?;
        let key = format!(
            "{:x}",
            Sha256::digest(destination.as_os_str().as_encoded_bytes())
        );
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(parent.join(format!(".wineo-lock-{key}")))?;
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .context("another install is using this destination")?;
        Ok(Self { file })
    }
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        // A forked child inherits a copy of the fd, which keeps the flock held
        // on the open file description until it exits or execs. An explicit
        // unlock frees the lock the moment this holder is done with it.
        let _ = rustix::fs::flock(&self.file, rustix::fs::FlockOperation::Unlock);
    }
}
pub fn publish(staging: &Path, destination: &Path) -> Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        staging,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .with_context(|| format!("cannot publish without replacing {}", destination.display()))?;
    Ok(())
}

fn safe_path(path: &Path) -> Result<PathBuf> {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(s) => result.push(s),
            Component::CurDir => {}
            _ => bail!("unsafe archive path: {}", path.display()),
        }
    }
    ensure!(!result.as_os_str().is_empty(), "empty archive path");
    Ok(result)
}
fn parents_are_directories(root: &Path, path: &Path) -> Result<()> {
    let mut parent = root.to_owned();
    for component in path.parent().unwrap_or(Path::new("")).components() {
        parent.push(component);
        match fs::symlink_metadata(&parent) {
            Ok(meta) => ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "archive parent is not a directory: {}",
                parent.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&parent)?,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn validate_link(path: &Path, links: &BTreeMap<PathBuf, PathBuf>) -> Result<()> {
    let mut todo: VecDeque<_> = path
        .components()
        .map(|p| p.as_os_str().to_owned())
        .collect();
    let mut resolved = PathBuf::new();
    let mut followed = 0;
    while let Some(part) = todo.pop_front() {
        if part == "." {
            continue;
        }
        if part == ".." {
            ensure!(resolved.pop(), "archive link escapes staging");
            continue;
        }
        resolved.push(part);
        if let Some(target) = links.get(&resolved) {
            followed += 1;
            ensure!(followed <= 40, "archive link cycle or excessive chain");
            ensure!(!target.is_absolute(), "absolute archive link");
            resolved.pop();
            let components: Vec<_> = target
                .components()
                .map(|p| p.as_os_str().to_owned())
                .collect();
            for component in components.into_iter().rev() {
                todo.push_front(component);
            }
        }
    }
    Ok(())
}
fn extract(archive: &Path, name: &str, root: &Path, cancelled: &Token) -> Result<()> {
    let file = File::open(archive)?;
    let reader: Box<dyn Read> = if name.ends_with(".tar.gz") {
        Box::new(flate2::read::GzDecoder::new(file))
    } else if name.ends_with(".tar.xz") {
        Box::new(xz2::read::XzDecoder::new(file))
    } else if name.ends_with(".tar.zst") {
        Box::new(zstd::stream::read::Decoder::new(file)?)
    } else {
        bail!("unsupported archive compression")
    };
    let mut archive = tar::Archive::new(reader);
    let mut symlinks = BTreeMap::new();
    let mut hardlinks = Vec::new();
    let mut seen = BTreeMap::new();
    for entry in archive.entries()? {
        check_cancel(cancelled)?;
        let mut entry = entry?;
        let path = safe_path(&entry.path()?)?;
        ensure!(
            path.file_name().is_none_or(|s| s != RECEIPT),
            "archive contains reserved receipt"
        );
        let kind = entry.header().entry_type();
        if let Some(previous) = seen.insert(path.clone(), kind) {
            ensure!(
                kind.is_dir() && previous.is_dir(),
                "duplicate archive path {}",
                path.display()
            );
        }
        parents_are_directories(root, &path)?;
        let target = root.join(&path);
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
        } else if kind.is_file() {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&target)?;
            let mut buffer = [0; 128 * 1024];
            loop {
                check_cancel(cancelled)?;
                let n = entry.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                file.write_all(&buffer[..n])?;
            }
            fs::set_permissions(
                &target,
                fs::Permissions::from_mode(entry.header().mode()? & 0o777),
            )?;
        } else if kind.is_symlink() || kind.is_hard_link() {
            let link = entry
                .link_name()?
                .context("archive link has no target")?
                .into_owned();
            if kind.is_symlink() {
                ensure!(!link.is_absolute(), "absolute archive symlink");
                symlinks.insert(path, link);
            } else {
                hardlinks.push((path, safe_path(&link)?));
            }
        } else {
            bail!("unsupported special archive entry: {}", path.display());
        }
    }
    for path in symlinks.keys() {
        validate_link(path, &symlinks)?;
    }
    // Hardlinks can only reference extracted regular files, never symlinks.
    while !hardlinks.is_empty() {
        let before = hardlinks.len();
        let mut pending = Vec::new();
        for (path, target) in hardlinks {
            match fs::symlink_metadata(root.join(&target)) {
                Ok(meta) => {
                    ensure!(meta.is_file(), "hardlink target is not a regular file");
                    fs::hard_link(root.join(target), root.join(path))?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => pending.push((path, target)),
                Err(e) => return Err(e.into()),
            }
        }
        ensure!(
            pending.len() < before,
            "missing or cyclic archive hardlink target"
        );
        hardlinks = pending;
    }
    for (path, target) in symlinks {
        ensure!(
            !root.join(&path).try_exists()?,
            "symlink conflicts with archive directory"
        );
        symlink(target, root.join(path))?;
    }
    Ok(())
}
fn validate_layout(provider: Provider, root: &Path) -> Result<()> {
    match provider {
        Provider::GeProton | Provider::ProtonCachyos => {
            let metadata = fs::metadata(root.join("proton"))
                .context("runner archive has no proton executable")?;
            ensure!(
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
                "proton is not executable"
            );
            ensure!(
                ["files", "dist"]
                    .into_iter()
                    .any(|d| ["wine", "wineserver"].iter().all(|name| {
                        fs::metadata(root.join(d).join("bin").join(name))
                            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    })),
                "runner archive has no complete Wine payload"
            );
        }
        Provider::Dxvk | Provider::Vkd3d => {
            let (narrow, marker) = if provider == Provider::Dxvk {
                ("x32", "dxgi.dll")
            } else {
                ("x86", "d3d12.dll")
            };
            ensure!(
                root.join("x64").join(marker).is_file() && root.join(narrow).join(marker).is_file(),
                "component archive is missing expected DLLs"
            );
        }
    }
    Ok(())
}
fn locate_build(provider: Provider, root: &Path) -> Result<PathBuf> {
    if validate_layout(provider, root).is_ok() {
        return Ok(root.to_owned());
    }
    let entries: Vec<_> = fs::read_dir(root)?.collect::<std::io::Result<_>>()?;
    ensure!(
        entries.len() == 1 && entries[0].file_type()?.is_dir(),
        "archive must contain one build directory"
    );
    let build = entries[0].path();
    validate_layout(provider, &build)?;
    Ok(build)
}

#[cfg(test)]
mod tests;
