//! Import existing Wine prefixes and runner trees without executing anything.
//!
//! Inspection is deliberately separate from execution.  Callers can render an
//! inspection to a review screen, then pass the same inspection to
//! `execute_reviewed`; the source fingerprint is checked again while the
//! operation locks are held.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::build_lock;
use crate::cancel::Token;
use crate::config::{Arch, EnvironmentConfig, GlobalConfig, HooksSetting, Target};
use crate::environment::Environment;
use crate::error::Error;
use crate::gamescope::GamescopeSetting;
use crate::install::{self, Wanted};
use crate::name;
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};
use crate::runner::Runner;
use crate::runners::Runners;
use crate::runtime::Runtime;
use crate::stage::PrefixCopy;

mod config_text;
mod copy;
mod fingerprint;
mod runner_copy;

pub use config_text::register_native;
use config_text::{config_text, stored_settings};
pub(crate) use config_text::{native_config, write_config_once, write_native_config};
#[cfg(test)]
pub(crate) use copy::{ARMED_DROPS, copy_tree_strict_reflink};
pub(crate) use copy::{
    Links, Staging, copy_tree, copy_tree_strict_reflink_with, copy_tree_with, sync_filesystem,
};
use fingerprint::{Check, FingerprintKind, import_fingerprint, scan};
pub(crate) use fingerprint::{external_links_with, fingerprint, fingerprint_with};
use runner_copy::staged_path;
pub use runner_copy::{copy_runner_reviewed, inspect_runner_copy};

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
    pub hooks: Option<HooksSetting>,
    pub gameid: Option<String>,
    pub store: Option<String>,
    pub dxvk: Option<String>,
    pub vkd3d: Option<String>,
    /// The Heroic game id, recorded only by a Heroic import.
    pub heroic_id: Option<String>,
    /// The GOG game id, recorded only by migrating a GOG game from Heroic.
    pub gog_id: Option<String>,
}

/// Events emitted while checking and copying a prefix or runner tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportProgress {
    Warning(String),
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

/// The checks an import must pass before anything is written, without
/// taking locks, so a review or dry run can refuse what `execute_reviewed`
/// would. Returns the destination prefix as given, not resolved.
pub fn preflight(
    paths: &Paths,
    global: &GlobalConfig,
    request: &ImportRequest,
    inspection: &Inspection,
) -> Result<PathBuf> {
    if !name::is_valid(&request.name) {
        bail!("invalid environment name `{}`", request.name);
    }
    let source = &inspection.source_root;
    let destination = if request.adopt {
        source.clone()
    } else {
        match &request.destination {
            Some(destination) => destination.clone(),
            None => global.prefix_root(paths)?.join(&request.name),
        }
    };
    paths.check_prefix(&destination)?;
    if !request.adopt {
        reject_overlapping_paths(source, &destination)?;
        // Resolved as the copy will be, so `missing/../existing` or a
        // symlink is checked and named as the directory it lands in.
        let target = match &request.destination {
            Some(destination) => paths::resolve(destination),
            None => destination.clone(),
        };
        if target.exists() {
            bail!("import destination already exists: {}", target.display());
        }
    }
    if paths
        .environment_config(&request.name)
        .symlink_metadata()
        .is_ok()
    {
        return Err(Error::EnvironmentExists(request.name.clone()).into());
    }
    check_shared_configs(paths, &request.name, [source, &destination])?;
    check_destination(paths, &request.name, request.adopt, &destination)?;
    Ok(destination)
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

    let (runner, _runner_guard) =
        Runners::new(paths, global)?.resolve_locked(paths, &request.runner)?;
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
    preflight(paths, global, &request, &first)?;
    let source_root = first.source_root.clone();
    let destination = if request.adopt {
        source_root.clone()
    } else {
        match request.destination {
            Some(destination) => paths::resolve(&destination),
            None => global.prefix_root(paths)?.join(&request.name),
        }
    };

    let lock = Locks::new(paths);
    let mut operation = lock.acquire(
        Operation::Create,
        &request.name,
        &[&source_root, &destination],
    )?;
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
    check_destination(paths, &request.name, request.adopt, &destination)?;
    for shared in shared {
        Runtime::new(paths, &shared).check_quiet(&shared)?;
    }
    Runtime::new(paths, &request.name).check_quiet(&request.name)?;

    // Validate Heroic settings before copying so a bad target or environment
    // variable cannot leave a published prefix with no registration.
    let prefix_value = paths.store_prefix(&destination)?;
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
    // Held through registration, like clone, so nothing else can claim the
    // destination before its config is written.
    let _publish_lock = if !request.adopt {
        let publish_lock = PrefixCopy {
            source: &source_root,
            destination: &destination,
            layout: locked.layout,
            links: &Links::for_paths(paths),
            staging_prefix: ".wineo-import-",
            label: "import",
        }
        .run(cancel, &mut progress, |progress| {
            let after = import_fingerprint(&source_root, &mut Check::new(cancel, progress))?;
            if after != locked.fingerprint {
                bail!(
                    "{} changed while it was being copied; try again",
                    source_root.display()
                );
            }
            Ok(())
        })?;
        published = true;
        progress(ImportProgress::Finished {
            entries: locked.fingerprint.entries,
        });
        Some(publish_lock)
    } else {
        check_cancelled(cancel)?;
        let after = import_fingerprint(&source_root, &mut Check::new(cancel, &mut progress))?;
        if after != locked.fingerprint {
            bail!(
                "{} changed while it was being reviewed; try again",
                source_root.display()
            );
        }
        None
    };

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
    let config = EnvironmentConfig::parse(&config_path, &config_text)?;
    let wine = config.require_wine(&request.name)?;
    let _build_guards = build_lock::wine(paths, global, wine)?;
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
    let environment = Environment::load(paths, &request.name).map_err(|err| {
        anyhow!(
            "prefix {} and config {} are ready, but the config could not be reloaded: {err}",
            destination.display(),
            config_path.display()
        )
    })?;
    if let Some(warning) = crate::library_lifecycle::fresh_warning(paths, &request.name) {
        progress(ImportProgress::Warning(warning));
    }
    Ok(environment)
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

/// A copy fills a new directory nobody else may name, while adoption
/// registers the source where it is, which other environments may share.
fn check_destination(paths: &Paths, name: &str, adopt: bool, destination: &Path) -> Result<()> {
    if adopt {
        crate::environment::check_nesting(paths, name, destination)?;
    } else {
        crate::environment::check_destination(paths, name, destination)?;
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
        let sharing = crate::environment::sharing_prefix(paths, name, &prefix)?;
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

pub(crate) fn check_cancelled(cancel: &Token) -> Result<()> {
    if cancel.is_canceled() {
        return Err(Cancelled.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod config_once_tests {
    use super::config_text::SYNCED_DIRS;
    use super::*;

    #[test]
    fn a_new_config_is_synced_into_its_directory_with_no_staging_left() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("environments");
        fs::create_dir(&dir).unwrap();
        let path = dir.join("game.toml");
        SYNCED_DIRS.with(|synced| synced.borrow_mut().clear());
        write_config_once(&path, "kind = \"native\"\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "kind = \"native\"\n");
        assert_eq!(
            SYNCED_DIRS.with(|synced| synced.borrow().clone()),
            [dir.as_path()]
        );
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["game.toml"]);

        // A config that's already there is neither replaced nor synced for.
        // The directories exist by now, so only the new name's directory
        // would have been synced above.
        SYNCED_DIRS.with(|synced| synced.borrow_mut().clear());
        assert!(write_config_once(&path, "other").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "kind = \"native\"\n");
        assert!(SYNCED_DIRS.with(|synced| synced.borrow().is_empty()));
    }
    #[test]
    fn a_fresh_config_root_syncs_every_directory_it_creates() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config/wineo/environments");
        SYNCED_DIRS.with(|synced| synced.borrow_mut().clear());
        write_config_once(&dir.join("game.toml"), "kind = \"native\"\n").unwrap();
        // Each new directory's parent, from the one that already existed
        // down, then the directory the config was linked into.
        assert_eq!(
            SYNCED_DIRS.with(|synced| synced.borrow().clone()),
            [
                root.path().to_owned(),
                root.path().join("config"),
                root.path().join("config/wineo"),
                dir,
            ]
        );
    }
}
