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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use crate::config::{Arch, EnvironmentConfig, GlobalConfig, Target, validate_env_var};
use crate::download::{InstallLock, publish};
use crate::environment::Environment;
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

/// A deterministic snapshot of a source tree, including regular-file content,
/// metadata, and link targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFingerprint {
    hash: u64,
    entries: u64,
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
    pub gameid: Option<String>,
    pub store: Option<String>,
    pub dxvk: Option<String>,
    pub vkd3d: Option<String>,
    /// The Heroic game id, recorded only by a Heroic import.
    pub heroic_id: Option<String>,
}

/// Events emitted while copying a prefix or runner tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportProgress {
    Started { entries: u64 },
    Copied { path: PathBuf },
    Finished { entries: u64 },
}

/// A record written beside a copied runner. Download receipts can use this
/// same small JSON envelope and add provider-specific fields later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerCopyReceipt {
    pub path: PathBuf,
    pub name: String,
    pub receipt: PathBuf,
    pub provenance: RunnerProvenance,
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
    let source = normalize_source(canonical_source(source.as_ref())?, runner);
    let (source_root, wine_prefix, layout) = resolve_layout(&source, runner)?;
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

    let external_dependencies = find_external_dependencies(&source_root)?;
    let fingerprint = fingerprint(&source_root)?;
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
    cancel: &AtomicBool,
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
    cancel: &AtomicBool,
    mut progress: impl FnMut(ImportProgress),
) -> Result<Environment> {
    if !name::is_valid(&request.name) {
        bail!("invalid environment name `{}`", request.name);
    }
    if request.adopt && request.destination.is_some() {
        bail!("--adopt cannot be combined with --dir");
    }

    let runner = Runners::new(paths, global)
        .map_err(|err| anyhow!(err))?
        .resolve(&request.runner)
        .map_err(|err| anyhow!(err))?;
    let first = inspect(&request.source, Some(&runner), request.arch)?;
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
        bail!("environment `{}` already exists", request.name);
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
    let locked = inspect(&request.source, Some(&runner), request.arch)?;
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
    let prefix_value = paths.contract(&destination).map_err(|err| anyhow!(err))?;
    let config_text = config_text(&prefix_value, &request.runner, locked.arch, settings)?;

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
        copy_tree(
            &source_root,
            &staging_path,
            &destination,
            cancel,
            &mut progress,
        )?;
        check_cancelled(cancel)?;
        let after = fingerprint(&source_root)?;
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
        let after = fingerprint(&source_root)?;
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
        bail!("environment `{}` already exists", request.name);
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

fn ensure_same_review(expected: &Inspection, actual: &Inspection) -> Result<()> {
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

fn validate_staged_prefix(staging: &Path, final_root: &Path, layout: ImportLayout) -> Result<()> {
    let root = prefix_for_layout(staging, layout);
    let system = staged_path(&root.join("system.reg"), staging, final_root)?;
    let drive_c = staged_path(&root.join("drive_c"), staging, final_root)?;
    if !system.is_file() || !drive_c.is_dir() {
        bail!("staged import is missing system.reg or drive_c");
    }
    File::open(&system).with_context(|| format!("failed to read {}", system.display()))?;
    Ok(())
}

fn reject_overlapping_paths(source: &Path, destination: &Path) -> Result<()> {
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

fn find_external_dependencies(root: &Path) -> Result<Vec<ExternalDependency>> {
    let mut found = Vec::new();
    walk_no_follow(root, &mut |path, metadata| {
        if !metadata.file_type().is_symlink() {
            return Ok(());
        }
        let target = fs::read_link(path)
            .with_context(|| format!("failed to read link {}", path.display()))?;
        let resolved = resolve_link(path, &target);
        if !resolved.starts_with(root) {
            found.push(ExternalDependency {
                source: path.to_owned(),
                target: resolved,
            });
        }
        Ok(())
    })?;
    found.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(found)
}

fn fingerprint(root: &Path) -> Result<SourceFingerprint> {
    let mut records = Vec::new();
    let mut entries = 0;
    walk_no_follow(root, &mut |path, metadata| {
        let relative = path.strip_prefix(root).unwrap_or(path);
        let link = if metadata.file_type().is_symlink() {
            Some(fs::read_link(path)?.to_string_lossy().into_owned())
        } else {
            None
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| (duration.as_secs(), duration.subsec_nanos()));
        let content = if metadata.file_type().is_file() {
            Some(file_digest(path)?)
        } else {
            None
        };
        records.push((
            relative.to_string_lossy().into_owned(),
            file_kind(metadata),
            metadata.len(),
            metadata.mode(),
            metadata.nlink(),
            modified,
            content,
            link,
        ));
        entries += 1;
        Ok(())
    })?;
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    records.hash(&mut hasher);
    Ok(SourceFingerprint {
        hash: hasher.finish(),
        entries,
    })
}

fn file_digest(path: &Path) -> Result<u64> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to read {} for fingerprinting", path.display()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut buffer = [0; 128 * 1024];
    loop {
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

fn copy_tree(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<()> {
    let total = fingerprint(source)?.entries;
    progress(ImportProgress::Started { entries: total });
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    let mut hardlinks = HashMap::new();
    copy_entry(
        source,
        destination,
        source,
        final_root,
        cancel,
        progress,
        &mut hardlinks,
    )?;
    Ok(())
}

fn copy_entry(
    source: &Path,
    destination: &Path,
    source_root: &Path,
    final_root: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(ImportProgress),
    hardlinks: &mut HashMap<(u64, u64), PathBuf>,
) -> Result<()> {
    check_cancelled(cancel)?;
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect {}", source.display()))?;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        let target = fs::read_link(source)
            .with_context(|| format!("failed to read link {}", source.display()))?;
        let resolved = resolve_link(source, &target);
        let target = if resolved.starts_with(source_root) {
            if target.is_absolute() {
                let relative = resolved.strip_prefix(source_root).unwrap_or(Path::new(""));
                lexical_normalize(&final_root.join(relative))
            } else {
                target
            }
        } else {
            // An external link would point somewhere else after copying when
            // it is relative, or through an internal link that is re-rooted.
            // Keep the resolved external data location explicit.
            resolved
        };
        std::os::unix::fs::symlink(&target, destination)
            .with_context(|| format!("failed to copy link {}", source.display()))?;
    } else if kind.is_dir() {
        if source != source_root {
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
                source_root,
                final_root,
                cancel,
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
            fs::copy(source, destination)
                .with_context(|| format!("failed to copy {}", source.display()))?;
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

fn check_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        bail!("import cancelled")
    }
    Ok(())
}

fn config_text(
    prefix: &str,
    runner: &str,
    arch: Arch,
    settings: Option<&ImportedSettings>,
) -> Result<String> {
    let mut doc = DocumentMut::new();
    doc["prefix"] = value(prefix);
    doc["runner"] = value(runner);
    doc["arch"] = value(arch.as_str());
    if let Some(settings) = settings {
        for (key, env_value) in &settings.env {
            validate_env_var(key, env_value).map_err(|message| anyhow!(message))?;
        }
        if let Some(gamemode) = settings.gamemode {
            doc["gamemode"] = Value::from(gamemode).into();
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

fn write_config_once(path: &Path, contents: &str) -> Result<()> {
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
pub fn copy_runner(
    source: impl AsRef<Path>,
    destination_root: impl AsRef<Path>,
    name: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ImportProgress),
) -> Result<RunnerCopyReceipt> {
    let source = canonical_source(source.as_ref())?;
    if !name::is_valid_runner(name) {
        bail!("invalid runner name `{name}`");
    }
    if runner_alias::is_alias(name) {
        bail!("runner name `{name}` is reserved for a latest alias");
    }
    validate_runner_tree(&source)?;
    let destination_root = std::path::absolute(destination_root.as_ref())?;
    fs::create_dir_all(&destination_root)
        .with_context(|| format!("failed to create {}", destination_root.display()))?;
    let destination = destination_root.join(name);
    reject_overlapping_paths(&source, &destination)?;
    if destination.exists() {
        bail!(
            "runner destination already exists: {}",
            destination.display()
        );
    }
    let before = fingerprint(&source)?;
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
    copy_tree(&source, &staging_path, &destination, cancel, &mut progress)?;
    check_cancelled(cancel)?;
    if fingerprint(&source)? != before {
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
    let provenance = runner_provenance(&source, name)?;
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

fn validate_runner_tree(root: &Path) -> Result<()> {
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
    if !proton_ok && !wine_ok {
        bail!(
            "{} is not a valid Wine or Proton runner tree",
            root.display()
        );
    }
    Ok(())
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
    match fs::symlink_metadata(path) {
        Ok(_) => staged_path(path, staging, final_root).map(Some),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to inspect {}", path.display())),
    }
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
    let mut current = path.to_owned();
    for _ in 0..40 {
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("failed to inspect {}", current.display()))?;
        if !metadata.file_type().is_symlink() {
            return Ok(current);
        }
        let target = fs::read_link(&current)
            .with_context(|| format!("failed to read link {}", current.display()))?;
        let target = if target.is_absolute() {
            let target = lexical_normalize(&target);
            if target.starts_with(final_root) {
                let relative = target.strip_prefix(final_root).unwrap_or(Path::new(""));
                staging.join(relative)
            } else {
                target
            }
        } else {
            current.parent().unwrap_or(Path::new(".")).join(target)
        };
        current = lexical_normalize(&target);
    }
    bail!("link chain is too deep: {}", path.display())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::config::Defaults;

    fn paths(root: &Path) -> Paths {
        let root = root.to_owned();
        Paths::from_lookup(move |key| match key {
            "HOME" => Some(OsString::from(root.join("home"))),
            _ => None,
        })
        .unwrap()
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
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        let copied = prefix_root.join("stellar");
        assert_eq!(fs::read_link(copied.join("pfx")).unwrap(), Path::new("."));
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
            &AtomicBool::new(false),
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
            &AtomicBool::new(false),
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
            &AtomicBool::new(false),
            |_| {},
        );
        assert!(first.is_ok(), "idle shared prefix should import: {first:?}");

        let state = crate::runtime::RunState {
            status: crate::runtime::RunStatus::Running,
            mode: crate::runtime::RunMode::Attached,
            target: "game".to_owned(),
            runner: None,
            pid: 1,
            group: None,
            started_at: "2026-09-21T00:00:00Z".parse().unwrap(),
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
            &AtomicBool::new(false),
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
        let receipt = copy_runner(
            &source,
            &runners,
            "GE-Proton9-20",
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            crate::runner_alias::version(&runners, "GE-Proton9-20", "GE-Proton-Latest"),
            Some(vec![9, 20])
        );
        assert!(receipt.receipt.is_file());
        assert!(
            copy_runner(
                &source,
                &runners,
                "GE-Proton-Latest",
                &AtomicBool::new(false),
                |_| {},
            )
            .is_err()
        );
    }
}
