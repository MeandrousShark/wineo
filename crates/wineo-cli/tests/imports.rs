mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use common::{Fixture, assert_success, stderr, stdout};

fn source(root: &Path) -> std::path::PathBuf {
    let prefix = root.join("old prefix");
    fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    fs::write(prefix.join("drive_c/Games/save.dat"), "save data").unwrap();
    prefix
}

#[test]
fn generic_import_copies_without_launching_and_rejects_collisions() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let destination = f.root.path().join("copy");
    let args = [
        "import",
        "prefix",
        prefix.to_str().unwrap(),
        "-n",
        "copied",
        "--runner",
        "wine",
        "--dir",
        destination.to_str().unwrap(),
    ];
    assert_success(&f.run(&args));
    assert_eq!(
        fs::read(destination.join("drive_c/Games/save.dat")).unwrap(),
        b"save data"
    );
    assert!(!prefix.join("reg.log").exists());
    assert!(!destination.join("reg.log").exists());
    assert!(!f.run(&args).status.success());
    assert_success(&f.run(&["rm", "copied"]));
    assert!(destination.join("system.reg").exists());
    assert!(prefix.join("system.reg").exists());
}

#[test]
fn generic_adoption_preserves_the_source_and_proton_self_link_is_preserved() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    symlink(".", prefix.join("pfx")).unwrap();
    f.install_proton("GE-Proton11-7");
    assert_success(&f.run(&[
        "import",
        "prefix",
        prefix.to_str().unwrap(),
        "-n",
        "adopted",
        "--runner",
        "GE-Proton-Latest",
        "--adopt",
    ]));
    let config = stdout(&f.run(&["config", "adopted"]));
    assert!(config.contains("runner = \"GE-Proton-Latest\""));
    assert!(config.contains(prefix.to_str().unwrap()));
    assert!(!prefix.join("reg.log").exists());
    assert_success(&f.run(&["rm", "adopted"]));
    let destination = f.root.path().join("copied-proton");
    assert_success(&f.run(&[
        "import",
        "prefix",
        prefix.to_str().unwrap(),
        "-n",
        "proton",
        "--runner",
        "GE-Proton-Latest",
        "--dir",
        destination.to_str().unwrap(),
    ]));
    assert_eq!(
        fs::read_link(destination.join("pfx")).unwrap(),
        Path::new(".")
    );
    assert_success(&f.run(&["rm", "proton", "--delete-prefix"]));
    assert!(!destination.exists());
    assert!(prefix.exists());
}

#[test]
fn import_without_required_flags_needs_a_terminal_and_never_writes() {
    let f = Fixture::new();
    let prefix = source(f.root.path());
    let output = f.run(&["import", "prefix", prefix.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("terminal"));
    assert!(!f.config_dir().join("environments").exists());
    assert_success(&f.run(&["import", "--help"]));
}
