//! Building the prefix of a hand-written config from the commands that hand
//! it to wine, against the fake wine and umu scripts.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

fn slow_wineboot(f: &Fixture) {
    let wine = f.root.path().join("bin/wine");
    let script = FAKE_WINE.replacen(
        "if [ \"$2\" = --init ]; then",
        "if [ \"$2\" = --init ]; then\n        touch \"$WINEPREFIX/partial\"\n        trap 'exit 0' TERM\n        while :; do sleep 0.1; done",
        1,
    );
    assert_ne!(script, FAKE_WINE);
    write_script(wine, script);
}

fn interrupt(child: &std::process::Child) {
    let sent = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
}

#[test]
fn ctrl_c_during_new_wineboot_removes_the_prefix_and_config() {
    let f = Fixture::new();
    slow_wineboot(&f);
    let prefix = f.prefix_root().join("app");
    let child = f
        .wineo()
        .args(["new", "-n", "app"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("wineboot to begin", || prefix.join("partial").exists());

    interrupt(&child);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        format!(
            "Creating a wine prefix in {}, this takes a few seconds\nCanceled. Nothing was created.\n",
            prefix.display()
        )
    );
    assert!(!prefix.exists());
    assert!(f.created_config("app").is_empty());
}

#[test]
fn ctrl_c_during_new_in_an_existing_empty_dir_empties_it_and_keeps_it() {
    let f = Fixture::new();
    slow_wineboot(&f);
    let prefix = f.root.path().join("chosen");
    fs::create_dir(&prefix).unwrap();
    let child = f
        .wineo()
        .args(["new", "-n", "app", "--dir", prefix.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("wineboot to begin", || prefix.join("partial").exists());

    interrupt(&child);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).ends_with("Canceled. Nothing was created.\n"));
    assert!(prefix.is_dir());
    assert_eq!(fs::read_dir(&prefix).unwrap().count(), 0);
    assert!(f.created_config("app").is_empty());
}

#[test]
fn ctrl_c_during_start_wineboot_keeps_the_config() {
    let f = Fixture::new();
    slow_wineboot(&f);
    let prefix = f.prefix_root().join("app");
    let text = f.hand_written("app", "wine", &prefix, APP);
    let child = f
        .wineo()
        .args(["start", "app"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("wineboot to begin", || prefix.join("partial").exists());

    interrupt(&child);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    assert_eq!(stderr(&output), wine_note("app", &prefix));
    assert!(!prefix.exists());
    assert_eq!(f.config_text("app"), text);
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
    write_script(&winetricks, "#!/bin/sh\necho \"winetricks: $*\"\n");
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

/// Live processes that inherited `prefix` as their WINEPREFIX, which is how
/// wineserver and its services can be told apart from other prefixes' wine.
fn wine_processes_in(prefix: &Path) -> Vec<String> {
    let needle = format!("WINEPREFIX={}", prefix.display());
    let mut found = Vec::new();
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let path = entry.path();
        let Ok(environ) = fs::read(path.join("environ")) else {
            continue;
        };
        if !environ
            .split(|byte| *byte == 0)
            .any(|var| var == needle.as_bytes())
        {
            continue;
        }
        let stat = fs::read_to_string(path.join("stat")).unwrap_or_default();
        // The state follows the parenthesized name, and zombies are as good as gone.
        if stat
            .rsplit(") ")
            .next()
            .is_some_and(|rest| rest.starts_with('Z'))
        {
            continue;
        }
        let name = fs::read_to_string(path.join("comm")).unwrap_or_default();
        found.push(format!(
            "{} {}",
            entry.file_name().to_string_lossy(),
            name.trim()
        ));
    }
    found
}

/// Stops any wineserver a failed test left behind.
struct ServerGuard<'a>(&'a Path);

impl Drop for ServerGuard<'_> {
    fn drop(&mut self) {
        let _ = Command::new("wineserver")
            .arg("-k")
            .env("WINEPREFIX", self.0)
            .output();
    }
}

fn cancel_real_new(f: &Fixture, prefix: &Path, args: &[&str]) -> std::process::Output {
    let child = f
        .wineo()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("wineboot to write into the prefix", || {
        fs::read_dir(prefix).is_ok_and(|mut entries| entries.next().is_some())
    });
    std::thread::sleep(std::time::Duration::from_millis(1500));
    interrupt(&child);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert!(stderr(&output).ends_with("Canceled. Nothing was created.\n"));
    // Wine used to keep rewriting the prefix a few seconds after the cleanup.
    std::thread::sleep(std::time::Duration::from_secs(5));
    output
}

#[test]
#[ignore = "runs wine"]
fn ctrl_c_during_real_new_in_an_existing_empty_dir_leaves_it_empty() {
    let f = Fixture::real();
    let prefix = f.root.path().join("chosen");
    fs::create_dir(&prefix).unwrap();
    let _guard = ServerGuard(&prefix);

    cancel_real_new(
        &f,
        &prefix,
        &["new", "-n", "real", "--dir", prefix.to_str().unwrap()],
    );

    assert!(prefix.is_dir());
    let left: Vec<_> = fs::read_dir(&prefix).unwrap().flatten().collect();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(wine_processes_in(&prefix), Vec::<String>::new());
    assert!(f.created_config("real").is_empty());
}

#[test]
#[ignore = "runs wine"]
fn ctrl_c_during_real_new_removes_a_dir_wineo_made_for_good() {
    let f = Fixture::real();
    let prefix = f.prefix_root().join("real");
    let _guard = ServerGuard(&prefix);

    cancel_real_new(&f, &prefix, &["new", "-n", "real"]);

    assert!(!prefix.exists());
    assert_eq!(wine_processes_in(&prefix), Vec::<String>::new());
    assert!(f.created_config("real").is_empty());
}
