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
    let config =
        "prefix = '/tmp/prefix'\nrunner = 'build'\narch = 'win64'\ndxvk = '2.0'\nvkd3d = '3.0'\n";
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
        let error = create_reviewed_with(&paths, &global, &review, &token, |event| match event {
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
        let error = restore_reviewed_with(&paths, &global, &review, &token, |event| match event {
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
    import::copy_tree_strict_reflink(&prefix, stage.path(), &prefix, &Token::new(), &mut |_| {})
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
            source: None,
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

#[test]
fn a_prefix_change_after_review_is_refused_under_the_lock() {
    let (_root, paths, prefix, _) = fixture(false);
    let global = GlobalConfig::default();
    let review = inspect_create(&paths, &global, "game", "one").unwrap();
    rewrite_keeping_mtime(&prefix.join("drive_c/game.exe"), "after!");
    let error = create_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap_err();
    assert_eq!(
        error.to_string(),
        "reviewed environment changed; inspect it again"
    );
    assert!(!review.snapshot.exists());

    if !reflinks_supported(&prefix) {
        return;
    }
    let create = inspect_create(&paths, &global, "game", "one").unwrap();
    create_reviewed(&paths, &global, &create, &Token::new(), |_| {}).unwrap();
    let review = inspect_restore(&paths, &global, "game", "one").unwrap();
    rewrite_keeping_mtime(&prefix.join("drive_c/game.exe"), "later!");
    let mut copying = false;
    let error = restore_reviewed_with(&paths, &global, &review, &Token::new(), |event| {
        if matches!(
            event,
            SnapshotEvent::Progress(ImportProgress::Started { .. } | ImportProgress::Copied { .. })
        ) {
            copying = true;
        }
    })
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "reviewed restore changed; inspect it again"
    );
    // The post-copy check gives the same error, so this proves the
    // under-lock check refused it.
    assert!(!copying);
    assert_eq!(
        fs::read_to_string(prefix.join("drive_c/game.exe")).unwrap(),
        "later!"
    );
    assert!(!marker_path(&paths, "game").exists());
}

/// Same size and mtime, so only the content fingerprint can see it.
fn rewrite_keeping_mtime(path: &Path, text: &str) {
    let modified = fs::metadata(path).unwrap().modified().unwrap();
    assert_eq!(fs::metadata(path).unwrap().len(), text.len() as u64);
    fs::write(path, text).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}
