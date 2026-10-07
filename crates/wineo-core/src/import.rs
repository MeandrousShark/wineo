//! Import existing Wine prefixes and runner trees without executing anything.
//!
//! Inspection is deliberately separate from execution.  Callers can render an
//! inspection to a review screen, then pass the same inspection to
//! `execute_reviewed`; the source fingerprint is checked again while the
//! operation locks are held.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::ioctl_ficlone;
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use crate::build_lock;
use crate::cancel::Token;
use crate::config::{Arch, EnvironmentConfig, GlobalConfig, Target, validate_env_var};
use crate::download::{InstallLock, publish};
use crate::environment::Environment;
use crate::error::Error;
use crate::gamescope::GamescopeSetting;
use crate::install::{self, Wanted};
use crate::name;
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};
use crate::runner::Runner;
use crate::runner_alias;
use crate::runners::Runners;
use crate::runtime::Runtime;

/// The two layouts wineo can import. A Proton import includes its complete
/// compatdata directory, while a Wine import is the prefix directory itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportLayout {
    Wine,
    Proton,
}

/// A link or other path outside an imported prefix that remains external.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalDependency {
    pub source: PathBuf,
    pub target: PathBuf,
}

/// A deterministic snapshot of a source tree, used to notice changes between
/// review and copy. It comes in two kinds that never compare equal.
///
/// Imports (`inspect`, `inspect_with`, `execute_reviewed`, Heroic planning,
/// and runner copies) hash metadata only: path, kind, size, mode, link count,
/// mtime, ctime, inode, and link target. A source can be a whole library, and
/// reading every file made a large import look stuck. ctime still catches a
/// same-size rewrite.
///
/// Snapshot, clone, and portable hash regular-file content plus metadata
/// without inode or ctime. A restore renames the old prefix aside, which
/// changes its ctime, and rollback must still recognize it. Restore markers
/// also persist this fingerprint, so its hash must not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFingerprint {
    hash: u64,
    pub(crate) entries: u64,
}

/// Read-only information shown before an import is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inspection {
    pub source_root: PathBuf,
    pub wine_prefix: PathBuf,
    pub layout: ImportLayout,
    pub arch: Arch,
    pub external_dependencies: Vec<ExternalDependency>,
    pub fingerprint: SourceFingerprint,
}

/// Generic prefix import request. `destination` is the compatdata parent for
/// Proton prefixes, or the prefix itself for Wine prefixes.
#[derive(Debug, Clone)]
pub struct ImportRequest {
    pub source: PathBuf,
    pub name: String,
    pub runner: String,
    pub destination: Option<PathBuf>,
    pub adopt: bool,
    pub arch: Option<Arch>,
}

/// Settings a Heroic or another importer can register with the prefix in one
/// config write. An empty settings value registers only the prefix.
#[derive(Debug, Clone, Default)]
pub struct ImportedSettings {
    pub targets: BTreeMap<String, Target>,
    pub env: BTreeMap<String, String>,
    pub gamemode: Option<bool>,
    pub wayland: Option<bool>,
    pub mangohud: Option<bool>,
    pub gamescope: Option<GamescopeSetting>,
    pub gameid: Option<String>,
    pub store: Option<String>,
    pub dxvk: Option<String>,
    pub vkd3d: Option<String>,
    /// The Heroic game id, recorded only by a Heroic import.
    pub heroic_id: Option<String>,
}

/// Events emitted while checking and copying a prefix or runner tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportProgress {
    /// Entries walked so far by the current source check. Each check starts
    /// counting from zero again.
    Checking {
        entries: u64,
    },
    Started {
        entries: u64,
    },
    Copied {
        path: PathBuf,
    },
    Finished {
        entries: u64,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("import cancelled")]
pub struct Cancelled;

/// A record written beside a copied runner. Download receipts can use this
/// same small JSON envelope and add provider-specific fields later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerCopyReceipt {
    pub path: PathBuf,
    pub name: String,
    pub receipt: PathBuf,
    pub provenance: RunnerProvenance,
    pub external_dependencies: Vec<ExternalDependency>,
}

/// A host-accessible runner tree that can be offered for copying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerCopyInspection {
    pub source: PathBuf,
    pub supports_wine: bool,
    pub supports_proton: bool,
    pub external_dependencies: Vec<ExternalDependency>,
    pub fingerprint: SourceFingerprint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerProvenance {
    pub provider: String,
    pub source: PathBuf,
    pub method: String,
    pub version: String,
    pub architecture: String,
    pub variant: String,
    pub stable: bool,
}

/// Inspect a source prefix without creating files or running Wine.
pub fn inspect(
    source: impl AsRef<Path>,
    runner: Option<&Runner>,
    arch: Option<Arch>,
) -> Result<Inspection> {
    inspect_with(source, runner, arch, &Token::new(), |_| {})
}

/// `inspect` that reports `ImportProgress::Checking` while it walks the
/// source and stops with `Cancelled` when `cancel` is set.
pub fn inspect_with(
    source: impl AsRef<Path>,
    runner: Option<&Runner>,
    arch: Option<Arch>,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<Inspection> {
    let mut check = Check::new(cancel, &mut progress);
    inspect_impl(
        source.as_ref(),
        runner,
        arch,
        true,
        FingerprintKind::Metadata,
        &mut check,
    )
}

pub(crate) fn inspect_root(
    source: &Path,
    runner: Option<&Runner>,
    arch: Arch,
) -> Result<Inspection> {
    inspect_root_with(source, runner, arch, &Token::new(), |_| {})
}

pub(crate) fn inspect_root_with(
    source: &Path,
    runner: Option<&Runner>,
    arch: Arch,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<Inspection> {
    check_cancelled(cancel)?;
    let mut check = Check::new(cancel, &mut progress);
    inspect_impl(
        source,
        runner,
        Some(arch),
        false,
        FingerprintKind::Content,
        &mut check,
    )
}

/// Whether a directory directly under `root` is itself a prefix. Heroic's
/// default prefix can hold every game's prefix, and importing it would copy
/// them all. Only immediate children are read so this stays cheap.
pub(crate) fn holds_other_prefixes(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        if name == "pfx" || name == "drive_c" || !entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            return false;
        }
        let child = entry.path();
        child.join("system.reg").is_file() || child.join("pfx/system.reg").is_file()
    })
}

pub(crate) fn root_layout_ambiguous(source: &Path) -> bool {
    is_prefix_shape(source) && is_prefix_shape(&source.join("pfx"))
}

fn inspect_impl(
    source: &Path,
    runner: Option<&Runner>,
    arch: Option<Arch>,
    normalize: bool,
    kind: FingerprintKind,
    check: &mut Check<'_>,
) -> Result<Inspection> {
    let source = canonical_source(source)?;
    let source = if normalize {
        normalize_source(source, runner)
    } else {
        source
    };
    let (source_root, wine_prefix, layout) = resolve_layout(&source, runner)?;
    if holds_other_prefixes(&source_root) {
        bail!(
            "{} holds other prefixes; choose the game's own prefix",
            source_root.display()
        );
    }
    validate_prefix(&wine_prefix)?;

    let detected = registry_arch(&wine_prefix)?;
    let detected_arch = detected.or(arch).ok_or_else(|| {
        anyhow!(
            "{} has no #arch marker; pass --arch win32 or --arch win64",
            wine_prefix.display()
        )
    })?;
    if layout == ImportLayout::Proton && detected_arch == Arch::Win32 {
        bail!("Proton prefixes must be win64: {}", wine_prefix.display());
    }
    if let Some(requested) = arch
        && let Some(actual) = detected
        && requested != actual
    {
        bail!(
            "prefix {} is {actual}, not {requested}",
            wine_prefix.display()
        );
    }

    let (external_dependencies, fingerprint) = scan(&source_root, kind, check)?;
    Ok(Inspection {
        source_root,
        wine_prefix,
        layout,
        arch: detected_arch,
        external_dependencies,
        fingerprint,
    })
}

/// Execute an import after inspecting its source. This convenience form
/// registers only the prefix and is equivalent to `execute_reviewed` with no
/// imported settings.
pub fn execute(
    paths: &Paths,
    global: &GlobalConfig,
    request: ImportRequest,
    cancel: &Token,
    progress: impl FnMut(ImportProgress),
) -> Result<Environment> {
    execute_reviewed(paths, global, request, None, None, cancel, progress)
}

/// Execute a reviewed import. If `reviewed` is supplied, its source and
/// fingerprint must still match when locks are held and immediately before
/// publication.
pub fn execute_reviewed(
    paths: &Paths,
    global: &GlobalConfig,
    request: ImportRequest,
    reviewed: Option<&Inspection>,
    settings: Option<&ImportedSettings>,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<Environment> {
    if !name::is_valid(&request.name) {
        bail!("invalid environment name `{}`", request.name);
    }
    if request.adopt && request.destination.is_some() {
        bail!("--adopt cannot be combined with --dir");
    }

    let (runner, _runner_guard) = Runners::new(paths, global)
        .map_err(|err| anyhow!(err))?
        .resolve_locked(paths, &request.runner)
        .map_err(|err| anyhow!(err))?;
    let first = inspect_with(
        &request.source,
        Some(&runner),
        request.arch,
        cancel,
        &mut progress,
    )?;
    if let Some(reviewed) = reviewed {
        ensure_same_review(reviewed, &first)?;
    }
    let source_root = first.source_root.clone();
    let destination = if request.adopt {
        source_root.clone()
    } else {
        match request.destination {
            Some(destination) => paths::resolve(&destination),
            None => global
                .prefix_root(paths)
                .map_err(|err| anyhow!(err))?
                .join(&request.name),
        }
    };
    paths.check_prefix(&destination)?;
    if !request.adopt {
        reject_overlapping_paths(&source_root, &destination)?;
    }
    if !request.adopt && destination.exists() {
        bail!(
            "import destination already exists: {}",
            destination.display()
        );
    }
    if Environment::ensure_exists(paths, &request.name).is_ok() {
        return Err(Error::EnvironmentExists(request.name.clone()).into());
    }
    check_shared_configs(paths, &request.name, [&source_root, &destination])?;

    let lock = Locks::new(paths);
    let mut operation = lock
        .acquire(
            Operation::Create,
            &request.name,
            &[&source_root, &destination],
        )
        .map_err(|err| anyhow!(err))?;
    let locked = inspect_with(
        &request.source,
        Some(&runner),
        request.arch,
        cancel,
        &mut progress,
    )?;
    ensure_same_review(&first, &locked)?;
    if let Some(reviewed) = reviewed {
        ensure_same_review(reviewed, &locked)?;
    }

    // A source may be shared by idle environments. Their locks are held by
    // the prefix lock above, so checking their runtime state here catches a
    // lost run or unresolved operation without refusing an idle shared prefix.
    let shared = shared_names(paths, &request.name, [&source_root, &destination])?;
    for shared in shared {
        Runtime::new(paths, &shared)
            .check_quiet(&shared)
            .map_err(|err| anyhow!(err))?;
    }
    Runtime::new(paths, &request.name)
        .check_quiet(&request.name)
        .map_err(|err| anyhow!(err))?;

    // Validate Heroic settings before copying so a bad target or environment
    // variable cannot leave a published prefix with no registration.
    let prefix_value = paths
        .store_prefix(&destination)
        .map_err(|err| anyhow!(err))?;
    if paths.portable_root().is_some()
        && settings.is_some_and(|settings| settings.targets.values().any(|target| target.desktop))
    {
        return Err(anyhow!(crate::Error::PortableDesktop));
    }
    let settings = settings
        .map(|settings| stored_settings(paths, settings))
        .transpose()?;
    let config_text = config_text(
        &prefix_value,
        &request.runner,
        locked.arch,
        settings.as_ref(),
    )?;

    let mut published = false;
    if !request.adopt {
        if destination.exists() {
            bail!(
                "import destination appeared during review: {}",
                destination.display()
            );
        }
        let parent = destination
            .parent()
            .ok_or_else(|| anyhow!("destination has no parent: {}", destination.display()))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let staging = tempfile::Builder::new()
            .prefix(".wineo-import-")
            .tempdir_in(parent)
            .with_context(|| {
                format!("failed to create staging directory in {}", parent.display())
            })?;
        let staging_path = staging.path().to_owned();
        let _publish_lock = InstallLock::acquire(&destination)?;
        copy_tree_with(
            &source_root,
            &staging_path,
            &destination,
            &Links::for_paths(paths),
            cancel,
            &mut progress,
        )?;
        check_cancelled(cancel)?;
        let after = import_fingerprint(&source_root, &mut Check::new(cancel, &mut progress))?;
        if after != locked.fingerprint {
            bail!(
                "{} changed while it was being copied; try again",
                source_root.display()
            );
        }
        validate_staged_prefix(&staging_path, &destination, locked.layout)?;
        if destination.exists() {
            bail!(
                "import destination appeared during publication: {}",
                destination.display()
            );
        }
        publish(&staging_path, &destination)?;
        published = true;
        progress(ImportProgress::Finished {
            entries: locked.fingerprint.entries,
        });
    } else {
        check_cancelled(cancel)?;
        let after = import_fingerprint(&source_root, &mut Check::new(cancel, &mut progress))?;
        if after != locked.fingerprint {
            bail!(
                "{} changed while it was being reviewed; try again",
                source_root.display()
            );
        }
    }

    // Config writes happen after the prefix is complete. Keep the operation
    // lock while rechecking the name and writing the atomically-created file.
    let config_path = paths.environment_config(&request.name);
    let _config_lock = lock.config(&config_path).map_err(|err| {
        if published {
            anyhow!(
                "prefix {} is ready but registration could not lock {}: {err}; retry this import with --adopt",
                destination.display(),
                config_path.display()
            )
        } else {
            anyhow!(err)
        }
    })?;
    if config_path.exists() {
        if published {
            bail!(
                "prefix {} is ready, but config {} appeared after publication; retry this import with --adopt",
                destination.display(),
                config_path.display(),
            );
        }
        return Err(Error::EnvironmentExists(request.name.clone()).into());
    }
    let config =
        EnvironmentConfig::parse(&config_path, &config_text).map_err(|err| anyhow!(err))?;
    let wine = config
        .wine()
        .ok_or_else(|| anyhow!(Error::NoPrefix(request.name.clone())))?;
    let _build_guards = build_lock::config(
        paths,
        global,
        Some(&wine.runner),
        wine.dxvk.as_deref(),
        wine.vkd3d.as_deref(),
    )?;
    if let Err(err) = install::check_available(
        paths,
        global,
        &request.name,
        &runner,
        &Wanted::from_config(&config),
    ) {
        bail!(
            "prefix {} is ready, but registration failed: {err}\nThen retry this import with --adopt.",
            destination.display(),
        );
    }
    if let Err(err) = write_config_once(&config_path, &config_text) {
        bail!(
            "prefix {} is ready but registration failed: {err}; retry this import with --adopt",
            destination.display(),
        );
    }
    if let Err(err) = operation.relabel(Operation::Configure) {
        bail!(
            "prefix {} and config {} are ready, but registration could not finish: {err}",
            destination.display(),
            config_path.display()
        );
    }
    Environment::load(paths, &request.name).map_err(|err| {
        anyhow!(
            "prefix {} and config {} are ready, but the config could not be reloaded: {err}",
            destination.display(),
            config_path.display()
        )
    })
}

pub(crate) fn ensure_same_review(expected: &Inspection, actual: &Inspection) -> Result<()> {
    if expected.source_root != actual.source_root
        || expected.layout != actual.layout
        || expected.fingerprint != actual.fingerprint
    {
        bail!("the reviewed prefix changed; inspect it again before importing");
    }
    Ok(())
}

fn check_shared_configs(paths: &Paths, name: &str, prefixes: [&Path; 2]) -> Result<()> {
    shared_names(paths, name, prefixes).map(|_| ())
}

fn shared_names(paths: &Paths, name: &str, prefixes: [&Path; 2]) -> Result<Vec<String>> {
    let mut resolved = Vec::new();
    for prefix in prefixes {
        let prefix = paths::resolve(prefix);
        if !resolved.contains(&prefix) {
            resolved.push(prefix);
        }
    }
    let mut names = Vec::new();
    for prefix in resolved {
        let sharing =
            crate::environment::sharing_prefix(paths, name, &prefix).map_err(|err| anyhow!(err))?;
        if !sharing.unreadable.is_empty() {
            bail!(
                "could not inspect existing environments: {}",
                sharing.unreadable.join(", ")
            );
        }
        for shared in sharing.names {
            if !names.contains(&shared) {
                names.push(shared);
            }
        }
    }
    Ok(names)
}

fn resolve_layout(
    source: &Path,
    runner: Option<&Runner>,
) -> Result<(PathBuf, PathBuf, ImportLayout)> {
    let direct = is_prefix_shape(source);
    let nested = is_prefix_shape(&source.join("pfx"));
    let layout = match (direct, nested, runner.map(Runner::is_proton)) {
        (true, true, None) => bail!(
            "{} contains both a Wine prefix and pfx/; select a runner/layout",
            source.display()
        ),
        (true, true, Some(true)) => ImportLayout::Proton,
        (true, true, Some(false)) => ImportLayout::Wine,
        (true, false, Some(true)) => {
            bail!("selected Proton runner requires {}/pfx", source.display())
        }
        (true, false, _) => ImportLayout::Wine,
        (false, true, Some(false)) => bail!(
            "selected Wine runner requires a direct prefix, not {}/pfx",
            source.display()
        ),
        (false, true, _) => ImportLayout::Proton,
        (false, false, _) => bail!(
            "{} is not a Wine prefix or Proton compatdata directory",
            source.display()
        ),
    };
    let root = source.to_owned();
    let wine_prefix = prefix_for_layout(&root, layout);
    if let Some(runner) = runner
        && runner.is_proton() != (layout == ImportLayout::Proton)
    {
        bail!(
            "prefix layout does not match selected {} runner",
            runner.kind_name()
        );
    }
    Ok((root, wine_prefix, layout))
}

fn prefix_for_layout(root: &Path, layout: ImportLayout) -> PathBuf {
    match layout {
        ImportLayout::Wine => root.to_owned(),
        ImportLayout::Proton => root.join("pfx"),
    }
}

fn canonical_source(source: &Path) -> Result<PathBuf> {
    let source = fs::canonicalize(source)
        .with_context(|| format!("failed to resolve import source {}", source.display()))?;
    if !source.is_dir() {
        bail!("import source is not a directory: {}", source.display());
    }
    Ok(source)
}

fn normalize_source(source: PathBuf, runner: Option<&Runner>) -> PathBuf {
    let is_pfx = source.file_name().and_then(|name| name.to_str()) == Some("pfx");
    if is_pfx
        && is_prefix_shape(&source)
        && source.parent().is_some_and(|parent| {
            parent.join("pfx") == source && runner.map(Runner::is_proton).unwrap_or(true)
        })
    {
        source.parent().unwrap().to_owned()
    } else {
        source
    }
}

fn is_prefix_shape(dir: &Path) -> bool {
    dir.join("system.reg").is_file() && dir.join("drive_c").is_dir()
}

fn validate_prefix(dir: &Path) -> Result<()> {
    if !dir.join("system.reg").is_file() {
        bail!("prefix {} is missing readable system.reg", dir.display());
    }
    if !dir.join("drive_c").is_dir() {
        bail!("prefix {} is missing drive_c", dir.display());
    }
    File::open(dir.join("system.reg"))
        .with_context(|| format!("failed to read {}/system.reg", dir.display()))?;
    Ok(())
}

fn registry_arch(dir: &Path) -> Result<Option<Arch>> {
    let path = dir.join("system.reg");
    let file = File::open(&path).with_context(|| format!("failed to read {}", path.display()))?;
    for line in BufReader::new(file).lines().take(10) {
        let line = line.with_context(|| format!("failed to read {}", path.display()))?;
        if let Some(value) = line.strip_prefix("#arch=") {
            return value.trim().parse().map(Some).map_err(|message: String| {
                anyhow!(
                    "invalid architecture marker in {}: {message}",
                    path.display()
                )
            });
        }
    }
    Ok(None)
}

pub(crate) fn validate_staged_prefix(
    staging: &Path,
    final_root: &Path,
    layout: ImportLayout,
) -> Result<()> {
    let root = prefix_for_layout(staging, layout);
    let system = staged_path(&root.join("system.reg"), staging, final_root)?;
    let drive_c = staged_path(&root.join("drive_c"), staging, final_root)?;
    if !system.is_file() || !drive_c.is_dir() {
        bail!("staged import is missing system.reg or drive_c");
    }
    File::open(&system).with_context(|| format!("failed to read {}", system.display()))?;
    Ok(())
}

pub(crate) fn reject_overlapping_paths(source: &Path, destination: &Path) -> Result<()> {
    let source = paths::resolve(source);
    let destination = paths::resolve(destination);
    if source == destination || source.starts_with(&destination) || destination.starts_with(&source)
    {
        bail!(
            "import source {} overlaps destination {}",
            source.display(),
            destination.display()
        );
    }
    Ok(())
}

fn external_dependency(
    root: &Path,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<Option<ExternalDependency>> {
    if !metadata.file_type().is_symlink() {
        return Ok(None);
    }
    let target =
        fs::read_link(path).with_context(|| format!("failed to read link {}", path.display()))?;
    let resolved = resolve_link(path, &target);
    Ok((!resolved.starts_with(root)).then(|| ExternalDependency {
        source: path.to_owned(),
        target: resolved,
    }))
}

const CHECK_INTERVAL: u64 = 1000;

/// Counts entries during a pre-copy walk so a large source reports progress
/// and can be canceled instead of looking stuck.
struct Check<'a> {
    cancel: &'a Token,
    progress: &'a mut dyn FnMut(ImportProgress),
    entries: u64,
}

impl<'a> Check<'a> {
    fn new(cancel: &'a Token, progress: &'a mut dyn FnMut(ImportProgress)) -> Self {
        Self {
            cancel,
            progress,
            entries: 0,
        }
    }

    fn tick(&mut self) -> Result<()> {
        check_cancelled(self.cancel)?;
        self.entries += 1;
        if self.entries.is_multiple_of(CHECK_INTERVAL) {
            self.report()?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.report()
    }

    // A cancel can arrive during the callback itself, and the last one of a
    // walk has no later entry to notice it before the caller publishes.
    fn report(&mut self) -> Result<()> {
        (self.progress)(ImportProgress::Checking {
            entries: self.entries,
        });
        check_cancelled(self.cancel)
    }
}

/// Which fingerprint a caller compares; see `SourceFingerprint`.
#[derive(Debug, Clone, Copy)]
enum FingerprintKind {
    Content,
    Metadata,
}

/// External links and the fingerprint of `root`, found in one walk.
fn scan(
    root: &Path,
    kind: FingerprintKind,
    check: &mut Check<'_>,
) -> Result<(Vec<ExternalDependency>, SourceFingerprint)> {
    let mut found = Vec::new();
    let fingerprint = fingerprint_walk(root, kind, check, Some(&mut found))?;
    found.sort_by(|a, b| a.source.cmp(&b.source));
    Ok((found, fingerprint))
}

/// The content fingerprint snapshot, clone, and portable compare against.
pub(crate) fn fingerprint(root: &Path) -> Result<SourceFingerprint> {
    fingerprint_with(root, &Token::new(), |_| {})
}

pub(crate) fn fingerprint_with(
    root: &Path,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<SourceFingerprint> {
    check_cancelled(cancel)?;
    fingerprint_walk(
        root,
        FingerprintKind::Content,
        &mut Check::new(cancel, &mut progress),
        None,
    )
}

fn import_fingerprint(root: &Path, check: &mut Check<'_>) -> Result<SourceFingerprint> {
    fingerprint_walk(root, FingerprintKind::Metadata, check, None)
}

fn fingerprint_walk(
    root: &Path,
    kind: FingerprintKind,
    check: &mut Check<'_>,
    external: Option<&mut Vec<ExternalDependency>>,
) -> Result<SourceFingerprint> {
    match kind {
        FingerprintKind::Content => {
            let cancel = check.cancel;
            hash_records(root, check, external, |relative, path, metadata| {
                content_record(relative, path, metadata, cancel)
            })
        }
        FingerprintKind::Metadata => hash_records(root, check, external, metadata_record),
    }
}

type ContentRecord = (
    String,
    u8,
    u64,
    u32,
    u64,
    Option<(u64, u32)>,
    Option<u64>,
    Option<String>,
);

// This tuple must hash exactly as before, since restore markers persist it.
fn content_record(
    relative: String,
    path: &Path,
    metadata: &fs::Metadata,
    cancel: &Token,
) -> Result<ContentRecord> {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| (duration.as_secs(), duration.subsec_nanos()));
    let content = if metadata.file_type().is_file() {
        Some(file_digest(path, cancel)?)
    } else {
        None
    };
    Ok((
        relative,
        file_kind(metadata),
        metadata.len(),
        metadata.mode(),
        metadata.nlink(),
        modified,
        content,
        link_target(path, metadata)?,
    ))
}

type MetadataRecord = (
    String,
    u8,
    u64,
    u32,
    u64,
    (i64, i64),
    (i64, i64),
    u64,
    Option<String>,
);

// ctime moves on every write, so a same-size rewrite still changes this.
fn metadata_record(
    relative: String,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<MetadataRecord> {
    Ok((
        relative,
        file_kind(metadata),
        metadata.len(),
        metadata.mode(),
        metadata.nlink(),
        (metadata.mtime(), metadata.mtime_nsec()),
        (metadata.ctime(), metadata.ctime_nsec()),
        metadata.ino(),
        link_target(path, metadata)?,
    ))
}

fn link_target(path: &Path, metadata: &fs::Metadata) -> Result<Option<String>> {
    if !metadata.file_type().is_symlink() {
        return Ok(None);
    }
    let target =
        fs::read_link(path).with_context(|| format!("failed to read link {}", path.display()))?;
    Ok(Some(target.to_string_lossy().into_owned()))
}

fn hash_records<R: Hash>(
    root: &Path,
    check: &mut Check<'_>,
    mut external: Option<&mut Vec<ExternalDependency>>,
    record: impl Fn(String, &Path, &fs::Metadata) -> Result<R>,
) -> Result<SourceFingerprint> {
    let mut records: Vec<(String, R)> = Vec::new();
    let mut entries = 0;
    walk_no_follow(root, &mut |path, metadata| {
        check.tick()?;
        if let Some(found) = external.as_deref_mut()
            && let Some(dependency) = external_dependency(root, path, metadata)?
        {
            found.push(dependency);
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        records.push((relative.clone(), record(relative, path, metadata)?));
        entries += 1;
        Ok(())
    })?;
    check.finish()?;
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let records: Vec<R> = records.into_iter().map(|(_, record)| record).collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    records.hash(&mut hasher);
    Ok(SourceFingerprint {
        hash: hasher.finish(),
        entries,
    })
}

fn file_digest(path: &Path, cancel: &Token) -> Result<u64> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to read {} for fingerprinting", path.display()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut buffer = [0; 128 * 1024];
    loop {
        check_cancelled(cancel)?;
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read {} for fingerprinting", path.display()))?;
        if count == 0 {
            break;
        }
        buffer[..count].hash(&mut hasher);
    }
    Ok(hasher.finish())
}

fn file_kind(metadata: &fs::Metadata) -> u8 {
    let kind = metadata.file_type();
    if kind.is_dir() {
        1
    } else if kind.is_file() {
        2
    } else if kind.is_symlink() {
        3
    } else {
        4
    }
}

fn walk_no_follow(
    root: &Path,
    callback: &mut impl FnMut(&Path, &fs::Metadata) -> Result<()>,
) -> Result<()> {
    let metadata = fs::symlink_metadata(root)
        .with_context(|| format!("failed to inspect {}", root.display()))?;
    callback(root, &metadata)?;
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut children: Vec<PathBuf> = fs::read_dir(root)
        .with_context(|| format!("failed to read {}", root.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<_>>()
        .with_context(|| format!("failed to read {}", root.display()))?;
    children.sort();
    for child in children {
        walk_no_follow(&child, callback)?;
    }
    Ok(())
}

pub(crate) fn copy_tree(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<()> {
    let links = Links::default();
    copy_tree_with_reflink_mode(
        source,
        destination,
        final_root,
        &links,
        cancel,
        progress,
        false,
    )
}

/// How a copy rewrites links that point inside what is being copied.
#[derive(Debug, Default)]
pub(crate) struct Links {
    /// Other trees copied alongside, as `(from, to)`. A link into one follows
    /// it the way a link into the copied tree itself does.
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// Rewritten links are written relative to where they sit, so a portable
    /// folder can move. Otherwise they are absolute.
    pub relative: bool,
}

impl Links {
    /// Relative links when the copy lands in a portable folder.
    pub(crate) fn for_paths(paths: &Paths) -> Self {
        Self {
            moved: Vec::new(),
            relative: paths.portable_root().is_some(),
        }
    }
}

pub(crate) fn copy_tree_with(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    links: &Links,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<()> {
    copy_tree_with_reflink_mode(
        source,
        destination,
        final_root,
        links,
        cancel,
        progress,
        false,
    )
}

/// Links under `root` that point outside it and outside every tree in `moved`.
pub(crate) fn external_links_with(
    root: &Path,
    moved: &[&Path],
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<Vec<ExternalDependency>> {
    let mut check = Check::new(cancel, progress);
    let mut found = Vec::new();
    walk_no_follow(root, &mut |path, metadata| {
        check.tick()?;
        if let Some(link) = external_dependency(root, path, metadata)?
            && !moved.iter().any(|tree| link.target.starts_with(tree))
        {
            found.push(link);
        }
        Ok(())
    })?;
    check.finish()?;
    found.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(found)
}

#[cfg(test)]
pub(crate) fn copy_tree_strict_reflink(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<()> {
    let links = Links::default();
    copy_tree_with_reflink_mode(
        source,
        destination,
        final_root,
        &links,
        cancel,
        progress,
        true,
    )
}

/// `copy_tree_strict_reflink` with the link handling `links` asks for.
pub(crate) fn copy_tree_strict_reflink_with(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    links: &Links,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<()> {
    copy_tree_with_reflink_mode(
        source,
        destination,
        final_root,
        links,
        cancel,
        progress,
        true,
    )
}

fn copy_tree_with_reflink_mode(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    links: &Links,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
    strict_reflink: bool,
) -> Result<()> {
    // Only the count is needed here, so skip reading file content.
    let mut total = 0;
    walk_no_follow(source, &mut |_, _| {
        check_cancelled(cancel)?;
        total += 1;
        Ok(())
    })?;
    progress(ImportProgress::Started { entries: total });
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    let mut hardlinks = HashMap::new();
    let options = CopyOptions {
        source_root: source,
        final_root,
        links,
        cancel,
        strict_reflink,
    };
    copy_entry(source, destination, &options, progress, &mut hardlinks)?;
    Ok(())
}

struct CopyOptions<'a> {
    source_root: &'a Path,
    final_root: &'a Path,
    links: &'a Links,
    cancel: &'a Token,
    strict_reflink: bool,
}

fn copy_entry(
    source: &Path,
    destination: &Path,
    options: &CopyOptions<'_>,
    progress: &mut impl FnMut(ImportProgress),
    hardlinks: &mut HashMap<(u64, u64), PathBuf>,
) -> Result<()> {
    check_cancelled(options.cancel)?;
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect {}", source.display()))?;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        let target = fs::read_link(source)
            .with_context(|| format!("failed to read link {}", source.display()))?;
        let resolved = resolve_link(source, &target);
        let rewrite = |to: PathBuf| {
            if !options.links.relative {
                return to;
            }
            // Relative to where the link ends up, not where it is staged.
            let inside = source
                .strip_prefix(options.source_root)
                .unwrap_or(Path::new(""));
            let placed = options.final_root.join(inside);
            relative_path(&to, placed.parent().unwrap_or(Path::new("/")))
        };
        let target = if resolved.starts_with(options.source_root) {
            if target.is_absolute() {
                let relative = resolved
                    .strip_prefix(options.source_root)
                    .unwrap_or(Path::new(""));
                rewrite(lexical_normalize(&options.final_root.join(relative)))
            } else {
                target
            }
        } else if let Some((from, to)) = options
            .links
            .moved
            .iter()
            .find(|(from, _)| resolved.starts_with(from))
        {
            let relative = resolved.strip_prefix(from).unwrap_or(Path::new(""));
            rewrite(lexical_normalize(&to.join(relative)))
        } else {
            // An external link would point somewhere else after copying when
            // it is relative, or through an internal link that is re-rooted.
            // Keep the resolved external data location explicit.
            resolved
        };
        std::os::unix::fs::symlink(&target, destination)
            .with_context(|| format!("failed to copy link {}", source.display()))?;
    } else if kind.is_dir() {
        if source != options.source_root {
            fs::create_dir(destination)
                .with_context(|| format!("failed to create {}", destination.display()))?;
        }
        let mut children: Vec<PathBuf> = fs::read_dir(source)
            .with_context(|| format!("failed to read {}", source.display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<_>>()
            .with_context(|| format!("failed to read {}", source.display()))?;
        children.sort();
        for child in children {
            let name = child
                .file_name()
                .ok_or_else(|| anyhow!("path has no file name: {}", child.display()))?;
            copy_entry(
                &child,
                &destination.join(name),
                options,
                progress,
                hardlinks,
            )?;
        }
        copy_metadata(&metadata, destination)?;
    } else if kind.is_file() {
        let key = (metadata.dev(), metadata.ino());
        if let Some(first) = hardlinks.get(&key) {
            fs::hard_link(first, destination).with_context(|| {
                format!("failed to preserve hardlink {}", destination.display())
            })?;
        } else {
            if options.strict_reflink {
                copy_regular_file_strict(source, destination)?;
            } else {
                copy_regular_file(source, destination)?;
            }
            hardlinks.insert(key, destination.to_owned());
        }
        copy_metadata(&metadata, destination)?;
    } else {
        bail!("unsupported special file in prefix: {}", source.display());
    }
    progress(ImportProgress::Copied {
        path: source.to_owned(),
    });
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path) -> Result<()> {
    copy_regular_file_with(source, destination, |output, input| {
        ioctl_ficlone(output, input)
    })
}

fn copy_regular_file_with(
    source: &Path,
    destination: &Path,
    reflink: impl FnOnce(&mut File, &mut File) -> rustix::io::Result<()>,
) -> Result<()> {
    copy_regular_file_with_mode(source, destination, reflink, false)
}

fn copy_regular_file_strict(source: &Path, destination: &Path) -> Result<()> {
    copy_regular_file_with_mode(
        source,
        destination,
        |output, input| ioctl_ficlone(output, input),
        true,
    )
}

fn copy_regular_file_with_mode(
    source: &Path,
    destination: &Path,
    reflink: impl FnOnce(&mut File, &mut File) -> rustix::io::Result<()>,
    strict_reflink: bool,
) -> Result<()> {
    let mut input =
        File::open(source).with_context(|| format!("failed to open {}", source.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    match reflink(&mut output, &mut input) {
        Ok(()) => Ok(()),
        Err(err)
            if strict_reflink
                && matches!(
                    err,
                    Errno::OPNOTSUPP | Errno::NOTTY | Errno::XDEV | Errno::INVAL | Errno::NOSYS
                ) =>
        {
            Err(anyhow!(err)).with_context(|| {
                format!(
                    "filesystem does not support reflinks from {} to {}",
                    source.display(),
                    destination.display()
                )
            })
        }
        Err(Errno::OPNOTSUPP | Errno::NOTTY | Errno::XDEV | Errno::INVAL | Errno::NOSYS) => {
            fs::copy(source, destination)
                .with_context(|| format!("failed to copy {}", source.display()))?;
            Ok(())
        }
        Err(err) => Err(anyhow!(err)).with_context(|| {
            format!(
                "failed to reflink {} to {}",
                source.display(),
                destination.display()
            )
        }),
    }
}

fn copy_metadata(source: &fs::Metadata, destination: &Path) -> Result<()> {
    fs::set_permissions(destination, fs::Permissions::from_mode(source.mode()))
        .with_context(|| format!("failed to preserve mode on {}", destination.display()))?;
    let modified = filetime::FileTime::from_last_modification_time(source);
    let accessed = filetime::FileTime::from_last_access_time(source);
    filetime::set_file_times(destination, accessed, modified)
        .with_context(|| format!("failed to preserve timestamps on {}", destination.display()))?;
    Ok(())
}

fn resolve_link(source: &Path, target: &Path) -> PathBuf {
    let base = if target.is_absolute() {
        target.to_owned()
    } else {
        source.parent().unwrap_or(Path::new(".")).join(target)
    };
    let lexical = lexical_normalize(&base);
    fs::canonicalize(&lexical).unwrap_or(lexical)
}

/// `target` as seen from the directory `base`, both absolute and normalized.
fn relative_path(target: &Path, base: &Path) -> PathBuf {
    let target: Vec<Component> = target.components().collect();
    let base: Vec<Component> = base.components().collect();
    let common = target
        .iter()
        .zip(&base)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..base.len() {
        relative.push("..");
    }
    for component in &target[common..] {
        relative.push(component.as_os_str());
    }
    if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            Component::RootDir | Component::Prefix(_) => result.push(component.as_os_str()),
            Component::Normal(part) => result.push(part),
        }
    }
    result
}

pub(crate) fn check_cancelled(cancel: &Token) -> Result<()> {
    if cancel.is_canceled() {
        return Err(Cancelled.into());
    }
    Ok(())
}

/// Target paths in the config form `paths` stores them in.
fn stored_settings(paths: &Paths, settings: &ImportedSettings) -> Result<ImportedSettings> {
    let mut stored = settings.clone();
    for target in stored.targets.values_mut() {
        target.exe = paths.store_value(&target.exe)?;
        if let Some(cwd) = &mut target.cwd {
            *cwd = paths.store_value(cwd)?;
        }
    }
    Ok(stored)
}

fn config_text(
    prefix: &str,
    runner: &str,
    arch: Arch,
    settings: Option<&ImportedSettings>,
) -> Result<String> {
    let mut doc = crate::config::new_document();
    doc["prefix"] = value(prefix);
    doc["runner"] = value(runner);
    doc["arch"] = value(arch.as_str());
    finish_config(doc, settings)
}

/// A native environment's config, with nothing about a prefix or runner.
fn native_config_text(settings: &ImportedSettings) -> Result<String> {
    let mut doc = crate::config::new_document();
    doc["kind"] = value("native");
    finish_config(doc, Some(settings))
}

fn finish_config(mut doc: DocumentMut, settings: Option<&ImportedSettings>) -> Result<String> {
    if let Some(settings) = settings {
        for (key, env_value) in &settings.env {
            validate_env_var(key, env_value).map_err(|message| anyhow!(message))?;
        }
        if let Some(gamemode) = settings.gamemode {
            doc["gamemode"] = Value::from(gamemode).into();
        }
        if let Some(wayland) = settings.wayland {
            doc["wayland"] = value(wayland);
        }
        if let Some(mangohud) = settings.mangohud {
            doc["mangohud"] = value(mangohud);
        }
        if let Some(gamescope) = &settings.gamescope {
            doc["gamescope"] = gamescope.to_item();
        }
        if let Some(gameid) = &settings.gameid {
            doc["gameid"] = value(gameid);
        }
        if let Some(store) = &settings.store {
            doc["store"] = value(store);
        }
        if let Some(dxvk) = &settings.dxvk {
            doc["dxvk"] = value(dxvk);
        }
        if let Some(vkd3d) = &settings.vkd3d {
            doc["vkd3d"] = value(vkd3d);
        }
        if let Some(heroic_id) = &settings.heroic_id {
            doc["heroic_id"] = value(heroic_id);
        }
        if !settings.env.is_empty() {
            let mut table = Table::new();
            for (key, env_value) in &settings.env {
                table[key] = value(env_value);
            }
            doc["env"] = Item::Table(table);
        }
        if !settings.targets.is_empty() {
            let mut targets = Table::new();
            for (name, target) in &settings.targets {
                if !crate::name::is_valid(name) {
                    bail!("invalid target name `{name}`");
                }
                let mut table = Table::new();
                table["exe"] = value(&target.exe);
                if !target.args.is_empty() {
                    let args: Array = target.args.iter().map(String::as_str).collect();
                    table["args"] = Value::Array(args).into();
                }
                if let Some(cwd) = &target.cwd {
                    table["cwd"] = value(cwd);
                }
                if target.default {
                    table["default"] = Value::from(true).into();
                }
                if let Some(gamemode) = target.gamemode {
                    table["gamemode"] = Value::from(gamemode).into();
                }
                if target.desktop {
                    table["desktop"] = Value::from(true).into();
                }
                if let Some(display_name) = &target.name {
                    table["name"] = value(display_name);
                }
                targets[name] = Item::Table(table);
            }
            doc["targets"] = Item::Table(targets);
        }
    }
    let text = doc.to_string();
    let path = Path::new("<import>");
    EnvironmentConfig::parse(path, &text).map_err(|err| anyhow!(err))?;
    Ok(text)
}

/// Registers a native environment from imported settings, which has no
/// prefix to copy or adopt. The config is written once, so an environment
/// that appeared meanwhile is never replaced.
pub fn register_native(
    paths: &Paths,
    name: &str,
    settings: &ImportedSettings,
) -> Result<Environment> {
    if !name::is_valid(name) {
        bail!("invalid environment name `{name}`");
    }
    let config_path = paths.environment_config(name);
    if config_path.exists() {
        return Err(Error::EnvironmentExists(name.to_owned()).into());
    }
    let text = native_config_text(&stored_settings(paths, settings)?)?;
    let locks = Locks::new(paths);
    let _operation = locks
        .acquire(Operation::Create, name, &[])
        .map_err(|err| anyhow!(err))?;
    let _config = locks.config(&config_path).map_err(|err| anyhow!(err))?;
    write_config_once(&config_path, &text)?;
    Environment::load(paths, name).map_err(|err| anyhow!(err))
}

pub(crate) fn write_config_once(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("config has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let staging = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create a config staging file in {}",
            parent.display()
        )
    })?;
    let staging_path = staging.path().to_owned();
    staging.as_file().write_all(contents.as_bytes())?;
    staging.as_file().sync_all()?;
    match fs::hard_link(&staging_path, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            if err.kind() == io::ErrorKind::AlreadyExists {
                bail!("environment config already exists: {}", path.display());
            }
            Err(err).with_context(|| format!("failed to register {}", path.display()))
        }
    }
}

/// Copy an existing runner tree into a wineo runner root without replacing an
/// installed build. The copy is staged and receives an import provenance file.
pub fn inspect_runner_copy(source: impl AsRef<Path>) -> Result<RunnerCopyInspection> {
    let source = canonical_source(source.as_ref())?;
    let (supports_wine, supports_proton) = runner_tree_kinds(&source)?;
    if !supports_wine && !supports_proton {
        bail!(
            "{} is not a valid Wine or Proton runner tree",
            source.display()
        );
    }
    let cancel = Token::new();
    let (external_dependencies, fingerprint) = scan(
        &source,
        FingerprintKind::Metadata,
        &mut Check::new(&cancel, &mut |_| {}),
    )?;
    Ok(RunnerCopyInspection {
        external_dependencies,
        fingerprint,
        source,
        supports_wine,
        supports_proton,
    })
}

pub fn copy_runner(
    source: impl AsRef<Path>,
    destination_root: impl AsRef<Path>,
    name: &str,
    cancel: &Token,
    progress: impl FnMut(ImportProgress),
) -> Result<RunnerCopyReceipt> {
    copy_runner_with_review(
        source.as_ref(),
        destination_root.as_ref(),
        name,
        None,
        cancel,
        progress,
    )
}

/// Copy the same runner tree that was offered in an earlier review.
pub fn copy_runner_reviewed(
    reviewed: &RunnerCopyInspection,
    destination_root: impl AsRef<Path>,
    name: &str,
    cancel: &Token,
    progress: impl FnMut(ImportProgress),
) -> Result<RunnerCopyReceipt> {
    copy_runner_with_review(
        &reviewed.source,
        destination_root.as_ref(),
        name,
        Some(reviewed),
        cancel,
        progress,
    )
}

fn copy_runner_with_review(
    source: &Path,
    destination_root: &Path,
    name: &str,
    reviewed: Option<&RunnerCopyInspection>,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<RunnerCopyReceipt> {
    let inspection = inspect_runner_copy(source)?;
    if reviewed.is_some_and(|reviewed| reviewed != &inspection) {
        bail!("the reviewed runner changed; inspect it again before copying");
    }
    let source = &inspection.source;
    if !name::is_valid_runner(name) {
        bail!("invalid runner name `{name}`");
    }
    if runner_alias::is_alias(name) {
        bail!("runner name `{name}` is reserved for a latest alias");
    }
    let destination_root = std::path::absolute(destination_root)?;
    fs::create_dir_all(&destination_root)
        .with_context(|| format!("failed to create {}", destination_root.display()))?;
    let destination = destination_root.join(name);
    reject_overlapping_paths(source, &destination)?;
    if destination.exists() {
        bail!(
            "runner destination already exists: {}",
            destination.display()
        );
    }
    let before = inspection.fingerprint;
    let _publish_lock = InstallLock::acquire(&destination)?;
    let staging = tempfile::Builder::new()
        .prefix(".wineo-runner-")
        .tempdir_in(&destination_root)
        .with_context(|| {
            format!(
                "failed to create staging directory in {}",
                destination_root.display()
            )
        })?;
    let staging_path = staging.path().to_owned();
    copy_tree(source, &staging_path, &destination, cancel, &mut progress)?;
    check_cancelled(cancel)?;
    if import_fingerprint(source, &mut Check::new(cancel, &mut |_| {}))? != before {
        bail!(
            "runner {} changed while it was being copied; try again",
            source.display()
        );
    }
    validate_staged_runner_tree(&staging_path, &destination)?;
    let receipt = staging_path.join(".wineo-install.json");
    if let Ok(metadata) = fs::symlink_metadata(&receipt) {
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            bail!(
                "runner source has a non-regular reserved receipt: {}",
                receipt.display()
            );
        }
        fs::remove_file(&receipt)
            .with_context(|| format!("failed to replace {}", receipt.display()))?;
    }
    let provenance = runner_provenance(source, name)?;
    let receipt_json = runner_receipt(&provenance)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt)
        .and_then(|mut file| file.write_all(&receipt_json))
        .with_context(|| format!("failed to write {}", receipt.display()))?;
    if destination.exists() {
        bail!(
            "runner destination appeared during publication: {}",
            destination.display()
        );
    }
    publish(&staging_path, &destination)?;
    Ok(RunnerCopyReceipt {
        path: destination.clone(),
        name: name.to_owned(),
        receipt: destination.join(".wineo-install.json"),
        provenance,
        external_dependencies: inspection
            .external_dependencies
            .into_iter()
            .map(|dependency| ExternalDependency {
                source: destination.join(dependency.source.strip_prefix(source).unwrap()),
                target: dependency.target,
            })
            .collect(),
    })
}

fn runner_provider(name: &str) -> String {
    if name.starts_with("GE-Proton") {
        "ge-proton".to_owned()
    } else if name.to_ascii_lowercase().starts_with("proton-cachyos-")
        || name.to_ascii_lowercase().starts_with("cachyos-")
    {
        "proton-cachyos".to_owned()
    } else {
        "import".to_owned()
    }
}

fn runner_provenance(source: &Path, name: &str) -> Result<RunnerProvenance> {
    let reported = fs::read_to_string(source.join("version"))
        .ok()
        .and_then(|text| text.split_whitespace().nth(1).map(str::to_owned));
    let version = reported.unwrap_or_else(|| name.to_owned());
    let provider = runner_provider(&version);
    let (provider, variant) = if provider == "import" {
        (runner_provider(name), "baseline".to_owned())
    } else if provider == "proton-cachyos" {
        (provider, "slr".to_owned())
    } else {
        (provider, "baseline".to_owned())
    };
    Ok(RunnerProvenance {
        stable: stable_runner_version(&version, &provider),
        provider,
        source: source.to_owned(),
        method: "copy".to_owned(),
        version,
        architecture: "x86_64".to_owned(),
        variant,
    })
}

fn stable_runner_version(version: &str, provider: &str) -> bool {
    let parts: Option<Vec<&str>> = if provider == "ge-proton" {
        version
            .strip_prefix("GE-Proton")
            .map(|rest| rest.split('-').collect())
    } else if provider == "proton-cachyos" {
        version
            .strip_prefix("cachyos-")
            .or_else(|| version.strip_prefix("proton-cachyos-"))
            .and_then(|rest| rest.strip_suffix("-slr"))
            .map(|rest| rest.split(['.', '-']).collect())
    } else {
        None
    };
    let Some(parts) = parts else {
        return false;
    };
    let count = if provider == "ge-proton" { 2 } else { 3 };
    parts.len() == count
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn runner_receipt(provenance: &RunnerProvenance) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(&serde_json::json!({
        "provider": provenance.provider,
        "version": provenance.version,
        "architecture": provenance.architecture,
        "variant": provenance.variant,
        "stable": provenance.stable,
        "asset": "",
        "digest": "",
        "verified_digests": [],
        "method": provenance.method,
        "source": provenance.source,
    }))?)
}

fn runner_tree_kinds(root: &Path) -> Result<(bool, bool)> {
    let proton = root.join("proton");
    let wine = root.join("bin/wine");
    let wineserver = root.join("bin/wineserver");
    let proton_ok = proton.is_file()
        && proton.metadata()?.permissions().mode() & 0o111 != 0
        && ["files", "dist"].into_iter().any(|payload| {
            let wine = root.join(payload).join("bin/wine");
            let wineserver = root.join(payload).join("bin/wineserver");
            wine.is_file()
                && wineserver.is_file()
                && wine
                    .metadata()
                    .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
                && wineserver
                    .metadata()
                    .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
        });
    let wine_ok = wine.is_file()
        && wineserver.is_file()
        && wine.metadata()?.permissions().mode() & 0o111 != 0
        && wineserver.metadata()?.permissions().mode() & 0o111 != 0;
    Ok((wine_ok, proton_ok))
}

fn validate_staged_runner_tree(staging: &Path, final_root: &Path) -> Result<()> {
    let proton = staged_path_if_exists(&staging.join("proton"), staging, final_root)?;
    let wine = staged_path_if_exists(&staging.join("bin/wine"), staging, final_root)?;
    let wineserver = staged_path_if_exists(&staging.join("bin/wineserver"), staging, final_root)?;
    let mut proton_payload = false;
    for payload in ["files", "dist"] {
        if staged_executable_if_exists(
            &staging.join(payload).join("bin/wine"),
            staging,
            final_root,
        )? && staged_executable_if_exists(
            &staging.join(payload).join("bin/wineserver"),
            staging,
            final_root,
        )? {
            proton_payload = true;
            break;
        }
    }
    let proton_ok = match proton.as_ref() {
        Some(path) => path.is_file() && executable_metadata(path)?,
        None => false,
    } && proton_payload;
    let wine_ok = match (wine.as_ref(), wineserver.as_ref()) {
        (Some(wine), Some(wineserver)) => {
            wine.is_file()
                && wineserver.is_file()
                && executable_metadata(wine)?
                && executable_metadata(wineserver)?
        }
        _ => false,
    };
    if !proton_ok && !wine_ok {
        bail!(
            "{} is not a valid Wine or Proton runner tree",
            staging.display()
        );
    }
    Ok(())
}

fn staged_path_if_exists(
    path: &Path,
    staging: &Path,
    final_root: &Path,
) -> Result<Option<PathBuf>> {
    resolve_staged_path(path, staging, final_root, true)
}

fn staged_executable_if_exists(path: &Path, staging: &Path, final_root: &Path) -> Result<bool> {
    let Some(path) = staged_path_if_exists(path, staging, final_root)? else {
        return Ok(false);
    };
    Ok(path.is_file() && executable_metadata(&path)?)
}

fn executable_metadata(path: &Path) -> Result<bool> {
    Ok(fs::metadata(path)?.permissions().mode() & 0o111 != 0)
}

fn staged_path(path: &Path, staging: &Path, final_root: &Path) -> Result<PathBuf> {
    resolve_staged_path(path, staging, final_root, false)?.ok_or_else(|| {
        anyhow!(
            "failed to inspect {}: no such file or directory",
            path.display()
        )
    })
}

fn resolve_staged_path(
    path: &Path,
    staging: &Path,
    final_root: &Path,
    allow_missing: bool,
) -> Result<Option<PathBuf>> {
    let mut current = path.to_owned();
    let mut links = 0;
    'resolve: loop {
        let mut resolved = PathBuf::new();
        let mut components = current.components();
        while let Some(component) = components.next() {
            match component {
                Component::Prefix(_) | Component::RootDir | Component::CurDir => {
                    resolved.push(component.as_os_str());
                    continue;
                }
                Component::ParentDir => {
                    resolved.pop();
                    continue;
                }
                Component::Normal(part) => resolved.push(part),
            }
            let metadata = match fs::symlink_metadata(&resolved) {
                Ok(metadata) => metadata,
                Err(err)
                    if allow_missing && links == 0 && err.kind() == io::ErrorKind::NotFound =>
                {
                    return Ok(None);
                }
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("failed to inspect {}", resolved.display()));
                }
            };
            if !metadata.file_type().is_symlink() {
                continue;
            }
            if links == 40 {
                bail!("link chain is too deep: {}", path.display());
            }
            links += 1;
            let target = fs::read_link(&resolved)
                .with_context(|| format!("failed to read link {}", resolved.display()))?;
            let target = if target.is_absolute() {
                let target = lexical_normalize(&target);
                if let Ok(relative) = target.strip_prefix(final_root) {
                    staging.join(relative)
                } else {
                    target
                }
            } else {
                resolved.parent().unwrap_or(Path::new(".")).join(target)
            };
            current = lexical_normalize(&target.join(components.as_path()));
            continue 'resolve;
        }
        return Ok(Some(resolved));
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::components::Component;
    use crate::config::Defaults;

    fn paths(root: &Path) -> Paths {
        let root = root.to_owned();
        Paths::from_lookup(move |key| match key {
            "HOME" => Some(OsString::from(root.join("home"))),
            _ => None,
        })
        .unwrap()
    }

    #[test]
    fn imported_targets_inside_a_portable_folder_are_stored_relative() {
        let paths = Paths::select(
            |key| (key == "HOME").then(|| OsString::from("/home/me")),
            Some(Path::new("/mnt/usb/wineo")),
        )
        .unwrap();
        let target = |exe: &str, cwd: &str| Target {
            exe: exe.to_owned(),
            args: Vec::new(),
            cwd: Some(cwd.to_owned()),
            default: false,
            gamemode: None,
            wayland: None,
            mangohud: None,
            gamescope: None,
            hooks: None,
            desktop: false,
            name: None,
        };
        let settings = ImportedSettings {
            targets: BTreeMap::from([
                (
                    "inside".to_owned(),
                    target(
                        "/mnt/usb/wineo/prefixes/game/drive_c/game.exe",
                        "/mnt/usb/wineo/prefixes/game/drive_c",
                    ),
                ),
                (
                    "outside".to_owned(),
                    target("/home/me/Games/game.exe", "C:\\Games"),
                ),
            ]),
            ..ImportedSettings::default()
        };
        let stored = stored_settings(&paths, &settings).unwrap();
        let inside = &stored.targets["inside"];
        assert_eq!(inside.exe, "prefixes/game/drive_c/game.exe");
        assert_eq!(inside.cwd.as_deref(), Some("prefixes/game/drive_c"));
        let outside = &stored.targets["outside"];
        assert_eq!(outside.exe, "/home/me/Games/game.exe");
        assert_eq!(outside.cwd.as_deref(), Some("C:\\Games"));
    }

    #[test]
    fn relative_paths_are_taken_from_where_the_link_sits() {
        let cases = [
            ("/usb/prefixes/game", "/usb/prefixes/game", "."),
            (
                "/usb/prefixes/game/drive_c/x",
                "/usb/prefixes/game/drive_c",
                "x",
            ),
            (
                "/usb/prefixes/game/drive_c",
                "/usb/prefixes/game/pfx/a",
                "../../drive_c",
            ),
            (
                "/usb/prefixes/game/drive_c/x",
                "/usb/prefixes/.wineo-snapshots/game/one/prefix",
                "../../../../game/drive_c/x",
            ),
        ];
        for (target, base, expected) in cases {
            assert_eq!(
                relative_path(Path::new(target), Path::new(base)),
                Path::new(expected),
                "{target} from {base}"
            );
        }
    }

    fn prefix(root: &Path, arch: &str) {
        fs::create_dir_all(root.join("drive_c")).unwrap();
        fs::write(
            root.join("system.reg"),
            format!("WINE REGISTRY Version 2\n\n#arch={arch}\n"),
        )
        .unwrap();
    }

    fn wine_runner(root: &Path, name: &str) {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("bin")).unwrap();
        for executable in ["wine", "wineserver"] {
            let path = dir.join("bin").join(executable);
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn proton_runner(root: &Path, name: &str) {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("files/bin")).unwrap();
        let proton = dir.join("proton");
        fs::write(&proton, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&proton, fs::Permissions::from_mode(0o755)).unwrap();
        for executable in ["wine", "wineserver"] {
            let path = dir.join("files/bin").join(executable);
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(dir.join("version"), format!("1 {name}\n")).unwrap();
    }

    fn base_global(runner_root: &Path, prefix_root: &Path) -> GlobalConfig {
        GlobalConfig {
            defaults: Defaults {
                runner: "wine-build".to_owned(),
                runner_root: Some(runner_root.to_string_lossy().into_owned()),
                prefix_root: Some(prefix_root.to_string_lossy().into_owned()),
                ..Defaults::default()
            },
            ..GlobalConfig::default()
        }
    }

    fn component_build(root: &Path, component: Component, name: &str) {
        let (wide, narrow) = component.arch_dirs();
        let marker = match component {
            Component::Dxvk => "dxgi.dll",
            Component::Vkd3d => "d3d12.dll",
        };
        for arch in [wide, narrow] {
            let dir = root.join(name).join(arch);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(marker), "build").unwrap();
        }
    }

    fn import_fails_when_reviewed_component_disappears(component: Component) {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runner_root = root.path().join("runners");
        let prefix_root = root.path().join("prefixes");
        let dxvk_root = root.path().join("dxvk");
        wine_runner(&runner_root, "wine-build");
        let source = root.path().join("source");
        prefix(&source, "win64");
        let mut global = base_global(&runner_root, &prefix_root);
        global.defaults.dxvk_root = Some(dxvk_root.to_string_lossy().into_owned());

        let dxvk = "dxvk-2.7";
        let vkd3d = "vkd3d-proton-2.14";
        component_build(&dxvk_root, Component::Dxvk, dxvk);
        let removed = match component {
            Component::Dxvk => dxvk_root.join(dxvk),
            Component::Vkd3d => {
                let vkd3d_root = root.path().join("vkd3d");
                global.defaults.vkd3d_root = Some(vkd3d_root.to_string_lossy().into_owned());
                component_build(&vkd3d_root, Component::Vkd3d, vkd3d);
                vkd3d_root.join(vkd3d)
            }
        };

        let runners = Runners::new(&paths, &global).unwrap();
        let runner = runners.resolve("wine-build").unwrap();
        let reviewed = inspect(&source, Some(&runner), None).unwrap();
        let settings = ImportedSettings {
            dxvk: Some(dxvk.to_owned()),
            vkd3d: (component == Component::Vkd3d).then(|| vkd3d.to_owned()),
            ..ImportedSettings::default()
        };
        fs::remove_dir_all(removed).unwrap();

        let error = execute_reviewed(
            &paths,
            &global,
            ImportRequest {
                source,
                name: "copied".to_owned(),
                runner: "wine-build".to_owned(),
                destination: None,
                adopt: false,
                arch: None,
            },
            Some(&reviewed),
            Some(&settings),
            &Token::new(),
            |_| {},
        )
        .unwrap_err()
        .to_string();

        let component_name = match component {
            Component::Dxvk => dxvk,
            Component::Vkd3d => vkd3d,
        };
        let build_root = match component {
            Component::Dxvk => dxvk_root,
            Component::Vkd3d => root.path().join("vkd3d"),
        };
        assert_eq!(
            error,
            format!(
                "prefix {} is ready, but registration failed: {} build `{component_name}` is not installed, and nothing is installed in {}\nGet one with: wineo download {}\nThen retry this import with --adopt.",
                prefix_root.join("copied").display(),
                component.as_str(),
                build_root.display(),
                component.as_str(),
            )
        );
        assert!(prefix_root.join("copied/system.reg").is_file());
        assert!(!paths.environment_config("copied").exists());
    }

    #[test]
    fn reviewed_dxvk_disappearance_does_not_register_the_import() {
        import_fails_when_reviewed_component_disappears(Component::Dxvk);
    }

    #[test]
    fn reviewed_vkd3d_disappearance_does_not_register_the_import() {
        import_fails_when_reviewed_component_disappears(Component::Vkd3d);
    }

    #[test]
    fn reviewed_import_registers_installed_dxvk_and_vkd3d_builds() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runner_root = root.path().join("runners");
        let prefix_root = root.path().join("prefixes");
        let dxvk_root = root.path().join("dxvk");
        let vkd3d_root = root.path().join("vkd3d");
        wine_runner(&runner_root, "wine-build");
        let source = root.path().join("source");
        prefix(&source, "win64");
        let mut global = base_global(&runner_root, &prefix_root);
        global.defaults.dxvk_root = Some(dxvk_root.to_string_lossy().into_owned());
        global.defaults.vkd3d_root = Some(vkd3d_root.to_string_lossy().into_owned());

        let dxvk = "dxvk-2.7";
        let vkd3d = "vkd3d-proton-2.14";
        component_build(&dxvk_root, Component::Dxvk, dxvk);
        component_build(&vkd3d_root, Component::Vkd3d, vkd3d);
        let reviewed = inspect(&source, None, None).unwrap();
        let settings = ImportedSettings {
            dxvk: Some(dxvk.to_owned()),
            vkd3d: Some(vkd3d.to_owned()),
            ..ImportedSettings::default()
        };

        let env = execute_reviewed(
            &paths,
            &global,
            ImportRequest {
                source,
                name: "copied".to_owned(),
                runner: "wine-build".to_owned(),
                destination: None,
                adopt: false,
                arch: None,
            },
            Some(&reviewed),
            Some(&settings),
            &Token::new(),
            |_| {},
        )
        .unwrap();

        let wine = env.config.wine().unwrap();
        assert_eq!(wine.dxvk.as_deref(), Some(dxvk));
        assert_eq!(wine.vkd3d.as_deref(), Some(vkd3d));
        let config_path = paths.environment_config("copied");
        assert!(config_path.is_file());
        let config_text = fs::read_to_string(&config_path).unwrap();
        let config = EnvironmentConfig::parse(&config_path, &config_text).unwrap();
        let wine = config.wine().unwrap();
        assert_eq!(wine.dxvk.as_deref(), Some(dxvk));
        assert_eq!(wine.vkd3d.as_deref(), Some(vkd3d));
    }

    #[test]
    fn inspection_normalizes_a_proton_pfx_path_and_reports_external_links() {
        let root = tempfile::tempdir().unwrap();
        let compatdata = root.path().join("compatdata/123");
        prefix(&compatdata.join("pfx"), "win64");
        fs::create_dir_all(compatdata.join("dosdevices")).unwrap();
        symlink("/mnt/games", compatdata.join("dosdevices/z:")).unwrap();

        let inspection = inspect(
            compatdata.join("pfx"),
            Some(&Runner::fake_proton("GE-Proton9-20")),
            None,
        )
        .unwrap();
        assert_eq!(inspection.source_root, compatdata);
        assert_eq!(inspection.layout, ImportLayout::Proton);
        assert_eq!(inspection.arch, Arch::Win64);
        assert_eq!(inspection.external_dependencies.len(), 1);
        assert_eq!(
            inspection.external_dependencies[0].target,
            Path::new("/mnt/games")
        );
    }

    #[test]
    fn import_fingerprint_sees_a_same_size_rewrite_without_reading_content() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        prefix(&source, "win64");
        let file = source.join("drive_c/save.dat");
        fs::write(&file, "aaaa").unwrap();
        let cancel = Token::new();
        let fingerprint = || import_fingerprint(&source, &mut Check::new(&cancel, &mut |_| {}));
        let before = fingerprint().unwrap();
        assert_eq!(fingerprint().unwrap(), before);

        // Coarse kernel timestamps could otherwise give the rewrite the same ctime.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let metadata = fs::metadata(&file).unwrap();
        fs::write(&file, "bbbb").unwrap();
        filetime::set_file_mtime(
            &file,
            filetime::FileTime::from_last_modification_time(&metadata),
        )
        .unwrap();
        let after = fs::metadata(&file).unwrap();
        assert_eq!(after.len(), metadata.len());
        assert_eq!(after.mtime_nsec(), metadata.mtime_nsec());
        assert_ne!(fingerprint().unwrap(), before);
    }

    #[test]
    fn content_fingerprint_of_a_copy_matches_the_original() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        prefix(&source, "win64");
        fs::write(source.join("drive_c/game.exe"), "game").unwrap();
        let copy = root.path().join("copy");
        copy_tree(&source, &copy, &copy, &Token::new(), &mut |_| {}).unwrap();
        assert_eq!(fingerprint(&copy).unwrap(), fingerprint(&source).unwrap());

        let cancel = Token::new();
        let mut quiet = |_| {};
        assert_ne!(
            import_fingerprint(&copy, &mut Check::new(&cancel, &mut quiet)).unwrap(),
            import_fingerprint(&source, &mut Check::new(&cancel, &mut quiet)).unwrap()
        );
    }

    #[test]
    fn a_folder_of_prefixes_is_refused_but_a_proton_prefix_is_not() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("Prefixes/default");
        prefix(&folder, "win64");
        prefix(&folder.join("Game One"), "win64");
        prefix(&folder.join("Game Two/pfx"), "win64");
        let error = inspect(&folder, None, None).unwrap_err().to_string();
        assert_eq!(
            error,
            format!(
                "{} holds other prefixes; choose the game's own prefix",
                fs::canonicalize(&folder).unwrap().display()
            )
        );

        let proton = root.path().join("compatdata/123");
        prefix(&proton, "win64");
        prefix(&proton.join("pfx"), "win64");
        let inspection =
            inspect(&proton, Some(&Runner::fake_proton("GE-Proton9-20")), None).unwrap();
        assert_eq!(inspection.layout, ImportLayout::Proton);
    }

    #[test]
    fn both_layout_markers_require_a_runner_choice() {
        let root = tempfile::tempdir().unwrap();
        prefix(root.path(), "win64");
        prefix(&root.path().join("pfx"), "win64");
        assert!(inspect(root.path(), None, None).is_err());
        assert_eq!(
            inspect(
                root.path(),
                Some(&Runner::fake_proton("GE-Proton9-20")),
                None
            )
            .unwrap()
            .layout,
            ImportLayout::Proton
        );
        assert_eq!(
            inspect(root.path(), Some(&Runner::fake("wine")), None)
                .unwrap()
                .layout,
            ImportLayout::Wine
        );
    }

    #[test]
    fn proton_import_preserves_a_pfx_dot_link_in_a_compatdata_root() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runner_root = root.path().join("runners");
        let prefix_root = root.path().join("prefixes");
        proton_runner(&runner_root, "GE-Proton9-20");
        let source = root.path().join("compatdata/123");
        prefix(&source, "win64");
        symlink(".", source.join("pfx")).unwrap();
        let global = {
            let mut global = base_global(&runner_root, &prefix_root);
            global.defaults.runner = "GE-Proton9-20".to_owned();
            global
        };
        let selected_runner = Runners::at(runner_root.clone())
            .resolve("GE-Proton9-20")
            .unwrap();
        let inspection = inspect(&source, Some(&selected_runner), None).unwrap();
        assert_eq!(inspection.layout, ImportLayout::Proton);
        execute_reviewed(
            &paths,
            &global,
            ImportRequest {
                source: source.clone(),
                name: "stellar".to_owned(),
                runner: "GE-Proton9-20".to_owned(),
                destination: None,
                adopt: false,
                arch: None,
            },
            Some(&inspection),
            None,
            &Token::new(),
            |_| {},
        )
        .unwrap();
        let copied = prefix_root.join("stellar");
        assert_eq!(fs::read_link(copied.join("pfx")).unwrap(), Path::new("."));
    }

    #[test]
    fn proton_import_rebases_an_absolute_pfx_self_link() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runner_root = root.path().join("runners");
        let prefix_root = root.path().join("prefixes");
        proton_runner(&runner_root, "GE-Proton9-20");
        let source = root.path().join("compatdata/123");
        prefix(&source, "win64");
        symlink(&source, source.join("pfx")).unwrap();
        let global = base_global(&runner_root, &prefix_root);

        execute(
            &paths,
            &global,
            ImportRequest {
                source: source.clone(),
                name: "stellar".to_owned(),
                runner: "GE-Proton9-20".to_owned(),
                destination: None,
                adopt: false,
                arch: None,
            },
            &Token::new(),
            |_| {},
        )
        .unwrap();

        let copied = prefix_root.join("stellar");
        assert_eq!(fs::read_link(source.join("pfx")).unwrap(), source);
        assert_eq!(fs::read_link(copied.join("pfx")).unwrap(), copied);
        assert!(copied.join("pfx/system.reg").is_file());
        assert!(copied.join("pfx/drive_c").is_dir());
        assert!(paths.environment_config("stellar").exists());
    }

    #[test]
    fn import_reports_checking_and_stops_when_canceled_during_a_check() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runner_root = root.path().join("runners");
        let prefix_root = root.path().join("prefixes");
        wine_runner(&runner_root, "wine-build");
        let source = root.path().join("source");
        prefix(&source, "win64");
        for index in 0..1200 {
            fs::write(source.join(format!("drive_c/{index}")), "x").unwrap();
        }
        let global = base_global(&runner_root, &prefix_root);
        let request = |name: &str| ImportRequest {
            source: source.clone(),
            name: name.to_owned(),
            runner: "wine-build".to_owned(),
            destination: None,
            adopt: false,
            arch: None,
        };

        let canceled = Token::new();
        let result = execute(&paths, &global, request("stopped"), &canceled, |event| {
            if matches!(event, ImportProgress::Checking { .. }) {
                canceled.cancel();
            }
        });
        assert!(result.unwrap_err().is::<Cancelled>());
        assert!(!prefix_root.join("stopped").exists());
        assert!(!paths.environment_config("stopped").exists());

        let mut events = Vec::new();
        execute(&paths, &global, request("done"), &Token::new(), |event| {
            events.push(event)
        })
        .unwrap();
        let started = events
            .iter()
            .position(|event| matches!(event, ImportProgress::Started { .. }))
            .unwrap();
        let checks = |events: &[ImportProgress]| {
            events
                .iter()
                .filter_map(|event| match event {
                    ImportProgress::Checking { entries } => Some(*entries),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        // Two inspections before the copy, each with a periodic and a final count.
        assert_eq!(checks(&events[..started]), [1000, 1203, 1000, 1203]);
        assert_eq!(checks(&events[started..]), [1000, 1203]);

        // Canceling on the last check after the copy must still stop before
        // publication.
        let late = Token::new();
        let mut copying = false;
        let result = execute(
            &paths,
            &global,
            request("late"),
            &late,
            |event| match event {
                ImportProgress::Started { .. } => copying = true,
                ImportProgress::Checking { entries: 1203 } if copying => late.cancel(),
                _ => {}
            },
        );
        assert!(result.unwrap_err().is::<Cancelled>());
        assert!(!prefix_root.join("late").exists());
        assert!(!paths.environment_config("late").exists());
        let staging = fs::read_dir(&prefix_root)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".wineo-import-")
            });
        assert!(!staging, "staging directory left behind");
    }

    #[test]
    fn staged_validation_rejects_missing_targets_and_link_cycles() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let destination = root.path().join("destination");
        fs::create_dir(&staging).unwrap();
        symlink(&destination, staging.join("pfx")).unwrap();
        assert!(validate_staged_prefix(&staging, &destination, ImportLayout::Proton).is_err());
        assert!(
            staged_path_if_exists(&staging.join("pfx/system.reg"), &staging, &destination).is_err()
        );

        fs::remove_file(staging.join("pfx")).unwrap();
        symlink(destination.join("loop"), staging.join("pfx")).unwrap();
        symlink(destination.join("pfx"), staging.join("loop")).unwrap();
        let error = validate_staged_prefix(&staging, &destination, ImportLayout::Proton)
            .unwrap_err()
            .to_string();
        assert!(error.contains("link chain is too deep"), "{error}");
        assert!(!destination.exists());
    }

    #[test]
    fn copy_preserves_hardlinks_and_rebases_internal_absolute_links() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        prefix(&source, "win64");
        let file = source.join("drive_c/data.bin");
        fs::write(&file, "same").unwrap();
        fs::hard_link(&file, source.join("drive_c/data-copy.bin")).unwrap();
        symlink(
            source.join("drive_c/data.bin"),
            source.join("drive_c/absolute-link"),
        )
        .unwrap();
        symlink("/outside/save", source.join("drive_c/external-link")).unwrap();

        copy_tree(
            &source,
            &destination,
            &destination,
            &Token::new(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(
            fs::read(destination.join("drive_c/data.bin")).unwrap(),
            b"same"
        );
        let one = fs::metadata(destination.join("drive_c/data.bin")).unwrap();
        let two = fs::metadata(destination.join("drive_c/data-copy.bin")).unwrap();
        assert_eq!((one.dev(), one.ino()), (two.dev(), two.ino()));
        assert_eq!(
            fs::read_link(destination.join("drive_c/absolute-link")).unwrap(),
            destination.join("drive_c/data.bin")
        );
        assert_eq!(
            fs::read_link(destination.join("drive_c/external-link")).unwrap(),
            Path::new("/outside/save")
        );
    }

    #[test]
    fn regular_file_copy_uses_reflink_and_falls_back_only_when_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::write(&source, b"contents").unwrap();
        let cloned = root.path().join("cloned");
        copy_regular_file_with(&source, &cloned, |output, input| {
            std::io::copy(input, output).unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&cloned).unwrap(), b"contents");

        let fallback = root.path().join("fallback");
        copy_regular_file_with(&source, &fallback, |_, _| Err(Errno::OPNOTSUPP)).unwrap();
        assert_eq!(fs::read(&fallback).unwrap(), b"contents");

        let failed = root.path().join("failed");
        let error = copy_regular_file_with(&source, &failed, |_, _| Err(Errno::IO)).unwrap_err();
        assert!(error.to_string().contains("failed to reflink"));
        assert_eq!(fs::metadata(&failed).unwrap().len(), 0);
    }

    #[test]
    fn strict_reflink_copy_fails_when_reflinks_are_unsupported() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::write(&source, b"contents").unwrap();
        let destination = root.path().join("destination");

        let error =
            copy_regular_file_with_mode(&source, &destination, |_, _| Err(Errno::OPNOTSUPP), true)
                .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("filesystem does not support reflinks")
        );
        assert_eq!(fs::metadata(destination).unwrap().len(), 0);
    }

    #[test]
    fn execute_copies_prefix_and_registers_heroic_settings_atomically() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runner_root = root.path().join("runners");
        let prefix_root = root.path().join("prefixes");
        wine_runner(&runner_root, "wine-build");
        let source = root.path().join("old-prefix");
        prefix(&source, "win64");
        let global = base_global(&runner_root, &prefix_root);
        let reviewed = inspect(&source, None, None).unwrap();
        let settings = ImportedSettings {
            targets: BTreeMap::from([(
                "game".to_owned(),
                Target {
                    exe: "Z:\\games\\game.exe".to_owned(),
                    args: vec!["--launcher".to_owned()],
                    cwd: Some("Z:\\games".to_owned()),
                    default: true,
                    gamemode: None,
                    wayland: None,
                    mangohud: None,
                    gamescope: None,
                    hooks: None,
                    desktop: false,
                    name: Some("Game".to_owned()),
                },
            )]),
            env: BTreeMap::from([("GAME_MODE".to_owned(), "heroic".to_owned())]),
            heroic_id: Some("kZXLzzKAFWBP4dpRatpPHu".to_owned()),
            ..ImportedSettings::default()
        };
        let env = execute_reviewed(
            &paths,
            &global,
            ImportRequest {
                source: source.clone(),
                name: "game".to_owned(),
                runner: "wine-build".to_owned(),
                destination: None,
                adopt: false,
                arch: None,
            },
            Some(&reviewed),
            Some(&settings),
            &Token::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(env.config.targets["game"].exe, "Z:\\games\\game.exe");
        assert_eq!(env.config.env["GAME_MODE"], "heroic");
        assert_eq!(
            env.config.heroic_id.as_deref(),
            Some("kZXLzzKAFWBP4dpRatpPHu")
        );
        assert!(prefix_root.join("game/system.reg").is_file());
        assert!(paths.environment_config("game").is_file());
    }

    #[test]
    fn imported_configs_start_with_the_version() {
        let heroic = ImportedSettings {
            heroic_id: Some("abc".to_owned()),
            ..ImportedSettings::default()
        };
        for settings in [None, Some(&heroic)] {
            let text = config_text("~/p", "wine", Arch::Win64, settings).unwrap();
            assert!(
                text.starts_with("schema_version = 1\nprefix = \"~/p\"\n"),
                "{text}"
            );
            let config = EnvironmentConfig::parse(Path::new("/x.toml"), &text).unwrap();
            assert_eq!(config.schema_version, 1);
        }
    }

    #[test]
    fn execute_allows_an_idle_shared_prefix_but_rejects_a_lost_run() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let runner_root = root.path().join("runners");
        let prefix_root = root.path().join("prefixes");
        wine_runner(&runner_root, "wine-build");
        let source = root.path().join("shared");
        prefix(&source, "win64");
        let global = base_global(&runner_root, &prefix_root);
        fs::create_dir_all(paths.environments_dir()).unwrap();
        fs::write(
            paths.environment_config("old"),
            format!(
                "prefix = \"{}\"\nrunner = \"wine-build\"\narch = \"win64\"\n",
                source.display()
            ),
        )
        .unwrap();
        let first = execute(
            &paths,
            &global,
            ImportRequest {
                source: source.clone(),
                name: "new".to_owned(),
                runner: "wine-build".to_owned(),
                destination: None,
                adopt: false,
                arch: None,
            },
            &Token::new(),
            |_| {},
        );
        assert!(first.is_ok(), "idle shared prefix should import: {first:?}");

        let state = crate::runtime::RunState {
            status: crate::runtime::RunStatus::Running,
            mode: crate::runtime::RunMode::Attached,
            target: "game".to_owned(),
            runner: None,
            prefix: None,
            native: false,
            pid: 1,
            group: None,
            boot_id: None,
            leader_start: None,
            started_at: "2026-09-21T00:00:00Z".parse().unwrap(),
            heartbeat_at: None,
            program_exited_at: None,
            ended_at: None,
            exit_code: None,
            requested_target: None,
            args: Vec::new(),
            env: BTreeMap::new(),
        };
        Runtime::new(&paths, "old").write_state(&state).unwrap();
        let lost = execute(
            &paths,
            &global,
            ImportRequest {
                source,
                name: "lost".to_owned(),
                runner: "wine-build".to_owned(),
                destination: None,
                adopt: false,
                arch: None,
            },
            &Token::new(),
            |_| {},
        );
        assert!(lost.is_err());
    }

    #[test]
    fn copy_runner_writes_alias_compatible_provenance_and_refuses_alias_names() {
        let root = tempfile::tempdir().unwrap();
        proton_runner(root.path(), "GE-Proton9-20");
        let source = root.path().join("GE-Proton9-20");
        let runners = root.path().join("installed");
        let receipt =
            copy_runner(&source, &runners, "GE-Proton9-20", &Token::new(), |_| {}).unwrap();
        assert_eq!(
            crate::runner_alias::version(&runners, "GE-Proton9-20", "GE-Proton-Latest"),
            Some(vec![9, 20])
        );
        assert!(receipt.receipt.is_file());
        assert!(
            copy_runner(&source, &runners, "GE-Proton-Latest", &Token::new(), |_| {},).is_err()
        );
    }

    #[test]
    fn runner_copy_preflight_rejects_unavailable_and_invalid_sources() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("app");
        assert!(inspect_runner_copy(&missing).is_err());
        fs::create_dir(&missing).unwrap();
        assert!(inspect_runner_copy(&missing).is_err());

        wine_runner(root.path(), "wine-build");
        let wine = inspect_runner_copy(root.path().join("wine-build")).unwrap();
        assert!(wine.supports_wine);
        assert!(!wine.supports_proton);

        proton_runner(root.path(), "GE-Proton9-20");
        let proton = inspect_runner_copy(root.path().join("GE-Proton9-20")).unwrap();
        assert!(proton.supports_proton);
    }

    #[test]
    fn runner_copy_retains_external_dependencies_and_rebases_internal_links() {
        let root = tempfile::tempdir().unwrap();
        wine_runner(root.path(), "wine-build");
        let source = root.path().join("wine-build");
        fs::rename(source.join("bin/wine"), source.join("bin/wine-real")).unwrap();
        symlink(source.join("bin/wine-real"), source.join("bin/wine")).unwrap();
        let external = root.path().join("shared-lib");
        fs::write(&external, "host data").unwrap();
        symlink("../../shared-lib", source.join("bin/lib-shared")).unwrap();

        let inspection = inspect_runner_copy(&source).unwrap();
        assert_eq!(inspection.external_dependencies.len(), 1);
        assert_eq!(inspection.external_dependencies[0].target, external);

        let receipt = copy_runner(
            &source,
            root.path().join("installed"),
            "wine-build",
            &Token::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(receipt.external_dependencies.len(), 1);
        assert_eq!(
            receipt.external_dependencies[0].source,
            receipt.path.join("bin/lib-shared")
        );
        assert_eq!(receipt.external_dependencies[0].target, external);
        assert_eq!(
            fs::read_to_string(receipt.path.join("bin/lib-shared")).unwrap(),
            "host data"
        );
        assert_eq!(
            fs::canonicalize(receipt.path.join("bin/wine")).unwrap(),
            receipt.path.join("bin/wine-real")
        );
        assert_eq!(fs::read_to_string(&external).unwrap(), "host data");
    }

    #[test]
    fn reviewed_runner_copy_rejects_changes_before_copy() {
        let root = tempfile::tempdir().unwrap();
        wine_runner(root.path(), "wine-build");
        let source = root.path().join("wine-build");
        let reviewed = inspect_runner_copy(&source).unwrap();
        fs::remove_dir_all(source.join("bin")).unwrap();
        fs::write(source.join("proton"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(source.join("proton"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(source.join("files/bin")).unwrap();
        for name in ["wine", "wineserver"] {
            let path = source.join("files/bin").join(name);
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let destination = root.path().join("installed");
        let error =
            copy_runner_reviewed(&reviewed, &destination, "wine-build", &Token::new(), |_| {})
                .unwrap_err();
        assert!(error.to_string().contains("reviewed runner changed"));
        assert!(!destination.exists());
    }

    #[test]
    fn runner_copy_cancellation_and_source_change_leave_no_publication() {
        let root = tempfile::tempdir().unwrap();
        wine_runner(root.path(), "wine-build");
        let source = root.path().join("wine-build");
        let destination = root.path().join("installed");
        let canceled = Token::new();
        let result = copy_runner(&source, &destination, "canceled", &canceled, |event| {
            if matches!(event, ImportProgress::Copied { .. }) {
                canceled.cancel();
            }
        });
        assert!(result.unwrap_err().is::<Cancelled>());
        assert!(!destination.join("canceled").exists());
        assert!(fs::read_dir(&destination).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".wineo-runner-")
        }));

        let changed = source.join("bin/wine");
        let result = copy_runner(&source, &destination, "changed", &Token::new(), |event| {
            if let ImportProgress::Copied { path } = event
                && path == changed
            {
                fs::write(&changed, "#!/bin/sh\necho changed\n").unwrap();
            }
        });
        assert!(result.unwrap_err().to_string().contains("changed"));
        assert!(!destination.join("changed").exists());
        assert!(fs::read_dir(&destination).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".wineo-runner-")
        }));
    }

    #[test]
    fn concurrent_runner_copies_publish_only_one_complete_build() {
        use std::sync::{Arc, Barrier};

        let root = tempfile::tempdir().unwrap();
        proton_runner(root.path(), "GE-Proton9-20");
        let source = root.path().join("GE-Proton9-20");
        let destination = root.path().join("installed");
        let barrier = Arc::new(Barrier::new(2));
        let copies = (0..2)
            .map(|_| {
                let source = source.clone();
                let destination = destination.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    copy_runner(source, destination, "GE-Proton9-20", &Token::new(), |_| {})
                })
            })
            .collect::<Vec<_>>();
        let results = copies
            .into_iter()
            .map(|copy| copy.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        assert!(destination.join("GE-Proton9-20/proton").is_file());
        assert!(destination.join("GE-Proton9-20/files/bin/wine").is_file());
        assert!(
            destination
                .join("GE-Proton9-20/.wineo-install.json")
                .is_file()
        );
        assert_eq!(
            fs::read_dir(&destination)
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
                .count(),
            1
        );
    }

    #[test]
    fn proton_runner_copy_preserves_executable_modes_and_internal_links() {
        let root = tempfile::tempdir().unwrap();
        proton_runner(root.path(), "GE-Proton9-20");
        let source = root.path().join("GE-Proton9-20");
        fs::rename(source.join("proton"), source.join("proton-real")).unwrap();
        symlink(source.join("proton-real"), source.join("proton")).unwrap();
        let reviewed = inspect_runner_copy(&source).unwrap();
        let receipt = copy_runner_reviewed(
            &reviewed,
            root.path().join("installed"),
            "GE-Proton9-20",
            &Token::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            fs::canonicalize(receipt.path.join("proton")).unwrap(),
            receipt.path.join("proton-real")
        );
        for path in ["proton-real", "files/bin/wine", "files/bin/wineserver"] {
            assert_ne!(
                fs::metadata(receipt.path.join(path))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
        }
    }
}
