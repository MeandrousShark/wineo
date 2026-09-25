//! Building the prefix of a hand-written config from the commands that hand
//! it to wine, against the fake wine and umu scripts.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::*;

const APP: &str = "[targets.app]\nexe = \"app\"\n";
const EXITS: &str = "[targets.app]\nexe = \"exit-0\"\n";

impl Fixture {
    /// Where wineo puts prefixes by default, which doesn't exist yet.
    fn prefix_root(&self) -> PathBuf {
        self.root.path().join("data/wineo/prefixes")
    }

    /// Writes a config by hand, pointing at `prefix` whatever is there, and
    /// returns its text.
    fn hand_written(&self, name: &str, runner: &str, prefix: &Path, targets: &str) -> String {
        let text = format!(
            "# written by hand\nprefix = \"{}\"\nrunner = \"{runner}\"\narch = \"win64\"\n\n{targets}",
            prefix.display()
        );
        let path = self.config_dir().join(format!("environments/{name}.toml"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &text).unwrap();
        text
    }

    fn config_text(&self, name: &str) -> String {
        fs::read_to_string(self.config_dir().join(format!("environments/{name}.toml"))).unwrap()
    }
}

fn wine_note(name: &str, prefix: &Path) -> String {
    format!(
        "Creating a wine prefix for {name} in {}, this takes a few seconds\n",
        prefix.display()
    )
}

#[test]
fn start_builds_a_missing_prefix_once() {
    let f = Fixture::new();
    let prefix = f.prefix_root().join("app");
    let text = f.hand_written("app", "wine", &prefix, APP);

    let started = f.run(&["start", "app"]);
    assert_success(&started);
    assert_eq!(stdout(&started), "Started app\n");
    assert_eq!(stderr(&started), wine_note("app", &prefix));
    assert!(prefix.join("system.reg").is_file());
    assert_eq!(f.config_text("app"), text);
    wait_for("the program's output in the log", || {
        f.log("app").contains("fake wine: app")
    });
    assert_success(&f.run(&["stop", "app"]));

    let again = f.run(&["start", "app"]);
    assert_success(&again);
    assert_eq!(stderr(&again), "");
    assert_success(&f.run(&["stop", "app"]));
    assert_eq!(f.config_text("app"), text);
}

#[test]
fn run_builds_a_missing_prefix() {
    let f = Fixture::new();
    let prefix = f.prefix_root().join("app");
    let text = f.hand_written("app", "wine", &prefix, EXITS);

    let ran = f.run(&["run", "app"]);
    assert_success(&ran);
    assert!(
        stderr(&ran).starts_with(&wine_note("app", &prefix)),
        "{}",
        stderr(&ran)
    );
    assert!(prefix.join("system.reg").is_file());
    assert_eq!(f.config_text("app"), text);
}

#[test]
fn exec_builds_a_missing_prefix_and_keeps_the_note_off_stdout() {
    let f = Fixture::new();
    let prefix = f.prefix_root().join("app");
    let text = f.hand_written("app", "wine", &prefix, "");

    let ran = f.run(&["exec", "app", "exit-0"]);
    assert_success(&ran);
    assert_eq!(stderr(&ran), wine_note("app", &prefix));
    assert!(!stdout(&ran).contains("Creating"), "{}", stdout(&ran));
    assert!(prefix.join("system.reg").is_file());
    assert_eq!(f.config_text("app"), text);
}

#[test]
fn winetricks_builds_a_missing_prefix() {
    let f = Fixture::new();
    let bin = f.root.path().join("bin-winetricks");
    fs::create_dir_all(&bin).unwrap();
    let winetricks = bin.join("winetricks");
    fs::write(&winetricks, "#!/bin/sh\necho \"winetricks: $*\"\n").unwrap();
    fs::set_permissions(&winetricks, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&f.path_var)))
        .unwrap();
    let prefix = f.prefix_root().join("app");
    let text = f.hand_written("app", "wine", &prefix, "");

    let ran = f
        .wineo()
        .env("PATH", path)
        .args(["winetricks", "app", "list-installed"])
        .output()
        .unwrap();
    assert_success(&ran);
    assert!(
        stderr(&ran).starts_with(&wine_note("app", &prefix)),
        "{}",
        stderr(&ran)
    );
    assert!(
        stdout(&ran).contains("winetricks: list-installed"),
        "{}",
        stdout(&ran)
    );
    assert!(prefix.join("system.reg").is_file());
    assert_eq!(f.config_text("app"), text);
}

#[test]
fn a_proton_prefix_is_built_through_umu() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    let prefix = f.prefix_root().join("hades");
    let text = f.hand_written("hades", "GE-Proton9-20", &prefix, "");

    let ran = f.run(&["exec", "hades", "exit-0"]);
    assert_success(&ran);
    assert_eq!(
        stderr(&ran),
        format!(
            "Creating a Proton prefix for hades in {}, this takes a while\n",
            prefix.display()
        )
    );
    assert!(prefix.join("pfx/system.reg").is_file());
    assert_eq!(f.config_text("hades"), text);
}

#[test]
fn programs_still_refuses_a_missing_prefix() {
    let f = Fixture::new();
    let prefix = f.prefix_root().join("app");
    f.hand_written("app", "wine", &prefix, "");

    let listed = f.run(&["programs", "app"]);
    assert!(!listed.status.success());
    assert!(
        stderr(&listed).contains("is missing or is not a wine prefix"),
        "{}",
        stderr(&listed)
    );
    assert!(!prefix.exists());
}

#[test]
fn an_empty_dir_outside_the_prefix_root_needs_setup() {
    let f = Fixture::new();
    let prefix = f.root.path().join("mnt/game-ssd");
    fs::create_dir_all(&prefix).unwrap();
    let text = f.hand_written("app", "wine", &prefix, APP);

    let refused = f.run(&["start", "app"]);
    assert!(!refused.status.success());
    assert_eq!(
        stderr(&refused),
        format!(
            "error: prefix {} is empty. Run `wineo setup app` to create it there.\n",
            prefix.display()
        )
    );
    assert_eq!(fs::read_dir(&prefix).unwrap().count(), 0);
    assert_eq!(f.config_text("app"), text);
}

#[test]
fn an_empty_parent_outside_the_prefix_root_needs_setup() {
    let f = Fixture::new();
    let games = f.root.path().join("mnt/games");
    fs::create_dir_all(&games).unwrap();
    let prefix = games.join("app");
    let text = f.hand_written("app", "wine", &prefix, "");

    let refused = f.run(&["exec", "app", "exit-0"]);
    assert!(!refused.status.success());
    assert_eq!(
        stderr(&refused),
        format!(
            "error: prefix {} is missing, and {} is empty. Run `wineo setup app` to create it there.\n",
            prefix.display(),
            games.display()
        )
    );
    assert!(!prefix.exists());
    assert_eq!(f.config_text("app"), text);
}

#[test]
fn a_missing_parent_outside_the_prefix_root_is_refused() {
    let f = Fixture::new();
    let prefix = f.root.path().join("mnt/games/prefixes/app");
    f.hand_written("app", "wine", &prefix, APP);

    let refused = f.run(&["start", "app"]);
    assert!(!refused.status.success());
    assert_eq!(
        stderr(&refused),
        format!(
            "error: prefix {} is missing, and so is {}\n",
            prefix.display(),
            prefix.parent().unwrap().display()
        )
    );
    assert!(!f.root.path().join("mnt").exists());
}

#[test]
fn an_uninstalled_runner_fails_before_anything_is_built() {
    let f = Fixture::new();
    let prefix = f.prefix_root().join("app");
    f.hand_written("app", "GE-Proton-Nope", &prefix, APP);

    let refused = f.run(&["start", "app"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("runner `GE-Proton-Nope` is not installed"),
        "{}",
        stderr(&refused)
    );
    assert!(!prefix.exists());
}

#[test]
fn restart_builds_a_missing_prefix() {
    let f = Fixture::new();
    let prefix = f.prefix_root().join("app");
    let text = f.hand_written("app", "wine", &prefix, APP);

    let restarted = f.run(&["restart", "app"]);
    assert_success(&restarted);
    assert_eq!(stdout(&restarted), "Started app\n");
    assert!(
        stderr(&restarted).starts_with(&wine_note("app", &prefix)),
        "{}",
        stderr(&restarted)
    );
    assert_eq!(stderr(&restarted).matches("Creating").count(), 1);
    assert!(prefix.join("system.reg").is_file());
    assert_eq!(f.config_text("app"), text);
    assert_success(&f.run(&["stop", "app"]));
}

#[test]
fn a_refused_prefix_stops_restart_before_anything_is_stopped() {
    let f = Fixture::new();
    f.environment("app", APP);
    assert_success(&f.run(&["start", "app"]));
    let prefix = f.root.path().join("mnt/game-ssd");
    fs::create_dir_all(&prefix).unwrap();
    let config = f.config_dir().join("environments/app.toml");
    let text = f.config_text("app");
    let old = f.root.path().join("prefixes/app");
    fs::write(
        &config,
        text.replace(&old.display().to_string(), &prefix.display().to_string()),
    )
    .unwrap();

    let refused = f.run(&["restart", "app"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("Run `wineo setup app` to create it there."),
        "{}",
        stderr(&refused)
    );
    assert!(f.state("app").contains("status = \"running\""));
    assert_eq!(fs::read_dir(&prefix).unwrap().count(), 0);
}

#[test]
fn fast_checks_fail_before_anything_is_built() {
    let f = Fixture::new();
    let prefix = f.prefix_root().join("app");
    f.hand_written("app", "wine", &prefix, APP);

    for (args, message) in [
        (
            &["start", "app", "--target", "nope"][..],
            "has no target `nope`",
        ),
        (&["run", "app", "--target", "nope"], "has no target `nope`"),
        (
            &["restart", "app", "--target", "nope"],
            "has no target `nope`",
        ),
        (
            &["exec", "app", "/nonexistent/setup.exe"],
            "`/nonexistent/setup.exe` does not exist",
        ),
        (
            &["winetricks", "app", "list-installed"],
            "`winetricks` was not found on PATH",
        ),
    ] {
        let refused = f.run_sandboxed(args);
        assert!(!refused.status.success(), "{args:?}");
        assert!(stderr(&refused).contains(message), "{}", stderr(&refused));
        assert!(!stderr(&refused).contains("Creating"), "{args:?}");
        assert!(!prefix.exists(), "{args:?}");
    }
}

#[test]
#[ignore = "runs wine"]
fn exec_builds_a_real_wine_prefix_from_a_hand_written_config() {
    let f = Fixture::real();
    let prefix = f.prefix_root().join("real");
    let text = f.hand_written("real", "wine", &prefix, "");

    let ran = f.run(&["exec", "real", "cmd", "/c", "exit", "0"]);
    assert_success(&ran);
    assert!(
        stderr(&ran).contains(&wine_note("real", &prefix)),
        "{}",
        stderr(&ran)
    );
    assert!(prefix.join("system.reg").is_file());
    assert!(prefix.join("drive_c/windows").is_dir());
    assert_eq!(f.config_text("real"), text);

    let again = f.run(&["exec", "real", "cmd", "/c", "exit", "0"]);
    assert_success(&again);
    assert!(!stderr(&again).contains("Creating"), "{}", stderr(&again));
}
