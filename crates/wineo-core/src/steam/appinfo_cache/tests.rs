use std::fs;
use std::path::Path;

use super::*;
use crate::steam::appinfo_fixture::{build, game};

fn install(home: &Path) -> Install {
    let root = home.join(".local/share/Steam");
    fs::create_dir_all(root.join("appcache")).unwrap();
    fs::create_dir_all(root.join("steamapps")).unwrap();
    crate::steam::installs(home)
        .into_iter()
        .next()
        .expect("the fixture install is found")
}

fn write(install: &Install, bytes: &[u8]) {
    fs::write(appinfo::path(install), bytes).unwrap();
}

fn oslist(result: &Result<AppInfo, ReadError>, id: u32) -> Option<String> {
    result.as_ref().ok()?.get(id)?.oslist.clone()
}

#[test]
fn an_unchanged_file_is_parsed_once() {
    let home = tempfile::tempdir().unwrap();
    let install = install(home.path());
    write(&install, &build(&[game(1, Some("linux"))]));
    let cache = AppInfoCache::new();

    for _ in 0..3 {
        let result = cache.read(&install);
        assert_eq!(oslist(&result, 1).as_deref(), Some("linux"));
    }
    assert_eq!(cache.reads(), 1);
}

#[test]
fn a_changed_file_is_parsed_again() {
    let home = tempfile::tempdir().unwrap();
    let install = install(home.path());
    write(&install, &build(&[game(1, Some("linux"))]));
    let cache = AppInfoCache::new();
    assert_eq!(oslist(&cache.read(&install), 1).as_deref(), Some("linux"));

    // The same size, so only the stamp can tell.
    write(&install, &build(&[game(1, Some("macos"))]));
    assert_eq!(oslist(&cache.read(&install), 1).as_deref(), Some("macos"));
    assert_eq!(cache.reads(), 2);

    // Replaced by rename, as Steam does.
    let moved = home.path().join("new");
    fs::write(&moved, build(&[game(1, Some("windows")), game(2, None)])).unwrap();
    fs::rename(&moved, appinfo::path(&install)).unwrap();
    assert_eq!(oslist(&cache.read(&install), 1).as_deref(), Some("windows"));
    assert_eq!(cache.reads(), 3);
}

#[test]
fn an_unreadable_file_is_kept_too_until_it_changes() {
    let home = tempfile::tempdir().unwrap();
    let install = install(home.path());
    write(&install, b"not an appinfo file at all");
    let cache = AppInfoCache::new();

    for _ in 0..3 {
        assert!(cache.read(&install).is_err());
    }
    assert_eq!(cache.reads(), 1);

    // Fixed, the next read parses it.
    write(&install, &build(&[game(1, Some("linux"))]));
    assert_eq!(oslist(&cache.read(&install), 1).as_deref(), Some("linux"));
    assert_eq!(cache.reads(), 2);
}

#[test]
fn a_missing_file_is_an_error_that_nothing_keeps() {
    let home = tempfile::tempdir().unwrap();
    let install = install(home.path());
    let cache = AppInfoCache::new();

    assert!(cache.read(&install).is_err());
    assert!(cache.read(&install).is_err());
    assert_eq!(cache.reads(), 2);

    // It shows up as soon as it is there.
    write(&install, &build(&[game(1, Some("linux"))]));
    assert_eq!(oslist(&cache.read(&install), 1).as_deref(), Some("linux"));
}

#[test]
fn another_install_is_not_served_from_the_cache() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let (first, second) = (install(first.path()), install(second.path()));
    write(&first, &build(&[game(1, Some("linux"))]));
    write(&second, &build(&[game(1, Some("windows"))]));
    let cache = AppInfoCache::new();

    assert_eq!(oslist(&cache.read(&first), 1).as_deref(), Some("linux"));
    assert_eq!(oslist(&cache.read(&second), 1).as_deref(), Some("windows"));
}
