mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use common::{Fixture, assert_success, stderr, stdout};

fn heroic_library(f: &Fixture, prefix: &Path) -> std::path::PathBuf {
    let config = f.root.path().join("heroic");
    fs::create_dir_all(config.join("sideload_apps")).unwrap();
    fs::create_dir_all(config.join("GamesConfig")).unwrap();
    let exe = prefix.join("drive_c/Games/Game.exe");
    fs::write(&exe, "game executable").unwrap();
    fs::write(
        config.join("config.json"),
        format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
            prefix.display()
        ),
    )
    .unwrap();
    let entries: Vec<_> = [("a", "Alpha"), ("b", "Beta")]
        .into_iter()
        .map(|(id, title)| {
            fs::write(config.join(format!("GamesConfig/{id}.json")), format!(r#"{{"{id}":{{}}}}"#)).unwrap();
            format!(r#"{{"app_name":"{id}","title":"{title}","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}"#, exe.display())
        })
        .collect();
    fs::write(
        config.join("sideload_apps/library.json"),
        format!(r#"{{"games":[{}]}}"#, entries.join(",")),
    )
    .unwrap();
    config
}

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
