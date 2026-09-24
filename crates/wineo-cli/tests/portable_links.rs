//! Links inside prefixes copied into a portable folder are written relative,
//! so they still resolve after the whole folder moves.

mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use common::*;

fn reflink_supported(dir: &Path) -> bool {
    let source = dir.join("reflink-source");
    let destination = dir.join("reflink-destination");
    fs::write(&source, b"probe").unwrap();
    let input = fs::File::open(&source).unwrap();
    let output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .unwrap();
    let supported = rustix::fs::ioctl_ficlone(&output, &input).is_ok();
    fs::remove_file(source).unwrap();
    fs::remove_file(destination).unwrap();
    supported
}

fn fixture() -> Option<Fixture> {
    let f = Fixture::new_in(Path::new(env!("CARGO_MANIFEST_DIR")));
    reflink_supported(f.root.path()).then_some(f)
}

/// A wine prefix whose `pfx` is an absolute link to its own root, the way
/// some Heroic prefixes are, plus an absolute link deeper in the tree.
fn linked_prefix(root: &Path) {
    fs::create_dir_all(root.join("drive_c/Games")).unwrap();
    fs::write(
        root.join("system.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n",
    )
    .unwrap();
    fs::write(root.join("drive_c/Games/game.exe"), "game").unwrap();
    symlink(root, root.join("pfx")).unwrap();
    symlink(
        root.join("drive_c/Games/game.exe"),
        root.join("drive_c/link.exe"),
    )
    .unwrap();
}

fn register(config_dir: &Path, name: &str, prefix: &str) {
    fs::create_dir_all(config_dir).unwrap();
    fs::write(
        config_dir.join(format!("{name}.toml")),
        format!("prefix = '{prefix}'\nrunner = 'wine'\narch = 'win64'\n"),
    )
    .unwrap();
}

fn portable(f: &Fixture, dir: &Path, args: &[&str]) -> std::process::Output {
    f.wineo()
        .arg("--portable")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
}

/// Checks a prefix after its folder moved: the links are relative, resolve at
/// the new location, and wineo can still snapshot and restore it there.
fn check_moved(f: &Fixture, dir: &Path, name: &str) {
    let prefix = dir.join("prefixes").join(name);
    assert_eq!(fs::read_link(prefix.join("pfx")).unwrap(), Path::new("."));
    let link = fs::read_link(prefix.join("drive_c/link.exe")).unwrap();
    assert!(link.is_relative(), "{}", link.display());
    assert_eq!(
        fs::read_to_string(prefix.join("drive_c/link.exe")).unwrap(),
        "game"
    );
    assert_eq!(
        fs::read_to_string(prefix.join("pfx/drive_c/link.exe")).unwrap(),
        "game"
    );

    assert_success(&portable(f, dir, &["snapshot", "create", name, "moved"]));
    fs::write(prefix.join("drive_c/Games/game.exe"), "changed").unwrap();
    assert_success(&portable(
        f,
        dir,
        &["snapshot", "restore", name, "moved", "--yes"],
    ));
    assert_eq!(
        fs::read_to_string(prefix.join("pfx/drive_c/link.exe")).unwrap(),
        "game"
    );
}

fn moved(f: &Fixture, dir: &Path) -> PathBuf {
    let moved = f.root.path().join("moved");
    fs::rename(dir, &moved).unwrap();
    moved
}

#[test]
fn a_portable_copy_writes_relative_links_in_the_prefix_and_its_snapshots() {
    let Some(f) = fixture() else { return };
    let source = f.root.path().join("prefixes/game");
    linked_prefix(&source);
    register(
        &f.config_dir().join("environments"),
        "game",
        source.to_str().unwrap(),
    );
    assert_success(&f.run(&["snapshot", "create", "game", "one"]));
    let dir = f.root.path().join("usb");
    assert_success(
        &f.wineo()
            .args(["portable", "copy", "game", "--to"])
            .arg(&dir)
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read_link(source.join("pfx")).unwrap(), source);

    let dir = moved(&f, &dir);
    let saved = dir.join("prefixes/.wineo-snapshots/game/one/prefix");
    assert_eq!(fs::read_link(saved.join("pfx")).unwrap(), Path::new("."));
    assert!(
        fs::read_link(saved.join("drive_c/link.exe"))
            .unwrap()
            .is_relative()
    );
    check_moved(&f, &dir, "game");
    assert_success(&portable(
        &f,
        &dir,
        &["snapshot", "restore", "game", "one", "--yes"],
    ));
    assert_eq!(
        fs::read_to_string(dir.join("prefixes/game/pfx/drive_c/link.exe")).unwrap(),
        "game"
    );
}

#[test]
fn a_portable_import_writes_relative_links() {
    let Some(f) = fixture() else { return };
    let source = f.root.path().join("heroic/game");
    linked_prefix(&source);
    let dir = f.root.path().join("usb");
    let imported = portable(
        &f,
        &dir,
        &[
            "import",
            "prefix",
            source.to_str().unwrap(),
            "-n",
            "game",
            "--runner",
            "wine",
        ],
    );
    assert_success(&imported);
    assert_eq!(fs::read_link(source.join("pfx")).unwrap(), source);
    let dir = moved(&f, &dir);
    check_moved(&f, &dir, "game");
}

#[test]
fn a_portable_clone_writes_relative_links() {
    let Some(f) = fixture() else { return };
    let dir = f.root.path().join("usb");
    let source = dir.join("prefixes/game");
    linked_prefix(&source);
    register(&dir.join("config/environments"), "game", "prefixes/game");
    assert_success(&portable(&f, &dir, &["clone", "game", "copy"]));
    assert_eq!(fs::read_link(source.join("pfx")).unwrap(), source);
    let dir = moved(&f, &dir);
    check_moved(&f, &dir, "copy");
}

#[test]
fn a_standard_clone_keeps_absolute_links() {
    let Some(f) = fixture() else { return };
    let source = f.root.path().join("prefixes/game");
    linked_prefix(&source);
    register(
        &f.config_dir().join("environments"),
        "game",
        source.to_str().unwrap(),
    );
    assert_success(&f.run(&["clone", "game", "copy"]));
    let copy = f.root.path().join("data/wineo/prefixes/copy");
    assert_eq!(fs::read_link(copy.join("pfx")).unwrap(), copy);
    assert_eq!(
        fs::read_link(copy.join("drive_c/link.exe")).unwrap(),
        copy.join("drive_c/Games/game.exe")
    );
}

/// A portable environment whose prefix gets an absolute internal link by hand,
/// the way a user or a tool running in it might add one.
fn hand_linked(f: &Fixture) -> PathBuf {
    let dir = f.root.path().join("usb");
    let prefix = dir.join("prefixes/game");
    fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n",
    )
    .unwrap();
    fs::write(prefix.join("drive_c/Games/game.exe"), "game").unwrap();
    register(&dir.join("config/environments"), "game", "prefixes/game");
    symlink(
        prefix.join("drive_c/Games/game.exe"),
        prefix.join("drive_c/hand.exe"),
    )
    .unwrap();
    dir
}

#[test]
fn a_portable_snapshot_round_trip_makes_hand_links_relative() {
    let Some(f) = fixture() else { return };
    let dir = hand_linked(&f);
    assert_success(&portable(&f, &dir, &["snapshot", "create", "game", "one"]));
    let saved = dir.join("prefixes/.wineo-snapshots/game/one/prefix/drive_c/hand.exe");
    assert!(fs::read_link(&saved).unwrap().is_relative());
    assert_success(&portable(
        &f,
        &dir,
        &["snapshot", "restore", "game", "one", "--yes"],
    ));
    let live = dir.join("prefixes/game/drive_c/hand.exe");
    assert!(fs::read_link(&live).unwrap().is_relative());

    let dir = moved(&f, &dir);
    assert_eq!(
        fs::read_to_string(dir.join("prefixes/game/drive_c/hand.exe")).unwrap(),
        "game"
    );
}

#[test]
fn a_portable_snapshot_restores_after_the_folder_moves() {
    let Some(f) = fixture() else { return };
    let dir = hand_linked(&f);
    assert_success(&portable(&f, &dir, &["snapshot", "create", "game", "one"]));
    let dir = moved(&f, &dir);
    fs::remove_file(dir.join("prefixes/game/drive_c/hand.exe")).unwrap();
    assert_success(&portable(
        &f,
        &dir,
        &["snapshot", "restore", "game", "one", "--yes"],
    ));
    let live = dir.join("prefixes/game/drive_c/hand.exe");
    assert!(fs::read_link(&live).unwrap().is_relative());
    assert_eq!(fs::read_to_string(&live).unwrap(), "game");
}
