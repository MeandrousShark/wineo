mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::snapshot::Snapshot;
use common::{Fixture, assert_success, stderr, stdout};

const RUNNER_MARKER: &str = "WINEO_TEST_RUNNER_MARKER";

fn install_guarded_wine(f: &Fixture) {
    f.install_runner("test-wine");
    for wine in [
        f.root.path().join("bin/wine"),
        f.root.path().join("data/wineo/runners/test-wine/bin/wine"),
    ] {
        write_executable(
            &wine,
            "#!/bin/sh\nif [ \"${1-}\" = \"--version\" ]; then\n  printf '%s\\n' 'wine-test'\n  exit 0\nfi\n: > \"$WINEO_TEST_RUNNER_MARKER\"\nexit 91\n",
        );
    }
    for server in [
        f.root.path().join("bin/wineserver"),
        f.root
            .path()
            .join("data/wineo/runners/test-wine/bin/wineserver"),
    ] {
        write_executable(
            &server,
            "#!/bin/sh\n: > \"$WINEO_TEST_RUNNER_MARKER\"\nexit 91\n",
        );
    }
    write_executable(
        &f.root.path().join("bin/umu-run"),
        "#!/bin/sh\n: > \"$WINEO_TEST_RUNNER_MARKER\"\nexit 91\n",
    );
}

fn write_executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn run_with(f: &Fixture, args: &[&str], configure: impl FnOnce(&mut Command)) -> Output {
    let mut command = f.wineo();
    command
        .env("HOME", f.root.path())
        .env("XDG_CACHE_HOME", f.root.path().join("cache"))
        .env(RUNNER_MARKER, f.root.path().join("runner-started"))
        .args(args);
    configure(&mut command);
    command.output().expect("wineo should start")
}

fn run(f: &Fixture, args: &[&str]) -> Output {
    run_with(f, args, |_| {})
}

fn dry_run(f: &Fixture, args: &[&str]) -> Output {
    dry_run_with(f, args, |_| {})
}

fn dry_run_with(f: &Fixture, args: &[&str], configure: impl FnOnce(&mut Command)) -> Output {
    let before = Snapshot::capture(f.root.path()).expect("capture fixture tree before command");
    let output = run_with(f, args, configure);
    assert!(
        !f.root.path().join("runner-started").exists(),
        "a fake Wine or umu runner was started\n{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    before.assert_unchanged(f.root.path());
    output
}

fn source(root: &Path) -> PathBuf {
    let prefix = root.join("old prefix");
    fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    fs::write(prefix.join("drive_c/Games/Game.exe"), "game executable").unwrap();
    prefix
}

fn heroic_windows(f: &Fixture, prefix: &Path, games: &[(&str, &str, &str)]) -> PathBuf {
    let config = f.root.path().join("heroic");
    fs::create_dir_all(config.join("sideload_apps")).unwrap();
    fs::create_dir_all(config.join("GamesConfig")).unwrap();
    let exe = prefix.join("drive_c/Games/Game.exe");
    fs::write(
        config.join("config.json"),
        format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
            prefix.display()
        ),
    )
    .unwrap();
    let entries: Vec<_> = games
        .iter()
        .map(|(id, title, settings)| {
            let settings = if settings.is_empty() {
                String::new()
            } else {
                format!(",{settings}")
            };
            fs::write(
                config.join(format!("GamesConfig/{id}.json")),
                format!(
                    r#"{{"{id}":{{"winePrefix":"{}"{settings}}}}}"#,
                    prefix.display()
                ),
            )
            .unwrap();
            format!(
                r#"{{"app_name":"{id}","title":"{title}","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}"#,
                exe.display()
            )
        })
        .collect();
    fs::write(
        config.join("sideload_apps/library.json"),
        format!(r#"{{"games":[{}]}}"#, entries.join(",")),
    )
    .unwrap();
    config
}

fn heroic_with_native(f: &Fixture, prefix: &Path) -> (PathBuf, PathBuf) {
    let config = heroic_windows(f, prefix, &[("a", "Alpha", "")]);
    let exe = f.root.path().join("games/dusklight");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    write_executable(&exe, "#!/bin/sh\nexit 0\n");
    fs::write(config.join("GamesConfig/dusk.json"), r#"{"dusk":{}}"#).unwrap();
    fs::write(
        config.join("sideload_apps/library.json"),
        format!(
            r#"{{"games":[{{"app_name":"a","title":"Alpha","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}},{{"app_name":"dusk","title":"Dusklight","runner":"sideload","install":{{"platform":"linux","executable":"{}"}}}}]}}"#,
            prefix.join("drive_c/Games/Game.exe").display(),
            exe.display()
        ),
    )
    .unwrap();
    (config, exe)
}

#[test]
fn prefix_copy_and_adopt_dry_runs_print_the_review_without_writing() {
    for adopt in [false, true] {
        let f = Fixture::new();
        install_guarded_wine(&f);
        let prefix = source(f.root.path());
        let destination = f.root.path().join("copy destination");
        let mut args = vec![
            "import",
            "prefix",
            prefix.to_str().unwrap(),
            "-n",
            "sample",
            "--runner",
            "test-wine",
        ];
        if adopt {
            args.push("--adopt");
        } else {
            args.extend(["--dir", destination.to_str().unwrap()]);
        }
        args.push("--dry-run");

        let output = dry_run(&f, &args);
        assert_success(&output);
        let text = stdout(&output);
        assert!(text.contains("Import sample\n"), "{text}");
        assert!(text.contains("source:"), "{text}");
        assert!(
            text.contains(if adopt { "adopt in place:" } else { "copy to:" }),
            "{text}"
        );
        assert!(text.contains("runner: test-wine -> test-wine"), "{text}");
        assert!(text.contains("Dry run, nothing was imported."), "{text}");
    }
}

#[test]
fn heroic_wine_game_dry_run_prints_the_game_review_without_writing() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    let heroic = heroic_windows(&f, &prefix, &[("a", "Alpha", "")]);
    let output = dry_run(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--game",
            "a",
            "-n",
            "alpha",
            "--runner",
            "test-wine",
            "--dry-run",
        ],
    );
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("Alpha -> alpha"), "{text}");
    assert!(text.contains("runner  test-wine"), "{text}");
    assert!(text.contains("prefix  "), "{text}");
    assert!(text.contains("Dry run, nothing was imported."), "{text}");
    assert!(!text.contains("Imported alpha."), "{text}");
}

#[test]
fn heroic_native_game_dry_run_prints_the_review_without_writing() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    let (heroic, _) = heroic_with_native(&f, &prefix);
    let output = dry_run(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--game",
            "dusk",
            "-n",
            "dusk",
            "--dry-run",
        ],
    );
    assert_success(&output);
    let text = stdout(&output);
    let row = |label: &str| {
        text.lines()
            .find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some(label)).then(|| words.collect::<Vec<_>>().join(" "))
            })
            .unwrap_or_default()
    };
    assert!(text.contains("Import Dusklight as dusk"), "{text}");
    assert_eq!(row("exe"), "~/games/dusklight", "{text}");
    assert_eq!(row("kind"), "native", "{text}");
    assert!(text.contains("Dry run, nothing was imported."), "{text}");
    assert!(!text.contains("Imported dusk."), "{text}");
}

#[test]
fn heroic_all_dry_run_reports_backfill_and_pending_build_without_a_terminal_or_yes() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    let heroic = heroic_windows(
        &f,
        &prefix,
        &[
            ("a", "Alpha", ""),
            (
                "b",
                "Beta",
                "\"autoInstallDxvk\":true,\"autoInstallVkd3d\":true",
            ),
        ],
    );
    let legacy = f.config_dir().join("environments/legacy.toml");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(
        legacy,
        format!(
            "prefix = {:?}\nrunner = \"test-wine\"\narch = \"win64\"\n",
            prefix.to_str().unwrap()
        ),
    )
    .unwrap();

    let output = dry_run(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--all",
            "--runner",
            "test-wine",
            "--dry-run",
        ],
    );
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("Checking 2 Heroic games..."), "{text}");
    assert!(
        text.contains("Would record the Heroic id for 1 existing environment: legacy"),
        "{text}"
    );
    assert!(
        text.contains("Would offer to download the latest DXVK for 1 game. Declining skips it."),
        "{text}"
    );
    assert!(
        text.contains(
            "Would offer to download the latest vkd3d-proton for 1 game. Declining skips it."
        ),
        "{text}"
    );
    assert!(text.contains("Dry run, nothing was imported."), "{text}");
    assert!(!text.contains("Imported"), "{text}");
}

#[test]
fn heroic_all_without_proton_dry_run_reports_native_only_planning() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    let (heroic, _) = heroic_with_native(&f, &prefix);
    let output = dry_run(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--all",
            "--dry-run",
        ],
    );
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains("Would offer to download the latest GE-Proton."),
        "{text}"
    );
    assert!(
        text.contains("1 Windows game not shown: it needs a Proton runner, which wineo would offer to download. Pass --runner to preview it."),
        "{text}"
    );
    assert!(text.contains("Checking 2 Heroic games..."), "{text}");
    assert!(text.contains("Import 1 Heroic game:"), "{text}");
    assert!(text.contains("Dusklight"), "{text}");
    assert!(!text.contains("Alpha"), "{text}");
    assert!(text.contains("Dry run, nothing was imported."), "{text}");
}

#[test]
fn incomplete_dry_runs_print_the_same_needs_message_as_noninteractive_runs() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    let heroic = heroic_windows(&f, &prefix, &[("a", "Alpha", "")]);

    let prefix_dry = dry_run(
        &f,
        &["import", "prefix", prefix.to_str().unwrap(), "--dry-run"],
    );
    let prefix_real = run(&f, &["import", "prefix", prefix.to_str().unwrap()]);
    assert_eq!(prefix_dry.status.code(), Some(1));
    assert_eq!(stderr(&prefix_dry), stderr(&prefix_real));
    assert!(stderr(&prefix_dry).contains("specify a source, -n <name>, and --runner"));

    let heroic_dry = dry_run(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--game",
            "a",
            "--dry-run",
        ],
    );
    let heroic_real = run(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--game",
            "a",
        ],
    );
    assert_eq!(heroic_dry.status.code(), Some(1));
    assert_eq!(stderr(&heroic_dry), stderr(&heroic_real));
    assert!(stderr(&heroic_dry).contains("specify --game, -n, and --runner"));
}

#[test]
fn dry_run_conflicts_with_interactive_and_list() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    let prefix_error = dry_run(
        &f,
        &[
            "import",
            "prefix",
            prefix.to_str().unwrap(),
            "-n",
            "sample",
            "--runner",
            "test-wine",
            "--interactive",
            "--dry-run",
        ],
    );
    assert!(!prefix_error.status.success());
    assert!(stderr(&prefix_error).contains("cannot be used with"));

    let heroic = heroic_windows(&f, &prefix, &[("a", "Alpha", "")]);
    let heroic_error = dry_run(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--list",
            "--dry-run",
        ],
    );
    assert!(!heroic_error.status.success());
    assert!(stderr(&heroic_error).contains("cannot be used with"));
}

fn assert_dry_run_error_matches_real(f: &Fixture, dry_args: &[&str], real_args: &[&str]) -> String {
    let output = dry_run(f, dry_args);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let real = run(f, real_args);
    assert_eq!(real.status.code(), Some(1), "{}", stderr(&real));
    assert_eq!(stderr(&output), stderr(&real));
    stderr(&output)
}

#[test]
fn dry_run_errors_match_real_import_errors() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    f.environment("taken", "");
    let taken = assert_dry_run_error_matches_real(
        &f,
        &[
            "import",
            "prefix",
            prefix.to_str().unwrap(),
            "-n",
            "taken",
            "--runner",
            "test-wine",
            "--dry-run",
        ],
        &[
            "import",
            "prefix",
            prefix.to_str().unwrap(),
            "-n",
            "taken",
            "--runner",
            "test-wine",
        ],
    );
    assert!(taken.contains("taken"), "{taken}");

    let unknown = assert_dry_run_error_matches_real(
        &f,
        &[
            "import",
            "prefix",
            prefix.to_str().unwrap(),
            "-n",
            "unknown-runner",
            "--runner",
            "not-installed",
            "--dry-run",
        ],
        &[
            "import",
            "prefix",
            prefix.to_str().unwrap(),
            "-n",
            "unknown-runner",
            "--runner",
            "not-installed",
        ],
    );
    assert!(unknown.contains("not-installed"), "{unknown}");

    let missing = f.root.path().join("missing prefix");
    let missing_error = assert_dry_run_error_matches_real(
        &f,
        &[
            "import",
            "prefix",
            missing.to_str().unwrap(),
            "-n",
            "missing-source",
            "--runner",
            "test-wine",
            "--dry-run",
        ],
        &[
            "import",
            "prefix",
            missing.to_str().unwrap(),
            "-n",
            "missing-source",
            "--runner",
            "test-wine",
        ],
    );
    assert!(
        missing_error.to_lowercase().contains("missing prefix"),
        "{missing_error}"
    );

    let heroic = heroic_windows(&f, &prefix, &[("a", "Alpha", "")]);
    let unknown_game = assert_dry_run_error_matches_real(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--game",
            "not-installed",
            "-n",
            "unknown-game",
            "--runner",
            "test-wine",
            "--dry-run",
        ],
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--game",
            "not-installed",
            "-n",
            "unknown-game",
            "--runner",
            "test-wine",
        ],
    );
    assert!(
        unknown_game.contains("game id is not in sideload library"),
        "{unknown_game}"
    );
}

#[test]
fn heroic_art_without_a_key_is_a_read_only_skip_and_empty_fetch_kinds_skip_art() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    let heroic = heroic_windows(&f, &prefix, &[("a", "Alpha", "")]);
    let output = dry_run(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--game",
            "a",
            "-n",
            "alpha",
            "--runner",
            "test-wine",
            "--art",
            "--dry-run",
        ],
    );
    assert_success(&output);
    assert!(
        format!("{}{}", stdout(&output), stderr(&output))
            .contains("note: run `wineo art key` to fetch art from SteamGridDB"),
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    assert!(stdout(&output).contains("Dry run, nothing was imported."));

    f.global_config("[art]\nfetch = []\n");
    let no_kinds = dry_run(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--game",
            "a",
            "-n",
            "alpha",
            "--runner",
            "test-wine",
            "--art",
            "--dry-run",
        ],
    );
    assert_success(&no_kinds);
    assert!(!format!("{}{}", stdout(&no_kinds), stderr(&no_kinds)).contains("SteamGridDB"));
    assert!(stdout(&no_kinds).contains("Dry run, nothing was imported."));
}

#[test]
fn heroic_art_with_a_saved_key_warns_on_search_failure_without_writing() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    let heroic = heroic_windows(&f, &prefix, &[("a", "Alpha", "")]);
    let key = f.root.path().join("data/wineo/steamgriddb.key");
    fs::create_dir_all(key.parent().unwrap()).unwrap();
    fs::write(&key, "test-key\n").unwrap();

    let output = dry_run_with(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--game",
            "a",
            "-n",
            "alpha",
            "--runner",
            "test-wine",
            "--art",
            "--dry-run",
        ],
        |command| {
            command
                .env("HTTPS_PROXY", "http://127.0.0.1:9")
                .env("https_proxy", "http://127.0.0.1:9")
                .env("NO_PROXY", "")
                .env("no_proxy", "")
                .env_remove("ALL_PROXY")
                .env_remove("all_proxy");
        },
    );
    assert_success(&output);
    assert!(
        stderr(&output).contains("warning: art fetch failed:"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).contains("Dry run, nothing was imported."));
    assert!(key.is_file());
    assert!(!f.root.path().join("data/wineo/art").exists());
}

#[test]
fn heroic_all_with_yes_and_no_proton_fails_like_the_real_run_without_writing() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    let (heroic, _) = heroic_with_native(&f, &prefix);
    let args = [
        "import",
        "heroic",
        "--config-dir",
        heroic.to_str().unwrap(),
        "--all",
        "--yes",
    ];
    let mut dry_args = args.to_vec();
    dry_args.push("--dry-run");
    let error = assert_dry_run_error_matches_real(&f, &dry_args, &args);
    assert!(
        error.contains("no Proton runner is installed; pass --runner or download one first"),
        "{error}"
    );
}

#[test]
fn heroic_all_without_proton_reports_multiple_hidden_windows_games() {
    let f = Fixture::new();
    install_guarded_wine(&f);
    let prefix = source(f.root.path());
    let heroic = heroic_windows(&f, &prefix, &[("a", "Alpha", ""), ("b", "Beta", "")]);
    let output = dry_run(
        &f,
        &[
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--all",
            "--dry-run",
        ],
    );
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains("Would offer to download the latest GE-Proton."),
        "{text}"
    );
    assert!(text.contains("2 Windows games not shown: they need a Proton runner, which wineo would offer to download. Pass --runner to preview them."), "{text}");
    assert!(
        text.find("Import ").unwrap() < text.find("not shown").unwrap(),
        "the hidden games note follows the table: {text}"
    );
    assert!(text.contains("Checking 2 Heroic games..."), "{text}");
    assert!(text.contains("Dry run, nothing was imported."), "{text}");
}

#[test]
fn heroic_all_pending_builds_follow_yes_and_pluralize_offers() {
    for count in [1, 2] {
        for yes in [false, true] {
            let f = Fixture::new();
            install_guarded_wine(&f);
            let prefix = source(f.root.path());
            let games = [
                (
                    "a",
                    "Alpha",
                    "\"autoInstallDxvk\":true,\"autoInstallVkd3d\":true",
                ),
                (
                    "b",
                    "Beta",
                    "\"autoInstallDxvk\":true,\"autoInstallVkd3d\":true",
                ),
            ];
            let heroic = heroic_windows(&f, &prefix, &games[..count]);
            let mut args = vec![
                "import",
                "heroic",
                "--config-dir",
                heroic.to_str().unwrap(),
                "--all",
                "--runner",
                "test-wine",
                "--dry-run",
            ];
            if yes {
                args.push("--yes");
            }
            let output = dry_run(&f, &args);
            assert_success(&output);
            let text = stdout(&output);
            assert!(
                text.contains(&format!("Checking {count} Heroic games...")),
                "{text}"
            );
            assert!(
                text.contains("skip: no dxvk builds are installed"),
                "{text}"
            );
            let noun = if count == 1 { "game" } else { "games" };
            assert!(
                text.contains(&format!("Import {count} Heroic {noun} with test-wine:")),
                "{text}"
            );
            if yes {
                assert!(!text.contains("Would offer"), "{text}");
                assert!(!text.contains("Would download"), "{text}");
                let real_args: Vec<_> = args
                    .iter()
                    .copied()
                    .filter(|arg| *arg != "--dry-run")
                    .collect();
                let before = Snapshot::capture(f.root.path()).unwrap();
                let real = run(&f, &real_args);
                assert_success(&real);
                before.assert_unchanged(f.root.path());
                assert!(!f.root.path().join("runner-started").exists());
                assert_eq!(
                    stdout(&real),
                    text.replace(
                        "Dry run, nothing was imported.",
                        "No Heroic games are ready to import."
                    )
                );
            } else {
                let target = if count == 1 { "it" } else { "them" };
                for component in ["DXVK", "vkd3d-proton"] {
                    assert!(text.contains(&format!("Would offer to download the latest {component} for {count} {noun}. Declining skips {target}.")), "{text}");
                }
            }
            assert!(text.contains("Dry run, nothing was imported."), "{text}");
        }
    }
}

#[test]
fn heroic_all_conflicting_destination_is_fail_soft_with_and_without_yes() {
    for yes in [false, true] {
        let f = Fixture::new();
        install_guarded_wine(&f);
        let prefix = source(f.root.path());
        let heroic = heroic_windows(&f, &prefix, &[("a", "Alpha", ""), ("b", "Beta", "")]);
        let destination = f.root.path().join("data/wineo/prefixes/alpha");
        fs::create_dir_all(&destination).unwrap();
        let mut args = vec![
            "import",
            "heroic",
            "--config-dir",
            heroic.to_str().unwrap(),
            "--all",
            "--runner",
            "test-wine",
            "--dry-run",
        ];
        if yes {
            args.push("--yes");
        }
        let output = dry_run(&f, &args);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        let text = stdout(&output);
        assert!(text.contains("Checking 2 Heroic games..."), "{text}");
        assert!(
            text.contains(&format!(
                "alpha: would fail: import destination already exists: {}",
                destination.display()
            )),
            "{text}"
        );
        assert!(text.contains("Beta"), "{text}");
        assert!(text.ends_with("Dry run, nothing was imported.\n"), "{text}");
    }
}
