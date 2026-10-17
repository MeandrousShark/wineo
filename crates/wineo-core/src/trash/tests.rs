use crate::test_script::write_script;
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::process::Command;

use super::digest::{copy_verified, digest, digest_matches, digest_v1, same_tree};
use super::identity::{create_storage, rename_owned, subvolume_property};
use super::markers::{marker_dir, publish_marker};
use super::payload::{make_payload, reserve_copy, return_payload};
use super::restore::{
    finish_entry, keep_changed, restore_locked_with, with_environment_build_locks,
};
use super::*;
use crate::build_lock;
use crate::config::{EnvironmentConfig, GlobalConfig};
use crate::environment::Environment;

struct Fixture {
    _root: tempfile::TempDir,
    paths: Paths,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        Self { _root: root, paths }
    }

    fn config(&self, name: &str, text: &str) {
        let path = self.paths.environment_config(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn wine(&self, name: &str) -> PathBuf {
        let prefix = self.paths.default_prefix_root().join(name);
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        self.config(
            name,
            &format!(
                "# keep me\nprefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
                prefix.display()
            ),
        );
        prefix
    }
}

fn drift_identity(saved: &mut Identity) -> bool {
    let (Some(mount), Some(_)) = (saved.mount, saved.birth) else {
        return false;
    };
    saved.mount = Some(mount + 10_000);
    true
}

fn drift_manifest(manifest: &mut Manifest) -> bool {
    let mut supported = drift_identity(&mut manifest.entry_identity);
    if let Some(identity) = &mut manifest.prefix_identity {
        supported &= drift_identity(identity);
    }
    for payload in &mut manifest.payloads {
        supported &= drift_identity(&mut payload.identity);
        supported &= drift_identity(&mut payload.source_parent);
        supported &= drift_identity(&mut payload.destination_parent);
        for identity in [&mut payload.copied_identity, &mut payload.returned_identity]
            .into_iter()
            .flatten()
        {
            supported &= drift_identity(identity);
        }
        for stage in [&mut payload.out_stage, &mut payload.return_stage]
            .into_iter()
            .flatten()
        {
            supported &= drift_identity(&mut stage.identity);
        }
    }
    supported
}

#[test]
fn mount_id_drift_preserves_read_only_listing_and_snapshot_restore() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let snapshots = f.paths.snapshot_root(&prefix, "game");
    fs::create_dir_all(&snapshots).unwrap();
    fs::write(snapshots.join("saved"), "snapshot").unwrap();
    let entry = remove(&f.paths, "game", true).unwrap();
    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    if !drift_manifest(&mut manifest) {
        return;
    }
    write_manifest(&f.paths, &manifest).unwrap();
    let file = entry_dir(&f.paths, &entry.id).unwrap().join(MANIFEST);
    let before = fs::read(&file).unwrap();

    assert_eq!(list(&f.paths).unwrap().len(), 1);
    assert_eq!(fs::read(&file).unwrap(), before);
    restore(&f.paths, &entry.id).unwrap();
    assert!(prefix.join("system.reg").exists());
    assert_eq!(fs::read(snapshots.join("saved")).unwrap(), b"snapshot");
}

#[test]
fn mount_id_drift_preserves_prune_ownership_checks() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n");
    let entry = remove(&f.paths, "game", false).unwrap();
    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    if !drift_manifest(&mut manifest) {
        return;
    }
    write_manifest(&f.paths, &manifest).unwrap();

    let review = review_prune(&f.paths, Selector::Id(&entry.id)).unwrap();
    let outcomes = prune_reviewed(&f.paths, &review);
    assert!(outcomes[0].1.is_ok(), "{:?}", outcomes[0].1);
    assert!(list(&f.paths).unwrap().is_empty());
}

#[test]
fn mount_id_drift_without_birth_time_fails_closed() {
    let saved = Identity {
        device: 5,
        inode: 9,
        mount: Some(1),
        birth: None,
    };
    assert!(!saved.matches(Identity {
        mount: Some(2),
        ..saved
    }));
    let with_birth = Identity {
        birth: Some((1, 0)),
        ..saved
    };
    assert!(with_birth.matches(Identity {
        mount: Some(2),
        ..with_birth
    }));
    assert!(!with_birth.matches(Identity {
        inode: 10,
        mount: Some(2),
        ..with_birth
    }));
    assert!(!with_birth.matches(Identity {
        birth: Some((2, 0)),
        mount: Some(2),
        ..with_birth
    }));
}

#[test]
fn mount_id_drift_keeps_relocated_portable_marker_recoverable() {
    let root = tempfile::tempdir().unwrap();
    let from = root.path().join("from");
    let to = root.path().join("to");
    let home = root.path().to_owned();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&from),
    )
    .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(paths.environment_config("game"), "kind = 'native'\n").unwrap();
    let entry = remove(&paths, "game", false).unwrap();
    let mut manifest = read_manifest(&paths, &entry.id).unwrap();
    start_marker(&paths, &manifest, Intent::Restore).unwrap();
    if !drift_manifest(&mut manifest) {
        return;
    }
    write_manifest(&paths, &manifest).unwrap();
    let (path, mut marker) = marker_for(&paths, &entry.id).unwrap().unwrap();
    if !drift_identity(marker.portable_identity.as_mut().unwrap())
        || !drift_identity(&mut marker.metadata_identity)
        || !drift_manifest(&mut marker.manifest)
    {
        return;
    }
    publish_marker(&path, &marker).unwrap();
    fs::rename(&from, &to).unwrap();
    let home = root.path().to_owned();
    let moved = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&to),
    )
    .unwrap();

    assert_eq!(pending_entries(&moved).unwrap().len(), 1);
    recover(&moved, &entry.id).unwrap();
    assert!(pending_entries(&moved).unwrap().is_empty());
    restore(&moved, &entry.id).unwrap();
    assert_eq!(
        fs::read(moved.environment_config("game")).unwrap(),
        b"kind = 'native'\n"
    );
}

#[test]
fn mount_id_drift_recovers_a_recorded_copy_stage() {
    let Some(config_root) = tempfile::tempdir_in("/dev/shm").ok() else {
        return;
    };
    if fs::metadata(config_root.path()).unwrap().dev()
        == fs::metadata(env!("CARGO_MANIFEST_DIR")).unwrap().dev()
    {
        return;
    }
    let f = Fixture::new();
    let home = f._root.path().to_owned();
    let config = config_root.path().to_owned();
    let paths = Paths::from_lookup(move |key| match key {
        "HOME" => Some(OsString::from(home.as_os_str())),
        "XDG_CONFIG_HOME" => Some(OsString::from(config.as_os_str())),
        _ => None,
    })
    .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    let source = paths.environment_config("game");
    fs::write(&source, "kind = 'native'\n").unwrap();
    set_trash_failpoint(Some("out.after_stage_write"));
    assert!(remove(&paths, "game", false).is_err());
    let (path, mut marker) = markers(&paths).unwrap().pop().unwrap();
    let id = marker.manifest.id.clone();
    let mut manifest = read_manifest(&paths, &id).unwrap();
    assert!(manifest.payloads[0].out_stage.is_some());
    if !drift_manifest(&mut manifest)
        || !drift_identity(&mut marker.metadata_identity)
        || !drift_manifest(&mut marker.manifest)
    {
        return;
    }
    write_manifest(&paths, &manifest).unwrap();
    publish_marker(&path, &marker).unwrap();

    recover(&paths, &id).unwrap();
    assert_eq!(fs::read(source).unwrap(), b"kind = 'native'\n");
    assert!(pending_entries(&paths).unwrap().is_empty());
}

#[test]
fn native_restore_with_malformed_global_config_completes() {
    let f = Fixture::new();
    let text = "kind = 'native'\n";
    f.config("game", text);
    let entry = remove(&f.paths, "game", false).unwrap();
    fs::write(f.paths.global_config(), "invalid = [TOML").unwrap();

    restore(&f.paths, &entry.id).unwrap();

    assert_eq!(
        fs::read_to_string(f.paths.environment_config("game")).unwrap(),
        text
    );
    assert!(list(&f.paths).unwrap().is_empty());
    assert!(pending_entries(&f.paths).unwrap().is_empty());
    assert!(
        fs::read_dir(f.paths.build_locks_dir())
            .unwrap()
            .all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("build-")
            })
    );
}

#[test]
fn wine_restore_with_malformed_global_config_completes() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let text = format!(
        "prefix = '{}'\nrunner = 'build'\narch = 'win64'\ndxvk = '2.0'\nvkd3d = '3.0'\n",
        prefix.display()
    );
    f.config("game", &text);
    let entry = remove(&f.paths, "game", true).unwrap();
    fs::write(f.paths.global_config(), "invalid = [TOML").unwrap();

    locked(&f.paths, &entry.id, true, |manifest, gog| {
        restore_locked_with(&f.paths, manifest, false, gog, || {
            for (kind, name) in [
                (build_lock::Kind::Runner, "build"),
                (build_lock::Kind::Dxvk, "2.0"),
                (build_lock::Kind::Vkd3d, "3.0"),
            ] {
                assert!(build_lock::try_exclusive(&f.paths, kind, name)?.is_some());
            }
            Ok(())
        })
    })
    .unwrap();

    assert_eq!(
        fs::read_to_string(f.paths.environment_config("game")).unwrap(),
        text
    );
    assert!(prefix.join("system.reg").exists());
    assert!(list(&f.paths).unwrap().is_empty());
    assert!(pending_entries(&f.paths).unwrap().is_empty());
}

#[test]
fn restore_publishes_config_while_all_named_builds_are_locked() {
    let f = Fixture::new();
    let path = f.paths.environment_config("game");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text =
        "prefix = '/tmp/prefix'\nrunner = 'build'\narch = 'win64'\ndxvk = '2.0'\nvkd3d = '3.0'\n";
    let config = EnvironmentConfig::parse(&path, text).unwrap();
    let global = GlobalConfig::default();

    with_environment_build_locks(&f.paths, Some(&global), Some(&config), || {
        for (kind, name) in [
            (build_lock::Kind::Runner, "build"),
            (build_lock::Kind::Dxvk, "2.0"),
            (build_lock::Kind::Vkd3d, "3.0"),
        ] {
            assert!(build_lock::try_exclusive(&f.paths, kind, name)?.is_none());
        }
        fs::write(&path, text)?;
        Ok(())
    })
    .unwrap();

    for (kind, name) in [
        (build_lock::Kind::Runner, "build"),
        (build_lock::Kind::Dxvk, "2.0"),
        (build_lock::Kind::Vkd3d, "3.0"),
    ] {
        assert!(
            build_lock::try_exclusive(&f.paths, kind, name)
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn restore_holds_build_locks_through_payload_publication() {
    use std::sync::mpsc;
    use std::time::Duration;

    let f = Fixture::new();
    let prefix = f.wine("game");
    let text = format!(
        "prefix = '{}'\nrunner = 'build'\narch = 'win64'\ndxvk = '2.0'\nvkd3d = '3.0'\n",
        prefix.display()
    );
    f.config("game", &text);
    let bin = f.paths.default_runner_root().join("build/bin");
    fs::create_dir_all(&bin).unwrap();
    for program in ["wine", "wineserver"] {
        std::os::unix::fs::symlink("/bin/true", bin.join(program)).unwrap();
    }
    let entry = remove(&f.paths, "game", true).unwrap();
    assert!(!prefix.exists());
    assert!(!f.paths.environment_config("game").exists());
    let (published_tx, published_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let paths = f.paths.clone();
    let id = entry.id.clone();
    let worker = std::thread::spawn(move || {
        locked(&paths, &id, true, |manifest, gog| {
            restore_locked_with(&paths, manifest, false, gog, || {
                published_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                Ok(())
            })
        })
    });
    published_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(
        fs::read_to_string(f.paths.environment_config("game")).unwrap(),
        text
    );
    assert!(prefix.join("system.reg").exists());
    assert!(entry_dir(&f.paths, &entry.id).unwrap().exists());
    for (kind, name) in [
        (build_lock::Kind::Runner, "build"),
        (build_lock::Kind::Dxvk, "2.0"),
        (build_lock::Kind::Vkd3d, "3.0"),
    ] {
        assert!(
            build_lock::try_exclusive(&f.paths, kind, name)
                .unwrap()
                .is_none()
        );
    }
    release_tx.send(()).unwrap();
    worker.join().unwrap().unwrap();
    assert!(list(&f.paths).unwrap().is_empty());
    assert!(pending_entries(&f.paths).unwrap().is_empty());
    for (kind, name) in [
        (build_lock::Kind::Runner, "build"),
        (build_lock::Kind::Dxvk, "2.0"),
        (build_lock::Kind::Vkd3d, "3.0"),
    ] {
        assert!(
            build_lock::try_exclusive(&f.paths, kind, name)
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn canceling_removal_during_the_wine_wait_leaves_everything_untouched() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    f.config(
        "game",
        &format!(
            "prefix = '{}'\nrunner = 'fake'\narch = 'win64'\n",
            prefix.display()
        ),
    );
    let bin = f.paths.default_runner_root().join("fake/bin");
    fs::create_dir_all(&bin).unwrap();
    for (program, script) in [
        ("wine", "#!/bin/sh\nexit 0\n"),
        (
            "wineserver",
            "#!/bin/sh\nif [ \"$1\" = -k ]; then\n  touch \"$WINEPREFIX/killed\"\nelse\n  touch \"$WINEPREFIX/waiting\"\n  exec sleep 30\nfi\n",
        ),
    ] {
        let path = bin.join(program);
        write_script(&path, script);
    }
    let global = f.paths.global_config();
    let files = [
        (global, "# global settings\n"),
        (f.paths.art_dir("game").join("art.toml"), "# art settings\n"),
        (f.paths.art_dir("game").join("icon.png"), "icon"),
        (f.paths.stats_log("game"), "playtime\n"),
        (prefix.join("drive_c/game.exe"), "game"),
        (
            f.paths.snapshot_root(&prefix, "game").join("snapshot"),
            "snapshot",
        ),
        (
            f.paths.applications_dir().join("wineo-game-game.desktop"),
            "[Desktop Entry]\nX-Wineo-Environment=game\nX-Wineo-Target=game\n",
        ),
        (
            f.paths
                .icons_dir()
                .join("hicolor/256x256/apps/wineo-game-game.png"),
            "desktop icon",
        ),
    ];
    for (path, text) in &files {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let before: Vec<_> = files
        .iter()
        .map(|(path, _)| path.clone())
        .chain([
            f.paths.environment_config("game"),
            prefix.join("system.reg"),
        ])
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            let identity = identity(&path).unwrap();
            (path, bytes, identity)
        })
        .collect();
    let token = crate::cancel::Token::new();
    let mut events = Vec::new();
    let result = remove_with(&f.paths, "game", true, &token, |event| {
        events.push(event);
        assert!(prefix.join("waiting").exists());
        token.cancel();
    });
    assert_eq!(events, [RemoveEvent::Waiting]);
    assert!(matches!(
        result.unwrap_err().downcast_ref::<crate::error::Error>(),
        Some(crate::error::Error::Canceled(_))
    ));
    for (path, bytes, original) in before {
        assert_eq!(fs::read(&path).unwrap(), bytes, "{}", path.display());
        assert_eq!(identity(&path).unwrap(), original, "{}", path.display());
    }
    assert!(prefix.join("drive_c").is_dir());
    assert!(!prefix.join("killed").exists());
    assert!(!f.paths.trash_dir().exists());
    assert!(!prefix.parent().unwrap().join(".wineo-trash").exists());
    assert!(pending_entries(&f.paths).unwrap().is_empty());
}

#[test]
fn a_precanceled_remove_does_not_create_trash_storage() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n");
    let token = crate::cancel::Token::new();
    token.cancel();
    assert!(remove_with(&f.paths, "game", false, &token, |event| panic!("{event:?}")).is_err());
    assert_eq!(
        fs::read_to_string(f.paths.environment_config("game")).unwrap(),
        "kind = 'native'\n"
    );
    assert!(!f.paths.trash_dir().exists());
}

#[test]
fn a_remove_reports_moving_once_before_it_writes() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n");
    let mut events = Vec::new();
    remove_with(
        &f.paths,
        "game",
        false,
        &crate::cancel::Token::new(),
        |event| {
            assert!(!f.paths.trash_dir().exists());
            events.push(event);
        },
    )
    .unwrap();
    assert_eq!(events, [RemoveEvent::Moving]);
}

#[test]
fn trash_ids_require_an_eight_character_hex_suffix() {
    assert!(id_valid("20260926T183000Z-a1b2c3d4"));
    assert!(!id_valid("20260926T183000Z-a1b2c3d4e5f60708"));
    assert!(!id_valid("20260926T183000Z-a1b2c3dg"));
}

#[test]
fn marker_blocks_absent_metadata_and_is_retired_only_after_recovery() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n");
    let entry = remove(&f.paths, "game", false).unwrap();
    let manifest = read_manifest(&f.paths, &entry.id).unwrap();
    assert!(pending_entries(&f.paths).unwrap().is_empty());
    start_marker(&f.paths, &manifest, Intent::Restore).unwrap();
    let hidden = f._root.path().join("metadata-offline");
    fs::rename(f.paths.trash_dir(), &hidden).unwrap();
    assert!(
        check_pending(&f.paths, "game", &[])
            .unwrap_err()
            .to_string()
            .contains(&entry.id)
    );
    assert!(check_pending(&f.paths, "other", &[]).is_ok());
    assert!(list(&f.paths).is_err());
    let report = crate::validate::check_all(&f.paths).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.code == crate::validate::Code::TrashInterrupted)
    );
    assert!(recover(&f.paths, &entry.id).is_err());
    assert_eq!(pending_entries(&f.paths).unwrap().len(), 1);
    fs::rename(hidden, f.paths.trash_dir()).unwrap();
    recover(&f.paths, &entry.id).unwrap();
    assert!(pending_entries(&f.paths).unwrap().is_empty());
    restore(&f.paths, &entry.id).unwrap();
    assert!(f.paths.environment_config("game").exists());
}

fn assert_scan_matches(paths: &Paths, affected: &[&Path]) {
    let scan = PendingScan::new(paths);
    for name in ["game", "other", "game", "other"] {
        assert_eq!(
            scan.check(paths, name, affected)
                .map_err(|err| err.to_string()),
            check_pending(paths, name, affected).map_err(|err| err.to_string()),
            "{name}"
        );
    }
}

#[test]
fn a_shared_scan_answers_like_check_pending() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let elsewhere = f.paths.default_prefix_root().join("elsewhere");
    assert_scan_matches(&f.paths, &[]);
    let entry = remove(&f.paths, "game", true).unwrap();
    let retained = prefix
        .parent()
        .unwrap()
        .join(".wineo-trash")
        .join(&entry.id)
        .join("prefix");
    f.config("other", "kind = 'native'\n");
    for affected in [&[][..], &[retained.as_path()], &[elsewhere.as_path()]] {
        assert_scan_matches(&f.paths, affected);
    }
    assert!(check_pending(&f.paths, "other", &[&retained]).is_err());

    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    manifest.phase = Phase::Removing;
    write_manifest(&f.paths, &manifest).unwrap();
    for affected in [&[][..], &[prefix.as_path()], &[elsewhere.as_path()]] {
        assert_scan_matches(&f.paths, affected);
    }
    assert!(check_pending(&f.paths, "game", &[]).is_err());
    assert!(check_pending(&f.paths, "other", &[]).is_ok());

    start_marker(&f.paths, &manifest, Intent::Remove).unwrap();
    for affected in [&[][..], &[prefix.as_path()], &[elsewhere.as_path()]] {
        assert_scan_matches(&f.paths, affected);
    }

    let manifest_path = entry_dir(&f.paths, &entry.id).unwrap().join(MANIFEST);
    let saved = fs::read(&manifest_path).unwrap();
    fs::write(&manifest_path, "not toml").unwrap();
    // The marker is checked first, so it still names the entry for its owner.
    assert_scan_matches(&f.paths, &[]);
    for (path, _) in markers(&f.paths).unwrap() {
        fs::remove_file(path).unwrap();
    }
    assert_scan_matches(&f.paths, &[]);
    assert!(check_pending(&f.paths, "other", &[]).is_err());

    fs::write(markers_dir_file(&f.paths), "not toml").unwrap();
    assert_scan_matches(&f.paths, &[]);
    assert!(check_pending(&f.paths, "other", &[]).is_err());
    fs::remove_file(markers_dir_file(&f.paths)).unwrap();
    fs::write(&manifest_path, saved).unwrap();
    assert_scan_matches(&f.paths, &[]);
}

fn expect_scan(paths: &Paths, expected: &[(&str, Result<(), String>)]) {
    let scan = PendingScan::new(paths);
    for _ in 0..2 {
        for (name, want) in expected {
            let got = scan.check(paths, name, &[]).map_err(|err| err.to_string());
            assert_eq!(&got, want, "shared scan, {name}");
            let single = check_pending(paths, name, &[]).map_err(|err| err.to_string());
            assert_eq!(&single, want, "check_pending, {name}");
        }
    }
}

#[test]
fn a_shared_scan_reports_restore_and_prune_work_to_its_owner_only() {
    for (phase, intent) in [
        (Phase::Restoring, Intent::Restore),
        (Phase::Pruning, Intent::Prune),
    ] {
        let f = Fixture::new();
        f.config("game", "kind = 'native'\n");
        let entry = remove(&f.paths, "game", false).unwrap();
        let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
        manifest.phase = phase;
        write_manifest(&f.paths, &manifest).unwrap();
        let interrupted = format!(
            "trash entry {} has interrupted {phase:?} for game; recover with `wineo validate --fix`",
            entry.id
        );
        expect_scan(
            &f.paths,
            &[
                ("game", Err(interrupted.clone())),
                ("other", Ok(())),
                ("game", Err(interrupted.clone())),
            ],
        );

        start_marker(&f.paths, &manifest, intent).unwrap();
        let pending = format!(
            "trash entry {} has pending {intent:?} for game; recover with `wineo validate --fix`",
            entry.id
        );
        expect_scan(
            &f.paths,
            &[("other", Ok(())), ("game", Err(pending)), ("other", Ok(()))],
        );
    }
}

#[test]
fn a_shared_scan_keeps_an_early_blocker_ahead_of_a_later_corrupt_manifest() {
    let f = Fixture::new();
    f.config("alpha", "kind = 'native'\n");
    f.config("beta", "kind = 'native'\n");
    let mut ids = [
        remove(&f.paths, "alpha", false).unwrap(),
        remove(&f.paths, "beta", false).unwrap(),
    ];
    ids.sort_by(|a, b| a.id.cmp(&b.id));
    let [first, second] = ids;
    let mut manifest = read_manifest(&f.paths, &first.id).unwrap();
    manifest.phase = Phase::Removing;
    write_manifest(&f.paths, &manifest).unwrap();
    let manifest_path = entry_dir(&f.paths, &second.id).unwrap().join(MANIFEST);
    fs::write(&manifest_path, "not toml").unwrap();
    let corrupt = read_manifest(&f.paths, &second.id).unwrap_err().to_string();
    let blocked = format!(
        "trash entry {} has interrupted Removing for {}; recover with `wineo validate --fix`",
        first.id, first.name
    );
    expect_scan(
        &f.paths,
        &[
            (first.name.as_str(), Err(blocked.clone())),
            (second.name.as_str(), Err(corrupt.clone())),
            ("gamma", Err(corrupt.clone())),
            (first.name.as_str(), Err(blocked)),
            (second.name.as_str(), Err(corrupt)),
        ],
    );
}

fn markers_dir_file(paths: &Paths) -> PathBuf {
    let dir = marker_dir(paths);
    fs::create_dir_all(&dir).unwrap();
    dir.join("trash-broken.toml")
}

#[test]
fn missing_metadata_marker_blocks_affected_prefix_of_another_name() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let entry = remove(&f.paths, "game", true).unwrap();
    let manifest = read_manifest(&f.paths, &entry.id).unwrap();
    start_marker(&f.paths, &manifest, Intent::Restore).unwrap();
    let hidden = f._root.path().join("metadata-offline");
    fs::rename(f.paths.trash_dir(), &hidden).unwrap();
    assert!(
        check_pending(&f.paths, "other", &[&prefix])
            .unwrap_err()
            .to_string()
            .contains(&entry.id)
    );
    fs::rename(hidden, f.paths.trash_dir()).unwrap();
    recover(&f.paths, &entry.id).unwrap();
    restore(&f.paths, &entry.id).unwrap();
}

#[test]
fn finished_marker_survives_manifest_unlink_until_retired() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n");
    let entry = remove(&f.paths, "game", false).unwrap();
    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    start_marker(&f.paths, &manifest, Intent::Restore).unwrap();
    manifest.phase = Phase::Restoring;
    manifest.payloads[0].step = Step::Returned;
    write_manifest(&f.paths, &manifest).unwrap();
    rename_owned(
        &entry_dir(&f.paths, &entry.id).unwrap().join("config.toml"),
        &f.paths.environment_config("game"),
    )
    .unwrap();
    finish_marker(&f.paths, &manifest).unwrap();
    finish_entry(&f.paths, &manifest).unwrap();
    assert!(check_pending(&f.paths, "game", &[]).is_err());
    recover(&f.paths, &entry.id).unwrap();
    assert!(pending_entries(&f.paths).unwrap().is_empty());
    assert_eq!(
        fs::read(f.paths.environment_config("game")).unwrap(),
        b"kind = 'native'\n"
    );
}

#[test]
fn prune_recovers_after_container_cleanup_before_manifest_unlink() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let entry = remove(&f.paths, "game", true).unwrap();
    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    start_marker(&f.paths, &manifest, Intent::Prune).unwrap();
    manifest.phase = Phase::Pruning;
    for payload in &mut manifest.payloads {
        delete_owned(&resolved(&f.paths, &payload.retained).unwrap()).unwrap();
        payload.step = Step::Deleted;
    }
    write_manifest(&f.paths, &manifest).unwrap();
    let container = prefix
        .parent()
        .unwrap()
        .join(".wineo-trash")
        .join(&entry.id);
    fs::remove_dir(container).unwrap();
    finish_marker(&f.paths, &manifest).unwrap();
    recover(&f.paths, &entry.id).unwrap();
    assert!(pending_entries(&f.paths).unwrap().is_empty());
    assert!(list(&f.paths).unwrap().is_empty());
}

#[test]
fn marker_follows_a_same_filesystem_portable_rename() {
    let root = tempfile::tempdir().unwrap();
    let from = root.path().join("from");
    let to = root.path().join("to");
    let home = root.path().to_owned();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&from),
    )
    .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(paths.environment_config("game"), "kind = 'native'\n").unwrap();
    let entry = remove(&paths, "game", false).unwrap();
    let manifest = read_manifest(&paths, &entry.id).unwrap();
    start_marker(&paths, &manifest, Intent::Restore).unwrap();
    fs::rename(from, &to).unwrap();
    let home = root.path().to_owned();
    let moved = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&to),
    )
    .unwrap();
    assert!(check_pending(&moved, "game", &[]).is_err());
    recover(&moved, &entry.id).unwrap();
    assert!(pending_entries(&moved).unwrap().is_empty());
    restore(&moved, &entry.id).unwrap();
}

#[test]
fn moved_portable_marker_blocks_other_roots_touching_its_prefix() {
    let root = tempfile::tempdir().unwrap();
    let from = root.path().join("from");
    let to = root.path().join("to");
    let home = root.path().to_owned();
    let portable = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&from),
    )
    .unwrap();
    let prefix = portable.default_prefix_root().join("game");
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    fs::create_dir_all(portable.environments_dir()).unwrap();
    fs::write(
        portable.environment_config("game"),
        "prefix = 'prefixes/game'\nrunner = 'wine'\narch = 'win64'\n",
    )
    .unwrap();
    let entry = remove(&portable, "game", true).unwrap();
    let manifest = read_manifest(&portable, &entry.id).unwrap();
    start_marker(&portable, &manifest, Intent::Restore).unwrap();
    fs::rename(&from, &to).unwrap();
    let home = root.path().to_owned();
    let host =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    let moved_prefix = to.join("prefixes/game");
    assert!(check_pending(&host, "other", &[&moved_prefix]).is_err());
    let moved_retained = to
        .join("prefixes/.wineo-trash")
        .join(&entry.id)
        .join("prefix");
    assert!(check_pending(&host, "other", &[&moved_retained]).is_err());
    assert!(check_pending(&host, "other", &[&to.join("prefixes/unrelated")]).is_ok());
    assert!(
        Locks::new(&host)
            .acquire(Operation::Launch, "other", &[&moved_prefix])
            .is_err()
    );
    let home = root.path().to_owned();
    let moved = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&to),
    )
    .unwrap();
    recover(&moved, &entry.id).unwrap();
    assert!(pending_entries(&moved).unwrap().is_empty());
}

#[test]
fn marker_created_before_manifest_publication_recovers_without_guessing() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n");
    create_storage(&f.paths.trash_dir()).unwrap();
    let id = "20260926T183000Z-a1b2c3d4";
    let dir = entry_dir(&f.paths, id).unwrap();
    fs::create_dir(&dir).unwrap();
    sync(&f.paths.trash_dir()).unwrap();
    let source = f.paths.environment_config("game");
    let payload = make_payload(&f.paths, Kind::Config, &source, &dir.join("config.toml"))
        .unwrap()
        .unwrap();
    let manifest = Manifest {
        version: VERSION,
        id: id.into(),
        entry_identity: identity(&dir).unwrap().unwrap(),
        name: "game".into(),
        removed_at: "2026-09-26T18:30:00Z".into(),
        include_prefix: false,
        prefix_present: None,
        prefix_reference: None,
        prefix_identity: None,
        phase: Phase::Removing,
        payloads: vec![payload],
        gog_folders: Vec::new(),
    };
    start_marker(&f.paths, &manifest, Intent::Remove).unwrap();
    assert!(check_pending(&f.paths, "game", &[]).is_err());
    fs::rename(&source, f._root.path().join("missing-config")).unwrap();
    assert!(recover(&f.paths, id).is_err());
    assert_eq!(pending_entries(&f.paths).unwrap().len(), 1);
    fs::rename(f._root.path().join("missing-config"), &source).unwrap();
    recover(&f.paths, id).unwrap();
    assert!(pending_entries(&f.paths).unwrap().is_empty());
    assert!(!dir.exists());
    assert_eq!(fs::read(source).unwrap(), b"kind = 'native'\n");
}

#[test]
fn marker_publication_and_retirement_boundaries_do_not_hide_pending_work() {
    for point in [
        "marker.before_publish",
        "marker.after_publish",
        "marker.before_finish",
        "marker.after_finish",
        "marker.before_retire",
        "marker.after_retire",
    ] {
        let f = Fixture::new();
        f.config("game", "kind = 'native'\n");
        set_trash_failpoint(Some(point));
        assert!(
            remove(&f.paths, "game", false)
                .unwrap_err()
                .to_string()
                .contains("injected"),
            "{point}"
        );
        let pending = pending_entries(&f.paths).unwrap();
        if point == "marker.before_publish" || point == "marker.after_retire" {
            assert!(pending.is_empty(), "{point}");
        } else {
            assert_eq!(pending.len(), 1, "{point}");
            assert!(check_pending(&f.paths, "game", &[]).is_err());
            recover(&f.paths, &pending[0].id).unwrap();
            assert!(pending_entries(&f.paths).unwrap().is_empty());
        }
        if point == "marker.before_publish" || point == "marker.after_publish" {
            assert_eq!(
                fs::read(f.paths.environment_config("game")).unwrap(),
                b"kind = 'native'\n"
            );
        } else {
            assert_eq!(list(&f.paths).unwrap().len(), 1, "{point}");
        }
    }
}

#[test]
fn metadata_copy_reservation_boundaries_recover_in_both_directions() {
    let Some(probe) = tempfile::tempdir_in("/dev/shm").ok() else {
        return;
    };
    if fs::metadata(probe.path()).unwrap().dev()
        == fs::metadata(env!("CARGO_MANIFEST_DIR")).unwrap().dev()
    {
        return;
    }
    for point in [
        "out.before_reservation",
        "out.after_reservation",
        "out.before_stage_write",
        "out.after_stage_write",
        "out.before_identity_write",
        "out.after_identity_write",
        "out.after_publish",
        "return.before_reservation",
        "return.after_reservation",
        "return.before_stage_write",
        "return.after_stage_write",
        "return.before_identity_write",
        "return.after_identity_write",
        "return.after_publish",
    ] {
        let f = Fixture::new();
        let config_root = tempfile::tempdir_in("/dev/shm").unwrap();
        let home = f._root.path().to_owned();
        let config = config_root.path().to_owned();
        let paths = Paths::from_lookup(move |key| match key {
            "HOME" => Some(OsString::from(home.as_os_str())),
            "XDG_CONFIG_HOME" => Some(OsString::from(config.as_os_str())),
            _ => None,
        })
        .unwrap();
        fs::create_dir_all(paths.environments_dir()).unwrap();
        let source = paths.environment_config("game");
        let raw = b"kind = 'native'\n# original\n";
        fs::write(&source, raw).unwrap();
        if point.starts_with("return.") {
            let entry = remove(&paths, "game", false).unwrap();
            set_trash_failpoint(Some(point));
            assert!(
                restore(&paths, &entry.id)
                    .unwrap_err()
                    .to_string()
                    .contains("injected"),
                "{point}"
            );
            assert_eq!(pending_entries(&paths).unwrap().len(), 1, "{point}");
            recover(&paths, &entry.id).unwrap();
            assert!(list(&paths).unwrap().is_empty(), "{point}");
        } else {
            set_trash_failpoint(Some(point));
            assert!(
                remove(&paths, "game", false)
                    .unwrap_err()
                    .to_string()
                    .contains("injected"),
                "{point}"
            );
            let pending = pending_entries(&paths).unwrap();
            assert_eq!(pending.len(), 1, "{point}");
            recover(&paths, &pending[0].id).unwrap();
        }
        assert_eq!(fs::read(&source).unwrap(), raw, "{point}");
        assert!(pending_entries(&paths).unwrap().is_empty(), "{point}");
    }
}

#[test]
fn plain_removal_preserves_raw_metadata_and_retained_prefix_identity() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let art = f.paths.art_dir("game");
    fs::create_dir_all(&art).unwrap();
    fs::write(art.join("image.bin"), [0, 255, 1]).unwrap();
    fs::create_dir_all(parent(&f.paths.stats_log("game")).unwrap()).unwrap();
    fs::write(f.paths.stats_log("game"), "session\n").unwrap();
    let raw = fs::read(f.paths.environment_config("game")).unwrap();
    let first = remove(&f.paths, "game", false).unwrap();
    assert_eq!(first.phase, Phase::Trashed);
    assert_eq!(list(&f.paths).unwrap().len(), 1);
    restore(&f.paths, &first.id).unwrap();
    assert_eq!(fs::read(f.paths.environment_config("game")).unwrap(), raw);
    assert_eq!(fs::read(art.join("image.bin")).unwrap(), [0, 255, 1]);
    assert_eq!(fs::read(f.paths.stats_log("game")).unwrap(), b"session\n");
    assert!(list(&f.paths).unwrap().is_empty());

    let second = remove(&f.paths, "game", false).unwrap();
    fs::remove_dir_all(&prefix).unwrap();
    f.wine("unrelated");
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(prefix.join("system.reg"), "replacement").unwrap();
    let err = restore(&f.paths, &second.id).unwrap_err();
    assert!(err.to_string().contains("identity"), "{err:#}");
    assert_eq!(inspect(&f.paths, &second.id).unwrap().phase, Phase::Trashed);
}

#[test]
fn inclusive_removal_and_prune_touch_only_owned_payloads() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let snapshots = f.paths.snapshot_root(&prefix, "game");
    fs::create_dir_all(snapshots.join("before")).unwrap();
    fs::write(snapshots.join("before/saved"), "save").unwrap();
    let unrelated = prefix.parent().unwrap().join("unrelated");
    fs::write(&unrelated, "keep").unwrap();
    let entry = remove(&f.paths, "game", true).unwrap();
    assert!(!prefix.exists());
    assert!(!snapshots.exists());
    let reviewed = review_prune(&f.paths, Selector::Id(&entry.id)).unwrap();
    let outcomes = prune_reviewed(&f.paths, &reviewed);
    assert!(outcomes[0].1.is_ok(), "{:?}", outcomes[0].1);
    assert_eq!(fs::read(unrelated).unwrap(), b"keep");
    assert!(list(&f.paths).unwrap().is_empty());
}

#[test]
fn proton_compatdata_root_round_trips_with_its_snapshots() {
    let f = Fixture::new();
    let prefix = f.paths.default_prefix_root().join("proton");
    fs::create_dir_all(prefix.join("pfx/drive_c")).unwrap();
    fs::write(prefix.join("pfx/system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    fs::write(prefix.join("version"), "compatdata").unwrap();
    let snapshots = f.paths.snapshot_root(&prefix, "proton");
    fs::create_dir_all(&snapshots).unwrap();
    fs::write(snapshots.join("saved"), "snapshot").unwrap();
    f.config(
        "proton",
        &format!(
            "prefix = '{}'\nrunner = 'GE-Proton'\narch = 'win64'\n",
            prefix.display()
        ),
    );
    let entry = remove(&f.paths, "proton", true).unwrap();
    assert!(!prefix.exists());
    restore(&f.paths, &entry.id).unwrap();
    assert_eq!(fs::read(prefix.join("version")).unwrap(), b"compatdata");
    assert!(prefix.join("pfx/system.reg").exists());
    assert_eq!(fs::read(snapshots.join("saved")).unwrap(), b"snapshot");
}

#[test]
fn a_prefix_on_a_separate_filesystem_is_moved_not_copied() {
    let Some(prefix_home) = tempfile::tempdir_in("/dev/shm").ok() else {
        return;
    };
    let f = Fixture::new();
    let prefix = prefix_home.path().join("game");
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    f.config(
        "game",
        &format!(
            "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
            prefix.display()
        ),
    );
    if fs::metadata(prefix_home.path()).unwrap().dev()
        == fs::metadata(f._root.path()).unwrap().dev()
    {
        return;
    }
    let before = identity(&prefix).unwrap().unwrap();
    let entry = remove(&f.paths, "game", true).unwrap();
    let retained = prefix_home
        .path()
        .join(format!(".wineo-trash/{}/prefix", entry.id));
    assert_eq!(identity(&retained).unwrap(), Some(before));
    restore(&f.paths, &entry.id).unwrap();
    assert_eq!(identity(&prefix).unwrap(), Some(before));
}

#[test]
fn btrfs_subvolume_prefix_and_nested_readonly_subvolume_round_trip_and_prune() {
    let Some(btrfs) = crate::runner::find_on_path("btrfs") else {
        return;
    };
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    if rustix::fs::statfs(root.path()).unwrap().f_type as u32 != 0x9123_683e {
        return;
    }
    let home = root.path().to_owned();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    let prefix = paths.default_prefix_root().join("game");
    fs::create_dir_all(parent(&prefix).unwrap()).unwrap();
    if !Command::new(&btrfs)
        .args(["subvolume", "create"])
        .arg(&prefix)
        .status()
        .unwrap()
        .success()
    {
        return;
    }
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if self.0.exists() {
                let _ = delete_owned(&self.0);
            }
        }
    }
    let _cleanup = Cleanup(prefix.clone());
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let nested = prefix.join("drive_c/nested");
    assert!(
        Command::new(&btrfs)
            .args(["subvolume", "create"])
            .arg(&nested)
            .status()
            .unwrap()
            .success()
    );
    fs::write(nested.join("save"), "important").unwrap();
    assert!(
        Command::new(&btrfs)
            .args(["property", "set", "-ts"])
            .arg(&nested)
            .args(["ro", "true"])
            .status()
            .unwrap()
            .success()
    );
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("game"),
        format!(
            "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
            prefix.display()
        ),
    )
    .unwrap();
    let first = remove(&paths, "game", true).unwrap();
    assert!(!prefix.exists());
    restore(&paths, &first.id).unwrap();
    assert_eq!(fs::read(nested.join("save")).unwrap(), b"important");
    assert_eq!(subvolume_property(&nested).unwrap(), Some(true));
    let second = remove(&paths, "game", true).unwrap();
    let selected = review_prune(&paths, Selector::Id(&second.id)).unwrap();
    assert!(prune_reviewed(&paths, &selected)[0].1.is_ok());
    assert!(!prefix.exists());
}

#[test]
fn btrfs_same_content_replacement_cannot_be_restored_or_pruned() {
    let Some(btrfs) = crate::runner::find_on_path("btrfs") else {
        return;
    };
    for prune in [false, true] {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        if rustix::fs::statfs(root.path()).unwrap().f_type as u32 != 0x9123_683e {
            return;
        }
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if self.0.exists() {
                    let _ = delete_owned(&self.0);
                }
            }
        }
        let _cleanup = Cleanup(root.path().to_owned());
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        let prefix = paths.default_prefix_root().join("game");
        fs::create_dir_all(parent(&prefix).unwrap()).unwrap();
        if !Command::new(&btrfs)
            .args(["subvolume", "create"])
            .arg(&prefix)
            .output()
            .unwrap()
            .status
            .success()
        {
            return;
        }
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        fs::write(prefix.join("drive_c/save"), "original").unwrap();
        let config = paths.environment_config("game");
        fs::create_dir_all(parent(&config).unwrap()).unwrap();
        fs::write(
            &config,
            format!(
                "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
                prefix.display()
            ),
        )
        .unwrap();
        let config_bytes = fs::read(&config).unwrap();
        let art = paths.art_dir("game");
        fs::create_dir_all(&art).unwrap();
        fs::write(art.join("cover.png"), b"art").unwrap();
        let stats = paths.stats_log("game");
        fs::create_dir_all(parent(&stats).unwrap()).unwrap();
        fs::write(&stats, "playtime\n").unwrap();
        let entry = remove(&paths, "game", true).unwrap();
        let retained = prefix
            .parent()
            .unwrap()
            .join(".wineo-trash")
            .join(&entry.id)
            .join("prefix");
        let saved = identity(&retained).unwrap().unwrap();
        assert_ne!(
            saved.device,
            identity(parent(&retained).unwrap())
                .unwrap()
                .unwrap()
                .device
        );
        assert!(
            read_manifest(&paths, &entry.id)
                .unwrap()
                .payloads
                .iter()
                .find(|payload| payload.kind == Kind::Prefix)
                .unwrap()
                .copied_identity
                .is_none()
        );
        let review = review_prune(&paths, Selector::Id(&entry.id)).unwrap();
        let manifest_path = entry_dir(&paths, &entry.id).unwrap().join(MANIFEST);
        let manifest_bytes = fs::read(&manifest_path).unwrap();
        let metadata = entry_dir(&paths, &entry.id).unwrap();
        let preserved = parent(&retained).unwrap().join("preserved-original");
        fs::rename(&retained, &preserved).unwrap();
        assert!(
            Command::new(&btrfs)
                .args(["subvolume", "create"])
                .arg(&retained)
                .output()
                .unwrap()
                .status
                .success()
        );
        fs::create_dir_all(retained.join("drive_c")).unwrap();
        fs::write(retained.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        fs::write(retained.join("drive_c/save"), "original").unwrap();
        let replacement = identity(&retained).unwrap().unwrap();
        assert_ne!(replacement, saved);
        assert_eq!(digest(&retained).unwrap(), digest(&preserved).unwrap());
        if prune {
            let outcomes = prune_reviewed(&paths, &review);
            assert!(outcomes[0].1.is_err());
        } else {
            assert!(restore(&paths, &entry.id).is_err());
        }
        assert_eq!(fs::read(&manifest_path).unwrap(), manifest_bytes);
        assert_eq!(inspect(&paths, &entry.id).unwrap().phase, Phase::Trashed);
        assert_eq!(
            fs::read(metadata.join("config.toml")).unwrap(),
            config_bytes
        );
        assert_eq!(fs::read(metadata.join("art/cover.png")).unwrap(), b"art");
        assert_eq!(
            fs::read(metadata.join("stats.jsonl")).unwrap(),
            b"playtime\n"
        );
        assert!(!config.exists());
        assert!(!art.exists());
        assert!(!stats.exists());
        assert!(!prefix.exists());
        assert_eq!(identity(&retained).unwrap(), Some(replacement));
        assert_eq!(identity(&preserved).unwrap(), Some(saved));
        assert!(retained.join("drive_c/save").exists());
        assert!(preserved.join("drive_c/save").exists());
        assert!(pending_entries(&paths).unwrap().is_empty());
    }
}

#[test]
fn pruning_a_plain_removal_leaves_prefix_and_snapshots_untouched() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let snapshots = f.paths.snapshot_root(&prefix, "game");
    fs::create_dir_all(&snapshots).unwrap();
    fs::write(snapshots.join("keep"), "saved").unwrap();
    let entry = remove(&f.paths, "game", false).unwrap();
    let reviewed = review_prune(&f.paths, Selector::Id(&entry.id)).unwrap();
    assert!(prune_reviewed(&f.paths, &reviewed)[0].1.is_ok());
    assert!(prefix.join("system.reg").exists());
    assert_eq!(fs::read(snapshots.join("keep")).unwrap(), b"saved");
}

#[test]
fn prune_age_uses_strict_elapsed_day_cutoff() {
    let f = Fixture::new();
    f.config("edge", "kind = 'native'\n");
    let edge = remove(&f.paths, "edge", false).unwrap();
    f.config("old", "kind = 'native'\n");
    let old = remove(&f.paths, "old", false).unwrap();
    for (entry, date) in [
        (&edge, "2026-09-26T00:00:00Z"),
        (&old, "2026-09-25T23:59:59.999999999Z"),
    ] {
        let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
        manifest.removed_at = date.to_owned();
        write_manifest(&f.paths, &manifest).unwrap();
    }
    let now = "2026-09-27T00:00:00Z".parse().unwrap();
    let reviewed = review_prune_at(&f.paths, Selector::OlderThan(1), now).unwrap();
    assert_eq!(
        reviewed.cutoff,
        Some("2026-09-26T00:00:00Z".parse().unwrap())
    );
    assert_eq!(
        reviewed
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        [&old.id]
    );
}

#[test]
fn interrupted_removal_rolls_back_without_overwriting_a_collision() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n# original\n");
    let entry = remove(&f.paths, "game", false).unwrap();
    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    manifest.phase = Phase::Removing;
    write_manifest(&f.paths, &manifest).unwrap();
    f.config("game", "kind = 'native'\n# new\n");
    assert!(
        recover(&f.paths, &entry.id)
            .unwrap_err()
            .to_string()
            .contains("collision")
    );
    assert_eq!(
        fs::read_to_string(f.paths.environment_config("game")).unwrap(),
        "kind = 'native'\n# new\n"
    );
    assert!(
        entry_dir(&f.paths, &entry.id)
            .unwrap()
            .join("config.toml")
            .exists()
    );
    fs::remove_file(f.paths.environment_config("game")).unwrap();
    recover(&f.paths, &entry.id).unwrap();
    assert_eq!(
        fs::read_to_string(f.paths.environment_config("game")).unwrap(),
        "kind = 'native'\n# original\n"
    );
}

#[test]
fn restore_recognizes_a_rename_completed_before_progress_was_written() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n# retained\n");
    let entry = remove(&f.paths, "game", false).unwrap();
    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    manifest.phase = Phase::Restoring;
    manifest.payloads[0].step = Step::Returning;
    write_manifest(&f.paths, &manifest).unwrap();
    let retained = entry_dir(&f.paths, &entry.id).unwrap().join("config.toml");
    rename_owned(&retained, &f.paths.environment_config("game")).unwrap();
    recover(&f.paths, &entry.id).unwrap();
    assert_eq!(
        fs::read(f.paths.environment_config("game")).unwrap(),
        b"kind = 'native'\n# retained\n"
    );
    assert!(list(&f.paths).unwrap().is_empty());
}

#[test]
fn stale_prune_review_and_symlink_substitution_do_not_delete_unrelated_data() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n");
    let entry = remove(&f.paths, "game", false).unwrap();
    let review = review_prune(&f.paths, Selector::All).unwrap();
    let manifest = read_manifest(&f.paths, &entry.id).unwrap();
    write_manifest(&f.paths, &manifest).unwrap();
    // A changed manifest, even when its entry still exists, invalidates
    // the previously confirmed selection.
    let mut changed = manifest;
    changed.removed_at = "2020-01-01T00:00:00Z".into();
    write_manifest(&f.paths, &changed).unwrap();
    assert!(prune_reviewed(&f.paths, &review)[0].1.is_err());
    let unrelated = f._root.path().join("keep");
    fs::write(&unrelated, "still here").unwrap();
    let retained = entry_dir(&f.paths, &entry.id).unwrap().join("config.toml");
    fs::remove_file(&retained).unwrap();
    std::os::unix::fs::symlink(&unrelated, &retained).unwrap();
    assert!(review_prune(&f.paths, Selector::Id(&entry.id)).is_ok());
    let selected = review_prune(&f.paths, Selector::Id(&entry.id)).unwrap();
    assert!(prune_reviewed(&f.paths, &selected)[0].1.is_err());
    assert_eq!(fs::read(unrelated).unwrap(), b"still here");
    assert_eq!(inspect(&f.paths, &entry.id).unwrap().phase, Phase::Trashed);
}

#[test]
fn a_cross_filesystem_metadata_copy_is_verified_and_recovered() {
    let Some(config_root) = tempfile::tempdir_in("/dev/shm").ok() else {
        return;
    };
    let f = Fixture::new();
    let home = f._root.path().to_owned();
    let config_home = config_root.path().to_owned();
    let paths = Paths::from_lookup(move |key| match key {
        "HOME" => Some(OsString::from(home.as_os_str())),
        "XDG_CONFIG_HOME" => Some(OsString::from(config_home.as_os_str())),
        _ => None,
    })
    .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::create_dir_all(paths.trash_dir().parent().unwrap()).unwrap();
    let config = paths.environment_config("game");
    fs::write(&config, "kind = 'native'\n# raw\n").unwrap();
    if fs::metadata(parent(&config).unwrap()).unwrap().dev()
        == fs::metadata(paths.trash_dir().parent().unwrap())
            .unwrap()
            .dev()
    {
        return;
    }
    let entry = remove(&paths, "game", false).unwrap();
    assert!(config.symlink_metadata().is_err());
    restore(&paths, &entry.id).unwrap();
    assert_eq!(fs::read(&config).unwrap(), b"kind = 'native'\n# raw\n");

    let entry = remove(&paths, "game", false).unwrap();
    let mut manifest = read_manifest(&paths, &entry.id).unwrap();
    manifest.phase = Phase::Restoring;
    let payload = &mut manifest.payloads[0];
    payload.step = Step::Returning;
    payload.returned_identity = Some(
        reserve_copy(
            &entry_dir(&paths, &entry.id).unwrap().join("config.toml"),
            &config,
        )
        .unwrap(),
    );
    fs::write(&config, b"incomplete copy").unwrap();
    write_manifest(&paths, &manifest).unwrap();
    recover(&paths, &entry.id).unwrap();
    assert_eq!(fs::read(&config).unwrap(), b"kind = 'native'\n# raw\n");
}

/// Art and trash share the data directory, so the cross-device copy
/// keeps its retained side on /dev/shm and drives the rollback directly.
fn interrupted_art_copy(
    step: Step,
) -> Option<(Fixture, tempfile::TempDir, PathBuf, PathBuf, Manifest)> {
    interrupted_art_copy_of(
        step,
        &[
            ("icon.png", "icon"),
            ("hero.png", "hero"),
            ("grid/a.png", "grid"),
        ],
    )
}

fn interrupted_art_copy_of(
    step: Step,
    files: &[(&str, &str)],
) -> Option<(Fixture, tempfile::TempDir, PathBuf, PathBuf, Manifest)> {
    let other = tempfile::tempdir_in("/dev/shm").ok()?;
    let f = Fixture::new();
    let source = f.paths.art_dir("game");
    fs::create_dir_all(&source).unwrap();
    for (file, text) in files {
        let path = source.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let retained = other.path().join("art");
    let mut payload = make_payload(&f.paths, Kind::Art, &source, &retained)
        .unwrap()
        .unwrap();
    if payload.source_parent.mount == payload.destination_parent.mount {
        return None;
    }
    copy_verified(&source, &retained, &payload.digest).unwrap();
    payload.copied_identity = identity(&retained).unwrap();
    payload.step = step;
    let id = "20260926T183000Z-a1b2c3d5";
    let dir = entry_dir(&f.paths, id).unwrap();
    create_storage(&dir).unwrap();
    let manifest = Manifest {
        version: VERSION,
        id: id.into(),
        entry_identity: identity(&dir).unwrap().unwrap(),
        name: "game".into(),
        removed_at: "2026-09-26T18:30:00Z".into(),
        include_prefix: false,
        prefix_present: None,
        prefix_reference: None,
        prefix_identity: None,
        phase: Phase::Removing,
        payloads: vec![payload],
        gog_folders: Vec::new(),
    };
    write_manifest(&f.paths, &manifest).unwrap();
    Some((f, other, source, retained, manifest))
}

#[test]
fn rollback_rebuilds_art_whose_source_deletion_was_interrupted() {
    let Some((f, _other, source, retained, mut manifest)) =
        interrupted_art_copy(Step::DeletingSource)
    else {
        return;
    };
    fs::remove_file(source.join("icon.png")).unwrap();
    fs::remove_file(source.join("grid/a.png")).unwrap();
    return_payload(&f.paths, &mut manifest, 0, true).unwrap();
    assert_eq!(manifest.payloads[0].step, Step::Returned);
    for (file, text) in [
        ("icon.png", "icon"),
        ("hero.png", "hero"),
        ("grid/a.png", "grid"),
    ] {
        assert_eq!(fs::read_to_string(source.join(file)).unwrap(), text);
    }
    assert!(!retained.exists());
}

#[test]
fn rollback_keeps_both_copies_when_an_untouched_source_differs() {
    let Some((f, _other, source, retained, mut manifest)) = interrupted_art_copy(Step::Copied)
    else {
        return;
    };
    fs::write(source.join("hero.png"), "edited").unwrap();
    return_payload(&f.paths, &mut manifest, 0, true).unwrap();
    assert!(manifest.payloads[0].kept);
    assert_eq!(
        fs::read_to_string(source.join("hero.png")).unwrap(),
        "edited"
    );
    assert_eq!(
        fs::read_to_string(retained.join("hero.png")).unwrap(),
        "hero"
    );
    assert_eq!(
        fs::read_to_string(retained.join("icon.png")).unwrap(),
        "icon"
    );
}

#[test]
fn rollback_turns_a_copy_of_a_changed_source_into_a_trash_entry() {
    changed_config_rollback(|source, text| fs::write(source, text).unwrap());
}

#[test]
fn rollback_keeps_a_config_replaced_by_an_atomic_save() {
    changed_config_rollback(|source, text| {
        let temp = source.with_extension("toml.tmp");
        fs::write(&temp, text).unwrap();
        fs::rename(&temp, source).unwrap();
    });
}

#[test]
fn rollback_drops_the_copy_when_an_atomic_save_kept_the_same_contents() {
    let Some(config_root) = tempfile::tempdir_in("/dev/shm").ok() else {
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let (paths, source, id) = copied_config(home.path(), config_root.path());
    let Some(id) = id else { return };
    let temp = source.with_extension("toml.tmp");
    fs::write(&temp, "kind = 'native'\n# before\n").unwrap();
    fs::rename(&temp, &source).unwrap();
    assert!(recover_noting(&paths, id).unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "kind = 'native'\n# before\n"
    );
    assert!(list(&paths).unwrap().is_empty());
}

/// A config copied across filesystems into a removal that stopped
/// before deleting it. None for the id when there is no second filesystem.
fn copied_config(home: &Path, config_home: &Path) -> (Paths, PathBuf, Option<&'static str>) {
    let home = home.to_owned();
    let config_home = config_home.to_owned();
    let paths = Paths::from_lookup(move |key| match key {
        "HOME" => Some(OsString::from(home.as_os_str())),
        "XDG_CONFIG_HOME" => Some(OsString::from(config_home.as_os_str())),
        _ => None,
    })
    .unwrap();
    let source = paths.environment_config("game");
    fs::create_dir_all(parent(&source).unwrap()).unwrap();
    fs::write(&source, "kind = 'native'\n# before\n").unwrap();
    let id = "20260926T183000Z-a1b2c3d6";
    let dir = entry_dir(&paths, id).unwrap();
    create_storage(&dir).unwrap();
    let dest = dir.join("config.toml");
    let mut payload = make_payload(&paths, Kind::Config, &source, &dest)
        .unwrap()
        .unwrap();
    if payload.source_parent.mount == payload.destination_parent.mount {
        return (paths, source, None);
    }
    copy_verified(&source, &dest, &payload.digest).unwrap();
    payload.copied_identity = identity(&dest).unwrap();
    payload.step = Step::Copied;
    let manifest = Manifest {
        version: VERSION,
        id: id.into(),
        entry_identity: identity(&dir).unwrap().unwrap(),
        name: "game".into(),
        removed_at: "2026-09-26T18:30:00Z".into(),
        include_prefix: false,
        prefix_present: None,
        prefix_reference: None,
        prefix_identity: None,
        phase: Phase::Removing,
        payloads: vec![payload],
        gog_folders: Vec::new(),
    };
    write_manifest(&paths, &manifest).unwrap();
    (paths, source, Some(id))
}

fn changed_config_rollback(edit: impl Fn(&Path, &str)) {
    let Some(config_root) = tempfile::tempdir_in("/dev/shm").ok() else {
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let (paths, source, id) = copied_config(home.path(), config_root.path());
    let Some(id) = id else { return };
    // An edit lands after the copy, before deletion begins.
    edit(&source, "kind = 'native'\n# after\n");
    let notes = recover_noting(&paths, id).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains(&format!("trash entry {id}")), "{notes:?}");
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "kind = 'native'\n# after\n"
    );
    let entry = inspect(&paths, id).unwrap();
    assert_eq!(entry.phase, Phase::Trashed);
    assert!(check_pending(&paths, "game", &[]).is_ok());
    // Restorable once the live environment is out of the way.
    assert!(restore(&paths, id).is_err());
    fs::remove_file(&source).unwrap();
    restore(&paths, id).unwrap();
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "kind = 'native'\n# before\n"
    );
    assert!(list(&paths).unwrap().is_empty());
}

#[test]
fn rollback_keeps_art_replaced_by_a_new_folder() {
    let Some((f, _other, source, retained, mut manifest)) = interrupted_art_copy(Step::Copied)
    else {
        return;
    };
    let replacement = source.with_file_name("game.new");
    fs::create_dir(&replacement).unwrap();
    fs::write(replacement.join("hero.png"), "new hero").unwrap();
    fs::remove_dir_all(&source).unwrap();
    fs::rename(&replacement, &source).unwrap();
    return_payload(&f.paths, &mut manifest, 0, true).unwrap();
    assert!(manifest.payloads[0].kept);
    assert_eq!(
        fs::read_to_string(source.join("hero.png")).unwrap(),
        "new hero"
    );
    assert_eq!(
        fs::read_to_string(retained.join("icon.png")).unwrap(),
        "icon"
    );
}

#[test]
fn digests_tell_apart_trees_whose_v1_input_was_the_same() {
    let root = tempfile::tempdir().unwrap();
    let (two, one) = (root.path().join("two"), root.path().join("one"));
    fs::create_dir_all(&two).unwrap();
    fs::create_dir_all(&one).unwrap();
    fs::write(two.join("a"), "x").unwrap();
    fs::write(two.join("b"), "y").unwrap();
    fs::write(one.join("a"), "xbfiley").unwrap();
    assert_eq!(digest_v1(&two).unwrap(), digest_v1(&one).unwrap());
    assert_ne!(digest(&two).unwrap(), digest(&one).unwrap());
    assert!(!same_tree(&two, &one).unwrap());
    // A manifest written before v2 still verifies its own payload.
    assert!(digest_matches(&two, &digest_v1(&two).unwrap()).unwrap());
    assert!(!digest_matches(&two, &digest(&one).unwrap()).unwrap());
}

#[test]
fn rollback_keeps_a_copy_whose_replacement_only_collides_in_a_v1_digest() {
    for v1 in [false, true] {
        let Some((f, _other, source, retained, mut manifest)) =
            interrupted_art_copy_of(Step::Copied, &[("a", "x"), ("b", "y")])
        else {
            return;
        };
        if v1 {
            manifest.payloads[0].digest = digest_v1(&retained).unwrap();
            write_manifest(&f.paths, &manifest).unwrap();
        }
        let replacement = source.with_file_name("game.new");
        fs::create_dir(&replacement).unwrap();
        fs::write(replacement.join("a"), "xbfiley").unwrap();
        fs::remove_dir_all(&source).unwrap();
        fs::rename(&replacement, &source).unwrap();
        return_payload(&f.paths, &mut manifest, 0, true).unwrap();
        assert!(manifest.payloads[0].kept, "v1: {v1}");
        assert_eq!(fs::read_to_string(source.join("a")).unwrap(), "xbfiley");
        assert_eq!(fs::read_to_string(retained.join("b")).unwrap(), "y");
    }
}

#[test]
fn dropping_the_copy_of_an_identical_replacement_resumes_after_an_interruption() {
    let Some(config_root) = tempfile::tempdir_in("/dev/shm").ok() else {
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let (paths, source, id) = copied_config(home.path(), config_root.path());
    let Some(id) = id else { return };
    let temp = source.with_extension("toml.tmp");
    fs::write(&temp, "kind = 'native'\n# before\n").unwrap();
    fs::rename(&temp, &source).unwrap();
    set_trash_failpoint(Some("rollback.after_dropping_copy"));
    let error = recover_noting(&paths, id).unwrap_err();
    set_trash_failpoint(None);
    assert!(error.to_string().contains("injected"), "{error}");
    assert!(!entry_dir(&paths, id).unwrap().join("config.toml").exists());
    assert!(recover_noting(&paths, id).unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "kind = 'native'\n# before\n"
    );
    assert!(list(&paths).unwrap().is_empty());
    assert!(check_pending(&paths, "game", &[]).is_ok());
}

#[test]
fn restore_keeps_a_retained_copy_that_no_longer_matches_the_returned_one() {
    let Some((f, _other, source, retained, mut manifest)) =
        interrupted_art_copy_of(Step::Retained, &[("a", "x"), ("b", "y")])
    else {
        return;
    };
    fs::remove_dir_all(&source).unwrap();
    manifest.phase = Phase::Restoring;
    // An older manifest, whose v1 digest cannot tell the trees apart.
    manifest.payloads[0].digest = digest_v1(&retained).unwrap();
    write_manifest(&f.paths, &manifest).unwrap();
    BEFORE_RETAINED_DROP.with(|hook| {
        hook.set(Some(|_, retained| {
            fs::remove_file(retained.join("b")).unwrap();
            fs::write(retained.join("a"), "xbfiley").unwrap();
        }))
    });
    let error = return_payload(&f.paths, &mut manifest, 0, false).unwrap_err();
    assert!(error.to_string().contains("both were kept"), "{error}");
    assert_eq!(fs::read_to_string(retained.join("a")).unwrap(), "xbfiley");
    assert_eq!(fs::read_to_string(source.join("b")).unwrap(), "y");
}

#[test]
fn rollback_keeps_changed_art_and_adds_the_live_config() {
    let Some((f, _other, source, retained, mut manifest)) = interrupted_art_copy(Step::Copied)
    else {
        return;
    };
    f.config("game", "kind = 'native'\n");
    fs::write(source.join("hero.png"), "edited").unwrap();
    let mut notes = Vec::new();
    return_payload(&f.paths, &mut manifest, 0, true).unwrap();
    keep_changed(&f.paths, &mut manifest, &mut notes).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(manifest.phase, Phase::Trashed);
    assert_eq!(
        manifest
            .payloads
            .iter()
            .map(|payload| payload.kind)
            .collect::<Vec<_>>(),
        [Kind::Config, Kind::Art]
    );
    assert_eq!(
        fs::read_to_string(retained.join("hero.png")).unwrap(),
        "hero"
    );
    assert_eq!(
        fs::read_to_string(source.join("hero.png")).unwrap(),
        "edited"
    );
}

#[test]
fn removal_recovers_when_both_metadata_copies_survive_a_crash() {
    let Some(config_root) = tempfile::tempdir_in("/dev/shm").ok() else {
        return;
    };
    let f = Fixture::new();
    let home = f._root.path().to_owned();
    let config_home = config_root.path().to_owned();
    let paths = Paths::from_lookup(move |key| match key {
        "HOME" => Some(OsString::from(home.as_os_str())),
        "XDG_CONFIG_HOME" => Some(OsString::from(config_home.as_os_str())),
        _ => None,
    })
    .unwrap();
    let source = paths.environment_config("game");
    fs::create_dir_all(parent(&source).unwrap()).unwrap();
    fs::write(&source, "kind = 'native'\n# only source is authoritative\n").unwrap();
    let id = "20260926T183000Z-a1b2c3d4";
    let dir = entry_dir(&paths, id).unwrap();
    create_storage(&dir).unwrap();
    let dest = dir.join("config.toml");
    let mut payload = make_payload(&paths, Kind::Config, &source, &dest)
        .unwrap()
        .unwrap();
    if payload.identity.device == payload.destination_parent.device {
        return;
    }
    payload.step = Step::Copying;
    let mut manifest = Manifest {
        version: VERSION,
        id: id.into(),
        entry_identity: identity(&dir).unwrap().unwrap(),
        name: "game".into(),
        removed_at: "2026-09-26T18:30:00Z".into(),
        include_prefix: false,
        prefix_present: None,
        prefix_reference: None,
        prefix_identity: None,
        phase: Phase::Removing,
        payloads: vec![payload],
        gog_folders: Vec::new(),
    };
    write_manifest(&paths, &manifest).unwrap();
    manifest.payloads[0].copied_identity = Some(reserve_copy(&source, &dest).unwrap());
    write_manifest(&paths, &manifest).unwrap();
    copy_verified(&source, &dest, &manifest.payloads[0].digest).unwrap();
    manifest.payloads[0].step = Step::Copied;
    write_manifest(&paths, &manifest).unwrap();
    recover(&paths, id).unwrap();
    assert_eq!(
        fs::read(&source).unwrap(),
        b"kind = 'native'\n# only source is authoritative\n"
    );
    assert!(!dir.exists());
}

#[test]
fn portable_entry_restores_after_the_folder_moves() {
    let root = tempfile::tempdir().unwrap();
    let from = root.path().join("old");
    let to = root.path().join("new");
    let home = root.path().to_owned();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&from),
    )
    .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(paths.environment_config("game"), "kind = 'native'\n").unwrap();
    let entry = remove(&paths, "game", false).unwrap();
    fs::rename(from, &to).unwrap();
    let home = root.path().to_owned();
    let moved = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&to),
    )
    .unwrap();
    assert_eq!(inspect(&moved, &entry.id).unwrap().name, "game");
    restore(&moved, &entry.id).unwrap();
    assert_eq!(
        fs::read(moved.environment_config("game")).unwrap(),
        b"kind = 'native'\n"
    );
}

#[test]
fn portable_prefix_and_snapshots_restore_after_a_rename() {
    let root = tempfile::tempdir().unwrap();
    let from = root.path().join("old");
    let to = root.path().join("new");
    let home = root.path().to_owned();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&from),
    )
    .unwrap();
    let prefix = paths.default_prefix_root().join("game");
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let snapshots = paths.snapshot_root(&prefix, "game");
    fs::create_dir_all(&snapshots).unwrap();
    fs::write(snapshots.join("saved"), "save").unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("game"),
        "prefix = 'prefixes/game'\nrunner = 'wine'\narch = 'win64'\n",
    )
    .unwrap();
    let entry = remove(&paths, "game", true).unwrap();
    fs::rename(from, &to).unwrap();
    let home = root.path().to_owned();
    let moved = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&to),
    )
    .unwrap();
    restore(&moved, &entry.id).unwrap();
    assert!(moved.default_prefix_root().join("game/system.reg").exists());
    assert_eq!(
        fs::read(
            moved
                .snapshot_root(&moved.default_prefix_root().join("game"), "game")
                .join("saved")
        )
        .unwrap(),
        b"save"
    );
}

#[test]
fn a_prune_resumes_after_an_unrecorded_unlink() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n");
    let entry = remove(&f.paths, "game", false).unwrap();
    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    manifest.phase = Phase::Pruning;
    manifest.payloads[0].step = Step::Deleting;
    write_manifest(&f.paths, &manifest).unwrap();
    let retained = entry_dir(&f.paths, &entry.id).unwrap().join("config.toml");
    delete_owned(&retained).unwrap();
    recover(&f.paths, &entry.id).unwrap();
    assert!(list(&f.paths).unwrap().is_empty());
}

#[test]
fn missing_retained_storage_never_counts_as_a_successful_prune() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let entry = remove(&f.paths, "game", true).unwrap();
    let container = prefix
        .parent()
        .unwrap()
        .join(".wineo-trash")
        .join(&entry.id);
    let hidden = prefix.parent().unwrap().join("hidden-trash");
    fs::rename(&container, &hidden).unwrap();
    let reviewed = review_prune(&f.paths, Selector::Id(&entry.id)).unwrap();
    assert!(prune_reviewed(&f.paths, &reviewed)[0].1.is_err());
    assert_eq!(inspect(&f.paths, &entry.id).unwrap().phase, Phase::Trashed);
    assert!(hidden.join("prefix/system.reg").exists());
    fs::rename(hidden, container).unwrap();
    assert!(prune_reviewed(&f.paths, &reviewed)[0].1.is_ok());
}

#[test]
fn an_unrelated_desktop_shortcut_is_not_overwritten_on_restore() {
    let f = Fixture::new();
    let exe = f._root.path().join("game.sh");
    write_script(&exe, "#!/bin/sh\nexit 0\n");
    f.config(
        "game",
        &format!(
            "kind = 'native'\n[targets.game]\nexe = '{}'\ndesktop = true\n",
            exe.display()
        ),
    );
    let entry = remove(&f.paths, "game", false).unwrap();
    let shortcut = f.paths.applications_dir().join("wineo-game-game.desktop");
    fs::create_dir_all(parent(&shortcut).unwrap()).unwrap();
    fs::write(&shortcut, "unrelated shortcut").unwrap();
    let err = restore(&f.paths, &entry.id).unwrap_err();
    assert!(err.to_string().contains("was not overwritten"), "{err:#}");
    assert_eq!(fs::read(&shortcut).unwrap(), b"unrelated shortcut");
    assert!(f.paths.environment_config("game").exists());
    assert!(list(&f.paths).unwrap().is_empty());
}

#[test]
fn an_absent_config_still_blocks_operations_until_validate_recovers() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n");
    let entry = remove(&f.paths, "game", false).unwrap();
    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    manifest.phase = Phase::Removing;
    write_manifest(&f.paths, &manifest).unwrap();
    assert!(
        Environment::load(&f.paths, "game")
            .unwrap_err()
            .to_string()
            .contains("validate --fix")
    );
    assert!(
        Locks::new(&f.paths)
            .acquire(Operation::Create, "game", &[])
            .is_err()
    );
    let serialized = toml::to_string(&Locks::new(&f.paths)).unwrap();
    let supervisor_locks: Locks = toml::from_str(&serialized).unwrap();
    assert!(
        supervisor_locks
            .acquire(Operation::Launch, "game", &[])
            .is_err()
    );
    assert!(
        crate::settings::update(
            &f.paths,
            crate::settings::ConfigFile::Environment("game"),
            &[("gamemode", "true")],
            &[],
        )
        .is_err()
    );
    let report = crate::validate::check_all(&f.paths).unwrap();
    let problem = report
        .problems
        .iter()
        .find(|problem| problem.code == crate::validate::Code::TrashInterrupted)
        .unwrap();
    let fix = problem.fix.as_ref().unwrap();
    crate::validate::apply(
        &f.paths,
        fix,
        &crate::cancel::Token::new(),
        crate::winetricks::EnsureOutput::Stderr,
        |_| {},
        |_, _| unreachable!(),
    )
    .unwrap();
    assert!(Environment::load(&f.paths, "game").is_ok());
}

#[test]
fn a_damaged_manifest_blocks_prune_and_stray_cleanup() {
    let f = Fixture::new();
    f.config("game", "kind = 'native'\n");
    let entry = remove(&f.paths, "game", false).unwrap();
    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    manifest.version = VERSION + 1;
    write_manifest(&f.paths, &manifest).unwrap();
    assert!(inspect(&f.paths, &entry.id).is_err());
    let review = review_prune(&f.paths, Selector::All).unwrap();
    assert!(review.entries.is_empty());
    assert_eq!(review.errors.len(), 1);
    let stats = f.paths.stats_log("stray");
    fs::create_dir_all(parent(&stats).unwrap()).unwrap();
    fs::write(&stats, "never delete through a damaged index").unwrap();
    let report = crate::validate::check_all(&f.paths).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.code == crate::validate::Code::TrashInvalid)
    );
    assert!(
        !report
            .problems
            .iter()
            .any(|problem| problem.code == crate::validate::Code::StatsStray)
    );
    assert!(stats.exists());
}

#[test]
fn batch_prune_continues_past_an_independent_damaged_entry() {
    let f = Fixture::new();
    f.config("bad", "kind = 'native'\n");
    let bad = remove(&f.paths, "bad", false).unwrap();
    f.config("good", "kind = 'native'\n");
    let good = remove(&f.paths, "good", false).unwrap();
    let mut damaged = read_manifest(&f.paths, &bad.id).unwrap();
    damaged.version = VERSION + 1;
    write_manifest(&f.paths, &damaged).unwrap();
    let review = review_prune(&f.paths, Selector::All).unwrap();
    assert_eq!(review.entries.len(), 1);
    assert_eq!(review.entries[0].id, good.id);
    assert_eq!(review.errors.len(), 1);
    assert!(prune_reviewed(&f.paths, &review)[0].1.is_ok());
    assert!(entry_dir(&f.paths, &bad.id).unwrap().exists());
    assert!(!entry_dir(&f.paths, &good.id).unwrap().exists());
}

mod gog {
    use super::*;
    use crate::gog::folder::{
        Committed, FolderState, Journal, JournalKind, Platform, Target, Track,
    };

    const NEVER: &dyn Fn() -> bool = &|| false;

    fn game(folder: &Path, owner: Option<&str>, journal: bool) -> FolderState {
        let target = Target {
            gog_id: 1094900565,
            platform: Platform::Linux,
            language: "en-US".to_owned(),
            track: Track::Installer,
            products: Vec::new(),
        };
        FolderState {
            committed: Some(Committed {
                source: None,
                gog_id: target.gog_id,
                platform: target.platform,
                language: target.language.clone(),
                track: target.track,
                products: Vec::new(),
                files: Vec::new(),
            }),
            journal: journal.then(|| Journal {
                operation: "op-1".to_owned(),
                kind: JournalKind::Update,
                target,
                packages: Vec::new(),
                attempted: Vec::new(),
            }),
            ..FolderState::new(folder.to_owned(), owner.map(str::to_owned))
        }
    }

    fn save(paths: &Paths, state: &FolderState) {
        FolderLock::exclusive(paths, &state.folder, NEVER)
            .unwrap()
            .write(state)
            .unwrap();
    }

    fn load(paths: &Paths, folder: &Path) -> FolderState {
        FolderLock::shared(paths, folder, NEVER)
            .unwrap()
            .read()
            .unwrap()
            .unwrap()
    }

    const RIMWORLD: &str = "kind = 'native'\ngog_id = '1094900565'\n";

    fn setup(journal: bool) -> (Fixture, PathBuf) {
        let f = Fixture::new();
        f.config("rimworld", RIMWORLD);
        let folder = f._root.path().join("Games/RimWorld");
        save(&f.paths, &game(&folder, Some("rimworld"), journal));
        (f, folder)
    }

    #[test]
    fn rm_releases_the_folder_and_restore_claims_it_again() {
        let (f, folder) = setup(false);
        let entry = remove(&f.paths, "rimworld", false).unwrap();
        assert_eq!(load(&f.paths, &folder).owner, None);
        assert_eq!(
            read_manifest(&f.paths, &entry.id).unwrap().gog_folders,
            std::slice::from_ref(&folder)
        );
        restore(&f.paths, &entry.id).unwrap();
        assert_eq!(load(&f.paths, &folder).owner.as_deref(), Some("rimworld"));
        assert_eq!(
            fs::read_to_string(f.paths.environment_config("rimworld")).unwrap(),
            RIMWORLD
        );
    }

    #[test]
    fn rm_of_a_galaxy_install_names_the_install_command() {
        let (f, folder) = setup(true);
        let mut state = load(&f.paths, &folder);
        let journal = state.journal.as_mut().unwrap();
        journal.kind = JournalKind::Install;
        journal.target.track = Track::Galaxy;
        save(&f.paths, &state);
        let error = remove(&f.paths, "rimworld", false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "rimworld's GOG install didn't finish. Finish it with wineo import gog --game 1094900565, or remove it anyway with --force."
        );
        assert!(f.paths.environment_config("rimworld").exists());
    }

    #[test]
    fn rm_mid_update_is_refused_without_force() {
        let (f, folder) = setup(true);
        let error = remove(&f.paths, "rimworld", false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "rimworld is mid-update. Finish it with wineo update rimworld, or remove it anyway with --force."
        );
        assert_eq!(load(&f.paths, &folder).owner.as_deref(), Some("rimworld"));
        assert!(f.paths.environment_config("rimworld").exists());
        assert!(list(&f.paths).unwrap().is_empty());

        let entry = remove_with_force(
            &f.paths,
            "rimworld",
            false,
            true,
            &crate::cancel::Token::new(),
            |_| {},
        )
        .unwrap();
        let state = load(&f.paths, &folder);
        assert_eq!(state.owner, None);
        assert!(state.journal.is_some());
        assert!(!f.paths.environment_config("rimworld").exists());

        // Restoring claims it back with the journal still there.
        restore(&f.paths, &entry.id).unwrap();
        let state = load(&f.paths, &folder);
        assert_eq!(state.owner.as_deref(), Some("rimworld"));
        assert!(state.journal.is_some());
    }

    #[test]
    fn a_crash_between_rm_state_and_config_writes_leaves_an_ownerless_folder() {
        let (f, folder) = setup(false);
        set_trash_failpoint(Some("gog.after_release"));
        assert!(remove(&f.paths, "rimworld", false).is_err());
        assert_eq!(load(&f.paths, &folder).owner, None);
        assert_eq!(
            fs::read_to_string(f.paths.environment_config("rimworld")).unwrap(),
            RIMWORLD
        );
        assert!(list(&f.paths).unwrap().is_empty());
        assert!(pending_entries(&f.paths).unwrap().is_empty());
        // Removing again works from that state.
        remove(&f.paths, "rimworld", false).unwrap();
    }

    #[test]
    fn a_crash_between_restore_config_and_state_writes_is_finished_by_recovery() {
        let (f, folder) = setup(false);
        let entry = remove(&f.paths, "rimworld", false).unwrap();
        set_trash_failpoint(Some("gog.before_reclaim"));
        assert!(restore(&f.paths, &entry.id).is_err());
        // The config is back, the folder still has no owner.
        assert_eq!(
            fs::read_to_string(f.paths.environment_config("rimworld")).unwrap(),
            RIMWORLD
        );
        assert_eq!(load(&f.paths, &folder).owner, None);
        recover(&f.paths, &entry.id).unwrap();
        assert_eq!(load(&f.paths, &folder).owner.as_deref(), Some("rimworld"));
        assert!(list(&f.paths).unwrap().is_empty());
    }

    #[test]
    fn restore_is_refused_when_another_environment_owns_the_folder() {
        let (f, folder) = setup(false);
        let entry = remove(&f.paths, "rimworld", false).unwrap();
        save(&f.paths, &game(&folder, Some("copy"), false));
        let error = restore(&f.paths, &entry.id).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("{} belongs to copy.", folder.display())
        );
        assert!(!f.paths.environment_config("rimworld").exists());
        assert_eq!(
            read_manifest(&f.paths, &entry.id).unwrap().phase,
            Phase::Trashed
        );
        assert!(pending_entries(&f.paths).unwrap().is_empty());
    }

    #[test]
    fn a_portable_environment_of_the_same_name_never_releases_the_host_folder() {
        let (f, folder) = setup(false);
        let home = f._root.path().to_owned();
        let portable = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            Some(&f._root.path().join("usb")),
        )
        .unwrap();
        let config = portable.environment_config("rimworld");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, RIMWORLD).unwrap();
        remove(&portable, "rimworld", false).unwrap();
        assert_eq!(load(&f.paths, &folder).owner.as_deref(), Some("rimworld"));
    }

    #[test]
    fn recovering_an_interrupted_rm_claims_the_folder_back() {
        let (f, folder) = setup(false);
        set_trash_failpoint(Some("remove.after_manifest"));
        assert!(remove(&f.paths, "rimworld", false).is_err());
        assert_eq!(load(&f.paths, &folder).owner, None);
        let entries = list(&f.paths).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].phase, Phase::Removing);
        recover(&f.paths, &entries[0].id).unwrap();
        assert_eq!(
            fs::read_to_string(f.paths.environment_config("rimworld")).unwrap(),
            RIMWORLD
        );
        assert_eq!(load(&f.paths, &folder).owner.as_deref(), Some("rimworld"));
        assert!(list(&f.paths).unwrap().is_empty());
    }

    #[test]
    fn an_unreadable_state_only_blocks_removing_an_environment_tracking_a_game() {
        let (f, folder) = setup(false);
        let bad = f.paths.gog_folders_dir().join("bad.toml");
        fs::write(&bad, "version = 9\nfolder = '/x'\n").unwrap();
        f.config("plain", "kind = 'native'\n");
        remove(&f.paths, "plain", false).unwrap();

        let error = remove(&f.paths, "rimworld", false).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("rimworld's GOG state can't be checked: "),
            "{error}"
        );
        assert!(f.paths.environment_config("rimworld").exists());
        assert_eq!(load(&f.paths, &folder).owner.as_deref(), Some("rimworld"));
    }
}

#[test]
fn rebinding_identities_changes_only_the_identity_numbers() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let snapshots = f.paths.snapshot_root(&prefix, "game");
    fs::create_dir_all(&snapshots).unwrap();
    fs::write(snapshots.join("saved"), "snapshot").unwrap();
    let entry = remove(&f.paths, "game", true).unwrap();
    let file = entry_dir(&f.paths, &entry.id).unwrap().join(MANIFEST);
    let written = fs::read_to_string(&file).unwrap();

    // What a checkout does: every recorded inode is somebody else's.
    let mut manifest = read_manifest(&f.paths, &entry.id).unwrap();
    manifest.entry_identity.inode += 1;
    if let Some(identity) = &mut manifest.prefix_identity {
        identity.inode += 1;
    }
    for payload in &mut manifest.payloads {
        payload.identity.inode += 1;
        payload.source_parent.inode += 1;
        payload.destination_parent.inode += 1;
    }
    write_manifest(&f.paths, &manifest).unwrap();
    assert!(read_manifest(&f.paths, &entry.id).is_err());

    assert_eq!(rebind_identities(&f.paths).unwrap(), 1);
    assert_eq!(fs::read_to_string(&file).unwrap(), written);
    read_manifest(&f.paths, &entry.id).unwrap();
}
