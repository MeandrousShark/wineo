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

/// A native Steam at `home` with these fully installed games.
fn steam_games(root: &Path, ids: &[&str]) -> std::path::PathBuf {
    let steam = root.join(".local/share/Steam");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    for id in ids {
        fs::write(
            steam.join(format!("steamapps/appmanifest_{id}.acf")),
            format!(r#""AppState" {{ "appid" "{id}" "name" "Game {id}" "StateFlags" "4" }}"#),
        )
        .unwrap();
    }
    steam
}

fn imported(paths: &Paths, name: &str, id: &str) {
    fs::write(
        paths.environment_config(name),
        format!("kind = 'proxied'\nlauncher = 'steam'\napp_id = '{id}'\n"),
    )
    .unwrap();
}

fn discovery(paths: &Paths) -> steam::Discovery {
    let saved = crate::library::load(paths);
    RefreshSnapshot::read(paths).unwrap().discovery(&saved)
}

fn apply_discovery(paths: &Paths) -> bool {
    crate::library::apply_changes(paths, &discovery(paths).proposed).unwrap()
}

#[test]
fn a_baseline_then_a_new_install_is_pending() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let steam = steam_games(root.path(), &["1", "2"]);
    imported(&paths, "two", "2");

    let first = discovery(&paths);
    assert_eq!(first.pending, []);
    assert_eq!(first.proposed.baseline, ["1"]);
    assert_eq!(first.proposed.imported, ["2"]);
    assert_eq!(first.proposed.libraries, [steam.to_str().unwrap()]);
    assert!(apply_discovery(&paths));
    assert!(!apply_discovery(&paths));
    assert_eq!(discovery(&paths), steam::Discovery::default());

    steam_games(root.path(), &["3"]);
    let later = discovery(&paths);
    let pending: Vec<_> = later.pending.iter().map(|game| &*game.app_id).collect();
    assert_eq!(pending, ["3"]);
    assert_eq!(later.pending[0].title, "Game 3");
    assert!(later.proposed.is_empty());
}

#[test]
fn a_cancelled_review_leaves_the_game_pending() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_games(root.path(), &[]);
    apply_discovery(&paths);
    steam_games(root.path(), &["1"]);

    // A review only reads, so backing out of one changes nothing.
    let before = fs::read(paths.library_file()).unwrap();
    let plan = crate::steam_import::plan(
        &paths,
        &GlobalConfig::load(&paths).unwrap(),
        crate::steam_import::OwnedGames::Unloaded,
        "1",
        None,
    );
    assert!(plan.is_ok());

    assert_eq!(fs::read(paths.library_file()).unwrap(), before);
    assert_eq!(discovery(&paths).pending.len(), 1);
}

#[test]
fn a_dismissed_game_stays_away_even_after_a_reinstall() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let steam = steam_games(root.path(), &[]);
    apply_discovery(&paths);
    steam_games(root.path(), &["1"]);
    crate::library::dismiss_game(&paths, "1").unwrap();
    assert_eq!(discovery(&paths).pending, []);

    fs::remove_file(steam.join("steamapps/appmanifest_1.acf")).unwrap();
    assert_eq!(discovery(&paths), steam::Discovery::default());
    steam_games(root.path(), &["1"]);
    assert_eq!(discovery(&paths), steam::Discovery::default());
}

#[test]
fn no_steam_stays_quiet_and_a_later_steam_is_baselined() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    imported(&paths, "one", "1");

    assert_eq!(discovery(&paths), steam::Discovery::default());
    assert!(!apply_discovery(&paths));
    assert!(!paths.library_file().exists());

    steam_games(root.path(), &["2"]);
    assert_eq!(discovery(&paths).proposed.baseline, ["2"]);
    assert_eq!(discovery(&paths).proposed.imported, ["1"]);
}

#[test]
fn a_returning_library_baselines_and_its_old_games_never_nudge() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let steam = steam_games(root.path(), &["1"]);
    let drive = root.path().join("drive");
    fs::create_dir_all(&drive).unwrap();
    fs::write(
        steam.join("steamapps/libraryfolders.vdf"),
        format!(
            r#""libraryfolders" {{ "0" {{ "path" "{}" }} "1" {{ "path" "{}" }} }}"#,
            steam.display(),
            drive.display()
        ),
    )
    .unwrap();

    // The mount point is there, with nothing on it yet.
    apply_discovery(&paths);
    let saved = crate::library::load(&paths);
    assert_eq!(saved.steam.baselined.len(), 1);

    fs::create_dir_all(drive.join("steamapps")).unwrap();
    fs::write(
        drive.join("steamapps/appmanifest_9.acf"),
        r#""AppState" { "appid" "9" "name" "Old" "StateFlags" "4" }"#,
    )
    .unwrap();
    let back = discovery(&paths);
    assert_eq!(back.pending, []);
    assert_eq!(back.proposed.baseline, ["9"]);
    assert!(apply_discovery(&paths));
    assert_eq!(discovery(&paths), steam::Discovery::default());
}

#[test]
fn a_broken_manifest_holds_back_only_its_library() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let steam = steam_games(root.path(), &["1"]);
    fs::write(steam.join("steamapps/appmanifest_2.acf"), "not vdf {").unwrap();

    assert!(discovery(&paths).proposed.is_empty());

    steam_games(root.path(), &["2"]);
    let fixed = discovery(&paths);
    assert_eq!(fixed.proposed.baseline, ["1", "2"]);
    assert_eq!(fixed.pending, []);
}

#[test]
fn a_library_toml_that_cant_be_read_proposes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_games(root.path(), &["1"]);
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), "not toml [").unwrap();

    assert_eq!(discovery(&paths), steam::Discovery::default());
}

#[test]
fn a_portable_folder_reads_no_steam_and_finds_nothing() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    steam_games(&home, &["1"]);
    let lookup_home = home.clone();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(&lookup_home)),
        Some(&root.path().join("portable")),
    )
    .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();

    crate::read_log::clear();
    assert_eq!(discovery(&paths), steam::Discovery::default());
    assert_eq!(crate::read_log::count("libraryfolders.vdf"), 0);
    assert_eq!(crate::read_log::count("appmanifest_1.acf"), 0);
}

#[test]
fn an_overview_and_discovery_share_one_read_of_steam() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_games(root.path(), &["1", "2", "3"]);
    imported(&paths, "one", "1");
    imported(&paths, "two", "2");

    crate::read_log::clear();
    let saved = crate::library::load(&paths);
    let snapshot = RefreshSnapshot::read(&paths).unwrap();
    crate::report::overview_from_snapshot(&snapshot, Default::default());
    snapshot.discovery(&saved);

    assert_eq!(crate::read_log::count("libraryfolders.vdf"), 1);
    assert_eq!(crate::read_log::count("appmanifest_1.acf"), 1);
    assert_eq!(crate::read_log::count("appmanifest_3.acf"), 1);
}
