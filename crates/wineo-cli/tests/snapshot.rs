mod common;

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use common::{Fixture, assert_success, stderr, stdout};

fn fixture() -> Option<Fixture> {
    let f = Fixture::new_in(Path::new(env!("CARGO_MANIFEST_DIR")));
    let source = f.root.path().join("reflink-source");
    let destination = f.root.path().join("reflink-destination");
    fs::write(&source, b"probe").unwrap();
    let input = File::open(&source).unwrap();
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .unwrap();
    let supported = rustix::fs::ioctl_ficlone(&output, &input).is_ok();
    fs::remove_file(source).unwrap();
    fs::remove_file(destination).unwrap();
    supported.then_some(f)
}

fn environment(f: &Fixture) -> (PathBuf, String) {
    let prefix = f.root.path().join("game-prefix");
    fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    fs::write(prefix.join("drive_c/Games/save.dat"), b"before").unwrap();
    let config_dir = f.config_dir().join("environments");
    fs::create_dir_all(&config_dir).unwrap();
    let config = format!(
        "# saved comment\nprefix = {:?}\nrunner = 'wine'\narch = 'win64'\n\n[targets.game]\nexe = 'C:\\Games\\Game.exe'\ndesktop = false\ndefault = true\n",
        prefix.to_str().unwrap()
    );
    fs::write(config_dir.join("game.toml"), &config).unwrap();
    (prefix, config)
}

#[test]
fn create_restore_and_delete_keep_the_saved_state_independent() {
    let Some(f) = fixture() else { return };
    let (prefix, saved_config) = environment(&f);
    assert_success(&f.run(&["snapshot", "create", "game", "before-update"]));

    let snapshots = stdout(&f.run(&["snapshot", "list", "game"]));
    assert!(snapshots.contains("before-update"), "{snapshots}");
    let snapshot = prefix
        .parent()
        .unwrap()
        .join(".wineo-snapshots/game/before-update/prefix");
    assert_eq!(
        fs::read(snapshot.join("drive_c/Games/save.dat")).unwrap(),
        b"before"
    );

    fs::write(prefix.join("drive_c/Games/save.dat"), b"after").unwrap();
    fs::write(
        f.config_dir().join("environments/game.toml"),
        format!("# current edit\n{saved_config}"),
    )
    .unwrap();
    let no_confirmation = f.run(&["snapshot", "restore", "game", "before-update"]);
    assert!(!no_confirmation.status.success());
    assert!(stderr(&no_confirmation).contains("--yes"));
    assert_eq!(
        fs::read(prefix.join("drive_c/Games/save.dat")).unwrap(),
        b"after"
    );

    assert_success(&f.run(&["snapshot", "restore", "game", "before-update", "--yes"]));
    assert_eq!(
        fs::read(prefix.join("drive_c/Games/save.dat")).unwrap(),
        b"before"
    );
    assert_eq!(f.created_config("game"), saved_config);
    assert!(snapshot.exists());
    assert_success(&f.run(&["snapshot", "delete", "game", "before-update"]));
    assert!(!snapshot.exists());
    assert!(prefix.exists());
}

#[test]
fn create_refuses_duplicate_label_without_changing_the_first_snapshot() {
    let Some(f) = fixture() else { return };
    let (prefix, _) = environment(&f);
    assert_success(&f.run(&["snapshot", "create", "game", "one"]));
    fs::write(prefix.join("drive_c/Games/save.dat"), b"changed").unwrap();
    let duplicate = f.run(&["snapshot", "create", "game", "one"]);
    assert!(!duplicate.status.success());
    let snapshot = prefix
        .parent()
        .unwrap()
        .join(".wineo-snapshots/game/one/prefix/drive_c/Games/save.dat");
    assert_eq!(fs::read(snapshot).unwrap(), b"before");
}

#[test]
fn interrupted_restore_blocks_config_edits_and_removal() {
    let Some(f) = fixture() else { return };
    let (prefix, config) = environment(&f);
    let lock_dir = f.root.path().join("state/wineo/.locks");
    fs::create_dir_all(&lock_dir).unwrap();
    fs::write(lock_dir.join("snapshot-restore-game-restore.json"), "{}").unwrap();

    for args in [
        vec!["set", "game", "runner=wine"],
        vec!["config", "game"],
        vec!["rm", "game", "--delete-prefix"],
    ] {
        let output = f.run(&args);
        assert!(!output.status.success(), "{args:?}");
        assert!(stderr(&output).contains("snapshot recover game"));
    }
    assert_eq!(f.created_config("game"), config);
    assert!(prefix.exists());
}

#[test]
fn removing_environment_leaves_snapshot_and_reports_where_it_is() {
    let Some(f) = fixture() else { return };
    let (prefix, _) = environment(&f);
    assert_success(&f.run(&["snapshot", "create", "game", "saved"]));
    let root = prefix.parent().unwrap().join(".wineo-snapshots/game");

    let output = f.run(&["rm", "game", "--delete-prefix"]);
    assert_success(&output);
    assert!(stdout(&output).contains(root.to_str().unwrap()));
    assert!(!prefix.exists());
    assert!(root.join("saved/prefix/system.reg").exists());
}
