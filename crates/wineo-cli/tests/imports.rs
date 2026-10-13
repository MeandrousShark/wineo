mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

use common::{Fixture, assert_success, heroic_library, stderr, stdout, write_script};

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
fn heroic_bulk_fails_on_a_broken_prefix_root_and_imports_nothing() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    f.global_config("[defaults]\nprefix_root = \"\"\n");

    let output = bulk_heroic(&f, &heroic);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cannot make an empty path absolute"),
        "{}",
        stderr(&output)
    );
    assert!(!stdout(&output).contains("Imported"), "{}", stdout(&output));
    assert!(!f.config_dir().join("environments").exists());
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
    assert!(stdout(&output).contains("Imported 1 of 1 game."));
    assert!(
        stdout(&output).contains("Copying includes the whole prefix."),
        "{}",
        stdout(&output)
    );
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
    assert!(stdout(&output).contains("Imported 1 of 1 game."));
    assert!(
        stdout(&output).contains("Copying includes the whole prefix."),
        "{}",
        stdout(&output)
    );
    let expected = format!("{original}heroic_id = \"a\"\nschema_version = 1\n");
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
                assert_eq!(config.wine().unwrap().prefix, prefix.to_str().unwrap());
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
fn heroic_import_dash_d_is_the_same_as_desktop() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    let single = heroic_import(&f, &heroic, &["--game", "a", "-n", "alpha", "-d"]);
    assert_success(&single);
    let path = f.config_dir().join("environments/alpha.toml");
    let config =
        wineo_core::config::EnvironmentConfig::parse(&path, &fs::read_to_string(&path).unwrap())
            .unwrap();
    let target = config.targets.get("alpha").unwrap();
    assert!(target.desktop);
    assert!(
        f.root
            .path()
            .join("data/applications/wineo-alpha-alpha.desktop")
            .is_file()
    );
}

#[test]
fn heroic_import_records_playtime_from_timestamp_json() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    fs::create_dir_all(heroic.join("store")).unwrap();
    fs::write(
        heroic.join("store/timestamp.json"),
        r#"{"a":{"firstPlayed":"2026-06-01T10:00:00.000Z","lastPlayed":"2026-09-20T19:02:11.000Z","totalPlayed":5700}}"#,
    )
    .unwrap();

    let single = heroic_import(&f, &heroic, &["--game", "a", "-n", "alpha"]);
    assert_success(&single);
    let log = f.read_stats_log("alpha");
    assert!(log.contains("\"seconds\":342000"), "{log}");
    assert!(
        log.contains("\"last_played\":\"2026-09-20T19:02:11Z\""),
        "{log}"
    );
}

#[test]
fn heroic_import_with_empty_dates_or_no_timestamp_file_does_not_fail() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    // No `store/timestamp.json` at all.
    let single = heroic_import(&f, &heroic, &["--game", "a", "-n", "alpha"]);
    assert_success(&single);
    assert!(f.read_stats_log("alpha").is_empty());

    fs::create_dir_all(heroic.join("store")).unwrap();
    fs::write(
        heroic.join("store/timestamp.json"),
        r#"{"b":{"firstPlayed":"","lastPlayed":"","totalPlayed":0}}"#,
    )
    .unwrap();
    let second = heroic_import(&f, &heroic, &["--game", "b", "-n", "beta"]);
    assert_success(&second);
    assert!(f.read_stats_log("beta").is_empty());
}

#[test]
fn heroic_bulk_import_records_playtime_for_every_game() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    fs::create_dir_all(heroic.join("store")).unwrap();
    fs::write(
        heroic.join("store/timestamp.json"),
        r#"{"a":{"firstPlayed":"2026-01-01T00:00:00.000Z","lastPlayed":"2026-01-02T00:00:00.000Z","totalPlayed":60},"b":{"firstPlayed":"","lastPlayed":"","totalPlayed":0}}"#,
    )
    .unwrap();

    let output = bulk_heroic(&f, &heroic);
    assert_success(&output);
    assert!(f.read_stats_log("alpha").contains("\"seconds\":3600"));
    assert!(f.read_stats_log("beta").is_empty());
}

#[test]
fn heroic_bulk_imports_absolute_pfx_self_links_and_recognizes_completed_games() {
    let f = Fixture::new();
    f.install_proton("GE-Proton11-7");
    let heroic = f.root.path().join("heroic");
    fs::create_dir_all(heroic.join("sideload_apps")).unwrap();
    fs::create_dir_all(heroic.join("GamesConfig")).unwrap();
    fs::write(
        heroic.join("config.json"),
        r#"{"defaultSettings":{"wineVersion":{"type":"proton","name":"GE-Proton11-7","bin":"proton"}}}"#,
    )
    .unwrap();
    let mut entries = Vec::new();
    for (id, title, linked) in [
        ("a", "Alpha", true),
        ("b", "Beta", true),
        ("c", "Gamma", false),
    ] {
        let prefix = f.root.path().join(format!("prefix-{id}"));
        let pfx = if linked {
            fs::create_dir_all(&prefix).unwrap();
            symlink(&prefix, prefix.join("pfx")).unwrap();
            prefix.clone()
        } else {
            prefix.join("pfx")
        };
        fs::create_dir_all(pfx.join("drive_c/Games")).unwrap();
        fs::write(
            pfx.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        let exe = pfx.join("drive_c/Games/Game.exe");
        fs::write(&exe, "game executable").unwrap();
        fs::write(
            heroic.join(format!("GamesConfig/{id}.json")),
            format!(r#"{{"{id}":{{"winePrefix":"{}"}}}}"#, prefix.display()),
        )
        .unwrap();
        entries.push(format!(
            r#"{{"app_name":"{id}","title":"{title}","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}"#,
            exe.display()
        ));
    }
    fs::write(
        heroic.join("sideload_apps/library.json"),
        format!(r#"{{"games":[{}]}}"#, entries.join(",")),
    )
    .unwrap();

    let run = || {
        f.wineo()
            .args([
                "import",
                "heroic",
                "--config-dir",
                heroic.to_str().unwrap(),
                "--all",
                "--yes",
                "--runner",
                "GE-Proton11-7",
            ])
            .output()
            .unwrap()
    };
    let output = run();
    assert_success(&output);
    let printed = stdout(&output);
    assert!(printed.contains("Imported 3 of 3 games."), "{printed}");
    for (id, name) in [("a", "alpha"), ("b", "beta"), ("c", "gamma")] {
        assert!(
            printed.contains(&format!("Importing {name}... done")),
            "{printed}"
        );
        assert!(
            f.config_dir()
                .join(format!("environments/{name}.toml"))
                .exists()
        );
        let copied = f.root.path().join(format!("data/wineo/prefixes/{name}"));
        assert!(copied.join("pfx/system.reg").is_file());
        if id != "c" {
            assert_eq!(fs::read_link(copied.join("pfx")).unwrap(), copied);
            let source = f.root.path().join(format!("prefix-{id}"));
            assert_eq!(fs::read_link(source.join("pfx")).unwrap(), source);
        }
    }
    let repeated = run();
    assert_success(&repeated);
    let printed = stdout(&repeated);
    assert!(
        printed.contains("Already imported: alpha, beta, gamma"),
        "{printed}"
    );
    assert!(
        printed.contains("No Heroic games are ready to import."),
        "{printed}"
    );
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

#[test]
fn heroic_bulk_preserves_omission_evidence_in_verbose_output_and_plain_logs() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    fs::write(heroic.join("GamesConfig/a.json"), r#"{"a":{
        "launcherArgs":"--profile 'two words'",
        "enviromentOptions":[{"key":"WINEPREFIX","value":"/do/not/use"},{"key":"API_TOKEN","value":"redact-me"}],
        "wrapperOptions":"unsupported-wrapper",
        "steamGridDbApiKey":"never-import-this"
    }}"#).unwrap();
    let output = f
        .wineo()
        .args([
            "-v",
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--all",
            "--yes",
            "--runner",
            "wine",
        ])
        .env("CLICOLOR_FORCE", "1")
        .env_remove("NO_COLOR")
        .output()
        .unwrap();
    assert_success(&output);
    let printed = stdout(&output);
    let log = fs::read_to_string(f.root.path().join("state/wineo/imports/alpha.log")).unwrap();
    for text in [&printed, &log] {
        assert!(text.contains("WINEPREFIX"), "{text}");
        assert!(text.contains("wrapperOptions"), "{text}");
        assert!(text.contains("two words"), "{text}");
        assert!(!text.contains("redact-me"), "{text}");
        assert!(!text.contains("never-import-this"), "{text}");
    }
    assert!(printed.contains("warn:"), "{printed}");
    assert!(!log.contains('\u{1b}'));
    let config_path = f.config_dir().join("environments/alpha.toml");
    let config_text = fs::read_to_string(&config_path).unwrap();
    let config = wineo_core::config::EnvironmentConfig::parse(&config_path, &config_text).unwrap();
    assert!(!config.env.contains_key("WINEPREFIX"));
    assert!(!config_text.contains("never-import-this"));
    assert_eq!(config.targets["alpha"].args, ["--profile", "two words"]);
}

#[test]
fn heroic_bulk_imports_gamescope_mangohud_and_supported_wine_wayland_hdr() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    f.install_runner("wine-test");
    let root = f.root.path().join("data/wineo/runners/wine-test");
    fs::create_dir_all(root.join("lib/wine/x86_64-unix")).unwrap();
    fs::write(root.join("lib/wine/x86_64-unix/winewayland.so"), "").unwrap();
    fs::write(heroic.join("GamesConfig/a.json"), r#"{"a":{"enableWineWayland":true,"enableHDR":true,"showMangohud":true,"gamescope":{"enableUpscaling":true,"enableLimiter":true,"gameWidth":"1920","upscaleMethod":"fsr","fpsLimiter":"60","additionalOptions":"--label 'two words'"},"enviromentOptions":[{"key":"DXVK_HDR","value":"custom"}]}}"#).unwrap();
    let output = f.run(&[
        "-v",
        "import",
        "heroic",
        "--config-dir",
        heroic.to_str().unwrap(),
        "--all",
        "--yes",
        "--runner",
        "wine-test",
    ]);
    assert_success(&output);
    let text = f.created_config("alpha");
    let path = f.config_dir().join("environments/alpha.toml");
    let config = wineo_core::config::EnvironmentConfig::parse(&path, &text).unwrap();
    assert_eq!(config.wayland, Some(true));
    assert_eq!(config.mangohud, Some(true));
    assert_eq!(config.env["DXVK_HDR"], "custom");
    let game = config.gamescope.unwrap();
    let game = game.on().unwrap();
    assert_eq!((game.width, game.fps_limit), (Some(1920), Some(60)));
    assert_eq!(game.args, ["--label", "two words"]);
    assert!(text.contains("[gamescope]"), "{text}");
    let log = fs::read_to_string(f.root.path().join("state/wineo/imports/alpha.log")).unwrap();
    for field in [
        "wayland: Some(true)",
        "mangohud: Some(true)",
        "gamescope: Some(On(",
        "enableHDR",
    ] {
        assert!(log.contains(field), "{field}: {log}");
    }
}

#[test]
fn heroic_direct_skips_wayland_on_unsupported_runner() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    fs::write(heroic.join("GamesConfig/a.json"), r#"{"a":{"enableWineWayland":true,"showMangohud":true,"gamescope":{"enableUpscaling":false,"enableLimiter":false}}}"#).unwrap();
    let output = heroic_import(&f, &heroic, &["--game", "a", "-n", "alpha"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("wayland") && out.contains("does not support Wine Wayland"),
        "{out}"
    );
    let text = f.created_config("alpha");
    assert!(!text.contains("wayland ="), "{text}");
    assert!(!text.contains("[gamescope]"), "{text}");
    assert!(text.contains("mangohud = true"), "{text}");
}

#[test]
fn heroic_explicit_false_overrides_global_game_toggles() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    f.global_config("wayland = true\nmangohud = true\n");
    fs::write(
        heroic.join("GamesConfig/a.json"),
        r#"{"a":{"enableWineWayland":false,"showMangohud":false}}"#,
    )
    .unwrap();
    let output = heroic_import(&f, &heroic, &["--game", "a", "-n", "alpha"]);
    assert_success(&output);
    let text = f.created_config("alpha");
    assert!(text.contains("wayland = false"), "{text}");
    assert!(text.contains("mangohud = false"), "{text}");
    let config = wineo_core::config::EnvironmentConfig::parse(
        &f.config_dir().join("environments/alpha.toml"),
        &text,
    )
    .unwrap();
    assert_eq!(config.wayland, Some(false));
    assert_eq!(config.mangohud, Some(false));
}

#[test]
fn heroic_direct_skips_non_numeric_gamescope_fields_with_a_warning() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    fs::write(heroic.join("GamesConfig/a.json"), r#"{"a":{"gamescope":{"enableUpscaling":true,"gameWidth":"bad","gameHeight":"","upscaleMethod":"integer"}}}"#).unwrap();
    let output = heroic_import(&f, &heroic, &["--game", "a", "-n", "alpha"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("gamescope gameWidth is not a number; skipped"),
        "{out}"
    );
    let text = f.created_config("alpha");
    assert!(text.contains("scaler = \"integer\""), "{text}");
    assert!(!text.contains("width ="), "{text}");
}

#[test]
fn heroic_proton_import_adds_hdr_only_with_supported_wayland() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    f.install_proton("proton-test");
    symlink(".", prefix.join("pfx")).unwrap();
    let proton = f.root.path().join("data/wineo/runners/proton-test/proton");
    write_script(&proton, "#!/bin/sh\n# PROTON_ENABLE_WAYLAND\nexit 0\n");
    let settings = fs::read_to_string(heroic.join("config.json"))
        .unwrap()
        .replace(
            "\"type\":\"wine\",\"name\":\"System\",\"bin\":\"wine\"",
            &format!(
                "\"type\":\"proton\",\"name\":\"proton-test\",\"bin\":\"{}\"",
                proton.display()
            ),
        );
    fs::write(heroic.join("config.json"), settings).unwrap();
    fs::write(
        heroic.join("GamesConfig/a.json"),
        r#"{"a":{"enableWineWayland":true,"enableHDR":true}}"#,
    )
    .unwrap();
    let output = f.run(&[
        "import",
        "heroic",
        "--config-dir",
        heroic.to_str().unwrap(),
        "--game",
        "a",
        "-n",
        "alpha",
        "--runner",
        "proton-test",
    ]);
    assert_success(&output);
    let text = f.created_config("alpha");
    assert!(text.contains("wayland = true"), "{text}");
    assert!(text.contains("PROTON_ENABLE_HDR = \"1\""), "{text}");
    assert!(!text.contains("DXVK_HDR"), "{text}");
}

#[test]
fn heroic_disable_umu_disclosure_follows_runner_and_per_game_setting() {
    for (proton, per_game, global_only) in [
        (true, Some(true), false),
        (true, Some(false), false),
        (true, None, false),
        (true, None, true),
        (false, Some(true), false),
        (false, Some(false), false),
        (false, None, false),
    ] {
        for bulk in [false, true] {
            for verbose in [false, true] {
                let f = Fixture::new();
                let prefix = source(f.root.path());
                let heroic = heroic_library(&f, &prefix);
                let runner = if proton {
                    f.install_proton("GE-Proton11-7");
                    symlink(".", prefix.join("pfx")).unwrap();
                    "GE-Proton11-7"
                } else {
                    "wine"
                };
                let mut settings = fs::read_to_string(heroic.join("config.json")).unwrap();
                if proton {
                    settings = settings.replace(
                        "\"type\":\"wine\",\"name\":\"System\",\"bin\":\"wine\"",
                        &format!(
                            "\"type\":\"proton\",\"name\":\"GE-Proton11-7\",\"bin\":\"{}\"",
                            f.root
                                .path()
                                .join("data/wineo/runners/GE-Proton11-7/proton")
                                .display()
                        ),
                    );
                }
                if global_only {
                    settings = settings.replace(
                        "\"defaultSettings\":{",
                        "\"defaultSettings\":{\"disableUMU\":true,",
                    );
                }
                fs::write(heroic.join("config.json"), settings).unwrap();
                if let Some(value) = per_game {
                    fs::write(
                        heroic.join("GamesConfig/a.json"),
                        format!(r#"{{"a":{{"disableUMU":{value}}}}}"#),
                    )
                    .unwrap();
                }
                let mut command = f.wineo();
                if verbose {
                    command.arg("-v");
                }
                command.args([
                    "import",
                    "heroic",
                    "--config-dir",
                    heroic.to_str().unwrap(),
                    "--runner",
                    runner,
                    "--adopt",
                ]);
                if bulk {
                    command.args(["--all", "--yes"]);
                } else {
                    command.args(["--game", "a", "--name", "alpha"]);
                }
                let output = command.output().unwrap();
                let mismatch = proton && per_game == Some(true);
                if mismatch && !bulk {
                    assert!(!output.status.success());
                    assert!(stderr(&output).contains("--interactive"));
                    assert!(
                        stderr(&output).contains("Heroic disabled umu; wineo uses umu for Proton")
                    );
                    assert!(!f.config_dir().join("environments/alpha.toml").exists());
                } else {
                    assert_success(&output);
                    if verbose {
                        let log = fs::read_to_string(f.root.path().join("state/wineo/imports/alpha.log"))
                    .unwrap_or_else(|error| panic!("proton={proton}, per_game={per_game:?}, global_only={global_only}, bulk={bulk}: {error}\n{}\n{}", stdout(&output), stderr(&output)));
                        assert_eq!(
                            log.contains("Heroic disabled umu; wineo uses umu for Proton"),
                            mismatch,
                            "{log}"
                        );
                    }
                    assert_eq!(
                        stdout(&output).contains("Heroic disabled umu; wineo uses umu for Proton"),
                        mismatch,
                        "{}",
                        stdout(&output)
                    );
                }
            }
        }
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
    let trash = prefix.parent().unwrap().join(".wineo-trash");
    fs::write(&trash, b"occupied").unwrap();
    let failed = f.run(&["rm", "adopted", "--delete-prefix"]);
    assert!(!failed.status.success());
    assert!(
        stderr(&failed).contains(".wineo-trash"),
        "{}",
        stderr(&failed)
    );
    assert_eq!(fs::read(&trash).unwrap(), b"occupied");
    assert_eq!(
        fs::read_to_string(configs.join("adopted.toml")).unwrap(),
        original
    );
    assert_eq!(fs::read(games.join("save.dat")).unwrap(), b"save data");
    assert_success(&f.run(&["rm", "adopted"]));
    assert!(prefix.exists());
}

fn heroic_import(f: &Fixture, config: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "import",
        "heroic",
        "--config-dir",
        config.to_str().unwrap(),
        "--runner",
        "wine",
    ];
    args.extend(extra);
    f.run(&args)
}

fn steamgriddb_lines(output: &std::process::Output) -> Vec<String> {
    format!("{}{}", stdout(output), stderr(output))
        .lines()
        .filter(|line| line.contains("SteamGridDB"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn heroic_art_without_a_key_notes_it_once_and_succeeds() {
    const NOTE: &str = "note: run `wineo art key` to fetch art from SteamGridDB";
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);

    let single = heroic_import(&f, &heroic, &["--game", "a", "-n", "alpha", "--art"]);
    assert_success(&single);
    assert_eq!(steamgriddb_lines(&single), [NOTE]);
    assert!(
        stdout(&single).contains("Imported alpha."),
        "{}",
        stdout(&single)
    );

    let bulk = heroic_import(&f, &heroic, &["--all", "--yes", "--art"]);
    assert_success(&bulk);
    assert_eq!(steamgriddb_lines(&bulk), [NOTE]);
    assert!(
        stdout(&bulk).contains("Imported 1 of 1 game."),
        "{}",
        stdout(&bulk)
    );
}

#[test]
fn heroic_imports_without_art_say_nothing_about_it() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);

    let single = heroic_import(&f, &heroic, &["--game", "a", "-n", "alpha"]);
    assert_success(&single);
    assert!(steamgriddb_lines(&single).is_empty());
    let bulk = heroic_import(&f, &heroic, &["--all", "--yes"]);
    assert_success(&bulk);
    assert!(steamgriddb_lines(&bulk).is_empty());
}

/// A Heroic library of native Linux sideloads: Dusklight, whose script is
/// there, and Ship of Harkinian, whose exe isn't. Heroic keeps Wine settings
/// for native games too, which the import ignores.
fn native_heroic(f: &Fixture) -> (std::path::PathBuf, std::path::PathBuf) {
    let config = f.root.path().join("heroic");
    fs::create_dir_all(config.join("sideload_apps")).unwrap();
    fs::create_dir_all(config.join("GamesConfig")).unwrap();
    let exe = f.root.path().join("games/dusklight");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    write_script(&exe, "#!/bin/sh\nexit 0\n");
    fs::write(
        config.join("config.json"),
        r#"{"defaultSettings":{"winePrefix":"/nonexistent/prefix","wineVersion":{"type":"wine","name":"System","bin":"wine"},"autoInstallDxvk":true}}"#,
    )
    .unwrap();
    fs::write(
        config.join("GamesConfig/dusk.json"),
        r#"{"dusk":{"useGameMode":true,"launcherArgs":"-x \"two words\"","enviromentOptions":[{"key":"FOO","value":"bar"}],"wrapperOptions":[{"exe":"mangohud","args":""}]}}"#,
    )
    .unwrap();
    fs::write(config.join("GamesConfig/soh.json"), r#"{"soh":{}}"#).unwrap();
    fs::write(
        config.join("sideload_apps/library.json"),
        format!(
            r#"{{"games":[{{"app_name":"dusk","title":"Dusklight","runner":"sideload","install":{{"platform":"linux","executable":"{}"}}}},{{"app_name":"soh","title":"Ship of Harkinian","runner":"sideload","install":{{"platform":"linux","executable":"/nonexistent/soh"}}}}]}}"#,
            exe.display()
        ),
    )
    .unwrap();
    (config, exe)
}

fn heroic_native(f: &Fixture, config: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["import", "heroic", "--config-dir", config.to_str().unwrap()];
    args.extend(extra);
    f.run(&args)
}

#[test]
fn a_native_heroic_game_imports_as_a_native_environment() {
    let f = Fixture::new();
    let (heroic, exe) = native_heroic(&f);

    let output = heroic_native(&f, &heroic, &["--game", "dusk", "-n", "dusk"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Import Dusklight as dusk"), "{out}");
    let row = |label: &str| {
        out.lines()
            .find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some(label)).then(|| words.collect::<Vec<_>>().join(" "))
            })
            .unwrap_or_default()
    };
    assert_eq!(row("kind"), "native", "{out}");
    assert_eq!(row("exe"), exe.display().to_string(), "{out}");
    assert_eq!(row("gamemode"), "on", "{out}");
    assert!(
        out.contains("wrapperOptions: Heroic wrappers require manual setup"),
        "{out}"
    );
    for wine in ["runner", "prefix", "dxvk", "vkd3d", "autoInstallDxvk"] {
        assert!(!out.contains(wine), "{wine}: {out}");
    }
    assert!(out.contains("Imported dusk."), "{out}");

    let path = f.root.path().join("config/wineo/environments/dusk.toml");
    let text = fs::read_to_string(&path).unwrap();
    let config = wineo_core::config::EnvironmentConfig::parse(&path, &text).unwrap();
    assert!(config.wine().is_none(), "{text}");
    assert_eq!(config.heroic_id.as_deref(), Some("dusk"));
    assert_eq!(config.gamemode, Some(true));
    assert_eq!(config.env["FOO"], "bar");
    let game = &config.targets["game"];
    assert_eq!(game.exe, exe.to_str().unwrap());
    assert_eq!(game.args, ["-x", "two words"]);
    assert!(game.default);
    assert!(!game.desktop);

    // It runs like any native environment.
    assert_success(&f.run(&["run", "dusk"]));
}

#[test]
fn native_heroic_import_carries_gamescope_and_mangohud_but_not_wine_keys() {
    let f = Fixture::new();
    let (heroic, _) = native_heroic(&f);
    fs::write(heroic.join("GamesConfig/dusk.json"), r#"{"dusk":{"enableWineWayland":true,"enableHDR":true,"showMangohud":true,"gamescope":{"enableUpscaling":true,"upscaleMethod":"stretch","windowType":"borderless","additionalOptions":"--label 'two words'"}}}"#).unwrap();
    let output = heroic_native(&f, &heroic, &["--game", "dusk", "-n", "dusk"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("mangohud") && out.contains("gamescope"),
        "{out}"
    );
    assert!(!out.contains("enableWineWayland"), "{out}");
    let text = f.created_config("dusk");
    let path = f.config_dir().join("environments/dusk.toml");
    let config = wineo_core::config::EnvironmentConfig::parse(&path, &text).unwrap();
    assert_eq!(config.mangohud, Some(true));
    let game = config.gamescope.unwrap();
    let game = game.on().unwrap();
    assert_eq!(game.scaler, Some(wineo_core::gamescope::Scaler::Stretch));
    assert_eq!(game.window, Some(wineo_core::gamescope::Window::Borderless));
    assert_eq!(game.args, ["--label", "two words"]);
    assert!(!text.contains("wayland ="), "{text}");
    assert!(!text.contains("DXVK_HDR"), "{text}");
}

#[test]
fn a_native_heroic_import_records_playtime_from_timestamp_json() {
    let f = Fixture::new();
    let (heroic, _) = native_heroic(&f);
    fs::create_dir_all(heroic.join("store")).unwrap();
    fs::write(
        heroic.join("store/timestamp.json"),
        r#"{"dusk":{"firstPlayed":"2026-01-01T00:00:00.000Z","lastPlayed":"2026-09-20T19:02:11.000Z","totalPlayed":90}}"#,
    )
    .unwrap();

    let output = heroic_native(&f, &heroic, &["--game", "dusk", "-n", "dusk"]);
    assert_success(&output);
    let log = f.read_stats_log("dusk");
    assert!(log.contains("\"seconds\":5400"), "{log}");
    assert!(
        log.contains("\"last_played\":\"2026-09-20T19:02:11Z\""),
        "{log}"
    );
}

#[test]
fn a_native_heroic_game_refuses_wine_flags_and_a_missing_exe() {
    let f = Fixture::new();
    let (heroic, _) = native_heroic(&f);

    let refused = heroic_native(
        &f,
        &heroic,
        &["--game", "dusk", "-n", "dusk", "--runner", "wine"],
    );
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("Dusklight is a native game, so --runner can't be used"),
        "{}",
        stderr(&refused)
    );

    let missing = heroic_native(&f, &heroic, &["--game", "soh", "-n", "soh"]);
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("executable not found: /nonexistent/soh"),
        "{}",
        stderr(&missing)
    );
    assert!(
        !f.root
            .path()
            .join("config/wineo/environments/dusk.toml")
            .exists()
    );
    assert!(
        !f.root
            .path()
            .join("config/wineo/environments/soh.toml")
            .exists()
    );

    let listed = heroic_native(&f, &heroic, &["--list"]);
    assert_success(&listed);
    assert!(
        stderr(&listed).contains("  issue: executable: executable not found: /nonexistent/soh"),
        "{}",
        stderr(&listed)
    );
}

#[test]
fn heroic_all_imports_native_games_and_recognizes_them_again() {
    let f = Fixture::new();
    let (heroic, _) = native_heroic(&f);

    // No runner is needed when every game is native.
    let output = heroic_native(&f, &heroic, &["--all", "--yes"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Import 2 Heroic games:"), "{out}");
    assert!(out.contains("dusklight"), "{out}");
    assert!(out.contains("native"), "{out}");
    assert!(
        out.contains("skip: executable not found: /nonexistent/soh"),
        "{out}"
    );
    assert!(out.contains("Imported 1 of 1 game."), "{out}");
    let text = fs::read_to_string(
        f.root
            .path()
            .join("config/wineo/environments/dusklight.toml"),
    )
    .unwrap();
    assert!(text.contains("kind = \"native\""), "{text}");
    assert!(text.contains("heroic_id = \"dusk\""), "{text}");

    let again = heroic_native(&f, &heroic, &["--all", "--yes"]);
    assert_success(&again);
    assert!(
        stdout(&again).contains("Already imported: dusklight"),
        "{}",
        stdout(&again)
    );
}

#[test]
fn heroic_all_backfills_a_native_environment_matched_by_its_exe() {
    let f = Fixture::new();
    let (heroic, exe) = native_heroic(&f);
    let path = f.root.path().join("config/wineo/environments/mine.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!(
            "kind = \"native\"\n\n[targets.play]\nexe = {:?}\n",
            exe.display()
        ),
    )
    .unwrap();

    let output = heroic_native(&f, &heroic, &["--all", "--yes"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("Already imported: mine"),
        "{}",
        stdout(&output)
    );
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("heroic_id = \"dusk\"")
    );
    assert!(
        !f.root
            .path()
            .join("config/wineo/environments/dusklight.toml")
            .exists()
    );
}

#[test]
fn heroic_all_imports_windows_and_native_games_together() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    let (_, exe) = {
        // Adds Dusklight next to the Windows games.
        let exe = f.root.path().join("games/dusklight");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        write_script(&exe, "#!/bin/sh\nexit 0\n");
        let library = heroic.join("sideload_apps/library.json");
        let text = fs::read_to_string(&library).unwrap().replace(
            "]}",
            &format!(
                r#",{{"app_name":"dusk","title":"Dusklight","runner":"sideload","install":{{"platform":"linux","executable":"{}"}}}}]}}"#,
                exe.display()
            ),
        );
        fs::write(&library, text).unwrap();
        (heroic.clone(), exe)
    };
    let output = bulk_heroic(&f, &heroic);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Imported 3 of 3 games."), "{out}");
    let dusk = fs::read_to_string(
        f.root
            .path()
            .join("config/wineo/environments/dusklight.toml"),
    )
    .unwrap();
    assert!(
        dusk.contains(&format!("exe = \"{}\"", exe.display())),
        "{dusk}"
    );
    assert!(!dusk.contains("prefix"), "{dusk}");
}

#[test]
fn a_native_heroic_import_keeps_wine_variables_and_carries_nvidia_prime() {
    let f = Fixture::new();
    let (heroic, _) = native_heroic(&f);
    fs::write(
        heroic.join("GamesConfig/dusk.json"),
        r#"{"dusk":{"nvidiaPrime":true,"enviromentOptions":[{"key":"WINEPREFIX","value":"/mine"},{"key":"PROTONPATH","value":"/proton"}]}}"#,
    )
    .unwrap();
    assert_success(&heroic_native(
        &f,
        &heroic,
        &["--game", "dusk", "-n", "dusk"],
    ));
    let path = f.root.path().join("config/wineo/environments/dusk.toml");
    let text = fs::read_to_string(&path).unwrap();
    let config = wineo_core::config::EnvironmentConfig::parse(&path, &text).unwrap();
    assert_eq!(config.env["WINEPREFIX"], "/mine");
    assert_eq!(config.env["PROTONPATH"], "/proton");
    assert_eq!(config.env["DRI_PRIME"], "1");
    assert_eq!(config.env["__NV_PRIME_RENDER_OFFLOAD"], "1");
    assert_eq!(config.env["__GLX_VENDOR_LIBRARY_NAME"], "nvidia");
}

fn slow_prefix(root: &Path) -> std::path::PathBuf {
    let prefix = root.join("large prefix");
    fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    fs::write(prefix.join("drive_c/Games/Game.exe"), b"game executable").unwrap();
    for index in 0..10_000 {
        fs::write(
            prefix.join(format!("drive_c/Games/file-{index}")),
            b"save data",
        )
        .unwrap();
    }
    prefix
}

fn interrupt_copy(mut command: Command) -> std::process::Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let pipe = child.stderr.take().unwrap();
    let reader = thread::spawn(move || {
        let mut captured = Vec::new();
        let mut interrupted = false;
        for line in BufReader::new(pipe).split(b'\n') {
            let line = line.unwrap();
            if !interrupted && line == b"Copied 500 entries" {
                let status = Command::new("kill")
                    .args(["-INT", &pid.to_string()])
                    .status()
                    .unwrap();
                assert!(status.success());
                interrupted = true;
            }
            captured.extend_from_slice(&line);
            captured.push(b'\n');
        }
        (interrupted, captured)
    });
    let mut output = child.wait_with_output().unwrap();
    let (interrupted, captured) = reader.join().unwrap();
    assert!(interrupted, "Copy never reached its progress checkpoint");
    output.stderr = captured;
    output
}

#[test]
fn heroic_direct_copy_interrupt_exits_130_and_removes_partial_import() {
    let f = Fixture::new();
    let prefix = slow_prefix(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    let mut command = f.wineo();
    command.args([
        "import",
        "heroic",
        "--config-dir",
        heroic.to_str().unwrap(),
        "--game",
        "a",
        "-n",
        "canceled",
        "--runner",
        "wine",
    ]);
    let output = interrupt_copy(command);
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Import canceled"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("error: canceled"),
        "{}",
        stderr(&output)
    );
    assert!(!f.config_dir().join("environments/canceled.toml").exists());
    assert!(!f.root.path().join("data/wineo/prefixes/canceled").exists());
    assert!(prefix.join("drive_c/Games/file-9999").exists());
}

#[test]
fn heroic_bulk_copy_interrupt_keeps_completed_games_and_cancels_the_rest() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    let large = slow_prefix(f.root.path());
    let library_path = heroic.join("sideload_apps/library.json");
    let mut library: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&library_path).unwrap()).unwrap();
    library["games"][1]["install"]["executable"] = large
        .join("drive_c/Games/Game.exe")
        .to_str()
        .unwrap()
        .into();
    let mut third = library["games"][0].clone();
    third["app_name"] = "c".into();
    third["title"] = "Gamma".into();
    library["games"].as_array_mut().unwrap().push(third);
    fs::write(&library_path, library.to_string()).unwrap();
    fs::write(
        heroic.join("GamesConfig/b.json"),
        serde_json::json!({"b": {"winePrefix": large}}).to_string(),
    )
    .unwrap();
    fs::write(heroic.join("GamesConfig/c.json"), r#"{"c":{}}"#).unwrap();
    let mut command = f.wineo();
    command.args([
        "import",
        "heroic",
        "--config-dir",
        heroic.to_str().unwrap(),
        "--all",
        "--yes",
        "--runner",
        "wine",
    ]);
    let output = interrupt_copy(command);
    assert_eq!(
        output.status.code(),
        Some(130),
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    let out = stdout(&output);
    assert!(out.contains("Importing beta... canceled"), "{out}");
    assert!(out.contains("Imported 1 of 3 games."), "{out}");
    assert!(out.contains("2 games canceled."), "{out}");
    assert!(!out.contains("failed: canceled"), "{out}");
    assert!(!out.contains("Importing gamma"), "{out}");
    assert!(f.config_dir().join("environments/alpha.toml").exists());
    assert!(!f.config_dir().join("environments/beta.toml").exists());
    assert!(!f.config_dir().join("environments/gamma.toml").exists());
    assert!(
        f.root
            .path()
            .join("data/wineo/prefixes/alpha/drive_c/Games/save.dat")
            .exists()
    );
    assert!(!f.root.path().join("data/wineo/prefixes/beta").exists());
}

#[test]
fn heroic_bulk_cancel_summary_uses_singular_for_one_game() {
    let f = Fixture::new();
    let prefix = slow_prefix(f.root.path());
    let heroic = heroic_library(&f, &prefix);
    let path = heroic.join("sideload_apps/library.json");
    let mut library: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    library["games"].as_array_mut().unwrap().truncate(1);
    fs::write(path, library.to_string()).unwrap();
    let mut command = f.wineo();
    command.args([
        "import",
        "heroic",
        "--config-dir",
        heroic.to_str().unwrap(),
        "--all",
        "--yes",
        "--runner",
        "wine",
    ]);
    let output = interrupt_copy(command);
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("Imported 0 of 1 game."), "{out}");
    assert!(out.contains("1 game canceled."), "{out}");
    assert!(!f.config_dir().join("environments/alpha.toml").exists());
}
