//! `wineo tag add|remove TAG NAME...`, one tag across many environments.

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::*;
use serde_json::json;

const HADES: &str =
    "# mine\nkind = 'native'\ntags = ['co-op'] # by hand\n\n[targets.game]\nexe = '/bin/true'\n";
const CELESTE: &str = "kind = 'native'\n\n[targets.game]\nexe = '/bin/true'\n";
const BG3: &str = "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1086940\"\n";

fn config(f: &Fixture, name: &str) -> PathBuf {
    f.config_dir().join(format!("environments/{name}.toml"))
}

fn write(f: &Fixture, name: &str, text: &str) {
    let path = config(f, name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read(f: &Fixture, name: &str) -> String {
    fs::read_to_string(config(f, name)).unwrap()
}

/// Inode and mtime, which a rewrite with the same bytes still changes.
fn stamp(path: &Path) -> (u64, i64, i64) {
    let found = fs::metadata(path).unwrap();
    (found.ino(), found.mtime(), found.mtime_nsec())
}

fn library() -> Fixture {
    let f = Fixture::new();
    write(&f, "hades", HADES);
    write(&f, "celeste", CELESTE);
    write(&f, "broken", "kind = 'native'\nnope = [\n");
    f
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", stdout(output)))
}

#[test]
fn add_and_remove_report_each_environment() {
    let f = library();
    let out = f.run(&["tag", "add", "co-op", "celeste"]);
    assert_success(&out);
    assert_eq!(stdout(&out), "Added co-op to celeste\n");
    assert_eq!(stderr(&out), "");
    assert_eq!(
        read(&f, "celeste"),
        "kind = 'native'\ntags = [\"co-op\"]\nschema_version = 1\n\n[targets.game]\nexe = '/bin/true'\n"
    );

    let out = f.run(&["tag", "remove", "co-op", "hades", "celeste"]);
    assert_success(&out);
    assert_eq!(
        stdout(&out),
        "Removed co-op from hades\nRemoved co-op from celeste\n"
    );
    // Comments and layout stay as written.
    assert_eq!(
        read(&f, "hades"),
        "# mine\nkind = 'native'\ntags = [] # by hand\nschema_version = 1\n\n[targets.game]\nexe = '/bin/true'\n"
    );
    fs::remove_file(config(&f, "broken")).unwrap();
    let out = f.run(&["list", "--tag", "co-op"]);
    assert_success(&out);
    assert_eq!(stdout(&out), "No environments are tagged co-op\n");
}

#[test]
fn already_in_place_is_unchanged_and_never_written() {
    let f = library();
    let hades = config(&f, "hades");
    let before = stamp(&hades);
    std::thread::sleep(std::time::Duration::from_millis(20));

    let out = f.run(&["tag", "add", "co-op", "hades"]);
    assert_success(&out);
    assert_eq!(stdout(&out), "");
    assert_eq!(stderr(&out), "hades already has co-op\n");

    let out = f.run(&["tag", "remove", "rpg", "hades"]);
    assert_success(&out);
    assert_eq!(stderr(&out), "hades doesn't have rpg\n");

    let out = f.run(&["tag", "add", "co-op", "hades", "--json"]);
    assert_success(&out);
    assert_eq!(
        json(&out),
        json!({"tag": "co-op", "changed": [], "unchanged": ["hades"], "errors": []})
    );
    assert_eq!(read(&f, "hades"), HADES);
    assert_eq!(stamp(&hades), before);
}

#[test]
fn each_environment_stands_alone() {
    let f = library();
    let out = f.run(&[
        "tag", "add", "co-op", "hades", "celeste", "broken", "missing",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Added co-op to celeste\n");
    let err = stderr(&out);
    assert!(
        err.starts_with("hades already has co-op\nerror: invalid config "),
        "{err}"
    );
    assert!(err.contains("broken.toml"), "{err}");
    assert!(
        err.ends_with("error: environment `missing` does not exist\n"),
        "{err}"
    );
    assert_eq!(read(&f, "broken"), "kind = 'native'\nnope = [\n");
    assert!(!config(&f, "missing").exists());
    assert!(read(&f, "celeste").contains("tags = [\"co-op\"]"));
}

#[test]
fn json_keeps_the_shape_for_every_exit_code() {
    let f = library();
    // Something failed: the rest still applied.
    let out = f.run(&[
        "tag", "add", "co-op", "hades", "celeste", "broken", "missing", "--json",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    let found = json(&out);
    let message = found["errors"][0]["message"].as_str().unwrap().to_owned();
    assert!(message.contains("broken.toml"), "{message}");
    assert_eq!(
        found,
        json!({
            "tag": "co-op",
            "changed": ["celeste"],
            "unchanged": ["hades"],
            "errors": [
                {"name": "broken", "message": message},
                {"name": "missing", "message": "environment `missing` does not exist"},
            ],
        })
    );

    // Everything changed.
    let out = f.run(&["tag", "remove", "co-op", "celeste", "hades", "--json"]);
    assert_success(&out);
    assert_eq!(
        json(&out),
        json!({"tag": "co-op", "changed": ["celeste", "hades"], "unchanged": [], "errors": []})
    );

    // Nothing to do.
    let out = f.run(&["tag", "remove", "co-op", "celeste", "hades", "--json"]);
    assert_success(&out);
    assert_eq!(
        json(&out),
        json!({"tag": "co-op", "changed": [], "unchanged": ["celeste", "hades"], "errors": []})
    );

    // A bad tag: nothing written and no JSON, like a command that can't start.
    let celeste = read(&f, "celeste");
    let out = f.run(&["tag", "add", "co op", "celeste", "missing", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert_eq!(
        stderr(&out),
        "error: Invalid tag \"co op\": spaces not allowed.\n"
    );
    assert_eq!(read(&f, "celeste"), celeste);

    // A missing argument is clap's.
    for args in [
        &["tag", "add", "co-op", "--json"][..],
        &["tag", "remove", "--json"],
    ] {
        let out = f.run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(stdout(&out), "");
        assert!(
            stderr(&out).contains("Usage: wineo tag"),
            "{}",
            stderr(&out)
        );
    }
}

#[test]
fn plain_tag_prints_help() {
    let f = Fixture::new();
    let out = f.run(&["tag"]);
    assert_eq!(out.status.code(), Some(2));
    let help = stderr(&out);
    assert!(help.contains("Usage: wineo tag"), "{help}");
    assert!(help.contains("add") && help.contains("remove"), "{help}");
}

#[test]
fn a_steam_game_takes_tags() {
    let f = Fixture::new();
    write(&f, "bg3", BG3);
    let out = f.run(&["tag", "add", "rpg", "bg3"]);
    assert_success(&out);
    assert_eq!(
        read(&f, "bg3"),
        format!("{BG3}tags = [\"rpg\"]\nschema_version = 1\n")
    );
    let out = f.run(&["tag", "remove", "rpg", "bg3", "--json"]);
    assert_success(&out);
    assert_eq!(json(&out)["changed"], json!(["bg3"]));
}

#[test]
fn portable_tags_the_portable_folder() {
    let f = library();
    let dir = f.root.path().join("portable");
    let portable = dir.join("config/environments/celeste.toml");
    fs::create_dir_all(portable.parent().unwrap()).unwrap();
    fs::write(&portable, CELESTE).unwrap();

    let dir_arg = dir.to_str().unwrap();
    let out = f.run(&["--portable", dir_arg, "tag", "add", "rpg", "celeste"]);
    assert_success(&out);
    assert!(
        fs::read_to_string(&portable)
            .unwrap()
            .contains("tags = [\"rpg\"]")
    );
    assert_eq!(read(&f, "celeste"), CELESTE);

    let out = f
        .wineo()
        .env("WINEO_PORTABLE_DIR", &dir)
        .args(["tag", "remove", "rpg", "celeste", "--json"])
        .output()
        .unwrap();
    assert_success(&out);
    assert_eq!(json(&out)["changed"], json!(["celeste"]));
    assert_eq!(read(&f, "celeste"), CELESTE);
}

#[test]
fn a_symlinked_config_stays_a_link_and_keeps_its_mode() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    let target = f.root.path().join("dotfiles/celeste.toml");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, CELESTE).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let link = config(&f, "celeste");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&target, &link).unwrap();

    assert_success(&f.run(&["tag", "add", "rpg", "celeste"]));
    assert_eq!(fs::read_link(&link).unwrap(), target);
    assert!(
        fs::read_to_string(&target)
            .unwrap()
            .contains("tags = [\"rpg\"]")
    );
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o600);

    // A dangling link is a missing environment, and nothing is created.
    fs::remove_file(&target).unwrap();
    let out = f.run(&["tag", "add", "rpg", "celeste", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        json(&out)["errors"],
        json!([{"name": "celeste", "message": "environment `celeste` does not exist"}])
    );
    assert!(!target.exists());
}

#[test]
fn tag_gets_man_pages() {
    let f = Fixture::new();
    let dir = f.root.path().join("man");
    assert_success(&f.run(&["manpages", dir.to_str().unwrap()]));
    for page in ["wineo-tag.1", "wineo-tag-add.1", "wineo-tag-remove.1"] {
        assert!(dir.join(page).is_file(), "missing {page}");
    }
    let add = fs::read_to_string(dir.join("wineo-tag-add.1")).unwrap();
    assert!(add.contains("\\-\\-json"), "{add}");
    let top = fs::read_to_string(dir.join("wineo.1")).unwrap();
    assert!(top.contains(r"wineo\-tag"), "{top}");
}

#[test]
fn rm_is_remove() {
    let f = library();
    let out = f.run(&["tag", "rm", "co-op", "hades", "--json"]);
    assert_success(&out);
    assert_eq!(
        json(&out),
        json!({"tag": "co-op", "changed": ["hades"], "unchanged": [], "errors": []})
    );
    let out = f.run(&["tag", "rm", "co-op", "hades"]);
    assert_success(&out);
    assert_eq!(stderr(&out), "hades doesn't have co-op\n");

    let help = stdout(&f.run(&["tag", "--help"]));
    assert!(help.contains("[alias: rm]"), "{help}");
    // Like the other `rm` aliases, it's documented on the `remove` page.
    let dir = f.root.path().join("man");
    assert_success(&f.run(&["manpages", dir.to_str().unwrap()]));
    assert!(dir.join("wineo-tag-remove.1").is_file());
    assert!(!dir.join("wineo-tag-rm.1").exists());
}
