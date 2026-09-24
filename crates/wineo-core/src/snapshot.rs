//! Local reflink restore points for registered prefixes.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::{RenameFlags, renameat_with};
use serde::{Deserialize, Serialize};

use crate::config::{EnvironmentConfig, GlobalConfig};
use crate::environment::{self, Environment};
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
    prefix: PathBuf,
    layout: ImportLayout,
    created_at: String,
}

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

#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    pub label: String,
    pub created_at: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RestoreOutcome {
    pub retained_old_prefix: Option<PathBuf>,
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
    validate_label(label)?;
    let (env, config_text, prefix) = registered(paths, name)?;
    let inspected = inspect_prefix(paths, global, &env, &prefix)?;
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
        runner: env.config.runner,
        external_dependencies: inspected.external_dependencies,
        config_text,
        fingerprint: inspected.fingerprint,
    })
}

pub fn create_reviewed(
    paths: &Paths,
    global: &GlobalConfig,
    reviewed: &CreateInspection,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ImportProgress),
) -> Result<SnapshotInfo> {
    let current = inspect_create(paths, global, &reviewed.name, &reviewed.label)?;
    ensure_create_same(reviewed, &current)?;
    let locks = Locks::new(paths);
    let _operation = locks.acquire(Operation::Snapshot, &reviewed.name, &[&reviewed.prefix])?;
    let _config_lock = locks.config(&paths.environment_config(&reviewed.name))?;
    let current = inspect_create(paths, global, &reviewed.name, &reviewed.label)?;
    ensure_create_same(reviewed, &current)?;
    Runtime::new(paths, &reviewed.name).check_quiet(&reviewed.name)?;
    import::check_cancelled(cancel)?;
    let parent = reviewed
        .snapshot
        .parent()
        .ok_or_else(|| anyhow!("snapshot has no parent"))?;
    ensure_storage_dirs(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".wineo-create-")
        .tempdir_in(parent)?;
    let staged_prefix = staging.path().join("prefix");
    import::copy_tree_strict_reflink(
        &reviewed.prefix,
        &staged_prefix,
        &reviewed.snapshot.join("prefix"),
        cancel,
        &mut progress,
    )?;
    import::check_cancelled(cancel)?;
    if import::fingerprint(&reviewed.prefix)? != reviewed.fingerprint {
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
        prefix: reviewed.prefix.clone(),
        layout: reviewed.layout,
        created_at: jiff::Timestamp::now().to_string(),
    };
    write_synced(
        &staging.path().join(MANIFEST),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    sync_dir(staging.path())?;
    crate::download::publish(staging.path(), &reviewed.snapshot)?;
    sync_dir(parent)?;
    progress(ImportProgress::Finished {
        entries: reviewed.fingerprint.entries,
    });
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
        let manifest = read_manifest(&path, name, label, &prefix)?;
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
    validate_label(label)?;
    let (env, current_config, prefix) = registered(paths, name)?;
    check_sharing(paths, name, &prefix)?;
    let current = inspect_prefix(paths, global, &env, &prefix)?;
    let snapshot = paths.snapshot_root(&prefix, name).join(label);
    import::reject_overlapping_paths(&prefix, &snapshot)?;
    let manifest = read_manifest(&snapshot, name, label, &prefix)?;
    let saved_config = fs::read_to_string(snapshot.join("config.toml"))?;
    let saved = EnvironmentConfig::parse(&snapshot.join("config.toml"), &saved_config)?;
    if paths::resolve(&paths.absolute(&saved.prefix)?) != prefix {
        bail!("snapshot config points to a different prefix");
    }
    let snap_prefix = snapshot.join("prefix");
    import::validate_staged_prefix(&snap_prefix, &snap_prefix, manifest.layout)?;
    let snapshot_fingerprint = import::fingerprint(&snap_prefix)?;
    Ok(RestoreInspection {
        name: name.to_owned(),
        label: label.to_owned(),
        prefix,
        snapshot,
        current_config,
        saved_config,
        current_runner: env.config.runner,
        saved_runner: saved.runner,
        layout: manifest.layout,
        current_fingerprint: current.fingerprint,
        snapshot_fingerprint,
    })
}

pub fn restore_reviewed(
    paths: &Paths,
    global: &GlobalConfig,
    reviewed: &RestoreInspection,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ImportProgress),
) -> Result<RestoreOutcome> {
    let current = inspect_restore(paths, global, &reviewed.name, &reviewed.label)?;
    ensure_restore_same(reviewed, &current)?;
    let locks = Locks::new(paths);
    let _operation = locks.acquire(Operation::Snapshot, &reviewed.name, &[&reviewed.prefix])?;
    let _config_lock = locks.config(&paths.environment_config(&reviewed.name))?;
    let current = inspect_restore(paths, global, &reviewed.name, &reviewed.label)?;
    ensure_restore_same(reviewed, &current)?;
    Runtime::new(paths, &reviewed.name).check_quiet(&reviewed.name)?;
    import::check_cancelled(cancel)?;
    let parent = reviewed
        .prefix
        .parent()
        .ok_or_else(|| anyhow!("prefix has no parent"))?;
    let staging = tempfile::Builder::new()
        .prefix(".wineo-restore-")
        .tempdir_in(parent)?;
    let staging_path = staging.path().to_owned();
    import::copy_tree_strict_reflink(
        &reviewed.snapshot.join("prefix"),
        &staging_path,
        &reviewed.prefix,
        cancel,
        &mut progress,
    )?;
    import::check_cancelled(cancel)?;
    let final_inspection = inspect_restore(paths, global, &reviewed.name, &reviewed.label)?;
    ensure_restore_same(reviewed, &final_inspection)?;
    import::validate_staged_prefix(&staging_path, &reviewed.prefix, reviewed.layout)?;
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
    let staging_path = staging.keep();
    if let Err(err) = write_marker(&marker_path, &marker) {
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
    let result = (|| -> Result<()> {
        exchange(&reviewed.prefix, &staging_path)?;
        sync_dir(parent)?;
        write_config_atomic(
            &paths.environment_config(&reviewed.name),
            &reviewed.saved_config,
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
    progress(ImportProgress::Finished {
        entries: reviewed.snapshot_fingerprint.entries,
    });
    Ok(RestoreOutcome {
        retained_old_prefix,
    })
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
    read_manifest(&snapshot, name, label, &prefix)?;
    remove_tree(&snapshot)?;
    Ok(())
}

pub fn recover(paths: &Paths, name: &str) -> Result<()> {
    if !name::is_valid(name) || !paths.environment_config(name).is_file() {
        bail!("unknown environment `{name}`");
    }
    let path = marker_path(paths, name);
    let marker = read_marker(&path, name)?;
    let prefix = marker.prefix.clone();
    validate_marker_config(paths, name, &marker)?;
    let locks = Locks::new(paths);
    let _operation = locks.acquire(Operation::Snapshot, name, &[&prefix])?;
    let _config_lock = locks.config(&paths.environment_config(name))?;
    Runtime::new(paths, name).check_quiet(name)?;
    let marker = read_marker(&path, name)?;
    validate_marker_config(paths, name, &marker)?;
    rollback_locked(paths, &path, &marker)
}

fn validate_marker_config(paths: &Paths, name: &str, marker: &RestoreMarker) -> Result<()> {
    let config = EnvironmentConfig::parse(&paths.environment_config(name), &marker.old_config)?;
    if paths::resolve(&paths.absolute(&config.prefix)?) != marker.prefix {
        bail!("restore marker config points to a different prefix");
    }
    Ok(())
}

fn registered(paths: &Paths, name: &str) -> Result<(Environment, String, PathBuf)> {
    let env = Environment::load(paths, name)?;
    let text = fs::read_to_string(&env.path)?;
    let prefix = paths::resolve(&env.prefix(paths)?);
    Ok((env, text, prefix))
}

fn inspect_prefix(
    paths: &Paths,
    global: &GlobalConfig,
    env: &Environment,
    prefix: &Path,
) -> Result<import::Inspection> {
    let runner = if import::root_layout_ambiguous(prefix) {
        Some(Runners::new(paths, global)?.resolve(&env.config.runner)?)
    } else {
        None
    };
    let inspected = import::inspect_root(prefix, runner.as_ref(), env.config.arch)?;
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
    let shared = environment::sharing_prefix(paths, name, prefix)?;
    if !shared.unreadable.is_empty() {
        bail!(
            "could not inspect environments: {}",
            shared.unreadable.join(", ")
        );
    }
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
        || a.current_fingerprint != b.current_fingerprint
        || a.snapshot_fingerprint != b.snapshot_fingerprint
    {
        bail!("reviewed restore changed; inspect it again");
    }
    Ok(())
}

fn read_manifest(path: &Path, name: &str, label: &str, prefix: &Path) -> Result<Manifest> {
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
        || manifest.prefix != prefix
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

fn read_marker(path: &Path, name: &str) -> Result<RestoreMarker> {
    ensure_regular(path)?;
    let marker: RestoreMarker = serde_json::from_slice(&fs::read(path)?)?;
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
    write_config_atomic(
        &paths.environment_config(&marker.environment),
        &marker.old_config,
    )?;
    fs::remove_file(marker_path)?;
    sync_dir(marker_path.parent().unwrap())?;
    remove_tree(&marker.staging)?;
    Ok(())
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

    #[test]
    fn restoring_a_legacy_config_keeps_it_unversioned_until_edited() {
        let (_root, paths, _prefix, config) = fixture(false);
        let global = GlobalConfig::default();
        let review = inspect_create(&paths, &global, "game", "legacy").unwrap();
        create_reviewed(&paths, &global, &review, &AtomicBool::new(false), |_| {}).unwrap();
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
        restore_reviewed(&paths, &global, &restore, &AtomicBool::new(false), |_| {}).unwrap();
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
        let info =
            create_reviewed(&paths, &global, &review, &AtomicBool::new(false), |_| {}).unwrap();
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
            let info =
                create_reviewed(&paths, &global, &review, &AtomicBool::new(false), |_| {}).unwrap();
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
            restore_reviewed(&paths, &global, &restore, &AtomicBool::new(false), |_| {}).unwrap();
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
        assert!(
            create_reviewed(&paths, &global, &review, &AtomicBool::new(false), |_| {}).is_err()
        );
        assert!(!review.snapshot.exists());
        let review = inspect_create(&paths, &global, "game", "one").unwrap();
        create_reviewed(&paths, &global, &review, &AtomicBool::new(false), |_| {}).unwrap();
        assert!(inspect_create(&paths, &global, "game", "one").is_err());
        fs::write(paths.environment_config("other"), config).unwrap();
        assert!(inspect_create(&paths, &global, "game", "two").is_err());
        assert!(inspect_restore(&paths, &global, "game", "one").is_err());
    }

    #[test]
    fn recovery_restores_original_after_interrupted_exchange() {
        let (_root, paths, prefix, original_config) = fixture(false);
        let global = GlobalConfig::default();
        let review = inspect_create(&paths, &global, "game", "one").unwrap();
        create_reviewed(&paths, &global, &review, &AtomicBool::new(false), |_| {}).unwrap();
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
            &AtomicBool::new(false),
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
            &AtomicBool::new(false),
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
        create_reviewed(&paths, &global, &create, &AtomicBool::new(false), |_| {}).unwrap();
        fs::write(prefix.join("drive_c/game.exe"), "later").unwrap();
        let restore = inspect_restore(&paths, &global, "game", "one").unwrap();
        let mut changed = false;
        let result = restore_reviewed(&paths, &global, &restore, &AtomicBool::new(false), |_| {
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
}
