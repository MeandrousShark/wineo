use std::ffi::OsString;
use std::fs;

use wineo_core::steamgriddb::{KeySource, KeyringError, NoKeyring};

use super::fake::{Fake, PNG};
use super::*;
use crate::prompt::Script;

#[test]
fn a_spot_splits_at_its_first_dot() {
    assert_eq!(split("hades"), ("hades", None));
    assert_eq!(split("hades.server"), ("hades", Some("server")));
    assert_eq!(split("hades.a.b"), ("hades", Some("a.b")));
    assert_eq!(split("hades."), ("hades", Some("")));
}

fn paths() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    let config = paths.environment_config("hades");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        "prefix = \"~/p\"\nrunner = \"wine\"\narch = \"win64\"\n",
    )
    .unwrap();
    (root, paths)
}

fn fetch_request() -> FetchRequest<'static> {
    FetchRequest {
        spot: "hades",
        search: None,
        game: None,
        only: Some(Kind::Icon),
        kinds: ArtFetch::All,
        mode: Mode::Fill,
    }
}

#[test]
fn steam_fetch_uses_the_app_id_even_with_a_saved_game() {
    let (_root, paths) = paths();
    fs::write(
        paths.environment_config("hades"),
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1145360\"\n",
    )
    .unwrap();
    let mut fake = Fake::default();
    fake.0.insert(
        format!("{}/games/steam/1145360", steamgriddb::API),
        (
            200,
            br#"{"success":true,"data":{"id":7,"name":"Hades"}}"#.to_vec(),
        ),
    );
    fake.icon_only();
    let request = fetch_request();
    let api = Api::new(&fake, "k");
    for saved in [None, Some(99)] {
        if let Some(saved) = saved {
            art::save_game(&paths, "hades", saved).unwrap();
        }
        assert_eq!(
            fetch_game(&paths, "hades", &request, &api).unwrap(),
            Some(7)
        );
    }
    let game = fetch_game(&paths, "hades", &request, &api)
        .unwrap()
        .unwrap();
    let spot = Spot {
        paths: &paths,
        env: "hades",
        target: None,
        identity: EnvironmentIdentity::acquire_existing(&paths, "hades").unwrap(),
    };
    let mut icon = false;
    let mut out = Vec::new();
    assert!(fetch_kinds(&spot, &request, &api, game, "en", &mut icon, &mut out).unwrap());
    assert!(icon);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "icon: fetched SteamGridDB 1\n"
    );
    assert!(paths.art_dir("hades").join("icon.png").exists());
}

#[test]
fn steam_fetch_honors_explicit_game_and_search_overrides() {
    let (_root, paths) = paths();
    fs::write(
        paths.environment_config("hades"),
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1145360\"\n",
    )
    .unwrap();
    art::save_game(&paths, "hades", 99).unwrap();
    let mut request = fetch_request();
    request.game = Some(42);
    let fake = Fake::default();
    assert_eq!(
        fetch_game(&paths, "hades", &request, &Api::new(&fake, "k")).unwrap(),
        Some(42)
    );

    let mut fake = Fake::default();
    fake.search("other", false);
    request.game = None;
    request.search = Some("other");
    let err = fetch_game(&paths, "hades", &request, &Api::new(&fake, "k")).unwrap_err();
    assert_eq!(err.to_string(), "no SteamGridDB games match `other`");
}

#[test]
fn steam_fetch_reports_a_missing_app_id_match_without_searching() {
    let (_root, paths) = paths();
    fs::write(
        paths.environment_config("hades"),
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1145360\"\n",
    )
    .unwrap();
    let mut fake = Fake::default();
    fake.0.insert(
        format!("{}/games/steam/1145360", steamgriddb::API),
        (404, Vec::new()),
    );
    let err = fetch_game(&paths, "hades", &fetch_request(), &Api::new(&fake, "k")).unwrap_err();
    assert_eq!(
        err.to_string(),
        "SteamGridDB has no game for Steam app 1145360"
    );
}

#[test]
fn steam_fetch_reports_invalid_app_ids() {
    let (_root, paths) = paths();
    let fake = Fake::default();
    for app_id in ["abc", "18446744073709551616"] {
        fs::write(
            paths.environment_config("hades"),
            format!("kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"{app_id}\"\n"),
        )
        .unwrap();
        let err = fetch_game(&paths, "hades", &fetch_request(), &Api::new(&fake, "k")).unwrap_err();
        assert_eq!(err.to_string(), "Invalid app_id: not a number.");
    }
}

#[test]
fn non_steam_fetches_keep_saved_games_and_title_searches() {
    let (_root, paths) = paths();
    for config in [
        "prefix = \"~/p\"\nrunner = \"wine\"\narch = \"win64\"\n",
        "kind = \"native\"\n",
        "invalid config",
    ] {
        fs::write(paths.environment_config("hades"), config).unwrap();
        let request = fetch_request();
        let mut fake = Fake::default();
        fake.search("hades", false);
        let api = Api::new(&fake, "k");
        let err = fetch_game(&paths, "hades", &request, &api).unwrap_err();
        assert_eq!(err.to_string(), "no SteamGridDB games match `hades`");
        art::save_game(&paths, "hades", 99).unwrap();
        assert_eq!(
            fetch_game(&paths, "hades", &request, &api).unwrap(),
            Some(99)
        );
        fs::remove_dir_all(paths.art_dir("hades")).unwrap();
    }
}

fn run(paths: &Paths, fake: &Fake, only: Option<Kind>) -> (anyhow::Result<bool>, String) {
    let request = FetchRequest {
        spot: "hades",
        search: None,
        game: None,
        only,
        kinds: ArtFetch::All,
        mode: Mode::Fill,
    };
    let spot = Spot {
        paths,
        env: "hades",
        target: None,
        identity: EnvironmentIdentity::acquire_existing(paths, "hades").unwrap(),
    };
    let mut out = Vec::new();
    let mut icon = false;
    let result = fetch_kinds(
        &spot,
        &request,
        &Api::new(fake, "k"),
        7,
        "en",
        &mut icon,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn games_are_listed_with_their_year_and_id() {
    let game = |year| Game {
        id: 1234,
        name: "Hades".into(),
        year,
    };
    assert_eq!(game_label(&game(Some(2020))), "Hades (2020), 1234");
    assert_eq!(game_label(&game(None)), "Hades, 1234");
}

#[test]
fn each_kind_gets_a_line_and_only_an_empty_only_fails() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.page(Kind::Icon, &[1]);
    fake.0
        .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
    fake.page(Kind::Grid, &[]);
    fake.page(Kind::Hero, &[]);
    fake.page(Kind::Logo, &[]);
    let (result, out) = run(&paths, &fake, None);
    assert!(result.unwrap());
    assert_eq!(
        out,
        "icon: fetched SteamGridDB 1\ngrid: nothing found\nhero: nothing found\nlogo: nothing found\n"
    );

    let (result, out) = run(&paths, &fake, Some(Kind::Icon));
    assert!(result.unwrap());
    assert_eq!(out, "icon: skipped, it already has art\n");
    let (result, out) = run(&paths, &fake, Some(Kind::Hero));
    assert!(!result.unwrap());
    assert_eq!(out, "hero: nothing found\n");
}

#[test]
fn kinds_fetched_before_an_error_are_shown_and_kept() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.page(Kind::Icon, &[1]);
    fake.0
        .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
    fake.page(Kind::Grid, &[2]);
    let (result, out) = run(&paths, &fake, None);
    let err = result.unwrap_err();
    assert!(err.to_string().contains("cdn.example/2"), "{err}");
    assert_eq!(out, "icon: fetched SteamGridDB 1\n");
    assert!(paths.art_dir("hades").join("icon.png").exists());
}

fn key() -> Key {
    Key {
        value: "k".into(),
        source: KeySource::Variable,
    }
}

struct Outcome {
    result: Result<(), Canceled>,
    out: String,
    err: String,
}

fn fetch_new_with(
    paths: &Paths,
    fake: &Fake,
    kinds: ArtFetch,
    answers: Option<&[&str]>,
) -> Outcome {
    let request = AutoFetch {
        env: "hades",
        term: "hades",
        kinds,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut prompt = answers.map(|answers| Prompt::new(Script::new(answers), Vec::new()));
    let pick = match &mut prompt {
        Some(prompt) => Pick::Ask(prompt),
        None => Pick::Top(&mut out),
    };
    let result = fetch_new(paths, fake, &key(), &request, None, pick, &mut err);
    if let Some(prompt) = prompt {
        out = prompt.output().clone();
    }
    Outcome {
        result,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

struct FailingKeyring;

impl Keyring for FailingKeyring {
    fn get(&self) -> Result<Option<String>, KeyringError> {
        Err(KeyringError::Failed("dismissed".into()))
    }

    fn set(&self, _key: &str) -> Result<(), KeyringError> {
        unreachable!()
    }

    fn delete(&self) -> Result<bool, KeyringError> {
        unreachable!()
    }
}

#[test]
fn a_missing_key_notes_how_to_add_one() {
    let (_root, paths) = paths();
    let check = check_key_with(&paths, |_| None, &NoKeyring);
    assert!(matches!(check, KeyCheck::Missing));
    assert_eq!(
        check.skip_line().unwrap(),
        "note: run `wineo art key` to fetch art from SteamGridDB"
    );

    let check = check_key_with(&paths, |_| None, &FailingKeyring);
    assert!(matches!(check, KeyCheck::Failed(_)));
    assert_eq!(
        check.skip_line().unwrap(),
        "warning: the keyring failed: dismissed, skipping art"
    );

    let check = check_key_with(&paths, |_| Some("k".into()), &FailingKeyring);
    assert!(matches!(check, KeyCheck::Found(_)));
    assert_eq!(check.skip_line(), None);
}

#[test]
fn top_takes_the_first_match_and_names_it() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.icon_only();
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, None);
    assert!(outcome.result.is_ok());
    assert_eq!(
        outcome.out,
        "hades: art from SteamGridDB game 7, Hades (2020)\n\
         icon: fetched SteamGridDB 1\ngrid: nothing found\nhero: nothing found\nlogo: nothing found\n"
    );
    assert_eq!(outcome.err, "");
    assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(7));
    assert!(paths.art_dir("hades").join("icon.png").exists());
}

#[test]
fn asking_offers_a_skip_entry_that_fetches_nothing() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.icon_only();
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["4"]));
    assert!(outcome.result.is_ok());
    let out = &outcome.out;
    assert!(out.contains("1) Hades (2020), 7"), "{out}");
    assert!(out.contains("2) Hades II, 8"), "{out}");
    assert!(out.contains("3) Search for something else"), "{out}");
    assert!(out.contains("4) None of these, skip art"), "{out}");
    assert!(
        out.ends_with(
            "Game [1]: Skipped art. Fetch later with: wineo art fetch hades --search <term>\n"
        ),
        "{out}"
    );
    assert!(!out.contains("icon:"), "{out}");
    assert_eq!(art::saved_game(&paths, "hades").unwrap(), None);
    assert!(!paths.art_dir("hades").join("icon.png").exists());
}

#[test]
fn asking_again_searches_a_new_term_and_saves_the_pick() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.search_one("arena", 42, "Hades Arena");
    fake.page(Kind::Icon, &[]);
    fake.page(Kind::Grid, &[]);
    fake.page(Kind::Hero, &[]);
    fake.page(Kind::Logo, &[]);
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["3", "arena", "1"]));
    assert!(outcome.result.is_ok(), "{}", outcome.err);
    let out = &outcome.out;
    assert!(out.contains("3) Search for something else"), "{out}");
    assert!(out.contains("Search for: "), "{out}");
    assert!(out.contains("1) Hades Arena, 42"), "{out}");
    assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(42));
}

#[test]
fn backing_out_of_a_new_search_returns_to_the_same_list() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.icon_only();
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["3", "<", "1"]));
    assert!(outcome.result.is_ok(), "{}", outcome.err);
    assert_eq!(
        outcome.out.matches("Which game is it?").count(),
        2,
        "{}",
        outcome.out
    );
    assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(7));
}

#[test]
fn asking_with_no_matches_asks_for_a_term_and_succeeds() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", false);
    fake.search("hadesii", true);
    fake.icon_only();
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["hadesii", "1"]));
    assert!(outcome.result.is_ok(), "{}", outcome.err);
    let out = &outcome.out;
    assert!(
        out.contains("No SteamGridDB games match `hades`.\n"),
        "{out}"
    );
    assert!(out.contains("Search for: "), "{out}");
    assert!(out.contains("icon: fetched SteamGridDB 1\n"), "{out}");
    assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(7));
}

#[test]
fn backing_out_of_an_empty_search_skips_art() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", false);
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["<"]));
    assert!(outcome.result.is_ok(), "{}", outcome.err);
    assert!(
        outcome
            .out
            .contains("Skipped art. Fetch later with: wineo art fetch hades --search <term>\n"),
        "{}",
        outcome.out
    );
    assert_eq!(art::saved_game(&paths, "hades").unwrap(), None);
}

#[test]
fn asking_fetches_the_picked_game() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.icon_only();
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["1"]));
    assert!(outcome.result.is_ok());
    assert!(
        outcome
            .out
            .contains("Game [1]: icon: fetched SteamGridDB 1\n"),
        "{}",
        outcome.out
    );
    assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(7));
}

#[test]
fn canceling_the_pick_list_is_an_error() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, Some(&["q"]));
    assert!(outcome.result.is_err());
    assert_eq!(outcome.err, "");
}

#[test]
fn automatic_fetch_respects_custom_kinds_and_all_off() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.page(Kind::Grid, &[1]);
    fake.0
        .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
    let outcome = fetch_new_with(&paths, &fake, "grid".parse().unwrap(), None);
    assert!(outcome.result.is_ok());
    assert!(outcome.out.contains("grid: fetched"));
    assert!(!outcome.out.contains("icon:"));
    assert_eq!(outcome.err, "");
    assert!(paths.art_dir("hades").join("grid.png").exists());
    assert!(!paths.art_dir("hades").join("icon.png").exists());

    let empty = fetch_new_with(&paths, &Fake::default(), "none".parse().unwrap(), None);
    assert!(empty.result.is_ok());
    assert_eq!(empty.out, "");
    assert_eq!(empty.err, "");
}

#[test]
fn the_icon_setting_fetches_only_the_icon() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.page(Kind::Icon, &[1]);
    fake.0
        .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::Icon, None);
    assert!(outcome.result.is_ok());
    assert_eq!(
        outcome.out,
        "hades: art from SteamGridDB game 7, Hades (2020)\nicon: fetched SteamGridDB 1\n"
    );
    assert_eq!(outcome.err, "");
}

#[test]
fn a_failed_download_warns_and_keeps_earlier_kinds() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.page(Kind::Icon, &[1]);
    fake.0
        .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
    fake.page(Kind::Grid, &[2]);
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, None);
    assert!(outcome.result.is_ok());
    assert!(
        outcome.out.ends_with("icon: fetched SteamGridDB 1\n"),
        "{}",
        outcome.out
    );
    assert!(
        outcome.err.contains("warning: art fetch failed:"),
        "{}",
        outcome.err
    );
    assert!(outcome.err.contains("cdn.example/2"), "{}", outcome.err);
    assert!(paths.art_dir("hades").join("icon.png").exists());
}

#[test]
fn top_with_no_matches_warns_and_fetches_nothing() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", false);
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, None);
    assert!(outcome.result.is_ok());
    assert_eq!(outcome.out, "");
    assert!(
        outcome
            .err
            .contains("warning: art fetch failed: no SteamGridDB games match `hades`"),
        "{}",
        outcome.err
    );
    assert_eq!(art::saved_game(&paths, "hades").unwrap(), None);
}

#[test]
fn a_rejected_key_warns() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.0.insert(
        format!("{}/search/autocomplete/hades", steamgriddb::API),
        (401, Vec::new()),
    );
    let outcome = fetch_new_with(&paths, &fake, ArtFetch::All, None);
    assert!(outcome.result.is_ok());
    assert!(
        outcome.err.contains("SteamGridDB rejected the key"),
        "{}",
        outcome.err
    );
}

/// Drives `ask_game`, the interactive core `pick_game` uses once it knows
/// it has a terminal, with a scripted prompt and the fake transport.
fn ask_game_with(
    fake: &Fake,
    term: &str,
    answers: &[&str],
) -> (anyhow::Result<Option<u64>>, String) {
    let mut prompt = Prompt::new(Script::new(answers), Vec::new());
    let result = ask_game(&Api::new(fake, "k"), term, &mut prompt, false)
        .map(|pick| pick.map(|(game, _)| game));
    (result, String::from_utf8(prompt.output().clone()).unwrap())
}

#[test]
fn ask_game_offers_a_search_again_entry_last() {
    let mut fake = Fake::default();
    fake.search("hades", true);
    let (result, out) = ask_game_with(&fake, "hades", &["1"]);
    assert_eq!(result.unwrap(), Some(7));
    assert!(out.contains("1) Hades (2020), 7"), "{out}");
    assert!(out.contains("2) Hades II, 8"), "{out}");
    assert!(out.contains("3) Search for something else"), "{out}");
}

#[test]
fn ask_game_again_searches_a_new_term_and_picks_from_it() {
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.search_one("arena", 42, "Hades Arena");
    let (result, out) = ask_game_with(&fake, "hades", &["3", "arena", "1"]);
    assert_eq!(result.unwrap(), Some(42));
    assert!(out.contains("3) Search for something else"), "{out}");
    assert!(out.contains("Search for: "), "{out}");
    assert!(out.contains("1) Hades Arena, 42"), "{out}");
}

#[test]
fn ask_game_backs_out_of_a_new_search_to_the_same_list() {
    let mut fake = Fake::default();
    fake.search("hades", true);
    let (result, out) = ask_game_with(&fake, "hades", &["3", "<", "1"]);
    assert_eq!(result.unwrap(), Some(7));
    assert_eq!(out.matches("Which game is it?").count(), 2, "{out}");
}

#[test]
fn ask_game_with_no_matches_asks_for_a_term_and_succeeds() {
    let mut fake = Fake::default();
    fake.search("hades", false);
    fake.search("hadesii", true);
    let (result, out) = ask_game_with(&fake, "hades", &["hadesii", "1"]);
    assert_eq!(result.unwrap(), Some(7));
    assert!(
        out.contains("No SteamGridDB games match `hades`.\n"),
        "{out}"
    );
    assert!(out.contains("Search for: "), "{out}");
}

#[test]
fn ask_game_backing_out_of_an_empty_search_gives_up_the_pick() {
    let mut fake = Fake::default();
    fake.search("hades", false);
    let (result, out) = ask_game_with(&fake, "hades", &["<"]);
    assert_eq!(result.unwrap(), None);
    assert!(
        out.contains("No SteamGridDB games match `hades`.\n"),
        "{out}"
    );
}

#[test]
fn picking_from_a_new_search_saves_that_game() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.search_one("arena", 42, "Hades Arena");
    let (result, _) = ask_game_with(&fake, "hades", &["3", "arena", "1"]);
    let game = result.unwrap().unwrap();
    art::save_game(&paths, "hades", game).unwrap();
    assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(42));
}

const CHANGED: &str =
    "warning: art fetch failed: environment `hades` changed before artwork could be written\n";

#[test]
fn art_is_not_stored_in_an_environment_replaced_during_the_fetch() {
    let (_root, paths) = paths();
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.icon_only();
    let swap = super::fake::Swap::new(fake, &paths, "hades");
    let request = AutoFetch {
        env: "hades",
        term: "hades",
        kinds: ArtFetch::Icon,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = fetch_new(
        &paths,
        &swap,
        &key(),
        &request,
        None,
        Pick::<Script, Vec<u8>>::Top(&mut out),
        &mut err,
    );
    assert!(result.is_ok());
    assert_eq!(String::from_utf8(err).unwrap(), CHANGED);
    assert!(!paths.art_dir("hades").join("icon.png").exists());
}

const CHANGED_ERROR: &str = "environment `hades` changed before artwork could be written";

#[test]
fn art_fetch_fails_when_the_environment_is_replaced() {
    for replaced in [false, true] {
        let (_root, paths) = paths();
        let mut fake = Fake::default();
        fake.icon_only();
        let swap = super::fake::Swap::new(fake, &paths, "hades");
        let mut request = fetch_request();
        request.game = Some(7);
        // Without a swap the same fetch stores its icon.
        swap.done.set(!replaced);
        let mut out = Vec::new();
        let result = fetch_with(
            &paths,
            &request,
            &Api::new(&swap, "k"),
            (&mut out, &mut Vec::new()),
        );
        let icon = paths.art_dir("hades").join("icon.png").exists();
        if replaced {
            // `main` prints this as `error: ...` and exits 1.
            assert_eq!(result.unwrap_err().to_string(), CHANGED_ERROR);
            assert_eq!(String::from_utf8(out).unwrap(), "");
            assert!(!icon);
        } else {
            assert!(result.unwrap());
            assert_eq!(
                String::from_utf8(out).unwrap(),
                "icon: fetched SteamGridDB 1\n"
            );
            assert!(icon);
        }
    }
}

/// An environment with a desktop target, so reconciling it writes an entry.
fn desktop_environment(paths: &Paths) {
    fs::write(
        paths.environment_config("hades"),
        "prefix = \"~/p\"\nrunner = \"wine\"\narch = \"win64\"\n\n[targets.game]\nexe = \"game.exe\"\ndesktop = true\n",
    )
    .unwrap();
}

fn entry(paths: &Paths) -> std::path::PathBuf {
    paths.applications_dir().join("wineo-hades-game.desktop")
}

#[test]
fn an_environment_replaced_after_the_icon_gets_no_desktop_update() {
    for replaced in [false, true] {
        let (_root, paths) = paths();
        desktop_environment(&paths);
        let mut fake = Fake::default();
        fake.search("hades", true);
        fake.icon_only();
        // A grid to be downloaded, whose store is refused once replaced.
        fake.page(Kind::Grid, &[2]);
        fake.0.insert(
            "https://cdn.example/2".into(),
            (200, super::fake::PNG.to_vec()),
        );
        let swap = super::fake::Swap::new(fake, &paths, "hades").at("/grids/");
        swap.done.set(!replaced);
        let request = AutoFetch {
            env: "hades",
            term: "hades",
            kinds: ArtFetch::All,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let result = fetch_new(
            &paths,
            &swap,
            &key(),
            &request,
            None,
            Pick::<Script, Vec<u8>>::Top(&mut out),
            &mut err,
        );
        assert!(result.is_ok());
        let err = String::from_utf8(err).unwrap();
        // The icon is in the store either way.
        assert!(paths.art_dir("hades").join("icon.png").exists());
        if replaced {
            assert_eq!(err, CHANGED);
            assert!(!entry(&paths).exists());
        } else {
            assert_eq!(err, "");
            assert!(entry(&paths).exists());
        }
    }
}

#[test]
fn art_fetch_leaves_a_replacement_environments_entry_alone() {
    for replaced in [false, true] {
        let (_root, paths) = paths();
        desktop_environment(&paths);
        let mut fake = Fake::default();
        fake.icon_only();
        // A grid to be downloaded, whose store is refused once replaced.
        fake.page(Kind::Grid, &[2]);
        fake.0.insert(
            "https://cdn.example/2".into(),
            (200, super::fake::PNG.to_vec()),
        );
        let swap = super::fake::Swap::new(fake, &paths, "hades").at("/grids/");
        swap.done.set(!replaced);
        let mut request = fetch_request();
        request.game = Some(7);
        request.only = None;
        let mut out = Vec::new();
        let result = fetch_with(
            &paths,
            &request,
            &Api::new(&swap, "k"),
            (&mut out, &mut Vec::new()),
        );
        assert!(paths.art_dir("hades").join("icon.png").exists());
        assert_eq!(entry(&paths).exists(), !replaced);
        match result {
            Err(error) => assert!(replaced && error.to_string() == CHANGED_ERROR),
            Ok(found) => assert!(!replaced && found),
        }
    }
}

/// A `Fake` that runs `then` once, at the first request other than a search
/// whose address contains `at`.
struct At {
    fake: Fake,
    at: &'static str,
    then: std::cell::RefCell<Option<Box<dyn FnOnce()>>>,
}

impl At {
    fn new(fake: Fake, at: &'static str, then: impl FnOnce() + 'static) -> Self {
        Self {
            fake,
            at,
            then: std::cell::RefCell::new(Some(Box::new(then))),
        }
    }

    fn note(&self, url: &str) {
        if !url.contains("/search/")
            && url.contains(self.at)
            && let Some(then) = self.then.borrow_mut().take()
        {
            then();
        }
    }
}

impl Transport for At {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn std::io::Read>> {
        self.note(url);
        self.fake.get(url)
    }

    fn request(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> anyhow::Result<wineo_core::download::Response> {
        self.note(url);
        self.fake.request(url, bearer)
    }
}

fn library_id(paths: &Paths) -> Option<wineo_core::library::Id> {
    wineo_core::library::load(paths)
        .environments
        .get("hades")
        .and_then(|entry| entry.id)
}

/// `fetch_new` for the icon of `hades`, given the identity its creation
/// returned, or None.
fn fetch_new_for(paths: &Paths, identity: Option<EnvironmentIdentity>) -> String {
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.icon_only();
    let request = AutoFetch {
        env: "hades",
        term: "hades",
        kinds: ArtFetch::Icon,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = fetch_new(
        paths,
        &fake,
        &key(),
        &request,
        identity,
        Pick::<Script, Vec<u8>>::Top(&mut out),
        &mut err,
    );
    assert!(result.is_ok());
    String::from_utf8(err).unwrap()
}

#[test]
fn a_fetch_after_creation_holds_to_the_identity_creation_returned() {
    for replaced in [false, true] {
        let (_root, paths) = paths();
        let created = EnvironmentIdentity::acquire_existing(&paths, "hades").unwrap();
        // Between creation and the fetch, where a fresh read would take the
        // replacement for the environment that was made.
        if replaced {
            super::fake::replace_environment(&paths, "hades");
        }
        let err = fetch_new_for(&paths, Some(created));
        let icon = paths.art_dir("hades").join("icon.png").exists();
        if replaced {
            assert_eq!(err, CHANGED);
            assert!(!icon);
        } else {
            assert_eq!(err, "");
            assert!(icon);
        }
    }
}

#[test]
fn a_fetch_after_a_creation_that_returned_no_identity_acquires_one() {
    let (_root, paths) = paths();
    assert_eq!(library_id(&paths), None);
    assert_eq!(fetch_new_for(&paths, None), "");
    assert!(paths.art_dir("hades").join("icon.png").exists());
    assert!(library_id(&paths).is_some());
}

/// A Steam environment, whose SteamGridDB game is looked up while `art
/// fetch` would otherwise be asking which game it is.
fn steam_environment(paths: &Paths) -> Fake {
    fs::write(
        paths.environment_config("hades"),
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1145360\"\n",
    )
    .unwrap();
    let mut fake = Fake::default();
    fake.0.insert(
        format!("{}/games/steam/1145360", steamgriddb::API),
        (
            200,
            br#"{"success":true,"data":{"id":7,"name":"Hades"}}"#.to_vec(),
        ),
    );
    fake.icon_only();
    fake
}

fn icon_request() -> FetchRequest<'static> {
    FetchRequest {
        only: None,
        kinds: ArtFetch::Icon,
        ..fetch_request()
    }
}

#[test]
fn art_fetch_refuses_an_environment_replaced_while_the_game_is_picked() {
    for with_id in [false, true] {
        let (_root, paths) = paths();
        let fake = steam_environment(&paths);
        if with_id {
            EnvironmentIdentity::acquire_existing(&paths, "hades").unwrap();
        }
        let swap = super::fake::Swap::new(fake, &paths, "hades").at("/games/steam/");
        let mut out = Vec::new();
        let result = fetch_with(
            &paths,
            &icon_request(),
            &Api::new(&swap, "k"),
            (&mut out, &mut Vec::new()),
        );
        assert!(swap.done.get());
        assert_eq!(result.unwrap_err().to_string(), CHANGED_ERROR, "{with_id}");
        assert_eq!(String::from_utf8(out).unwrap(), "");
        assert!(!paths.art_dir("hades").join("icon.png").exists());
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), None);
    }
}

#[test]
fn art_fetch_gives_an_old_environment_an_id_only_once_a_game_is_picked() {
    let (_root, paths) = paths();
    let mut fake = steam_environment(&paths);
    let lookup = format!("{}/games/steam/1145360", steamgriddb::API);
    let answer = fake.0.remove(&lookup).unwrap();
    // No game, so nothing is picked and nothing is written.
    let result = fetch_with(
        &paths,
        &icon_request(),
        &Api::new(&fake, "k"),
        (&mut Vec::new(), &mut Vec::new()),
    );
    assert!(result.is_err());
    assert!(!paths.library_file().exists());

    fake.0.insert(lookup, answer);
    let result = fetch_with(
        &paths,
        &icon_request(),
        &Api::new(&fake, "k"),
        (&mut Vec::new(), &mut Vec::new()),
    );
    assert!(result.unwrap());
    assert!(paths.art_dir("hades").join("icon.png").exists());
    assert!(library_id(&paths).is_some());
}

const ENTRIES_SKIPPED: &str =
    "warning: library.toml can't be read. The desktop entries for `hades` weren't updated.\n";
const NOT_STORED: &str = "library.toml can't be read. Artwork for `hades` wasn't stored.";

/// A game with an icon, after which `library.toml` stops being readable.
fn library_breaks_after_the_icon(paths: &Paths) -> At {
    let mut fake = Fake::default();
    fake.search("hades", true);
    fake.icon_only();
    let library = paths.library_file();
    At::new(fake, "/grids/", move || {
        fs::write(library, "not [toml").unwrap();
    })
}

#[test]
fn entries_skipped_because_the_library_cant_be_read_are_warned_about() {
    let (_root, paths) = paths();
    desktop_environment(&paths);
    let request = AutoFetch {
        env: "hades",
        term: "hades",
        kinds: ArtFetch::All,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = fetch_new(
        &paths,
        &library_breaks_after_the_icon(&paths),
        &key(),
        &request,
        None,
        Pick::<Script, Vec<u8>>::Top(&mut out),
        &mut err,
    );
    assert!(result.is_ok());
    assert!(paths.art_dir("hades").join("icon.png").exists());
    assert!(!entry(&paths).exists());
    assert_eq!(
        String::from_utf8(err).unwrap(),
        format!("{ENTRIES_SKIPPED}warning: art fetch failed: {NOT_STORED}\n")
    );
}

#[test]
fn art_fetch_warns_about_entries_skipped_because_the_library_cant_be_read() {
    let (_root, paths) = paths();
    desktop_environment(&paths);
    let mut request = fetch_request();
    request.game = Some(7);
    request.only = None;
    let transport = library_breaks_after_the_icon(&paths);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = fetch_with(
        &paths,
        &request,
        &Api::new(&transport, "k"),
        (&mut out, &mut err),
    );
    assert_eq!(result.unwrap_err().to_string(), NOT_STORED);
    assert!(paths.art_dir("hades").join("icon.png").exists());
    assert!(!entry(&paths).exists());
    assert_eq!(String::from_utf8(err).unwrap(), ENTRIES_SKIPPED);
}
