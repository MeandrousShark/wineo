//! Verified upstream downloads, staged on the destination filesystem.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};

use crate::config::GlobalConfig;
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
}
pub struct Https(reqwest::blocking::Client);
impl Https {
    /// An HTTPS client for API requests.
    pub fn new() -> Result<Self> {
        Ok(Self(http_client(true, Duration::from_secs(10))?))
    }
}
impl Transport for Https {
    fn request(&self, url: &str, bearer: Option<&str>) -> Result<Response> {
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
            .with_context(|| format!("request failed: {url}"))?;
        Ok(Response {
            status: response.status().as_u16(),
            body: Box::new(response),
        })
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
            .with_context(|| format!("request failed: {url}"))?;
        let status = response.status();
        if status.as_u16() == 403 || status.as_u16() == 429 {
            bail!("GitHub denied or rate-limited the request ({status}); try again later");
        }
        Ok(Box::new(
            response
                .error_for_status()
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
    // Blocking responses apply this timeout to each Read, not the entire archive.
    Ok(reqwest::blocking::Client::builder()
        .https_only(https_only)
        .connect_timeout(timeout)
        .timeout(timeout)
        .user_agent(concat!("wineo/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?)
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
            .read_to_end(&mut data)?;
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
    pub fn install(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
        release: &Release,
        cancelled: &AtomicBool,
        progress: impl FnMut(Progress),
    ) -> Result<PathBuf> {
        self.install_with_phase(paths, global, release, cancelled, progress, |_| {})
    }
    fn install_with_phase(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
        release: &Release,
        cancelled: &AtomicBool,
        mut progress: impl FnMut(Progress),
        mut phase: impl FnMut(InstallPhase),
    ) -> Result<PathBuf> {
        check_cancel(cancelled)?;
        let destination = destination_identity(&release.destination(paths, global)?)?;
        let _lock = InstallLock::acquire(&destination)?;
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
                .read_to_string(&mut data)?;
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
fn check_cancel(cancelled: &AtomicBool) -> Result<()> {
    ensure!(!cancelled.load(Ordering::Relaxed), "download canceled");
    Ok(())
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
fn extract(archive: &Path, name: &str, root: &Path, cancelled: &AtomicBool) -> Result<()> {
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
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Fixture(BTreeMap<String, Vec<u8>>);
    impl Transport for Fixture {
        fn get(&self, url: &str) -> Result<Box<dyn Read>> {
            Ok(Box::new(Cursor::new(
                self.0
                    .get(url)
                    .with_context(|| format!("unexpected fixture URL {url}"))?
                    .clone(),
            )))
        }
    }
    fn upstream(tag: &str, names: &[&str]) -> UpstreamRelease {
        UpstreamRelease {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: names
                .iter()
                .map(|name| Asset {
                    name: (*name).into(),
                    size: 1,
                    digest: None,
                    browser_download_url: format!("https://fixtures/{name}"),
                })
                .collect(),
        }
    }
    fn tar_bytes(entries: &[(&str, &[u8], u32)], links: &[(&str, &str, bool)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, bytes, mode) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            builder.append_data(&mut header, path, *bytes).unwrap();
        }
        for (path, target, hard) in links {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_entry_type(if *hard {
                tar::EntryType::Link
            } else {
                tar::EntryType::Symlink
            });
            header.set_link_name(target).unwrap();
            header.set_cksum();
            builder.append_data(&mut header, path, &[][..]).unwrap();
        }
        builder.into_inner().unwrap()
    }
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(bytes).unwrap();
        gzip.finish().unwrap()
    }
    fn extract_bytes(bytes: &[u8], name: &str, root: &Path) -> Result<()> {
        let archive = root.parent().unwrap().join("archive");
        fs::write(&archive, bytes)?;
        extract(&archive, name, root, &AtomicBool::new(false))
    }
    fn fixture_paths(root: &Path) -> Paths {
        Paths::from_lookup(|key| match key {
            "HOME" => Some(root.as_os_str().to_owned()),
            _ => None,
        })
        .unwrap()
    }
    #[test]
    fn provider_assets_cover_historical_and_architecture_suffixes() {
        for (provider, tag, filename) in [
            (Provider::GeProton, "GE-Proton9-19", "GE-Proton9-19.tar.gz"),
            (
                Provider::GeProton,
                "GE-Proton11-5",
                "GE-Proton11-5-x86_64.tar.gz",
            ),
            (
                Provider::ProtonCachyos,
                "cachyos-10.0-20260425-slr",
                "proton-cachyos-10.0-20260425-slr-x86_64.tar.xz",
            ),
            (
                Provider::ProtonCachyos,
                "cachyos-10.0-20260425-slr",
                "proton-cachyos-10.0-20260425-slr-x86-64.tar.xz",
            ),
            (Provider::Dxvk, "v2.6", "dxvk-2.6.tar.gz"),
            (Provider::Vkd3d, "v2.14", "vkd3d-proton-2.14.tar.zst"),
            (Provider::Vkd3d, "v2.10", "vkd3d-proton-2.10.tar.gz"),
        ] {
            assert_eq!(
                select_asset(provider, upstream(tag, &[filename]))
                    .unwrap()
                    .unwrap()
                    .asset_name,
                filename
            );
        }
        assert!(
            select_asset(
                Provider::Dxvk,
                upstream(
                    "v2.6",
                    &["dxvk-native-2.6.tar.gz", "dxvk-2.6-aarch64.tar.gz"]
                )
            )
            .unwrap()
            .is_none()
        );
        assert!(
            select_asset(
                Provider::ProtonCachyos,
                upstream(
                    "x",
                    &[
                        "proton-cachyos-10-slr-x86_64-v3.tar.xz",
                        "proton-cachyos-10-slr-aarch64.tar.xz"
                    ]
                )
            )
            .unwrap()
            .is_none()
        );
        assert!(
            select_asset(
                Provider::GeProton,
                upstream(
                    "GE-Proton11-5",
                    &["GE-Proton11-5.tar.gz", "GE-Proton11-5-x86_64.tar.gz"]
                )
            )
            .is_err()
        );
    }
    #[test]
    fn checksums_match_the_exact_asset_and_fail_closed() {
        let sha = "a".repeat(64);
        assert_eq!(
            parse_checksum(
                &format!("{sha} *build.tar.gz\n"),
                "build.tar.gz",
                "sha256sums"
            )
            .unwrap(),
            format!("sha256:{sha}")
        );
        assert!(
            parse_checksum(&format!("{sha} other.tar.gz"), "build.tar.gz", "checksums").is_err()
        );
        assert!(
            parse_checksum(
                &format!("{sha} build.tar.gz\n{sha} build.tar.gz"),
                "build.tar.gz",
                "checksums"
            )
            .is_err()
        );
        assert!(parse_checksum("invalid build.tar.gz", "build.tar.gz", "checksums").is_err());
        assert!(validate_digest("sha1:abc", "sha256").is_err());
    }
    #[test]
    fn extraction_preserves_modes_internal_symlinks_and_hardlinks() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("out");
        fs::create_dir(&root).unwrap();
        let bytes = gzip(&tar_bytes(
            &[("build/bin/program", b"abc", 0o755)],
            &[
                ("build/run", "bin/program", false),
                ("build/hard", "build/bin/program", true),
            ],
        ));
        extract_bytes(&bytes, "b.tar.gz", &root).unwrap();
        assert_eq!(fs::read(root.join("build/run")).unwrap(), b"abc");
        assert_eq!(
            fs::metadata(root.join("build/run"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(root.join("build/hard")).unwrap().ino(),
            fs::metadata(root.join("build/bin/program")).unwrap().ino()
        );
    }
    #[test]
    fn extraction_rejects_escaping_links_duplicates_and_special_files() {
        for links in [
            vec![("build/link", "../../escape", false)],
            vec![("build/link", "/tmp/escape", false)],
            vec![("build/a", "b", false), ("build/b", "a", false)],
            vec![
                ("build/link", "../dir", false),
                ("build/other", "link/../../escape", false),
            ],
            vec![("build/hard", "../escape", true)],
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("out");
            fs::create_dir(&root).unwrap();
            assert!(extract_bytes(&gzip(&tar_bytes(&[], &links)), "b.tar.gz", &root).is_err());
        }
        assert!(safe_path(Path::new("../escape")).is_err());
        assert!(safe_path(Path::new("/escape")).is_err());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("out");
        fs::create_dir(&root).unwrap();
        assert!(
            extract_bytes(
                &gzip(&tar_bytes(
                    &[("file", b"a", 0o644), ("file", b"b", 0o644)],
                    &[]
                )),
                "b.tar.gz",
                &root
            )
            .is_err()
        );
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("out");
        fs::create_dir(&root).unwrap();
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Fifo);
        header.set_size(0);
        header.set_mode(0o600);
        header.set_cksum();
        builder.append_data(&mut header, "fifo", &[][..]).unwrap();
        assert!(extract_bytes(&gzip(&builder.into_inner().unwrap()), "b.tar.gz", &root).is_err());
    }
    #[test]
    fn gzip_xz_and_zstd_are_supported() {
        let bytes = tar_bytes(&[("file", b"contents", 0o644)], &[]);
        let mut xz = xz2::write::XzEncoder::new(Vec::new(), 1);
        xz.write_all(&bytes).unwrap();
        for (name, compressed) in [
            ("b.tar.gz", gzip(&bytes)),
            ("b.tar.xz", xz.finish().unwrap()),
            (
                "b.tar.zst",
                zstd::stream::encode_all(bytes.as_slice(), 1).unwrap(),
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("out");
            fs::create_dir(&root).unwrap();
            extract_bytes(&compressed, name, &root).unwrap();
            assert_eq!(fs::read(root.join("file")).unwrap(), b"contents");
        }
    }
    fn install_fixture(checksum: bool) -> (Client<Fixture>, Release) {
        let bytes = gzip(&tar_bytes(
            &[
                ("dxvk-2.6/x64/dxgi.dll", b"wide", 0o644),
                ("dxvk-2.6/x32/dxgi.dll", b"narrow", 0o644),
            ],
            &[],
        ));
        let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        let mut release = upstream("v2.6", &["dxvk-2.6.tar.gz"]);
        release.assets[0].size = bytes.len() as u64;
        let mut fixture =
            BTreeMap::from([("https://fixtures/dxvk-2.6.tar.gz".into(), bytes.clone())]);
        if checksum {
            release.assets.push(Asset {
                name: "dxvk-2.6.sha512sum".into(),
                browser_download_url: "https://fixtures/sum".into(),
                size: 1,
                digest: None,
            });
            fixture.insert(
                "https://fixtures/sum".into(),
                format!("{:x}  dxvk-2.6.tar.gz\n", Sha512::digest(&bytes)).into_bytes(),
            );
        } else {
            release.assets[0].digest = Some(digest);
        }
        (
            Client::with_transport(Fixture(fixture)),
            select_asset(Provider::Dxvk, release).unwrap().unwrap(),
        )
    }
    #[test]
    fn installation_is_verified_atomic_and_idempotent() {
        for checksum in [false, true] {
            let (client, release) = install_fixture(checksum);
            let temp = tempfile::tempdir().unwrap();
            let paths = fixture_paths(temp.path());
            let destination = client
                .install(
                    &paths,
                    &GlobalConfig::default(),
                    &release,
                    &AtomicBool::new(false),
                    |_| {},
                )
                .unwrap();
            assert!(destination.join("x64/dxgi.dll").is_file());
            let receipt: Receipt =
                serde_json::from_slice(&fs::read(destination.join(RECEIPT)).unwrap()).unwrap();
            assert_eq!(receipt.version, "v2.6");
            let mut already = false;
            client
                .install(
                    &paths,
                    &GlobalConfig::default(),
                    &release,
                    &AtomicBool::new(false),
                    |p| {
                        already |= matches!(p, Progress::AlreadyInstalled(_));
                    },
                )
                .unwrap();
            assert!(already);
        }
    }
    #[test]
    fn cancellation_on_either_side_of_publication_matches_installed_state() {
        let (client, release) = install_fixture(false);
        let temp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(temp.path());
        let global = GlobalConfig::default();
        let destination = release.destination(&paths, &global).unwrap();
        let cancelled = AtomicBool::new(false);
        let result = client.install_with_phase(
            &paths,
            &global,
            &release,
            &cancelled,
            |_| {},
            |phase| {
                if matches!(phase, InstallPhase::BeforePublication) {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        );
        assert!(result.unwrap_err().to_string().contains("canceled"));
        assert!(!destination.exists());
        let parent = destination.parent().unwrap();
        assert!(fs::read_dir(parent).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&stage_prefix(&destination))
        }));

        cancelled.store(false, Ordering::Relaxed);
        let result = client.install_with_phase(
            &paths,
            &global,
            &release,
            &cancelled,
            |_| {},
            |phase| {
                if matches!(phase, InstallPhase::AfterPublication) {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(result.unwrap(), destination);
        assert!(cancelled.load(Ordering::Relaxed));
        assert_eq!(fs::read(destination.join("x64/dxgi.dll")).unwrap(), b"wide");
        assert!(destination.join(RECEIPT).is_file());
    }
    #[test]
    fn abandoned_stage_cleanup_is_locked_and_scoped_to_destination() {
        let (client, release) = install_fixture(false);
        let (racing_client, _) = install_fixture(false);
        let temp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(temp.path());
        let global = GlobalConfig::default();
        let destination =
            destination_identity(&release.destination(&paths, &global).unwrap()).unwrap();
        let parent = destination.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        let abandoned = parent.join(format!("{}abandoned", stage_prefix(&destination)));
        let other_destination = parent.join("other-build");
        let unrelated = parent.join(format!("{}active", stage_prefix(&other_destination)));
        let completed = parent.join("completed-build");
        for dir in [&abandoned, &unrelated, &completed] {
            fs::create_dir(dir).unwrap();
            fs::write(dir.join("marker"), dir.as_os_str().as_encoded_bytes()).unwrap();
        }
        let mut checked_active_stage = false;
        client
            .install(
                &paths,
                &global,
                &release,
                &AtomicBool::new(false),
                |phase| {
                    if matches!(phase, Progress::Downloading { .. }) && !checked_active_stage {
                        checked_active_stage = true;
                        let active = fs::read_dir(parent)
                            .unwrap()
                            .map(|entry| entry.unwrap().path())
                            .find(|path| {
                                path.file_name()
                                    .unwrap()
                                    .to_string_lossy()
                                    .starts_with(&stage_prefix(&destination))
                            })
                            .unwrap();
                        assert!(
                            racing_client
                                .install(&paths, &global, &release, &AtomicBool::new(false), |_| {})
                                .unwrap_err()
                                .to_string()
                                .contains("another install")
                        );
                        assert!(active.exists());
                    }
                },
            )
            .unwrap();
        assert!(checked_active_stage);
        assert!(!abandoned.exists());
        assert!(unrelated.join("marker").is_file());
        assert!(completed.join("marker").is_file());
        assert!(destination.join(RECEIPT).is_file());

        fs::create_dir(&abandoned).unwrap();
        fs::write(abandoned.join("marker"), b"orphan").unwrap();
        client
            .install(
                &paths,
                &global,
                &release,
                &AtomicBool::new(false),
                |phase| {
                    assert!(matches!(phase, Progress::AlreadyInstalled(_)));
                },
            )
            .unwrap();
        assert!(!abandoned.exists());
        assert!(unrelated.join("marker").is_file());
        assert!(completed.join("marker").is_file());
        assert!(destination.join(RECEIPT).is_file());
    }
    #[test]
    fn corruption_cancellation_and_collision_leave_no_partial_install() {
        let (client, mut release) = install_fixture(false);
        let temp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(temp.path());
        let global = GlobalConfig::default();
        let destination = release.destination(&paths, &global).unwrap();
        assert!(
            client
                .install(&paths, &global, &release, &AtomicBool::new(true), |_| {})
                .is_err()
        );
        assert!(!destination.exists());
        release.asset.digest = Some(format!("sha256:{}", "0".repeat(64)));
        assert!(
            client
                .install(&paths, &global, &release, &AtomicBool::new(false), |_| {})
                .is_err()
        );
        assert!(!destination.exists());
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("mine"), "preserved").unwrap();
        assert!(
            client
                .install(&paths, &global, &release, &AtomicBool::new(false), |_| {})
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(destination.join("mine")).unwrap(),
            "preserved"
        );
    }
    #[test]
    fn destination_lock_and_publication_refuse_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("build");
        let guard = InstallLock::acquire(&dest).unwrap();
        assert!(InstallLock::acquire(&dest).is_err());
        drop(guard);
        let _guard = InstallLock::acquire(&dest).unwrap();
        fs::create_dir(&dest).unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        assert!(publish(&source, &dest).is_err());
        assert!(source.is_dir());
    }
    #[test]
    fn install_lock_releases_even_if_a_forked_child_still_holds_the_fd() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("build");
        let guard = InstallLock::acquire(&dest).unwrap();
        // Stands in for a forked child's copy of the fd, kept alive past the drop.
        let clone = guard.file.try_clone().unwrap();
        drop(guard);
        assert!(InstallLock::acquire(&dest).is_ok());
        drop(clone);
    }
    #[test]
    fn metadata_selection_obeys_stability_pagination_and_exact_tags() {
        let mut releases = Vec::new();
        for index in 0..30 {
            let mut release = upstream(&format!("v3.{index}"), &["unsupported.zip"]);
            release.prerelease = true;
            releases.push(
                serde_json::json!({"tag_name":release.tag_name,"prerelease":true,"assets":[]}),
            );
        }
        let page2 = serde_json::json!([{"tag_name":"v2.6","assets":[{"name":"dxvk-2.6.tar.gz","browser_download_url":"https://fixture/a","size":1,"digest":null}]}]);
        let fixture = Fixture(BTreeMap::from([
            (
                "https://api.github.com/repos/doitsujin/dxvk/releases?per_page=30&page=1".into(),
                serde_json::to_vec(&releases).unwrap(),
            ),
            (
                "https://api.github.com/repos/doitsujin/dxvk/releases?per_page=30&page=2".into(),
                serde_json::to_vec(&page2).unwrap(),
            ),
        ]));
        let client = Client::with_transport(fixture);
        assert!(client.list(Provider::Dxvk, 1).unwrap().has_more);
        assert_eq!(client.select(Provider::Dxvk, None).unwrap().tag, "v2.6");
        assert!(client.select(Provider::Dxvk, Some("v0.0")).is_err());
    }
    #[test]
    fn existing_destination_symlink_is_not_followed() {
        let (client, release) = install_fixture(false);
        let temp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(temp.path());
        let global = GlobalConfig::default();
        let destination = client
            .install(&paths, &global, &release, &AtomicBool::new(false), |_| {})
            .unwrap();
        let elsewhere = temp.path().join("elsewhere");
        fs::rename(&destination, &elsewhere).unwrap();
        symlink(&elsewhere, &destination).unwrap();
        let error = client
            .install(&paths, &global, &release, &AtomicBool::new(false), |_| {})
            .unwrap_err();
        assert!(error.to_string().contains("symlink"));
        assert!(elsewhere.join(RECEIPT).is_file());
    }

    #[test]
    fn missing_malformed_and_disagreeing_verification_is_rejected() {
        let (client, mut release) = install_fixture(true);
        release.asset.digest = Some(format!("sha256:{}", "0".repeat(64)));
        let temp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(temp.path());
        let global = GlobalConfig::default();
        assert!(
            client
                .install(&paths, &global, &release, &AtomicBool::new(false), |_| {})
                .is_err()
        );
        assert!(!release.destination(&paths, &global).unwrap().exists());
        release.asset.digest = Some("sha1:invalid".into());
        assert!(client.expected_digests(&release).is_err());
        release.asset.digest = None;
        release.checksums.clear();
        assert!(client.expected_digests(&release).is_err());
    }

    #[test]
    fn truncated_download_and_midstream_cancel_remove_staging() {
        let (client, mut release) = install_fixture(false);
        let temp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(temp.path());
        let global = GlobalConfig::default();
        let destination = release.destination(&paths, &global).unwrap();
        release.size += 1;
        assert!(
            client
                .install(&paths, &global, &release, &AtomicBool::new(false), |_| {})
                .is_err()
        );
        release.size -= 1;
        let cancelled = AtomicBool::new(false);
        assert!(
            client
                .install(&paths, &global, &release, &cancelled, |p| {
                    if matches!(p, Progress::Downloading { .. }) {
                        cancelled.store(true, Ordering::Relaxed);
                    }
                })
                .is_err()
        );
        assert!(!destination.exists());
        assert!(
            fs::read_dir(destination.parent().unwrap())
                .unwrap()
                .all(|e| !e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".wineo-stage-"))
        );
    }
    #[test]
    fn blocking_http_body_reads_without_an_async_runtime() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfixture",
                )
                .unwrap();
        });
        let client = http_client(false, Duration::from_secs(2)).unwrap();
        let mut response = client
            .get(format!("http://{address}/fixture"))
            .send()
            .unwrap();
        let mut body = String::new();
        response.read_to_string(&mut body).unwrap();
        assert_eq!(body, "fixture");
        server.join().unwrap();
        assert!(Https(client).get("http://example.invalid/fixture").is_err());
    }

    #[test]
    fn blocking_http_stalled_body_has_a_bounded_read_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nabc")
                .unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });
        let client = http_client(false, Duration::from_millis(100)).unwrap();
        let mut response = client
            .get(format!("http://{address}/fixture"))
            .send()
            .unwrap();
        let started = std::time::Instant::now();
        assert!(response.read_exact(&mut [0; 8]).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }
    #[test]
    fn streaming_can_outlast_the_per_read_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\n")
                .unwrap();
            for _ in 0..8 {
                std::thread::sleep(Duration::from_millis(50));
                stream.write_all(b"x").unwrap();
                stream.flush().unwrap();
            }
        });
        let client = http_client(false, Duration::from_millis(200)).unwrap();
        let started = std::time::Instant::now();
        let mut response = client
            .get(format!("http://{address}/fixture"))
            .send()
            .unwrap();
        let mut body = [0; 8];
        response.read_exact(&mut body).unwrap();
        assert_eq!(&body, b"xxxxxxxx");
        assert!(started.elapsed() > Duration::from_millis(200));
        server.join().unwrap();
    }
    #[test]
    fn ge_transition_releases_prefer_the_checksum_backed_gzip_package() {
        // Official GE-Proton10-28 publishes both formats; its .sha512sum names gzip.
        for names in [
            ["GE-Proton10-28.tar.zst", "GE-Proton10-28.tar.gz"],
            ["GE-Proton10-28.tar.gz", "GE-Proton10-28.tar.zst"],
        ] {
            let release = select_asset(Provider::GeProton, upstream("GE-Proton10-28", &names))
                .unwrap()
                .unwrap();
            assert_eq!(release.asset_name, "GE-Proton10-28.tar.gz");
        }
        assert!(
            select_asset(
                Provider::GeProton,
                upstream(
                    "GE-Proton10-28",
                    &["GE-Proton10-28.tar.gz", "GE-Proton10-28-x86_64.tar.zst"]
                )
            )
            .is_err()
        );
        assert!(
            select_asset(
                Provider::GeProton,
                upstream(
                    "GE-Proton10-28",
                    &["GE-Proton10-28.tar.gz", "GE-Proton10-28.tar.gz"]
                )
            )
            .is_err()
        );
    }
}
