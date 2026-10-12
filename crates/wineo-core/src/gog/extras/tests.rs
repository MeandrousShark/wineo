use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use wineo_gog::UserId;

use super::*;
use crate::art::{ArtConfig, Format};
use crate::gog::account::{StoredLogin, store};
use crate::gog::test_support::{FakeKeyring, Scripted, client_at, setup, tokens};
use crate::steam::api::fixture::reply;
use crate::steamgriddb::fake::FakeApi;
use crate::steamgriddb::{self, Filters};

const GAME: u64 = 1094900565;
const USER: u64 = 48_628_349_957_132_247;
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const JPEG: &[u8] = b"\xff\xd8\xff\xe0\0\x10JFIF\0";

fn url(name: &str) -> String {
    format!("https://images.gog-statics.com/{name}")
}

/// GOG's v2 answer with every link, minus `without`.
fn art_body(without: &[&str]) -> String {
    let mut links = serde_json::Map::new();
    for (key, file) in [
        ("icon", "icon.png"),
        ("iconSquare", "square.png"),
        ("logo", "logo.png"),
        ("boxArtImage", "box.jpg"),
        ("backgroundImage", "bg.jpg"),
        ("galaxyBackgroundImage", "galaxy.jpg"),
    ] {
        if !without.contains(&key) {
            links.insert(key.to_owned(), json!({ "href": url(file) }));
        }
    }
    json!({
        "_links": links,
        "_embedded": {"product": {"id": GAME, "title": "RimWorld"}},
    })
    .to_string()
}

/// The images GOG's links lead to.
fn images() -> FakeApi {
    let api = FakeApi::default();
    api.bytes(url("box.jpg"), JPEG);
    api.bytes(url("bg.jpg"), JPEG);
    api.bytes(url("galaxy.jpg"), JPEG);
    api.bytes(url("logo.png"), PNG);
    api.bytes(url("icon.png"), PNG);
    api.bytes(url("square.png"), PNG);
    api
}

struct Fixture {
    _root: tempfile::TempDir,
    paths: Paths,
    keyring: FakeKeyring,
}

impl Fixture {
    fn new() -> Self {
        let (root, paths) = setup();
        let fixture = Self {
            _root: root,
            paths,
            keyring: FakeKeyring::default(),
        };
        fixture.environment("rimworld", &GAME.to_string());
        fixture
    }

    fn environment(&self, name: &str, gog_id: &str) {
        let path = self.paths.environment_config(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "schema_version = 1\nkind = \"native\"\ngog_id = \"{gog_id}\"\n[targets.game]\nexe = \"/bin/true\"\ndefault = true\n"
            ),
        )
        .unwrap();
    }

    fn login(&self) {
        let login = StoredLogin {
            refresh_token: "REFRESH0".to_owned(),
            user_id: UserId::new(USER),
        };
        store(&self.paths, &self.keyring, &login).unwrap();
    }

    fn session(&self, base: &str) -> Session<'_> {
        Session::open(&self.paths, &self.keyring, client_at(base))
            .unwrap()
            .unwrap()
    }

    fn imported(&self) -> Option<Imported> {
        stats::read(&self.paths.stats_log("rimworld"))
            .unwrap()
            .imported
    }

    fn imported_lines(&self) -> usize {
        fs::read_to_string(self.paths.stats_log("rimworld"))
            .map(|text| {
                text.lines()
                    .filter(|line| line.contains("imported"))
                    .count()
            })
            .unwrap_or(0)
    }

    fn art_config(&self) -> ArtConfig {
        ArtConfig::load(&self.paths.art_dir("rimworld")).unwrap()
    }

    fn art_file(&self, name: &str) -> std::path::PathBuf {
        self.paths.art_dir("rimworld").join(name)
    }
}

/// Answers the token, playtime, and art requests of one game.
fn gog(minutes: Option<u64>, art: String) -> impl Fn(&str) -> Vec<u8> + Send + 'static {
    move |line| {
        if line.contains("/token") {
            tokens("ACCESS", "REFRESH1", USER, 3600)
        } else if line.contains("/sessions") {
            match minutes {
                Some(minutes) => reply(200, &json!({ "time_sum": minutes }).to_string()),
                None => reply(404, "{}"),
            }
        } else if line.contains("/v2/games/") {
            reply(200, &art)
        } else {
            reply(404, "{}")
        }
    }
}

struct Options<'a> {
    kinds: &'a [Kind],
    key: Option<&'a str>,
}

const ALL: Options = Options {
    kinds: &Kind::ALL,
    key: None,
};

fn go(
    fixture: &Fixture,
    session: Option<&mut Session<'_>>,
    server: &Scripted,
    transport: &FakeApi,
    options: &Options,
) -> Result<Report> {
    let request = Request {
        name: "rimworld",
        gog_id: GAME,
        title: "RimWorld",
        kinds: options.kinds,
        transport,
        steamgriddb_key: options.key,
        language: "en",
    };
    run(
        &fixture.paths,
        session,
        &request,
        Ok(client_at(&server.base)),
        &Token::new(),
    )
}

/// The notes about artwork, without the logged-out one these tests share.
fn art_notes(report: &Report) -> Vec<&str> {
    report
        .notes
        .iter()
        .map(String::as_str)
        .filter(|note| !note.starts_with("Skipped GOG playtime"))
        .collect()
}

fn filled(report: &Report) -> Vec<(Kind, Source)> {
    report
        .art
        .iter()
        .map(|filled| (filled.kind, filled.source.clone()))
        .collect()
}

#[test]
fn gog_playtime_becomes_the_baseline_and_importing_again_changes_nothing() {
    let fixture = Fixture::new();
    fixture.login();
    let server = Scripted::start(gog(Some(120), art_body(&[])));
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    assert_eq!(
        report.playtime,
        Some(Playtime {
            gog_seconds: 7200,
            before_seconds: 0,
            after_seconds: 7200,
        })
    );
    assert!(report.playtime.unwrap().raised());
    assert_eq!(report.notes, Vec::<String>::new());
    assert_eq!(fixture.imported().unwrap().seconds, 7200);

    let again = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    assert_eq!(
        again.playtime,
        Some(Playtime {
            gog_seconds: 7200,
            before_seconds: 7200,
            after_seconds: 7200,
        })
    );
    assert!(!again.playtime.unwrap().raised());
    assert_eq!(fixture.imported_lines(), 1, "nothing was appended");
}

#[test]
fn a_smaller_gog_total_never_lowers_the_baseline() {
    let fixture = Fixture::new();
    fixture.login();
    // What Heroic's playtime carried over at migration.
    let played = "2026-05-01T10:00:00Z".parse().unwrap();
    stats::set_imported(
        &fixture.paths,
        "rimworld",
        Imported {
            seconds: 10_000,
            last_played: Some(played),
        },
    )
    .unwrap();
    let server = Scripted::start(gog(Some(60), art_body(&[])));
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    let playtime = report.playtime.unwrap();
    assert_eq!(
        (playtime.before_seconds, playtime.after_seconds),
        (10_000, 10_000)
    );
    assert_eq!(fixture.imported_lines(), 1);
}

#[test]
fn a_larger_gog_total_raises_the_baseline_and_keeps_the_last_played_date() {
    let fixture = Fixture::new();
    fixture.login();
    let played = "2026-05-01T10:00:00Z".parse().unwrap();
    stats::set_imported(
        &fixture.paths,
        "rimworld",
        Imported {
            seconds: 1000,
            last_played: Some(played),
        },
    )
    .unwrap();
    let server = Scripted::start(gog(Some(60), art_body(&[])));
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    let playtime = report.playtime.unwrap();
    assert_eq!(
        (playtime.before_seconds, playtime.after_seconds),
        (1000, 3600)
    );
    assert_eq!(
        fixture.imported(),
        Some(Imported {
            seconds: 3600,
            last_played: Some(played),
        })
    );
}

#[test]
fn a_gog_total_of_zero_writes_nothing() {
    let fixture = Fixture::new();
    fixture.login();
    let server = Scripted::start(gog(Some(0), art_body(&[])));
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    assert_eq!(
        report.playtime.map(|playtime| playtime.after_seconds),
        Some(0)
    );
    assert!(!fixture.paths.stats_log("rimworld").exists());
}

#[test]
fn a_game_gog_has_no_sessions_for_is_a_note_not_an_error() {
    let fixture = Fixture::new();
    fixture.login();
    let server = Scripted::start(gog(None, art_body(&[])));
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    assert_eq!(report.playtime, None);
    assert_eq!(report.notes, ["GOG has no playtime for RimWorld."]);
    assert!(!fixture.paths.stats_log("rimworld").exists());
    assert_eq!(report.art.len(), 4, "art isn't held up by it");
}

#[test]
fn logged_out_skips_playtime_with_a_note_and_still_fills_art() {
    let fixture = Fixture::new();
    let server = Scripted::start(gog(Some(120), art_body(&[])));
    let report = go(&fixture, None, &server, &images(), &ALL).unwrap();
    assert_eq!(report.playtime, None);
    assert_eq!(
        report.notes,
        ["Skipped GOG playtime: not logged in to GOG."]
    );
    assert!(!fixture.paths.stats_log("rimworld").exists());
    assert_eq!(report.art.len(), 4);
    assert!(
        server
            .requests()
            .iter()
            .all(|line| !line.contains("/sessions")),
        "{:?}",
        server.requests()
    );
}

#[test]
fn a_playtime_failure_is_a_note_and_leaves_the_baseline() {
    let fixture = Fixture::new();
    fixture.login();
    let art = art_body(&[]);
    let server = Scripted::start(move |line| {
        if line.contains("/sessions") {
            reply(500, "{}")
        } else {
            gog(Some(1), art.clone())(line)
        }
    });
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    assert_eq!(report.playtime, None);
    assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
    assert!(
        report.notes[0].starts_with("Couldn't import GOG playtime: GOG playtime request failed"),
        "{:?}",
        report.notes
    );
    assert_eq!(fixture.imported(), None);
    assert_eq!(report.art.len(), 4);
}

#[test]
fn a_refused_access_token_is_renewed_and_playtime_still_imports() {
    let fixture = Fixture::new();
    fixture.login();
    let sessions = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&sessions);
    let art = art_body(&[]);
    let server = Scripted::start(move |line| {
        if line.contains("/sessions") && counted.fetch_add(1, Ordering::Relaxed) == 0 {
            reply(401, "{}")
        } else {
            gog(Some(30), art.clone())(line)
        }
    });
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    assert_eq!(
        report.playtime.map(|playtime| playtime.after_seconds),
        Some(1800)
    );
    assert_eq!(sessions.load(Ordering::Relaxed), 2);
}

#[test]
fn a_login_that_is_gone_is_a_note_without_failing() {
    let fixture = Fixture::new();
    fixture.login();
    let art = art_body(&[]);
    let server = Scripted::start(move |line| {
        if line.contains("/token") {
            reply(400, r#"{"error":"invalid_grant"}"#)
        } else {
            gog(Some(30), art.clone())(line)
        }
    });
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    assert_eq!(report.playtime, None);
    assert!(
        report.notes[0].starts_with("Couldn't import GOG playtime: Log in to GOG again"),
        "{:?}",
        report.notes
    );
    assert_eq!(report.art.len(), 4);
}

#[test]
fn a_total_too_large_to_hold_is_a_note() {
    let fixture = Fixture::new();
    fixture.login();
    let server = Scripted::start(gog(Some(u64::MAX / 30), art_body(&[])));
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    assert_eq!(report.playtime, None);
    assert!(
        report.notes[0].contains("out of range"),
        "{:?}",
        report.notes
    );
    assert_eq!(fixture.imported(), None);
}

#[test]
fn an_environment_replaced_while_fetching_gets_no_playtime() {
    let fixture = Fixture::new();
    fixture.login();
    let config = fixture.paths.environment_config("rimworld");
    let art = art_body(&[]);
    let server = Scripted::start(move |line| {
        if line.contains("/sessions") {
            // A new file renamed over the old one has another inode.
            let next = config.with_extension("next");
            fs::write(&next, fs::read(&config).unwrap()).unwrap();
            fs::rename(&next, &config).unwrap();
        }
        gog(Some(30), art.clone())(line)
    });
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    assert_eq!(report.playtime, None);
    assert!(
        report.notes[0].contains("changed while"),
        "{:?}",
        report.notes
    );
    assert_eq!(fixture.imported(), None);
    // Art is refused for the replaced config too.
    assert!(report.art.is_empty());
}

#[test]
fn gog_art_fills_every_empty_kind_recorded_with_source_gog() {
    let fixture = Fixture::new();
    let server = Scripted::start(gog(None, art_body(&[])));
    let transport = images();
    let report = go(&fixture, None, &server, &transport, &ALL).unwrap();
    assert_eq!(
        filled(&report),
        [
            (Kind::Icon, Source::Gog(GAME)),
            (Kind::Grid, Source::Gog(GAME)),
            (Kind::Hero, Source::Gog(GAME)),
            (Kind::Logo, Source::Gog(GAME)),
        ]
    );
    // The cover, the wider Galaxy background, the transparent logo, and the
    // square icon.
    assert_eq!(fs::read(fixture.art_file("grid.jpg")).unwrap(), JPEG);
    assert_eq!(fs::read(fixture.art_file("hero.jpg")).unwrap(), JPEG);
    assert_eq!(fs::read(fixture.art_file("logo.png")).unwrap(), PNG);
    assert_eq!(fs::read(fixture.art_file("icon.png")).unwrap(), PNG);
    let urls = transport.urls();
    for wanted in ["box.jpg", "galaxy.jpg", "logo.png", "square.png"] {
        assert!(urls.contains(&url(wanted)), "{wanted} in {urls:?}");
    }
    for unwanted in ["bg.jpg", "icon.png"] {
        assert!(!urls.contains(&url(unwanted)), "{unwanted} in {urls:?}");
    }
    let config = fixture.art_config();
    for kind in Kind::ALL {
        assert_eq!(
            config.record(None, kind).map(|record| &record.source),
            Some(&Source::Gog(GAME)),
            "{kind}"
        );
    }
    let text = fs::read_to_string(fixture.art_file(art::ART_FILE)).unwrap();
    assert!(text.contains(&format!("gog = {GAME}")), "{text}");
}

#[test]
fn the_plain_background_and_icon_stand_in_when_the_better_ones_are_missing() {
    let fixture = Fixture::new();
    let server = Scripted::start(gog(
        None,
        art_body(&["galaxyBackgroundImage", "iconSquare"]),
    ));
    let transport = images();
    let report = go(&fixture, None, &server, &transport, &ALL).unwrap();
    assert_eq!(report.art.len(), 4);
    let urls = transport.urls();
    assert!(urls.contains(&url("bg.jpg")) && urls.contains(&url("icon.png")));
    assert!(!urls.contains(&url("galaxy.jpg")) && !urls.contains(&url("square.png")));
}

#[test]
fn art_the_environment_already_has_is_never_replaced() {
    let fixture = Fixture::new();
    let dir = fixture.paths.art_dir("rimworld");
    fs::create_dir_all(&dir).unwrap();
    // Unrecorded, and recorded as manual, both count as art.
    fs::write(dir.join("grid.png"), PNG).unwrap();
    fs::write(dir.join("hero.png"), PNG).unwrap();
    fs::write(
        dir.join(art::ART_FILE),
        "schema_version = 1\n[hero]\nfile = \"hero.png\"\nmanual = \"mine.png\"\n",
    )
    .unwrap();
    let server = Scripted::start(gog(None, art_body(&[])));
    let transport = images();
    let report = go(&fixture, None, &server, &transport, &ALL).unwrap();
    assert_eq!(
        filled(&report),
        [
            (Kind::Icon, Source::Gog(GAME)),
            (Kind::Logo, Source::Gog(GAME)),
        ]
    );
    let urls = transport.urls();
    assert!(!urls.contains(&url("box.jpg")) && !urls.contains(&url("galaxy.jpg")));
    assert!(!dir.join("grid.jpg").exists() && !dir.join("hero.jpg").exists());
    assert_eq!(fs::read(dir.join("grid.png")).unwrap(), PNG);
    let config = fixture.art_config();
    assert_eq!(
        config.record(None, Kind::Hero).map(|record| &record.source),
        Some(&Source::Manual("mine.png".to_owned()))
    );
    assert_eq!(config.record(None, Kind::Grid), None);
}

#[test]
fn nothing_is_asked_of_gog_when_every_kind_already_has_art() {
    let fixture = Fixture::new();
    let dir = fixture.paths.art_dir("rimworld");
    fs::create_dir_all(&dir).unwrap();
    for file in ["icon.png", "grid.png", "hero.png", "logo.png"] {
        fs::write(dir.join(file), PNG).unwrap();
    }
    let server = Scripted::start(gog(None, art_body(&[])));
    let report = go(&fixture, None, &server, &images(), &ALL).unwrap();
    assert!(report.art.is_empty());
    assert!(
        server
            .requests()
            .iter()
            .all(|line| !line.contains("/v2/games")),
        "{:?}",
        server.requests()
    );
}

#[test]
fn only_the_asked_for_kinds_are_filled() {
    let fixture = Fixture::new();
    let server = Scripted::start(gog(None, art_body(&[])));
    let options = Options {
        kinds: &[Kind::Icon],
        key: None,
    };
    let report = go(&fixture, None, &server, &images(), &options).unwrap();
    assert_eq!(filled(&report), [(Kind::Icon, Source::Gog(GAME))]);
    assert!(!fixture.art_file("grid.jpg").exists());
}

/// SteamGridDB's only page of `kind` for game 7.
fn serve(api: &FakeApi, kind: Kind, id: u64, bytes: &[u8]) {
    let url = format!("https://cdn.example/{id}");
    api.json(
        steamgriddb::Api::images_url(7, kind, 0, Filters::Ranked),
        json!([{ "id": id, "url": url, "score": 1 }]),
    );
    api.json(
        steamgriddb::Api::images_url(7, kind, 1, Filters::Ranked),
        json!([]),
    );
    api.bytes(url, bytes);
}

#[test]
fn a_kind_gog_lacks_comes_from_the_saved_steamgriddb_game() {
    let fixture = Fixture::new();
    art::save_game(&fixture.paths, "rimworld", 7).unwrap();
    let server = Scripted::start(gog(None, art_body(&["logo"])));
    let transport = images();
    serve(&transport, Kind::Logo, 41, PNG);
    let options = Options {
        kinds: &Kind::ALL,
        key: Some("sgdb-key"),
    };
    let report = go(&fixture, None, &server, &transport, &options).unwrap();
    assert_eq!(
        filled(&report),
        [
            (Kind::Icon, Source::Gog(GAME)),
            (Kind::Grid, Source::Gog(GAME)),
            (Kind::Hero, Source::Gog(GAME)),
            (Kind::Logo, Source::SteamGridDb(41)),
        ]
    );
    assert_eq!(art_notes(&report), Vec::<&str>::new());
    assert_eq!(fs::read(fixture.art_file("logo.png")).unwrap(), PNG);
}

#[test]
fn without_a_saved_game_steamgriddb_is_searched_for_the_exact_title() {
    let fixture = Fixture::new();
    let server = Scripted::start(gog(None, art_body(&["logo"])));
    let transport = images();
    transport.json(
        format!("{}/search/autocomplete/RimWorld", steamgriddb::API),
        json!([{"id": 9, "name": "RimWorld Demo"}, {"id": 7, "name": "rimworld"}]),
    );
    serve(&transport, Kind::Logo, 41, PNG);
    let options = Options {
        kinds: &Kind::ALL,
        key: Some("sgdb-key"),
    };
    let report = go(&fixture, None, &server, &transport, &options).unwrap();
    assert!(filled(&report).contains(&(Kind::Logo, Source::SteamGridDb(41))));
    assert_eq!(
        art::saved_game(&fixture.paths, "rimworld").unwrap(),
        Some(7)
    );
}

#[test]
fn a_search_with_no_exact_title_fills_nothing_and_says_so() {
    let fixture = Fixture::new();
    let server = Scripted::start(gog(None, art_body(&["logo"])));
    let transport = images();
    transport.json(
        format!("{}/search/autocomplete/RimWorld", steamgriddb::API),
        json!([{"id": 9, "name": "RimWorld Demo"}]),
    );
    let options = Options {
        kinds: &Kind::ALL,
        key: Some("sgdb-key"),
    };
    let report = go(&fixture, None, &server, &transport, &options).unwrap();
    assert_eq!(report.art.len(), 3);
    assert_eq!(
        art_notes(&report),
        ["SteamGridDB has no game titled RimWorld."]
    );
    assert_eq!(art::saved_game(&fixture.paths, "rimworld").unwrap(), None);
    assert!(!fixture.art_file("logo.png").exists());
}

#[test]
fn without_a_steamgriddb_key_a_missing_kind_stays_empty() {
    let fixture = Fixture::new();
    let server = Scripted::start(gog(None, art_body(&["logo"])));
    let transport = images();
    let report = go(&fixture, None, &server, &transport, &ALL).unwrap();
    assert_eq!(report.art.len(), 3);
    assert_eq!(art_notes(&report), Vec::<&str>::new());
    assert!(
        transport
            .urls()
            .iter()
            .all(|url| !url.contains("steamgriddb")),
        "{:?}",
        transport.urls()
    );
}

#[test]
fn steamgriddb_having_nothing_for_a_kind_is_a_note() {
    let fixture = Fixture::new();
    art::save_game(&fixture.paths, "rimworld", 7).unwrap();
    let server = Scripted::start(gog(None, art_body(&["logo"])));
    let transport = images();
    transport.json(
        steamgriddb::Api::images_url(7, Kind::Logo, 0, Filters::Ranked),
        json!([]),
    );
    let options = Options {
        kinds: &Kind::ALL,
        key: Some("sgdb-key"),
    };
    let report = go(&fixture, None, &server, &transport, &options).unwrap();
    assert_eq!(
        art_notes(&report),
        ["SteamGridDB has no logo artwork for RimWorld."]
    );
}

#[test]
fn a_failed_download_is_a_warning_and_the_other_kinds_still_fill() {
    let fixture = Fixture::new();
    let server = Scripted::start(gog(None, art_body(&[])));
    let transport = FakeApi::default();
    transport.bytes(url("box.jpg"), JPEG);
    transport.bytes(url("logo.png"), PNG);
    transport.bytes(url("square.png"), PNG);
    // The hero's link answers with something that isn't an image.
    transport.bytes(url("galaxy.jpg"), b"<html>nope</html>");
    let options = Options {
        kinds: &Kind::ALL,
        key: Some("sgdb-key"),
    };
    let report = go(&fixture, None, &server, &transport, &options).unwrap();
    assert_eq!(
        report
            .art
            .iter()
            .map(|filled| filled.kind)
            .collect::<Vec<_>>(),
        [Kind::Icon, Kind::Grid, Kind::Logo]
    );
    assert_eq!(art_notes(&report).len(), 1, "{:?}", report.notes);
    assert!(
        art_notes(&report)[0].starts_with("Couldn't fetch GOG hero artwork: "),
        "{:?}",
        report.notes
    );
    assert!(!fixture.art_file("hero.jpg").exists());

    // A link that can't be downloaded at all is a warning too, and a failed
    // GOG image doesn't count as GOG lacking one, so nothing is searched.
    let other = Fixture::new();
    let empty = FakeApi::default();
    let report = go(&other, None, &server, &empty, &options).unwrap();
    assert!(report.art.is_empty());
    assert_eq!(art_notes(&report).len(), 4, "{:?}", report.notes);
    assert!(empty.urls().iter().all(|url| url.contains("gog-statics")));
}

#[test]
fn a_failed_art_request_is_a_note_and_never_falls_back() {
    let fixture = Fixture::new();
    fixture.login();
    let server = Scripted::start(|line| {
        if line.contains("/v2/games/") {
            reply(500, "{}")
        } else {
            gog(Some(60), String::new())(line)
        }
    });
    let mut session = fixture.session(&server.base);
    let transport = images();
    let options = Options {
        kinds: &Kind::ALL,
        key: Some("sgdb-key"),
    };
    let report = go(&fixture, Some(&mut session), &server, &transport, &options).unwrap();
    assert!(report.art.is_empty());
    assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
    assert!(
        report.notes[0].starts_with("Couldn't get GOG artwork: GOG artwork request failed"),
        "{:?}",
        report.notes
    );
    assert!(transport.urls().is_empty());
    // Playtime is unaffected.
    assert_eq!(
        report.playtime.map(|playtime| playtime.after_seconds),
        Some(3600)
    );
}

#[test]
fn no_note_shows_a_token_or_a_url_with_a_query() {
    let fixture = Fixture::new();
    fixture.login();
    let server = Scripted::start(|_| reply(500, "{}"));
    let mut session = fixture.session(&server.base);
    let report = go(&fixture, Some(&mut session), &server, &images(), &ALL).unwrap();
    assert_eq!(report.notes.len(), 2);
    let shown = report.notes.join("\n");
    for secret in ["ACCESS", "REFRESH", "127.0.0.1", "?", &USER.to_string()] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
}

#[test]
fn a_portable_root_is_refused_before_anything_is_fetched() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let portable = Paths::select(
        move |key| (key == "HOME").then(|| home.clone().into_os_string()),
        Some(&root.path().join("usb")),
    )
    .unwrap();
    let server = Scripted::start(gog(Some(1), art_body(&[])));
    let request = Request {
        name: "rimworld",
        gog_id: GAME,
        title: "RimWorld",
        kinds: &Kind::ALL,
        transport: &images(),
        steamgriddb_key: None,
        language: "en",
    };
    let error = run(
        &portable,
        None,
        &request,
        Ok(client_at(&server.base)),
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "GOG games live in the host library. Run this without --portable."
    );
    assert!(server.requests().is_empty());
}

#[test]
fn an_environment_that_doesnt_run_the_game_is_refused_before_anything_is_fetched() {
    let fixture = Fixture::new();
    fixture.environment("rimworld", "42");
    let server = Scripted::start(gog(Some(1), art_body(&[])));
    let error = go(&fixture, None, &server, &images(), &ALL).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("rimworld doesn't run GOG game {GAME}.")
    );
    fs::remove_file(fixture.paths.environment_config("rimworld")).unwrap();
    let error = go(&fixture, None, &server, &images(), &ALL).unwrap_err();
    assert!(error.to_string().contains("rimworld"), "{error}");
    assert!(server.requests().is_empty());
    assert!(!fixture.paths.art_dir("rimworld").exists());
}

#[test]
fn a_gog_client_that_cant_be_built_skips_art_with_a_note() {
    let fixture = Fixture::new();
    let request = Request {
        name: "rimworld",
        gog_id: GAME,
        title: "RimWorld",
        kinds: &Kind::ALL,
        transport: &FakeApi::default(),
        steamgriddb_key: None,
        language: "en",
    };
    let error = wineo_gog::Client::with_base("wineo-test", "not a url")
        .err()
        .unwrap();
    let report = run(&fixture.paths, None, &request, Err(error), &Token::new()).unwrap();
    assert!(report.art.is_empty());
    assert_eq!(report.notes.len(), 2, "{:?}", report.notes);
    assert!(
        report.notes[1].starts_with("Skipped GOG artwork"),
        "{:?}",
        report.notes
    );
}

#[test]
fn formats_gog_serves_are_stored_under_their_real_extension() {
    // The bytes decide the format, not the link's extension.
    let fixture = Fixture::new();
    let server = Scripted::start(gog(None, art_body(&[])));
    let transport = images();
    transport.bytes(url("box.jpg"), PNG);
    let options = Options {
        kinds: &[Kind::Grid],
        key: None,
    };
    go(&fixture, None, &server, &transport, &options).unwrap();
    let config = fixture.art_config();
    assert_eq!(
        config.record(None, Kind::Grid).map(|record| record.format),
        Some(Format::Png)
    );
    assert!(fixture.art_file("grid.png").exists() && !fixture.art_file("grid.jpg").exists());
}

/// Runs `hook` when `when` is requested, as another command replacing the
/// environment would at that moment.
struct Hooked<'a> {
    inner: &'a FakeApi,
    when: String,
    hook: Box<dyn Fn() + 'a>,
}

impl crate::download::Transport for Hooked<'_> {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn std::io::Read>> {
        self.inner.get(url)
    }

    fn request(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> anyhow::Result<crate::download::Response> {
        self.request_with_length(url, bearer)
            .map(|(response, _)| response)
    }

    fn request_with_length(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> anyhow::Result<(crate::download::Response, Option<u64>)> {
        if url.contains(&self.when) {
            (self.hook)();
        }
        self.inner.request_with_length(url, bearer)
    }
}

/// Puts a different game's config in place of the environment's, as a new
/// file so it has another identity.
fn replace_environment(fixture: &Fixture) {
    let config = fixture.paths.environment_config("rimworld");
    let next = config.with_extension("next");
    fs::write(
        &next,
        "schema_version = 1\nkind = \"native\"\ngog_id = \"999\"\n[targets.game]\nexe = \"/bin/true\"\ndefault = true\n",
    )
    .unwrap();
    fs::rename(&next, &config).unwrap();
}

#[test]
fn an_environment_replaced_during_the_steamgriddb_search_is_left_alone() {
    let fixture = Fixture::new();
    let server = Scripted::start(gog(None, art_body(&["logo"])));
    let inner = images();
    inner.json(
        format!("{}/search/autocomplete/RimWorld", steamgriddb::API),
        json!([{"id": 7, "name": "RimWorld"}]),
    );
    serve(&inner, Kind::Logo, 41, PNG);
    let transport = Hooked {
        inner: &inner,
        when: "/search/autocomplete/".to_owned(),
        hook: Box::new(|| replace_environment(&fixture)),
    };
    let request = Request {
        name: "rimworld",
        gog_id: GAME,
        title: "RimWorld",
        kinds: &Kind::ALL,
        transport: &transport,
        steamgriddb_key: Some("sgdb-key"),
        language: "en",
    };
    let report = run(
        &fixture.paths,
        None,
        &request,
        Ok(client_at(&server.base)),
        &Token::new(),
    )
    .unwrap();
    assert_eq!(report.art.len(), 3, "{:?}", report.art);
    assert!(!report.art.iter().any(|filled| filled.kind == Kind::Logo));
    assert_eq!(art::saved_game(&fixture.paths, "rimworld").unwrap(), None);
    assert!(!fixture.art_file("logo.png").exists());
    assert!(
        art_notes(&report)
            .iter()
            .any(|note| note.starts_with("Couldn't save the SteamGridDB game")),
        "{:?}",
        report.notes
    );
}

#[test]
fn an_environment_replaced_during_the_steamgriddb_download_is_left_alone() {
    let fixture = Fixture::new();
    art::save_game(&fixture.paths, "rimworld", 7).unwrap();
    let server = Scripted::start(gog(None, art_body(&["logo"])));
    let inner = images();
    serve(&inner, Kind::Logo, 41, PNG);
    let transport = Hooked {
        inner: &inner,
        when: "https://cdn.example/41".to_owned(),
        hook: Box::new(|| replace_environment(&fixture)),
    };
    let request = Request {
        name: "rimworld",
        gog_id: GAME,
        title: "RimWorld",
        kinds: &Kind::ALL,
        transport: &transport,
        steamgriddb_key: Some("sgdb-key"),
        language: "en",
    };
    let report = run(
        &fixture.paths,
        None,
        &request,
        Ok(client_at(&server.base)),
        &Token::new(),
    )
    .unwrap();
    assert_eq!(report.art.len(), 3, "{:?}", report.art);
    assert!(!fixture.art_file("logo.png").exists());
    assert_eq!(fixture.art_config().record(None, Kind::Logo), None);
    assert!(
        art_notes(&report)
            .iter()
            .any(|note| note.starts_with("Couldn't fetch logo artwork from SteamGridDB")),
        "{:?}",
        report.notes
    );
}

/// Cancels `token` as an image download starts, like Cancel pressed while
/// the bytes come in.
struct CancelingTransport<'a> {
    inner: FakeApi,
    token: &'a Token,
}

impl Transport for CancelingTransport<'_> {
    fn get(&self, url: &str) -> Result<Box<dyn std::io::Read>> {
        self.inner.get(url)
    }

    fn request_with_length(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> Result<(crate::download::Response, Option<u64>)> {
        self.token.cancel();
        self.inner.request_with_length(url, bearer)
    }
}

#[test]
fn a_cancel_before_starting_skips_everything_and_says_so() {
    let fixture = Fixture::new();
    fixture.login();
    let server = Scripted::start(gog(Some(120), art_body(&[])));
    let mut session = fixture.session(&server.base);
    let token = Token::new();
    token.cancel();
    let request = Request {
        name: "rimworld",
        gog_id: GAME,
        title: "RimWorld",
        kinds: &Kind::ALL,
        transport: &images(),
        steamgriddb_key: None,
        language: "en",
    };
    let report = run(
        &fixture.paths,
        Some(&mut session),
        &request,
        Ok(client_at(&server.base)),
        &token,
    )
    .unwrap();
    assert_eq!(
        report.notes,
        [
            "Skipped GOG playtime: canceled.",
            "Skipped GOG artwork: canceled."
        ]
    );
    assert_eq!(report.playtime, None);
    assert!(report.art.is_empty());
    assert!(fixture.imported().is_none());
    assert!(server.requests().is_empty(), "nothing was asked of GOG");
}

#[test]
fn a_cancel_during_a_download_keeps_what_was_done_and_writes_nothing_more() {
    let fixture = Fixture::new();
    fixture.login();
    let server = Scripted::start(gog(Some(120), art_body(&[])));
    let mut session = fixture.session(&server.base);
    let token = Token::new();
    let transport = CancelingTransport {
        inner: images(),
        token: &token,
    };
    let request = Request {
        name: "rimworld",
        gog_id: GAME,
        title: "RimWorld",
        kinds: &Kind::ALL,
        transport: &transport,
        steamgriddb_key: Some("key"),
        language: "en",
    };
    let report = run(
        &fixture.paths,
        Some(&mut session),
        &request,
        Ok(client_at(&server.base)),
        &token,
    )
    .unwrap();
    // Playtime came first and stays.
    assert_eq!(fixture.imported().unwrap().seconds, 7200);
    assert!(report.art.is_empty(), "{:?}", report.art);
    assert_eq!(report.notes.len(), Kind::ALL.len());
    assert!(
        report
            .notes
            .iter()
            .all(|note| note.starts_with("Skipped ") && note.ends_with(" artwork: canceled.")),
        "{:?}",
        report.notes
    );
    for kind in Kind::ALL {
        assert!(
            fixture.art_config().record(None, kind).is_none(),
            "{kind} was written"
        );
    }
}

fn request<'a>(
    transport: &'a dyn Transport,
    kinds: &'a [Kind],
    key: Option<&'a str>,
) -> Request<'a> {
    Request {
        name: "rimworld",
        gog_id: GAME,
        title: "RimWorld",
        kinds,
        transport,
        steamgriddb_key: key,
        language: "en",
    }
}

#[test]
fn a_cancel_while_gog_answers_with_playtime_writes_none() {
    let fixture = Fixture::new();
    fixture.login();
    let token = Token::new();
    let answering = token.clone();
    let inner = gog(Some(120), art_body(&[]));
    let server = Scripted::start(move |line| {
        if line.contains("/sessions") {
            // The cancel lands while the client waits for this answer.
            answering.cancel();
        }
        inner(line)
    });
    let mut session = fixture.session(&server.base);
    let report = run(
        &fixture.paths,
        Some(&mut session),
        &request(&images(), &Kind::ALL, None),
        Ok(client_at(&server.base)),
        &token,
    )
    .unwrap();
    assert!(
        server
            .requests()
            .iter()
            .any(|line| line.contains("/sessions"))
    );
    assert_eq!(report.playtime, None);
    assert!(fixture.imported().is_none(), "nothing was written");
    assert_eq!(
        report.notes,
        [
            "Skipped GOG playtime: canceled.",
            "Skipped GOG artwork: canceled."
        ]
    );
}

#[test]
fn a_cancel_while_waiting_for_the_config_lock_writes_no_playtime() {
    use std::sync::mpsc;
    let fixture = Fixture::new();
    fixture.login();
    let token = Token::new();
    let (locked_tx, locked) = mpsc::channel::<()>();
    let (release, release_rx) = mpsc::channel::<()>();
    let held = std::sync::Mutex::new(Some((locked_tx, release_rx)));
    let config = fixture.paths.environment_config("rimworld");
    let paths = fixture.paths.clone();
    let inner = gog(Some(120), art_body(&[]));
    let server = Scripted::start(move |line| {
        if line.contains("/sessions")
            && let Some((locked_tx, release_rx)) = held.lock().unwrap().take()
        {
            // Someone else takes the config lock before GOG answers, so the
            // import has to wait for it.
            let (paths, config) = (paths.clone(), config.clone());
            let (taken_tx, taken) = mpsc::channel();
            std::thread::spawn(move || {
                let _lock = Locks::new(&paths).config(&config).unwrap();
                taken_tx.send(()).unwrap();
                let _ = release_rx.recv();
            });
            taken.recv().unwrap();
            locked_tx.send(()).unwrap();
        }
        inner(line)
    });
    let report = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let mut session = fixture.session(&server.base);
            run(
                &fixture.paths,
                Some(&mut session),
                &request(&images(), &[], None),
                Ok(client_at(&server.base)),
                &token,
            )
        });
        locked
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("GOG was asked for playtime");
        // Give it time to reach the lock, then cancel while it waits.
        std::thread::sleep(std::time::Duration::from_millis(150));
        token.cancel();
        release.send(()).unwrap();
        worker.join().unwrap()
    })
    .unwrap();
    assert_eq!(report.playtime, None);
    assert!(fixture.imported().is_none(), "nothing was written");
    assert!(
        report
            .notes
            .contains(&"Skipped GOG playtime: canceled.".to_owned()),
        "{:?}",
        report.notes
    );
}
