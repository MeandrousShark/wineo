use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;
use crate::library::Recorded;
use crate::steam::appinfo;
use crate::steam::appinfo_fixture::{self, App};
use crate::steam::{Install, InstallKind, LibraryScan, ManifestEntry};

fn manifest(app_id: &str, name: &str, flags: u32) -> Manifest {
    Manifest {
        app_id: app_id.to_owned(),
        name: name.to_owned(),
        install_dir: name.to_owned(),
        state_flags: flags,
        size_on_disk: 1,
        last_played: 0,
        bytes_to_download: 0,
        bytes_downloaded: 0,
        bytes_to_stage: 0,
        bytes_staged: 0,
    }
}

fn entry(library: &str, app_id: &str, name: &str, flags: u32) -> ManifestEntry {
    ManifestEntry {
        app_id: app_id.to_owned(),
        library: PathBuf::from(library),
        manifest: Ok(manifest(app_id, name, flags)),
    }
}

fn library(path: &str, state: LibraryState) -> LibraryScan {
    LibraryScan {
        path: PathBuf::from(path),
        state,
    }
}

fn scan(libraries: Vec<LibraryScan>, manifests: Vec<ManifestEntry>) -> Scan {
    Scan {
        libraries,
        manifests,
        problem: None,
    }
}

fn record(games: &[(&str, Recorded)], baselined: &[&str]) -> SteamRecord {
    SteamRecord {
        games: games
            .iter()
            .map(|(id, reason)| ((*id).to_owned(), *reason))
            .collect(),
        baselined: baselined.iter().map(|path| (*path).to_owned()).collect(),
    }
}

fn no_appinfo<'a>() -> Option<&'a AppInfo> {
    None
}

fn pending_ids(discovery: &Discovery) -> Vec<&str> {
    discovery
        .pending
        .iter()
        .map(|game| game.app_id.as_str())
        .collect()
}

/// `AppInfo` from synthetic entries, through the real reader.
fn appinfo_of(root: &Path, apps: &[App<'_>]) -> AppInfo {
    let file = root.join("appcache/appinfo.vdf");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, appinfo_fixture::build(apps)).unwrap();
    let install = Install {
        kind: InstallKind::Native,
        root: root.to_owned(),
    };
    appinfo::read(&install).unwrap()
}

fn typed(id: u32, kind: &str) -> App<'_> {
    App {
        id,
        oslist: None,
        kind: Some(kind),
    }
}

const FULL: u32 = 4;

#[test]
fn a_first_complete_scan_proposes_its_games_and_nudges_nothing() {
    let scan = scan(
        vec![library("/a", LibraryState::Complete)],
        vec![
            entry("/a", "1", "Game", FULL),
            entry("/a", "2", "Proton 9.0", FULL),
            entry("/a", "3", "Updating", FULL | 1024),
            entry("/a", "4", "Mid update", 2 | 1024),
            entry("/a", "5", "Gone", 0),
            entry("/a", "6", "Leaving", FULL | 2048),
        ],
    );

    let discovery = discover(&scan, &HashSet::new(), &record(&[], &[]), || {
        panic!("a baseline never asks what type a game is")
    });

    assert_eq!(discovery.pending, []);
    assert_eq!(discovery.proposed.baseline, ["1", "2", "3", "4"]);
    assert_eq!(discovery.proposed.libraries, ["/a"]);
    assert_eq!(discovery.proposed.imported, Vec::<String>::new());
}

#[test]
fn imported_games_join_the_record_as_imported_not_baseline() {
    let scan = scan(
        vec![library("/a", LibraryState::Complete)],
        vec![
            entry("/a", "1", "Game", FULL),
            entry("/a", "2", "Mine", FULL),
        ],
    );
    let imported = HashSet::from(["2", "3", "not-a-number"]);

    let discovery = discover(&scan, &imported, &record(&[], &[]), no_appinfo);

    assert_eq!(discovery.proposed.baseline, ["1"]);
    assert_eq!(discovery.proposed.imported, ["2", "3"]);
}

#[test]
fn an_imported_game_already_in_the_record_proposes_nothing() {
    let scan = scan(vec![library("/a", LibraryState::Complete)], Vec::new());
    let imported = HashSet::from(["2"]);
    let record = record(&[("2", Recorded::Claimed)], &["/a"]);

    let discovery = discover(&scan, &imported, &record, no_appinfo);

    assert_eq!(discovery, Discovery::default());
}

#[test]
fn a_baselined_library_nudges_new_games_only() {
    let scan = scan(
        vec![library("/a", LibraryState::Complete)],
        vec![
            entry("/a", "1", "Old", FULL),
            entry("/a", "2", "Dismissed", FULL),
            entry("/a", "3", "Imported", FULL),
            entry("/a", "4", "Has an environment", FULL),
            entry("/a", "5", "New", FULL),
            entry("/a", "6", "Downloading", 1024),
            entry("/a", "7", "Queued", 2),
        ],
    );
    let record = record(
        &[
            ("1", Recorded::Baseline),
            ("2", Recorded::Dismissed),
            ("3", Recorded::Imported),
        ],
        &["/a"],
    );
    let imported = HashSet::from(["4"]);

    let discovery = discover(&scan, &imported, &record, no_appinfo);

    assert_eq!(
        discovery.pending,
        [PendingGame {
            app_id: "5".to_owned(),
            title: "New".to_owned(),
        }]
    );
    assert_eq!(discovery.proposed.baseline, Vec::<String>::new());
    assert_eq!(discovery.proposed.libraries, Vec::<String>::new());
    assert_eq!(discovery.proposed.imported, ["4"]);
}

#[test]
fn a_claimed_game_is_not_pending() {
    let scan = scan(
        vec![library("/a", LibraryState::Complete)],
        vec![entry("/a", "1", "Game", FULL)],
    );
    let record = record(&[("1", Recorded::Claimed)], &["/a"]);
    assert_eq!(
        discover(&scan, &HashSet::new(), &record, no_appinfo),
        Discovery::default()
    );
}

#[test]
fn a_library_that_is_not_complete_neither_nudges_nor_baselines() {
    for state in [
        LibraryState::Unavailable,
        LibraryState::NoSteamapps,
        LibraryState::ListingFailed,
        LibraryState::BadManifest,
    ] {
        let scan = scan(
            vec![
                library("/whole", LibraryState::Complete),
                library("/seen", state),
                library("/new", state),
            ],
            vec![
                entry("/seen", "1", "Seen", FULL),
                entry("/new", "2", "New", FULL),
            ],
        );
        let record = record(&[], &["/whole", "/seen"]);

        let discovery = discover(&scan, &HashSet::new(), &record, no_appinfo);

        assert_eq!(discovery, Discovery::default(), "{state:?}");
    }
}

#[test]
fn no_readable_library_proposes_nothing_at_all() {
    for state in [LibraryState::Unavailable, LibraryState::NoSteamapps] {
        let scan = scan(vec![library("/a", state)], Vec::new());
        let imported = HashSet::from(["1"]);
        assert_eq!(
            discover(&scan, &imported, &record(&[], &[]), no_appinfo),
            Discovery::default()
        );
    }
}

#[test]
fn a_library_is_baselined_once_it_comes_back_whole() {
    let installed = vec![entry("/a", "1", "Game", FULL)];
    let broken = scan(
        vec![library("/a", LibraryState::BadManifest)],
        installed.clone(),
    );
    assert_eq!(
        discover(&broken, &HashSet::new(), &record(&[], &[]), no_appinfo),
        Discovery::default()
    );

    let whole = scan(vec![library("/a", LibraryState::Complete)], installed);
    let discovery = discover(&whole, &HashSet::new(), &record(&[], &[]), no_appinfo);
    assert_eq!(discovery.pending, []);
    assert_eq!(discovery.proposed.baseline, ["1"]);
    assert_eq!(discovery.proposed.libraries, ["/a"]);
}

#[test]
fn a_game_on_two_libraries_counts_once() {
    let scan = scan(
        vec![
            library("/a", LibraryState::Complete),
            library("/b", LibraryState::Complete),
        ],
        vec![
            entry("/a", "1", "Game", FULL),
            entry("/b", "1", "Game", FULL),
            entry("/a", "2", "Other", FULL),
            entry("/b", "2", "Other", FULL),
        ],
    );
    let first = discover(&scan, &HashSet::new(), &record(&[], &[]), no_appinfo);
    assert_eq!(first.proposed.baseline, ["1", "2"]);

    let second = discover(
        &scan,
        &HashSet::new(),
        &record(&[], &["/a", "/b"]),
        no_appinfo,
    );
    assert_eq!(pending_ids(&second), ["1", "2"]);
}

#[test]
fn only_games_and_applications_nudge() {
    let root = tempfile::tempdir().unwrap();
    let info = appinfo_of(
        root.path(),
        &[
            typed(1, "Game"),
            typed(2, "application"),
            typed(3, "demo"),
            typed(4, "Music"),
            typed(5, "tool"),
            typed(6, "DLC"),
            typed(7, "config"),
            typed(8, "video"),
        ],
    );
    let manifests = (1..=8)
        .map(|id| entry("/a", &id.to_string(), "Thing", FULL))
        .collect();
    let scan = scan(vec![library("/a", LibraryState::Complete)], manifests);

    let discovery = discover(&scan, &HashSet::new(), &record(&[], &["/a"]), || {
        Some(&info)
    });

    assert_eq!(pending_ids(&discovery), ["1", "2"]);
}

#[test]
fn appinfo_saying_nothing_falls_back_to_the_tool_check() {
    let root = tempfile::tempdir().unwrap();
    // 1 has no type, 2 and 3 aren't in the file.
    let info = appinfo_of(
        root.path(),
        &[App {
            id: 1,
            oslist: None,
            kind: None,
        }],
    );
    let scan = scan(
        vec![library("/a", LibraryState::Complete)],
        vec![
            entry("/a", "1", "Game", FULL),
            entry("/a", "2", "Other", FULL),
            entry("/a", "3", "Proton 9.0", FULL),
            entry("/a", "228980", "Redistributables", FULL),
        ],
    );
    let record = record(&[], &["/a"]);

    let found = discover(&scan, &HashSet::new(), &record, || Some(&info));
    assert_eq!(pending_ids(&found), ["1", "2"]);

    // A file that can't be read at all answers the same way.
    let unreadable = discover(&scan, &HashSet::new(), &record, no_appinfo);
    assert_eq!(pending_ids(&unreadable), ["1", "2"]);
}

#[test]
fn appinfo_is_only_read_once_a_game_needs_its_type() {
    let scan = scan(
        vec![library("/a", LibraryState::Complete)],
        vec![entry("/a", "1", "Old", FULL)],
    );
    let record = record(&[("1", Recorded::Baseline)], &["/a"]);

    let discovery = discover(&scan, &HashSet::new(), &record, || -> Option<&AppInfo> {
        panic!("nothing needed it")
    });

    assert_eq!(discovery, Discovery::default());
}

#[test]
fn a_real_scan_reports_each_library_and_every_way_it_can_fall_short() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let steam = home.join(".local/share/Steam");
    let write = |path: PathBuf, text: &str| {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    let good = |library: &Path, id: &str| {
        write(
            library.join(format!("steamapps/appmanifest_{id}.acf")),
            &format!(r#""AppState" {{ "appid" "{id}" "name" "G" "StateFlags" "4" }}"#),
        );
    };
    good(&steam, "1");
    let broken = home.join("broken");
    good(&broken, "2");
    write(broken.join("steamapps/appmanifest_3.acf"), "not vdf {");
    let empty_mount = home.join("mount");
    fs::create_dir_all(&empty_mount).unwrap();
    let gone = home.join("gone");
    let locked = home.join("locked");
    good(&locked, "4");
    let listed = [&steam, &broken, &empty_mount, &gone, &locked]
        .iter()
        .enumerate()
        .map(|(index, path)| format!("\"{index}\" {{ \"path\" \"{}\" }}", path.display()))
        .collect::<Vec<_>>()
        .join(" ");
    write(
        steam.join("steamapps/libraryfolders.vdf"),
        &format!("\"libraryfolders\" {{ {listed} }}"),
    );
    let steamapps = locked.join("steamapps");
    fs::set_permissions(&steamapps, fs::Permissions::from_mode(0o000)).unwrap();

    let install = Install {
        kind: InstallKind::Native,
        root: steam.clone(),
    };
    let found = crate::steam::scan(&install);
    let manifests = crate::steam::manifests(&install);
    fs::set_permissions(&steamapps, fs::Permissions::from_mode(0o755)).unwrap();

    let states: Vec<_> = found
        .libraries
        .iter()
        .map(|library| library.state)
        .collect();
    assert_eq!(
        states,
        [
            LibraryState::Complete,
            LibraryState::BadManifest,
            LibraryState::NoSteamapps,
            LibraryState::Unavailable,
            LibraryState::Unavailable,
        ]
    );
    assert!(found.readable());
    assert_eq!(found.manifests, manifests);
}
