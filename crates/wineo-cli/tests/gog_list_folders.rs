//! `wineo import gog --list` decides installed or imported from the GOG
//! folder states. The lookup fails closed: a state that can't be trusted
//! reads as imported rather than installed.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use common::gog_api::{CODE, GogApi, installer, owned_game, product, success_address};
use common::gog_world::RIMWORLD;
use common::pty::Pty;
use common::{Fixture, stderr, stdout};
use wineo_core::gog::folder::{FolderLock, Source};

const HADES: u64 = 777;

fn world() -> (Fixture, GogApi) {
    let f = Fixture::new();
    let api = GogApi::with_library(
        vec![
            owned_game(RIMWORLD, "RimWorld", true, false),
            owned_game(HADES, "Hades", true, true),
        ],
        vec![
            product(RIMWORLD, "RimWorld", &[installer("linux", "en", "1")], &[]),
            product(HADES, "Hades", &[installer("linux", "en", "1")], &[]),
        ],
    );
    let mut command = f.gog_command(&api);
    command.args(["import", "gog", "--login"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Address: ");
    pty.send(&success_address(CODE));
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");

    for (name, id) in [("alpha", RIMWORLD), ("beta", HADES)] {
        let folder = f.native_game(name, id, "1");
        installed_by_wineo(&f, &folder);
    }
    (f, api)
}

/// Marks the folder's game as installed by wineo rather than adopted.
fn installed_by_wineo(f: &Fixture, folder: &Path) {
    let mut state = f.gog_state(folder).unwrap();
    state.committed.as_mut().unwrap().source = Some(Source::Install);
    FolderLock::exclusive(&f.paths(), &state.folder, &|| false)
        .unwrap()
        .write(&state)
        .unwrap();
}

struct Listing {
    states: BTreeMap<u64, String>,
    table: String,
    stderr: String,
    code: Option<i32>,
}

fn list(f: &Fixture, api: &GogApi) -> Listing {
    let json = f.gog(api, &["import", "gog", "--list", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
    let states = value["games"]
        .as_array()
        .unwrap()
        .iter()
        .map(|game| {
            (
                game["id"].as_str().unwrap().parse().unwrap(),
                game["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let table = f.gog(api, &["import", "gog", "--list"]);
    Listing {
        states,
        table: stdout(&table),
        stderr: stderr(&json),
        code: json.status.code(),
    }
}

fn state_of(listing: &Listing, id: u64) -> &str {
    &listing.states[&id]
}

#[test]
fn installed_games_list_as_installed() {
    let (f, api) = world();
    let listing = list(&f, &api);
    assert_eq!(state_of(&listing, RIMWORLD), "installed");
    assert_eq!(state_of(&listing, HADES), "installed");
    for title in ["RimWorld", "Hades"] {
        let row = listing
            .table
            .lines()
            .find(|line| line.contains(title))
            .unwrap();
        assert!(!row.contains("imported"), "{}", listing.table);
    }
}

#[test]
fn one_unreadable_state_makes_every_game_list_as_imported() {
    let (f, api) = world();
    let dir = f.paths().gog_folders_dir();
    fs::write(dir.join("zzz.toml"), "this is not a folder state").unwrap();

    let listing = list(&f, &api);
    assert_eq!(state_of(&listing, RIMWORLD), "imported");
    assert_eq!(state_of(&listing, HADES), "imported");
    for title in ["RimWorld", "Hades"] {
        let row = listing
            .table
            .lines()
            .find(|line| line.contains(title))
            .unwrap();
        assert!(row.contains("imported"), "{}", listing.table);
    }
    // The unreadable state is reported by the update check too.
    assert_eq!(listing.code, Some(1), "{}", listing.stderr);
}

#[test]
fn a_game_with_two_folders_lists_as_imported_and_others_stay_installed() {
    let (f, api) = world();
    let second = f.root.path().join("Games/alpha-copy");
    fs::create_dir_all(&second).unwrap();
    f.gog_folder(&second, Some("alpha"), RIMWORLD, false);

    let listing = list(&f, &api);
    assert_eq!(state_of(&listing, RIMWORLD), "imported");
    assert_eq!(state_of(&listing, HADES), "installed");
    let row = |title: &str| {
        listing
            .table
            .lines()
            .find(|line| line.contains(title))
            .unwrap()
            .to_owned()
    };
    assert!(row("RimWorld").contains("imported"), "{}", listing.table);
    assert!(!row("Hades").contains("imported"), "{}", listing.table);
}

#[test]
fn folders_of_other_games_or_owners_do_not_unsettle_a_listing() {
    let (f, api) = world();
    // The same environment owning a folder of another game, and a folder of
    // the same game that no environment owns, aren't a second folder of the
    // game.
    let other_game = f.root.path().join("Games/alpha-other");
    fs::create_dir_all(&other_game).unwrap();
    f.gog_folder(&other_game, Some("alpha"), 12345, false);
    let unowned = f.root.path().join("Games/hades-unowned");
    fs::create_dir_all(&unowned).unwrap();
    f.gog_folder(&unowned, None, HADES, false);

    let listing = list(&f, &api);
    assert_eq!(state_of(&listing, RIMWORLD), "installed");
    assert_eq!(state_of(&listing, HADES), "installed");
}
