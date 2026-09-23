mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use common::{Fixture, assert_success, heroic_library, stderr, stdout};

fn bulk_heroic(f: &Fixture, config: &Path) -> std::process::Output {
    f.run(&[
        "import",
        "heroic",
        "--config-dir",
        config.to_str().unwrap(),
        "--all",
        "--yes",
        "--runner",
        "wine",
    ])
}

#[test]
fn heroic_bulk_preserves_recorded_ids_and_imports_another_game_with_the_same_paths() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    let configs = f.config_dir().join("environments");
    fs::create_dir_all(&configs).unwrap();
    let original = format!(
        "# keep this comment\nprefix = {:?}\nrunner = \"wine\"\narch = \"win64\"\nheroic_id = \"a\" # stable identity\n\n[targets.play]\nexe = {:?}\nargs = [\"--saved\"]\n",
        prefix.to_str().unwrap(),
        prefix.join("drive_c/Games/Game.exe").to_str().unwrap()
    );
    fs::write(configs.join("alpha.toml"), &original).unwrap();

    let output = bulk_heroic(&f, &heroic);
    assert_success(&output);
    assert!(stdout(&output).contains("Already imported: alpha"));
    assert!(stdout(&output).contains("Imported 1 of 1 games."));
    assert!(!stdout(&output).contains("warn:"));
    assert_eq!(
        fs::read_to_string(configs.join("alpha.toml")).unwrap(),
        original
    );
    let beta = fs::read_to_string(configs.join("beta.toml")).unwrap();
    assert!(beta.contains("heroic_id = \"b\""));
    let repeated = bulk_heroic(&f, &heroic);
    assert_success(&repeated);
    assert!(stdout(&repeated).contains("Already imported: alpha, beta"));
    assert!(stdout(&repeated).contains("No Heroic games are ready to import."));
    assert_eq!(
        fs::read_to_string(configs.join("alpha.toml")).unwrap(),
        original
    );
    assert_eq!(fs::read_to_string(configs.join("beta.toml")).unwrap(), beta);
    assert_eq!(
        fs::read(prefix.join("drive_c/Games/save.dat")).unwrap(),
        b"save data"
    );
}

#[test]
fn heroic_bulk_backfills_a_legacy_environment_only_once_per_invocation() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    let configs = f.config_dir().join("environments");
    fs::create_dir_all(&configs).unwrap();
    let original = format!(
        "# legacy settings\nprefix = {:?}\nrunner = \"wine\"\narch = \"win64\"\n",
        prefix.to_str().unwrap()
    );
    fs::write(configs.join("legacy.toml"), &original).unwrap();
    let output = bulk_heroic(&f, &heroic);
    assert_success(&output);
    assert!(stdout(&output).contains("Already imported: legacy"));
    assert!(stdout(&output).contains("Imported 1 of 1 games."));
    assert!(!stdout(&output).contains("warn:"));
    let expected = format!("{original}heroic_id = \"a\"\n");
    assert_eq!(
        fs::read_to_string(configs.join("legacy.toml")).unwrap(),
        expected
    );
    let beta = fs::read_to_string(configs.join("beta.toml")).unwrap();
    assert!(beta.contains("heroic_id = \"b\""));
    let repeated = bulk_heroic(&f, &heroic);
    assert_success(&repeated);
    assert!(stdout(&repeated).contains("No Heroic games are ready to import."));
    assert_eq!(
        fs::read_to_string(configs.join("legacy.toml")).unwrap(),
        expected
    );
    assert_eq!(fs::read_to_string(configs.join("beta.toml")).unwrap(), beta);
}

#[test]
fn heroic_bulk_reports_a_name_collision_without_retagging_the_existing_game() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    let configs = f.config_dir().join("environments");
    fs::create_dir_all(&configs).unwrap();
    let original = format!(
        "# custom name\nprefix = {:?}\nrunner = \"wine\"\narch = \"win64\"\nheroic_id = \"a\"\n",
        prefix.to_str().unwrap()
    );
    fs::write(configs.join("beta.toml"), &original).unwrap();
    for _ in 0..2 {
        let output = bulk_heroic(&f, &heroic);
        assert_success(&output);
        let text = stdout(&output);
        assert!(text.contains("Already imported: beta"), "{text}");
        assert!(
            text.contains("skip: name `beta` is already taken"),
            "{text}"
        );
        assert!(text.contains("No Heroic games are ready to import."));
        assert_eq!(
            fs::read_to_string(configs.join("beta.toml")).unwrap(),
            original
        );
        assert!(!configs.join("alpha.toml").exists());
    }
}

#[test]
fn heroic_bulk_copy_and_adopt_register_all_games_with_explicit_desktop_entries() {
    for adopt in [false, true] {
        let f = Fixture::new();
        let prefix = source(f.root.path());
        let heroic = heroic_library(&f, &prefix);
        let mut command = f.wineo();
        command.args([
            "-v",
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--all",
            "--yes",
            "--runner",
            "wine",
            "--desktop",
        ]);
        if adopt {
            command.arg("--adopt");
        }
        let output = command.output().unwrap();
        assert_success(&output);
        assert!(stdout(&output).contains("Imported 2 of 2 games."));
        for (name, id) in [("alpha", "a"), ("beta", "b")] {
            let path = f.config_dir().join(format!("environments/{name}.toml"));
            let text = fs::read_to_string(&path).unwrap();
            let config = wineo_core::config::EnvironmentConfig::parse(&path, &text).unwrap();
            assert_eq!(config.heroic_id.as_deref(), Some(id));
            let target = config.targets.get(name).unwrap();
            assert!(target.desktop);
            assert!(target.default);
            assert!(
                f.root
                    .path()
                    .join(format!("data/applications/wineo-{name}-{name}.desktop"))
                    .exists()
            );
            if adopt {
                assert_eq!(config.prefix, prefix.to_str().unwrap());
            } else {
                let copied = f.root.path().join("data/wineo/prefixes").join(name);
                assert_eq!(
                    fs::read(copied.join("drive_c/Games/save.dat")).unwrap(),
                    b"save data"
                );
                assert_eq!(
                    target.exe,
                    copied.join("drive_c/Games/Game.exe").to_str().unwrap()
                );
            }
        }
        assert_eq!(
            fs::read(prefix.join("drive_c/Games/save.dat")).unwrap(),
            b"save data"
        );
        assert_eq!(
            fs::read(prefix.join("drive_c/Games/Game.exe")).unwrap(),
            b"game executable"
        );
    }
}

#[test]
fn heroic_bulk_continues_after_a_destination_failure_and_reports_counts() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    let collision = f.root.path().join("data/wineo/prefixes/beta");
    fs::create_dir_all(&collision).unwrap();
    fs::write(collision.join("keep.txt"), b"unrelated destination").unwrap();
    let output = bulk_heroic(&f, &heroic);
    assert!(!output.status.success());
    let text = stdout(&output);
    assert!(text.contains("Importing alpha... done"), "{text}");
    assert!(text.contains("Importing beta... failed"), "{text}");
    assert!(text.contains("Imported 1 of 2"), "{text}");
    assert!(f.config_dir().join("environments/alpha.toml").exists());
    assert!(!f.config_dir().join("environments/beta.toml").exists());
    assert!(
        !f.root
            .path()
            .join("data/applications/wineo-alpha-alpha.desktop")
            .exists()
    );
    assert_eq!(
        fs::read(collision.join("keep.txt")).unwrap(),
        b"unrelated destination"
    );
    assert_eq!(
        fs::read(prefix.join("drive_c/Games/save.dat")).unwrap(),
        b"save data"
    );
}

#[test]
fn heroic_bulk_yes_skips_missing_components_without_downloading() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    let settings = heroic.join("config.json");
    let text = fs::read_to_string(&settings).unwrap().replace(
        "\"defaultSettings\":{",
        "\"defaultSettings\":{\"autoInstallDxvk\":true,",
    );
    fs::write(settings, text).unwrap();
    let output = bulk_heroic(&f, &heroic);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("skip:"), "{text}");
    assert!(
        text.contains("No Heroic games are ready to import."),
        "{text}"
    );
    assert!(!f.config_dir().join("environments/alpha.toml").exists());
    assert!(!f.config_dir().join("environments/beta.toml").exists());
    assert!(!f.root.path().join("data/wineo/dxvk").exists());
    assert_eq!(
        fs::read(prefix.join("drive_c/Games/save.dat")).unwrap(),
        b"save data"
    );
}

fn source(root: &Path) -> std::path::PathBuf {
    let prefix = root.join("old prefix");
    fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    fs::write(prefix.join("drive_c/Games/save.dat"), "save data").unwrap();
    prefix
}

#[test]
fn generic_import_copies_without_launching_and_rejects_collisions() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let destination = f.root.path().join("copy");
    let args = [
        "import",
        "prefix",
        prefix.to_str().unwrap(),
        "-n",
        "copied",
        "--runner",
        "wine",
        "--dir",
        destination.to_str().unwrap(),
    ];
    assert_success(&f.run(&args));
    assert_eq!(
        fs::read(destination.join("drive_c/Games/save.dat")).unwrap(),
        b"save data"
    );
    assert!(!prefix.join("reg.log").exists());
    assert!(!destination.join("reg.log").exists());
    assert!(!f.run(&args).status.success());
    assert_success(&f.run(&["rm", "copied"]));
    assert!(destination.join("system.reg").exists());
    assert!(prefix.join("system.reg").exists());
}

#[test]
fn generic_adoption_preserves_the_source_and_proton_self_link_is_preserved() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    symlink(".", prefix.join("pfx")).unwrap();
    f.install_proton("GE-Proton11-7");
    assert_success(&f.run(&[
        "import",
        "prefix",
        prefix.to_str().unwrap(),
        "-n",
        "adopted",
        "--runner",
        "GE-Proton-Latest",
        "--adopt",
    ]));
    let config = stdout(&f.run(&["config", "adopted"]));
    assert!(config.contains("runner = \"GE-Proton-Latest\""));
    assert!(config.contains(prefix.to_str().unwrap()));
    assert!(!prefix.join("reg.log").exists());
    assert_success(&f.run(&["rm", "adopted"]));
    let destination = f.root.path().join("copied-proton");
    assert_success(&f.run(&[
        "import",
        "prefix",
        prefix.to_str().unwrap(),
        "-n",
        "proton",
        "--runner",
        "GE-Proton-Latest",
        "--dir",
        destination.to_str().unwrap(),
    ]));
    assert_eq!(
        fs::read_link(destination.join("pfx")).unwrap(),
        Path::new(".")
    );
    assert_success(&f.run(&["rm", "proton", "--delete-prefix"]));
    assert!(!destination.exists());
    assert!(prefix.exists());
}

#[test]
fn import_without_required_flags_needs_a_terminal_and_never_writes() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let output = f.run(&["import", "prefix", prefix.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("terminal"));
    assert!(!f.config_dir().join("environments").exists());
    assert_success(&f.run(&["import", "--help"]));
}

#[test]
fn adopted_wine_and_proton_prefixes_can_be_deleted_without_following_external_links() {
    for proton in [false, true] {
        let f = Fixture::new();
        let mut prefix = source(f.root.path());
        let runner = if proton {
            f.install_proton("GE-Proton11-7");
            let compatdata = f.root.path().join("compatdata");
            fs::create_dir(&compatdata).unwrap();
            fs::rename(&prefix, compatdata.join("pfx")).unwrap();
            prefix = compatdata.join("pfx");
            "GE-Proton11-7"
        } else {
            "wine"
        };
        let external = f.root.path().join("external saves");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("save.dat"), b"external save\0bytes").unwrap();
        symlink(&external, prefix.join("drive_c/external")).unwrap();
        assert_success(&f.run(&[
            "import",
            "prefix",
            prefix.to_str().unwrap(),
            "-n",
            "adopted",
            "--runner",
            runner,
            "--adopt",
        ]));
        let config_path = f.config_dir().join("environments/adopted.toml");
        let config = fs::read_to_string(&config_path).unwrap();
        let configured_root = if proton {
            prefix.parent().unwrap()
        } else {
            &prefix
        };
        assert!(
            config.contains(configured_root.to_str().unwrap()),
            "{config}"
        );
        assert_success(&f.run(&["rm", "adopted", "--delete-prefix"]));
        assert!(!configured_root.exists());
        assert!(!config_path.exists());
        assert_eq!(
            fs::read(external.join("save.dat")).unwrap(),
            b"external save\0bytes"
        );
    }
}

#[test]
fn adopted_prefix_deletion_refuses_registered_aliases_and_retains_failed_removals() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    assert_success(&f.run(&[
        "import",
        "prefix",
        prefix.to_str().unwrap(),
        "-n",
        "adopted",
        "--runner",
        "wine",
        "--adopt",
    ]));
    let configs = f.config_dir().join("environments");
    let original = fs::read_to_string(configs.join("adopted.toml")).unwrap();
    let alias = f.root.path().join("prefix alias");
    symlink(&prefix, &alias).unwrap();
    fs::write(
        configs.join("other.toml"),
        original.replace(prefix.to_str().unwrap(), alias.to_str().unwrap()),
    )
    .unwrap();
    let refused = f.run(&["rm", "adopted", "--delete-prefix"]);
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("also used by other"));
    assert_eq!(
        fs::read_to_string(configs.join("adopted.toml")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(prefix.join("drive_c/Games/save.dat")).unwrap(),
        b"save data"
    );
    assert_success(&f.run(&["rm", "other"]));
    assert!(prefix.exists());

    let games = prefix.join("drive_c/Games");
    fs::set_permissions(&games, fs::Permissions::from_mode(0o555)).unwrap();
    let failed = f.run(&["rm", "adopted", "--delete-prefix"]);
    fs::set_permissions(&games, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!failed.status.success());
    assert!(stderr(&failed).contains("adopted was kept and the prefix may be partly deleted"));
    assert_eq!(
        fs::read_to_string(configs.join("adopted.toml")).unwrap(),
        original
    );
    assert_eq!(fs::read(games.join("save.dat")).unwrap(), b"save data");
    assert_success(&f.run(&["rm", "adopted"]));
    assert!(prefix.exists());
}
