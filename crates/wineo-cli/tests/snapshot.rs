mod common;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{Fixture, assert_success, stderr, stdout};
use rustix::fs::OFlags;
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

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
fn create_and_restore_wait_for_wine_to_exit() {
    let Some(f) = fixture() else { return };
    let (prefix, _) = environment(&f);
    let handoff = |f: &Fixture| {
        // Null output, so only the wait on the prefix can outlast the handoff.
        let exec = f
            .wineo()
            .args(["exec", "game", "handoff"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(exec.success());
    };

    handoff(&f);
    let created = f.run(&["snapshot", "create", "game", "after-exec"]);
    assert_success(&created);
    assert!(
        stderr(&created).contains("Waiting for wine to exit in game"),
        "{}",
        stderr(&created)
    );
    assert!(
        prefix
            .parent()
            .unwrap()
            .join(".wineo-snapshots/game/after-exec/prefix/handoff-done")
            .exists()
    );

    fs::remove_file(prefix.join("handoff-done")).unwrap();
    handoff(&f);
    let restored = f.run(&["snapshot", "restore", "game", "after-exec", "--yes"]);
    assert_success(&restored);
    assert!(
        stderr(&restored).contains("Waiting for wine to exit in game"),
        "{}",
        stderr(&restored)
    );
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
    let removed = f.run(&["rm", "game", "--delete-prefix"]);
    assert_success(&removed);
    assert!(!stdout(&removed).contains("Snapshots left"));
}

#[test]
fn declining_restore_confirmation_keeps_the_prefix() {
    let Some(f) = fixture() else { return };
    let (prefix, _) = environment(&f);
    assert_success(&f.run(&["snapshot", "create", "game", "before-update"]));
    fs::write(prefix.join("drive_c/Games/save.dat"), b"after").unwrap();

    for (response, input) in [("n", &b"n\r"[..]), ("Ctrl+D", &b"\x04"[..])] {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let slave_name = ptsname(&master, Vec::new()).unwrap();
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(OFlags::NOCTTY.bits() as i32)
            .open(slave_name.to_str().unwrap())
            .unwrap();
        let mut command: Command = f.wineo();
        command
            .args(["snapshot", "restore", "game", "before-update"])
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave);
        let mut child = command.spawn().unwrap();
        drop(command);
        let mut master = File::from(master);
        master.write_all(input).unwrap();

        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(130), "{response}");
        let mut output = String::new();
        let mut bytes = [0; 1024];
        while let Ok(read) = master.read(&mut bytes) {
            if read == 0 {
                break;
            }
            output.push_str(&String::from_utf8_lossy(&bytes[..read]));
        }
        assert!(output.contains("Restore canceled"), "{response}: {output}");
        assert_eq!(
            fs::read(prefix.join("drive_c/Games/save.dat")).unwrap(),
            b"after"
        );
    }
}

#[test]
fn a_restore_leaves_play_stats_alone() {
    let Some(f) = fixture() else { return };
    environment(&f);
    f.write_stats_log(
        "game",
        "{\"target\":\"game\",\"started_at\":\"2026-01-01T00:00:00Z\",\"ended_at\":\"2026-01-01T01:00:00Z\",\"seconds\":3600,\"end\":\"exited\"}\n",
    );
    assert_success(&f.run(&["snapshot", "create", "game", "before-update"]));
    let before = f.read_stats_log("game");
    f.write_stats_log(
        "game",
        "{\"target\":\"game\",\"started_at\":\"2026-01-02T00:00:00Z\",\"ended_at\":\"2026-01-02T02:00:00Z\",\"seconds\":7200,\"end\":\"exited\"}\n",
    );
    assert_success(&f.run(&["snapshot", "restore", "game", "before-update", "--yes"]));
    // A restore rolls back the prefix, but never playtime.
    assert_ne!(f.read_stats_log("game"), before);
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
fn removing_environment_without_prefix_leaves_snapshot_and_reports_where_it_is() {
    let Some(f) = fixture() else { return };
    let (prefix, _) = environment(&f);
    assert_success(&f.run(&["snapshot", "create", "game", "saved"]));
    let root = prefix.parent().unwrap().join(".wineo-snapshots/game");

    let output = f.run(&["rm", "game"]);
    assert_success(&output);
    assert!(stdout(&output).contains(&format!("Snapshots left at {}", root.display())));
    assert!(prefix.exists());
    assert!(root.join("saved/prefix/system.reg").exists());
    assert!(stdout(&f.run(&["trash"])).contains("prefix left in place"));
}

#[test]
fn removing_prefix_and_snapshots_can_be_restored_from_trash() {
    let Some(f) = fixture() else { return };
    let (prefix, config) = environment(&f);
    assert_success(&f.run(&["snapshot", "create", "game", "saved"]));
    let snapshots = prefix.parent().unwrap().join(".wineo-snapshots/game");
    f.write_stats_log("game", "saved playtime\n");

    let removed = f.run(&["rm", "game", "--delete-prefix"]);
    assert_success(&removed);
    assert!(!prefix.exists());
    assert!(!snapshots.exists());
    let listing = f.run(&["trash", "list"]);
    assert_success(&listing);
    assert!(stdout(&listing).contains("prefix included"));
    let id = stdout(&listing)
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    assert!(stdout(&removed).contains(&id));

    assert_success(&f.run(&["restore", &id]));
    assert_eq!(f.created_config("game"), config);
    assert_eq!(f.read_stats_log("game"), "saved playtime\n");
    assert!(prefix.join("system.reg").exists());
    assert!(snapshots.join("saved/prefix/system.reg").exists());
    assert!(stdout(&f.run(&["trash", "list"])).trim().is_empty());
}

#[test]
fn a_restore_that_drops_a_target_keeps_its_art() {
    let Some(f) = fixture() else { return };
    environment(&f);
    assert_success(&f.run(&["snapshot", "create", "game", "one"]));
    assert_success(&f.run(&["set", "game", "targets.server.exe=server.exe"]));
    let icon = f.root.path().join("server.svg");
    fs::write(&icon, b"<svg/>").unwrap();
    assert_success(&f.run(&["art", "set", "game.server", "icon", icon.to_str().unwrap()]));

    assert_success(&f.run(&["snapshot", "restore", "game", "one", "--yes"]));
    let art = f.root.path().join("data/wineo/art/game");
    assert_eq!(fs::read(art.join("server/icon.svg")).unwrap(), b"<svg/>");
    let listed = f.run(&["art", "list", "game"]);
    assert_success(&listed);
    assert_eq!(
        stderr(&listed),
        "warning: art for unknown target `server`\n"
    );
}

#[test]
fn a_restore_from_before_migration_keeps_the_gog_id() {
    let Some(f) = fixture() else { return };
    let (_, config) = environment(&f);
    assert_success(&f.run(&["snapshot", "create", "game", "before-gog"]));
    let migrated = config.replacen(
        "# saved comment\n",
        "# saved comment\ngog_id = \"1094900565\"\n",
        1,
    );
    fs::write(f.config_dir().join("environments/game.toml"), &migrated).unwrap();
    let folder = f.root.path().join("Games/game");
    fs::create_dir_all(&folder).unwrap();
    f.gog_folder(&folder, Some("game"), 1094900565, false);

    let output = f.run(&["snapshot", "restore", "game", "before-gog", "--yes"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("Put gog_id 1094900565 back in game's config.\n"),
        "{}",
        stdout(&output)
    );
    let restored = f.created_config("game");
    assert!(restored.starts_with("# saved comment\n"), "{restored}");
    assert!(restored.contains("gog_id = \"1094900565\""), "{restored}");
    assert_eq!(f.gog_state(&folder).unwrap().owner.as_deref(), Some("game"));
}

#[test]
fn a_restore_naming_another_gog_game_is_refused_before_anything_changes() {
    let Some(f) = fixture() else { return };
    let (prefix, config) = environment(&f);
    let other = config.replacen("# saved comment\n", "# saved comment\ngog_id = \"1\"\n", 1);
    fs::write(f.config_dir().join("environments/game.toml"), &other).unwrap();
    assert_success(&f.run(&["snapshot", "create", "game", "other-game"]));
    let migrated = config.replacen(
        "# saved comment\n",
        "# saved comment\ngog_id = \"1094900565\"\n",
        1,
    );
    fs::write(f.config_dir().join("environments/game.toml"), &migrated).unwrap();
    fs::write(prefix.join("drive_c/Games/save.dat"), b"after").unwrap();
    let folder = f.root.path().join("Games/game");
    fs::create_dir_all(&folder).unwrap();
    f.gog_folder(&folder, Some("game"), 1094900565, false);

    let output = f.run(&["snapshot", "restore", "game", "other-game", "--yes"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output)
            .contains("The snapshot's gog_id 1 doesn't match game's GOG game 1094900565."),
        "{}",
        stderr(&output)
    );
    assert_eq!(f.created_config("game"), migrated);
    assert_eq!(
        fs::read(prefix.join("drive_c/Games/save.dat")).unwrap(),
        b"after"
    );
}

#[test]
fn an_unrelated_unreadable_gog_state_doesnt_block_a_restore() {
    let Some(f) = fixture() else { return };
    environment(&f);
    assert_success(&f.run(&["snapshot", "create", "game", "plain"]));
    let dir = f.paths().gog_folders_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("bad.toml"), "version = 9\nfolder = \"/x\"\n").unwrap();
    assert_success(&f.run(&["snapshot", "restore", "game", "plain", "--yes"]));
}

#[test]
fn a_restore_fails_closed_on_an_unreadable_gog_state_when_either_config_has_gog_id() {
    let Some(f) = fixture() else { return };
    let (prefix, config) = environment(&f);
    let tracked = config.replacen(
        "# saved comment\n",
        "# saved comment\ngog_id = \"1094900565\"\n",
        1,
    );
    let config_path = f.config_dir().join("environments/game.toml");
    // The snapshot's config has gog_id and the current one doesn't, then
    // the other way around.
    fs::write(&config_path, &tracked).unwrap();
    assert_success(&f.run(&["snapshot", "create", "game", "tracked"]));
    fs::write(&config_path, &config).unwrap();
    assert_success(&f.run(&["snapshot", "create", "game", "plain"]));
    let dir = f.paths().gog_folders_dir();
    fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad.toml");
    fs::write(&bad, "version = 9\nfolder = \"/x\"\n").unwrap();
    fs::write(prefix.join("drive_c/Games/save.dat"), b"after").unwrap();

    for (label, current) in [("tracked", &config), ("plain", &tracked)] {
        fs::write(&config_path, current).unwrap();
        let output = f.run(&["snapshot", "restore", "game", label, "--yes"]);
        assert!(!output.status.success(), "{label}");
        assert!(
            stderr(&output).contains(&format!(
                "game's GOG state can't be checked: {} isn't a valid GOG folder state (unsupported GOG folder state version 9).",
                bad.display()
            )),
            "{label}: {}",
            stderr(&output)
        );
        assert_eq!(&f.created_config("game"), current, "{label}");
        assert_eq!(
            fs::read(prefix.join("drive_c/Games/save.dat")).unwrap(),
            b"after",
            "{label}"
        );
    }
}

/// A real bind mount needs a mount namespace. Run with
/// `cargo test -p wineo-cli --test snapshot restore_refuses_a_real_bind_mount -- --ignored`;
/// it needs `unshare` and unprivileged user namespaces, and skips without them.
#[test]
#[ignore]
fn restore_refuses_a_real_bind_mount() {
    let Some(f) = fixture() else { return };
    let (Some(unshare), Some(mount)) = (
        common::find_on_path("unshare"),
        common::find_on_path("mount"),
    ) else {
        return;
    };
    if !Command::new(&unshare)
        .args(["-rm", "true"])
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }
    let (prefix, _) = environment(&f);
    assert_success(&f.run(&["snapshot", "create", "game", "one"]));
    // Same filesystem as the prefix, so only the mount ID tells them apart.
    let external = f.root.path().join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("keep.txt"), b"external").unwrap();
    let target = prefix.join("drive_c/shared");
    fs::create_dir(&target).unwrap();

    let wineo = f.wineo();
    let mut command = Command::new(&unshare);
    for (key, value) in wineo.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let output = command
        .args([
            "-rm",
            "/bin/sh",
            "-c",
            r#"m=$1; shift; "$m" --bind "$1" "$2" && shift 2 && exec "$@""#,
            "sh",
        ])
        .arg(&mount)
        .arg(&external)
        .arg(&target)
        .arg(wineo.get_program())
        .args(["snapshot", "restore", "game", "one", "--yes"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains(&format!(
            "game's prefix has a mount at {}. Unmount it first.",
            target.display()
        )),
        "{}",
        stderr(&output)
    );
    assert_eq!(fs::read(external.join("keep.txt")).unwrap(), b"external");
}
