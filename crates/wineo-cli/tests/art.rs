//! `wineo art set` and `wineo art rm`.

mod common;

use std::fs;
use std::path::PathBuf;

use common::*;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

fn art(f: &Fixture) -> PathBuf {
    f.root.path().join("data/wineo/art/hades")
}

fn image(f: &Fixture, name: &str, bytes: &[u8]) -> String {
    let path = f.root.path().join(name);
    fs::write(&path, bytes).unwrap();
    path.to_str().unwrap().to_owned()
}

#[test]
fn set_and_rm_an_environments_and_a_targets_art() {
    let f = Fixture::new();
    f.environment("hades", "[targets.server]\nexe = \"server.exe\"\n");
    let png = image(&f, "Cover Art.png", PNG);
    let svg = image(&f, "server.svg", b"<svg/>");

    let set = f.run(&["art", "set", "hades", "grid", &png]);
    assert_success(&set);
    assert_eq!(stdout(&set), "Set grid for hades\n");
    let set = f.run(&["art", "set", "hades.server", "icon", &svg]);
    assert_success(&set);
    assert_eq!(stdout(&set), "Set icon for hades.server\n");
    assert_eq!(fs::read(art(&f).join("grid.png")).unwrap(), PNG);
    assert_eq!(
        fs::read(art(&f).join("server/icon.svg")).unwrap(),
        b"<svg/>"
    );
    let record = fs::read_to_string(art(&f).join("art.toml")).unwrap();
    assert!(record.contains("manual = \"Cover Art.png\""), "{record}");
    assert!(record.contains("[targets.server.icon]"), "{record}");

    let removed = f.run(&["art", "rm", "hades.server", "icon"]);
    assert_success(&removed);
    assert_eq!(stdout(&removed), "Removed icon from hades.server\n");
    assert!(!art(&f).join("server").exists());
    let removed = f.run(&["art", "rm", "hades", "grid"]);
    assert_success(&removed);
    assert_eq!(stdout(&removed), "Removed grid from hades\n");
    assert!(!art(&f).join("grid.png").exists());
    let again = f.run(&["art", "rm", "hades", "grid"]);
    assert!(!again.status.success());
    assert_eq!(stderr(&again).trim(), "error: `hades` has no grid art");
}

#[test]
fn set_refuses_bad_files_kinds_and_spots() {
    let f = Fixture::new();
    f.environment("hades", "[targets.server]\nexe = \"server.exe\"\n");
    let text = image(&f, "notes.png", b"just notes");
    let png = image(&f, "a.png", PNG);

    let refused = f.run(&["art", "set", "hades", "icon", &text]);
    assert!(!refused.status.success());
    assert_eq!(
        stderr(&refused).trim(),
        format!("error: {text} is not a PNG, SVG, ICO, or JPEG image")
    );
    let refused = f.run(&["art", "set", "hades", "banner", &png]);
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("banner"), "{}", stderr(&refused));
    let refused = f.run(&["art", "set", "hades.nope", "icon", &png]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("has no target `nope`"),
        "{}",
        stderr(&refused)
    );
    let refused = f.run(&["art", "set", "nope", "icon", &png]);
    assert!(!refused.status.success());
    assert!(!f.root.path().join("data/wineo/art").exists());
}

#[test]
fn list_matches_the_plan_example() {
    let f = Fixture::new();
    f.environment("hades", "[targets.server]\nexe = \"server.exe\"\n");
    let dir = art(&f);
    fs::create_dir_all(dir.join("server")).unwrap();
    fs::write(
        dir.join("art.toml"),
        "schema_version = 1\ngame = 5247\n\n[icon]\nfile = \"icon.png\"\nsteamgriddb = 36952\n\n\
         [grid]\nfile = \"grid.jpg\"\nmanual = \"poster final.jpg\"\n\n\
         [targets.server.icon]\nfile = \"icon.svg\"\nmanual = \"server.svg\"\n",
    )
    .unwrap();
    for name in [
        "icon.png",
        "grid.jpg",
        "grid.png",
        "hero.webp",
        "server/icon.svg",
    ] {
        fs::write(dir.join(name), b"x").unwrap();
    }
    let listed = f.run(&["art", "list", "hades"]);
    assert_success(&listed);
    assert_eq!(
        stdout(&listed),
        "hades (SteamGridDB game 5247)
  icon  icon.png   SteamGridDB 36952
  grid  grid.jpg   manual, from poster final.jpg
  hero  hero.webp  unrecorded
  logo  none
hades.server
  icon  icon.svg   manual, from server.svg
"
    );
    assert_eq!(
        stderr(&listed),
        "warning: grid.png ignored, grid.jpg is recorded\n"
    );
}

#[test]
fn list_warns_about_stale_records_and_leaves_them() {
    let f = Fixture::new();
    f.environment("hades", "[targets.server]\nexe = \"server.exe\"\n");
    let dir = art(&f);
    fs::create_dir_all(dir.join("server")).unwrap();
    let record = "schema_version = 1\n[targets.server.logo]\nfile = \"logo.png\"\nmanual = \"a.png\"\n\
                  [targets.old.icon]\nfile = \"icon.png\"\nmanual = \"b.png\"\n";
    fs::write(dir.join("art.toml"), record).unwrap();
    for name in ["server/hero.png", "server/hero.jpg"] {
        fs::write(dir.join(name), b"x").unwrap();
    }
    let listed = f.run(&["art", "list", "hades"]);
    assert_success(&listed);
    assert_eq!(
        stdout(&listed),
        "hades
  icon  none
  grid  none
  hero  none
  logo  none
hades.server
  hero  hero.png  unrecorded
"
    );
    assert_eq!(
        stderr(&listed),
        "warning: server/hero.jpg ignored, server/hero.png takes precedence
warning: server/logo.png is missing
warning: art for unknown target `old`
"
    );
    assert_eq!(fs::read_to_string(dir.join("art.toml")).unwrap(), record);

    let missing = f.run(&["art", "list", "nope"]);
    assert!(!missing.status.success());
}

#[test]
fn key_removal_and_empty_input_never_reach_the_network() {
    let f = Fixture::new();
    let file = f.root.path().join("data/wineo/steamgriddb.key");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "abc\n").unwrap();

    let removed = f.run(&["art", "key", "--remove"]);
    assert_success(&removed);
    assert_eq!(stdout(&removed), "Removed the SteamGridDB key\n");
    assert!(!file.exists());
    let again = f.run(&["art", "key", "--remove"]);
    assert_success(&again);
    assert_eq!(stdout(&again), "No SteamGridDB key was saved\n");

    let empty = f
        .wineo()
        .args(["art", "key"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!empty.status.success());
    assert_eq!(stderr(&empty), "error: no key entered\n");
    assert!(!file.exists());
}

#[test]
fn key_notes_a_variable_that_overrides_the_saved_key() {
    let f = Fixture::new();
    let removed = f
        .wineo()
        .args(["art", "key", "--remove"])
        .env("WINEO_STEAMGRIDDB_KEY", "abc")
        .output()
        .unwrap();
    assert_success(&removed);
    assert_eq!(
        stderr(&removed),
        "note: WINEO_STEAMGRIDDB_KEY is set and overrides the saved key\n"
    );
    let empty = f
        .wineo()
        .args(["art", "key"])
        .env("WINEO_STEAMGRIDDB_KEY", "abc")
        .output()
        .unwrap();
    assert_eq!(
        stderr(&empty),
        "note: WINEO_STEAMGRIDDB_KEY is set and overrides the saved key\nerror: no key entered\n"
    );
    let unset = f
        .wineo()
        .args(["art", "key", "--remove"])
        .env("WINEO_STEAMGRIDDB_KEY", "")
        .output()
        .unwrap();
    assert_eq!(stderr(&unset), "");
}

#[test]
fn fetch_stops_before_the_network_without_a_key_or_a_spot() {
    let f = Fixture::new();
    f.environment("hades", "[targets.server]\nexe = \"server.exe\"\n");
    let missing = f.run(&["art", "fetch", "hades"]);
    assert!(!missing.status.success());
    assert_eq!(
        stderr(&missing),
        "error: no SteamGridDB key, run `wineo art key` or set WINEO_STEAMGRIDDB_KEY\n"
    );
    assert!(!art(&f).exists());

    let bad = f
        .wineo()
        .args(["art", "fetch", "hades.nope"])
        .env("WINEO_STEAMGRIDDB_KEY", "abc")
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(
        stderr(&bad).contains("has no target `nope`"),
        "{}",
        stderr(&bad)
    );

    for args in [
        &["art", "fetch", "hades", "--asset", "5"][..],
        &["art", "fetch", "hades", "--game", "1", "--search", "x"],
        &[
            "art",
            "fetch",
            "hades",
            "--only",
            "icon",
            "--asset",
            "5",
            "--replace",
        ],
        &["art", "fetch", "hades", "--only", "banner"],
    ] {
        let refused = f.run(args);
        assert!(!refused.status.success(), "{args:?}");
        assert!(
            stderr(&refused).contains("Usage:") || stderr(&refused).contains("invalid value"),
            "{args:?}: {}",
            stderr(&refused)
        );
    }
}

/// A 1x1 PNG that decodes, unlike `PNG`, which only has the magic.
const REAL_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\rIDAT\x78\x9c\x63\xf8\xcf\xc0\xf0\x1f\0\x05\0\x01\xff\x89\x99\x3d\x1d\0\0\0\0IEND\xae\x42\x60\x82";

#[test]
fn icon_art_reaches_the_desktop_entry_right_away() {
    let f = Fixture::new();
    f.environment(
        "hades",
        "[targets.game]\nexe = \"game.exe\"\ndesktop = true\n",
    );
    let entry = f
        .root
        .path()
        .join("data/applications/wineo-hades-game.desktop");
    let icon = f
        .root
        .path()
        .join("data/icons/hicolor/16x16/apps/wineo-hades-game.png");
    let png = image(&f, "icon.png", REAL_PNG);

    let set = f.run(&["art", "set", "hades", "icon", &png]);
    assert_success(&set);
    assert_eq!(stdout(&set), "Set icon for hades\n");
    assert_eq!(stderr(&set), "Added a desktop entry for game in hades\n");
    let text = fs::read_to_string(&entry).unwrap();
    assert!(
        text.contains("\nIcon=wineo-hades-game\nX-Wineo-Icon=art:"),
        "{text}"
    );
    assert!(icon.is_file());

    // Setting the same file again changes nothing, so nothing is said.
    let again = f.run(&["art", "set", "hades", "icon", &png]);
    assert_success(&again);
    assert_eq!(stderr(&again), "");

    let removed = f.run(&["art", "rm", "hades", "icon"]);
    assert_success(&removed);
    assert_eq!(
        stderr(&removed),
        "Updated the desktop entry for game in hades\n"
    );
    let text = fs::read_to_string(&entry).unwrap();
    assert!(!text.contains("Icon="), "{text}");
    assert!(!icon.exists());
}

#[test]
fn an_env_without_entries_says_nothing_about_them() {
    let f = Fixture::new();
    f.environment("hades", "[targets.game]\nexe = \"game.exe\"\n");
    let png = image(&f, "icon.png", REAL_PNG);
    let set = f.run(&["art", "set", "hades", "icon", &png]);
    assert_success(&set);
    assert_eq!(stderr(&set), "");
    let removed = f.run(&["art", "rm", "hades", "icon"]);
    assert_success(&removed);
    assert_eq!(stderr(&removed), "");
    assert!(!f.root.path().join("data/applications").exists());
}

#[test]
fn portable_art_says_nothing_about_desktop_entries() {
    let f = Fixture::new();
    let dir = f.root.path().join("usb");
    let config = dir.join("config/environments/hades.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "prefix = 'prefixes/hades'\nrunner = 'wine'\narch = 'win64'\n\n[targets.game]\nexe = 'game.exe'\ndesktop = true\n",
    )
    .unwrap();
    let png = image(&f, "icon.png", REAL_PNG);
    let portable = |args: &[&str]| {
        f.wineo()
            .arg("--portable")
            .arg(&dir)
            .args(args)
            .output()
            .unwrap()
    };

    let set = portable(&["art", "set", "hades", "icon", &png]);
    assert_success(&set);
    assert_eq!(stdout(&set), "Set icon for hades\n");
    assert_eq!(stderr(&set), "");
    let removed = portable(&["art", "rm", "hades", "icon"]);
    assert_success(&removed);
    assert_eq!(stderr(&removed), "");
    assert!(!f.root.path().join("data/applications").exists());
}

#[test]
fn new_with_art_and_no_key_notes_it_and_succeeds() {
    let f = Fixture::new();
    let created = f.run(&["new", "-n", "hades", "--art"]);
    assert_success(&created);
    assert!(
        stdout(&created).contains("Created environment hades"),
        "{}",
        stdout(&created)
    );
    assert_eq!(
        stderr(&created)
            .lines()
            .filter(|line| line.contains("SteamGridDB"))
            .collect::<Vec<_>>(),
        ["note: run `wineo art key` to fetch art from SteamGridDB"]
    );
    assert!(!art(&f).join("art.toml").exists());

    let plain = f.run(&["new", "-n", "zagreus"]);
    assert_success(&plain);
    assert!(
        !stderr(&plain).contains("SteamGridDB") && !stdout(&plain).contains("SteamGridDB"),
        "{}",
        stderr(&plain)
    );
}

/// `library.toml`'s bytes, or None while there is none.
fn library(f: &Fixture) -> Option<Vec<u8>> {
    fs::read(f.paths().library_file()).ok()
}

fn assert_refused(output: &std::process::Output, error: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", stderr(output));
    assert_eq!(stderr(output), format!("error: {error}\n"));
}

#[test]
fn set_and_rm_keep_their_errors_and_leave_the_library_alone() {
    let f = Fixture::new();
    f.environment("hades", "");
    let png = image(&f, "a.png", PNG);
    let before = library(&f);

    // The first dot starts the target, so the environment's name is empty.
    let set = f.run(&["art", "set", "../config", "grid", &png]);
    assert_refused(
        &set,
        &wineo_core::Error::InvalidName(String::new()).to_string(),
    );
    let removed = f.run(&["art", "rm", "../config", "grid"]);
    assert_refused(
        &removed,
        &wineo_core::Error::InvalidName("./config".to_owned()).to_string(),
    );
    for output in [
        f.run(&["art", "set", "nope", "grid", &png]),
        f.run(&["art", "rm", "nope", "grid"]),
    ] {
        assert_refused(&output, "environment `nope` does not exist");
    }
    assert_eq!(library(&f), before);
}

#[test]
fn set_and_rm_keep_a_pending_trash_entrys_error() {
    let f = Fixture::new();
    f.environment("hades", "");
    let png = image(&f, "a.png", PNG);
    let removed = f.run(&["rm", "hades"]);
    assert_success(&removed);
    let paths = f.paths();
    let manifest = paths
        .trash_dir()
        .join(trash_id(&removed))
        .join("manifest.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    assert!(text.contains("phase = \"trashed\""), "{text}");
    fs::write(
        &manifest,
        text.replace("phase = \"trashed\"", "phase = \"removing\""),
    )
    .unwrap();
    let pending = wineo_core::environment::Environment::ensure_exists(&paths, "hades")
        .unwrap_err()
        .to_string();
    assert!(!pending.contains("does not exist"), "{pending}");
    let before = library(&f);

    assert_refused(&f.run(&["art", "set", "hades", "grid", &png]), &pending);
    assert_refused(&f.run(&["art", "rm", "hades", "grid"]), &pending);
    assert_eq!(library(&f), before);
}

#[test]
fn an_interrupted_restore_with_its_config_missing_does_not_exist_for_art() {
    let f = Fixture::new();
    f.environment("hades", "");
    let png = image(&f, "a.png", PNG);
    let paths = f.paths();
    let marker = paths.restore_marker("hades");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, "{}").unwrap();
    let interrupted = f.run(&["art", "set", "hades", "grid", &png]);
    assert!(
        stderr(&interrupted).contains("interrupted snapshot restore"),
        "{}",
        stderr(&interrupted)
    );

    fs::remove_file(paths.environment_config("hades")).unwrap();
    assert_refused(
        &f.run(&["art", "set", "hades", "grid", &png]),
        "environment `hades` does not exist",
    );
    // A target's spot reads the environment, which says the restore first,
    // as it always has.
    let target = f.run(&["art", "set", "hades.server", "icon", &png]);
    assert_eq!(target.status.code(), Some(1));
    assert!(
        stderr(&target).contains("interrupted snapshot restore"),
        "{}",
        stderr(&target)
    );
    assert_refused(
        &f.run(&["art", "rm", "hades", "grid"]),
        "environment `hades` does not exist",
    );
}

#[test]
fn set_and_rm_give_an_environment_from_before_ids_one() {
    let f = Fixture::new();
    let paths = f.paths();
    let png = image(&f, "a.png", PNG);
    for (name, args) in [
        ("hades", vec!["art", "set", "hades", "grid", png.as_str()]),
        ("zagreus", vec!["art", "rm", "zagreus", "grid"]),
    ] {
        f.environment(name, "");
        if name == "zagreus" {
            fs::create_dir_all(paths.art_dir(name)).unwrap();
            fs::write(paths.art_dir(name).join("grid.png"), PNG).unwrap();
        }
        let id = || {
            wineo_core::library::load(&paths)
                .environments
                .get(name)
                .and_then(|entry| entry.id)
        };
        assert_eq!(id(), None);
        assert_success(&f.run(&args));
        assert!(id().is_some(), "{name}");
    }
}

#[test]
fn a_set_that_cant_write_gives_an_old_environment_no_id() {
    use wineo_core::art::Kind;
    let f = Fixture::new();
    f.environment("hades", "[targets.server]\nexe = \"server.exe\"\n");
    let png = image(&f, "a.png", PNG);
    let text = image(&f, "notes.png", b"just notes");
    let missing = f.root.path().join("missing.png").display().to_string();
    let folder = f.root.path().join("folder.png");
    fs::create_dir_all(&folder).unwrap();
    let folder = folder.display().to_string();
    let big = f.root.path().join("big.png");
    fs::File::create(&big)
        .unwrap()
        .set_len(wineo_core::art::MAX_BYTES + 1)
        .unwrap();
    let big = big.display().to_string();
    for (spot, kind, file, expected) in [
        (
            "hades.nope",
            Kind::Icon,
            png.as_str(),
            "environment `hades` has no target `nope`".to_owned(),
        ),
        (
            "hades.bad name",
            Kind::Icon,
            png.as_str(),
            "environment `hades` has no target `bad name`".to_owned(),
        ),
        (
            "hades",
            Kind::Icon,
            missing.as_str(),
            format!("failed to read {missing}: No such file or directory (os error 2)"),
        ),
        (
            "hades",
            Kind::Icon,
            folder.as_str(),
            format!("failed to read {folder}: Is a directory (os error 21)"),
        ),
        (
            "hades",
            Kind::Hero,
            big.as_str(),
            format!("{big} is larger than 32 MiB"),
        ),
        (
            "hades",
            Kind::Icon,
            text.as_str(),
            format!("{text} is not a PNG, SVG, ICO, or JPEG image"),
        ),
    ] {
        let set = f.run(&["art", "set", spot, kind.as_str(), file]);
        assert_refused(&set, &expected);
        assert_eq!(library(&f), None, "{spot} {file}");
    }

    // A target whose config doesn't parse.
    let config = f.paths().environment_config("hades");
    fs::write(&config, "this isn't toml [").unwrap();
    assert_refused(
        &f.run(&["art", "set", "hades.server", "icon", &png]),
        &format!(
            "invalid config {}: TOML parse error at line 1, column 6\n  |\n\
             1 | this isn't toml [\n  |      ^\nkey with no value, expected `=`\n",
            config.display()
        ),
    );
    assert_eq!(library(&f), None);
}

#[test]
fn an_rm_with_nothing_to_remove_gives_an_old_environment_no_id() {
    let f = Fixture::new();
    f.environment("hades", "[targets.server]\nexe = \"server.exe\"\n");
    for (spot, error) in [
        ("hades", "`hades` has no grid art"),
        ("hades.server", "`hades.server` has no grid art"),
        ("hades.nope", "`hades.nope` has no grid art"),
        (
            "hades.bad name",
            "invalid name `bad name`: use letters, digits, `-`, and `_`",
        ),
    ] {
        assert_refused(&f.run(&["art", "rm", spot, "grid"]), error);
        assert_eq!(library(&f), None, "{spot}");
    }
}
