use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

use super::*;
use crate::settings::update;

fn setup() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    (root, paths)
}

const NOTEPAD: &str = "# hand written\nschema_version = 1\nprefix = \"~/prefixes/notepad\"\nrunner = \"wine\"\narch = \"win64\"\n";

fn write_env(paths: &Paths, name: &str, text: &str) {
    let path = paths.environment_config(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read_env(paths: &Paths, name: &str) -> String {
    fs::read_to_string(paths.environment_config(name)).unwrap()
}

fn tags(paths: &Paths, name: &str) -> Vec<String> {
    EnvironmentConfig::parse(Path::new("/x"), &read_env(paths, name))
        .unwrap()
        .tags
}

/// Inode and mtime, which a rewrite with the same bytes still changes.
fn stamp(paths: &Paths, name: &str) -> (u64, i64, i64) {
    let found = fs::metadata(paths.environment_config(name)).unwrap();
    (found.ino(), found.mtime(), found.mtime_nsec())
}

#[test]
fn adds_a_tag_to_a_config_without_any() {
    let (_root, paths) = setup();
    write_env(&paths, "notepad", NOTEPAD);
    let tagged = change_tags(&paths, "notepad", TagChange::Add("co-op")).unwrap();
    assert_eq!(tagged, Tagged::Changed);
    assert_eq!(
        read_env(&paths, "notepad"),
        format!("{NOTEPAD}tags = [\"co-op\"]\n")
    );
}

#[test]
fn adds_and_removes_keeping_comments_and_quotes() {
    let (_root, paths) = setup();
    let text = "# mine\nschema_version = 1\nkind = 'native'\ntags = ['rpg', 'solo'] # sorted by hand\n\n[targets.game]\nexe = '/bin/true' # the game\n";
    write_env(&paths, "game", text);
    assert_eq!(
        change_tags(&paths, "game", TagChange::Add("co-op")).unwrap(),
        Tagged::Changed
    );
    assert_eq!(
        read_env(&paths, "game"),
        text.replace("['rpg', 'solo']", "['rpg', 'solo', 'co-op']")
    );
    change_tags(&paths, "game", TagChange::Remove("rpg")).unwrap();
    assert_eq!(
        read_env(&paths, "game"),
        text.replace("['rpg', 'solo']", "['solo', 'co-op']")
    );
    change_tags(&paths, "game", TagChange::Remove("co-op")).unwrap();
    change_tags(&paths, "game", TagChange::Remove("solo")).unwrap();
    assert_eq!(
        read_env(&paths, "game"),
        text.replace("['rpg', 'solo']", "[]")
    );
    change_tags(&paths, "game", TagChange::Add("rpg")).unwrap();
    assert_eq!(
        read_env(&paths, "game"),
        text.replace("['rpg', 'solo']", "[\"rpg\"]")
    );
}

#[test]
fn a_multi_line_list_keeps_one_tag_a_line() {
    let (_root, paths) = setup();
    for (before, add, remove, last) in [
        (
            "tags = [\n  \"rpg\",\n  # the main one\n  \"solo\",\n]\n",
            "tags = [\n  \"rpg\",\n  # the main one\n  \"solo\",\n  \"co-op\",\n]\n",
            "tags = [\n  # the main one\n  \"solo\",\n  \"co-op\",\n]\n",
            "tags = [\n  # the main one\n  \"solo\",\n]\n",
        ),
        (
            "tags = [\n  \"rpg\",\n  \"solo\"\n]\n",
            "tags = [\n  \"rpg\",\n  \"solo\",\n  \"co-op\"\n]\n",
            "tags = [\n  \"solo\",\n  \"co-op\"\n]\n",
            "tags = [\n  \"solo\"\n]\n",
        ),
    ] {
        let text = format!("schema_version = 1\nkind = 'native'\n{before}");
        write_env(&paths, "game", &text);
        change_tags(&paths, "game", TagChange::Add("co-op")).unwrap();
        assert_eq!(read_env(&paths, "game"), text.replace(before, add));
        change_tags(&paths, "game", TagChange::Remove("rpg")).unwrap();
        assert_eq!(read_env(&paths, "game"), text.replace(before, remove));
        // The last tag, which held the line break before `]`.
        change_tags(&paths, "game", TagChange::Remove("co-op")).unwrap();
        assert_eq!(read_env(&paths, "game"), text.replace(before, last));
    }
}

/// Applies `change` to a native config holding `before` and checks the result
/// is exactly `after`.
fn edits_to(paths: &Paths, before: &str, change: TagChange, after: &str) {
    let text = format!("schema_version = 1\nkind = 'native'\n{before}");
    write_env(paths, "game", &text);
    assert_eq!(change_tags(paths, "game", change).unwrap(), Tagged::Changed);
    assert_eq!(
        read_env(paths, "game"),
        format!("schema_version = 1\nkind = 'native'\n{after}"),
        "{change:?} on {before:?}"
    );
}

#[test]
fn removing_keeps_every_comment_but_the_removed_tags_own() {
    let (_root, paths) = setup();
    let three =
        "tags = [\n  \"rpg\", # first tag\n  \"mid\", # middle tag\n  \"solo\" # last tag\n]\n";
    edits_to(
        &paths,
        three,
        TagChange::Remove("solo"),
        "tags = [\n  \"rpg\", # first tag\n  \"mid\" # middle tag\n]\n",
    );
    edits_to(
        &paths,
        three,
        TagChange::Remove("rpg"),
        "tags = [\n  \"mid\", # middle tag\n  \"solo\" # last tag\n]\n",
    );
    edits_to(
        &paths,
        three,
        TagChange::Remove("mid"),
        "tags = [\n  \"rpg\", # first tag\n  \"solo\" # last tag\n]\n",
    );
    // With a trailing comma, and a note closing the array.
    let closed = "tags = [\n  \"rpg\", # first tag\n  \"solo\", # last tag\n  # end note\n]\n";
    edits_to(
        &paths,
        closed,
        TagChange::Remove("solo"),
        "tags = [\n  \"rpg\", # first tag\n  # end note\n]\n",
    );
    edits_to(
        &paths,
        closed,
        TagChange::Remove("rpg"),
        "tags = [\n  \"solo\", # last tag\n  # end note\n]\n",
    );
    // A single-line list.
    edits_to(
        &paths,
        "tags = [\"rpg\", \"solo\"] # mine\n",
        TagChange::Remove("solo"),
        "tags = [\"rpg\"] # mine\n",
    );
}

#[test]
fn removing_the_only_tag_keeps_the_comments_above_it() {
    let (_root, paths) = setup();
    edits_to(
        &paths,
        "tags = [\n  # the one tag\n  \"rpg\", # mine\n]\n",
        TagChange::Remove("rpg"),
        "tags = [\n  # the one tag\n]\n",
    );
    edits_to(
        &paths,
        "tags = [\n  # the one tag\n  \"rpg\" # mine\n]\n",
        TagChange::Remove("rpg"),
        "tags = [\n  # the one tag\n]\n",
    );
}

#[test]
fn adding_keeps_the_last_tags_comment_beside_it() {
    let (_root, paths) = setup();
    edits_to(
        &paths,
        "tags = [\n  \"rpg\", # first tag\n  \"solo\" # last tag\n]\n",
        TagChange::Add("co-op"),
        "tags = [\n  \"rpg\", # first tag\n  \"solo\", # last tag\n  \"co-op\"\n]\n",
    );
    edits_to(
        &paths,
        "tags = [\n  # the one tag\n  \"rpg\", # mine\n  # end note\n]\n",
        TagChange::Add("co-op"),
        "tags = [\n  # the one tag\n  \"rpg\", # mine\n  \"co-op\",\n  # end note\n]\n",
    );
    edits_to(
        &paths,
        "tags = [\"rpg\" # mine\n]\n",
        TagChange::Add("co-op"),
        "tags = [\"rpg\", # mine\n\"co-op\"\n]\n",
    );
}

#[test]
fn an_unchanged_list_leaves_the_file_byte_identical() {
    let (_root, paths) = setup();
    let text = "# legacy, no version\nkind = 'native'\ntags = [ 'rpg' ]\n";
    write_env(&paths, "game", text);
    let before = stamp(&paths, "game");
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(
        change_tags(&paths, "game", TagChange::Add("rpg")).unwrap(),
        Tagged::Unchanged
    );
    assert_eq!(
        change_tags(&paths, "game", TagChange::Remove("co-op")).unwrap(),
        Tagged::Unchanged
    );
    assert_eq!(read_env(&paths, "game"), text);
    assert_eq!(stamp(&paths, "game"), before);
}

#[test]
fn a_change_to_a_legacy_file_stamps_its_version() {
    let (_root, paths) = setup();
    write_env(&paths, "game", "kind = 'native'\n");
    change_tags(&paths, "game", TagChange::Add("rpg")).unwrap();
    assert_eq!(
        read_env(&paths, "game"),
        "kind = 'native'\ntags = [\"rpg\"]\nschema_version = 1\n"
    );
}

#[test]
fn a_bad_tag_is_refused_before_anything_is_read() {
    let (_root, paths) = setup();
    write_env(&paths, "notepad", NOTEPAD);
    for tag in ["co op", "", "a/b"] {
        let err = check_tag(tag).unwrap_err();
        assert!(err.to_string().starts_with("Invalid tag"), "{err}");
        for change in [TagChange::Add(tag), TagChange::Remove(tag)] {
            let err = change_tags(&paths, "notepad", change).unwrap_err();
            assert!(err.to_string().starts_with("Invalid tag"), "{err}");
        }
    }
    // Refused even for an environment that isn't there.
    let err = change_tags(&paths, "missing", TagChange::Add("co op")).unwrap_err();
    assert!(err.to_string().starts_with("Invalid tag"), "{err}");
    assert_eq!(read_env(&paths, "notepad"), NOTEPAD);
}

#[test]
fn a_missing_environment_is_an_error() {
    let (_root, paths) = setup();
    let err = change_tags(&paths, "missing", TagChange::Add("rpg")).unwrap_err();
    assert!(matches!(err, Error::UnknownEnvironment(_)), "{err}");
    assert_eq!(err.to_string(), "environment `missing` does not exist");
    assert!(!paths.environment_config("missing").exists());
    let err = change_tags(&paths, "../x", TagChange::Add("rpg")).unwrap_err();
    assert!(err.to_string().contains("invalid name"), "{err}");
}

#[test]
fn an_invalid_config_is_an_error_whatever_the_change() {
    let (_root, paths) = setup();
    for text in [
        "not valid toml = [",
        "kind = 'native'\ntags = ['rpg']\nnope = 1\n",
        "kind = 'native'\ntags = ['rpg', 'rpg']\n",
        "kind = 'native'\ntags = 'rpg'\n",
        "schema_version = 99\nkind = 'native'\ntags = ['rpg']\n",
    ] {
        write_env(&paths, "broken", text);
        for change in [
            TagChange::Add("rpg"),
            TagChange::Add("co-op"),
            TagChange::Remove("rpg"),
        ] {
            let err = change_tags(&paths, "broken", change).unwrap_err();
            assert!(
                err.to_string().contains("broken.toml"),
                "{text:?} {change:?}: {err}"
            );
            assert_eq!(read_env(&paths, "broken"), text);
        }
    }
}

#[test]
fn an_unreadable_config_is_an_error() {
    let (_root, paths) = setup();
    // Bytes that aren't UTF-8 fail the read itself, even as root.
    let path = paths.environment_config("broken");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"kind = 'native'\ntags = ['\xff']\n").unwrap();
    let err = change_tags(&paths, "broken", TagChange::Add("rpg")).unwrap_err();
    assert!(err.to_string().contains("broken.toml"), "{err}");
    assert_eq!(
        fs::read(&path).unwrap(),
        b"kind = 'native'\ntags = ['\xff']\n"
    );
}

#[test]
fn a_steam_game_takes_tags() {
    let (_root, paths) = setup();
    let text = "schema_version = 1\nkind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1086940\"\ntitle = \"Baldur's Gate 3\"\n";
    write_env(&paths, "bg3", text);
    change_tags(&paths, "bg3", TagChange::Add("rpg")).unwrap();
    assert_eq!(read_env(&paths, "bg3"), format!("{text}tags = [\"rpg\"]\n"));
    change_tags(&paths, "bg3", TagChange::Remove("rpg")).unwrap();
    assert_eq!(read_env(&paths, "bg3"), format!("{text}tags = []\n"));
}

#[test]
fn a_change_is_read_under_the_lock_so_a_concurrent_write_is_kept() {
    let (_root, paths) = setup();
    write_env(&paths, "notepad", NOTEPAD);
    let config = paths.environment_config("notepad");
    let held = Locks::new(&paths).config(&config).unwrap();
    let change = {
        let paths = paths.clone();
        std::thread::spawn(move || change_tags(&paths, "notepad", TagChange::Add("b")))
    };
    std::thread::sleep(Duration::from_millis(50));
    // What another writer holding the lock gets in first.
    fs::write(&config, format!("{NOTEPAD}tags = [\"a\"]\n")).unwrap();
    drop(held);
    assert_eq!(change.join().unwrap().unwrap(), Tagged::Changed);
    assert_eq!(tags(&paths, "notepad"), ["a", "b"]);
}

#[test]
fn two_writers_at_once_both_land() {
    let (_root, paths) = setup();
    write_env(&paths, "notepad", NOTEPAD);
    let config = paths.environment_config("notepad");
    let held = Locks::new(&paths).config(&config).unwrap();
    let mut writers = Vec::new();
    for tag in ["a", "b", "c", "d"] {
        let paths = paths.clone();
        writers.push(std::thread::spawn(move || {
            change_tags(&paths, "notepad", TagChange::Add(tag)).map(|_| ())
        }));
    }
    let set = {
        let paths = paths.clone();
        std::thread::spawn(move || {
            update(
                &paths,
                ConfigFile::Environment("notepad"),
                &[("gamemode", "true")],
                &[],
            )
            .map(|_| ())
        })
    };
    std::thread::sleep(Duration::from_millis(50));
    drop(held);
    for writer in writers {
        writer.join().unwrap().unwrap();
    }
    set.join().unwrap().unwrap();
    let mut found = tags(&paths, "notepad");
    found.sort();
    assert_eq!(found, ["a", "b", "c", "d"]);
    assert!(read_env(&paths, "notepad").contains("gamemode = true"));
}

#[test]
fn tags_write_through_a_symlinked_config() {
    use crate::files::link_check::{Dangling, NewFile, Writer};
    let (_root, paths) = setup();
    Writer {
        link: &paths.environment_config("notepad"),
        seed: NOTEPAD,
        write: &mut || change_tags(&paths, "notepad", TagChange::Add("rpg")).map(|_| ()),
        no_op: Some((NOTEPAD, &mut || {
            change_tags(&paths, "notepad", TagChange::Remove("rpg")).map(|tagged| {
                assert_eq!(tagged, Tagged::Unchanged);
            })
        })),
        dangling: Dangling::Missing("notepad"),
        new_file: NewFile::NotCreated,
    }
    .check();
}

#[test]
fn the_writer_gets_the_locked_target() {
    let (root, paths) = setup();
    let target = root.path().join("elsewhere/notepad.toml");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, NOTEPAD).unwrap();
    let link = paths.environment_config("notepad");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut seen = None;
    change_tags_with_target(
        &paths,
        "notepad",
        TagChange::Add("rpg"),
        |path, locked, _| {
            seen = Some((path.to_owned(), locked.to_owned()));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(seen, Some((link, target)));
}
