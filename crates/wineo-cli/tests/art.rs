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
