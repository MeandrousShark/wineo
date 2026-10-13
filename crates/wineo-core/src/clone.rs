//! Review and copy a registered environment without running its prefix.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use toml_edit::{DocumentMut, TableLike, Value};

use crate::art::StagedArt;
use crate::build_lock;
use crate::cancel::Token;
use crate::config::{EnvironmentConfig, GlobalConfig, Kind};
use crate::download::{InstallLock, publish};
use crate::edit;
use crate::environment::Environment;
use crate::import::{self, ExternalDependency, ImportLayout, ImportProgress};
use crate::name;
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};
use crate::runners::Runners;
use crate::runtime::Runtime;

pub use crate::import::Cancelled as Canceled;

#[derive(Debug, Clone)]
pub struct CloneInspection {
    pub source_name: String,
    pub new_name: String,
    pub source_root: PathBuf,
    pub destination: PathBuf,
    pub runner: String,
    pub layout: ImportLayout,
    pub targets: Vec<String>,
    pub external_dependencies: Vec<ExternalDependency>,
    pub external_target_paths: Vec<String>,
    pub changed_settings: Vec<String>,
    pub literal_source_paths: Vec<String>,
    /// Whether the source has art, which is copied along.
    pub art: bool,
    source_config: String,
    prefix: import::Inspection,
    config_text: String,
}

pub fn inspect(
    paths: &Paths,
    global: &GlobalConfig,
    source_name: &str,
    new_name: &str,
    destination: Option<&Path>,
) -> Result<CloneInspection> {
    if !name::is_valid(new_name) {
        bail!("invalid environment name `{new_name}`");
    }
    Environment::ensure_exists(paths, source_name).map_err(|err| anyhow!(err))?;
    let source_path = paths.environment_config(source_name);
    let source_config = fs::read_to_string(&source_path)
        .with_context(|| format!("failed to read {}", source_path.display()))?;
    let config =
        EnvironmentConfig::parse(&source_path, &source_config).map_err(|err| anyhow!(err))?;
    let wine = config
        .require_wine(source_name)
        .map_err(|err| anyhow!(err))?;
    let configured_root = paths
        .config_path(&wine.prefix)
        .map_err(|err| anyhow!(err))?;
    let (runner, _runner_guard) = if import::root_layout_ambiguous(&configured_root) {
        let (runner, guard) = Runners::new(paths, global)
            .map_err(|err| anyhow!(err))?
            .resolve_locked(paths, &wine.runner)
            .map_err(|err| anyhow!(err))?;
        (Some(runner), guard)
    } else {
        (None, None)
    };
    let prefix = import::inspect_root(&configured_root, runner.as_ref(), wine.arch)?;
    if paths::resolve(&configured_root) != prefix.source_root {
        bail!("configured prefix root does not match inspected source root");
    }
    let source_root = prefix.source_root.clone();
    let destination = match destination {
        Some(path) => paths::resolve(&absolute(path)?),
        None => paths::resolve(
            &global
                .prefix_root(paths)
                .map_err(|err| anyhow!(err))?
                .join(new_name),
        ),
    };
    paths.check_prefix(&destination)?;
    import::reject_overlapping_paths(&source_root, &destination)?;
    if destination.exists() {
        bail!(
            "clone destination already exists: {}",
            destination.display()
        );
    }
    if paths.environment_config(new_name).exists() {
        bail!("environment `{new_name}` already exists");
    }
    let art = paths.art_dir(new_name);
    if fs::symlink_metadata(&art).is_ok() {
        bail!(
            "{} already exists; move it aside if a clone was interrupted",
            art.display()
        );
    }
    check_shared_configs(paths, new_name, [&source_root, &destination])?;
    crate::environment::check_destination(paths, new_name, &destination)
        .map_err(|err| anyhow!(err))?;

    let mut review = review_config(paths, &config, &source_root);
    if config.heroic_id.is_some() {
        review
            .changed_settings
            .insert(1, "heroic_id removed".to_owned());
    }
    let ConfigReview {
        changed_settings,
        external_target_paths,
        literal_source_paths,
    } = review;
    let config_text = transform_config(
        paths,
        paths,
        &source_config,
        &config,
        &source_root,
        &destination,
        false,
    )?;
    Ok(CloneInspection {
        source_name: source_name.to_owned(),
        new_name: new_name.to_owned(),
        source_root,
        destination,
        runner: wine.runner.clone(),
        layout: prefix.layout,
        targets: config.targets.keys().cloned().collect(),
        external_dependencies: prefix.external_dependencies.clone(),
        external_target_paths,
        changed_settings,
        literal_source_paths,
        art: fs::symlink_metadata(paths.art_dir(source_name)).is_ok(),
        source_config,
        prefix,
        config_text,
    })
}

pub fn execute_reviewed(
    paths: &Paths,
    global: &GlobalConfig,
    reviewed: &CloneInspection,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<Environment> {
    let current = inspect(
        paths,
        global,
        &reviewed.source_name,
        &reviewed.new_name,
        Some(&reviewed.destination),
    )?;
    ensure_same(reviewed, &current)?;
    let locks = Locks::new(paths);
    let mut operation = locks
        .acquire(
            Operation::Create,
            &reviewed.new_name,
            &[&reviewed.source_root, &reviewed.destination],
        )
        .map_err(|err| anyhow!(err))?;
    let _source_config_lock = locks
        .config(&paths.environment_config(&reviewed.source_name))
        .map_err(|err| anyhow!(err))?;
    let current = inspect(
        paths,
        global,
        &reviewed.source_name,
        &reviewed.new_name,
        Some(&reviewed.destination),
    )?;
    ensure_same(reviewed, &current)?;
    for shared in shared_names(
        paths,
        &reviewed.new_name,
        [&reviewed.source_root, &reviewed.destination],
    )? {
        Runtime::new(paths, &shared)
            .check_quiet(&shared)
            .map_err(|err| anyhow!(err))?;
    }
    Runtime::new(paths, &reviewed.new_name)
        .check_quiet(&reviewed.new_name)
        .map_err(|err| anyhow!(err))?;
    import::check_cancelled(cancel)?;
    if reviewed.destination.exists() {
        bail!(
            "clone destination appeared during review: {}",
            reviewed.destination.display()
        );
    }
    let parent = reviewed.destination.parent().ok_or_else(|| {
        anyhow!(
            "destination has no parent: {}",
            reviewed.destination.display()
        )
    })?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let staging = import::Staging::new_in(".wineo-clone-", parent)
        .with_context(|| format!("failed to create staging directory in {}", parent.display()))?;
    let staging_path = staging.path().to_owned();
    let _publish_lock = InstallLock::acquire(&reviewed.destination)?;
    import::copy_tree_with(
        &reviewed.source_root,
        &staging_path,
        &reviewed.destination,
        &import::Links::for_paths(paths),
        cancel,
        &mut progress,
    )?;
    import::check_cancelled(cancel)?;
    if import::fingerprint(&reviewed.source_root)? != reviewed.prefix.fingerprint {
        bail!(
            "{} changed while it was being copied; try again",
            reviewed.source_root.display()
        );
    }
    let text = fs::read_to_string(paths.environment_config(&reviewed.source_name))?;
    if text != reviewed.source_config {
        bail!("source environment config changed while it was being copied; try again");
    }
    import::validate_staged_prefix(&staging_path, &reviewed.destination, reviewed.layout)?;
    if reviewed.destination.exists() {
        bail!(
            "clone destination appeared during publication: {}",
            reviewed.destination.display()
        );
    }
    publish(&staging_path, &reviewed.destination)?;
    import::sync_filesystem(&reviewed.destination)?;
    progress(ImportProgress::Finished {
        entries: reviewed.prefix.fingerprint.entries,
    });

    let art = StagedArt::stage(
        &locks,
        &paths.art_dir(&reviewed.source_name),
        &paths.art_dir(&reviewed.new_name),
    )
    .and_then(StagedArt::publish)
    .map_err(|err| {
        anyhow!(
            "{err}; copied prefix remains at {}; move it aside and retry clone, or register it manually",
            reviewed.destination.display()
        )
    })?;

    let config_path = paths.environment_config(&reviewed.new_name);
    let recovery = || match &art {
        Some(art) => format!(
            "copied prefix remains at {} and its art at {}; move them aside and retry clone, or register it manually",
            reviewed.destination.display(),
            art.display()
        ),
        None => format!(
            "copied prefix remains at {}; move it aside and retry clone, or register it manually",
            reviewed.destination.display()
        ),
    };
    let _config_lock = locks.config(&config_path).map_err(|err| {
        anyhow!(
            "{}; registration could not lock {}: {err}",
            recovery(),
            config_path.display()
        )
    })?;
    if config_path.exists() {
        bail!(
            "{}; config {} appeared after publication",
            recovery(),
            config_path.display()
        );
    }
    let config = EnvironmentConfig::parse(&config_path, &reviewed.config_text)
        .map_err(|err| anyhow!(err))?;
    let wine = config
        .require_wine(&reviewed.new_name)
        .map_err(|err| anyhow!(err))?;
    let _build_guards = build_lock::config(
        paths,
        global,
        Some(&wine.runner),
        wine.dxvk.as_deref(),
        wine.vkd3d.as_deref(),
    )?;
    import::write_config_once(&config_path, &reviewed.config_text)
        .map_err(|err| anyhow!("{}; registration failed: {err}", recovery()))?;
    operation.relabel(Operation::Configure).map_err(|err| {
        anyhow!(
            "prefix {} and config {} are ready, but registration could not finish: {err}",
            reviewed.destination.display(),
            config_path.display()
        )
    })?;
    Environment::load(paths, &reviewed.new_name).map_err(|err| anyhow!(err))
}

fn ensure_same(expected: &CloneInspection, actual: &CloneInspection) -> Result<()> {
    import::ensure_same_review(&expected.prefix, &actual.prefix)?;
    if expected.source_config != actual.source_config
        || expected.config_text != actual.config_text
        || expected.source_root != actual.source_root
        || expected.destination != actual.destination
    {
        bail!("the reviewed environment changed; inspect it again before cloning");
    }
    Ok(())
}

/// What copying a config changes or leaves pointing elsewhere, for a review.
pub(crate) struct ConfigReview {
    pub changed_settings: Vec<String>,
    pub external_target_paths: Vec<String>,
    pub literal_source_paths: Vec<String>,
}

/// Reviews `config` read through `from` for a copy of the prefix at `source_root`.
pub(crate) fn review_config(
    from: &Paths,
    config: &EnvironmentConfig,
    source_root: &Path,
) -> ConfigReview {
    let mut changed_settings = vec!["prefix".to_owned()];
    // A copy shares the original game folder without owning it.
    if config.gog_id.is_some() {
        changed_settings.push("gog_id removed".to_owned());
    }
    let mut external_target_paths = Vec::new();
    let mut literal_source_paths = Vec::new();
    let source_path = source_root.to_string_lossy();
    for (key, value) in &config.env {
        if value.contains(source_path.as_ref()) {
            literal_source_paths.push(format!("env.{key}"));
        }
    }
    for (key, target) in &config.targets {
        for (field, raw) in [("exe", Some(&target.exe)), ("cwd", target.cwd.as_ref())] {
            let Some(raw) = raw else { continue };
            let Some(path) = stored_path(from, raw) else {
                continue;
            };
            if !paths::resolve(&path).starts_with(source_root) {
                external_target_paths.push(format!("targets.{key}.{field}: {raw}"));
            }
        }
        for (index, arg) in target.args.iter().enumerate() {
            if arg.contains(source_path.as_ref()) {
                literal_source_paths.push(format!("targets.{key}.args[{index}]"));
            }
        }
        if target.desktop {
            changed_settings.push(format!("targets.{key}.desktop disabled"));
        }
    }
    ConfigReview {
        changed_settings,
        external_target_paths,
        literal_source_paths,
    }
}

/// `source`, read through `from`, rewritten for a copy at `destination`
/// written through `to`. Internal target paths follow the copy and desktop
/// entries are turned off.
pub(crate) fn transform_config(
    from: &Paths,
    to: &Paths,
    source: &str,
    config: &EnvironmentConfig,
    source_root: &Path,
    destination: &Path,
    keep_heroic_id: bool,
) -> Result<String> {
    let mut doc: DocumentMut = source.parse()?;
    let prefix = to.store_prefix(destination).map_err(|err| anyhow!(err))?;
    edit::store_value(doc.as_table_mut(), "prefix", Value::from(prefix));
    if !keep_heroic_id {
        doc.as_table_mut().remove("heroic_id");
    }
    doc.as_table_mut().remove("gog_id");
    if let Some(targets) = doc
        .get_mut("targets")
        .and_then(|item| item.as_table_like_mut())
    {
        for (name, target) in &config.targets {
            let table = targets
                .get_mut(name)
                .and_then(|item| item.as_table_like_mut())
                .ok_or_else(|| anyhow!("target `{name}` has an unsupported TOML shape"))?;
            for (field, old) in [("exe", Some(&target.exe)), ("cwd", target.cwd.as_ref())] {
                if let Some(old) = old
                    .map(|old| rebase(from, to, old, source_root, destination))
                    .transpose()?
                    .flatten()
                {
                    replace_existing(table, field, Value::from(old))?;
                }
            }
            if target.desktop {
                replace_existing(table, "desktop", Value::from(false))?;
            }
        }
    }
    crate::config::stamp_schema(&mut doc);
    let text = doc.to_string();
    EnvironmentConfig::parse(Path::new("<clone>"), &text).map_err(|err| anyhow!(err))?;
    Ok(text)
}

/// What copying a native environment's config changes, and its config for
/// the copy. It has no prefix, so only desktop entries are turned off and,
/// unless kept, the Heroic id dropped.
pub(crate) fn native_config(
    source: &str,
    config: &EnvironmentConfig,
    keep_heroic_id: bool,
) -> Result<(Vec<String>, String)> {
    let mut changed = Vec::new();
    let mut doc: DocumentMut = source.parse()?;
    if !keep_heroic_id && doc.as_table_mut().remove("heroic_id").is_some() {
        changed.push("heroic_id removed".to_owned());
    }
    // A copy shares the original game folder without owning it.
    if doc.as_table_mut().remove("gog_id").is_some() {
        changed.push("gog_id removed".to_owned());
    }
    if let Some(targets) = doc
        .get_mut("targets")
        .and_then(|item| item.as_table_like_mut())
    {
        for (name, target) in &config.targets {
            if !target.desktop {
                continue;
            }
            let table = targets
                .get_mut(name)
                .and_then(|item| item.as_table_like_mut())
                .ok_or_else(|| anyhow!("target `{name}` has an unsupported TOML shape"))?;
            replace_existing(table, "desktop", Value::from(false))?;
            changed.push(format!("targets.{name}.desktop disabled"));
        }
    }
    crate::config::stamp_schema(&mut doc);
    let text = doc.to_string();
    EnvironmentConfig::parse(Path::new("<clone>"), &text).map_err(|err| anyhow!(err))?;
    Ok((changed, text))
}

/// A reviewed copy of a native environment under a new name.
#[derive(Debug, Clone)]
pub struct NativeCloneInspection {
    pub source_name: String,
    pub new_name: String,
    pub targets: Vec<String>,
    pub changed_settings: Vec<String>,
    /// Whether the source has art, which is copied along.
    pub art: bool,
    source_config: String,
    config_text: String,
}

/// Reviews cloning a native environment, which is its config and art.
pub fn inspect_native(
    paths: &Paths,
    source_name: &str,
    new_name: &str,
) -> Result<NativeCloneInspection> {
    if !name::is_valid(new_name) {
        bail!("invalid environment name `{new_name}`");
    }
    Environment::ensure_exists(paths, source_name).map_err(|err| anyhow!(err))?;
    let source_path = paths.environment_config(source_name);
    let source_config = fs::read_to_string(&source_path)
        .with_context(|| format!("failed to read {}", source_path.display()))?;
    let config =
        EnvironmentConfig::parse(&source_path, &source_config).map_err(|err| anyhow!(err))?;
    match &config.kind {
        Kind::Native => {}
        Kind::Wine(_) => bail!("{source_name} is not a native environment"),
        Kind::Proxied(_) => return Err(anyhow!(config.no_prefix(source_name))),
    }
    if paths.environment_config(new_name).exists() {
        bail!("environment `{new_name}` already exists");
    }
    let art = paths.art_dir(new_name);
    if fs::symlink_metadata(&art).is_ok() {
        bail!(
            "{} already exists; move it aside if a clone was interrupted",
            art.display()
        );
    }
    let (changed_settings, config_text) = native_config(&source_config, &config, false)?;
    Ok(NativeCloneInspection {
        source_name: source_name.to_owned(),
        new_name: new_name.to_owned(),
        targets: config.targets.keys().cloned().collect(),
        changed_settings,
        art: fs::symlink_metadata(paths.art_dir(source_name)).is_ok(),
        source_config,
        config_text,
    })
}

/// Clones a reviewed native environment. The art is published before the
/// config, which registers the copy.
pub fn execute_native(paths: &Paths, reviewed: &NativeCloneInspection) -> Result<Environment> {
    let locks = Locks::new(paths);
    let _operation = locks
        .acquire(Operation::Create, &reviewed.new_name, &[])
        .map_err(|err| anyhow!(err))?;
    let _source_config_lock = locks
        .config(&paths.environment_config(&reviewed.source_name))
        .map_err(|err| anyhow!(err))?;
    let current = inspect_native(paths, &reviewed.source_name, &reviewed.new_name)?;
    if current.source_config != reviewed.source_config
        || current.config_text != reviewed.config_text
    {
        bail!("the reviewed environment changed; inspect it again before cloning");
    }
    let art = StagedArt::stage(
        &locks,
        &paths.art_dir(&reviewed.source_name),
        &paths.art_dir(&reviewed.new_name),
    )
    .and_then(StagedArt::publish)?;
    let config_path = paths.environment_config(&reviewed.new_name);
    let _config_lock = locks.config(&config_path).map_err(|err| anyhow!(err))?;
    import::write_config_once(&config_path, &reviewed.config_text).map_err(|err| match &art {
        Some(art) => anyhow!(
            "{err}; its art was copied to {}, move it aside and retry clone",
            art.display()
        ),
        None => err,
    })?;
    Environment::load(paths, &reviewed.new_name).map_err(|err| anyhow!(err))
}

fn replace_existing(table: &mut dyn TableLike, field: &str, mut new: Value) -> Result<()> {
    let old = table
        .get_mut(field)
        .and_then(|item| item.as_value_mut())
        .ok_or_else(|| anyhow!("target field `{field}` is missing or not a value"))?;
    *new.decor_mut() = old.decor().clone();
    *old = new;
    Ok(())
}

/// Where a target path points, for the paths that can point into a prefix.
/// Outside portable mode only absolute paths and `~/...` qualify, as before.
fn stored_path(paths: &Paths, value: &str) -> Option<PathBuf> {
    if paths.portable_root().is_some() {
        return paths.resolve_stored(value).ok().flatten();
    }
    let path = paths.expand(value);
    path.is_absolute().then_some(path)
}

fn rebase(
    from: &Paths,
    to: &Paths,
    value: &str,
    source_root: &Path,
    destination: &Path,
) -> Result<Option<String>> {
    if from.portable_root().is_none() && !Path::new(value).is_absolute() {
        return Ok(None);
    }
    let Some(path) = stored_path(from, value) else {
        return Ok(None);
    };
    let Ok(relative) = paths::resolve(&path)
        .strip_prefix(source_root)
        .map(Path::to_owned)
    else {
        return Ok(None);
    };
    let rebased = destination.join(relative);
    if to.portable_root().is_some() {
        return to.store(&rebased).map(Some).map_err(|err| anyhow!(err));
    }
    Ok(Some(rebased.to_string_lossy().into_owned()))
}

fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).with_context(|| format!("failed to resolve {}", path.display()))
}

fn check_shared_configs(paths: &Paths, name: &str, prefixes: [&Path; 2]) -> Result<()> {
    shared_names(paths, name, prefixes).map(|_| ())
}

fn shared_names(paths: &Paths, name: &str, prefixes: [&Path; 2]) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for prefix in prefixes {
        let sharing = crate::environment::checked_sharing(paths, name, prefix).map_err(|err| {
            if let crate::error::Error::UnreadableEnvironments(names) = err {
                anyhow!(
                    "could not inspect existing environments: {}",
                    names.join(", ")
                )
            } else {
                anyhow!(err)
            }
        })?;
        for shared in sharing.names {
            if !names.contains(&shared) {
                names.push(shared);
            }
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use crate::test_script::write_script;
    use std::ffi::OsString;
    use std::os::unix::fs::{MetadataExt, symlink};

    use super::*;

    fn paths(root: &Path) -> Paths {
        Paths::from_lookup(|key| match key {
            "HOME" => Some(OsString::from(root)),
            _ => None,
        })
        .unwrap()
    }

    fn prefix(root: &Path, proton: bool) {
        let wine = if proton {
            root.join("pfx")
        } else {
            root.to_owned()
        };
        fs::create_dir_all(wine.join("drive_c")).unwrap();
        fs::write(
            wine.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        fs::write(wine.join("drive_c/game.exe"), b"game").unwrap();
    }

    fn setup(root: &Path, proton: bool) -> (Paths, PathBuf, String) {
        let paths = paths(root);
        let source = root.join("source");
        prefix(&source, proton);
        fs::write(source.join("extra"), b"same").unwrap();
        fs::hard_link(source.join("extra"), source.join("extra_link")).unwrap();
        symlink(source.join("extra"), source.join("internal_link")).unwrap();
        symlink("/some/external/game", source.join("external_link")).unwrap();
        let wine = if proton {
            source.join("pfx")
        } else {
            source.clone()
        };
        let text = format!(
            "# source comment\nprefix = '{}'\nrunner = 'wine'\narch = 'win64'\nheroic_id = 'id'\ngameid = 'game'\nstore = 'steam'\n\n[env]\nSOURCE_NOTE = '{}'\n\n[targets.game]\nexe = '{}' # exe comment\ncwd = '{}'\nargs = ['--from={}']\ndesktop = true\nname = 'Game'\ndefault = true\n\n[targets.external]\nexe = '/some/external/game'\ncwd = 'C:\\\\Games'\n\n[targets.home]\nexe = '~/Games/ProjectZomboid/ProjectZomboid64.exe'\n",
            source.display(),
            source.display(),
            wine.join("drive_c/game.exe").display(),
            wine.join("drive_c").display(),
            source.display(),
        );
        fs::create_dir_all(paths.environments_dir()).unwrap();
        fs::write(paths.environment_config("source"), &text).unwrap();
        (paths, source, text)
    }

    #[test]
    fn clone_keeps_source_and_rewrites_only_selected_config_fields() {
        for proton in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let (paths, source, original) = setup(root.path(), proton);
            let destination = root.path().join("copy");
            let review = inspect(
                &paths,
                &GlobalConfig::default(),
                "source",
                "copy",
                Some(&destination),
            )
            .unwrap();
            assert_eq!(review.layout == ImportLayout::Proton, proton);
            assert!(
                review
                    .literal_source_paths
                    .contains(&"env.SOURCE_NOTE".to_owned())
            );
            assert!(
                review
                    .external_target_paths
                    .iter()
                    .any(|path| path.contains("targets.external.exe"))
            );
            assert!(review.external_target_paths.iter().any(|path| {
                path.contains("targets.home.exe: ~/Games/ProjectZomboid/ProjectZomboid64.exe")
            }));
            assert!(
                !review
                    .external_target_paths
                    .iter()
                    .any(|path| path.contains("targets.external.cwd"))
            );
            let clone = execute_reviewed(
                &paths,
                &GlobalConfig::default(),
                &review,
                &Token::new(),
                |_| {},
            )
            .unwrap();
            assert_eq!(clone.config.heroic_id, None);
            assert_eq!(clone.config.wine().unwrap().gameid.as_deref(), Some("game"));
            assert_eq!(clone.config.wine().unwrap().store.as_deref(), Some("steam"));
            assert_eq!(clone.config.targets["game"].name.as_deref(), Some("Game"));
            assert!(clone.config.targets["game"].default);
            assert!(!clone.config.targets["game"].desktop);
            assert_eq!(clone.config.targets["external"].exe, "/some/external/game");
            assert_eq!(
                clone.config.targets["home"].exe,
                "~/Games/ProjectZomboid/ProjectZomboid64.exe"
            );
            assert_eq!(
                clone.config.targets["external"].cwd.as_deref(),
                Some("C:\\\\Games")
            );
            let text = fs::read_to_string(paths.environment_config("copy")).unwrap();
            assert!(text.contains("# source comment"));
            assert!(text.contains("# exe comment"));
            assert!(!text.contains("heroic_id"));
            assert!(
                text.contains(
                    &destination
                        .join(if proton {
                            "pfx/drive_c/game.exe"
                        } else {
                            "drive_c/game.exe"
                        })
                        .to_string_lossy()
                        .to_string()
                )
            );
            assert_eq!(
                fs::read_to_string(paths.environment_config("source")).unwrap(),
                original
            );
            assert_eq!(fs::read(source.join("extra")).unwrap(), b"same");
            assert_eq!(
                fs::metadata(destination.join("extra")).unwrap().ino(),
                fs::metadata(destination.join("extra_link")).unwrap().ino()
            );
            assert_ne!(
                fs::metadata(source.join("extra")).unwrap().ino(),
                fs::metadata(destination.join("extra")).unwrap().ino()
            );
            assert_eq!(
                fs::read_link(destination.join("internal_link")).unwrap(),
                destination.join("extra")
            );
            assert_eq!(
                fs::read_link(destination.join("external_link")).unwrap(),
                Path::new("/some/external/game")
            );
        }
    }

    #[test]
    fn cloning_a_legacy_config_versions_only_the_copy() {
        let root = tempfile::tempdir().unwrap();
        let (paths, _source, original) = setup(root.path(), false);
        let source_config = paths.environment_config("source");
        let before = fs::metadata(&source_config).unwrap().modified().unwrap();
        let global = GlobalConfig::default();
        let destination = root.path().join("copy");
        let review = inspect(&paths, &global, "source", "copy", Some(&destination)).unwrap();
        let clone = execute_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap();
        assert_eq!(clone.config.schema_version, 1);
        let text = fs::read_to_string(paths.environment_config("copy")).unwrap();
        assert!(text.starts_with("# source comment\n"), "{text}");
        assert!(
            text.contains("store = 'steam'\nschema_version = 1\n\n[env]"),
            "{text}"
        );
        assert_eq!(fs::read_to_string(&source_config).unwrap(), original);
        assert_eq!(
            fs::metadata(&source_config).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn cloning_refuses_an_unsupported_version() {
        let root = tempfile::tempdir().unwrap();
        let (paths, _source, original) = setup(root.path(), false);
        let text = format!("schema_version = 2\n{original}");
        fs::write(paths.environment_config("source"), &text).unwrap();
        let destination = root.path().join("copy");
        let err = inspect(
            &paths,
            &GlobalConfig::default(),
            "source",
            "copy",
            Some(&destination),
        )
        .unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<crate::error::Error>(),
                Some(crate::error::Error::SchemaVersion { .. })
            ),
            "{err}"
        );
        assert!(!destination.exists());
        assert!(!paths.environment_config("copy").exists());
        assert_eq!(
            fs::read_to_string(paths.environment_config("source")).unwrap(),
            text
        );
    }

    #[test]
    fn clone_refuses_changes_collisions_and_cancellation_without_publication() {
        let root = tempfile::tempdir().unwrap();
        let (paths, source, original) = setup(root.path(), false);
        let global = GlobalConfig::default();
        let destination = root.path().join("copy");
        let review = inspect(&paths, &global, "source", "copy", Some(&destination)).unwrap();
        let cancelled = Token::new();
        cancelled.cancel();
        assert!(execute_reviewed(&paths, &global, &review, &cancelled, |_| {}).is_err());
        assert!(!destination.exists());
        assert!(!paths.environment_config("copy").exists());
        fs::write(source.join("extra"), b"changed").unwrap();
        assert!(execute_reviewed(&paths, &global, &review, &Token::new(), |_| {}).is_err());
        assert!(!destination.exists());
        fs::write(source.join("extra"), b"same").unwrap();
        fs::write(
            paths.environment_config("source"),
            format!("{original}\n# changed\n"),
        )
        .unwrap();
        assert!(execute_reviewed(&paths, &global, &review, &Token::new(), |_| {}).is_err());
        assert!(!destination.exists());
        assert!(
            inspect(
                &paths,
                &global,
                "source",
                "copy",
                Some(&source.join("nested"))
            )
            .is_err()
        );
        fs::create_dir(&destination).unwrap();
        assert!(inspect(&paths, &global, "source", "copy", Some(&destination)).is_err());
    }

    #[test]
    fn inline_targets_rebase_paths_and_disable_desktop_without_changing_layout() {
        for inline_root in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let (paths, source, _) = setup(root.path(), false);
            let target = format!(
                "game = {{ exe = '{}', cwd = '{}', desktop = true, name = 'Game' }}",
                source.join("drive_c/game.exe").display(),
                source.join("drive_c").display()
            );
            let targets = if inline_root {
                format!("targets = {{ {target} }} # target comment\n")
            } else {
                format!("[targets]\n{target} # target comment\n")
            };
            let original = format!(
                "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\nheroic_id = 'id'\n{targets}",
                source.display()
            );
            fs::write(paths.environment_config("source"), &original).unwrap();
            let destination = root.path().join("copy");
            let review = inspect(
                &paths,
                &GlobalConfig::default(),
                "source",
                "copy",
                Some(&destination),
            )
            .unwrap();
            let clone = execute_reviewed(
                &paths,
                &GlobalConfig::default(),
                &review,
                &Token::new(),
                |_| {},
            )
            .unwrap();
            assert_eq!(
                clone.config.targets["game"].exe,
                destination.join("drive_c/game.exe").to_string_lossy()
            );
            assert_eq!(
                clone.config.targets["game"].cwd.as_deref(),
                Some(destination.join("drive_c").to_string_lossy().as_ref())
            );
            assert!(!clone.config.targets["game"].desktop);
            let text = fs::read_to_string(paths.environment_config("copy")).unwrap();
            assert!(text.contains("# target comment"));
            assert!(text.contains("game = {"));
            assert!(!text.contains("heroic_id"));
            assert_eq!(
                fs::read_to_string(paths.environment_config("source")).unwrap(),
                original
            );
        }
    }

    #[test]
    fn ambiguous_configured_root_uses_runner_layout_without_normalizing_root() {
        for proton in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let (paths, source, _) = setup(root.path(), false);
            prefix(&source.join("pfx"), false);
            let runner_root = root.path().join("runners");
            let runner_name = if proton { "proton-build" } else { "wine-build" };
            let executable = if proton {
                runner_root.join(runner_name).join("proton")
            } else {
                runner_root.join(runner_name).join("bin/wine")
            };
            fs::create_dir_all(executable.parent().unwrap()).unwrap();
            write_script(&executable, b"#!/bin/sh\n");
            if !proton {
                let server = executable.with_file_name("wineserver");
                write_script(&server, b"#!/bin/sh\n");
            }
            fs::write(
                paths.environment_config("source"),
                format!(
                    "prefix = '{}'\nrunner = '{runner_name}'\narch = 'win64'\n",
                    source.display()
                ),
            )
            .unwrap();
            let mut global = GlobalConfig::default();
            global.defaults.runner_root = Some(runner_root.to_string_lossy().into_owned());
            let destination = root.path().join("copy");
            let review = inspect(&paths, &global, "source", "copy", Some(&destination)).unwrap();
            assert_eq!(review.layout == ImportLayout::Proton, proton);
            assert_eq!(review.source_root, source);
            execute_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap();
            assert!(destination.join("system.reg").is_file());
            assert!(destination.join("pfx/system.reg").is_file());
        }
    }

    #[test]
    fn copy_change_leaves_no_destination_and_registration_failure_keeps_completed_copy() {
        let root = tempfile::tempdir().unwrap();
        let (paths, source, _) = setup(root.path(), false);
        let global = GlobalConfig::default();
        let destination = root.path().join("copy");
        let review = inspect(&paths, &global, "source", "copy", Some(&destination)).unwrap();
        let result = execute_reviewed(&paths, &global, &review, &Token::new(), |event| {
            if matches!(event, ImportProgress::Started { .. }) {
                fs::write(source.join("extra"), b"changed during copy").unwrap();
            }
        });
        assert!(result.unwrap_err().to_string().contains("changed while"));
        assert!(!destination.exists());
        assert!(!paths.environment_config("copy").exists());

        fs::write(source.join("extra"), b"same").unwrap();
        let review = inspect(&paths, &global, "source", "copy", Some(&destination)).unwrap();
        let config_path = paths.environment_config("copy");
        let result = execute_reviewed(&paths, &global, &review, &Token::new(), |event| {
            if matches!(event, ImportProgress::Finished { .. }) {
                fs::write(&config_path, "occupied").unwrap();
            }
        });
        let error = result.unwrap_err().to_string();
        assert!(error.contains("move it aside and retry clone"), "{error}");
        assert!(destination.join("drive_c/game.exe").is_file());
        assert_eq!(fs::read(&config_path).unwrap(), b"occupied");
    }
}
