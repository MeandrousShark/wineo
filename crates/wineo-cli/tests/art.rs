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
    assert_success(&f.run(&["art", "set", "hades.server", "icon", &svg]));
    assert_eq!(fs::read(art(&f).join("grid.png")).unwrap(), PNG);
    assert_eq!(
        fs::read(art(&f).join("server/icon.svg")).unwrap(),
        b"<svg/>"
    );
    let record = fs::read_to_string(art(&f).join("art.toml")).unwrap();
    assert!(record.contains("manual = \"Cover Art.png\""), "{record}");
    assert!(record.contains("[targets.server.icon]"), "{record}");

    assert_success(&f.run(&["art", "rm", "hades.server", "icon"]));
    assert!(!art(&f).join("server").exists());
    assert_success(&f.run(&["art", "rm", "hades", "grid"]));
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
