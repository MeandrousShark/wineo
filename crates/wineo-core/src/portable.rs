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
use crate::files;
use crate::import::{self, ExternalDependency, ImportLayout, ImportProgress, SourceFingerprint};
use crate::library;
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
    /// The folder's global config was passed over for defaults, which the
    /// copied environment is checked against.
    Warning(String),
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
    progress: impl FnMut(ImportProgress),
    warn: impl FnMut(String),
) -> Result<Environment> {
    execute_reviewed_noting(from, to, global, reviewed, cancel, progress, warn)
}

/// `execute_reviewed` that also hands warnings, such as the folder's global
/// config having been passed over for defaults, to `warn`.
pub fn execute_reviewed_noting(
    from: &Paths,
    to: &Paths,
    global: &GlobalConfig,
    reviewed: &CopyInspection,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
    mut warn: impl FnMut(String),
) -> Result<Environment> {
    execute_reviewed_with(from, to, global, reviewed, cancel, |event| match event {
        CopyEvent::Progress(event) => {
            let event = match event {
                ImportProgress::Warning(warning) => {
                    warn(warning);
                    return;
                }
                event => event,
            };
            if let ImportProgress::Copied { path } = &event
                && path.starts_with(from.art_dir(&reviewed.name))
            {
                return;
            }
            progress(event);
        }
        CopyEvent::Warning(warning) => warn(warning),
        CopyEvent::Publishing => {}
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
            emit_import_progress(&mut events, event)
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
        &mut |event| emit_import_progress(&mut events, event),
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
            &mut |event| emit_import_progress(&mut events, event),
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
            &mut |event| emit_import_progress(&mut events, event),
        )?;
    }
    import::check_cancelled(cancel)?;

    if import::fingerprint_with(&reviewed.source_root, cancel, &mut |event| {
        emit_import_progress(&mut events, event)
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
            emit_import_progress(&mut events, event)
        })? != *expected
        {
            bail!("snapshot `{label}` changed while it was being copied; try again");
        }
    }
    if let Some(expected) = reviewed.templates_fingerprint
        && import::fingerprint_with(&from.templates_dir(), cancel, &mut |event| {
            emit_import_progress(&mut events, event)
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
        &mut |event| emit_import_progress(&mut events, event),
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
            let destination_global =
                destination_global(to, reviewed.global_text.as_deref())?.config;
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
        // Said here rather than with the template guards, so it's once a copy.
        let destination_global = destination_global(to, reviewed.global_text.as_deref())?
            .note_with(|warning| events(CopyEvent::Warning(warning)));
        write_environment_once(to, &destination_global, name, &reviewed.config_text)
    })();
    result.map_err(|err| recovery(&published, err))?;
    transfer_library_entry(from, to, name, &mut events);
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
        files::read_to_string_optional(&from.global_config())
            .with_context(|| format!("failed to read {}", from.global_config().display()))?
            .map(|text| portable_global(to, &from.global_config(), &text))
            .transpose()?
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
    mut warn: impl FnMut(String),
) -> Result<Environment> {
    execute_native_with(from, to, reviewed, cancel, |event| {
        if let CopyEvent::Warning(warning) = event {
            warn(warning);
        }
    })
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
        emit_import_progress(&mut events, event)
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
            &mut |event| emit_import_progress(&mut events, event),
        )?;
        if reviewed.templates_fingerprint
            != Some(import::fingerprint_with(
                &from.templates_dir(),
                cancel,
                &mut |event| emit_import_progress(&mut events, event),
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
        &mut |event| emit_import_progress(&mut events, event),
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
            let destination_global =
                destination_global(to, reviewed.global_text.as_deref())?.config;
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
        // Said here rather than with the template guards, so it's once a copy.
        let destination_global = destination_global(to, reviewed.global_text.as_deref())?
            .note_with(|warning| events(CopyEvent::Warning(warning)));
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
    transfer_library_entry(from, to, name, &mut events);
    events(CopyEvent::Progress(ImportProgress::Finished {
        entries: copied.get(),
    }));
    Ok(Environment::load(to, name)?)
}

fn destination_global(to: &Paths, copied: Option<&str>) -> Result<config::Loaded> {
    match copied {
        Some(text) => Ok(config::Loaded {
            config: GlobalConfig::parse(&to.global_config(), text)?,
            warning: None,
        }),
        None => Ok(GlobalConfig::load_or_default(to)?),
    }
}

fn emit_import_progress(events: &mut impl FnMut(CopyEvent), event: ImportProgress) {
    match event {
        ImportProgress::Warning(warning) => events(CopyEvent::Warning(warning)),
        event => events(CopyEvent::Progress(event)),
    }
}

fn transfer_library_entry(
    from: &Paths,
    to: &Paths,
    name: &str,
    events: &mut impl FnMut(CopyEvent),
) {
    let source = library::load(from);
    let entry = source.environments.get(name).cloned();
    if let Err(error) = library::put(to, name, entry.as_ref()) {
        events(CopyEvent::Warning(format!(
            "Can't update library.toml: {}. The copy of {name} may not keep its favorite or hidden flag.",
            crate::error::chain(&error)
        )));
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
        Some(wine) => build_lock::wine(paths, global, wine)?,
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
mod tests;
