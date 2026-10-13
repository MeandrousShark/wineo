mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Fixture, assert_success, stderr};

/// `outer` with a registered environment `inner` whose prefix sits inside it.
fn nested(f: &Fixture) -> (PathBuf, PathBuf) {
    f.environment("outer", "");
    let outer = f.root.path().join("prefixes/outer");
    let inner = outer.join("drive_c/nested-prefix");
    fs::create_dir_all(inner.join("drive_c")).unwrap();
    fs::write(
        inner.join("system.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n",
    )
    .unwrap();
    fs::write(inner.join("drive_c/save.dat"), "inner save").unwrap();
    register(f, "inner", &inner);
    (outer, inner)
}

fn register(f: &Fixture, name: &str, prefix: &Path) {
    fs::write(
        f.config_dir().join(format!("environments/{name}.toml")),
        format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n",
            prefix.display()
        ),
    )
    .unwrap();
}

fn assert_refused(output: &std::process::Output, message: &str) {
    assert!(!output.status.success(), "{}", stderr(output));
    assert!(stderr(output).contains(message), "{}", stderr(output));
}

const INSIDE_OUTER: &str = "prefix is inside outer's prefix. Move one of them first.";

#[test]
fn rm_refuses_to_take_a_nested_prefix_along() {
    let f = Fixture::new();
    let (outer, inner) = nested(&f);
    assert_refused(
        &f.run(&["rm", "outer", "--delete-prefix"]),
        &format!("inner's {INSIDE_OUTER}"),
    );
    assert!(outer.join("system.reg").exists());
    assert_eq!(
        fs::read_to_string(inner.join("drive_c/save.dat")).unwrap(),
        "inner save"
    );
    assert!(f.config_dir().join("environments/outer.toml").exists());
}

#[test]
fn new_refuses_a_prefix_inside_another() {
    let f = Fixture::new();
    f.environment("outer", "");
    let dir = f.root.path().join("prefixes/outer/drive_c/other");
    assert_refused(
        &f.run(&["new", "-n", "other", "--dir", dir.to_str().unwrap()]),
        &format!("other's {INSIDE_OUTER}"),
    );
    assert!(!dir.exists());
    assert!(!f.config_dir().join("environments/other.toml").exists());
}

#[test]
fn new_refuses_a_prefix_around_another() {
    let f = Fixture::new();
    f.environment("inner", "");
    let dir = f.root.path().join("prefixes");
    assert_refused(
        &f.run(&["new", "-n", "outer", "--dir", dir.to_str().unwrap()]),
        &format!("inner's {INSIDE_OUTER}"),
    );
    assert!(!f.config_dir().join("environments/outer.toml").exists());
}

#[test]
fn clone_refuses_a_destination_inside_another_prefix() {
    let f = Fixture::new();
    f.environment("outer", "");
    f.environment("source", "");
    let dir = f.root.path().join("prefixes/outer/drive_c/copy");
    assert_refused(
        &f.run(&["clone", "source", "copy", "--dir", dir.to_str().unwrap()]),
        &format!("copy's {INSIDE_OUTER}"),
    );
    assert!(!dir.exists());
    assert!(!f.config_dir().join("environments/copy.toml").exists());
}

#[test]
fn import_refuses_a_destination_inside_another_prefix() {
    let f = Fixture::new();
    f.environment("outer", "");
    f.environment("source", "");
    let source = f.root.path().join("prefixes/source");
    let dir = f.root.path().join("prefixes/outer/drive_c/copy");
    assert_refused(
        &f.run(&[
            "import",
            "prefix",
            source.to_str().unwrap(),
            "-n",
            "copy",
            "--runner",
            "wine",
            "--dir",
            dir.to_str().unwrap(),
        ]),
        &format!("copy's {INSIDE_OUTER}"),
    );
    assert!(!dir.exists());
    assert!(!f.config_dir().join("environments/copy.toml").exists());
}

#[test]
fn import_adopt_refuses_a_prefix_inside_another() {
    let f = Fixture::new();
    let (_outer, inner) = nested(&f);
    fs::remove_file(f.config_dir().join("environments/inner.toml")).unwrap();
    assert_refused(
        &f.run(&[
            "import",
            "prefix",
            inner.to_str().unwrap(),
            "-n",
            "adopted",
            "--runner",
            "wine",
            "--adopt",
        ]),
        &format!("adopted's {INSIDE_OUTER}"),
    );
    assert!(!f.config_dir().join("environments/adopted.toml").exists());
}

#[test]
fn set_refuses_to_point_a_prefix_inside_another() {
    let f = Fixture::new();
    f.environment("outer", "");
    f.environment("mover", "");
    let before = fs::read_to_string(f.config_dir().join("environments/mover.toml")).unwrap();
    let dir = f.root.path().join("prefixes/outer/drive_c/moved");
    assert_refused(
        &f.run(&["set", "mover", &format!("prefix={}", dir.display())]),
        &format!("mover's {INSIDE_OUTER}"),
    );
    assert_eq!(
        fs::read_to_string(f.config_dir().join("environments/mover.toml")).unwrap(),
        before
    );
}

#[test]
fn equal_prefixes_stay_shareable() {
    let f = Fixture::new();
    f.environment("first", "");
    f.environment("second", "");
    let shared = f.root.path().join("prefixes/first");
    assert_success(&f.run(&["set", "second", &format!("prefix={}", shared.display())]));
}

/// `other` names a prefix that is missing, like one on an unplugged drive.
fn missing_prefix(f: &Fixture) -> PathBuf {
    let gone = f.root.path().join("gone");
    fs::create_dir_all(f.config_dir().join("environments")).unwrap();
    register(f, "other", &gone);
    gone
}

#[test]
fn clone_refuses_a_destination_another_environment_names() {
    let f = Fixture::new();
    f.environment("source", "");
    let gone = missing_prefix(&f);
    assert_refused(
        &f.run(&["clone", "source", "copy", "--dir", gone.to_str().unwrap()]),
        &format!("{} is already other's prefix.", gone.display()),
    );
    assert!(!gone.exists());
    assert!(!f.config_dir().join("environments/copy.toml").exists());
}

#[test]
fn import_refuses_a_destination_another_environment_names() {
    let f = Fixture::new();
    f.environment("source", "");
    let gone = missing_prefix(&f);
    let source = f.root.path().join("prefixes/source");
    assert_refused(
        &f.run(&[
            "import",
            "prefix",
            source.to_str().unwrap(),
            "-n",
            "copy",
            "--runner",
            "wine",
            "--dir",
            gone.to_str().unwrap(),
        ]),
        &format!("{} is already other's prefix.", gone.display()),
    );
    assert!(!gone.exists());
    assert!(!f.config_dir().join("environments/copy.toml").exists());
}

#[test]
fn import_adopt_still_shares_a_registered_prefix() {
    let f = Fixture::new();
    f.environment("source", "");
    let source = f.root.path().join("prefixes/source");
    assert_success(&f.run(&[
        "import",
        "prefix",
        source.to_str().unwrap(),
        "-n",
        "adopted",
        "--runner",
        "wine",
        "--adopt",
    ]));
}

#[test]
fn trash_restore_refuses_a_prefix_that_now_nests() {
    let f = Fixture::new();
    f.environment("outer", "");
    f.environment("mover", "");
    let removed = f.run(&["rm", "outer"]);
    assert_success(&removed);
    let id = common::trash_id(&removed);
    let dir = f.root.path().join("prefixes/outer/drive_c/moved");
    assert_success(&f.run(&["set", "mover", &format!("prefix={}", dir.display())]));
    assert_refused(
        &f.run(&["restore", &id]),
        &format!("mover's {INSIDE_OUTER}"),
    );
    assert!(!f.config_dir().join("environments/outer.toml").exists());
}

#[test]
fn trash_restore_refuses_a_prefix_another_environment_now_names() {
    let f = Fixture::new();
    f.environment("outer", "");
    let removed = f.run(&["rm", "outer", "--delete-prefix"]);
    assert_success(&removed);
    let id = common::trash_id(&removed);
    let prefix = f.root.path().join("prefixes/outer");
    register(&f, "other", &prefix);
    assert_refused(
        &f.run(&["restore", &id]),
        &format!("{} is already other's prefix.", prefix.display()),
    );
    assert!(!prefix.exists());
    assert!(!f.config_dir().join("environments/outer.toml").exists());
}
