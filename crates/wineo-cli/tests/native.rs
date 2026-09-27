//! Launching and stopping native environments, whose targets are Linux
//! programs run directly with no prefix and no runner.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use common::*;

/// Prints its arguments, a variable, and any WINE variables it was given,
/// then runs until asked to close.
const GAME: &str = r#"#!/bin/sh
echo "game: $*"
echo "game: FAKE_VAR=${FAKE_VAR-} WINEPREFIX=${WINEPREFIX-unset} cwd=$(pwd)"
env | while IFS= read -r line; do
    case "$line" in WINE*) echo "game: wine var $line" ;; esac
done
touch "$0.running"
trap 'rm -f "$0.running"; exit 0' TERM
while :; do sleep 0.1; done
"#;

/// Exits right away and leaves a child running in its process group, the way
/// a launcher hands off to the game.
const HANDOFF: &str = r#"#!/bin/sh
(sleep 2; touch "$0.done") &
exit 0
"#;

/// Ignores a request to close, so only SIGKILL ends it.
const STUBBORN: &str = r#"#!/bin/sh
trap '' TERM
touch "$0.running"
while :; do sleep 0.1; done
"#;

const EXIT_3: &str = "#!/bin/sh\nexit 3\n";

impl NativeFixture for Fixture {
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let dir = self.root.path().join("games");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn native(&self, name: &str, text: &str) {
        let config = self.config_dir().join(format!("environments/{name}.toml"));
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(config, format!("kind = \"native\"\n{text}")).unwrap();
    }
}

trait NativeFixture {
    fn script(&self, name: &str, body: &str) -> PathBuf;
    fn native(&self, name: &str, text: &str);
}

fn target(exe: &std::path::Path) -> String {
    format!("[targets.game]\nexe = \"{}\"\n", exe.display())
}

fn marker(exe: &std::path::Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", exe.display()))
}

#[test]
fn start_runs_a_native_program_and_status_shows_it_running() {
    let f = Fixture::new();
    let game = f.script("game.sh", GAME);
    f.native(
        "dusk",
        &format!(
            "[env]\nFAKE_VAR = \"env\"\nWINEPREFIX = \"/mine\"\n\n{}args = [\"-a\"]\n",
            target(&game)
        ),
    );

    let started = f.run(&["start", "dusk", "-e", "FAKE_VAR=cli", "--", "-b"]);
    assert_success(&started);
    assert_eq!(stdout(&started), "Started dusk\n");
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });

    let state = f.state("dusk");
    assert!(state.contains("status = \"running\""), "{state}");
    assert!(state.contains("native = true"), "{state}");
    assert!(!state.contains("runner"), "{state}");
    let shown = f.run(&["status", "dusk"]);
    assert_success(&shown);
    assert!(
        stdout(&shown).starts_with("dusk: running since"),
        "{}",
        stdout(&shown)
    );

    let stopped = f.run(&["stop", "dusk"]);
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "Stopped dusk\n");
    assert!(!marker(&game, ".running").exists());
    assert!(f.state("dusk").contains("status = \"stopped\""));

    let log = f.log("dusk");
    assert!(log.contains("game: -a -b"), "{log}");
    let dir = game.parent().unwrap().canonicalize().unwrap();
    assert!(
        log.contains(&format!(
            "FAKE_VAR=cli WINEPREFIX=/mine cwd={}",
            dir.display()
        )),
        "{log}"
    );
    // Only the WINEPREFIX the config set, and whatever wineo itself inherited.
    let inherited: Vec<String> = std::env::vars()
        .filter(|(key, _)| key.starts_with("WINE"))
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    for var in log
        .lines()
        .filter_map(|line| line.strip_prefix("game: wine var "))
    {
        assert!(
            var == "WINEPREFIX=/mine" || inherited.iter().any(|own| own == var),
            "{log}"
        );
    }
}

#[test]
fn a_handoff_keeps_the_run_going_until_its_child_exits() {
    let f = Fixture::new();
    let launcher = f.script("launcher.sh", HANDOFF);
    f.native("dusk", &target(&launcher));

    assert_success(&f.run(&["start", "dusk"]));
    wait_for("the launcher to exit", || {
        f.state("dusk").contains("program_exited_at")
    });
    // The launcher is gone, but its child still holds the run.
    let state = f.state("dusk");
    assert!(state.contains("status = \"running\""), "{state}");
    assert!(!marker(&launcher, ".done").exists());
    assert!(f.runtime("dusk").is_running().unwrap());
    let shown = stdout(&f.run(&["status", "dusk"]));
    assert!(
        shown.contains("other processes are keeping dusk running"),
        "{shown}"
    );

    wait_for("the run to end", || {
        f.state("dusk").contains("status = \"exited\"")
    });
    assert!(marker(&launcher, ".done").exists());
}

#[test]
fn stop_kills_a_native_program_that_ignores_a_request_to_close() {
    let f = Fixture::new();
    f.global_config("stop_timeout = 1\n");
    let game = f.script("stubborn.sh", STUBBORN);
    f.native("dusk", &target(&game));
    assert_success(&f.run(&["start", "dusk"]));
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });

    let began = Instant::now();
    assert_success(&f.run(&["stop", "dusk"]));
    assert!(began.elapsed() >= Duration::from_secs(1));
    assert!(!f.run_group_alive("dusk"));
    let log = f.log("dusk");
    assert!(log.contains("wineo: stopping"), "{log}");
    assert!(
        log.contains("wineo: killing everything left in the process group"),
        "{log}"
    );
    assert!(f.state("dusk").contains("status = \"stopped\""));
}

#[test]
fn stop_cleans_up_a_lost_native_run_through_its_group() {
    let f = Fixture::new();
    let game = f.script("game.sh", GAME);
    f.native("dusk", &target(&game));
    assert_success(&f.run(&["start", "dusk"]));
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });

    let supervisor = f.runtime("dusk").holder_pid().unwrap().unwrap();
    assert!(
        Command::new("kill")
            .args(["-KILL", &supervisor.to_string()])
            .status()
            .unwrap()
            .success()
    );
    wait_for("the lock to be released", || {
        !f.runtime("dusk").is_running().unwrap()
    });
    assert!(f.runtime("dusk").status().unwrap().lost);
    assert!(f.run_group_alive("dusk"));

    let stopped = f.run(&["stop", "dusk"]);
    assert_success(&stopped);
    assert!(
        stderr(&stopped).contains("killed anything left behind"),
        "{}",
        stderr(&stopped)
    );
    wait_for("the process group to go", || !f.run_group_alive("dusk"));
    assert!(f.state("dusk").contains("status = \"stopped\""));
}

#[test]
fn gamemode_wraps_a_native_program() {
    let f = Fixture::new();
    f.global_config("gamemode = true\n");
    let game = f.script("game.sh", GAME);
    f.native("dusk", &target(&game));

    assert_success(&f.run(&["start", "dusk"]));
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });
    assert_success(&f.run(&["stop", "dusk"]));
    let log = f.log("dusk");
    assert!(
        log.contains(&format!("gamemoderun wrapped: {}", game.display())),
        "{log}"
    );
    assert!(log.contains("game: "), "{log}");
}

#[test]
fn run_and_restart_work_for_native() {
    let f = Fixture::new();
    let failing = f.script("fail.sh", EXIT_3);
    f.native("fail", &target(&failing));
    let ran = f.run(&["run", "fail"]);
    assert_eq!(ran.status.code(), Some(3), "{}", stderr(&ran));
    assert!(f.state("fail").contains("status = \"failed\""));

    let game = f.script("game.sh", GAME);
    f.native("dusk", &target(&game));
    let restarted = f.run(&["restart", "dusk"]);
    assert_success(&restarted);
    assert_eq!(stdout(&restarted), "Started dusk\n");
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });
    let restarted = f.run(&["restart", "dusk"]);
    assert_success(&restarted);
    assert_eq!(stdout(&restarted), "Restarted dusk\n");
    assert_success(&f.run(&["stop", "dusk"]));
}

#[test]
fn start_refuses_a_bad_native_target_before_anything_runs() {
    let f = Fixture::new();
    let plain = f.root.path().join("games/plain.sh");
    fs::create_dir_all(plain.parent().unwrap()).unwrap();
    fs::write(&plain, "#!/bin/sh\n").unwrap();
    f.native("dusk", &target(&plain));
    let refused = f.run(&["start", "dusk"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("is not executable. Run chmod +x on it."),
        "{}",
        stderr(&refused)
    );

    f.native("dusk", "[targets.game]\nexe = 'C:\\Games\\game.exe'\n");
    let refused = f.run(&["start", "dusk"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("dusk is native, so its target can't be a Windows program"),
        "{}",
        stderr(&refused)
    );
    assert!(f.state("dusk").is_empty());
}

#[test]
fn a_wineprefix_override_is_only_refused_for_wine() {
    let f = Fixture::new();
    f.environment("app", "[targets.app]\nexe = \"app\"\n");
    let refused = f.run(&["start", "app", "-e", "WINEPREFIX=/elsewhere"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("WINEPREFIX is managed by wineo"),
        "{}",
        stderr(&refused)
    );
    assert!(f.state("app").is_empty());

    let game = f.script("game.sh", GAME);
    f.native("dusk", &target(&game));
    assert_success(&f.run(&["start", "dusk", "-e", "WINEPREFIX=/cli"]));
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });
    assert_success(&f.run(&["stop", "dusk"]));
    assert!(
        f.log("dusk").contains("WINEPREFIX=/cli"),
        "{}",
        f.log("dusk")
    );
}

#[test]
fn a_bare_name_is_found_on_the_path_the_game_gets() {
    let f = Fixture::new();
    let game = f.script("dusklight", GAME);
    let dir = game.parent().unwrap();
    f.native(
        "dusk",
        &format!(
            "[env]\nPATH = \"{}:/usr/bin:/bin\"\n\n[targets.game]\nexe = \"dusklight\"\n",
            dir.display()
        ),
    );
    assert_success(&f.run(&["start", "dusk"]));
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });
    assert_success(&f.run(&["stop", "dusk"]));
}

#[test]
fn status_shows_the_kind_in_place_of_runner_and_prefix() {
    let f = Fixture::new();
    let game = f.script("game.sh", GAME);
    f.native("dusk", &format!("gamemode = false\n{}", target(&game)));
    f.environment("app", "[targets.app]\nexe = \"app\"\n");

    let shown = f.run(&["status", "dusk"]);
    assert_success(&shown);
    assert_eq!(stdout(&shown), "dusk: inactive\n  kind      native\n");

    let json: serde_json::Value =
        serde_json::from_str(&stdout(&f.run(&["status", "dusk", "--json"]))).unwrap();
    assert_eq!(json["kind"], "native");
    assert!(json["runner"].is_null(), "{json}");
    assert!(json["prefix"].is_null(), "{json}");

    let wine: serde_json::Value =
        serde_json::from_str(&stdout(&f.run(&["status", "app", "--json"]))).unwrap();
    assert_eq!(wine["kind"], "wine");
    assert_eq!(wine["runner"]["configured"], "wine");
    assert!(wine["prefix"].is_string(), "{wine}");
    assert!(!stdout(&f.run(&["status", "app"])).contains("kind"));
}

#[test]
fn set_refuses_what_a_native_environment_cant_have() {
    let f = Fixture::new();
    let game = f.script("game.sh", GAME);
    f.native("dusk", &target(&game));
    let before = f.created_config("dusk");
    for (key, value) in [
        ("prefix", "/p"),
        ("runner", "wine"),
        ("arch", "win64"),
        ("gameid", "umu-1"),
        ("store", "gog"),
        ("dxvk", "latest"),
        ("vkd3d", "latest"),
    ] {
        let refused = f.run(&["set", "dusk", &format!("{key}={value}")]);
        assert!(!refused.status.success(), "{key}");
        assert_eq!(
            stderr(&refused),
            format!("error: native environment dusk can't set {key}\n")
        );
    }

    for args in [
        &["set", "dusk", "kind=wine"][..],
        &["set", "dusk", "--unset", "kind"][..],
    ] {
        let refused = f.run(args);
        assert!(!refused.status.success());
        assert_eq!(
            stderr(&refused),
            "error: dusk's kind can't be changed, make a new environment instead\n"
        );
    }
    f.environment("app", "[targets.app]\nexe = \"app\"\n");
    let refused = f.run(&["set", "app", "kind=native"]);
    assert_eq!(
        stderr(&refused),
        "error: app's kind can't be changed, make a new environment instead\n"
    );
    assert_eq!(f.created_config("dusk"), before);
}

#[test]
fn set_works_for_native_and_checks_the_exes_it_writes() {
    let f = Fixture::new();
    let game = f.script("game.sh", GAME);
    f.native("dusk", &target(&game));
    assert_success(&f.run(&["set", "dusk", "gamemode=true", "env.WINEPREFIX=/mine"]));
    let text = f.created_config("dusk");
    assert!(text.contains("gamemode = true"), "{text}");
    assert!(text.contains("WINEPREFIX = \"/mine\""), "{text}");

    let missing = f.root.path().join("games/missing.sh");
    let refused = f.run(&[
        "set",
        "dusk",
        &format!("targets.other.exe={}", missing.display()),
    ]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("does not exist"),
        "{}",
        stderr(&refused)
    );

    let refused = f.run(&["set", "dusk", "targets.other.exe=game.exe"]);
    assert!(
        stderr(&refused).contains("dusk is native, so its target can't be a Windows program"),
        "{}",
        stderr(&refused)
    );
    let refused = f.run(&[
        "set",
        "dusk",
        "targets.other.exe=wineo-surely-not-installed",
    ]);
    assert!(
        stderr(&refused).contains("was not found on PATH"),
        "{}",
        stderr(&refused)
    );

    let other = f.script("other.sh", GAME);
    assert_success(&f.run(&[
        "set",
        "dusk",
        &format!("targets.other.exe={}", other.display()),
    ]));
    assert!(f.created_config("dusk").contains("[targets.other]"));

    // Removing the last target leaves a valid config that `start` explains.
    assert_success(&f.run(&[
        "set",
        "dusk",
        "--unset",
        "targets.game",
        "--unset",
        "targets.other",
    ]));
    let refused = f.run(&["start", "dusk"]);
    assert!(
        stderr(&refused).contains("environment `dusk` has no targets"),
        "{}",
        stderr(&refused)
    );
}

/// A 1x1 PNG that decodes.
const REAL_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\rIDAT\x78\x9c\x63\xf8\xcf\xc0\xf0\x1f\0\x05\0\x01\xff\x89\x99\x3d\x1d\0\0\0\0IEND\xae\x42\x60\x82";

#[test]
fn desktop_entries_work_for_native_with_icons_from_art_only() {
    let f = Fixture::new();
    let game = f.script("game.sh", GAME);
    f.native("dusk", &target(&game));
    let entry = f
        .root
        .path()
        .join("data/applications/wineo-dusk-game.desktop");

    let set = f.run(&["set", "dusk", "targets.game.desktop=true"]);
    assert_success(&set);
    assert_eq!(
        stderr(&set),
        "Added a desktop entry for game in dusk, with no icon\n"
    );
    let text = fs::read_to_string(&entry).unwrap();
    assert!(
        text.contains("Exec=wineo start dusk --target game\n"),
        "{text}"
    );
    assert!(text.contains("Categories=Utility;Software;\n"), "{text}");
    assert!(!text.contains("Icon="), "{text}");
    assert!(!text.contains("StartupWMClass"), "{text}");

    assert_success(&f.run(&["set", "dusk", "gamemode=true"]));
    let text = fs::read_to_string(&entry).unwrap();
    assert!(text.contains("Categories=Game;\n"), "{text}");

    let png = f.root.path().join("icon.png");
    fs::write(&png, REAL_PNG).unwrap();
    let set = f.run(&["art", "set", "dusk", "icon", png.to_str().unwrap()]);
    assert_success(&set);
    assert_eq!(stderr(&set), "Updated the desktop entry for game in dusk\n");
    let text = fs::read_to_string(&entry).unwrap();
    assert!(
        text.contains("\nIcon=wineo-dusk-game\nX-Wineo-Icon=art:"),
        "{text}"
    );
    assert!(
        f.root
            .path()
            .join("data/icons/hicolor/16x16/apps/wineo-dusk-game.png")
            .is_file()
    );

    let listed = f.run(&["desktop"]);
    assert_success(&listed);
    assert!(stdout(&listed).contains("dusk"), "{}", stdout(&listed));

    // A start reconciles like any other launch, and a clean one says nothing.
    let started = f.run(&["start", "dusk"]);
    assert_success(&started);
    assert_eq!(stderr(&started), "");
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });
    assert_success(&f.run(&["stop", "dusk"]));

    let refused = f.run(&[
        "set",
        "dusk",
        "targets.other.exe=/nonexistent/game",
        "targets.other.desktop=true",
    ]);
    assert!(!refused.status.success());
    assert!(
        !f.root
            .path()
            .join("data/applications/wineo-dusk-other.desktop")
            .exists()
    );

    assert_success(&f.run(&["rm", "dusk"]));
    assert!(!entry.exists());
}

#[test]
fn new_native_writes_one_default_target_named_from_the_environment() {
    let f = Fixture::new();
    let game = f.script("dusklight", GAME);
    let created = f.run(&[
        "new",
        "-n",
        "dusk",
        "--native",
        "--exe",
        game.to_str().unwrap(),
    ]);
    assert_success(&created);
    assert_eq!(
        stdout(&created),
        "Created native environment dusk with target dusk\n"
    );
    assert_eq!(
        f.created_config("dusk"),
        format!(
            "schema_version = 1\nkind = \"native\"\n\n[targets.dusk]\nexe = \"{}\"\ndefault = true\n",
            game.display()
        )
    );
    assert_success(&f.run(&["start", "dusk"]));
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });
    assert_success(&f.run(&["stop", "dusk"]));

    // A bare name is looked up on PATH when it is written.
    let dir = game.parent().unwrap();
    let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
    let created = f
        .wineo()
        .env("PATH", path)
        .args(["new", "-n", "bare", "--native", "--exe", "dusklight"])
        .output()
        .unwrap();
    assert_success(&created);
    assert!(f.created_config("bare").contains("exe = \"dusklight\""));

    let refused = f.run(&[
        "new",
        "-n",
        "missing",
        "--native",
        "--exe",
        "wineo-surely-not-installed",
    ]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("`wineo-surely-not-installed` was not found on PATH"),
        "{}",
        stderr(&refused)
    );
    let refused = f.run(&["new", "-n", "win", "--native", "--exe", "setup.exe"]);
    assert!(
        stderr(&refused).contains("win is native, so its target can't be a Windows program"),
        "{}",
        stderr(&refused)
    );
    assert!(f.created_config("missing").is_empty());
    assert!(f.created_config("win").is_empty());

    let taken = f.run(&[
        "new",
        "-n",
        "dusk",
        "--native",
        "--exe",
        game.to_str().unwrap(),
    ]);
    assert!(!taken.status.success());
    assert!(
        stderr(&taken).contains("already exists"),
        "{}",
        stderr(&taken)
    );
}

#[test]
fn new_native_alone_gives_an_empty_environment_and_the_hint() {
    let f = Fixture::new();
    let created = f.run(&["new", "-n", "dusk", "--native"]);
    assert_success(&created);
    assert_eq!(
        stdout(&created),
        "Created native environment dusk\nAdd a target with: wineo setup dusk\n"
    );
    assert_eq!(
        f.created_config("dusk"),
        "schema_version = 1\nkind = \"native\"\n"
    );
}

#[test]
fn new_native_refuses_a_value_the_old_syntax_gave_it() {
    let f = Fixture::new();
    let game = f.script("dusklight", GAME);
    let refused = f.run(&["new", "-n", "dusk", "--native", game.to_str().unwrap()]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("unexpected argument"),
        "{}",
        stderr(&refused)
    );
    assert!(f.created_config("dusk").is_empty());
}

#[test]
fn new_native_with_desktop_writes_an_entry_for_the_exe_target() {
    let f = Fixture::new();
    let game = f.script("dusklight", GAME);
    let created = f.run(&[
        "new",
        "-n",
        "dusk",
        "--native",
        "--exe",
        game.to_str().unwrap(),
        "-d",
    ]);
    assert_success(&created);
    assert!(
        f.root
            .path()
            .join("data/applications/wineo-dusk-dusk.desktop")
            .is_file()
    );
}

#[test]
fn new_native_refuses_the_wine_only_flags() {
    let f = Fixture::new();
    let game = f.script("dusklight", GAME);
    let exe = game.to_str().unwrap();
    for flags in [
        &["--runner", "wine"][..],
        &["--arch", "win64"][..],
        &["--dir", "/tmp/x"][..],
        &["--template", "gaming"][..],
        &["--install-exe", "/tmp/setup.exe"][..],
    ] {
        let refused = f
            .wineo()
            .args(["new", "-n", "dusk", "--native", "--exe", exe])
            .args(flags)
            .output()
            .unwrap();
        assert!(!refused.status.success(), "{flags:?}");
        assert!(
            stderr(&refused).contains("cannot be used with"),
            "{flags:?}: {}",
            stderr(&refused)
        );
    }
    assert!(f.created_config("dusk").is_empty());
}

#[test]
fn clone_copies_a_native_config_and_art_with_desktop_entries_off() {
    let f = Fixture::new();
    let game = f.script("game.sh", GAME);
    f.native(
        "dusk",
        &format!("heroic_id = \"abc\"\n{}desktop = true\n", target(&game)),
    );
    assert_success(&f.run(&["set", "dusk", "gamemode=true"]));
    let png = f.root.path().join("icon.png");
    fs::write(&png, REAL_PNG).unwrap();
    assert_success(&f.run(&["art", "set", "dusk", "icon", png.to_str().unwrap()]));

    let refused = f.run(&["clone", "dusk", "copy", "--dir", "/tmp/elsewhere"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("dusk is a native environment, so it has no prefix"),
        "{}",
        stderr(&refused)
    );

    let cloned = f.run(&["clone", "dusk", "copy"]);
    assert_success(&cloned);
    assert_eq!(
        stdout(&cloned),
        "Clone dusk as copy\n  kind: native\n  targets: game\n  art: copied\n  change: heroic_id removed\n  change: targets.game.desktop disabled\nCloned dusk as copy\n"
    );
    let config = f.created_config("copy");
    assert!(config.contains("kind = \"native\""), "{config}");
    assert!(config.contains("gamemode = true"), "{config}");
    assert!(config.contains("desktop = false"), "{config}");
    assert!(!config.contains("heroic_id"), "{config}");
    assert!(
        f.root
            .path()
            .join("data/wineo/art/copy")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
    assert!(
        !f.root
            .path()
            .join("data/applications/wineo-copy-game.desktop")
            .exists()
    );
    let taken = f.run(&["clone", "dusk", "copy"]);
    assert!(!taken.status.success());
}

#[test]
fn portable_copy_takes_a_native_config_and_lists_host_paths() {
    let f = Fixture::new();
    let game = f.script("game.sh", GAME);
    f.native(
        "dusk",
        &format!(
            "{}desktop = true\n\n[targets.bare]\nexe = \"sh\"\n",
            target(&game)
        ),
    );
    f.global_config("gamemode = false\n");
    let dir = f.root.path().join("usb");
    let copied = f.run(&["portable", "copy", "dusk", "--to", dir.to_str().unwrap()]);
    assert_success(&copied);
    let out = stdout(&copied);
    assert!(out.starts_with("Copy dusk to "), "{out}");
    assert!(out.contains("  kind: native\n"), "{out}");
    assert!(out.contains("  targets: bare, game\n"), "{out}");
    assert!(
        out.contains("  change: targets.game.desktop disabled\n"),
        "{out}"
    );
    assert!(
        out.contains("  global config: copied, prefix_root follows the folder\n"),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "  external target path: targets.game.exe: {} (kept in place)\n",
            game.display()
        )),
        "{out}"
    );
    assert!(!out.contains("targets.bare.exe"), "{out}");
    assert!(
        out.ends_with(&format!("Copied dusk to {}\n", dir.display())),
        "{out}"
    );

    let portable = |args: &[&str]| {
        f.wineo()
            .arg("--portable")
            .arg(&dir)
            .args(args)
            .output()
            .unwrap()
    };
    let config = stdout(&portable(&["config", "dusk"]));
    assert!(config.contains("kind = \"native\""), "{config}");
    assert!(
        config.contains(&format!("exe = \"{}\"", game.display())),
        "{config}"
    );
    assert!(!dir.join("prefixes").exists());
    assert_success(&portable(&["start", "dusk", "--target", "game"]));
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });
    assert_success(&portable(&["stop", "dusk"]));

    let again = f.run(&["portable", "copy", "dusk", "--to", dir.to_str().unwrap()]);
    assert!(!again.status.success());
}

#[test]
fn prefix_commands_refuse_a_native_environment() {
    let f = Fixture::new();
    let game = f.script("game.sh", GAME);
    f.native("dusk", &target(&game));
    for args in [
        &["exec", "dusk", "winecfg"][..],
        &["winetricks", "dusk", "corefonts"][..],
        &["programs", "dusk"][..],
        &["snapshot", "create", "dusk", "clean"][..],
        &["snapshot", "list", "dusk"][..],
        &["snapshot", "restore", "dusk", "clean"][..],
    ] {
        let refused = f.run(args);
        assert!(!refused.status.success(), "{args:?}");
        assert_eq!(
            stderr(&refused),
            "error: dusk is a native environment, so it has no prefix\n",
            "{args:?}"
        );
    }

    let refused = f.run(&["rm", "dusk", "--delete-prefix"]);
    assert!(!refused.status.success());
    assert_eq!(
        stderr(&refused),
        "error: dusk has no prefix. Run wineo rm dusk\n"
    );
    assert!(!f.created_config("dusk").is_empty());
}

#[test]
fn list_logs_and_rm_work_for_native() {
    let f = Fixture::new();
    let game = f.script("game.sh", GAME);
    f.native("dusk", &target(&game));
    assert_success(&f.run(&["start", "dusk", "--", "-hello"]));
    wait_for("the program to start", || {
        marker(&game, ".running").exists()
    });
    let listed = stdout(&f.run(&["list"]));
    assert!(listed.starts_with("dusk"), "{listed}");
    assert!(listed.contains("running"), "{listed}");
    assert_success(&f.run(&["stop", "dusk"]));
    let logs = f.run(&["logs", "dusk"]);
    assert_success(&logs);
    assert!(stdout(&logs).contains("game: -hello"), "{}", stdout(&logs));

    let removed = f.run(&["rm", "dusk"]);
    assert_success(&removed);
    assert!(stdout(&removed).contains("Removed environment dusk"));
    assert!(stdout(&removed).contains("wineo trash restore "));
    assert!(f.created_config("dusk").is_empty());
    assert!(game.exists());
}
