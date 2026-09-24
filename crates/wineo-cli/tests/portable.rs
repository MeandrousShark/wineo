//! Selecting a portable folder with `--portable` or `WINEO_PORTABLE_DIR`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use common::*;

const APP: &str = "[targets.app]\nexe = \"app\"\n";

/// Writes an environment into a portable folder, its prefix inside the folder.
fn portable_environment(dir: &Path, name: &str) -> PathBuf {
    let prefix = dir.join("prefixes").join(name);
    fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
    fs::create_dir_all(prefix.join("drive_c/windows/syswow64")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n",
    )
    .unwrap();
    let config = dir.join("config/environments").join(format!("{name}.toml"));
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n\n{APP}",
            prefix.display()
        ),
    )
    .unwrap();
    prefix
}

fn names(output: &std::process::Output) -> Vec<String> {
    assert_success(output);
    stdout(output)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn read_only_commands_do_not_create_the_folder() {
    let f = Fixture::new();
    let dir = f.root.path().join("portable");
    let dir_arg = dir.to_str().unwrap();
    for args in [
        vec!["list"],
        vec!["status"],
        vec!["config", "--global"],
        vec!["config", "hades"],
        vec!["templates"],
        vec!["runners"],
        vec!["dxvk"],
        vec!["vkd3d"],
        vec!["desktop"],
        vec!["logs", "hades"],
        vec!["programs", "hades"],
        vec!["snapshot", "list", "hades"],
    ] {
        let mut full = vec!["--portable", dir_arg];
        full.extend(&args);
        f.run(&full);
        assert!(!dir.exists(), "{args:?} created the folder");
        f.wineo()
            .env("WINEO_PORTABLE_DIR", &dir)
            .args(&args)
            .output()
            .unwrap();
        assert!(!dir.exists(), "{args:?} created the folder");
    }
}

#[test]
fn the_option_wins_over_the_variable_and_xdg_stays_put() {
    let f = Fixture::new();
    f.environment("xdg", APP);
    let one = f.root.path().join("one");
    let two = f.root.path().join("two");
    portable_environment(&one, "alpha");
    portable_environment(&two, "beta");

    assert_eq!(names(&f.run(&["list"])), ["xdg"]);
    let listed = f
        .wineo()
        .env("WINEO_PORTABLE_DIR", &two)
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(names(&listed), ["beta"]);
    let listed = f
        .wineo()
        .env("WINEO_PORTABLE_DIR", &two)
        .args(["list", "--portable"])
        .arg(&one)
        .output()
        .unwrap();
    assert_eq!(names(&listed), ["alpha"]);
    let listed = f
        .wineo()
        .env("WINEO_PORTABLE_DIR", "")
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(names(&listed), ["xdg"]);
}

#[test]
fn a_relative_folder_resolves_from_where_wineo_starts() {
    let f = Fixture::new();
    let dir = f.root.path().join("portable");
    portable_environment(&dir, "alpha");
    let listed = f
        .wineo()
        .current_dir(f.root.path())
        .args(["--portable", "portable", "list"])
        .output()
        .unwrap();
    assert_eq!(names(&listed), ["alpha"]);
}

/// Stops the portable run even when an assertion fails first.
struct StopOnDrop<'a>(&'a Fixture, PathBuf);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        let _ = self
            .0
            .wineo()
            .arg("--portable")
            .arg(&self.1)
            .args(["stop", "--force", "hades"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[test]
fn same_named_environments_keep_separate_runs() {
    let f = Fixture::new();
    f.environment("hades", APP);
    let dir = f.root.path().join("portable");
    let prefix = portable_environment(&dir, "hades");
    let _stop = StopOnDrop(&f, dir.clone());
    let portable = |args: &[&str]| {
        f.wineo()
            .arg("--portable")
            .arg(&dir)
            .args(args)
            .output()
            .unwrap()
    };

    assert_success(&portable(&["start", "hades"]));
    wait_for("the portable program", || prefix.join("fake-pids").exists());
    assert!(!f.fake_program_running("hades"));
    assert_eq!(f.state("hades"), "");
    let supervisor = format!("__supervise\0--portable\0{}\0", dir.display());
    let carried = fs::read_dir("/proc").unwrap().flatten().any(|entry| {
        fs::read(entry.path().join("cmdline"))
            .is_ok_and(|cmdline| String::from_utf8_lossy(&cmdline).ends_with(&supervisor))
    });
    assert!(
        carried,
        "no supervisor was started with the portable folder"
    );
    assert!(stdout(&portable(&["status", "hades"])).contains("running"));
    assert!(!stdout(&f.run(&["status", "hades"])).contains("running"));

    // The XDG environment starts on its own while the portable one runs.
    assert_success(&f.run(&["start", "hades"]));
    assert!(f.state("hades").contains("status = \"running\""));
    assert_success(&f.run(&["stop", "hades"]));
    assert!(prefix.join("fake-pids").exists());

    assert_success(&portable(&["stop", "hades"]));
    assert!(!prefix.join("fake-pids").exists());
    assert!(!stdout(&portable(&["status", "hades"])).contains("running"));
}
