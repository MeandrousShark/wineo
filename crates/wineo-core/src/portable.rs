//! Copying an environment from the standard layout into a portable folder.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use toml_edit::{DocumentMut, Value};

use crate::art::StagedArt;
use crate::build_lock;
use crate::cancel::Token;
use crate::clone::{self, ConfigReview};
use crate::config::{self, EnvironmentConfig, GlobalConfig, Kind};
use crate::download::{InstallLock, publish};
use crate::environment::{self, Environment};
use crate::import::{self, ExternalDependency, ImportLayout, ImportProgress, SourceFingerprint};
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};
use crate::runners::Runners;
use crate::runtime::Runtime;
use crate::snapshot;
use crate::stage::Stage;
use crate::stats;
use crate::templates::{NEWEST_PROTON, Templates};

pub use crate::import::Cancelled as Canceled;

/// Progress and the point after which a portable copy cannot be canceled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyEvent {
    Progress(ImportProgress),
    /// Publication starts now. Later cancel requests do not stop the copy.
    Publishing,
}

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
    inspect_with(from, to, global, name, &Token::new(), |_| {})
}

/// Reviews a Wine copy with cancellable source checks and checking progress.
pub fn inspect_with(
    from: &Paths,
    to: &Paths,
    global: &GlobalConfig,
    name: &str,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<CopyInspection> {
    import::check_cancelled(cancel)?;
    if from.portable_root().is_some() {
        bail!("portable copy reads the standard layout; drop --portable");
    }
    let folder = to
        .portable_root()
        .ok_or_else(|| anyhow!("no portable folder selected"))?
        .to_owned();
    Environment::ensure_exists(from, name)?;
    let source_path = from.environment_config(name);
    let source_config = fs::read_to_string(&source_path)
        .with_context(|| format!("failed to read {}", source_path.display()))?;
    let config = EnvironmentConfig::parse(&source_path, &source_config)?;
    let wine = config.require_wine(name)?;
    let configured_root = from.config_path(&wine.prefix)?;
    let (runner, _runner_guard) = if import::root_layout_ambiguous(&configured_root) {
        let (runner, guard) = Runners::new(from, global)?.resolve_locked(from, &wine.runner)?;
        (Some(runner), guard)
    } else {
        (None, None)
    };
    let prefix = import::inspect_root_with(
        &configured_root,
        runner.as_ref(),
        wine.arch,
        cancel,
        &mut progress,
    )?;
    if paths::resolve(&configured_root) != prefix.source_root {
        bail!("configured prefix root does not match inspected source root");
    }
    let source_root = prefix.source_root.clone();
    let destination = folder.join("prefixes").join(name);
    to.check_prefix(&destination)?;

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
    for path in [
        &destination,
        &destination_snapshots,
        &to.art_dir(name),
        &to.stats_log(name),
    ] {
        if fs::symlink_metadata(path).is_ok() {
            bail!(
                "{} already exists; move it aside if a copy was interrupted",
                path.display()
            );
        }
    }
    environment::checked_sharing(from, name, &source_root)?;
    environment::check_destination(to, name, &destination)?;

    let snapshots = snapshot::list(from, name)?;
    let resolved_snapshots = paths::resolve(&source_snapshots);
    let moved = [source_root.as_path(), &resolved_snapshots];
    let mut snapshot_links = Vec::new();
    for item in &snapshots {
        snapshot_links.extend(import::external_links_with(
            &item.path,
            &moved,
            cancel,
            &mut progress,
        )?);
    }
    let snapshot_fingerprints = snapshots
        .iter()
        .map(|item| import::fingerprint_with(&item.path, cancel, &mut progress))
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
    } = first_copy(from, to, cancel, &mut progress)?;

    import::check_cancelled(cancel)?;
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
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<Environment> {
    execute_reviewed_with(from, to, global, reviewed, cancel, |event| {
        if let CopyEvent::Progress(event) = event {
            if let ImportProgress::Copied { path } = &event
                && path.starts_with(from.art_dir(&reviewed.name))
            {
                return;
            }
            progress(event);
        }
    })
}

/// Executes a Wine copy with cancellable revalidation and a publishing boundary.
pub fn execute_reviewed_with(
    from: &Paths,
    to: &Paths,
    global: &GlobalConfig,
    reviewed: &CopyInspection,
    cancel: &Token,
    mut events: impl FnMut(CopyEvent),
) -> Result<Environment> {
    let name = &reviewed.name;
    // The review is checked once, under the locks. This cheap check keeps an
    // interrupted restore of the source reported as one, since the copy lock
    // would otherwise refuse it first with a busy error.
    Environment::ensure_exists(from, name)?;
    let source_locks = Locks::new(from);
    // The prefix locks are shared by every root, so they are only taken once.
    let _source = source_locks.acquire(Operation::Copy, name, &[])?;
    let locks = Locks::new(to);
    let _operation = locks.acquire(
        Operation::Create,
        name,
        &[&reviewed.source_root, &reviewed.destination],
    )?;
    let _source_config = source_locks.config(&from.environment_config(name))?;
    ensure_same(
        reviewed,
        &inspect_with(from, to, global, name, cancel, |event| {
            events(CopyEvent::Progress(event))
        })?,
    )?;
    for shared in environment::sharing_prefix(from, name, &reviewed.source_root)?.names {
        Runtime::new(from, &shared).check_quiet(&shared)?;
    }
    Runtime::new(from, name).check_quiet(name)?;
    Runtime::new(to, name).check_quiet(name)?;
    import::check_cancelled(cancel)?;

    if reviewed.destination.exists() {
        bail!(
            "portable copy destination appeared during review: {}",
            reviewed.destination.display()
        );
    }
    let staging = Stage::new(".wineo-copy-", &reviewed.folder.join("prefixes"))?;
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
        &mut |event| events(CopyEvent::Progress(event)),
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
            &mut |event| events(CopyEvent::Progress(event)),
        )?;
        let layout = snapshot::relocate_manifest(to, &staged, &reviewed.destination)?;
        let saved_path = staged.join("config.toml");
        let saved_text = fs::read_to_string(&saved_path)?;
        let saved = EnvironmentConfig::parse(&saved_path, &saved_text)?;
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
            &mut |event| events(CopyEvent::Progress(event)),
        )?;
    }
    import::check_cancelled(cancel)?;

    if import::fingerprint_with(&reviewed.source_root, cancel, &mut |event| {
        events(CopyEvent::Progress(event))
    })? != reviewed.prefix.fingerprint
    {
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
        if import::fingerprint_with(&source_snapshots.join(label), cancel, &mut |event| {
            events(CopyEvent::Progress(event))
        })? != *expected
        {
            bail!("snapshot `{label}` changed while it was being copied; try again");
        }
    }
    if let Some(expected) = reviewed.templates_fingerprint
        && import::fingerprint_with(&from.templates_dir(), cancel, &mut |event| {
            events(CopyEvent::Progress(event))
        })? != expected
    {
        bail!("templates changed while they were being copied; try again");
    }
    if fs::read_to_string(from.environment_config(name))? != reviewed.source_config {
        bail!("source environment config changed while it was being copied; try again");
    }
    import::validate_staged_prefix(&staged_prefix, &reviewed.destination, reviewed.layout)?;

    let art = StagedArt::stage_with(
        &source_locks,
        &from.art_dir(name),
        &to.art_dir(name),
        staging.path(),
        cancel,
        &mut |event| events(CopyEvent::Progress(event)),
    )?;
    import::check_cancelled(cancel)?;
    events(CopyEvent::Publishing);

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
    if reviewed.destination.exists() {
        bail!(
            "portable copy destination appeared during publication: {}",
            reviewed.destination.display()
        );
    }
    staging.publish_part(&staged_prefix, &reviewed.destination)?;
    published.push(reviewed.destination.clone());
    let result = (|| -> Result<()> {
        if !reviewed.snapshots.is_empty() {
            let parent = destination_snapshots
                .parent()
                .ok_or_else(|| anyhow!("snapshot storage has no parent"))?;
            fs::create_dir_all(parent)?;
            staging.publish_part(&staged_snapshots, &destination_snapshots)?;
            published.push(destination_snapshots.clone());
        }
        if reviewed.templates_fingerprint.is_some() {
            fs::create_dir_all(
                to.templates_dir()
                    .parent()
                    .expect("templates have a parent"),
            )?;
            let destination_global = destination_global(to, reviewed.global_text.as_deref())?;
            let _template_guards =
                template_build_guards(to, &destination_global, &staged_templates)?;
            staging.publish_part(&staged_templates, &to.templates_dir())?;
            published.push(to.templates_dir());
        }
        published.extend(art.publish()?);
        published.extend(stats::copy_log(from, to, name)?);
        for path in &published {
            import::sync_filesystem(path)?;
        }
        if let Some(text) = &reviewed.global_text {
            let path = to.global_config();
            let _guard = locks.config(&path)?;
            write_global_once(to, text)?;
            published.push(path);
        }
        let config_path = to.environment_config(name);
        let _guard = locks.config(&config_path)?;
        let destination_global = destination_global(to, reviewed.global_text.as_deref())?;
        write_environment_once(to, &destination_global, name, &reviewed.config_text)
    })();
    result.map_err(|err| recovery(&published, err))?;
    events(CopyEvent::Progress(ImportProgress::Finished {
        entries: reviewed.prefix.fingerprint.entries,
    }));
    Ok(Environment::load(to, name)?)
}

/// What the first copy into a folder brings along besides the environment:
/// the source global config and templates. Later copies keep the folder's own.
struct FirstCopy {
    first_copy: bool,
    global_text: Option<String>,
    copied_templates: Vec<String>,
    templates_fingerprint: Option<SourceFingerprint>,
}

fn first_copy(
    from: &Paths,
    to: &Paths,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<FirstCopy> {
    import::check_cancelled(cancel)?;
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
        let mut names = Vec::new();
        for entry in fs::read_dir(from.templates_dir())? {
            import::check_cancelled(cancel)?;
            if let Ok(entry) = entry
                && let Some(file) = entry.file_name().to_str()
                && let Some(name) = file.strip_suffix(".toml")
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        (
            names,
            Some(import::fingerprint_with(
                &from.templates_dir(),
                cancel,
                progress,
            )?),
        )
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
    inspect_native_with(from, to, name, &Token::new(), |_| {})
}

/// Reviews a native copy with cancellable template checks and checking progress.
pub fn inspect_native_with(
    from: &Paths,
    to: &Paths,
    name: &str,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<NativeCopyInspection> {
    import::check_cancelled(cancel)?;
    if from.portable_root().is_some() {
        bail!("portable copy reads the standard layout; drop --portable");
    }
    let folder = to
        .portable_root()
        .ok_or_else(|| anyhow!("no portable folder selected"))?
        .to_owned();
    Environment::ensure_exists(from, name)?;
    let source_path = from.environment_config(name);
    let source_config = fs::read_to_string(&source_path)
        .with_context(|| format!("failed to read {}", source_path.display()))?;
    let config = EnvironmentConfig::parse(&source_path, &source_config)?;
    match &config.kind {
        Kind::Native => {}
        Kind::Wine(_) => bail!("{name} is not a native environment"),
        Kind::Proxied(_) => return Err(anyhow!(config.no_prefix(name))),
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
    for path in [to.art_dir(name), to.stats_log(name)] {
        if fs::symlink_metadata(&path).is_ok() {
            bail!(
                "{} already exists; move it aside if a copy was interrupted",
                path.display()
            );
        }
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
    } = first_copy(from, to, cancel, &mut progress)?;
    import::check_cancelled(cancel)?;
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
    cancel: &Token,
) -> Result<Environment> {
    execute_native_with(from, to, reviewed, cancel, |_| {})
}

/// Executes a native copy with copy progress and a publishing boundary.
pub fn execute_native_with(
    from: &Paths,
    to: &Paths,
    reviewed: &NativeCopyInspection,
    cancel: &Token,
    mut events: impl FnMut(CopyEvent),
) -> Result<Environment> {
    let copied = std::cell::Cell::new(0);
    let mut events = |event| {
        if matches!(event, CopyEvent::Progress(ImportProgress::Copied { .. })) {
            copied.set(copied.get() + 1);
        }
        events(event);
    };
    let name = &reviewed.name;
    let source_locks = Locks::new(from);
    let _source = source_locks.acquire(Operation::Copy, name, &[])?;
    let locks = Locks::new(to);
    let _operation = locks.acquire(Operation::Create, name, &[])?;
    let _source_config = source_locks.config(&from.environment_config(name))?;
    let current = inspect_native_with(from, to, name, cancel, |event| {
        events(CopyEvent::Progress(event))
    })?;
    if current.source_config != reviewed.source_config
        || current.config_text != reviewed.config_text
        || current.global_text != reviewed.global_text
        || current.templates_fingerprint != reviewed.templates_fingerprint
    {
        bail!("the reviewed environment changed; inspect it again before copying");
    }
    import::check_cancelled(cancel)?;

    fs::create_dir_all(&reviewed.folder)?;
    let staging = import::Staging::new_in(".wineo-copy-", &reviewed.folder)?;
    let staged_templates = staging.path().join("templates");
    if reviewed.templates_fingerprint.is_some() {
        import::copy_tree(
            &from.templates_dir(),
            &staged_templates,
            &to.templates_dir(),
            cancel,
            &mut |event| events(CopyEvent::Progress(event)),
        )?;
        if reviewed.templates_fingerprint
            != Some(import::fingerprint_with(
                &from.templates_dir(),
                cancel,
                &mut |event| events(CopyEvent::Progress(event)),
            )?)
        {
            bail!("templates changed while they were being copied; try again");
        }
    }
    let art = StagedArt::stage_with(
        &source_locks,
        &from.art_dir(name),
        &to.art_dir(name),
        staging.path(),
        cancel,
        &mut |event| events(CopyEvent::Progress(event)),
    )?;
    import::check_cancelled(cancel)?;
    events(CopyEvent::Publishing);

    let mut published: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<()> {
        if reviewed.templates_fingerprint.is_some() {
            fs::create_dir_all(
                to.templates_dir()
                    .parent()
                    .expect("templates have a parent"),
            )?;
            let staged = &staged_templates;
            let destination_global = destination_global(to, reviewed.global_text.as_deref())?;
            let _template_guards = template_build_guards(to, &destination_global, staged)?;
            publish(staged, &to.templates_dir())?;
            published.push(to.templates_dir());
        }
        published.extend(art.publish()?);
        published.extend(stats::copy_log(from, to, name)?);
        for path in &published {
            import::sync_filesystem(path)?;
        }
        if let Some(text) = &reviewed.global_text {
            let path = to.global_config();
            let _guard = locks.config(&path)?;
            write_global_once(to, text)?;
            published.push(path);
        }
        let config_path = to.environment_config(name);
        let _guard = locks.config(&config_path)?;
        let destination_global = destination_global(to, reviewed.global_text.as_deref())?;
        write_environment_once(to, &destination_global, name, &reviewed.config_text)
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
    events(CopyEvent::Progress(ImportProgress::Finished {
        entries: copied.get(),
    }));
    Ok(Environment::load(to, name)?)
}

fn destination_global(to: &Paths, copied: Option<&str>) -> Result<GlobalConfig> {
    match copied {
        Some(text) => GlobalConfig::parse(&to.global_config(), text).map_err(Into::into),
        None => Ok(GlobalConfig::load_or_default(to).unwrap_or_default()),
    }
}

fn template_build_guards(
    to: &Paths,
    global: &GlobalConfig,
    dir: &Path,
) -> Result<Vec<build_lock::Guard>> {
    let templates = Templates::at(dir.to_owned());
    let mut guards = Vec::new();
    for row in templates.list()?.into_iter().filter(|row| !row.built_in) {
        let Ok(template) = templates.resolve(&row.name) else {
            continue;
        };
        let runner = if template.runner.as_deref() == Some(NEWEST_PROTON) {
            Runners::new(to, global).ok().and_then(|runners| {
                template
                    .runner_name(&runners, &global.defaults.runner)
                    .ok()
                    .flatten()
            })
        } else {
            template.runner.clone()
        };
        guards.extend(build_lock::config(
            to,
            global,
            runner.as_deref(),
            template.dxvk.as_deref(),
            template.vkd3d.as_deref(),
        )?);
    }
    Ok(guards)
}

fn write_global_once(paths: &Paths, text: &str) -> Result<()> {
    let path = paths.global_config();
    let global = GlobalConfig::parse(&path, text)?;
    let _guards = build_lock::config(paths, &global, Some(&global.defaults.runner), None, None)?;
    import::write_config_once(&path, text)
}

fn write_environment_once(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    text: &str,
) -> Result<()> {
    let path = paths.environment_config(name);
    let config = EnvironmentConfig::parse(&path, text)?;
    let _guards = match config.wine() {
        Some(wine) => build_lock::config(
            paths,
            global,
            Some(&wine.runner),
            wine.dxvk.as_deref(),
            wine.vkd3d.as_deref(),
        )?,
        None => Vec::new(),
    };
    import::write_config_once(&path, text)
}

/// The source global config for a portable folder: its prefix root follows
/// the folder, and everything else, like external runner roots, is kept.
fn portable_global(to: &Paths, path: &Path, text: &str) -> Result<String> {
    GlobalConfig::parse(path, text)?;
    let mut doc: DocumentMut = text.parse()?;
    if let Some(defaults) = doc
        .get_mut("defaults")
        .and_then(|item| item.as_table_like_mut())
        && defaults.contains_key("prefix_root")
    {
        let root = to.store(&to.default_prefix_root())?;
        defaults.insert("prefix_root", toml_edit::value(Value::from(root)));
    }
    let updated = doc.to_string();
    if updated == text {
        return Ok(updated);
    }
    config::stamp_schema(&mut doc);
    let updated = doc.to_string();
    GlobalConfig::parse(path, &updated)?;
    Ok(updated)
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
        let canceled = Token::new();
        canceled.cancel();
        let err = execute_reviewed(&from, &to, &global, &reviewed, &canceled, |_| {}).unwrap_err();
        assert!(err.is::<Canceled>(), "{err}");
        assert!(!usb.join("prefixes/game").exists());
        assert!(!to.environment_config("game").exists());
        assert!(!usb.join("config").exists());
        assert!(inspect(&from, &to, &global, "game").is_ok());
    }
    struct Fixture {
        _root: tempfile::TempDir,
        from: Paths,
        to: Paths,
        global: GlobalConfig,
        prefix: PathBuf,
    }

    impl Fixture {
        fn new(native: bool) -> Self {
            let root = tempfile::tempdir().unwrap();
            let from = paths(root.path(), None);
            let to = paths(root.path(), Some(&root.path().join("usb")));
            let prefix = root.path().join("prefix");
            fs::create_dir_all(prefix.join("drive_c")).unwrap();
            fs::write(
                prefix.join("system.reg"),
                "WINE REGISTRY Version 2\n#arch=win64\n",
            )
            .unwrap();
            fs::write(prefix.join("drive_c/data"), "data").unwrap();
            fs::create_dir_all(from.environments_dir()).unwrap();
            let text = if native {
                "kind = 'native'\n[targets.app]\nexe = '/opt/app'\ndesktop = true\n".to_owned()
            } else {
                format!(
                    "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
                    prefix.display()
                )
            };
            fs::write(from.environment_config("game"), text).unwrap();
            fs::create_dir_all(from.templates_dir()).unwrap();
            fs::write(from.templates_dir().join("work.toml"), "gamemode = true\n").unwrap();
            fs::write(from.global_config(), "# global\n").unwrap();
            fs::create_dir_all(from.art_dir("game").join("app")).unwrap();
            fs::write(from.art_dir("game").join("hero.png"), "art").unwrap();
            std::os::unix::fs::symlink("../hero.png", from.art_dir("game").join("app/link"))
                .unwrap();
            Self {
                _root: root,
                from,
                to,
                global: GlobalConfig::default(),
                prefix,
            }
        }

        fn assert_unpublished(&self) {
            assert!(!self.to.environment_config("game").exists());
            assert!(!self.to.default_prefix_root().join("game").exists());
            assert!(!self.to.art_dir("game").exists());
            assert!(!self.to.templates_dir().exists());
            assert!(!self.to.global_config().parent().unwrap().exists());
            for directory in [
                self.to.default_prefix_root(),
                self.to.portable_root().unwrap().to_owned(),
            ] {
                if directory.exists() {
                    for entry in fs::read_dir(directory).unwrap() {
                        let name = entry.unwrap().file_name();
                        assert!(!name.to_string_lossy().starts_with(".wineo-copy-"));
                        assert!(!name.to_string_lossy().starts_with(".wineo-art-"));
                    }
                }
            }
        }

        fn assert_no_staging(&self) {
            for directory in [
                self.to.default_prefix_root(),
                self.to.portable_root().unwrap().to_owned(),
            ] {
                for entry in fs::read_dir(directory).unwrap() {
                    let name = entry.unwrap().file_name();
                    assert!(!name.to_string_lossy().starts_with(".wineo-copy-"));
                    assert!(!name.to_string_lossy().starts_with(".wineo-art-"));
                }
            }
            assert!(!self.to.environment_config("game").exists());
        }
    }

    #[test]
    fn inspection_cancels_wine_and_native_without_destination_writes() {
        for native in [false, true] {
            let f = Fixture::new(native);
            let cancel = Token::new();
            let mut progress = |event| {
                assert!(matches!(event, ImportProgress::Checking { .. }));
                cancel.cancel();
            };
            let err = if native {
                inspect_native_with(&f.from, &f.to, "game", &cancel, &mut progress).unwrap_err()
            } else {
                inspect_with(&f.from, &f.to, &f.global, "game", &cancel, &mut progress).unwrap_err()
            };
            assert!(err.is::<Canceled>(), "{err}");
            f.assert_unpublished();
            assert!(!f.to.portable_root().unwrap().exists());
        }
    }

    #[test]
    fn copy_cancellation_cleans_prefix_template_and_art_staging() {
        for native in [false, true] {
            for part in ["prefix", "templates", "art"] {
                if native && part == "prefix" {
                    continue;
                }
                let f = Fixture::new(native);
                let cancel = Token::new();
                let source = match part {
                    "prefix" => f.prefix.clone(),
                    "templates" => f.from.templates_dir(),
                    _ => f.from.art_dir("game"),
                };
                let mut canceled = false;
                let mut events = |event| {
                    assert_ne!(event, CopyEvent::Publishing);
                    if let CopyEvent::Progress(ImportProgress::Copied { path }) = event
                        && path.starts_with(&source)
                    {
                        canceled = true;
                        cancel.cancel();
                    }
                };
                let err = if native {
                    let review = inspect_native(&f.from, &f.to, "game").unwrap();
                    execute_native_with(&f.from, &f.to, &review, &cancel, &mut events).unwrap_err()
                } else {
                    let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
                    execute_reviewed_with(&f.from, &f.to, &f.global, &review, &cancel, &mut events)
                        .unwrap_err()
                };
                assert!(canceled);
                assert!(err.is::<Canceled>(), "{err}");
                f.assert_unpublished();
                if native {
                    assert!(inspect_native(&f.from, &f.to, "game").unwrap().first_copy);
                } else {
                    assert!(
                        inspect(&f.from, &f.to, &f.global, "game")
                            .unwrap()
                            .first_copy
                    );
                }
            }
        }
    }

    #[test]
    fn execution_revalidation_is_cancellable_before_and_after_copy() {
        for native in [false, true] {
            for after_copy in [false, true] {
                let f = Fixture::new(native);
                let cancel = Token::new();
                let mut copying = false;
                let mut canceled = false;
                let mut events = |event| match event {
                    CopyEvent::Progress(ImportProgress::Started { .. }) => copying = true,
                    CopyEvent::Progress(ImportProgress::Checking { .. })
                        if copying == after_copy =>
                    {
                        canceled = true;
                        cancel.cancel();
                    }
                    CopyEvent::Publishing => panic!("published canceled review"),
                    _ => {}
                };
                let err = if native {
                    let review = inspect_native(&f.from, &f.to, "game").unwrap();
                    execute_native_with(&f.from, &f.to, &review, &cancel, &mut events).unwrap_err()
                } else {
                    let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
                    execute_reviewed_with(&f.from, &f.to, &f.global, &review, &cancel, &mut events)
                        .unwrap_err()
                };
                assert!(canceled);
                assert!(err.is::<Canceled>(), "{err}");
                f.assert_unpublished();
            }
        }
    }

    #[test]
    fn destination_appearing_during_review_is_not_replaced() {
        let f = Fixture::new(false);
        let mut checks = 0;
        let review = inspect_with(&f.from, &f.to, &f.global, "game", &Token::new(), |event| {
            if let ImportProgress::Checking { .. } = event {
                checks += 1;
            }
        })
        .unwrap();
        let destination = review.destination.clone();
        // The last check of the locked review runs after its destination
        // test, so creating it then is only caught by the copy itself.
        let mut seen = 0;
        let err = execute_reviewed(&f.from, &f.to, &f.global, &review, &Token::new(), |event| {
            if let ImportProgress::Checking { .. } = event {
                seen += 1;
                if seen == checks {
                    fs::create_dir_all(&destination).unwrap();
                    fs::write(destination.join("theirs"), "keep").unwrap();
                }
            }
        })
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "portable copy destination appeared during review: {}",
                destination.display()
            )
        );
        assert_eq!(
            fs::read_to_string(destination.join("theirs")).unwrap(),
            "keep"
        );
        f.assert_no_staging();
    }

    #[test]
    fn destination_appearing_during_publication_is_not_replaced() {
        let f = Fixture::new(false);
        let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
        let destination = review.destination.clone();
        let err =
            execute_reviewed_with(&f.from, &f.to, &f.global, &review, &Token::new(), |event| {
                if event == CopyEvent::Publishing {
                    fs::create_dir(&destination).unwrap();
                    fs::write(destination.join("theirs"), "keep").unwrap();
                }
            })
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "portable copy destination appeared during publication: {}",
                destination.display()
            )
        );
        assert_eq!(
            fs::read_to_string(destination.join("theirs")).unwrap(),
            "keep"
        );
        assert!(!f.to.art_dir("game").exists());
        assert!(!f.to.templates_dir().exists());
        f.assert_no_staging();
    }

    #[test]
    fn late_cancel_preserves_success_and_native_copy_reports_real_progress() {
        for native in [false, true] {
            let f = Fixture::new(native);
            let cancel = Token::new();
            let mut events = Vec::new();
            let mut callback = |event| {
                if event == CopyEvent::Publishing {
                    assert!(!f.to.environment_config("game").exists());
                    assert!(!f.to.default_prefix_root().join("game").exists());
                    assert!(!f.to.art_dir("game").exists());
                    assert!(!f.to.templates_dir().exists());
                    cancel.cancel();
                }
                events.push(event);
            };
            let result = if native {
                let review = inspect_native(&f.from, &f.to, "game").unwrap();
                execute_native_with(&f.from, &f.to, &review, &cancel, &mut callback)
            } else {
                let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
                execute_reviewed_with(&f.from, &f.to, &f.global, &review, &cancel, &mut callback)
            };
            assert_eq!(result.unwrap().name, "game");
            assert!(cancel.is_canceled());
            assert_eq!(
                events
                    .iter()
                    .filter(|event| **event == CopyEvent::Publishing)
                    .count(),
                1
            );
            let boundary = events
                .iter()
                .position(|event| *event == CopyEvent::Publishing)
                .unwrap();
            assert!(matches!(
                events.last(),
                Some(CopyEvent::Progress(ImportProgress::Finished { .. }))
            ));
            for source in [f.from.templates_dir(), f.from.art_dir("game")] {
                assert!(events[..boundary].iter().any(|event| matches!(event,
                    CopyEvent::Progress(ImportProgress::Copied { path }) if path.starts_with(&source))));
            }
            assert_eq!(
                fs::read_link(f.to.art_dir("game").join("app/link")).unwrap(),
                Path::new("../hero.png")
            );
            let destination = fs::read_to_string(f.to.environment_config("game")).unwrap();
            if native {
                assert!(destination.contains("desktop = false"));
                let totals: Vec<_> = events
                    .iter()
                    .filter_map(|event| match event {
                        CopyEvent::Progress(ImportProgress::Started { entries }) => Some(*entries),
                        _ => None,
                    })
                    .collect();
                assert_eq!(totals, [2, 4]);
                assert_eq!(
                    events.last(),
                    Some(&CopyEvent::Progress(ImportProgress::Finished {
                        entries: 6
                    }))
                );
            }
            assert_eq!(
                fs::read_to_string(f.to.templates_dir().join("work.toml")).unwrap(),
                "gamemode = true\n"
            );
        }
    }

    #[test]
    fn publish_failures_still_report_published_paths_after_late_cancel() {
        for native in [false, true] {
            let f = Fixture::new(native);
            let cancel = Token::new();
            let mut events = |event| {
                if event == CopyEvent::Publishing {
                    // Racing art prevents publication after templates (and the Wine prefix).
                    fs::create_dir_all(f.to.art_dir("game")).unwrap();
                    cancel.cancel();
                }
            };
            let err = if native {
                let review = inspect_native(&f.from, &f.to, "game").unwrap();
                execute_native_with(&f.from, &f.to, &review, &cancel, &mut events).unwrap_err()
            } else {
                let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
                execute_reviewed_with(&f.from, &f.to, &f.global, &review, &cancel, &mut events)
                    .unwrap_err()
            };
            assert!(!err.is::<Canceled>());
            let message = err.to_string();
            assert!(message.contains("copied files remain at"), "{message}");
            assert!(
                message.contains(&f.to.templates_dir().display().to_string()),
                "{message}"
            );
            if !native {
                assert!(
                    message.contains(
                        &f.to
                            .default_prefix_root()
                            .join("game")
                            .display()
                            .to_string()
                    ),
                    "{message}"
                );
            }
            assert!(message.contains("move them aside and retry"), "{message}");
            assert!(!f.to.environment_config("game").exists());
        }
    }

    #[test]
    fn a_prefix_change_after_review_is_refused_under_the_lock() {
        let f = Fixture::new(false);
        let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
        // Same size and mtime, so only the content fingerprint can see it.
        let data = f.prefix.join("drive_c/data");
        let modified = fs::metadata(&data).unwrap().modified().unwrap();
        fs::write(&data, "edit").unwrap();
        fs::File::options()
            .write(true)
            .open(&data)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let err = execute_reviewed(&f.from, &f.to, &f.global, &review, &Token::new(), |_| {})
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "the reviewed prefix changed; inspect it again before importing"
        );
        f.assert_unpublished();
    }

    #[test]
    fn an_interrupted_restore_after_review_is_reported_as_one() {
        let f = Fixture::new(false);
        let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
        let marker = f.from.restore_marker("game");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, "{}").unwrap();
        let err = execute_reviewed(&f.from, &f.to, &f.global, &review, &Token::new(), |_| {})
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("interrupted snapshot restore; run `wineo snapshot recover game`"),
            "{err}"
        );
        f.assert_unpublished();
    }
}
