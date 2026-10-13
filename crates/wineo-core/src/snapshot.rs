//! Local reflink restore points for registered prefixes.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::{RenameFlags, renameat_with};
use serde::{Deserialize, Serialize};

use crate::build_lock;
use crate::cancel::Token;
use crate::config::{EnvironmentConfig, GlobalConfig};
use crate::environment::{self, Environment};
use crate::gog;
use crate::import::{self, ExternalDependency, ImportLayout, ImportProgress, SourceFingerprint};
use crate::name;
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};
use crate::runners::Runners;
use crate::runtime::Runtime;

const VERSION: u32 = 1;
const MANIFEST: &str = "manifest.json";

pub use crate::import::Cancelled as Canceled;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    environment: String,
    label: String,
    /// Absolute, or relative to the root for a portable folder.
    prefix: PathBuf,
    layout: ImportLayout,
    created_at: String,
}

/// On disk, a portable marker's paths are relative to the root, so recovery
/// still finds them after the folder moves.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RestoreMarker {
    version: u32,
    environment: String,
    prefix: PathBuf,
    staging: PathBuf,
    old_directory: DirectoryIdentity,
    old_fingerprint: SourceFingerprint,
    old_config: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

/// Frontend progress, including the boundary after which cancellation is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotEvent {
    Progress(ImportProgress),
    /// The staged data is ready. Publication or restore now finishes without cancellation.
    Finishing,
}

#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    pub label: String,
    pub created_at: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RestoreOutcome {
    pub retained_old_prefix: Option<PathBuf>,
    /// The GOG id put back into a config saved before the game was migrated.
    pub gog_id_added: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateInspection {
    pub name: String,
    pub label: String,
    pub prefix: PathBuf,
    pub snapshot: PathBuf,
    pub layout: ImportLayout,
    pub runner: String,
    pub external_dependencies: Vec<ExternalDependency>,
    config_text: String,
    fingerprint: SourceFingerprint,
}

#[derive(Debug, Clone)]
pub struct RestoreInspection {
    pub name: String,
    pub label: String,
    pub prefix: PathBuf,
    pub snapshot: PathBuf,
    pub current_config: String,
    pub saved_config: String,
    pub current_runner: String,
    pub saved_runner: String,
    pub layout: ImportLayout,
    /// The game folder's GOG id, which the restore puts back when the saved
    /// config predates the migration.
    pub gog_id_added: Option<String>,
    current_fingerprint: SourceFingerprint,
    snapshot_fingerprint: SourceFingerprint,
}

pub fn check_recovery_marker(paths: &Paths, name: &str) -> crate::Result<()> {
    let marker = marker_path(paths, name);
    let exists = match fs::symlink_metadata(&marker) {
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => {
            return Err(crate::Error::Io {
                source: err,
                context: format!("failed to inspect {}", marker.display()),
            });
        }
    };
    if exists {
        return Err(crate::Error::ConfigInvalid {
            path: paths.environment_config(name),
            message: format!("interrupted snapshot restore; run `wineo snapshot recover {name}`"),
        });
    }
    Ok(())
}

pub fn inspect_create(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    label: &str,
) -> Result<CreateInspection> {
    inspect_create_with(paths, global, name, label, &Token::new(), |_| {})
}

/// Cancellable inspection with entry-count progress for each tree check.
pub fn inspect_create_with(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    label: &str,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<CreateInspection> {
    import::check_cancelled(cancel)?;
    validate_label(label)?;
    let (env, config_text, prefix) = registered(paths, name)?;
    let inspected = inspect_prefix(paths, global, &env, &prefix, cancel, &mut progress)?;
    check_sharing(paths, name, &prefix)?;
    let snapshot = paths.snapshot_root(&prefix, name).join(label);
    import::reject_overlapping_paths(&prefix, &snapshot)?;
    if fs::symlink_metadata(&snapshot).is_ok() {
        bail!("snapshot `{label}` already exists");
    }
    Ok(CreateInspection {
        name: name.to_owned(),
        label: label.to_owned(),
        prefix,
        snapshot,
        layout: inspected.layout,
        runner: env.wine()?.runner.clone(),
        external_dependencies: inspected.external_dependencies,
        config_text,
        fingerprint: inspected.fingerprint,
    })
}

pub fn create_reviewed(
    paths: &Paths,
    global: &GlobalConfig,
    reviewed: &CreateInspection,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<SnapshotInfo> {
    create_reviewed_with(paths, global, reviewed, cancel, |event| {
        if let SnapshotEvent::Progress(progress_event) = event
            && !matches!(progress_event, ImportProgress::Checking { .. })
        {
            progress(progress_event);
        }
    })
}

/// Executes the frozen review with cancellable checks and an explicit finishing boundary.
pub fn create_reviewed_with(
    paths: &Paths,
    global: &GlobalConfig,
    reviewed: &CreateInspection,
    cancel: &Token,
    mut events: impl FnMut(SnapshotEvent),
) -> Result<SnapshotInfo> {
    let current = inspect_create_with(
        paths,
        global,
        &reviewed.name,
        &reviewed.label,
        cancel,
        |progress| events(SnapshotEvent::Progress(progress)),
    )?;
    ensure_create_same(reviewed, &current)?;
    let locks = Locks::new(paths);
    let _operation = locks.acquire(Operation::Snapshot, &reviewed.name, &[&reviewed.prefix])?;
    let _config_lock = locks.config(&paths.environment_config(&reviewed.name))?;
    let current = inspect_create_with(
        paths,
        global,
        &reviewed.name,
        &reviewed.label,
        cancel,
        |progress| events(SnapshotEvent::Progress(progress)),
    )?;
    ensure_create_same(reviewed, &current)?;
    Runtime::new(paths, &reviewed.name).check_quiet(&reviewed.name)?;
    import::check_cancelled(cancel)?;
    let parent = reviewed
        .snapshot
        .parent()
        .ok_or_else(|| anyhow!("snapshot has no parent"))?;
    ensure_storage_dirs(parent)?;
    let staging = import::Staging::new_in(".wineo-create-", parent)?;
    let staged_prefix = staging.path().join("prefix");
    import::copy_tree_strict_reflink_with(
        &reviewed.prefix,
        &staged_prefix,
        &reviewed.snapshot.join("prefix"),
        &import::Links::for_paths(paths),
        cancel,
        &mut |progress| events(SnapshotEvent::Progress(progress)),
    )?;
    import::check_cancelled(cancel)?;
    if import::fingerprint_with(&reviewed.prefix, cancel, |progress| {
        events(SnapshotEvent::Progress(progress))
    })? != reviewed.fingerprint
    {
        bail!("prefix changed while creating snapshot; retry");
    }
    if fs::read_to_string(paths.environment_config(&reviewed.name))? != reviewed.config_text {
        bail!("config changed while creating snapshot; retry");
    }
    import::validate_staged_prefix(
        &staged_prefix,
        &reviewed.snapshot.join("prefix"),
        reviewed.layout,
    )?;
    write_synced(
        &staging.path().join("config.toml"),
        reviewed.config_text.as_bytes(),
    )?;
    let manifest = Manifest {
        version: VERSION,
        environment: reviewed.name.clone(),
        label: reviewed.label.clone(),
        prefix: prefix_identity(paths, &reviewed.prefix)?,
        layout: reviewed.layout,
        created_at: jiff::Timestamp::now().to_string(),
    };
    write_synced(
        &staging.path().join(MANIFEST),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    sync_dir(staging.path())?;
    import::check_cancelled(cancel)?;
    events(SnapshotEvent::Finishing);
    crate::download::publish(staging.path(), &reviewed.snapshot)?;
    import::sync_filesystem(&reviewed.snapshot)?;
    events(SnapshotEvent::Progress(ImportProgress::Finished {
        entries: reviewed.fingerprint.entries,
    }));
    Ok(SnapshotInfo {
        label: reviewed.label.clone(),
        created_at: manifest.created_at,
        path: reviewed.snapshot.clone(),
    })
}

pub fn list(paths: &Paths, name: &str) -> Result<Vec<SnapshotInfo>> {
    let (_, _, prefix) = registered(paths, name)?;
    let root = paths.snapshot_root(&prefix, name);
    if !root.exists() {
        return Ok(Vec::new());
    }
    ensure_directory(&root)?;
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(&root)? {
        let path = entry?.path();
        if !path.is_dir() || path.is_symlink() {
            continue;
        }
        let Some(label) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name::is_valid(label) {
            continue;
        }
        let manifest = read_manifest(paths, &path, name, label, &prefix)?;
        snapshots.push(SnapshotInfo {
            label: label.to_owned(),
            created_at: manifest.created_at,
            path,
        });
    }
    snapshots.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(snapshots)
}

pub fn inspect_restore(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    label: &str,
) -> Result<RestoreInspection> {
    inspect_restore_with(paths, global, name, label, &Token::new(), |_| {})
}

/// Cancellable inspection with entry-count progress for each tree check.
pub fn inspect_restore_with(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    label: &str,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<RestoreInspection> {
    import::check_cancelled(cancel)?;
    validate_label(label)?;
    let (env, current_config, prefix) = registered(paths, name)?;
    check_sharing(paths, name, &prefix)?;
    environment::check_nesting(paths, name, &prefix)?;
    // The old prefix is deleted after the swap, and that would reach into
    // whatever is mounted inside it.
    if let Some(mount) = crate::trash::foreign_mount(&prefix)? {
        bail!(
            "{name}'s prefix has a mount at {}. Unmount it first.",
            mount.display()
        );
    }
    let current = inspect_prefix(paths, global, &env, &prefix, cancel, &mut progress)?;
    let snapshot = paths.snapshot_root(&prefix, name).join(label);
    import::reject_overlapping_paths(&prefix, &snapshot)?;
    let manifest = read_manifest(paths, &snapshot, name, label, &prefix)?;
    let saved_config = fs::read_to_string(snapshot.join("config.toml"))?;
    let saved = EnvironmentConfig::parse(&snapshot.join("config.toml"), &saved_config)?;
    let gog_id_added = saved_gog_id(paths, name, &saved)?;
    let Some(saved) = saved.wine() else {
        bail!("snapshot config points to a different prefix");
    };
    if paths::resolve(&paths.config_path(&saved.prefix)?) != prefix {
        bail!("snapshot config points to a different prefix");
    }
    let snap_prefix = snapshot.join("prefix");
    import::validate_staged_prefix(&snap_prefix, &snap_prefix, manifest.layout)?;
    let snapshot_fingerprint = import::fingerprint_with(&snap_prefix, cancel, &mut progress)?;
    Ok(RestoreInspection {
        name: name.to_owned(),
        label: label.to_owned(),
        prefix,
        snapshot,
        current_config,
        saved_config,
        current_runner: env.wine()?.runner.clone(),
        saved_runner: saved.runner.clone(),
        layout: manifest.layout,
        gog_id_added,
        current_fingerprint: current.fingerprint,
        snapshot_fingerprint,
    })
}

pub fn restore_reviewed(
    paths: &Paths,
    global: &GlobalConfig,
    reviewed: &RestoreInspection,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<RestoreOutcome> {
    restore_reviewed_with(paths, global, reviewed, cancel, |event| {
        if let SnapshotEvent::Progress(progress_event) = event
            && !matches!(progress_event, ImportProgress::Checking { .. })
        {
            progress(progress_event);
        }
    })
}

/// Executes the frozen review with cancellable checks and an explicit finishing boundary.
pub fn restore_reviewed_with(
    paths: &Paths,
    global: &GlobalConfig,
    reviewed: &RestoreInspection,
    cancel: &Token,
    mut events: impl FnMut(SnapshotEvent),
) -> Result<RestoreOutcome> {
    let current = inspect_restore_with(
        paths,
        global,
        &reviewed.name,
        &reviewed.label,
        cancel,
        |progress| events(SnapshotEvent::Progress(progress)),
    )?;
    ensure_restore_same(reviewed, &current)?;
    let locks = Locks::new(paths);
    let _operation = locks.acquire(Operation::Snapshot, &reviewed.name, &[&reviewed.prefix])?;
    let _config_lock = locks.config(&paths.environment_config(&reviewed.name))?;
    let current = inspect_restore_with(
        paths,
        global,
        &reviewed.name,
        &reviewed.label,
        cancel,
        |progress| events(SnapshotEvent::Progress(progress)),
    )?;
    ensure_restore_same(reviewed, &current)?;
    Runtime::new(paths, &reviewed.name).check_quiet(&reviewed.name)?;
    import::check_cancelled(cancel)?;
    let parent = reviewed
        .prefix
        .parent()
        .ok_or_else(|| anyhow!("prefix has no parent"))?;
    let staging = import::Staging::new_in(".wineo-restore-", parent)?;
    let staging_path = staging.path().to_owned();
    // A saved link into the live prefix is internal to what the restore
    // puts back, so it is written relative too.
    let mut links = import::Links::for_paths(paths);
    if links.relative {
        links
            .moved
            .push((reviewed.prefix.clone(), reviewed.prefix.clone()));
    }
    import::copy_tree_strict_reflink_with(
        &reviewed.snapshot.join("prefix"),
        &staging_path,
        &reviewed.prefix,
        &links,
        cancel,
        &mut |progress| events(SnapshotEvent::Progress(progress)),
    )?;
    import::check_cancelled(cancel)?;
    let final_inspection = inspect_restore_with(
        paths,
        global,
        &reviewed.name,
        &reviewed.label,
        cancel,
        |progress| events(SnapshotEvent::Progress(progress)),
    )?;
    ensure_restore_same(reviewed, &final_inspection)?;
    import::validate_staged_prefix(&staging_path, &reviewed.prefix, reviewed.layout)?;
    import::sync_filesystem(&staging_path)?;
    let old_is_subvolume = is_btrfs_subvolume(&reviewed.prefix)?;
    let marker_path = marker_path(paths, &reviewed.name);
    let marker = RestoreMarker {
        version: VERSION,
        environment: reviewed.name.clone(),
        prefix: reviewed.prefix.clone(),
        staging: staging_path.clone(),
        old_directory: directory_identity(&reviewed.prefix)?,
        old_fingerprint: reviewed.current_fingerprint,
        old_config: reviewed.current_config.clone(),
    };
    let stored = stored_marker(paths, &marker)?;
    import::check_cancelled(cancel)?;
    events(SnapshotEvent::Finishing);
    let staging_path = staging.keep();
    if let Err(err) = write_marker(&marker_path, &stored) {
        if fs::symlink_metadata(&marker_path).is_ok() {
            bail!(
                "restore stopped while publishing recovery marker: {err}; staged prefix remains at {}; marker at {}; run `wineo snapshot recover {}`",
                staging_path.display(),
                marker_path.display(),
                reviewed.name
            );
        } else {
            remove_tree(&staging_path)?;
        }
        return Err(err);
    }
    let saved_config = match &reviewed.gog_id_added {
        Some(gog_id) => with_gog_id(&reviewed.saved_config, gog_id)?,
        None => reviewed.saved_config.clone(),
    };
    let result = (|| -> Result<()> {
        exchange(&reviewed.prefix, &staging_path)?;
        sync_dir(parent)?;
        with_config_build_locks(
            paths,
            global,
            &paths.environment_config(&reviewed.name),
            &saved_config,
            || write_config_atomic(&paths.environment_config(&reviewed.name), &saved_config),
        )?;
        Ok(())
    })();
    if let Err(err) = result {
        if let Err(rollback) = rollback_locked(paths, &marker_path, &marker) {
            bail!(
                "restore failed: {err}; rollback failed: {rollback}; original copy at {}; recovery marker at {}; run `wineo snapshot recover {}`",
                staging_path.display(),
                marker_path.display(),
                reviewed.name
            );
        }
        return Err(err);
    }
    if let Err(err) = fs::remove_file(&marker_path) {
        if let Err(rollback) = rollback_locked(paths, &marker_path, &marker) {
            bail!(
                "restore could not clear marker: {err}; rollback failed: {rollback}; original copy at {}; recovery marker at {}; run `wineo snapshot recover {}`",
                staging_path.display(),
                marker_path.display(),
                reviewed.name
            );
        }
        return Err(err.into());
    }
    sync_dir(marker_path.parent().unwrap()).with_context(|| {
        format!(
            "restore completed, but marker durability is uncertain; old prefix remains at {}",
            staging_path.display()
        )
    })?;
    let retained_old_prefix = if old_is_subvolume {
        Some(staging_path.clone())
    } else {
        remove_tree(&staging_path).with_context(|| {
            format!(
                "restored environment, but old prefix remains at {}",
                staging_path.display()
            )
        })?;
        None
    };
    events(SnapshotEvent::Progress(ImportProgress::Finished {
        entries: reviewed.snapshot_fingerprint.entries,
    }));
    Ok(RestoreOutcome {
        retained_old_prefix,
        gog_id_added: reviewed.gog_id_added.clone(),
    })
}

/// The game folder's state follows the folder, not the snapshot. A saved
/// config from before migration gets the owned game's id back, and one
/// naming another game is refused before anything changes.
fn saved_gog_id(paths: &Paths, name: &str, saved: &EnvironmentConfig) -> Result<Option<String>> {
    // GOG games live in the host library only.
    if paths.portable_root().is_some() {
        return Ok(None);
    }
    let strict = saved.gog_id.is_some() || gog::folder::may_track_game(paths, name);
    let owned = gog::folder::owned_by(paths, name, strict)?;
    match &saved.gog_id {
        Some(gog_id) => {
            if let Some(state) = owned.iter().find(|state| !state.is_game(gog_id))
                && !owned.iter().any(|state| state.is_game(gog_id))
            {
                bail!(
                    "The snapshot's gog_id {gog_id} doesn't match {name}'s GOG game {}.",
                    state.gog_id().map_or_else(String::new, |id| id.to_string())
                );
            }
            Ok(None)
        }
        None => {
            let ids: BTreeSet<u64> = owned.iter().filter_map(|state| state.gog_id()).collect();
            if ids.len() > 1 {
                bail!("{name} owns more than one GOG game folder.");
            }
            Ok(ids.into_iter().next().map(|id| id.to_string()))
        }
    }
}

/// `text` with a `gog_id`, keeping its comments and formatting.
fn with_gog_id(text: &str, gog_id: &str) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    crate::edit::store_value(doc.as_table_mut(), "gog_id", toml_edit::Value::from(gog_id));
    crate::config::stamp_schema(&mut doc);
    Ok(doc.to_string())
}

pub fn delete(paths: &Paths, name: &str, label: &str) -> Result<()> {
    validate_label(label)?;
    let (_, _, prefix) = registered(paths, name)?;
    let locks = Locks::new(paths);
    let _operation = locks.acquire(Operation::Snapshot, name, &[&prefix])?;
    let _config_lock = locks.config(&paths.environment_config(name))?;
    let (_, _, current_prefix) = registered(paths, name)?;
    if current_prefix != prefix {
        bail!("environment prefix changed; retry");
    }
    let snapshot = paths.snapshot_root(&prefix, name).join(label);
    read_manifest(paths, &snapshot, name, label, &prefix)?;
    remove_tree(&snapshot)?;
    Ok(())
}

/// Checks the marker's shape and saved config without attempting rollback.
/// Recovery still checks runtime, locks, directory identity, and contents when applied.
pub fn inspect_recovery(paths: &Paths, name: &str) -> Result<()> {
    if !name::is_valid(name) || !paths.environment_config(name).is_file() {
        bail!("unknown environment `{name}`");
    }
    let marker = read_marker(paths, &marker_path(paths, name), name)?;
    validate_marker_config(paths, name, &marker)
}

pub fn recover(paths: &Paths, name: &str) -> Result<()> {
    if !name::is_valid(name) || !paths.environment_config(name).is_file() {
        bail!("unknown environment `{name}`");
    }
    let path = marker_path(paths, name);
    let marker = read_marker(paths, &path, name)?;
    let prefix = marker.prefix.clone();
    validate_marker_config(paths, name, &marker)?;
    let locks = Locks::new(paths);
    let _operation = locks.acquire(Operation::Snapshot, name, &[&prefix])?;
    let _config_lock = locks.config(&paths.environment_config(name))?;
    Runtime::new(paths, name).check_quiet(name)?;
    let marker = read_marker(paths, &path, name)?;
    validate_marker_config(paths, name, &marker)?;
    // Rollback deletes whichever tree is not put back.
    environment::check_nesting(paths, name, &marker.prefix).map_err(|err| match err {
        crate::Error::UnreadableEnvironments(names) => anyhow!(
            "{} can't be read, so it's unknown whether its prefix is inside {name}'s. Fix it and run `wineo snapshot recover {name}` again.",
            names
                .iter()
                .map(|other| paths.environment_config(other).display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        err => err.into(),
    })?;
    rollback_locked(paths, &path, &marker)
}

fn validate_marker_config(paths: &Paths, name: &str, marker: &RestoreMarker) -> Result<()> {
    let config = EnvironmentConfig::parse(&paths.environment_config(name), &marker.old_config)?;
    let Some(wine) = config.wine() else {
        bail!("restore marker config points to a different prefix");
    };
    if paths::resolve(&paths.config_path(&wine.prefix)?) != marker.prefix {
        bail!("restore marker config points to a different prefix");
    }
    Ok(())
}

fn registered(paths: &Paths, name: &str) -> Result<(Environment, String, PathBuf)> {
    let env = Environment::load(paths, name)?;
    let text = fs::read_to_string(&env.path)?;
    let prefix = paths::resolve(&env.prefix(paths)?);
    paths.check_prefix(&prefix)?;
    Ok((env, text, prefix))
}

/// How a manifest names its prefix: relative to a portable root, so a moved
/// folder still matches, or absolute as before.
fn prefix_identity(paths: &Paths, prefix: &Path) -> Result<PathBuf> {
    if paths.portable_root().is_none() {
        return Ok(prefix.to_owned());
    }
    paths
        .portable_relative(prefix)
        .ok_or_else(|| anyhow!("prefix is outside the portable folder"))
}

/// Points a copied snapshot's manifest at `prefix`, named the way `to` names
/// it, and returns the layout it records.
pub(crate) fn relocate_manifest(
    to: &Paths,
    snapshot: &Path,
    prefix: &Path,
) -> Result<ImportLayout> {
    let path = snapshot.join(MANIFEST);
    let mut manifest: Manifest = serde_json::from_slice(&fs::read(&path)?)?;
    manifest.prefix = prefix_identity(to, prefix)?;
    fs::write(&path, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(manifest.layout)
}

fn stored_marker(paths: &Paths, marker: &RestoreMarker) -> Result<RestoreMarker> {
    let mut stored = marker.clone();
    stored.prefix = prefix_identity(paths, &marker.prefix)?;
    stored.staging = prefix_identity(paths, &marker.staging)?;
    Ok(stored)
}

fn inspect_prefix(
    paths: &Paths,
    global: &GlobalConfig,
    env: &Environment,
    prefix: &Path,
    cancel: &Token,
    progress: impl FnMut(ImportProgress),
) -> Result<import::Inspection> {
    let wine = env.wine()?;
    let runner = if import::root_layout_ambiguous(prefix) {
        Some(Runners::new(paths, global)?.resolve(&wine.runner)?)
    } else {
        None
    };
    let inspected =
        import::inspect_root_with(prefix, runner.as_ref(), wine.arch, cancel, progress)?;
    if inspected.source_root != prefix {
        bail!("configured prefix root changed during inspection");
    }
    Ok(inspected)
}

fn validate_label(label: &str) -> Result<()> {
    if !name::is_valid(label) {
        bail!("invalid snapshot label `{label}`");
    }
    Ok(())
}

fn check_sharing(paths: &Paths, name: &str, prefix: &Path) -> Result<()> {
    let shared = environment::checked_sharing(paths, name, prefix)?;
    if !shared.names.is_empty() {
        bail!(
            "prefix is shared with environment(s): {}",
            shared.names.join(", ")
        );
    }
    Ok(())
}

fn ensure_create_same(a: &CreateInspection, b: &CreateInspection) -> Result<()> {
    if a.prefix != b.prefix
        || a.snapshot != b.snapshot
        || a.layout != b.layout
        || a.config_text != b.config_text
        || a.fingerprint != b.fingerprint
    {
        bail!("reviewed environment changed; inspect it again");
    }
    Ok(())
}

fn ensure_restore_same(a: &RestoreInspection, b: &RestoreInspection) -> Result<()> {
    if a.prefix != b.prefix
        || a.snapshot != b.snapshot
        || a.current_config != b.current_config
        || a.saved_config != b.saved_config
        || a.current_runner != b.current_runner
        || a.saved_runner != b.saved_runner
        || a.layout != b.layout
        || a.gog_id_added != b.gog_id_added
        || a.current_fingerprint != b.current_fingerprint
        || a.snapshot_fingerprint != b.snapshot_fingerprint
    {
        bail!("reviewed restore changed; inspect it again");
    }
    Ok(())
}

fn read_manifest(
    paths: &Paths,
    path: &Path,
    name: &str,
    label: &str,
    prefix: &Path,
) -> Result<Manifest> {
    ensure_directory(
        path.parent()
            .ok_or_else(|| anyhow!("snapshot has no parent"))?,
    )?;
    ensure_directory(
        path.parent()
            .and_then(Path::parent)
            .ok_or_else(|| anyhow!("snapshot storage has no parent"))?,
    )?;
    ensure_directory(path)?;
    ensure_regular(&path.join(MANIFEST))?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(path.join(MANIFEST))?)?;
    if manifest.version != VERSION
        || manifest.environment != name
        || manifest.label != label
        || manifest.prefix != prefix_identity(paths, prefix)?
    {
        bail!(
            "snapshot manifest does not match `{name}/{label}` and prefix {}",
            prefix.display()
        );
    }
    ensure_directory(&path.join("prefix"))?;
    ensure_regular(&path.join("config.toml"))?;
    Ok(manifest)
}

fn ensure_storage_dirs(parent: &Path) -> Result<()> {
    let root = parent
        .parent()
        .ok_or_else(|| anyhow!("snapshot storage has no parent"))?;
    if !root.exists() {
        fs::create_dir(root)?;
    }
    ensure_directory(root)?;
    if !parent.exists() {
        fs::create_dir(parent)?;
    }
    ensure_directory(parent)
}

fn ensure_directory(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        bail!("expected directory without symlink: {}", path.display());
    }
    Ok(())
}

fn ensure_regular(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        bail!("expected regular file: {}", path.display());
    }
    Ok(())
}

fn marker_path(paths: &Paths, name: &str) -> PathBuf {
    paths.restore_marker(name)
}

fn write_marker(path: &Path, marker: &RestoreMarker) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("marker has no parent"))?;
    fs::create_dir_all(parent)?;
    ensure_directory(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&serde_json::to_vec_pretty(marker)?)?;
    file.as_file().sync_all()?;
    crate::download::publish(file.path(), path)?;
    sync_dir(path.parent().unwrap())
}

fn read_marker(paths: &Paths, path: &Path, name: &str) -> Result<RestoreMarker> {
    ensure_regular(path)?;
    let mut marker: RestoreMarker = serde_json::from_slice(&fs::read(path)?)?;
    if let Some(root) = paths.portable_root() {
        if marker.prefix.is_absolute() || marker.staging.is_absolute() {
            bail!("invalid restore marker at {}", path.display());
        }
        marker.prefix = root.join(&marker.prefix);
        marker.staging = root.join(&marker.staging);
    }
    if marker.version != VERSION
        || marker.environment != name
        || !marker.prefix.is_absolute()
        || marker.staging.parent() != marker.prefix.parent()
        || !marker
            .staging
            .file_name()
            .is_some_and(|s| s.to_string_lossy().starts_with(".wineo-restore-"))
    {
        bail!("invalid restore marker at {}", path.display());
    }
    Ok(marker)
}

fn rollback_locked(paths: &Paths, marker_path: &Path, marker: &RestoreMarker) -> Result<()> {
    let live = directory_identity(&marker.prefix)?;
    if live != marker.old_directory {
        if directory_identity(&marker.staging)? != marker.old_directory {
            bail!(
                "cannot identify original prefix at {} or {}",
                marker.prefix.display(),
                marker.staging.display()
            );
        }
        if import::fingerprint(&marker.staging)? != marker.old_fingerprint {
            bail!("original prefix changed at {}", marker.staging.display());
        }
        exchange(&marker.prefix, &marker.staging)?;
        sync_dir(marker.prefix.parent().unwrap())?;
    } else if import::fingerprint(&marker.prefix)? != marker.old_fingerprint {
        bail!("original prefix changed at {}", marker.prefix.display(),);
    }
    let config_path = paths.environment_config(&marker.environment);
    // Build discovery must not leave the exchanged prefix awaiting recovery.
    let _guards = EnvironmentConfig::parse(&config_path, &marker.old_config)
        .ok()
        .and_then(|config| {
            let wine = config.wine()?;
            let global = GlobalConfig::load(paths).ok()?;
            build_lock::config(
                paths,
                &global,
                Some(&wine.runner),
                wine.dxvk.as_deref(),
                wine.vkd3d.as_deref(),
            )
            .ok()
        })
        .unwrap_or_default();
    write_config_atomic(&config_path, &marker.old_config)?;
    fs::remove_file(marker_path)?;
    sync_dir(marker_path.parent().unwrap())?;
    remove_tree(&marker.staging).with_context(|| {
        format!(
            "original prefix is back, but the restored copy remains at {}",
            marker.staging.display()
        )
    })?;
    Ok(())
}

fn with_config_build_locks<T>(
    paths: &Paths,
    global: &GlobalConfig,
    config_path: &Path,
    contents: &str,
    publish: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let config = EnvironmentConfig::parse(config_path, contents)?;
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
    publish()
}

fn directory_identity(path: &Path) -> Result<DirectoryIdentity> {
    use std::os::unix::fs::MetadataExt;

    ensure_directory(path)?;
    let metadata = fs::symlink_metadata(path)?;
    Ok(DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn exchange(a: &Path, b: &Path) -> Result<()> {
    renameat_with(
        rustix::fs::CWD,
        a,
        rustix::fs::CWD,
        b,
        RenameFlags::EXCHANGE,
    )
    .with_context(|| {
        format!(
            "atomic directory exchange unavailable for {} and {}",
            a.display(),
            b.display()
        )
    })
}

fn write_synced(path: &Path, content: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

fn write_config_atomic(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("config has no parent"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|err| anyhow!(err.error))?;
    sync_dir(parent)
}

fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?
        .sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))
}

fn remove_tree(path: &Path) -> Result<()> {
    ensure_directory(path)?;
    // remove_dir_all would empty a mount before failing on its mount point.
    if let Some(mount) = crate::trash::foreign_mount(path)? {
        bail!(
            "{} has a mount at {}, so it was kept",
            path.display(),
            mount.display()
        );
    }
    fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))
}

fn is_btrfs_subvolume(path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    const BTRFS_SUPER_MAGIC: u32 = 0x9123_683e;
    let metadata = fs::symlink_metadata(path)?;
    let stat = rustix::fs::statfs(path)?;
    Ok(metadata.ino() == 256 && stat.f_type as u32 == BTRFS_SUPER_MAGIC)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::fs::{MetadataExt, symlink};

    use super::*;

    #[test]
    fn restore_publishes_config_while_all_named_builds_are_locked() {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        let config_path = paths.environment_config("game");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let config = "prefix = '/tmp/prefix'\nrunner = 'build'\narch = 'win64'\ndxvk = '2.0'\nvkd3d = '3.0'\n";
        let global = GlobalConfig::default();

        with_config_build_locks(&paths, &global, &config_path, config, || {
            for (kind, name) in [
                (build_lock::Kind::Runner, "build"),
                (build_lock::Kind::Dxvk, "2.0"),
                (build_lock::Kind::Vkd3d, "3.0"),
            ] {
                assert!(build_lock::try_exclusive(&paths, kind, name)?.is_none());
            }
            write_config_atomic(&config_path, config)
        })
        .unwrap();

        for (kind, name) in [
            (build_lock::Kind::Runner, "build"),
            (build_lock::Kind::Dxvk, "2.0"),
            (build_lock::Kind::Vkd3d, "3.0"),
        ] {
            assert!(
                build_lock::try_exclusive(&paths, kind, name)
                    .unwrap()
                    .is_some()
            );
        }
    }

    fn fixture(proton: bool) -> (tempfile::TempDir, Paths, PathBuf, String) {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        let prefix = root.path().join("live");
        let wine = if proton {
            prefix.join("pfx")
        } else {
            prefix.clone()
        };
        fs::create_dir_all(wine.join("drive_c")).unwrap();
        fs::write(
            wine.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        fs::write(wine.join("drive_c/game.exe"), "before").unwrap();
        fs::write(prefix.join("linked"), "hardlink").unwrap();
        fs::hard_link(prefix.join("linked"), prefix.join("linked-again")).unwrap();
        symlink(prefix.join("linked"), prefix.join("internal")).unwrap();
        symlink("/external/game.exe", prefix.join("external")).unwrap();
        let config = format!(
            "# keep this comment\nprefix = '{}'\nrunner = 'wine'\narch = 'win64'\n[targets.game]\nexe = '{}'\n",
            prefix.display(),
            wine.join("drive_c/game.exe").display()
        );
        fs::create_dir_all(paths.environments_dir()).unwrap();
        fs::write(paths.environment_config("game"), &config).unwrap();
        (root, paths, prefix, config)
    }

    fn reflinks_supported(prefix: &Path) -> bool {
        let destination = prefix.join("reflink-probe");
        let input = File::open(prefix.join("linked")).unwrap();
        let output = File::create(&destination).unwrap();
        let supported = rustix::fs::ioctl_ficlone(&output, &input).is_ok();
        fs::remove_file(destination).unwrap();
        supported
    }

    fn interrupted_marker(paths: &Paths, prefix: &Path, config: &str) -> RestoreMarker {
        let staging = tempfile::Builder::new()
            .prefix(".wineo-restore-")
            .tempdir_in(prefix.parent().unwrap())
            .unwrap()
            .keep();
        let marker = RestoreMarker {
            version: VERSION,
            environment: "game".to_owned(),
            prefix: prefix.to_owned(),
            staging,
            old_directory: directory_identity(prefix).unwrap(),
            old_fingerprint: import::fingerprint(prefix).unwrap(),
            old_config: config.to_owned(),
        };
        write_marker(&marker_path(paths, "game"), &marker).unwrap();
        marker
    }

    #[test]
    fn create_inspection_and_revalidation_are_cancellable() {
        let (_root, paths, prefix, config) = fixture(false);
        let global = GlobalConfig::default();
        let token = Token::new();
        let error = inspect_create_with(&paths, &global, "game", "one", &token, |event| {
            assert!(matches!(event, ImportProgress::Checking { entries } if entries > 0));
            token.cancel();
        })
        .unwrap_err();
        assert!(error.is::<Canceled>(), "{error}");
        let supports_reflinks = reflinks_supported(&prefix);
        let review = inspect_create(&paths, &global, "game", "one").unwrap();
        for cancel_after_copy in [false, true] {
            if cancel_after_copy && !supports_reflinks {
                continue;
            }
            let token = Token::new();
            let mut copied = false;
            let error =
                create_reviewed_with(&paths, &global, &review, &token, |event| match event {
                    SnapshotEvent::Progress(ImportProgress::Copied { .. }) => copied = true,
                    SnapshotEvent::Progress(ImportProgress::Checking { .. })
                        if !cancel_after_copy || copied =>
                    {
                        token.cancel()
                    }
                    SnapshotEvent::Finishing => panic!("canceled before publication"),
                    _ => {}
                })
                .unwrap_err();
            assert!(error.is::<Canceled>(), "{error}");
            assert!(!review.snapshot.exists());
            assert_eq!(
                fs::read_to_string(paths.environment_config("game")).unwrap(),
                config
            );
            assert_eq!(
                fs::read_to_string(prefix.join("drive_c/game.exe")).unwrap(),
                "before"
            );
            if let Ok(entries) = fs::read_dir(review.snapshot.parent().unwrap()) {
                assert_eq!(entries.count(), 0);
            }
        }
    }

    #[test]
    fn restore_checks_cancel_before_swap_and_keep_live_state() {
        let (_root, paths, prefix, config) = fixture(false);
        if !reflinks_supported(&prefix) {
            return;
        }
        let global = GlobalConfig::default();
        let create = inspect_create(&paths, &global, "game", "one").unwrap();
        create_reviewed(&paths, &global, &create, &Token::new(), |_| {}).unwrap();
        fs::write(prefix.join("drive_c/game.exe"), "later").unwrap();
        let current_config = format!("# current edit\n{config}");
        fs::write(paths.environment_config("game"), &current_config).unwrap();
        let review = inspect_restore(&paths, &global, "game", "one").unwrap();
        // The second check walks the saved tree, after the live tree.
        let token = Token::new();
        let mut checks = 0;
        let error = inspect_restore_with(&paths, &global, "game", "one", &token, |_| {
            checks += 1;
            if checks == 2 {
                token.cancel();
            }
        })
        .unwrap_err();
        assert!(error.is::<Canceled>(), "{error}");
        assert_eq!(checks, 2);
        for cancel_after_copy in [false, true] {
            let token = Token::new();
            let mut copied = false;
            let error =
                restore_reviewed_with(&paths, &global, &review, &token, |event| match event {
                    SnapshotEvent::Progress(ImportProgress::Copied { .. }) => copied = true,
                    SnapshotEvent::Progress(ImportProgress::Checking { .. })
                        if !cancel_after_copy || copied =>
                    {
                        token.cancel()
                    }
                    SnapshotEvent::Finishing => panic!("canceled before swap"),
                    _ => {}
                })
                .unwrap_err();
            assert!(error.is::<Canceled>(), "{error}");
            assert_eq!(
                fs::read_to_string(paths.environment_config("game")).unwrap(),
                current_config
            );
            assert_eq!(
                fs::read_to_string(prefix.join("drive_c/game.exe")).unwrap(),
                "later"
            );
            assert!(!marker_path(&paths, "game").exists());
            assert!(
                fs::read_dir(prefix.parent().unwrap())
                    .unwrap()
                    .all(|entry| {
                        !entry
                            .unwrap()
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".wineo-restore-")
                    })
            );
        }
    }

    #[test]
    fn late_cancel_at_finishing_preserves_create_and_restore_success() {
        let (_root, paths, prefix, config) = fixture(false);
        if !reflinks_supported(&prefix) {
            return;
        }
        let global = GlobalConfig::default();
        let create = inspect_create(&paths, &global, "game", "one").unwrap();
        let token = Token::new();
        let mut finishing = 0;
        let mut finished = false;
        create_reviewed_with(&paths, &global, &create, &token, |event| match event {
            SnapshotEvent::Finishing => {
                assert!(!create.snapshot.exists());
                finishing += 1;
                token.cancel();
            }
            SnapshotEvent::Progress(ImportProgress::Finished { .. }) => finished = true,
            _ => {}
        })
        .unwrap();
        assert_eq!(finishing, 1);
        assert!(finished);
        assert!(token.is_canceled());
        fs::write(prefix.join("drive_c/game.exe"), "later").unwrap();
        fs::write(
            paths.environment_config("game"),
            format!("# edit\n{config}"),
        )
        .unwrap();
        let restore = inspect_restore(&paths, &global, "game", "one").unwrap();
        let token = Token::new();
        let mut finishing = 0;
        let mut finished = false;
        restore_reviewed_with(&paths, &global, &restore, &token, |event| match event {
            SnapshotEvent::Finishing => {
                assert!(!marker_path(&paths, "game").exists());
                assert_eq!(
                    fs::read_to_string(prefix.join("drive_c/game.exe")).unwrap(),
                    "later"
                );
                finishing += 1;
                token.cancel();
            }
            SnapshotEvent::Progress(ImportProgress::Finished { .. }) => finished = true,
            _ => {}
        })
        .unwrap();
        assert_eq!(finishing, 1);
        assert!(finished);
        assert!(token.is_canceled());
        assert_eq!(
            fs::read_to_string(prefix.join("drive_c/game.exe")).unwrap(),
            "before"
        );
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            config
        );
        assert!(!marker_path(&paths, "game").exists());
    }

    #[test]
    fn explicit_validate_recovery_rolls_back_and_batch_paths_leave_marker_alone() {
        let (_root, paths, prefix, config) = fixture(false);
        let marker = interrupted_marker(&paths, &prefix, &config);
        exchange(&prefix, &marker.staging).unwrap();
        fs::write(prefix.join("new-file"), "new").unwrap();
        fs::write(paths.environment_config("game"), "invalid = [").unwrap();
        let report = crate::validate::check_environment(&paths, "game").unwrap();
        let fix = report.problems[0].fix.as_ref().unwrap();
        assert!(
            matches!(fix, crate::validate::Fix::RecoverSnapshot { environment } if environment == "game")
        );
        assert!(report.fixes().is_empty());
        assert!(
            crate::validate::apply(
                &paths,
                fix,
                &Token::new(),
                crate::winetricks::EnsureOutput::Stderr,
                |_| {},
                |_, _| panic!("no download"),
            )
            .is_err()
        );
        assert!(marker_path(&paths, "game").exists());
        // Recovery is non-cancellable, even for an already canceled token.
        let token = Token::new();
        token.cancel();
        crate::validate::apply_single(
            &paths,
            fix,
            &token,
            crate::winetricks::EnsureOutput::Stderr,
            |_| {},
            |_, _| panic!("no download"),
        )
        .unwrap();
        assert_eq!(directory_identity(&prefix).unwrap(), marker.old_directory);
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            config
        );
        assert_eq!(
            fs::read_to_string(prefix.join("drive_c/game.exe")).unwrap(),
            "before"
        );
        assert!(!prefix.join("new-file").exists());
        assert!(!marker_path(&paths, "game").exists());
        assert!(!marker.staging.exists());
    }

    #[test]
    fn recovery_refuses_busy_changed_and_unidentified_originals() {
        for refusal in [
            "busy",
            "changed-live",
            "changed-staging",
            "identity",
            "config",
        ] {
            let (_root, paths, prefix, config) = fixture(false);
            let mut marker = interrupted_marker(&paths, &prefix, &config);
            let lock = if refusal == "busy" {
                Some(
                    Locks::new(&paths)
                        .acquire(Operation::Snapshot, "game", &[&prefix])
                        .unwrap(),
                )
            } else {
                None
            };
            match refusal {
                "changed-live" => fs::write(prefix.join("linked"), "changed").unwrap(),
                "changed-staging" => {
                    exchange(&prefix, &marker.staging).unwrap();
                    fs::write(marker.staging.join("linked"), "changed").unwrap();
                }
                "identity" => {
                    marker.old_directory.inode = 0;
                    fs::remove_file(marker_path(&paths, "game")).unwrap();
                    write_marker(&marker_path(&paths, "game"), &marker).unwrap();
                }
                "config" => {
                    marker.old_config =
                        "prefix = '/different'\nrunner = 'wine'\narch = 'win64'\n".to_owned();
                    fs::remove_file(marker_path(&paths, "game")).unwrap();
                    write_marker(&marker_path(&paths, "game"), &marker).unwrap();
                }
                _ => {}
            }
            let before = directory_identity(&prefix).unwrap();
            let err = recover(&paths, "game").unwrap_err();
            let expected = match refusal {
                "busy" => "busy",
                "changed-live" | "changed-staging" => "original prefix changed",
                "identity" => "cannot identify original prefix",
                "config" => "different prefix",
                _ => unreachable!(),
            };
            assert!(err.to_string().contains(expected), "{refusal}: {err}");
            assert_eq!(directory_identity(&prefix).unwrap(), before);
            assert_eq!(
                fs::read_to_string(paths.environment_config("game")).unwrap(),
                config
            );
            assert!(marker_path(&paths, "game").exists());
            assert!(marker.staging.exists());
            drop(lock);
        }
    }

    #[test]
    fn restoring_a_legacy_config_keeps_it_unversioned_until_edited() {
        let (_root, paths, _prefix, config) = fixture(false);
        let global = GlobalConfig::default();
        let review = inspect_create(&paths, &global, "game", "legacy").unwrap();
        create_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap();
        crate::settings::update(
            &paths,
            crate::settings::ConfigFile::Environment("game"),
            &[("runner", "other")],
            &[],
        )
        .unwrap();
        let current = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert!(current.contains("schema_version = 1\n"), "{current}");

        let restore = inspect_restore(&paths, &global, "game", "legacy").unwrap();
        restore_reviewed(&paths, &global, &restore, &Token::new(), |_| {}).unwrap();
        let restored = fs::read_to_string(paths.environment_config("game")).unwrap();
        assert_eq!(restored, config);
        let path = paths.environment_config("game");
        assert_eq!(
            EnvironmentConfig::parse(&path, &restored)
                .unwrap()
                .schema_version,
            0
        );

        crate::settings::update(
            &paths,
            crate::settings::ConfigFile::Environment("game"),
            &[("gamemode", "true")],
            &[],
        )
        .unwrap();
        let edited = fs::read_to_string(&path).unwrap();
        assert!(edited.starts_with("# keep this comment\n"), "{edited}");
        assert_eq!(
            EnvironmentConfig::parse(&path, &edited)
                .unwrap()
                .schema_version,
            1
        );
    }

    #[test]
    fn restore_refuses_a_snapshot_with_an_unsupported_version() {
        let (_root, paths, _prefix, config) = fixture(false);
        let global = GlobalConfig::default();
        let review = inspect_create(&paths, &global, "game", "future").unwrap();
        let info = create_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap();
        let saved = info.path.join("config.toml");
        fs::write(&saved, format!("schema_version = 2\n{config}")).unwrap();
        let err = inspect_restore(&paths, &global, "game", "future").unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<crate::error::Error>(),
                Some(crate::error::Error::SchemaVersion { .. })
            ),
            "{err}"
        );
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            config
        );
    }

    #[test]
    fn create_restore_and_delete_preserve_config_and_links() {
        for proton in [false, true] {
            let (_root, paths, prefix, config) = fixture(proton);
            let global = GlobalConfig::default();
            let review = inspect_create(&paths, &global, "game", "before").unwrap();
            assert_eq!(review.layout == ImportLayout::Proton, proton);
            let info = create_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap();
            assert_eq!(list(&paths, "game").unwrap()[0].label, "before");
            assert_eq!(
                fs::read_to_string(info.path.join("config.toml")).unwrap(),
                config
            );
            assert_eq!(
                fs::read_link(info.path.join("prefix/internal")).unwrap(),
                info.path.join("prefix/linked")
            );
            assert_eq!(
                fs::read_link(info.path.join("prefix/external")).unwrap(),
                Path::new("/external/game.exe")
            );
            assert_eq!(
                fs::metadata(info.path.join("prefix/linked")).unwrap().ino(),
                fs::metadata(info.path.join("prefix/linked-again"))
                    .unwrap()
                    .ino()
            );
            let wine = if proton {
                prefix.join("pfx")
            } else {
                prefix.clone()
            };
            fs::write(wine.join("drive_c/game.exe"), "after").unwrap();
            fs::write(
                paths.environment_config("game"),
                config.replace("runner = 'wine'", "runner = 'other'"),
            )
            .unwrap();
            let restore = inspect_restore(&paths, &global, "game", "before").unwrap();
            assert_eq!(restore.current_runner, "other");
            assert_eq!(restore.saved_runner, "wine");
            restore_reviewed(&paths, &global, &restore, &Token::new(), |_| {}).unwrap();
            assert_eq!(
                fs::read_to_string(wine.join("drive_c/game.exe")).unwrap(),
                "before"
            );
            assert_eq!(
                fs::read_to_string(paths.environment_config("game")).unwrap(),
                config
            );
            assert_eq!(
                fs::read_link(prefix.join("internal")).unwrap(),
                prefix.join("linked")
            );
            assert!(!marker_path(&paths, "game").exists());
            delete(&paths, "game", "before").unwrap();
            assert!(list(&paths, "game").unwrap().is_empty());
            assert!(prefix.exists());
        }
    }

    #[test]
    fn collision_changed_source_and_shared_prefix_are_rejected() {
        let (_root, paths, prefix, config) = fixture(false);
        let global = GlobalConfig::default();
        let review = inspect_create(&paths, &global, "game", "one").unwrap();
        fs::write(prefix.join("drive_c/game.exe"), "changed").unwrap();
        assert!(create_reviewed(&paths, &global, &review, &Token::new(), |_| {}).is_err());
        assert!(!review.snapshot.exists());
        let review = inspect_create(&paths, &global, "game", "one").unwrap();
        create_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap();
        assert!(inspect_create(&paths, &global, "game", "one").is_err());
        fs::write(paths.environment_config("other"), config).unwrap();
        assert!(inspect_create(&paths, &global, "game", "two").is_err());
        assert!(inspect_restore(&paths, &global, "game", "one").is_err());
    }

    fn portable(home: &Path, root: &Path) -> Paths {
        let home = home.to_owned();
        Paths::select(
            move |key| (key == "HOME").then(|| home.clone().into_os_string()),
            Some(root),
        )
        .unwrap()
    }

    /// A portable folder holding one environment whose config paths are all
    /// relative to the root.
    fn portable_fixture() -> (tempfile::TempDir, PathBuf, String) {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let usb = root.path().join("usb");
        let prefix = usb.join("prefixes/game");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        fs::write(prefix.join("drive_c/game.exe"), "before").unwrap();
        let config = "prefix = 'prefixes/game'\nrunner = 'wine'\narch = 'win64'\n[targets.game]\nexe = 'prefixes/game/drive_c/game.exe'\n".to_owned();
        fs::create_dir_all(usb.join("config/environments")).unwrap();
        fs::write(usb.join("config/environments/game.toml"), &config).unwrap();
        (root, usb, config)
    }

    #[test]
    fn portable_snapshots_survive_moving_the_folder() {
        let (root, usb, config) = portable_fixture();
        let global = GlobalConfig::default();
        let paths = portable(root.path(), &usb);
        let review = inspect_create(&paths, &global, "game", "one").unwrap();
        create_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap();
        let manifest = usb
            .join("prefixes/.wineo-snapshots/game/one")
            .join(MANIFEST);
        let manifest: Manifest = serde_json::from_slice(&fs::read(manifest).unwrap()).unwrap();
        assert_eq!(manifest.prefix, Path::new("prefixes/game"));

        let moved = root.path().join("moved");
        fs::rename(&usb, &moved).unwrap();
        let paths = portable(root.path(), &moved);
        fs::write(moved.join("prefixes/game/drive_c/game.exe"), "after").unwrap();
        assert_eq!(list(&paths, "game").unwrap().len(), 1);
        let restore = inspect_restore(&paths, &global, "game", "one").unwrap();
        assert_eq!(restore.prefix, moved.join("prefixes/game"));
        restore_reviewed(&paths, &global, &restore, &Token::new(), |_| {}).unwrap();
        assert_eq!(
            fs::read_to_string(moved.join("prefixes/game/drive_c/game.exe")).unwrap(),
            "before"
        );
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            config
        );
    }

    #[test]
    fn a_portable_manifest_for_another_prefix_is_refused() {
        let (root, usb, _) = portable_fixture();
        let global = GlobalConfig::default();
        let paths = portable(root.path(), &usb);
        let review = inspect_create(&paths, &global, "game", "one").unwrap();
        create_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap();
        let path = usb
            .join("prefixes/.wineo-snapshots/game/one")
            .join(MANIFEST);
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replace("prefixes/game", "prefixes/other")).unwrap();
        assert!(inspect_restore(&paths, &global, "game", "one").is_err());
    }

    #[test]
    fn a_portable_restore_interrupted_before_a_move_recovers_after_it() {
        let (root, usb, config) = portable_fixture();
        let global = GlobalConfig::default();
        let paths = portable(root.path(), &usb);
        let prefix = usb.join("prefixes/game");
        let review = inspect_create(&paths, &global, "game", "one").unwrap();
        create_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap();
        fs::write(prefix.join("drive_c/game.exe"), "later").unwrap();
        let old_fingerprint = import::fingerprint(&prefix).unwrap();
        let restore = inspect_restore(&paths, &global, "game", "one").unwrap();
        let stage = tempfile::Builder::new()
            .prefix(".wineo-restore-")
            .tempdir_in(prefix.parent().unwrap())
            .unwrap();
        import::copy_tree_strict_reflink(
            &restore.snapshot.join("prefix"),
            stage.path(),
            &prefix,
            &Token::new(),
            &mut |_| {},
        )
        .unwrap();
        let stage = stage.keep();
        let marker = RestoreMarker {
            version: VERSION,
            environment: "game".to_owned(),
            prefix: prefix.clone(),
            staging: stage.clone(),
            old_directory: directory_identity(&prefix).unwrap(),
            old_fingerprint,
            old_config: config.clone(),
        };
        let marker_path = marker_path(&paths, "game");
        assert!(marker_path.starts_with(usb.join("prefixes/.wineo-snapshots/game")));
        write_marker(&marker_path, &stored_marker(&paths, &marker).unwrap()).unwrap();
        exchange(&prefix, &stage).unwrap();
        let text = fs::read_to_string(&marker_path).unwrap();
        assert!(!text.contains(usb.to_str().unwrap()), "{text}");

        let moved = root.path().join("moved");
        fs::rename(&usb, &moved).unwrap();
        let paths = portable(root.path(), &moved);
        assert!(Environment::load(&paths, "game").is_err());
        recover(&paths, "game").unwrap();
        assert_eq!(
            fs::read_to_string(moved.join("prefixes/game/drive_c/game.exe")).unwrap(),
            "later"
        );
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            config
        );
        assert!(!paths.restore_marker("game").exists());
        assert!(Environment::load(&paths, "game").is_ok());
    }

    #[test]
    fn a_portable_prefix_outside_the_folder_is_refused() {
        let (root, usb, _) = portable_fixture();
        let outside = root.path().join("outside");
        fs::rename(usb.join("prefixes/game"), &outside).unwrap();
        fs::write(
            usb.join("config/environments/game.toml"),
            format!(
                "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
                outside.display()
            ),
        )
        .unwrap();
        let paths = portable(root.path(), &usb);
        let err = inspect_create(&paths, &GlobalConfig::default(), "game", "one").unwrap_err();
        assert!(err.to_string().contains("portable prefixes"), "{err}");
    }

    #[test]
    fn recovery_restores_original_after_interrupted_exchange() {
        let (_root, paths, prefix, original_config) = fixture(false);
        let global = GlobalConfig::default();
        let review = inspect_create(&paths, &global, "game", "one").unwrap();
        create_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap();
        fs::write(prefix.join("drive_c/game.exe"), "later").unwrap();
        let old_fingerprint = import::fingerprint(&prefix).unwrap();
        let restore = inspect_restore(&paths, &global, "game", "one").unwrap();
        let stage = tempfile::Builder::new()
            .prefix(".wineo-restore-")
            .tempdir_in(prefix.parent().unwrap())
            .unwrap();
        import::copy_tree_strict_reflink(
            &restore.snapshot.join("prefix"),
            stage.path(),
            &prefix,
            &Token::new(),
            &mut |_| {},
        )
        .unwrap();
        let stage = stage.keep();
        let marker = RestoreMarker {
            version: VERSION,
            environment: "game".to_owned(),
            prefix: prefix.clone(),
            staging: stage.clone(),
            old_directory: directory_identity(&prefix).unwrap(),
            old_fingerprint,
            old_config: original_config.clone(),
        };
        let marker_path = marker_path(&paths, "game");
        write_marker(&marker_path, &marker).unwrap();
        exchange(&prefix, &stage).unwrap();
        assert!(Environment::load(&paths, "game").is_err());
        recover(&paths, "game").unwrap();
        assert_eq!(
            fs::read_to_string(prefix.join("drive_c/game.exe")).unwrap(),
            "later"
        );
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            original_config
        );
        assert!(restore.snapshot.exists());
        assert!(!stage.exists());
        assert!(!marker_path.exists());
    }

    #[test]
    fn rollback_with_malformed_global_config_restores_config_and_clears_marker() {
        let (_root, paths, prefix, config) = fixture(false);
        let old_config = config.replace("runner = 'wine'", "runner = 'build'");
        let original = directory_identity(&prefix).unwrap();
        let stage = tempfile::Builder::new()
            .prefix(".wineo-restore-")
            .tempdir_in(prefix.parent().unwrap())
            .unwrap()
            .keep();
        let marker = RestoreMarker {
            version: VERSION,
            environment: "game".to_owned(),
            prefix: prefix.clone(),
            staging: stage.clone(),
            old_directory: original,
            old_fingerprint: import::fingerprint(&prefix).unwrap(),
            old_config: old_config.clone(),
        };
        let marker_path = marker_path(&paths, "game");
        write_marker(&marker_path, &marker).unwrap();
        exchange(&prefix, &stage).unwrap();
        fs::write(paths.global_config(), "invalid = [TOML").unwrap();

        recover(&paths, "game").unwrap();

        assert_eq!(directory_identity(&prefix).unwrap(), original);
        assert_eq!(
            fs::read_to_string(prefix.join("drive_c/game.exe")).unwrap(),
            "before"
        );
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            old_config
        );
        assert!(!stage.exists());
        assert!(!marker_path.exists());
        assert!(fs::read_dir(paths.build_locks_dir()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("build-")
        }));
    }

    #[test]
    fn recovery_before_exchange_blocks_broken_config_and_restores_it() {
        let (_root, paths, prefix, original_config) = fixture(false);
        let stage = tempfile::Builder::new()
            .prefix(".wineo-restore-")
            .tempdir_in(prefix.parent().unwrap())
            .unwrap()
            .keep();
        let marker = RestoreMarker {
            version: VERSION,
            environment: "game".to_owned(),
            prefix: prefix.clone(),
            staging: stage.clone(),
            old_directory: directory_identity(&prefix).unwrap(),
            old_fingerprint: import::fingerprint(&prefix).unwrap(),
            old_config: original_config.clone(),
        };
        let marker_path = marker_path(&paths, "game");
        write_marker(&marker_path, &marker).unwrap();
        fs::write(paths.environment_config("game"), "invalid = [").unwrap();
        assert!(Environment::ensure_exists(&paths, "game").is_err());
        assert!(Environment::load(&paths, "game").is_err());
        recover(&paths, "game").unwrap();
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            original_config
        );
        assert!(!stage.exists());
        assert!(!marker_path.exists());
    }

    #[test]
    fn recovery_uses_directory_identity_when_contents_match() {
        let (_root, paths, prefix, config) = fixture(false);
        let original = directory_identity(&prefix).unwrap();
        let fingerprint = import::fingerprint(&prefix).unwrap();
        let stage = tempfile::Builder::new()
            .prefix(".wineo-restore-")
            .tempdir_in(prefix.parent().unwrap())
            .unwrap();
        import::copy_tree_strict_reflink(
            &prefix,
            stage.path(),
            &prefix,
            &Token::new(),
            &mut |_| {},
        )
        .unwrap();
        let stage = stage.keep();
        for name in ["internal", "external"] {
            let metadata = fs::symlink_metadata(prefix.join(name)).unwrap();
            filetime::set_symlink_file_times(
                stage.join(name),
                filetime::FileTime::from_last_access_time(&metadata),
                filetime::FileTime::from_last_modification_time(&metadata),
            )
            .unwrap();
        }
        let marker = RestoreMarker {
            version: VERSION,
            environment: "game".to_owned(),
            prefix: prefix.clone(),
            staging: stage.clone(),
            old_directory: original,
            old_fingerprint: fingerprint,
            old_config: config,
        };
        let marker_path = marker_path(&paths, "game");
        write_marker(&marker_path, &marker).unwrap();
        exchange(&prefix, &stage).unwrap();
        assert_eq!(import::fingerprint(&prefix).unwrap(), fingerprint);
        assert_ne!(directory_identity(&prefix).unwrap(), original);
        recover(&paths, "game").unwrap();
        assert_eq!(directory_identity(&prefix).unwrap(), original);
        assert!(!stage.exists());
        assert!(!marker_path.exists());
    }

    #[test]
    fn restore_refuses_snapshot_config_changed_during_copy() {
        let (_root, paths, prefix, config) = fixture(false);
        let global = GlobalConfig::default();
        let create = inspect_create(&paths, &global, "game", "one").unwrap();
        create_reviewed(&paths, &global, &create, &Token::new(), |_| {}).unwrap();
        fs::write(prefix.join("drive_c/game.exe"), "later").unwrap();
        let restore = inspect_restore(&paths, &global, "game", "one").unwrap();
        let mut changed = false;
        let result = restore_reviewed(&paths, &global, &restore, &Token::new(), |_| {
            if !changed {
                fs::write(
                    restore.snapshot.join("config.toml"),
                    config.replace("runner = 'wine'", "runner = 'other'"),
                )
                .unwrap();
                changed = true;
            }
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(prefix.join("drive_c/game.exe")).unwrap(),
            "later"
        );
        assert_eq!(
            fs::read_to_string(paths.environment_config("game")).unwrap(),
            config
        );
        assert!(!marker_path(&paths, "game").exists());
    }

    #[test]
    fn a_portable_restore_never_consults_host_gog_state() {
        use crate::gog::folder::{Committed, FolderLock, FolderState, Platform, Track};

        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let lookup = move |key: &str| (key == "HOME").then(|| home.clone().into_os_string());
        let host = Paths::select(lookup.clone(), None).unwrap();
        let portable = Paths::select(lookup, Some(&root.path().join("usb"))).unwrap();
        let folder = root.path().join("game");
        let state = FolderState {
            committed: Some(Committed {
                gog_id: 7,
                platform: Platform::Linux,
                language: "en".to_owned(),
                track: Track::Installer,
                products: Vec::new(),
                files: Vec::new(),
            }),
            ..FolderState::new(folder.clone(), Some("game".to_owned()))
        };
        FolderLock::exclusive(&host, &folder, &|| false)
            .unwrap()
            .write(&state)
            .unwrap();
        let path = Path::new("/test/game.toml");
        let plain = EnvironmentConfig::parse(path, "kind = 'native'\n").unwrap();
        let other = EnvironmentConfig::parse(path, "kind = 'native'\ngog_id = '8'\n").unwrap();
        assert_eq!(
            saved_gog_id(&host, "game", &plain).unwrap().as_deref(),
            Some("7")
        );
        assert!(saved_gog_id(&host, "game", &other).is_err());
        assert_eq!(saved_gog_id(&portable, "game", &plain).unwrap(), None);
        assert_eq!(saved_gog_id(&portable, "game", &other).unwrap(), None);
    }

    #[test]
    fn restore_refuses_a_prefix_with_a_mount_inside() {
        let (_root, paths, prefix, _config) = fixture(false);
        if !reflinks_supported(&prefix) {
            return;
        }
        let global = GlobalConfig::default();
        let create = inspect_create(&paths, &global, "game", "one").unwrap();
        create_reviewed(&paths, &global, &create, &Token::new(), |_| {}).unwrap();
        let review = inspect_restore(&paths, &global, "game", "one").unwrap();
        let shared = prefix.join("drive_c/shared");
        fs::create_dir(&shared).unwrap();
        fs::write(shared.join("external.txt"), "keep").unwrap();
        crate::trash::set_fake_mounts(vec![shared.clone()]);
        let error = inspect_restore(&paths, &global, "game", "one").unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "game's prefix has a mount at {}. Unmount it first.",
                shared.display()
            )
        );
        let error = restore_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap_err();
        assert!(error.to_string().contains("Unmount it first"), "{error}");
        crate::trash::set_fake_mounts(Vec::new());
        assert_eq!(
            fs::read_to_string(shared.join("external.txt")).unwrap(),
            "keep"
        );
        assert!(!marker_path(&paths, "game").exists());
    }

    #[test]
    fn snapshot_cleanup_stops_at_a_mount() {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let old = root.path().join(".wineo-restore-old");
        let mount = old.join("drive_c/shared");
        fs::create_dir_all(&mount).unwrap();
        fs::write(mount.join("external.txt"), "keep").unwrap();
        crate::trash::set_fake_mounts(vec![mount.clone()]);
        let error = remove_tree(&old).unwrap_err();
        crate::trash::set_fake_mounts(Vec::new());
        assert!(
            error.to_string().contains(&format!(
                "has a mount at {}, so it was kept",
                mount.display()
            )),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(mount.join("external.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn snapshot_cleanup_refuses_a_mounted_root() {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let snapshot = root.path().join("one");
        fs::create_dir_all(snapshot.join("prefix")).unwrap();
        fs::write(snapshot.join("keep.txt"), "keep").unwrap();
        crate::trash::set_fake_mounts(vec![snapshot.clone()]);
        let error = remove_tree(&snapshot).unwrap_err();
        crate::trash::set_fake_mounts(Vec::new());
        assert!(
            error.to_string().contains(&format!(
                "has a mount at {}, so it was kept",
                snapshot.display()
            )),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(snapshot.join("keep.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn recovery_names_an_unreadable_config_it_cannot_check() {
        let (_root, paths, prefix, config) = fixture(false);
        interrupted_marker(&paths, &prefix, &config);
        let broken = paths.environment_config("broken");
        fs::write(&broken, "prefix = [").unwrap();
        let error = recover(&paths, "game").unwrap_err().to_string();
        assert!(
            error.starts_with(&format!("{} can't be read", broken.display())),
            "{error}"
        );
        assert!(marker_path(&paths, "game").exists());
        fs::remove_file(&broken).unwrap();
        recover(&paths, "game").unwrap();
    }
}
