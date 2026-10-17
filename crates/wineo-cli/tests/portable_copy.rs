//! `wineo portable copy`, from the standard layout into a portable folder.

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;
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

/// An XDG environment with internal, Windows, external, and home targets, a
/// global config, and a template. Snapshots need reflinks, so a fixture on a
/// filesystem without them is skipped.
fn fixture() -> Option<(Fixture, PathBuf)> {
    let f = Fixture::new_in(Path::new(env!("CARGO_MANIFEST_DIR")));
    if !reflink_supported(f.root.path()) {
        return None;
    }
    let prefix = f.root.path().join("prefixes/game");
    f.environment(
        "game",
        &format!(
            "[env]\nSOURCE_NOTE = '{prefix}/notes'\n\n[targets.game]\nexe = '{prefix}/drive_c/game.exe'\ncwd = '{prefix}/drive_c'\ndesktop = true\ndefault = true\n\n[targets.windows]\nexe = 'C:\\Games\\win.exe'\n\n[targets.external]\nexe = '/opt/external/game.exe'\n\n[targets.home]\nexe = '~/Games/home.exe'\n",
            prefix = prefix.display()
        ),
    );
    fs::write(prefix.join("drive_c/game.exe"), "game").unwrap();
    fs::write(prefix.join("drive_c/save.dat"), "first").unwrap();
    f.global_config(&format!(
        "[defaults]\nprefix_root = '{}'\nrunner_root = '/opt/runners'\n",
        f.root.path().join("prefixes").display()
    ));
    f.write_template("gaming", "gamemode = true\n");
    Some((f, prefix))
}

fn copy(f: &Fixture, name: &str, dir: &Path) -> std::process::Output {
    f.wineo()
        .args(["portable", "copy", name, "--to"])
        .arg(dir)
        .output()
        .unwrap()
}

fn portable(f: &Fixture, dir: &Path, args: &[&str]) -> std::process::Output {
    f.wineo()
        .arg("--portable")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn a_copy_carries_targets_and_snapshots_and_leaves_the_source_alone() {
    let Some((f, prefix)) = fixture() else { return };
    assert_success(&f.run(&["snapshot", "create", "game", "one"]));
    fs::write(prefix.join("drive_c/save.dat"), "second").unwrap();
    assert_success(&f.run(&["snapshot", "create", "game", "two"]));
    let source_config = f.created_config("game");
    let source_global = read(f.config_dir().join("config.toml"));
    let dir = f.root.path().join("usb");

    let copied = copy(&f, "game", &dir);
    assert_success(&copied);
    let review = stdout(&copied);
    assert!(!review.contains("art:"), "{review}");
    for expected in [
        "snapshots: one, two",
        "change: targets.game.desktop disabled",
        "external target path: targets.external.exe: /opt/external/game.exe",
        "source path in settings: env.SOURCE_NOTE",
        "global config: copied",
        "templates: gaming",
        "storage: reflinked",
    ] {
        assert!(review.contains(expected), "{expected}: {review}");
    }

    assert_eq!(f.created_config("game"), source_config);
    assert_eq!(read(f.config_dir().join("config.toml")), source_global);
    assert_eq!(read(prefix.join("drive_c/save.dat")), "second");
    assert!(stdout(&f.run(&["snapshot", "list", "game"])).contains("two"));

    let config = read(dir.join("config/environments/game.toml"));
    for expected in [
        "prefix = \"prefixes/game\"",
        "exe = \"prefixes/game/drive_c/game.exe\"",
        "cwd = \"prefixes/game/drive_c\"",
        "desktop = false",
        "exe = 'C:\\Games\\win.exe'",
        "exe = '/opt/external/game.exe'",
        "exe = '~/Games/home.exe'",
    ] {
        assert!(config.contains(expected), "{expected}: {config}");
    }
    let global = read(dir.join("config/config.toml"));
    assert!(global.contains("prefix_root = \"prefixes\""), "{global}");
    assert!(global.contains("runner_root = '/opt/runners'"), "{global}");
    assert_eq!(
        read(dir.join("config/templates/gaming.toml")),
        "gamemode = true\n"
    );

    let moved = f.root.path().join("moved");
    fs::rename(&dir, &moved).unwrap();
    let listed = stdout(&portable(&f, &moved, &["snapshot", "list", "game"]));
    assert!(listed.contains("one") && listed.contains("two"), "{listed}");
    let saved = read(moved.join("prefixes/.wineo-snapshots/game/one/config.toml"));
    assert!(saved.contains("prefix = \"prefixes/game\""), "{saved}");
    assert_success(&portable(
        &f,
        &moved,
        &["snapshot", "restore", "game", "one", "--yes"],
    ));
    assert_eq!(read(moved.join("prefixes/game/drive_c/save.dat")), "first");
    assert_eq!(read(prefix.join("drive_c/save.dat")), "second");
}

#[test]
fn later_copies_keep_the_folders_global_config_and_templates() {
    let Some((f, _)) = fixture() else { return };
    f.environment("other", "");
    let dir = f.root.path().join("usb");
    assert_success(&copy(&f, "game", &dir));
    fs::write(dir.join("config/config.toml"), "# mine\n").unwrap();
    fs::remove_file(dir.join("config/templates/gaming.toml")).unwrap();

    let copied = copy(&f, "other", &dir);
    assert_success(&copied);
    assert!(
        stdout(&copied).contains("global config and templates: the folder's own are kept"),
        "{}",
        stdout(&copied)
    );
    assert!(!stdout(&copied).contains("templates: gaming"));
    assert_eq!(read(dir.join("config/config.toml")), "# mine\n");
    assert!(!dir.join("config/templates/gaming.toml").exists());
    assert!(read(dir.join("config/environments/other.toml")).contains("prefixes/other"));
}

#[test]
fn collisions_overlaps_and_busy_sources_are_refused() {
    let Some((f, prefix)) = fixture() else { return };
    let dir = f.root.path().join("usb");
    assert_success(&copy(&f, "game", &dir));
    let again = copy(&f, "game", &dir);
    assert!(!again.status.success());
    assert!(
        stderr(&again).contains("already exists"),
        "{}",
        stderr(&again)
    );

    for inside in [prefix.join("usb"), f.config_dir().join("usb")] {
        let overlap = copy(&f, "game", &inside);
        assert!(!overlap.status.success());
        assert!(
            stderr(&overlap).contains("overlaps"),
            "{}",
            stderr(&overlap)
        );
        assert!(!inside.exists());
    }

    let other = f.root.path().join("other");
    let from_portable = f
        .wineo()
        .arg("--portable")
        .arg(&dir)
        .args(["portable", "copy", "game", "--to"])
        .arg(&other)
        .output()
        .unwrap();
    assert!(!from_portable.status.success());
    assert!(stderr(&from_portable).contains("drop --portable"));

    let locks = f.root.path().join("state/wineo/.locks");
    fs::create_dir_all(&locks).unwrap();
    let marker = locks.join("snapshot-restore-game-restore.json");
    fs::write(&marker, "{}").unwrap();
    let marked = copy(&f, "game", &other);
    assert!(!marked.status.success());
    assert!(
        stderr(&marked).contains("snapshot recover game"),
        "{}",
        stderr(&marked)
    );
    fs::remove_file(&marker).unwrap();

    f.environment("app", "[targets.app]\nexe = \"app\"\n");
    assert_success(&f.run(&["start", "app"]));
    let running = copy(&f, "app", &other);
    assert!(!running.status.success());
    assert_success(&f.run(&["stop", "app"]));
    assert!(!other.join("prefixes/app").exists());
    assert!(!other.join("config/environments/app.toml").exists());
}

#[test]
fn an_interrupted_copy_is_recovered_by_moving_its_files_aside() {
    let Some((f, _)) = fixture() else { return };
    let dir = f.root.path().join("usb");
    // What a copy killed after publishing the prefix leaves behind, plus a
    // staging directory it never cleaned up.
    fs::create_dir_all(dir.join("prefixes/game/drive_c")).unwrap();
    fs::create_dir_all(dir.join("prefixes/.wineo-copy-leftover/prefix")).unwrap();
    let blocked = copy(&f, "game", &dir);
    assert!(!blocked.status.success());
    assert!(
        stderr(&blocked).contains("move it aside"),
        "{}",
        stderr(&blocked)
    );
    assert!(!dir.join("config/environments/game.toml").exists());

    fs::rename(dir.join("prefixes/game"), f.root.path().join("aside")).unwrap();
    assert_success(&copy(&f, "game", &dir));
    assert!(dir.join("prefixes/game/system.reg").exists());
    assert_eq!(names(&portable(&f, &dir, &["list"])), ["game"]);
}

#[test]
fn a_destination_without_reflinks_is_reported() {
    let Some((f, _)) = fixture() else { return };
    let elsewhere = tempfile::tempdir().unwrap();
    let dir = elsewhere.path().join("usb");
    let same_device =
        fs::metadata(f.root.path()).unwrap().dev() == fs::metadata(elsewhere.path()).unwrap().dev();
    if same_device {
        return;
    }
    assert_success(&f.run(&["snapshot", "create", "game", "one"]));
    let copied = copy(&f, "game", &dir);
    assert_success(&copied);
    let review = stdout(&copied);
    assert!(review.contains("no reflink here"), "{review}");
    assert!(review.contains("needs a reflink filesystem"), "{review}");
    assert!(
        dir.join("prefixes/.wineo-snapshots/game/one/prefix")
            .exists()
    );
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
fn a_folder_with_a_config_dir_keeps_its_own_even_without_a_global_config() {
    let Some((f, _)) = fixture() else { return };
    let dir = f.root.path().join("usb");
    assert_success(&portable(&f, &dir, &["new", "-n", "fresh"]));
    assert!(dir.join("config").is_dir());
    assert!(!dir.join("config/config.toml").exists());

    let copied = copy(&f, "game", &dir);
    assert_success(&copied);
    let review = stdout(&copied);
    assert!(
        review.contains("global config and templates: the folder's own are kept"),
        "{review}"
    );
    assert!(!dir.join("config/config.toml").exists());
    assert!(!dir.join("config/templates").exists());
}

#[test]
fn links_in_snapshots_follow_the_copied_prefix() {
    let Some((f, prefix)) = fixture() else { return };
    assert_success(&f.run(&["snapshot", "create", "game", "one"]));
    let saved = prefix
        .parent()
        .unwrap()
        .join(".wineo-snapshots/game/one/prefix/drive_c");
    std::os::unix::fs::symlink(prefix.join("drive_c/game.exe"), saved.join("live")).unwrap();
    std::os::unix::fs::symlink("/opt/external/thing", saved.join("outside")).unwrap();
    let dir = f.root.path().join("usb");

    let copied = f
        .wineo()
        .args(["-v", "portable", "copy", "game", "--to"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_success(&copied);
    let review = stdout(&copied);
    assert!(
        review.contains("snapshot external links: 1, kept in place\n")
            && review.contains("snapshot external:")
            && review.contains("-> /opt/external/thing"),
        "{review}"
    );
    assert!(
        !review.contains(&format!("-> {}", prefix.join("drive_c/game.exe").display())),
        "{review}"
    );

    assert_success(&portable(
        &f,
        &dir,
        &["snapshot", "restore", "game", "one", "--yes"],
    ));
    let live = dir.join("prefixes/game/drive_c");
    // Restoring inside a portable folder writes the link relative, and it
    // resolves into the portable prefix.
    assert_eq!(
        fs::read_link(live.join("live")).unwrap(),
        Path::new("game.exe")
    );
    assert_eq!(
        fs::canonicalize(live.join("live")).unwrap(),
        fs::canonicalize(live.join("game.exe")).unwrap()
    );
    assert_eq!(
        fs::read_link(live.join("outside")).unwrap(),
        Path::new("/opt/external/thing")
    );
    assert_eq!(
        fs::read_link(saved.join("live")).unwrap(),
        prefix.join("drive_c/game.exe")
    );
}

#[test]
fn art_follows_the_copy_and_a_move() {
    let Some((f, _)) = fixture() else { return };
    let source = f.root.path().join("data/wineo/art/game");
    fs::create_dir_all(source.join("game")).unwrap();
    fs::write(source.join("hero.png"), b"hero").unwrap();
    fs::write(source.join("game/icon.svg"), b"<svg/>").unwrap();
    let dir = f.root.path().join("usb");
    fs::create_dir_all(dir.join("art/game")).unwrap();
    let blocked = copy(&f, "game", &dir);
    assert!(!blocked.status.success());
    assert!(
        stderr(&blocked).contains("move it aside"),
        "{}",
        stderr(&blocked)
    );
    assert!(!dir.join("config/environments/game.toml").exists());

    fs::remove_dir(dir.join("art/game")).unwrap();
    let copied = copy(&f, "game", &dir);
    assert_success(&copied);
    assert!(
        stdout(&copied).contains("\n  art: copied\n"),
        "{}",
        stdout(&copied)
    );
    assert_eq!(read(dir.join("art/game/hero.png")), "hero");
    assert_eq!(read(dir.join("art/game/game/icon.svg")), "<svg/>");
    assert_eq!(read(source.join("hero.png")), "hero");

    let moved = f.root.path().join("moved");
    fs::rename(&dir, &moved).unwrap();
    let removed = portable(&f, &moved, &["rm", "game"]);
    assert_success(&removed);
    let id = common::trash_id(&removed);
    assert!(!moved.join("art/game").exists());
    assert_eq!(read(source.join("hero.png")), "hero");
    assert_success(&portable(&f, &moved, &["restore", &id]));
    assert_eq!(read(moved.join("art/game/hero.png")), "hero");
    assert_eq!(read(moved.join("art/game/game/icon.svg")), "<svg/>");
    assert_eq!(read(source.join("hero.png")), "hero");
}

#[test]
fn play_stats_follow_the_copy_like_art() {
    let Some((f, _)) = fixture() else { return };
    f.write_stats_log(
        "game",
        "{\"target\":\"game\",\"started_at\":\"2026-01-01T00:00:00Z\",\"ended_at\":\"2026-01-01T01:00:00Z\",\"seconds\":3600,\"end\":\"exited\"}\n",
    );
    let dir = f.root.path().join("usb");
    let copied = copy(&f, "game", &dir);
    assert_success(&copied);
    assert_eq!(read(dir.join("stats/game.jsonl")), f.read_stats_log("game"));
}

#[test]
fn copy_refuses_to_replace_an_existing_play_log() {
    let Some((f, _)) = fixture() else { return };
    f.write_stats_log(
        "game",
        "{\"target\":\"game\",\"started_at\":\"2026-01-01T00:00:00Z\",\"ended_at\":\"2026-01-01T01:00:00Z\",\"seconds\":3600,\"end\":\"exited\"}\n",
    );
    let dir = f.root.path().join("usb");
    fs::create_dir_all(dir.join("stats")).unwrap();
    fs::write(dir.join("stats/game.jsonl"), "orphan\n").unwrap();
    let copied = copy(&f, "game", &dir);
    assert!(!copied.status.success());
    assert!(
        stderr(&copied).contains("game.jsonl already exists"),
        "{}",
        stderr(&copied)
    );
    assert_eq!(read(dir.join("stats/game.jsonl")), "orphan\n");
}

const UNKNOWN_KEY_HINT: &str =
    "A newer version of wineo may have written it; update wineo or remove the key.";

#[test]
fn a_copy_refuses_a_source_global_config_with_an_unknown_key() {
    let Some((f, _)) = fixture() else { return };
    f.global_config("foo = 1\n");
    let dir = f.root.path().join("usb");
    let copied = copy(&f, "game", &dir);
    assert!(!copied.status.success());
    let message = stderr(&copied);
    assert!(message.contains("unknown field `foo`"), "{message}");
    assert!(message.contains(UNKNOWN_KEY_HINT), "{message}");
    assert!(!dir.join("config/environments/game.toml").exists());
}

#[test]
fn a_later_copy_refuses_a_folder_global_config_with_an_unknown_key() {
    let Some((f, _)) = fixture() else { return };
    f.environment("other", "");
    let dir = f.root.path().join("usb");
    assert_success(&copy(&f, "game", &dir));
    fs::write(dir.join("config/config.toml"), "foo = 1\n").unwrap();

    let copied = copy(&f, "other", &dir);
    assert!(!copied.status.success());
    let message = stderr(&copied);
    assert!(message.contains("unknown field `foo`"), "{message}");
    assert!(message.contains(UNKNOWN_KEY_HINT), "{message}");
    assert!(!dir.join("config/environments/other.toml").exists());
}
