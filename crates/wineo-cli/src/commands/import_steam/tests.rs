use std::ffi::OsString;
use std::fs;

use wineo_core::art::Kind;
use wineo_core::steamgriddb::{self, Api};

use super::super::art::fake::{Fake, Swap};
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
    let identity = EnvironmentIdentity::read(&paths, "hades");
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
