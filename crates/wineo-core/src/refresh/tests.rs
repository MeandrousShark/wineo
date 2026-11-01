use std::ffi::OsString;
use std::fs;
use std::path::Path;

use super::*;

fn test_paths(root: &Path) -> Paths {
    let home = root.to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(&home)));
    let paths = paths.unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    paths
}

fn write_marker(paths: &Paths, name: &str) {
    let marker = paths.restore_marker(name);
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(marker, "").unwrap();
}

#[test]
fn a_refused_load_keeps_its_error_and_lends_stats_the_config() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    fs::write(paths.environment_config("blocked"), "kind = 'native'\n").unwrap();
    write_marker(&paths, "blocked");

    crate::read_log::clear();
    let snapshot = RefreshSnapshot::read(&paths).unwrap();
    assert_eq!(crate::read_log::count("blocked.toml"), 0);

    let error = snapshot.environment("blocked").unwrap().unwrap_err();
    assert!(error.to_string().contains("snapshot recover"), "{error}");
    assert!(snapshot.lenient_config("blocked").is_some());
    assert!(snapshot.lenient_config("blocked").is_some());
    assert_eq!(crate::read_log::count("blocked.toml"), 1);
}

#[test]
fn a_bad_config_has_no_lenient_view_and_is_not_read_for_one() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    fs::write(paths.environment_config("broken"), "not toml [").unwrap();
    fs::write(paths.environment_config("good"), "kind = 'native'\n").unwrap();

    crate::read_log::clear();
    let snapshot = RefreshSnapshot::read(&paths).unwrap();

    assert!(snapshot.environment("broken").unwrap().is_err());
    assert!(snapshot.lenient_config("broken").is_none());
    assert!(snapshot.environment("good").unwrap().is_ok());
    assert!(snapshot.lenient_config("good").is_some());
    assert!(snapshot.environment("missing").is_none());
    assert!(snapshot.lenient_config("missing").is_none());
    assert_eq!(crate::read_log::count("broken.toml"), 1);
    assert_eq!(crate::read_log::count("good.toml"), 1);
}

#[test]
fn a_bad_global_config_is_read_once_and_kept_for_both_meanings() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
    fs::write(paths.global_config(), "not toml [").unwrap();
    fs::create_dir_all(paths.home().join(".local/share/Steam")).unwrap();

    crate::read_log::clear();
    let snapshot = RefreshSnapshot::read(&paths).unwrap();
    assert_eq!(crate::read_log::count("config.toml"), 0);

    let steam = snapshot.steam();
    assert!(steam.install().is_none());
    assert!(matches!(
        steam.playtime("10"),
        steam::Playtime::Unknown(reason) if reason.starts_with("global config can't be read: ")
    ));
    assert_eq!(crate::read_log::count("config.toml"), 1);
}
