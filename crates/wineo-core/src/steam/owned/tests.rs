use std::ffi::OsString;
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::api::fixture::{Server, refused, reply};
use crate::files::write_unique_with;

use super::*;

const KEY: &str = "SECRETKEY0123456789";
const ID: u64 = 76_561_197_960_265_729;
const T: i64 = 1_800_000_000;

fn setup() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        None,
    )
    .unwrap();
    (root, paths)
}

fn library(games: &[(u64, &str)]) -> Vec<u8> {
    let list: Vec<Value> = games
        .iter()
        .map(|(appid, name)| serde_json::json!({ "appid": appid, "name": name }))
        .collect();
    let body = serde_json::json!({
        "response": { "game_count": games.len(), "games": list }
    });
    reply(200, &body.to_string())
}

fn game(app_id: &str, name: &str) -> OwnedGame {
    OwnedGame {
        app_id: app_id.to_owned(),
        name: name.to_owned(),
    }
}

fn run(server: &Server, paths: &Paths, refresh: bool, now: i64) -> Result<Loaded, LoadError> {
    let api = SteamApi::with_base(&server.base).unwrap();
    load_at(paths, ID, Some(KEY), &api, refresh, now)
}

fn games(loaded: Result<Loaded, LoadError>) -> (Vec<OwnedGame>, Option<String>) {
    match loaded.unwrap() {
        Loaded::Owned(Owned { games, warning }) => (games, warning),
        Loaded::NoKey => panic!("no key"),
    }
}

#[test]
fn fetch_sends_the_documented_parameters_and_parses_games() {
    let server = Server::start(vec![library(&[
        (1086940, "Baldur's Gate 3"),
        (10, "Counter-Strike"),
    ])]);
    let api = SteamApi::with_base(&server.base).unwrap();
    let found = fetch(&api, KEY, ID).unwrap();
    assert_eq!(
        found,
        [
            game("1086940", "Baldur's Gate 3"),
            game("10", "Counter-Strike")
        ]
    );
    let request = &server.requests()[0];
    for part in [
        "/IPlayerService/GetOwnedGames/v1/?",
        &format!("key={KEY}"),
        &format!("steamid={ID}"),
        "include_appinfo=1",
        "include_played_free_games=1",
        "include_free_sub=1",
    ] {
        assert!(request.contains(part), "{part} missing from {request}");
    }
}

#[test]
fn rejected_keys_and_bad_answers_are_distinct_failures() {
    let cases = [
        (reply(401, "x"), OwnedError::Api(ApiError::Rejected)),
        (reply(403, "x"), OwnedError::Api(ApiError::Rejected)),
        (reply(200, r#"{"response":{}}"#), OwnedError::Hidden),
        (reply(200, "{"), OwnedError::Api(ApiError::Json)),
        (reply(200, r#"{"nope":1}"#), OwnedError::Malformed),
        (
            reply(200, r#"{"response":{"game_count":3}}"#),
            OwnedError::Malformed,
        ),
    ];
    for (answer, expected) in cases {
        let server = Server::start(vec![answer]);
        let api = SteamApi::with_base(&server.base).unwrap();
        assert_eq!(fetch(&api, KEY, ID).unwrap_err(), expected);
    }
}

#[test]
fn a_count_of_zero_is_an_empty_library() {
    let server = Server::start(vec![reply(200, r#"{"response":{"game_count":0}}"#)]);
    let api = SteamApi::with_base(&server.base).unwrap();
    assert_eq!(fetch(&api, KEY, ID).unwrap(), []);
}

#[test]
fn a_game_without_a_name_is_shown_by_its_id() {
    let server = Server::start(vec![reply(
        200,
        r#"{"response":{"game_count":1,"games":[{"appid":5}]}}"#,
    )]);
    let api = SteamApi::with_base(&server.base).unwrap();
    assert_eq!(fetch(&api, KEY, ID).unwrap(), [game("5", "App 5")]);
}

#[test]
fn the_key_check_passes_only_with_a_game_count() {
    for (answer, ok) in [
        (library(&[(1, "A")]), true),
        (reply(200, r#"{"response":{"game_count":0}}"#), true),
        (reply(200, r#"{"response":{}}"#), false),
        (reply(401, "x"), false),
    ] {
        let server = Server::start(vec![answer]);
        let api = SteamApi::with_base(&server.base).unwrap();
        assert_eq!(check_key_for(&api, ID, KEY).is_ok(), ok);
    }
}

#[test]
fn failures_never_show_the_key() {
    let cases = [
        Server::start(vec![reply(401, "x")]).base,
        Server::start(vec![reply(200, "{")]).base,
        refused(),
    ];
    for base in cases {
        let api = SteamApi::with_base(&base).unwrap();
        let error = fetch(&api, KEY, ID).unwrap_err();
        let text = format!("{error:#} {error:?} {:#}", LoadError(error.clone()));
        assert!(!text.contains(KEY), "{text}");
    }
}

#[test]
fn no_key_is_its_own_outcome_and_asks_nothing() {
    let (_root, paths) = setup();
    let server = Server::start(vec![]);
    let api = SteamApi::with_base(&server.base).unwrap();
    assert_eq!(
        load_at(&paths, ID, None, &api, false, T).unwrap(),
        Loaded::NoKey
    );
    assert!(server.requests().is_empty());
    assert!(!paths.steam_owned_cache(ID).exists());
}

#[test]
fn a_fresh_cache_is_used_without_asking() {
    let (_root, paths) = setup();
    let first = Server::start(vec![library(&[(1, "Alpha")])]);
    assert_eq!(games(run(&first, &paths, false, T)).0, [game("1", "Alpha")]);

    let quiet = Server::start(vec![]);
    let (found, warning) = games(run(&quiet, &paths, false, T + 3599));
    assert_eq!(found, [game("1", "Alpha")]);
    assert_eq!(warning, None);
    assert!(quiet.requests().is_empty());

    let cache = fs::read_to_string(paths.steam_owned_cache(ID)).unwrap();
    assert!(cache.contains("\"fetched_at\":1800000000"));
    assert!(!cache.contains(KEY));
}

#[test]
fn a_stale_cache_and_a_forced_refresh_both_fetch() {
    let (_root, paths) = setup();
    let first = Server::start(vec![library(&[(1, "Alpha")])]);
    games(run(&first, &paths, false, T));

    let stale = Server::start(vec![library(&[(2, "Beta")])]);
    assert_eq!(
        games(run(&stale, &paths, false, T + 3600)).0,
        [game("2", "Beta")]
    );
    assert_eq!(stale.requests().len(), 1);

    let forced = Server::start(vec![library(&[(3, "Gamma")])]);
    assert_eq!(
        games(run(&forced, &paths, true, T + 3601)).0,
        [game("3", "Gamma")]
    );
    assert_eq!(forced.requests().len(), 1);

    // The forced fetch restarted the hour.
    let quiet = Server::start(vec![]);
    assert_eq!(
        games(run(&quiet, &paths, false, T + 3601 + 60)).0,
        [game("3", "Gamma")]
    );
}

#[test]
fn a_failed_refresh_falls_back_to_the_old_list_with_a_warning() {
    let (_root, paths) = setup();
    let first = Server::start(vec![library(&[(1, "Alpha")])]);
    games(run(&first, &paths, false, T));

    let down = Server::start(vec![reply(500, "x")]);
    let (found, warning) = games(run(&down, &paths, false, T + 7200));
    assert_eq!(found, [game("1", "Alpha")]);
    assert_eq!(
        warning.as_deref(),
        Some("Couldn't refresh owned games: Steam answered with status 500")
    );
}

#[test]
fn a_failure_without_a_cache_is_an_error() {
    let (_root, paths) = setup();
    let rejected = Server::start(vec![reply(403, "x")]);
    let error = run(&rejected, &paths, false, T).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Couldn't get owned games: Steam rejected the Web API key"
    );
    assert!(!paths.steam_owned_cache(ID).exists());
}

#[test]
fn an_empty_response_keeps_the_cache_but_a_zero_count_replaces_it() {
    let (_root, paths) = setup();
    let first = Server::start(vec![library(&[(1, "Alpha")])]);
    games(run(&first, &paths, false, T));
    let before = fs::read_to_string(paths.steam_owned_cache(ID)).unwrap();

    let hidden = Server::start(vec![reply(200, r#"{"response":{}}"#)]);
    let (found, warning) = games(run(&hidden, &paths, true, T + 10));
    assert_eq!(found, [game("1", "Alpha")]);
    assert_eq!(
        warning.as_deref(),
        Some("Couldn't refresh owned games: the key can't see this library")
    );
    assert_eq!(
        fs::read_to_string(paths.steam_owned_cache(ID)).unwrap(),
        before
    );

    let empty = Server::start(vec![reply(200, r#"{"response":{"game_count":0}}"#)]);
    let (found, warning) = games(run(&empty, &paths, true, T + 20));
    assert_eq!((found, warning), (vec![], None));
    let quiet = Server::start(vec![]);
    assert_eq!(games(run(&quiet, &paths, false, T + 30)).0, []);
}

#[test]
fn an_empty_response_without_a_cache_fails() {
    let (_root, paths) = setup();
    let hidden = Server::start(vec![reply(200, r#"{"response":{}}"#)]);
    let error = run(&hidden, &paths, false, T).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Couldn't get owned games: the key can't see this library"
    );
    assert!(!paths.steam_owned_cache(ID).exists());
}

#[test]
fn tools_are_filtered_from_fresh_and_cached_lists() {
    let (_root, paths) = setup();
    let all = [
        (1086940, "Baldur's Gate 3"),
        (1493710, "Proton Experimental"),
        (228980, "Steamworks Common Redistributables"),
        (1628350, "Steam Linux Runtime 3.0 (sniper)"),
    ];
    let first = Server::start(vec![library(&all)]);
    assert_eq!(
        games(run(&first, &paths, false, T)).0,
        [game("1086940", "Baldur's Gate 3")]
    );
    let quiet = Server::start(vec![]);
    assert_eq!(
        games(run(&quiet, &paths, false, T + 1)).0,
        [game("1086940", "Baldur's Gate 3")]
    );
}

#[test]
fn caches_are_per_account_and_an_unreadable_one_counts_as_none() {
    let (_root, paths) = setup();
    assert_ne!(paths.steam_owned_cache(ID), paths.steam_owned_cache(ID + 1));
    assert!(
        paths
            .steam_owned_cache(ID)
            .ends_with(format!("wineo/steam-owned-{ID}.json"))
    );
    fs::create_dir_all(paths.steam_owned_cache(ID).parent().unwrap()).unwrap();
    fs::write(paths.steam_owned_cache(ID), "not json").unwrap();
    let server = Server::start(vec![library(&[(1, "Alpha")])]);
    assert_eq!(
        games(run(&server, &paths, false, T)).0,
        [game("1", "Alpha")]
    );
}

#[test]
fn loading_without_saving_fetches_but_never_touches_the_cache() {
    let (_root, paths) = setup();
    let path = paths.steam_owned_cache(ID);
    let server = Server::start(vec![library(&[(1, "Alpha")]), library(&[(2, "Beta")])]);
    let api = SteamApi::with_base(&server.base).unwrap();

    let fetched = load_inner(&paths, ID, Some(KEY), &api, false, T, false);
    assert_eq!(games(fetched), (vec![game("1", "Alpha")], None));
    assert!(!path.exists());

    write_raw_cache(&paths, T - 7200, &[("9", "Old")]);
    let before = fs::read(&path).unwrap();
    let stale = load_inner(&paths, ID, Some(KEY), &api, false, T, false);
    assert_eq!(games(stale), (vec![game("2", "Beta")], None));
    assert_eq!(fs::read(&path).unwrap(), before);
}

fn write_raw_cache(paths: &Paths, fetched_at: i64, games: &[(&str, &str)]) {
    let cache = Cache {
        fetched_at,
        games: games.iter().map(|(id, name)| game(id, name)).collect(),
    };
    write_cache(&paths.steam_owned_cache(ID), &cache).unwrap();
}

#[test]
fn racing_writers_never_leave_unparseable_json() {
    let (_root, paths) = setup();
    let path = paths.steam_owned_cache(ID);
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    write_raw_cache(&paths, T, &[("1", "Seed")]);
    let reader = {
        let (path, done) = (path.clone(), done.clone());
        std::thread::spawn(move || {
            let mut seen = 0_u32;
            while !done.load(Ordering::Relaxed) {
                if let Ok(text) = fs::read_to_string(&path) {
                    serde_json::from_str::<Cache>(&text).expect("a torn cache was visible");
                    seen += 1;
                }
            }
            seen
        })
    };
    let writers: Vec<_> = (0..4)
        .map(|writer| {
            let path = path.clone();
            std::thread::spawn(move || {
                let games = (0..2000)
                    .map(|id| game(&id.to_string(), &format!("Game {writer} {id}")))
                    .collect();
                let cache = Cache {
                    fetched_at: writer,
                    games,
                };
                for _ in 0..25 {
                    write_cache(&path, &cache).unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    done.store(true, Ordering::Relaxed);
    assert!(reader.join().unwrap() > 0, "the reader saw no snapshot");
    assert!(read_cache(&path).is_some());
    // No temp files are left behind.
    let left = fs::read_dir(path.parent().unwrap()).unwrap().count();
    assert_eq!(left, 1);
}

#[test]
fn extreme_future_and_invalid_timestamps_are_stale() {
    for fetched_at in [i64::MIN, i64::MAX, T + 1, T + 3600] {
        let (_root, paths) = setup();
        write_raw_cache(&paths, fetched_at, &[("1", "Old")]);
        let server = Server::start(vec![library(&[(2, "New")])]);
        let (found, _) = games(run(&server, &paths, false, T));
        assert_eq!(found, [game("2", "New")], "fetched_at {fetched_at}");
        assert_eq!(server.requests().len(), 1);
    }
    // An overflowing age is stale too, whatever the clock says.
    let (_root, paths) = setup();
    write_raw_cache(&paths, i64::MIN, &[("1", "Old")]);
    let server = Server::start(vec![library(&[(2, "New")])]);
    assert_eq!(
        games(run(&server, &paths, false, i64::MAX)).0,
        [game("2", "New")]
    );
}

#[test]
fn a_cache_that_cannot_be_written_is_a_warning() {
    let (root, paths) = setup();
    // A file where the cache dir should be.
    fs::create_dir_all(root.path().join(".cache")).unwrap();
    fs::write(root.path().join(".cache/wineo"), "in the way").unwrap();
    let server = Server::start(vec![library(&[(1, "Alpha")])]);
    let (found, warning) = games(run(&server, &paths, false, T));
    assert_eq!(found, [game("1", "Alpha")]);
    let warning = warning.unwrap();
    assert!(
        warning.starts_with("Couldn't save owned games: "),
        "{warning}"
    );
    assert!(!warning.contains(KEY), "{warning}");
}

#[test]
fn no_key_wins_over_an_existing_cache() {
    let (_root, paths) = setup();
    write_raw_cache(&paths, T, &[("1", "Alpha")]);
    let server = Server::start(vec![]);
    let api = SteamApi::with_base(&server.base).unwrap();
    assert_eq!(
        load_at(&paths, ID, None, &api, false, T + 1).unwrap(),
        Loaded::NoKey
    );
    assert!(server.requests().is_empty());
}

#[test]
fn tools_are_filtered_from_a_stale_fallback_too() {
    let (_root, paths) = setup();
    write_raw_cache(
        &paths,
        T,
        &[
            ("1", "Alpha"),
            ("1493710", "Proton Experimental"),
            ("228980", "Redist"),
        ],
    );
    let server = Server::start(vec![reply(500, "x")]);
    let (found, warning) = games(run(&server, &paths, false, T + 7200));
    assert_eq!(found, [game("1", "Alpha")]);
    assert!(
        warning
            .unwrap()
            .starts_with("Couldn't refresh owned games: ")
    );
}

#[test]
fn a_taken_temp_name_is_left_alone_and_the_next_is_used() {
    let (_root, paths) = setup();
    let path = paths.steam_owned_cache(ID);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let taken = path.with_file_name(format!(
        "{}.{}-0.tmp",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    fs::write(&taken, "someone else's").unwrap();

    let counter = AtomicU64::new(0);
    write_unique_with(&path, b"{\"fetched_at\":1,\"games\":[]}", &counter).unwrap();
    assert_eq!(fs::read_to_string(&taken).unwrap(), "someone else's");
    assert!(read_cache(&path).is_some());
    assert_eq!(counter.load(Ordering::Relaxed), 2);
}

#[test]
fn every_temp_name_taken_is_a_write_error() {
    let (_root, paths) = setup();
    let path = paths.steam_owned_cache(ID);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for n in 0..8 {
        fs::write(
            path.with_file_name(format!(
                "{}.{}-{n}.tmp",
                path.file_name().unwrap().to_string_lossy(),
                std::process::id()
            )),
            "x",
        )
        .unwrap();
    }
    let error = write_unique_with(&path, b"{}", &AtomicU64::new(0)).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(!path.exists());
}

#[test]
fn a_failure_after_creating_the_temp_file_removes_only_that_file() {
    let (_root, paths) = setup();
    let path = paths.steam_owned_cache(ID);
    // A non-empty directory can't be renamed over, so the rename fails.
    fs::create_dir_all(path.join("kept")).unwrap();
    assert!(write_unique_with(&path, b"{}", &AtomicU64::new(0)).is_err());
    assert!(path.join("kept").is_dir());
    let names: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
}
