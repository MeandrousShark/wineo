//! Copying an environment from the standard layout into a portable folder.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, anyhow, bail};
use toml_edit::{DocumentMut, Value};

use crate::art::StagedArt;
use crate::clone::{self, ConfigReview};
use crate::config::{self, EnvironmentConfig, GlobalConfig};
use crate::download::{InstallLock, publish};
use crate::environment::{self, Environment};
use crate::error::Error;
use crate::import::{self, ExternalDependency, ImportLayout, ImportProgress, SourceFingerprint};
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};
use crate::runners::Runners;
use crate::runtime::Runtime;
use crate::stats;
use crate::snapshot;

pub use crate::import::Cancelled as Canceled;

/// Whether files can be reflinked from the source into the folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    Reflink,
    FullCopy,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct CopyInspection {
    pub name: String,
    pub folder: PathBuf,
    pub source_root: PathBuf,
    pub destination: PathBuf,
    pub runner: String,
    pub layout: ImportLayout,
    pub targets: Vec<String>,
    pub snapshots: Vec<String>,
    pub external_dependencies: Vec<ExternalDependency>,
    pub external_target_paths: Vec<String>,
    pub changed_settings: Vec<String>,
    pub literal_source_paths: Vec<String>,
    pub storage: Storage,
    /// The folder has no config dir yet, so the source global config and
    /// templates come along. Otherwise the folder's own are kept.
    pub first_copy: bool,
    pub copies_global_config: bool,
    pub copied_templates: Vec<String>,
    /// Links in snapshots that point outside the prefix and its snapshots,
    /// so they keep pointing at the source.
    pub snapshot_links: Vec<ExternalDependency>,
    /// Whether the source has art, which is copied along.
    pub art: bool,
    source_config: String,
    prefix: import::Inspection,
    config_text: String,
    snapshot_fingerprints: Vec<SourceFingerprint>,
    global_text: Option<String>,
    templates_fingerprint: Option<SourceFingerprint>,
}

/// Reviews copying `name` from the standard layout in `from` into the portable
/// folder `to` selects, without writing anything.
pub fn inspect(
    from: &Paths,
    to: &Paths,
    global: &GlobalConfig,
    name: &str,
) -> Result<CopyInspection> {
    if from.portable_root().is_some() {
        bail!("portable copy reads the standard layout; drop --portable");
    }
    let folder = to
        .portable_root()
        .ok_or_else(|| anyhow!("no portable folder selected"))?
        .to_owned();
    Environment::ensure_exists(from, name).map_err(|err| anyhow!(err))?;
    let source_path = from.environment_config(name);
    let source_config = fs::read_to_string(&source_path)
        .with_context(|| format!("failed to read {}", source_path.display()))?;
    let config =
        EnvironmentConfig::parse(&source_path, &source_config).map_err(|err| anyhow!(err))?;
    let wine = config
        .wine()
        .ok_or_else(|| anyhow!(Error::NoPrefix(name.to_owned())))?;
    let configured_root = from.config_path(&wine.prefix).map_err(|err| anyhow!(err))?;
    let runner = if import::root_layout_ambiguous(&configured_root) {
        Some(
            Runners::new(from, global)
                .map_err(|err| anyhow!(err))?
                .resolve(&wine.runner)
                .map_err(|err| anyhow!(err))?,
        )
    } else {
        None
    };
    let prefix = import::inspect_root(&configured_root, runner.as_ref(), wine.arch)?;
    if paths::resolve(&configured_root) != prefix.source_root {
        bail!("configured prefix root does not match inspected source root");
    }
    let source_root = prefix.source_root.clone();
    let destination = folder.join("prefixes").join(name);
    to.check_prefix(&destination).map_err(|err| anyhow!(err))?;

    let source_snapshots = from.snapshot_root(&source_root, name);
    let source_config_dir = from
        .environments_dir()
        .parent()
        .ok_or_else(|| anyhow!("config dir has no parent"))?
        .to_owned();
    for source in [&source_root, &source_snapshots, &source_config_dir] {
        reject_overlap(source, &folder)?;
    }
    if to.environment_config(name).exists() {
        bail!(
            "environment `{name}` already exists in {}",
            folder.display()
        );
    }
    let destination_snapshots = to.snapshot_root(&destination, name);
    for path in [&destination, &destination_snapshots, &to.art_dir(name)] {
        if fs::symlink_metadata(path).is_ok() {
            bail!(
                "{} already exists; move it aside if a copy was interrupted",
                path.display()
            );
        }
    }
    check_sharing(from, name, &source_root)?;

    let snapshots = snapshot::list(from, name)?;
    let resolved_snapshots = paths::resolve(&source_snapshots);
    let moved = [source_root.as_path(), &resolved_snapshots];
    let mut snapshot_links = Vec::new();
    for item in &snapshots {
        snapshot_links.extend(import::external_links(&item.path, &moved)?);
    }
    let snapshot_fingerprints = snapshots
        .iter()
        .map(|item| import::fingerprint(&item.path))
        .collect::<Result<Vec<_>>>()?;

    let ConfigReview {
        changed_settings,
        external_target_paths,
        literal_source_paths,
    } = clone::review_config(from, &config, &source_root);
    let config_text = clone::transform_config(
        from,
        to,
        &source_config,
        &config,
        &source_root,
        &destination,
        true,
    )?;

    let FirstCopy {
        first_copy,
        global_text,
        copied_templates,
        templates_fingerprint,
    } = first_copy(from, to)?;

    Ok(CopyInspection {
        name: name.to_owned(),
        storage: storage(&prefix.wine_prefix.join("system.reg"), &folder),
        folder,
        source_root,
        destination,
        runner: wine.runner.clone(),
        layout: prefix.layout,
        targets: config.targets.keys().cloned().collect(),
        snapshots: snapshots.into_iter().map(|item| item.label).collect(),
        external_dependencies: prefix
            .external_dependencies
            .iter()
            .filter(|link| !link.target.starts_with(&resolved_snapshots))
            .cloned()
            .collect(),
        external_target_paths,
        changed_settings,
        literal_source_paths,
        first_copy,
        copies_global_config: global_text.is_some(),
        snapshot_links,
        copied_templates,
        art: fs::symlink_metadata(from.art_dir(name)).is_ok(),
        source_config,
        prefix,
        config_text,
        snapshot_fingerprints,
        global_text,
        templates_fingerprint,
    })
}

/// Copies a reviewed environment into the folder. The source is only read.
/// Everything is staged inside the folder first, then published without
/// replacing anything, with the environment config written last.
pub fn execute_reviewed(
    from: &Paths,
    to: &Paths,
    global: &GlobalConfig,
    reviewed: &CopyInspection,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ImportProgress),
) -> Result<Environment> {
    let name = &reviewed.name;
    ensure_same(reviewed, &inspect(from, to, global, name)?)?;
    let source_locks = Locks::new(from);
    // The prefix locks are shared by every root, so they are only taken once.
    let _source = source_locks
        .acquire(Operation::Copy, name, &[])
        .map_err(|err| anyhow!(err))?;
    let locks = Locks::new(to);
    let _operation = locks
        .acquire(
            Operation::Create,
            name,
            &[&reviewed.source_root, &reviewed.destination],
        )
        .map_err(|err| anyhow!(err))?;
    let _source_config = source_locks
        .config(&from.environment_config(name))
        .map_err(|err| anyhow!(err))?;
    ensure_same(reviewed, &inspect(from, to, global, name)?)?;
    for shared in environment::sharing_prefix(from, name, &reviewed.source_root)
        .map_err(|err| anyhow!(err))?
        .names
    {
        Runtime::new(from, &shared)
            .check_quiet(&shared)
            .map_err(|err| anyhow!(err))?;
    }
    Runtime::new(from, name)
        .check_quiet(name)
        .map_err(|err| anyhow!(err))?;
    Runtime::new(to, name)
        .check_quiet(name)
        .map_err(|err| anyhow!(err))?;
    import::check_cancelled(cancel)?;

    let prefixes = reviewed.folder.join("prefixes");
    fs::create_dir_all(&prefixes)
        .with_context(|| format!("failed to create {}", prefixes.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".wineo-copy-")
        .tempdir_in(&prefixes)
        .with_context(|| {
            format!(
                "failed to create staging directory in {}",
                prefixes.display()
            )
        })?;
    let _publish_lock = InstallLock::acquire(&reviewed.destination)?;

    let destination_snapshots = to.snapshot_root(&reviewed.destination, name);
    let source_snapshots = from.snapshot_root(&reviewed.source_root, name);
    let snapshots_moved = (
        paths::resolve(&source_snapshots),
        destination_snapshots.clone(),
    );
    let staged_prefix = staging.path().join("prefix");
    let links = import::Links {
        moved: vec![snapshots_moved.clone()],
        relative: true,
    };
    import::copy_tree_with(
        &reviewed.source_root,
        &staged_prefix,
        &reviewed.destination,
        &links,
        cancel,
        &mut progress,
    )?;
    // A snapshot becomes the live prefix again on restore, so its links into
    // the prefix or its other snapshots follow the copy too.
    let snapshot_links = import::Links {
        moved: vec![
            (reviewed.source_root.clone(), reviewed.destination.clone()),
            snapshots_moved,
        ],
        relative: true,
    };
    let staged_snapshots = staging.path().join("snapshots");
    if !reviewed.snapshots.is_empty() {
        fs::create_dir(&staged_snapshots)?;
    }
    for label in &reviewed.snapshots {
        let staged = staged_snapshots.join(label);
        import::copy_tree_with(
            &source_snapshots.join(label),
            &staged,
            &destination_snapshots.join(label),
            &snapshot_links,
            cancel,
            &mut progress,
        )?;
        let layout = snapshot::relocate_manifest(to, &staged, &reviewed.destination)?;
        let saved_path = staged.join("config.toml");
        let saved_text = fs::read_to_string(&saved_path)?;
        let saved =
            EnvironmentConfig::parse(&saved_path, &saved_text).map_err(|err| anyhow!(err))?;
        let relocated = clone::transform_config(
            from,
            to,
            &saved_text,
            &saved,
            &reviewed.source_root,
            &reviewed.destination,
            true,
        )?;
        fs::write(&saved_path, relocated)?;
        import::validate_staged_prefix(
            &staged.join("prefix"),
            &destination_snapshots.join(label).join("prefix"),
            layout,
        )?;
    }
    let staged_templates = staging.path().join("templates");
    if reviewed.templates_fingerprint.is_some() {
        import::copy_tree(
            &from.templates_dir(),
            &staged_templates,
            &to.templates_dir(),
            cancel,
            &mut progress,
        )?;
    }
    import::check_cancelled(cancel)?;

    if import::fingerprint(&reviewed.source_root)? != reviewed.prefix.fingerprint {
        bail!(
            "{} changed while it was being copied; try again",
            reviewed.source_root.display()
        );
    }
    for (label, expected) in reviewed
        .snapshots
        .iter()
        .zip(&reviewed.snapshot_fingerprints)
    {
        if import::fingerprint(&source_snapshots.join(label))? != *expected {
            bail!("snapshot `{label}` changed while it was being copied; try again");
        }
    }
    if let Some(expected) = reviewed.templates_fingerprint
        && import::fingerprint(&from.templates_dir())? != expected
    {
        bail!("templates changed while they were being copied; try again");
    }
    if fs::read_to_string(from.environment_config(name))? != reviewed.source_config {
        bail!("source environment config changed while it was being copied; try again");
    }
    import::validate_staged_prefix(&staged_prefix, &reviewed.destination, reviewed.layout)?;

    let mut published: Vec<PathBuf> = Vec::new();
    let recovery = |published: &[PathBuf], err: anyhow::Error| {
        if published.is_empty() {
            return err;
        }
        let paths: Vec<String> = published
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        anyhow!(
            "{err}; copied files remain at {}; move them aside and retry",
            paths.join(", ")
        )
    };
    publish(&staged_prefix, &reviewed.destination)?;
    published.push(reviewed.destination.clone());
    let result = (|| -> Result<()> {
        if !reviewed.snapshots.is_empty() {
            let parent = destination_snapshots
                .parent()
                .ok_or_else(|| anyhow!("snapshot storage has no parent"))?;
            fs::create_dir_all(parent)?;
            publish(&staged_snapshots, &destination_snapshots)?;
            published.push(destination_snapshots.clone());
        }
        if reviewed.templates_fingerprint.is_some() {
            fs::create_dir_all(
                to.templates_dir()
                    .parent()
                    .expect("templates have a parent"),
            )?;
            publish(&staged_templates, &to.templates_dir())?;
            published.push(to.templates_dir());
        }
        let art = StagedArt::stage(&source_locks, &from.art_dir(name), &to.art_dir(name))
            .and_then(StagedArt::publish)?;
        published.extend(art);
        published.extend(stats::copy_log(from, to, name)?);
        if let Some(text) = &reviewed.global_text {
            let path = to.global_config();
            let _guard = locks.config(&path).map_err(|err| anyhow!(err))?;
            write_once(&path, text)?;
            published.push(path);
        }
        let config_path = to.environment_config(name);
        let _guard = locks.config(&config_path).map_err(|err| anyhow!(err))?;
        import::write_config_once(&config_path, &reviewed.config_text)
    })();
    result.map_err(|err| recovery(&published, err))?;
    progress(ImportProgress::Finished {
        entries: reviewed.prefix.fingerprint.entries,
    });
    Environment::load(to, name).map_err(|err| anyhow!(err))
}

/// What the first copy into a folder brings along besides the environment:
/// the source global config and templates. Later copies keep the folder's own.
struct FirstCopy {
    first_copy: bool,
    global_text: Option<String>,
    copied_templates: Vec<String>,
    templates_fingerprint: Option<SourceFingerprint>,
}

fn first_copy(from: &Paths, to: &Paths) -> Result<FirstCopy> {
    let config_dir = to
        .environments_dir()
        .parent()
        .ok_or_else(|| anyhow!("config dir has no parent"))?
        .to_owned();
    let first_copy = fs::symlink_metadata(&config_dir).is_err();
    let global_text = if !first_copy {
        None
    } else {
        match fs::read_to_string(from.global_config()) {
            Ok(text) => Some(portable_global(to, &from.global_config(), &text)?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read {}", from.global_config().display()));
            }
        }
    };
    let (copied_templates, templates_fingerprint) = if !first_copy || !from.templates_dir().is_dir()
    {
        (Vec::new(), None)
    } else {
        let mut names: Vec<String> = fs::read_dir(from.templates_dir())?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .filter_map(|file| file.strip_suffix(".toml").map(str::to_owned))
            .collect();
        names.sort();
        (names, Some(import::fingerprint(&from.templates_dir())?))
    };
    Ok(FirstCopy {
        first_copy,
        global_text,
        copied_templates,
        templates_fingerprint,
    })
}

/// A reviewed copy of a native environment into a portable folder: its
/// config and art, and on a first copy the global config and templates.
#[derive(Debug, Clone)]
pub struct NativeCopyInspection {
    pub name: String,
    pub folder: PathBuf,
    pub targets: Vec<String>,
    pub changed_settings: Vec<String>,
    /// Target paths outside the folder, which keep pointing at the host.
    pub external_target_paths: Vec<String>,
    pub first_copy: bool,
    pub copies_global_config: bool,
    pub copied_templates: Vec<String>,
    /// Whether the source has art, which is copied along.
    pub art: bool,
    source_config: String,
    config_text: String,
    global_text: Option<String>,
    templates_fingerprint: Option<SourceFingerprint>,
}

/// Reviews copying the native environment `name` into the folder `to` selects.
pub fn inspect_native(from: &Paths, to: &Paths, name: &str) -> Result<NativeCopyInspection> {
    if from.portable_root().is_some() {
        bail!("portable copy reads the standard layout; drop --portable");
    }
    let folder = to
        .portable_root()
        .ok_or_else(|| anyhow!("no portable folder selected"))?
        .to_owned();
    Environment::ensure_exists(from, name).map_err(|err| anyhow!(err))?;
    let source_path = from.environment_config(name);
    let source_config = fs::read_to_string(&source_path)
        .with_context(|| format!("failed to read {}", source_path.display()))?;
    let config =
        EnvironmentConfig::parse(&source_path, &source_config).map_err(|err| anyhow!(err))?;
    if config.wine().is_some() {
        bail!("{name} is not a native environment");
    }
    let source_config_dir = from
        .environments_dir()
        .parent()
        .ok_or_else(|| anyhow!("config dir has no parent"))?
        .to_owned();
    reject_overlap(&source_config_dir, &folder)?;
    if to.environment_config(name).exists() {
        bail!(
            "environment `{name}` already exists in {}",
            folder.display()
        );
    }
    if fs::symlink_metadata(to.art_dir(name)).is_ok() {
        bail!(
            "{} already exists; move it aside if a copy was interrupted",
            to.art_dir(name).display()
        );
    }
    let (changed_settings, config_text) = clone::native_config(&source_config, &config, true)?;
    let mut external_target_paths = Vec::new();
    for (key, target) in &config.targets {
        for (field, raw) in [("exe", Some(&target.exe)), ("cwd", target.cwd.as_ref())] {
            let Some(raw) = raw else { continue };
            // A bare name is looked up on PATH wherever it runs.
            if !raw.contains('/') && !raw.starts_with('~') {
                continue;
            }
            let path = from.expand(raw);
            if !path.is_absolute() || !paths::resolve(&path).starts_with(&folder) {
                external_target_paths.push(format!("targets.{key}.{field}: {raw}"));
            }
        }
    }
    let FirstCopy {
        first_copy,
        global_text,
        copied_templates,
        templates_fingerprint,
    } = first_copy(from, to)?;
    Ok(NativeCopyInspection {
        name: name.to_owned(),
        folder,
        targets: config.targets.keys().cloned().collect(),
        changed_settings,
        external_target_paths,
        first_copy,
        copies_global_config: global_text.is_some(),
        copied_templates,
        art: fs::symlink_metadata(from.art_dir(name)).is_ok(),
        source_config,
        config_text,
        global_text,
        templates_fingerprint,
    })
}

/// Copies a reviewed native environment into the folder, with its config
/// written last so a failure never leaves it half registered.
pub fn execute_native(
    from: &Paths,
    to: &Paths,
    reviewed: &NativeCopyInspection,
    cancel: &AtomicBool,
) -> Result<Environment> {
    let name = &reviewed.name;
    let source_locks = Locks::new(from);
    let _source = source_locks
        .acquire(Operation::Copy, name, &[])
        .map_err(|err| anyhow!(err))?;
    let locks = Locks::new(to);
    let _operation = locks
        .acquire(Operation::Create, name, &[])
        .map_err(|err| anyhow!(err))?;
    let _source_config = source_locks
        .config(&from.environment_config(name))
        .map_err(|err| anyhow!(err))?;
    let current = inspect_native(from, to, name)?;
    if current.source_config != reviewed.source_config
        || current.config_text != reviewed.config_text
        || current.global_text != reviewed.global_text
        || current.templates_fingerprint != reviewed.templates_fingerprint
    {
        bail!("the reviewed environment changed; inspect it again before copying");
    }
    import::check_cancelled(cancel)?;

    let mut published: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<()> {
        if reviewed.templates_fingerprint.is_some() {
            let parent = to.templates_dir();
            let parent = parent.parent().expect("templates have a parent");
            fs::create_dir_all(parent)?;
            let staging = tempfile::Builder::new()
                .prefix(".wineo-copy-")
                .tempdir_in(parent)?;
            let staged = staging.path().join("templates");
            import::copy_tree(
                &from.templates_dir(),
                &staged,
                &to.templates_dir(),
                cancel,
                &mut |_| {},
            )?;
            if reviewed.templates_fingerprint != Some(import::fingerprint(&from.templates_dir())?) {
                bail!("templates changed while they were being copied; try again");
            }
            publish(&staged, &to.templates_dir())?;
            published.push(to.templates_dir());
        }
        let art = StagedArt::stage(&source_locks, &from.art_dir(name), &to.art_dir(name))
            .and_then(StagedArt::publish)?;
        published.extend(art);
        published.extend(stats::copy_log(from, to, name)?);
        if let Some(text) = &reviewed.global_text {
            let path = to.global_config();
            let _guard = locks.config(&path).map_err(|err| anyhow!(err))?;
            write_once(&path, text)?;
            published.push(path);
        }
        let config_path = to.environment_config(name);
        let _guard = locks.config(&config_path).map_err(|err| anyhow!(err))?;
        import::write_config_once(&config_path, &reviewed.config_text)
    })();
    result.map_err(|err| {
        if published.is_empty() {
            return err;
        }
        let paths: Vec<String> = published
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        anyhow!(
            "{err}; copied files remain at {}; move them aside and retry",
            paths.join(", ")
        )
    })?;
    Environment::load(to, name).map_err(|err| anyhow!(err))
}

/// The source global config for a portable folder: its prefix root follows
/// the folder, and everything else, like external runner roots, is kept.
fn portable_global(to: &Paths, path: &Path, text: &str) -> Result<String> {
    GlobalConfig::parse(path, text).map_err(|err| anyhow!(err))?;
    let mut doc: DocumentMut = text.parse()?;
    if let Some(defaults) = doc
        .get_mut("defaults")
        .and_then(|item| item.as_table_like_mut())
        && defaults.contains_key("prefix_root")
    {
        let root = to
            .store(&to.default_prefix_root())
            .map_err(|err| anyhow!(err))?;
        defaults.insert("prefix_root", toml_edit::value(Value::from(root)));
    }
    let updated = doc.to_string();
    if updated == text {
        return Ok(updated);
    }
    config::stamp_schema(&mut doc);
    let updated = doc.to_string();
    GlobalConfig::parse(path, &updated).map_err(|err| anyhow!(err))?;
    Ok(updated)
}

fn write_once(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("config has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn reject_overlap(source: &Path, folder: &Path) -> Result<()> {
    let source = paths::resolve(source);
    let folder = paths::resolve(folder);
    if source.starts_with(&folder) || folder.starts_with(&source) {
        bail!(
            "{} overlaps the portable folder {}",
            source.display(),
            folder.display()
        );
    }
    Ok(())
}

fn check_sharing(from: &Paths, name: &str, prefix: &Path) -> Result<()> {
    let shared = environment::sharing_prefix(from, name, prefix).map_err(|err| anyhow!(err))?;
    if !shared.unreadable.is_empty() {
        bail!(
            "could not inspect environments: {}",
            shared.unreadable.join(", ")
        );
    }
    Ok(())
}

/// Tries a reflink from `sample` into the nearest existing directory at or
/// above `folder`, removing the probe right away.
fn storage(sample: &Path, folder: &Path) -> Storage {
    let Some(dir) = folder.ancestors().find(|dir| dir.is_dir()) else {
        return Storage::Unknown;
    };
    let Ok(input) = File::open(sample) else {
        return Storage::Unknown;
    };
    let Ok(output) = tempfile::Builder::new()
        .prefix(".wineo-reflink-")
        .tempfile_in(dir)
    else {
        return Storage::Unknown;
    };
    if rustix::fs::ioctl_ficlone(output.as_file(), &input).is_ok() {
        Storage::Reflink
    } else {
        Storage::FullCopy
    }
}

fn ensure_same(expected: &CopyInspection, actual: &CopyInspection) -> Result<()> {
    import::ensure_same_review(&expected.prefix, &actual.prefix)?;
    if expected.source_config != actual.source_config
        || expected.config_text != actual.config_text
        || expected.source_root != actual.source_root
        || expected.destination != actual.destination
        || expected.snapshots != actual.snapshots
        || expected.snapshot_fingerprints != actual.snapshot_fingerprints
        || expected.global_text != actual.global_text
        || expected.templates_fingerprint != actual.templates_fingerprint
    {
        bail!("the reviewed environment changed; inspect it again before copying");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn paths(home: &Path, portable: Option<&Path>) -> Paths {
        let home = home.to_owned();
        Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            portable,
        )
        .unwrap()
    }

    #[test]
    fn a_canceled_copy_publishes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let from = paths(root.path(), None);
        let usb = root.path().join("usb");
        let to = paths(root.path(), Some(&usb));
        let prefix = root.path().join("prefix");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        fs::create_dir_all(from.environments_dir()).unwrap();
        fs::write(
            from.environment_config("game"),
            format!(
                "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
                prefix.display()
            ),
        )
        .unwrap();
        let global = GlobalConfig::default();
        let reviewed = inspect(&from, &to, &global, "game").unwrap();
        let err = execute_reviewed(
            &from,
            &to,
            &global,
            &reviewed,
            &AtomicBool::new(true),
            |_| {},
        )
        .unwrap_err();
        assert!(err.is::<Canceled>(), "{err}");
        assert!(!usb.join("prefixes/game").exists());
        assert!(!to.environment_config("game").exists());
        assert!(!usb.join("config").exists());
        assert!(inspect(&from, &to, &global, "game").is_ok());
    }
}
