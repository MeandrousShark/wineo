mod common;

use std::fs;
use std::os::unix::fs::symlink;

use common::{Fixture, assert_success, stderr, stdout};

fn source(f: &Fixture) -> std::path::PathBuf {
    let prefix = f.root.path().join("source-prefix");
    fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    fs::write(prefix.join("drive_c/Games/Game.exe"), b"game").unwrap();
    fs::write(prefix.join("drive_c/Games/save.dat"), b"original save").unwrap();
    let config_dir = f.config_dir().join("environments");
    fs::create_dir_all(&config_dir).unwrap();
    let config = format!(
        "# keep this comment\nprefix = {:?}\nrunner = 'wine'\narch = 'win64'\nheroic_id = 'source-id'\n\n[targets.game]\nexe = {:?}\ndesktop = true\ndefault = true\n",
        prefix.to_str().unwrap(),
        prefix.join("drive_c/Games/Game.exe").to_str().unwrap()
    );
    fs::write(config_dir.join("zomboid.toml"), config).unwrap();
    prefix
}

#[test]
fn clone_uses_default_destination_and_keeps_source_independent() {
    let f = Fixture::new();
    let source = source(&f);
    let original_config = f.created_config("zomboid");
    let output = f.run(&["clone", "zomboid", "zomboid_modded"]);
    assert_success(&output);

    let destination = f.root.path().join("data/wineo/prefixes/zomboid_modded");
    assert_eq!(
        fs::read(destination.join("drive_c/Games/save.dat")).unwrap(),
        b"original save"
    );
    assert_eq!(f.created_config("zomboid"), original_config);
    let cloned = f.created_config("zomboid_modded");
    assert!(cloned.contains("# keep this comment"), "{cloned}");
    assert!(cloned.contains(destination.to_str().unwrap()), "{cloned}");
    assert!(cloned.contains("desktop = false"), "{cloned}");
    assert!(!cloned.contains("heroic_id"), "{cloned}");
    assert!(
        cloned.contains(destination.join("drive_c/Games/Game.exe").to_str().unwrap()),
        "{cloned}"
    );
    assert!(!source.join("reg.log").exists());
    assert!(!destination.join("reg.log").exists());

    fs::write(destination.join("drive_c/Games/save.dat"), b"modded save").unwrap();
    assert_eq!(
        fs::read(source.join("drive_c/Games/save.dat")).unwrap(),
        b"original save"
    );
    assert_success(&f.run(&["rm", "zomboid_modded", "--delete-prefix"]));
    assert!(!destination.exists());
    assert!(source.exists());
    assert_eq!(f.created_config("zomboid"), original_config);
}

#[test]
fn clone_requires_the_new_name_and_does_not_write() {
    let f = Fixture::new();
    let source = source(&f);
    let original_config = f.created_config("zomboid");
    let output = f.run(&["clone", "zomboid"]);
    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(error.contains("NEW_NAME"), "{error}");
    assert!(error.contains("Usage:"), "{error}");
    assert_eq!(f.created_config("zomboid"), original_config);
    assert_eq!(
        fs::read(source.join("drive_c/Games/save.dat")).unwrap(),
        b"original save"
    );
    assert!(
        !f.config_dir()
            .join("environments/zomboid_modded.toml")
            .exists()
    );
}

#[test]
fn clone_accepts_an_explicit_destination() {
    let f = Fixture::new();
    source(&f);
    let destination = f.root.path().join("custom-clone");
    assert_success(&f.run(&[
        "clone",
        "zomboid",
        "custom",
        "--dir",
        destination.to_str().unwrap(),
    ]));
    assert!(destination.join("system.reg").exists());
    assert!(!f.root.path().join("data/wineo/prefixes/custom").exists());
    assert_success(&f.run(&["rm", "custom", "--delete-prefix"]));
    assert!(!destination.exists());
}

#[test]
fn clone_review_summarizes_external_links_unless_verbose() {
    let f = Fixture::new();
    let prefix = source(&f);
    let external = f.root.path().join("external");
    fs::create_dir(&external).unwrap();
    for index in 0..15 {
        symlink(&external, prefix.join(format!("external-{index}"))).unwrap();
    }

    let normal = f.run(&["clone", "zomboid", "normal"]);
    assert_success(&normal);
    let normal = stdout(&normal);
    assert!(normal.contains("external links: 15 (showing first 12"));
    assert_eq!(normal.matches("  external:").count(), 12);

    let verbose = f.run(&["-v", "clone", "zomboid", "verbose"]);
    assert_success(&verbose);
    let verbose = stdout(&verbose);
    assert!(verbose.contains("external links: 15"));
    assert_eq!(verbose.matches("  external:").count(), 15);
}

#[test]
fn clone_copies_art_and_rm_removes_only_the_clones() {
    let f = Fixture::new();
    source(&f);
    let art = f.root.path().join("data/wineo/art");
    let original = art.join("zomboid");
    fs::create_dir_all(original.join("game")).unwrap();
    let record = "# mine\nschema_version = 1\n[icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n";
    fs::write(original.join("art.toml"), record).unwrap();
    fs::write(original.join("icon.png"), b"icon").unwrap();
    fs::write(original.join("game/grid.jpg"), b"grid").unwrap();

    let output = f.run(&["clone", "zomboid", "zomboid_modded"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("\n  art: copied\n"),
        "{}",
        stdout(&output)
    );
    let copy = art.join("zomboid_modded");
    assert_eq!(fs::read_to_string(copy.join("art.toml")).unwrap(), record);
    assert_eq!(fs::read(copy.join("icon.png")).unwrap(), b"icon");
    assert_eq!(fs::read(copy.join("game/grid.jpg")).unwrap(), b"grid");

    let removed = f.run(&["rm", "zomboid_modded", "--delete-prefix"]);
    assert_success(&removed);
    let id = common::trash_id(&removed);
    assert!(!copy.exists());
    assert_eq!(fs::read(original.join("icon.png")).unwrap(), b"icon");
    let mut names: Vec<_> = fs::read_dir(&art)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["zomboid"]);
    assert_success(&f.run(&["restore", &id]));
    assert_eq!(fs::read_to_string(copy.join("art.toml")).unwrap(), record);
    assert_eq!(fs::read(copy.join("icon.png")).unwrap(), b"icon");
    assert_eq!(fs::read(copy.join("game/grid.jpg")).unwrap(), b"grid");
    assert_eq!(fs::read(original.join("icon.png")).unwrap(), b"icon");
}

#[test]
fn clone_does_not_copy_play_stats() {
    let f = Fixture::new();
    source(&f);
    f.write_stats_log(
        "zomboid",
        "{\"target\":\"game\",\"started_at\":\"2026-01-01T00:00:00Z\",\"ended_at\":\"2026-01-01T01:00:00Z\",\"seconds\":3600,\"end\":\"exited\"}\n",
    );
    let output = f.run(&["clone", "zomboid", "zomboid_modded"]);
    assert_success(&output);
    assert!(!f.stats_log("zomboid_modded").exists());
    assert!(!f.read_stats_log("zomboid").is_empty());

    let stats = f.read_stats_log("zomboid");
    let removed = f.run(&["rm", "zomboid", "--delete-prefix"]);
    assert_success(&removed);
    let id = common::trash_id(&removed);
    assert!(!f.stats_log("zomboid").exists());
    assert_success(&f.run(&["restore", &id]));
    assert_eq!(f.read_stats_log("zomboid"), stats);
    assert!(!f.stats_log("zomboid_modded").exists());
}

#[test]
fn clone_refuses_leftover_art_under_the_new_name() {
    let f = Fixture::new();
    source(&f);
    let leftover = f.root.path().join("data/wineo/art/zomboid_modded");
    fs::create_dir_all(&leftover).unwrap();
    fs::write(leftover.join("icon.png"), b"old").unwrap();
    let output = f.run(&["clone", "zomboid", "zomboid_modded"]);
    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(error.contains("move it aside"), "{error}");
    assert!(
        !f.root
            .path()
            .join("data/wineo/prefixes/zomboid_modded")
            .exists()
    );
    assert!(
        !f.config_dir()
            .join("environments/zomboid_modded.toml")
            .exists()
    );
    assert_eq!(fs::read(leftover.join("icon.png")).unwrap(), b"old");

    // A clone with no art to copy writes and mentions none.
    fs::remove_dir_all(&leftover).unwrap();
    let output = f.run(&["clone", "zomboid", "zomboid_modded"]);
    assert_success(&output);
    assert!(!stdout(&output).contains("art:"), "{}", stdout(&output));
    assert!(!leftover.exists());
    let removed = f.run(&["rm", "zomboid_modded"]);
    assert_success(&removed);
    assert!(!stderr(&removed).contains("art"), "{}", stderr(&removed));
}
