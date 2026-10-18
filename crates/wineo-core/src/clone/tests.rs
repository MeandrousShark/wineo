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
fn wine_clone_replaces_stale_library_entry_and_warns_when_broken() {
    let root = tempfile::tempdir().unwrap();
    let (paths, _, _) = setup(root.path(), false);
    crate::library::set_favorite(&paths, "copy", true).unwrap();
    crate::library::set_hidden(&paths, "copy", true).unwrap();
    let review = inspect(&paths, &GlobalConfig::default(), "source", "copy", None).unwrap();
    let mut events = Vec::new();
    execute_reviewed(
        &paths,
        &GlobalConfig::default(),
        &review,
        &Token::new(),
        |event| events.push(event),
    )
    .unwrap();
    assert!(!crate::library::load(&paths).environments["copy"].favorite);
    assert!(!crate::library::load(&paths).environments["copy"].hidden);

    let library = paths.library_file();
    let mut broken = fs::read(&library).unwrap();
    broken.extend_from_slice(b"\n[broken\n");
    fs::write(&library, &broken).unwrap();
    let review = inspect(&paths, &GlobalConfig::default(), "source", "another", None).unwrap();
    execute_reviewed(
        &paths,
        &GlobalConfig::default(),
        &review,
        &Token::new(),
        |event| events.push(event),
    )
    .unwrap();
    assert!(paths.environment_config("another").exists());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ImportProgress::Warning(warning)
        if warning.starts_with("Can't update library.toml: ")
        && warning.contains("it can't be read")))
    );
    assert_eq!(fs::read(&library).unwrap(), broken);
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
fn cancel_during_verification_publishes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let (paths, _source, _) = setup(root.path(), false);
    let global = GlobalConfig::default();
    let destination = root.path().join("copy");
    let review = inspect(&paths, &global, "source", "copy", Some(&destination)).unwrap();
    let cancel = Token::new();
    let mut copying = false;
    let mut checks = 0;
    let err = execute_reviewed(&paths, &global, &review, &cancel, |event| match event {
        ImportProgress::Started { .. } => copying = true,
        ImportProgress::Checking { .. } if copying => {
            checks += 1;
            cancel.cancel();
        }
        ImportProgress::Finished { .. } => panic!("published a canceled clone"),
        _ => {}
    })
    .unwrap_err();
    assert!(err.is::<Canceled>(), "{err}");
    assert!(checks > 0);
    assert!(!destination.exists());
    assert!(!paths.environment_config("copy").exists());
    for entry in fs::read_dir(root.path()).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(!name.to_string_lossy().starts_with(".wineo-clone-"));
    }
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

#[test]
fn a_prefix_change_after_review_is_refused_under_the_lock() {
    let root = tempfile::tempdir().unwrap();
    let (paths, source, _) = setup(root.path(), false);
    let global = GlobalConfig::default();
    let destination = root.path().join("copy");
    let review = inspect(&paths, &global, "source", "copy", Some(&destination)).unwrap();
    fs::write(source.join("drive_c/game.exe"), b"edit").unwrap();
    let error = execute_reviewed(&paths, &global, &review, &Token::new(), |_| {}).unwrap_err();
    assert_eq!(
        error.to_string(),
        "the reviewed prefix changed; inspect it again before importing"
    );
    assert!(!destination.exists());
    assert!(!paths.environment_config("copy").exists());
}
