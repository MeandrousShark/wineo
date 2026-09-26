//! `wineo new --exe` and `-d`/`--desktop`, which work the same for a Wine
//! environment as for a native one. See native.rs for the native side.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::*;

fn executable(f: &Fixture, path: &str) -> std::path::PathBuf {
    let exe = f.root.path().join(path);
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

#[test]
fn new_exe_adds_a_default_target_named_after_the_environment() {
    let f = Fixture::new();
    let exe = executable(&f, "games/Palworld/start.sh");

    let created = f.run(&["new", "-n", "palworld", "--exe", exe.to_str().unwrap()]);
    assert_success(&created);
    assert_eq!(
        stdout(&created),
        "Created environment palworld with target palworld\n"
    );
    let config = f.created_config("palworld");
    assert!(config.contains("[targets.palworld]"), "{config}");
    assert!(
        config.contains(&format!("exe = \"{}\"", exe.display())),
        "{config}"
    );
    assert!(config.contains("default = true"), "{config}");
}

#[test]
fn new_exe_not_found_keeps_the_environment_with_a_combined_error() {
    let f = Fixture::new();
    let exe = f.root.path().join("games/missing.sh");

    let created = f.run(&["new", "-n", "palworld", "--exe", exe.to_str().unwrap()]);
    assert!(!created.status.success());
    assert!(
        stderr(&created).ends_with(&format!(
            "Created environment palworld, but {} was not found. Add a target with: wineo setup palworld\n",
            exe.display()
        )),
        "{}",
        stderr(&created)
    );
    let config = f.created_config("palworld");
    assert!(!config.is_empty(), "the environment should be kept");
    assert!(!config.contains("[targets"), "{config}");
}

#[test]
fn new_exe_and_install_exe_conflict() {
    let f = Fixture::new();
    let refused = f.run(&[
        "new",
        "-n",
        "app",
        "--exe",
        "start.sh",
        "--install-exe",
        "/nonexistent/setup.exe",
    ]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("cannot be used with"),
        "{}",
        stderr(&refused)
    );
    assert!(f.created_config("app").is_empty());
}

#[test]
fn new_desktop_without_exe_is_refused() {
    let f = Fixture::new();
    let refused = f.run(&["new", "-n", "app", "-d"]);
    assert!(!refused.status.success());
    assert!(f.created_config("app").is_empty());
}

#[test]
fn new_exe_with_desktop_writes_an_entry_for_wine() {
    let f = Fixture::new();
    let exe = executable(&f, "games/Palworld/start.sh");

    let created = f.run(&[
        "new",
        "-n",
        "palworld",
        "--exe",
        exe.to_str().unwrap(),
        "-d",
    ]);
    assert_success(&created);
    assert!(
        f.root
            .path()
            .join("data/applications/wineo-palworld-palworld.desktop")
            .is_file()
    );
}

#[test]
fn new_exe_not_found_writes_no_desktop_entry() {
    let f = Fixture::new();
    let exe = f.root.path().join("games/missing.sh");

    let created = f.run(&[
        "new",
        "-n",
        "palworld",
        "--exe",
        exe.to_str().unwrap(),
        "-d",
    ]);
    assert!(!created.status.success());
    assert!(
        !f.root
            .path()
            .join("data/applications/wineo-palworld-palworld.desktop")
            .exists()
    );
}
