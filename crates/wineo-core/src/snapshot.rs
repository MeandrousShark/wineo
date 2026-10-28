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
use crate::files;
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
    #[serde(default)]
    birth: Option<(i64, u32)>,
}

impl DirectoryIdentity {
    /// Recovery often runs after a crash and reboot, which can renumber a
    /// btrfs subvolume's device, so the device only counts without birth
    /// times to compare.
    fn matches(self, live: Self) -> bool {
        self.inode == live.inode
            && match (self.birth, live.birth) {
                (Some(saved), Some(now)) => saved == now,
                _ => self.device == live.device,
            }
    }
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
    // The review is checked once, under the locks. A second full read before
    // locking would only repeat that check.
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
    // The review is checked once, under the locks. A second full read before
    // locking would only repeat that check.
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
    if !marker.old_directory.matches(live) {
        if !marker
            .old_directory
            .matches(directory_identity(&marker.staging)?)
        {
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
            // Recovery restores a config file and must finish for a global
            // config that can't be read, so this only skips the advisory
            // build locks rather than refusing.
            let global = GlobalConfig::load(paths).ok()?;
            build_lock::wine(paths, &global, wine).ok()
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
        Some(wine) => build_lock::wine(paths, global, wine)?,
        None => Vec::new(),
    };
    publish()
}

fn directory_identity(path: &Path) -> Result<DirectoryIdentity> {
    use std::os::unix::fs::MetadataExt;

    ensure_directory(path)?;
    let metadata = fs::symlink_metadata(path)?;
    let birth = match rustix::fs::statx(
        rustix::fs::CWD,
        path,
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        rustix::fs::StatxFlags::BTIME,
    ) {
        Ok(stat) => (stat.stx_mask & rustix::fs::StatxFlags::BTIME.bits() != 0)
            .then_some((stat.stx_btime.tv_sec, stat.stx_btime.tv_nsec)),
        Err(rustix::io::Errno::NOSYS) => None,
        Err(err) => return Err(err.into()),
    };
    Ok(DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        birth,
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
    files::persist_synced(parent, path, content.as_bytes())?;
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
mod tests;
