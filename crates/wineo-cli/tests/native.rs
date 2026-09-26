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
