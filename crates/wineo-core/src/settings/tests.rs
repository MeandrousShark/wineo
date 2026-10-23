use crate::test_script::write_script;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

use super::*;

fn setup() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    (root, paths)
}

#[test]
fn setting_a_runner_waits_for_its_build_lock_before_writing() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    update_with_writer(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("runner", "build")],
        &[],
        |path, contents| {
            assert!(
                build_lock::try_exclusive(&paths, build_lock::Kind::Runner, "build")?.is_none()
            );
            assert_eq!(fs::read_to_string(path).unwrap(), NOTEPAD);
            write_atomic(path, contents)
        },
    )
    .unwrap();
    assert!(
        build_lock::try_exclusive(&paths, build_lock::Kind::Runner, "build")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        Environment::load(&paths, "notepad")
            .unwrap()
            .wine()
            .unwrap()
            .runner,
        "build"
    );
}

#[test]
fn a_config_write_keeps_the_selected_build_locked() {
    let (_root, paths) = setup();
    let path = paths.global_config();
    write_with_build_locks(
        &paths,
        &GlobalConfig::default(),
        Some("build"),
        Some("2.0"),
        Some("3.0"),
        || {
            for (kind, name) in [
                (build_lock::Kind::Runner, "build"),
                (build_lock::Kind::Dxvk, "2.0"),
                (build_lock::Kind::Vkd3d, "3.0"),
            ] {
                assert!(build_lock::try_exclusive(&paths, kind, name)?.is_none());
            }
            write_atomic(&path, "[defaults]\nrunner = 'build'\n")
        },
    )
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

const NOTEPAD: &str =
    "# hand written\nprefix = \"~/prefixes/notepad\"\nrunner = \"wine\"\narch = \"win64\"\n";

fn write_env(paths: &Paths, text: &str) {
    let path = paths.environment_config("notepad");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn sets_values_and_keeps_comments() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let missing = update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[
            ("targets.notepad.exe", "notepad"),
            ("env.WINEDEBUG", "-all"),
        ],
        &["env.NOPE"],
    )
    .unwrap();
    assert_eq!(missing, ["env.NOPE"]);

    let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
    assert!(text.starts_with("# hand written\n"), "{text}");
    let config = EnvironmentConfig::parse(Path::new("/x"), &text).unwrap();
    assert_eq!(config.targets["notepad"].exe, "notepad");
    assert_eq!(config.env["WINEDEBUG"], "-all");
}

#[test]
fn invalid_results_are_not_written() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let err = update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("targets.a.default", "true")],
        &[],
    )
    .unwrap_err();
    assert!(matches!(err, Error::ConfigSyntax { .. }), "{err}");
    assert_eq!(
        fs::read_to_string(paths.environment_config("notepad")).unwrap(),
        NOTEPAD
    );
}

#[test]
fn prefix_paths_are_stored_relative_to_home() {
    let (root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let elsewhere = root.path().join("Games/notepad");
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("prefix", elsewhere.to_str().unwrap())],
        &[],
    )
    .unwrap();
    let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
    assert!(text.contains("prefix = \"~/Games/notepad\""), "{text}");
}

#[test]
fn global_config_is_created_on_first_set() {
    let (_root, paths) = setup();
    update(&paths, ConfigFile::Global, &[("stop_timeout", "30")], &[]).unwrap();
    let config = GlobalConfig::load(&paths).unwrap();
    assert_eq!(config.stop_timeout, 30);
}

// Without the lock, a removal between the read and the write would have the
// write bring the config back.
#[test]
fn a_change_waits_for_a_removal_and_does_not_bring_the_config_back() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let config = paths.environment_config("notepad");
    let held = Locks::new(&paths).config(&config).unwrap();
    let change = {
        let paths = paths.clone();
        std::thread::spawn(move || {
            update(
                &paths,
                ConfigFile::Environment("notepad"),
                &[("runner", "wine")],
                &[],
            )
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::remove_file(&config).unwrap();
    drop(held);
    assert!(matches!(
        change.join().unwrap(),
        Err(Error::UnknownEnvironment(_))
    ));
    assert!(!config.exists());
}

fn modified(path: &Path) -> std::time::SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

#[test]
fn an_edit_stamps_a_legacy_file_and_keeps_its_formatting() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("runner", "wine-tkg")],
        &[],
    )
    .unwrap();
    let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
    assert_eq!(
        text,
        "# hand written\nprefix = \"~/prefixes/notepad\"\nrunner = \"wine-tkg\"\narch = \"win64\"\nschema_version = 1\n"
    );
    assert_eq!(
        EnvironmentConfig::parse(Path::new("/x"), &text)
            .unwrap()
            .schema_version,
        1
    );
}

#[test]
fn a_no_op_edit_leaves_a_legacy_file_alone() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let path = paths.environment_config("notepad");
    let before = modified(&path);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let missing = update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("runner", "wine")],
        &["env.NOPE"],
    )
    .unwrap();
    assert_eq!(missing, ["env.NOPE"]);
    assert_eq!(fs::read_to_string(&path).unwrap(), NOTEPAD);
    assert_eq!(modified(&path), before);
}

#[test]
fn a_no_op_edit_leaves_a_versioned_wine_file_byte_identical() {
    let (_root, paths) = setup();
    let text = format!(
        "{NOTEPAD}schema_version = 1\ngamemode = true # mine\n\n[targets.notepad]\nexe = \"notepad\"\n"
    );
    write_env(&paths, &text);
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("gamemode", "true"), ("runner", "wine")],
        &[],
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(paths.environment_config("notepad")).unwrap(),
        text
    );
}

#[test]
fn a_title_is_set_trimmed_and_unset_on_wine_and_native() {
    let (_root, paths) = setup();
    let native = "# hand written\nkind = \"native\" # mine\n";
    for original in [NOTEPAD, native] {
        write_env(&paths, original);
        let path = paths.environment_config("notepad");
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("title", "  Notepad: Classic \u{1f4dd}  ")],
            &[],
        )
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            format!("{original}title = \"Notepad: Classic \u{1f4dd}\"\nschema_version = 1\n")
        );
        let config = EnvironmentConfig::parse(&path, &text).unwrap();
        assert_eq!(config.title.as_deref(), Some("Notepad: Classic \u{1f4dd}"));

        let missing = update(&paths, ConfigFile::Environment("notepad"), &[], &["title"]).unwrap();
        assert!(missing.is_empty());
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, format!("{original}schema_version = 1\n"));
        assert_eq!(EnvironmentConfig::parse(&path, &text).unwrap().title, None);
    }
}

#[test]
fn a_bad_title_is_refused_without_writing() {
    let (_root, paths) = setup();
    let native = "kind = \"native\"\ntitle = \"Old\" # mine\n";
    for original in [NOTEPAD, native] {
        write_env(&paths, original);
        let long = "\u{e9}".repeat(121);
        for (raw, message) in [
            ("", "Invalid title: can't be empty."),
            ("   ", "Invalid title: can't be empty."),
            ("a\nb", "Invalid title: can't contain control characters."),
            (
                long.as_str(),
                "Invalid title: can't be longer than 120 characters.",
            ),
        ] {
            let err = update(
                &paths,
                ConfigFile::Environment("notepad"),
                &[("title", raw)],
                &[],
            )
            .unwrap_err();
            assert_eq!(err.to_string(), message, "{raw:?}");
            assert_eq!(
                fs::read_to_string(paths.environment_config("notepad")).unwrap(),
                original
            );
        }
    }
}

#[test]
fn a_legacy_file_without_a_title_is_left_alone() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let path = paths.environment_config("notepad");
    let before = modified(&path);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let environment = crate::environment::Environment::load(&paths, "notepad").unwrap();
    assert_eq!(environment.config.title, None);
    let missing = update(&paths, ConfigFile::Environment("notepad"), &[], &["title"]).unwrap();
    assert_eq!(missing, ["title"]);
    assert_eq!(fs::read_to_string(&path).unwrap(), NOTEPAD);
    assert_eq!(modified(&path), before);
}

#[test]
fn the_same_title_again_writes_nothing() {
    let (_root, paths) = setup();
    let text = format!("{NOTEPAD}title  =  'Notepad' # mine\n");
    write_env(&paths, &text);
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("title", " Notepad ")],
        &[],
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(paths.environment_config("notepad")).unwrap(),
        text
    );
}

#[test]
fn a_steam_title_is_still_set_as_given() {
    let (_root, paths) = setup();
    let original =
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1086940\"\ntitle = \"Old\"\n";
    write_env(&paths, original);
    let path = paths.environment_config("notepad");
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("title", "  New  ")],
        &[],
    )
    .unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let config = EnvironmentConfig::parse(&path, &text).unwrap();
    assert_eq!(config.proxied().unwrap().title.as_deref(), Some("  New  "));
    update(&paths, ConfigFile::Environment("notepad"), &[], &["title"]).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(EnvironmentConfig::parse(&path, &text).unwrap().title, None);
}

#[test]
fn an_edit_never_adds_kind_to_a_wine_file() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("gamemode", "true")],
        &[],
    )
    .unwrap();
    let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
    assert!(!text.contains("kind"), "{text}");
}

#[test]
fn a_wine_only_key_is_not_written_to_a_native_file() {
    let (_root, paths) = setup();
    let text = "kind = \"native\"\n";
    write_env(&paths, text);
    let err = update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("runner", "wine")],
        &[],
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .ends_with("native environment notepad can't set runner"),
        "{err}"
    );
    assert_eq!(
        fs::read_to_string(paths.environment_config("notepad")).unwrap(),
        text
    );
}

#[test]
fn native_wayland_is_refused_at_both_levels_without_writing() {
    let (_root, paths) = setup();
    let text = "kind = 'native'\n[targets.game]\nexe = 'game'\n";
    write_env(&paths, text);
    for key in ["wayland", "targets.game.wayland"] {
        let err = update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[(key, "true")],
            &[],
        )
        .unwrap_err();
        assert!(
            err.to_string().contains(&format!("can't set {key}")),
            "{err}"
        );
        assert_eq!(
            fs::read_to_string(paths.environment_config("notepad")).unwrap(),
            text
        );
    }
}

#[test]
fn gamescope_false_becomes_a_table_and_an_unset_field_keeps_it_on() {
    let (_root, paths) = setup();
    write_env(
        &paths,
        &format!(
            "{NOTEPAD}gamescope = false # keep\n[targets.game]\nexe = 'game'\ngamescope = false # keep target\n"
        ),
    );
    for prefix in ["gamescope", "targets.game.gamescope"] {
        let field = format!("{prefix}.width");
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[(field.as_str(), "1920")],
            &[],
        )
        .unwrap();
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[],
            &[field.as_str()],
        )
        .unwrap();
    }
    let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
    assert!(text.contains("[gamescope]"), "{text}");
    assert!(text.contains("[targets.game.gamescope]"), "{text}");
    assert!(text.contains("# keep"), "{text}");
    assert!(text.contains("# keep target"), "{text}");
    let config = EnvironmentConfig::parse(Path::new("/x"), &text).unwrap();
    assert!(config.gamescope.unwrap().on().is_some());
    assert!(
        config.targets["game"]
            .gamescope
            .as_ref()
            .unwrap()
            .on()
            .is_some()
    );
}

#[test]
fn a_versioned_file_is_not_stamped_twice() {
    let (_root, paths) = setup();
    let versioned = format!("{NOTEPAD}schema_version = 1\n");
    write_env(&paths, &versioned);
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("gamemode", "true")],
        &[],
    )
    .unwrap();
    let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
    assert_eq!(text, format!("{versioned}gamemode = true\n"));
}

#[test]
fn a_new_global_config_starts_with_the_version() {
    let (_root, paths) = setup();
    update(&paths, ConfigFile::Global, &[("stop_timeout", "30")], &[]).unwrap();
    assert_eq!(
        fs::read_to_string(paths.global_config()).unwrap(),
        "schema_version = 1\nstop_timeout = 30\n"
    );
}

#[test]
fn a_no_op_edit_creates_no_global_config() {
    let (_root, paths) = setup();
    let missing = update(&paths, ConfigFile::Global, &[], &["gamemode"]).unwrap();
    assert_eq!(missing, ["gamemode"]);
    assert!(!paths.global_config().exists());
}

#[test]
fn steam_no_op_and_invalid_edits_never_write_the_global_config() {
    let (_root, paths) = setup();
    let original = "# mine\nsteam  =  'flatpak' # preferred\n";
    fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
    fs::write(paths.global_config(), original).unwrap();
    update_with_writer(
        &paths,
        ConfigFile::Global,
        &[("steam", "flatpak")],
        &[],
        |_, _| panic!("a no-op must not write"),
    )
    .unwrap();
    assert!(
        update_with_writer(
            &paths,
            ConfigFile::Global,
            &[("stop_timeout", "3"), ("steam", "snap")],
            &[],
            |_, _| panic!("an invalid edit must not write"),
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(paths.global_config()).unwrap(), original);

    fs::remove_file(paths.global_config()).unwrap();
    let missing = update_with_writer(&paths, ConfigFile::Global, &[], &["steam"], |_, _| {
        panic!("unsetting an absent key must not write")
    })
    .unwrap();
    assert_eq!(missing, ["steam"]);
    assert!(!paths.global_config().exists());
}

#[test]
fn the_external_runners_answer_keeps_the_rest_of_the_file() {
    let (_root, paths) = setup();
    assert_eq!(GlobalConfig::load(&paths).unwrap().external_runners, None);
    let original = "# mine
schema_version = 1
stop_timeout = 3 # short
";
    fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
    fs::write(paths.global_config(), original).unwrap();

    set_external_runners(&paths, true).unwrap();
    let text = fs::read_to_string(paths.global_config()).unwrap();
    assert_eq!(text, format!("{original}external_runners = true\n"));
    assert_eq!(
        GlobalConfig::load(&paths).unwrap().external_runners,
        Some(true)
    );

    set_external_runners(&paths, false).unwrap();
    assert_eq!(
        GlobalConfig::load(&paths).unwrap().external_runners,
        Some(false)
    );
    let missing = update(&paths, ConfigFile::Global, &[], &["external_runners"]).unwrap();
    assert!(missing.is_empty());
    assert_eq!(fs::read_to_string(paths.global_config()).unwrap(), original);
}

fn write_global(paths: &Paths, text: &str) {
    fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
    fs::write(paths.global_config(), text).unwrap();
}

#[test]
fn the_gog_settings_keep_comments_and_formatting() {
    let (_root, paths) = setup();
    let original =
        "schema_version = 1\n# mine\n[defaults]\nrunner = \"wine\" # keep\n\n[env]\nA = \"1\"\n";
    write_global(&paths, original);
    update(
        &paths,
        ConfigFile::Global,
        &[
            ("gog.language", "de-AT"),
            ("defaults.games_root", "~/Library/Games"),
        ],
        &[],
    )
    .unwrap();
    let text = fs::read_to_string(paths.global_config()).unwrap();
    assert_eq!(
        text,
        "schema_version = 1\n# mine\n[defaults]\nrunner = \"wine\" # keep\ngames_root = \"~/Library/Games\"\n\n[env]\nA = \"1\"\n\n[gog]\nlanguage = \"de-AT\"\n"
    );
    let config = GlobalConfig::load(&paths).unwrap();
    assert_eq!(config.gog_language(), "de-AT");
    assert_eq!(
        config.games_root(&paths).unwrap(),
        paths.expand("~/Library/Games")
    );
}

#[test]
fn games_root_is_stored_absolute_like_prefix_root() {
    let (_root, paths) = setup();
    update(
        &paths,
        ConfigFile::Global,
        &[("defaults.games_root", "relative/games")],
        &[],
    )
    .unwrap();
    let stored = GlobalConfig::load(&paths).unwrap().defaults.games_root;
    let absolute = paths.absolute("relative/games").unwrap();
    assert_eq!(stored, Some(paths.contract(&absolute).unwrap()));
    assert!(
        Path::new(&paths.expand(stored.as_deref().unwrap())).is_absolute(),
        "{stored:?}"
    );
}

#[test]
fn a_bad_gog_language_is_refused_without_writing() {
    let (_root, paths) = setup();
    let original = "schema_version = 1\n[defaults]\nrunner = \"wine\"\n";
    write_global(&paths, original);
    let path = paths.global_config();
    let before = modified(&path);
    std::thread::sleep(std::time::Duration::from_millis(20));
    for bad in ["english", "en_US", "", "en-"] {
        let err = update(&paths, ConfigFile::Global, &[("gog.language", bad)], &[]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid value for `gog.language`: use a locale tag like en-US",
            "{bad:?}"
        );
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert_eq!(modified(&path), before);
}

#[test]
fn unsetting_the_gog_settings_drops_emptied_tables() {
    let (_root, paths) = setup();
    write_global(
        &paths,
        "schema_version = 1\n[defaults]\ngames_root = \"/g\"\n\n[gog]\nlanguage = \"fr\"\n",
    );
    let missing = update(
        &paths,
        ConfigFile::Global,
        &[],
        &["gog.language", "defaults.games_root", "gog.language"],
    )
    .unwrap();
    assert_eq!(missing, ["gog.language"]);
    assert_eq!(
        fs::read_to_string(paths.global_config()).unwrap(),
        "schema_version = 1\n"
    );
}

#[test]
fn a_no_op_gog_edit_leaves_a_legacy_global_file_alone() {
    let (_root, paths) = setup();
    let original = "# mine\n[defaults]\nrunner = \"wine\"\n";
    write_global(&paths, original);
    let path = paths.global_config();
    let before = modified(&path);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let missing = update(
        &paths,
        ConfigFile::Global,
        &[],
        &["gog.language", "defaults.games_root"],
    )
    .unwrap();
    assert_eq!(missing, ["gog.language", "defaults.games_root"]);
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert_eq!(modified(&path), before);

    write_global(&paths, "[gog]\nlanguage = \"de\"\n");
    let before = modified(&path);
    std::thread::sleep(std::time::Duration::from_millis(20));
    update(&paths, ConfigFile::Global, &[("gog.language", "de")], &[]).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "[gog]\nlanguage = \"de\"\n"
    );
    assert_eq!(modified(&path), before);
}

#[test]
fn setting_an_unchanged_value_in_other_quotes_writes_nothing() {
    let (_root, paths) = setup();
    let path = paths.global_config();
    for (text, key, value) in [
        ("[gog]\nlanguage = 'de'\n", "gog.language", "de"),
        (
            "[defaults]\ngames_root = '/mnt/games'\n",
            "defaults.games_root",
            "/mnt/games",
        ),
        (
            "[defaults]\ngames_root = '~/Games'\n",
            "defaults.games_root",
            "~/Games",
        ),
        ("[defaults]\nrunner = 'wine'\n", "defaults.runner", "wine"),
        ("stop_timeout =   30 # mine\n", "stop_timeout", "30"),
    ] {
        write_global(&paths, text);
        let before = modified(&path);
        std::thread::sleep(std::time::Duration::from_millis(20));
        update(&paths, ConfigFile::Global, &[(key, value)], &[]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "{key}");
        assert_eq!(modified(&path), before, "{key}");
    }
}

#[test]
fn a_legacy_global_config_is_stamped_on_edit() {
    let (_root, paths) = setup();
    let original = "# mine\n[defaults]\nrunner = \"wine\"\n";
    fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
    fs::write(paths.global_config(), original).unwrap();
    update(
        &paths,
        ConfigFile::Global,
        &[("defaults.arch", "win32")],
        &[],
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(paths.global_config()).unwrap(),
        "schema_version = 1\n# mine\n[defaults]\nrunner = \"wine\"\narch = \"win32\"\n"
    );
}

#[test]
fn the_version_is_not_a_setting() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    for (set, unset) in [
        (vec![("schema_version", "2")], vec![]),
        (vec![], vec!["schema_version"]),
    ] {
        assert!(matches!(
            update(&paths, ConfigFile::Environment("notepad"), &set, &unset),
            Err(Error::Edit(edit::EditError::UnknownKey(_)))
        ));
    }
    assert_eq!(
        fs::read_to_string(paths.environment_config("notepad")).unwrap(),
        NOTEPAD
    );
}

#[test]
fn unsupported_versions_are_never_edited() {
    let (_root, paths) = setup();
    for version in ["0", "2", "\"1\""] {
        let text = format!("schema_version = {version}\n{NOTEPAD}");
        write_env(&paths, &text);
        let path = paths.environment_config("notepad");
        let before = modified(&path);
        let err = update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("runner", "wine-tkg")],
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, Error::SchemaVersion { .. }), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert_eq!(modified(&path), before);

        fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
        fs::write(
            paths.global_config(),
            format!("schema_version = {version}\n"),
        )
        .unwrap();
        let err = update(&paths, ConfigFile::Global, &[("stop_timeout", "3")], &[]).unwrap_err();
        assert!(matches!(err, Error::SchemaVersion { .. }), "{err}");
        assert_eq!(
            fs::read_to_string(paths.global_config()).unwrap(),
            format!("schema_version = {version}\n")
        );
    }
}

#[test]
fn unknown_environments_are_errors() {
    let (_root, paths) = setup();
    assert!(matches!(
        update(
            &paths,
            ConfigFile::Environment("nope"),
            &[("runner", "wine")],
            &[]
        ),
        Err(Error::UnknownEnvironment(_))
    ));
}

#[test]
fn unsetting_a_target_removes_its_art() {
    let (_root, paths) = setup();
    write_env(
        &paths,
        &format!("{NOTEPAD}[targets.a]\nexe = \"a\"\n[targets.b]\nexe = \"b\"\n"),
    );
    let art = paths.art_dir("notepad");
    for target in ["a", "b"] {
        fs::create_dir_all(art.join(target)).unwrap();
        fs::write(art.join(target).join("icon.png"), b"x").unwrap();
    }
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("targets.b.exe", "b2")],
        &["targets.a.cwd"],
    )
    .unwrap();
    assert!(art.join("a").exists() && art.join("b").exists());
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[],
        &["targets.a"],
    )
    .unwrap();
    assert!(!art.join("a").exists());
    assert!(art.join("b/icon.png").exists());
}

#[test]
fn an_invalid_art_toml_stops_a_target_removal_first() {
    let (_root, paths) = setup();
    let text = format!("{NOTEPAD}[targets.a]\nexe = \"a\"\n");
    write_env(&paths, &text);
    let art = paths.art_dir("notepad");
    fs::create_dir_all(art.join("a")).unwrap();
    fs::write(art.join("art.toml"), "schema_version = 1\ncolor = 1\n").unwrap();
    let err = update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[],
        &["targets.a"],
    )
    .unwrap_err();
    assert!(matches!(err, Error::ConfigUnknownKey { .. }), "{err}");
    assert_eq!(
        fs::read_to_string(paths.environment_config("notepad")).unwrap(),
        text
    );
    assert!(art.join("a").exists());

    // Nothing to clean up means art.toml is not read at all.
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("targets.a.cwd", "C:\\")],
        &[],
    )
    .unwrap();
}

fn executable(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_script(path, "#!/bin/sh\n");
}

fn fake_wine(paths: &Paths, name: &str) {
    let root = GlobalConfig::default().runner_root(paths).unwrap();
    for program in ["wine", "wineserver"] {
        executable(&root.join(name).join("bin").join(program));
    }
}

fn fake_proton(paths: &Paths, name: &str) {
    let dir = GlobalConfig::default()
        .runner_root(paths)
        .unwrap()
        .join(name);
    executable(&dir.join("proton"));
    for program in ["wine", "wineserver"] {
        executable(&dir.join("files/bin").join(program));
    }
    fs::write(dir.join("version"), format!("1 {name}\n")).unwrap();
}

#[test]
fn apply_refuses_to_change_the_kind() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let err = apply(&paths, ConfigFile::Environment("notepad"), &[], &["kind"]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "notepad's kind can't be changed, make a new environment instead"
    );
    assert_eq!(
        fs::read_to_string(paths.environment_config("notepad")).unwrap(),
        NOTEPAD
    );
}

#[test]
fn apply_refuses_while_the_environment_is_in_use() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let prefix = Environment::load(&paths, "notepad")
        .unwrap()
        .prefix(&paths)
        .unwrap();
    let _running = Locks::new(&paths)
        .acquire(Operation::Launch, "notepad", &[&prefix])
        .unwrap();
    let err = apply(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("runner", "other")],
        &[],
    )
    .unwrap_err();
    assert!(
        matches!(err.downcast_ref::<Error>(), Some(Error::Busy { .. })),
        "{err}"
    );
    assert_eq!(
        fs::read_to_string(paths.environment_config("notepad")).unwrap(),
        NOTEPAD
    );
}

#[test]
fn apply_leaves_a_runner_kind_mismatch_to_the_caller_to_word() {
    let (_root, paths) = setup();
    fake_wine(&paths, "wine-tkg");
    fake_proton(&paths, "GE-Proton9-20");
    let text = NOTEPAD.replace("runner = \"wine\"", "runner = \"wine-tkg\"");
    write_env(&paths, &text);
    let err = apply(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("runner", "GE-Proton9-20")],
        &[],
    )
    .unwrap_err();
    match err.downcast_ref::<CheckError>() {
        Some(CheckError::RunnerKindMismatch {
            current,
            changed,
            kind,
        }) => {
            assert_eq!(current, "wine-tkg");
            assert_eq!(changed, "GE-Proton9-20");
            assert_eq!(kind, "proton");
        }
        None => panic!("{err:#}"),
    }
    assert_eq!(
        fs::read_to_string(paths.environment_config("notepad")).unwrap(),
        text
    );
}

#[test]
fn apply_reports_what_it_did() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let applied = apply(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("env.WINEDEBUG", "-all")],
        &["env.NOPE"],
    )
    .unwrap();
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    assert!(applied.components.is_empty());
    assert_eq!(applied.not_set, ["env.NOPE"]);
    assert!(applied.desktop.is_empty());
    assert_eq!(applied.changed_runner, None);
    assert!(!applied.running);
    let config = Environment::load(&paths, "notepad").unwrap().config;
    assert_eq!(config.env["WINEDEBUG"], "-all");
}

#[test]
fn apply_returns_desktop_changes() {
    let (root, paths) = setup();
    let prefix = root.path().join("prefix");
    fs::create_dir_all(prefix.join("dosdevices")).unwrap();
    fs::create_dir_all(prefix.join("drive_c/Hades")).unwrap();
    fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    std::os::unix::fs::symlink("../drive_c", prefix.join("dosdevices/c:")).unwrap();
    fs::write(prefix.join("drive_c/Hades/Hades.exe"), b"").unwrap();
    // A runner that doesn't resolve, so nothing starts wine for an icon.
    write_env(
        &paths,
        &format!(
            "prefix = \"{}\"\nrunner = \"missing\"\narch = \"win64\"\n\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\n",
            prefix.display()
        ),
    );
    let applied = apply(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("targets.hades.desktop", "true")],
        &[],
    )
    .unwrap();
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    assert_eq!(
        applied.desktop,
        [desktop::Change::Wrote {
            target: "hades".to_owned(),
            icon: false,
        }]
    );
    assert_eq!(desktop::list(&paths).unwrap().len(), 1);
}

#[test]
fn apply_keeps_a_failed_write_in_failed() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let applied = apply(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("targets.a.default", "true")],
        &["env.NOPE"],
    )
    .unwrap();
    assert!(
        matches!(
            applied
                .failed
                .as_ref()
                .and_then(|err| err.downcast_ref::<Error>()),
            Some(Error::ConfigSyntax { .. })
        ),
        "{:?}",
        applied.failed
    );
    assert!(applied.not_set.is_empty());
    assert_eq!(applied.changed_runner, None);
}

mod gog_id {
    use super::*;
    use crate::gog::folder::{
        Committed, FolderLock, FolderState, Journal, JournalKind, Platform, Target, Track,
    };

    const NEVER: &dyn Fn() -> bool = &|| false;
    const RIMWORLD: &str = "# mine\nkind = \"native\"\ngog_id = \"1094900565\"\n";

    fn game(folder: &Path, journal: bool) -> FolderState {
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
                operation: "op".to_owned(),
                kind: JournalKind::Update,
                target,
                packages: Vec::new(),
                attempted: Vec::new(),
            }),
            ..FolderState::new(folder.to_owned(), Some("rimworld".to_owned()))
        }
    }

    fn setup(state: Option<bool>) -> (tempfile::TempDir, Paths, PathBuf) {
        let (root, paths) = super::setup();
        let config = paths.environment_config("rimworld");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, RIMWORLD).unwrap();
        let folder = root.path().join("Games/RimWorld");
        if let Some(journal) = state {
            FolderLock::exclusive(&paths, &folder, NEVER)
                .unwrap()
                .write(&game(&folder, journal))
                .unwrap();
        }
        (root, paths, folder)
    }

    fn owner(paths: &Paths, folder: &Path) -> Option<String> {
        FolderLock::shared(paths, folder, NEVER)
            .unwrap()
            .read()
            .unwrap()
            .unwrap()
            .owner
    }

    fn text(paths: &Paths) -> String {
        fs::read_to_string(paths.environment_config("rimworld")).unwrap()
    }

    #[test]
    fn setting_gog_id_is_refused() {
        let (_root, paths, _) = setup(None);
        let error = apply(
            &paths,
            ConfigFile::Environment("rimworld"),
            &[("gog_id", "1")],
            &[],
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "gog_id is set by wineo import gog.");
        assert_eq!(text(&paths), RIMWORLD);
        let mut doc: DocumentMut = RIMWORLD.parse().unwrap();
        assert_eq!(
            edit::set(&mut doc, Scope::Environment, "gog_id", "1")
                .unwrap_err()
                .to_string(),
            "gog_id is set by wineo import gog."
        );
    }

    #[test]
    fn unsetting_gog_id_releases_the_folder() {
        let (_root, paths, folder) = setup(Some(false));
        let applied = apply(
            &paths,
            ConfigFile::Environment("rimworld"),
            &[],
            &["gog_id"],
        )
        .unwrap();
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        assert_eq!(owner(&paths, &folder), None);
        assert_eq!(
            text(&paths),
            "# mine\nkind = \"native\"\nschema_version = 1\n"
        );
    }

    #[test]
    fn unsetting_gog_id_mid_update_is_refused() {
        let (_root, paths, folder) = setup(Some(true));
        let error = apply(
            &paths,
            ConfigFile::Environment("rimworld"),
            &[],
            &["gog_id"],
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "rimworld is mid-update. Finish it with wineo update rimworld."
        );
        assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
        assert_eq!(text(&paths), RIMWORLD);
    }

    #[test]
    fn unsetting_gog_id_during_a_galaxy_install_names_the_install_command() {
        let (_root, paths, folder) = setup(Some(true));
        let mut state = game(&folder, true);
        let journal = state.journal.as_mut().unwrap();
        journal.kind = JournalKind::Install;
        journal.target.track = Track::Galaxy;
        FolderLock::exclusive(&paths, &folder, NEVER)
            .unwrap()
            .write(&state)
            .unwrap();
        let error = apply(
            &paths,
            ConfigFile::Environment("rimworld"),
            &[],
            &["gog_id"],
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "rimworld's GOG install didn't finish. Finish it with wineo import gog --game 1094900565."
        );
        assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
        assert_eq!(text(&paths), RIMWORLD);
    }

    #[test]
    fn unsetting_gog_id_with_a_lost_state_only_edits_the_config() {
        let (_root, paths, _) = setup(None);
        let applied = apply(
            &paths,
            ConfigFile::Environment("rimworld"),
            &[],
            &["gog_id"],
        )
        .unwrap();
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        assert!(!text(&paths).contains("gog_id"));
        assert!(gog::folder::list(&paths).unwrap().is_empty());
    }

    #[test]
    fn a_typo_beside_the_unset_leaves_the_owner() {
        let (_root, paths, folder) = setup(Some(false));
        let result = apply(
            &paths,
            ConfigFile::Environment("rimworld"),
            &[("runner", "nope")],
            &["gog_id"],
        );
        let error = result.unwrap_err();
        assert_eq!(
            error.to_string(),
            "native environment rimworld can't set runner"
        );
        assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
        assert_eq!(text(&paths), RIMWORLD);
    }

    #[test]
    fn unsetting_gog_id_takes_the_operation_lock_first() {
        let (_root, paths, folder) = setup(Some(false));
        let _running = Locks::new(&paths)
            .acquire(Operation::Launch, "rimworld", &[])
            .unwrap();
        let error = apply(
            &paths,
            ConfigFile::Environment("rimworld"),
            &[],
            &["gog_id"],
        )
        .unwrap_err();
        assert!(
            matches!(error.downcast_ref::<Error>(), Some(Error::Busy { .. })),
            "{error}"
        );
        assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
        assert_eq!(text(&paths), RIMWORLD);
    }

    #[test]
    fn a_crash_between_release_and_the_config_write_leaves_an_ownerless_folder() {
        let (_root, paths, folder) = setup(Some(false));
        gog::folder::set_failpoint(Some("write.after"));
        assert!(
            apply(
                &paths,
                ConfigFile::Environment("rimworld"),
                &[],
                &["gog_id"],
            )
            .is_err()
        );
        // The rule for an environment with gog_id and an ownerless
        // folder covers this: the next claim takes it back.
        assert_eq!(owner(&paths, &folder), None);
        assert_eq!(text(&paths), RIMWORLD);
    }

    #[test]
    fn a_portable_unset_never_touches_host_state() {
        let (root, paths, folder) = setup(Some(false));
        let home = root.path().to_owned();
        let portable = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            Some(&root.path().join("usb")),
        )
        .unwrap();
        let config = portable.environment_config("rimworld");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, RIMWORLD).unwrap();
        let applied = apply(
            &portable,
            ConfigFile::Environment("rimworld"),
            &[],
            &["gog_id"],
        )
        .unwrap();
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        assert!(!fs::read_to_string(&config).unwrap().contains("gog_id"));
        assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
        assert_eq!(text(&paths), RIMWORLD);
    }

    #[test]
    fn an_unreadable_state_only_blocks_an_environment_tracking_a_game() {
        let (_root, paths, folder) = setup(Some(false));
        let dir = paths.gog_folders_dir();
        fs::write(dir.join("bad.toml"), "version = 9\nfolder = '/x'\n").unwrap();
        let error = apply(
            &paths,
            ConfigFile::Environment("rimworld"),
            &[],
            &["gog_id"],
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "rimworld's GOG state can't be checked: {} isn't a valid GOG folder state (unsupported GOG folder state version 9).",
                dir.join("bad.toml").display()
            )
        );
        assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));

        write_env(&paths, NOTEPAD);
        let applied = apply(&paths, ConfigFile::Environment("notepad"), &[], &["gog_id"]).unwrap();
        assert!(applied.failed.is_none(), "{:?}", applied.failed);
        assert_eq!(applied.not_set, ["gog_id"]);
    }
}

#[test]
fn apply_reports_a_global_config_that_fell_back_to_defaults() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
    fs::write(paths.global_config(), "stop_timeout = [\n").unwrap();
    let applied = apply(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("gamemode", "true")],
        &[],
    )
    .unwrap();
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    // The desktop step reuses the config `apply` loaded, so it says so once.
    assert_eq!(applied.warnings.len(), 1, "{:?}", applied.warnings);
    assert!(
        applied.warnings[0].starts_with("using default settings: "),
        "{:?}",
        applied.warnings
    );
}

#[test]
fn apply_refuses_a_global_config_with_an_unknown_key() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
    fs::write(paths.global_config(), "foo = 1\n").unwrap();
    let err = apply(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("gamemode", "true")],
        &[],
    )
    .unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<Error>(),
            Some(Error::ConfigUnknownKey { .. })
        ),
        "{err}"
    );
    assert_eq!(
        fs::read_to_string(paths.environment_config("notepad")).unwrap(),
        NOTEPAD
    );
}

const OLD_SPELLING: &str = "# hand written\nschema_version = 1\nprefix = \"~/prefixes/notepad\"\ntest_old_gamemode = false # mine\nrunner = \"wine\"\narch = \"win64\"\n\n[targets.main]\ntest_old_exe = \"C:/a.exe\"\n";

fn read_notepad(paths: &Paths) -> String {
    fs::read_to_string(paths.environment_config("notepad")).unwrap()
}

#[test]
fn setting_either_spelling_updates_the_one_the_file_has() {
    for key in ["gamemode", "test_old_gamemode"] {
        let (_root, paths) = setup();
        write_env(&paths, OLD_SPELLING);
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[(key, "true")],
            &[],
        )
        .unwrap();
        assert_eq!(
            read_notepad(&paths),
            OLD_SPELLING.replace("test_old_gamemode = false", "test_old_gamemode = true"),
            "{key}"
        );
    }
    for key in ["targets.main.exe", "targets.main.test_old_exe"] {
        let (_root, paths) = setup();
        write_env(&paths, OLD_SPELLING);
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[(key, "C:/b.exe")],
            &[],
        )
        .unwrap();
        assert_eq!(
            read_notepad(&paths),
            OLD_SPELLING.replace("C:/a.exe", "C:/b.exe"),
            "{key}"
        );
    }
}

#[test]
fn setting_the_old_spelling_on_a_file_without_it_writes_the_new_one() {
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("test_old_gamemode", "true")],
        &[],
    )
    .unwrap();
    let text = read_notepad(&paths);
    assert!(text.contains("\ngamemode = true\n"), "{text}");
    assert!(!text.contains("test_old_gamemode"), "{text}");
}

#[test]
fn unsetting_either_spelling_removes_the_key() {
    for key in ["gamemode", "test_old_gamemode"] {
        let (_root, paths) = setup();
        write_env(&paths, OLD_SPELLING);
        let missing = update(&paths, ConfigFile::Environment("notepad"), &[], &[key]).unwrap();
        assert!(missing.is_empty(), "{key}");
        assert_eq!(
            read_notepad(&paths),
            OLD_SPELLING.replace("test_old_gamemode = false # mine\n", ""),
            "{key}"
        );
    }
    let (_root, paths) = setup();
    write_env(&paths, NOTEPAD);
    let missing = update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[],
        &["test_old_gamemode"],
    )
    .unwrap();
    assert_eq!(missing, ["test_old_gamemode"]);
}

#[test]
fn an_unrelated_edit_leaves_the_old_spelling_alone() {
    let (_root, paths) = setup();
    write_env(&paths, OLD_SPELLING);
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("mangohud", "true")],
        &[],
    )
    .unwrap();
    assert_eq!(
        read_notepad(&paths),
        OLD_SPELLING.replace("arch = \"win64\"\n", "arch = \"win64\"\nmangohud = true\n")
    );
}

#[test]
fn a_no_op_edit_leaves_the_old_spelling_byte_for_byte() {
    let (_root, paths) = setup();
    write_env(&paths, OLD_SPELLING);
    let path = paths.environment_config("notepad");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    update(
        &paths,
        ConfigFile::Environment("notepad"),
        &[("gamemode", "false"), ("targets.main.exe", "C:/a.exe")],
        &[],
    )
    .unwrap();
    assert_eq!(read_notepad(&paths), OLD_SPELLING);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
}

#[test]
fn an_edit_never_makes_both_spellings() {
    let (_root, paths) = setup();
    let path = paths.global_config();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "test_old_stop_timeout = 3\n[defaults]\ntest_old_runner = \"ge\"\n",
    )
    .unwrap();
    update(
        &paths,
        ConfigFile::Global,
        &[("stop_timeout", "4"), ("defaults.runner", "wine")],
        &[],
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "test_old_stop_timeout = 4\nschema_version = 1\n[defaults]\ntest_old_runner = \"wine\"\n"
    );

    fs::write(&path, "stop_timeout = 3\ntest_old_stop_timeout = 3\n").unwrap();
    let err = update(&paths, ConfigFile::Global, &[("log_limit_mb", "1")], &[]).unwrap_err();
    assert!(err.to_string().contains("test_old_stop_timeout"), "{err}");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "stop_timeout = 3\ntest_old_stop_timeout = 3\n"
    );
}
