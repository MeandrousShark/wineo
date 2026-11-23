use std::ffi::OsString;
use std::fs;

use wineo_core::art::Kind;
use wineo_core::config::GlobalConfig;
use wineo_core::download::Transport;
use wineo_core::environment::Environment;
use wineo_core::steam::InstallState;
use wineo_core::steamgriddb::{self, Api, Key, KeySource};

use super::super::art::fake::{Fake, PNG, Swap};
use super::*;

const CHANGED: &str =
    "warning: art fetch failed: environment `hades` changed before artwork could be written\n";

fn imported_environment() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    let config = paths.environment_config("hades");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1086940\"\n",
    )
    .unwrap();
    (root, paths)
}

fn hades() -> Fake {
    let mut fake = Fake::default();
    fake.0.insert(
        format!("{}/games/steam/1086940", steamgriddb::API),
        (
            200,
            br#"{"success":true,"data":{"id":7,"name":"Hades"}}"#.to_vec(),
        ),
    );
    fake.icon_only();
    fake
}

/// Fetches the icon for `hades`, replacing the environment at the first
/// request containing `replace_at` when that is given.
fn fetch(replace_at: Option<&'static str>) -> (Paths, String, tempfile::TempDir) {
    let (root, paths) = imported_environment();
    let identity = EnvironmentIdentity::acquire(&paths, "hades").unwrap();
    let swap = Swap::new(hades(), &paths, "hades").at(replace_at.unwrap_or(""));
    swap.done.set(replace_at.is_none());
    let mut err = Vec::new();
    store_art(
        (&paths, "hades"),
        1086940,
        &[Kind::Icon],
        &Api::new(&swap, "k"),
        identity,
        &mut err,
    );
    (paths, String::from_utf8(err).unwrap(), root)
}

#[test]
fn an_unchanged_environment_gets_its_art() {
    let (paths, err, _root) = fetch(None);
    assert_eq!(err, "");
    assert!(paths.art_dir("hades").join("icon.png").exists());
    assert_eq!(
        wineo_core::art::saved_game(&paths, "hades").unwrap(),
        Some(7)
    );
}

#[test]
fn an_environment_replaced_before_the_game_is_saved_gets_nothing() {
    let (paths, err, _root) = fetch(Some(""));
    assert_eq!(err, CHANGED);
    assert!(!paths.art_dir("hades").join("icon.png").exists());
    assert_eq!(wineo_core::art::saved_game(&paths, "hades").unwrap(), None);
}

#[test]
fn an_environment_replaced_during_the_download_gets_no_image_and_one_warning() {
    let (paths, err, _root) = fetch(Some("/icons/"));
    assert_eq!(err, CHANGED);
    assert!(!paths.art_dir("hades").join("icon.png").exists());
}

/// A game with an icon and a grid, so two kinds have something to store.
fn two_kinds() -> Fake {
    let mut fake = hades();
    fake.page(Kind::Grid, &[2]);
    fake.0
        .insert("https://cdn.example/2".into(), (200, PNG.to_vec()));
    fake
}

#[test]
fn a_replacement_that_stops_two_kinds_is_warned_about_once() {
    let (_root, paths) = imported_environment();
    let identity = EnvironmentIdentity::acquire(&paths, "hades").unwrap();
    let swap = Swap::new(two_kinds(), &paths, "hades").at("/icons/");
    let mut err = Vec::new();
    store_art(
        (&paths, "hades"),
        1086940,
        &[Kind::Icon, Kind::Grid],
        &Api::new(&swap, "k"),
        identity,
        &mut err,
    );
    assert_eq!(String::from_utf8(err).unwrap(), CHANGED);
    assert!(!paths.art_dir("hades").join("icon.png").exists());
    assert!(!paths.art_dir("hades").join("grid.png").exists());
}

/// Runs `imported` for the game as `applied`, fetching through `fake`.
fn run_imported(replaced: bool) -> (Paths, tempfile::TempDir) {
    let (root, paths) = imported_environment();
    let identity = EnvironmentIdentity::acquire(&paths, "hades").unwrap();
    let applied = Applied {
        environment: Environment::load(&paths, "hades").unwrap(),
        identity,
        art: Vec::new(),
        warnings: Vec::new(),
    };
    let plan = Plan {
        app_id: "1086940".to_owned(),
        title: "Hades".to_owned(),
        name: "hades".to_owned(),
        size_on_disk: 0,
        state: InstallState::Installed,
    };
    let key = Key {
        value: "k".to_owned(),
        source: KeySource::Variable,
    };
    let swap = Swap::new(hades(), &paths, "hades");
    swap.done.set(!replaced);
    let connect = || -> anyhow::Result<Box<dyn Transport>> { Ok(Box::new(swap.clone())) };
    imported(
        &paths,
        &GlobalConfig::default(),
        &plan,
        &applied,
        Some(&key),
        &connect,
        &Token::new(),
    )
    .unwrap();
    (paths, root)
}

#[test]
fn an_import_fetches_into_the_environment_it_registered_and_no_other() {
    let (paths, _root) = run_imported(false);
    assert!(paths.art_dir("hades").join("icon.png").exists());
    // Replaced between registering and the download, nothing is stored.
    let (paths, _root) = run_imported(true);
    assert!(!paths.art_dir("hades").join("icon.png").exists());
}
