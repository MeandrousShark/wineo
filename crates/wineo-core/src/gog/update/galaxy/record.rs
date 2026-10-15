//! The Galaxy record: `<folder key>.galaxy.json` beside a folder's state
//! file, holding what the version 1 state has no room for. The state stays
//! authoritative; the record is evidence, never authority.
//!
//! - `installed`: the filtered manifest of the committed selection, with the
//!   selection's digest and the hash of `goggame-<id>.info` at commit. It
//!   counts only while both still match.
//! - `pending`: the target's manifest, the journal operation it belongs to,
//!   the digest of the selection it will commit, the redistributable
//!   versions it uses, and for an update the manifest it started from. It
//!   counts only with a matching journal.
//! - `follow`: the branch followed and the pinned build.
//! - `setup`: the setup steps that finished.
//!
//! Write order: `pending` before an operation's journal, `installed` before
//! its commit, and `pending` dropped by the next write after the commit.
//! Nothing is ever promoted from `pending`. A record that is missing,
//! corrupt, or stale means the game has no stored manifest.

use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::gog::folder::{Committed, FolderLock, FolderState, JournalKind, Source, Target, Track};

/// The record's format. Any other version reads as no record at all.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<Installed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    #[serde(default)]
    pub follow: Follow,
    #[serde(default)]
    pub setup: Vec<SetupStep>,
}

impl Default for Record {
    fn default() -> Self {
        Self {
            version: VERSION,
            installed: None,
            pending: None,
            follow: Follow::default(),
            setup: Vec::new(),
        }
    }
}

/// What the committed selection installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    /// `digest` of the state's `committed` this was written for.
    pub selection: String,
    /// `info_hash` of the folder when it was written. None when the game had
    /// no info file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
    pub manifest: Manifest,
}

/// What an operation in flight is installing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    /// The journal operation this belongs to.
    pub operation: String,
    /// `digest` of the `committed` the operation will write.
    pub selection: String,
    pub manifest: Manifest,
    /// Every redistributable version the operation uses, named before any
    /// is downloaded so store cleanup keeps them.
    #[serde(default)]
    pub store: Vec<StoreVersion>,
    /// What an update started from, kept so finishing it after a crash
    /// treats each file as the first attempt did. Its new info file makes
    /// `installed` stop counting partway through. None for an install, and
    /// for an update with no stored manifest, which deletes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Base>,
}

/// The manifest an update compares the target with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Base {
    pub manifest: Manifest,
    /// Whether it was fetched from GOG for the installed build rather than
    /// stored at commit, for a game installed by another tool.
    #[serde(default)]
    pub fetched: bool,
}

/// The branch a game follows and the build it's pinned to. Part B sets
/// these; until then a game follows the default branch, unpinned.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Follow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<String>,
}

/// A setup step that finished, with what identifies it, so a change runs it
/// again: a step counts as done only while every field still matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SetupStep {
    /// GOG's script interpreter, run for one product.
    ScriptInterpreter {
        product: u64,
        /// The build it set up.
        build: String,
        /// The MD5 of the product's `goggame-<id>.script`. None when the
        /// build has none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        script: Option<String>,
        /// The interpreter's own manifest hash, so a new one runs again.
        interpreter: String,
    },
    /// A product's own setup program, from its support folder.
    Program {
        product: u64,
        /// Relative to the product's support folder.
        path: String,
        /// Everything it was given.
        arguments: String,
        md5: String,
    },
    /// A redistributable from the store.
    Redistributable {
        dependency: String,
        manifest: String,
    },
}

impl SetupStep {
    /// Whether `other` is the same step, whatever its identity: the same
    /// product's script or program, or the same dependency.
    pub fn same_step(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::ScriptInterpreter { product: a, .. },
                Self::ScriptInterpreter { product: b, .. },
            )
            | (Self::Program { product: a, .. }, Self::Program { product: b, .. }) => a == b,
            (
                Self::Redistributable { dependency: a, .. },
                Self::Redistributable { dependency: b, .. },
            ) => a == b,
            _ => false,
        }
    }

    fn check(&self) -> Result<(), String> {
        let fine = match self {
            Self::ScriptInterpreter {
                script,
                interpreter,
                ..
            } => is_hex(interpreter, 32) && script.as_deref().is_none_or(|md5| is_hex(md5, 32)),
            Self::Program { path, md5, .. } => check_path(path).is_ok() && is_hex(md5, 32),
            Self::Redistributable {
                dependency,
                manifest,
            } => super::store::valid_id(dependency) && is_hex(manifest, 32),
        };
        if fine {
            Ok(())
        } else {
            Err("invalid setup step".to_owned())
        }
    }
}

/// One version of a redistributable in the shared store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreVersion {
    pub dependency: String,
    /// The dependency's depot manifest hash, which names its folder.
    pub manifest: String,
}

/// A build's files as wineo installs them: filtered to the selection, with
/// paths in the form they have on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The base game's build.
    pub build: String,
    /// In the game folder, relative to it, with `/` between parts. Sorted by
    /// path.
    #[serde(default)]
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub directories: Vec<DirEntry>,
    #[serde(default)]
    pub links: Vec<LinkEntry>,
    /// Support files, each relative to its product's support folder.
    #[serde(default)]
    pub support: Vec<FileEntry>,
    #[serde(default)]
    pub setup: SetupInfo,
}

/// Who a manifest entry comes from: a product, or a dependency installed
/// into the game folder. Exactly one is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency: Option<String>,
    pub size: u64,
    /// The whole file's MD5. Known before downloading when GOG lists it or
    /// the file is one chunk, else filled in once it's downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default)]
    pub chunks: Vec<ChunkEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkEntry {
    pub md5: String,
    pub size: u64,
    pub compressed_md5: String,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirEntry {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkEntry {
    pub path: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency: Option<String>,
}

/// What setup runs, kept for the setup step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupInfo {
    /// Whether GOG's script interpreter runs each product's script.
    #[serde(default)]
    pub script_interpreter: bool,
    #[serde(default)]
    pub products: Vec<ProductSetup>,
    /// Redistributables from the store, in the order the build lists them,
    /// the script interpreter included when it runs.
    #[serde(default)]
    pub redist: Vec<Redist>,
    /// GOG's name for the build, like `1.0.4`, which setup programs are
    /// given. Kept here so finishing from `pending` has it offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductSetup {
    pub product: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temp_executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temp_arguments: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redist {
    pub dependency: String,
    pub manifest: String,
    /// Relative to the version's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default)]
    pub arguments: String,
}

impl Manifest {
    /// Every store version setup uses.
    pub fn store_versions(&self) -> Vec<StoreVersion> {
        let mut versions: Vec<StoreVersion> = self
            .setup
            .redist
            .iter()
            .map(|redist| StoreVersion {
                dependency: redist.dependency.clone(),
                manifest: redist.manifest.clone(),
            })
            .collect();
        versions.sort();
        versions.dedup();
        versions
    }

    fn check(&self) -> Result<(), String> {
        let owned = |product: &Option<u64>, dependency: &Option<String>| {
            product.is_some() != dependency.is_some()
                && dependency.as_deref().is_none_or(super::store::valid_id)
        };
        // Setup reads support files from their product's folder.
        if let Some(file) = self.support.iter().find(|file| file.product.is_none()) {
            return Err(format!("support file {} has no product", file.path));
        }
        for file in self.files.iter().chain(&self.support) {
            check_path(&file.path)?;
            if !owned(&file.product, &file.dependency)
                || file.md5.as_deref().is_some_and(|md5| !is_hex(md5, 32))
                || file.sha256.as_deref().is_some_and(|sha| !is_hex(sha, 64))
                || file
                    .chunks
                    .iter()
                    .any(|chunk| !is_hex(&chunk.md5, 32) || !is_hex(&chunk.compressed_md5, 32))
                || file
                    .chunks
                    .iter()
                    .try_fold(0u64, |total, chunk| total.checked_add(chunk.size))
                    != Some(file.size)
            {
                return Err(format!("file {} doesn't add up", file.path));
            }
        }
        for dir in &self.directories {
            check_path(&dir.path)?;
            if !owned(&dir.product, &dir.dependency) {
                return Err(format!("folder {} has no owner", dir.path));
            }
        }
        for link in &self.links {
            check_path(&link.path)?;
            if !owned(&link.product, &link.dependency) || link.target.is_empty() {
                return Err(format!("link {} is incomplete", link.path));
            }
            if link.target.starts_with('/') || link.target.contains([':', '\\', '\0']) {
                return Err(format!("link {} points outside the game", link.path));
            }
        }
        // The same rule a fresh build's links are held to.
        let links: std::collections::BTreeMap<&str, &str> = self
            .links
            .iter()
            .map(|link| (link.path.as_str(), link.target.as_str()))
            .collect();
        crate::gog::package::check_link_targets(&links).map_err(|error| error.to_string())?;
        for redist in &self.setup.redist {
            if !super::store::valid_id(&redist.dependency) || !is_hex(&redist.manifest, 32) {
                return Err(format!("redistributable {} is invalid", redist.dependency));
            }
        }
        Ok(())
    }
}

/// A relative path in normal form with `/` between parts, as every entry
/// has once it's on disk.
fn check_path(path: &str) -> Result<(), String> {
    let fine = !path.is_empty()
        && path.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.contains(['\0', ':', '\\'])
        });
    if fine {
        Ok(())
    } else {
        Err(format!("{path:?} isn't a relative path"))
    }
}

fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Record {
    /// A record from its file's bytes. None for anything that isn't a valid
    /// version 1 record, which counts as no record.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let record: Self = serde_json::from_slice(bytes).ok()?;
        record.check().ok()?;
        Some(record)
    }

    pub fn to_json(&self) -> Result<String> {
        self.check().map_err(anyhow::Error::msg)?;
        Ok(serde_json::to_string_pretty(self)?)
    }

    fn check(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!(
                "unsupported Galaxy record version {}",
                self.version
            ));
        }
        if let Some(installed) = &self.installed {
            installed.manifest.check()?;
        }
        for step in &self.setup {
            step.check()?;
        }
        if let Some(pending) = &self.pending {
            pending.manifest.check()?;
            if let Some(base) = &pending.base {
                base.manifest.check()?;
            }
            if pending.operation.is_empty()
                || !pending
                    .operation
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err("invalid operation id".to_owned());
            }
            if pending.store.iter().any(|version| {
                !super::store::valid_id(&version.dependency) || !is_hex(&version.manifest, 32)
            }) {
                return Err("invalid redistributable version".to_owned());
            }
        }
        Ok(())
    }

    /// Whether `installed` or `pending` names a store version, read as
    /// widely as can be: whatever the state says, so store cleanup never
    /// removes a version a record might still need.
    pub fn names(&self, version: &StoreVersion) -> bool {
        self.installed
            .as_ref()
            .is_some_and(|installed| installed.manifest.store_versions().contains(version))
            || self.pending.as_ref().is_some_and(|pending| {
                pending.store.contains(version)
                    || pending.manifest.store_versions().contains(version)
            })
    }
}

/// The digest of a selection: SHA-256 of the whole `committed` record, so a
/// DLC-only change, a language change, or a retried DLC changes it even
/// when the base build stays the same.
pub fn digest(committed: &Committed) -> String {
    let bytes = serde_json::to_vec(committed).expect("a committed record serializes");
    crate::units::hex(Sha256::digest(bytes))
}

/// The `committed` an operation on `state` writes when it finishes `target`,
/// as `FolderLock::commit` will write it.
pub fn expected(state: &FolderState, kind: JournalKind, target: &Target) -> Committed {
    let source = match kind {
        JournalKind::Install => Some(Source::Install),
        _ => state.committed.as_ref().and_then(|old| old.source),
    };
    Committed {
        gog_id: target.gog_id,
        platform: target.platform,
        language: target.language.clone(),
        track: target.track,
        products: target.products.clone(),
        files: Vec::new(),
        source,
    }
}

/// The SHA-256 of the folder's `goggame-<id>.info`, read without following
/// a symlink there. None when there's no such file.
pub fn info_hash(folder: &Path, gog_id: u64) -> Result<Option<String>> {
    let path = folder.join(format!("goggame-{gog_id}.info"));
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(&path)
    {
        Ok(file) => file,
        Err(error)
            if matches!(error.kind(), io::ErrorKind::NotFound)
                || error.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) =>
        {
            return Ok(None);
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    let mut hasher = Sha256::new();
    let mut reader = io::BufReader::new(file);
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()));
            }
        };
        hasher.update(&buffer[..read]);
    }
    Ok(Some(crate::units::hex(hasher.finalize())))
}

/// The folder's record under its lock. None when it's missing or isn't a
/// valid record, both of which mean no stored manifest.
pub fn read(lock: &FolderLock) -> Result<Option<Record>> {
    Ok(lock.read_sidecar()?.and_then(|bytes| Record::parse(&bytes)))
}

/// Replaces the folder's record in one synced write.
pub fn write(lock: &FolderLock, record: &Record) -> Result<()> {
    lock.write_sidecar(&record.to_json()?)
}

/// `installed`, when it describes what `state` has committed: its digest
/// matches the state's `committed` and the info file's hash still matches.
pub fn installed<'a>(state: &FolderState, record: &'a Record) -> Result<Option<&'a Installed>> {
    let (Some(committed), Some(installed)) = (&state.committed, &record.installed) else {
        return Ok(None);
    };
    if committed.track != Track::Galaxy || installed.selection != digest(committed) {
        return Ok(None);
    }
    if info_hash(&state.folder, committed.gog_id)? != installed.info {
        return Ok(None);
    }
    Ok(Some(installed))
}

/// `pending`, when the state's journal is its operation and the selection
/// that journal commits is the one it was written for. With no journal it
/// belongs to an operation that never started, whatever its target.
pub fn pending<'a>(state: &FolderState, record: &'a Record) -> Option<&'a Pending> {
    let journal = state.journal.as_ref()?;
    let pending = record.pending.as_ref()?;
    (journal.target.track == Track::Galaxy
        && pending.operation == journal.operation
        && pending.selection == digest(&expected(state, journal.kind, &journal.target)))
    .then_some(pending)
}

/// Whether the setup steps `record` shows done still count for an
/// operation on `state`: only while its `pending` is that operation's or
/// its `installed` is the committed selection's. Anything else, like a
/// record left from an earlier install or a lost `pending`, runs every step
/// again, since a step skipped wrongly would leave the prefix without it.
pub fn carries_setup(state: &FolderState, record: &Record) -> Result<bool> {
    Ok(pending(state, record).is_some() || installed(state, record)?.is_some())
}

/// Reads every folder's record without locks, for store cleanup. Fails when
/// one can't be read at all; one that isn't a valid record names nothing.
pub(super) fn all(dir: &Path) -> Result<Vec<Record>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", dir.display()));
        }
    };
    let mut records = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') || !name.ends_with(crate::gog::folder::SIDECAR_SUFFIX) {
            continue;
        }
        let mut bytes = Vec::new();
        match File::open(&path) {
            Ok(mut file) => {
                file.read_to_end(&mut bytes)
                    .with_context(|| format!("failed to read {}", path.display()))?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()));
            }
        }
        if let Some(record) = Record::parse(&bytes) {
            records.push(record);
        }
    }
    Ok(records)
}
