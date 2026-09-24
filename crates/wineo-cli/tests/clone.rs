mod common;

use std::fs;

use common::{Fixture, assert_success, stderr};

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
