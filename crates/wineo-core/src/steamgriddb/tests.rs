use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::download::Response;

/// A keyring in memory, or one that is unavailable or broken.
struct FakeKeyring {
    key: RefCell<Option<String>>,
    error: Option<KeyringError>,
}

impl FakeKeyring {
    fn with(key: Option<&str>) -> Self {
        Self {
            key: RefCell::new(key.map(str::to_owned)),
            error: None,
        }
    }

    fn failing(error: KeyringError) -> Self {
        Self {
            key: RefCell::new(None),
            error: Some(error),
        }
    }

    fn check(&self) -> Result<(), KeyringError> {
        self.error.clone().map_or(Ok(()), Err)
    }
}

impl Keyring for FakeKeyring {
    fn get(&self) -> Result<Option<String>, KeyringError> {
        self.check()?;
        Ok(self.key.borrow().clone())
    }

    fn set(&self, key: &str) -> Result<(), KeyringError> {
        self.check()?;
        *self.key.borrow_mut() = Some(key.to_owned());
        Ok(())
    }

    fn delete(&self) -> Result<bool, KeyringError> {
        self.check()?;
        Ok(self.key.borrow_mut().take().is_some())
    }
}

fn setup() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&root.path().join("usb")),
    )
    .unwrap();
    (root, paths)
}

fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = pairs
        .iter()
        .map(|(key, value)| (key.to_string(), OsString::from(value)))
        .collect();
    move |key| map.get(key).cloned()
}

fn found(paths: &Paths, env: &[(&str, &str)], keyring: &dyn Keyring) -> Option<Key> {
    find_key(paths, vars(env), keyring).unwrap()
}

#[test]
fn key_debug_redacts_the_value() {
    let key = Key {
        value: "private-key-value".to_owned(),
        source: KeySource::Keyring,
    };

    let debug = format!("{key:?}");
    assert!(!debug.contains(&key.value));
    assert!(debug.contains("<redacted>"));
    assert!(debug.contains("Keyring"));
}

#[test]
fn the_key_file_is_on_the_host_even_in_a_portable_folder() {
    let (root, paths) = setup();
    assert!(paths.portable_root().is_some());
    assert_eq!(
        key_file(&paths),
        root.path().join(".local/share/wineo/steamgriddb.key")
    );
}

#[test]
fn lookup_tries_the_variable_then_the_keyring_then_the_file() {
    let (_root, paths) = setup();
    let file = key_file(&paths);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "from-file\n").unwrap();
    let keyring = FakeKeyring::with(Some("from-keyring"));
    let key = |value: &str, source| {
        Some(Key {
            value: value.to_owned(),
            source,
        })
    };

    assert_eq!(
        found(&paths, &[(KEY_VAR, " from-var ")], &keyring),
        key("from-var", KeySource::Variable)
    );
    assert_eq!(
        found(&paths, &[(KEY_VAR, "")], &keyring),
        key("from-keyring", KeySource::Keyring)
    );
    assert_eq!(
        found(&paths, &[], &FakeKeyring::with(None)),
        key("from-file", KeySource::File)
    );
    assert_eq!(
        found(
            &paths,
            &[],
            &FakeKeyring::failing(KeyringError::Unavailable)
        ),
        key("from-file", KeySource::File)
    );
    let err = find_key(
        &paths,
        vars(&[]),
        &FakeKeyring::failing(KeyringError::Failed("locked".into())),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Keyring(_)), "{err}");
}

#[test]
fn a_missing_key_says_how_to_add_one() {
    let (_root, paths) = setup();
    let file = key_file(&paths);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "\n").unwrap();
    assert_eq!(found(&paths, &[], &FakeKeyring::with(Some(" "))), None);
    let err = require_key(&paths, vars(&[]), &NoKeyring).unwrap_err();
    assert_eq!(
        err.to_string(),
        "no SteamGridDB key, run `wineo art key` or set WINEO_STEAMGRIDDB_KEY"
    );
}

#[test]
fn saving_prefers_the_keyring_and_drops_a_stale_file() {
    let (_root, paths) = setup();
    let file = key_file(&paths);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "old\n").unwrap();
    let keyring = FakeKeyring::with(None);
    assert_eq!(save_key(&paths, &keyring, "new").unwrap(), Saved::Keyring);
    assert_eq!(keyring.key.borrow().as_deref(), Some("new"));
    assert!(!file.exists());

    let err = save_key(
        &paths,
        &FakeKeyring::failing(KeyringError::Failed("denied".into())),
        "new",
    )
    .unwrap_err();
    assert!(matches!(err, Error::Keyring(_)), "{err}");
    assert!(!file.exists());
}

#[test]
fn without_a_keyring_the_key_file_is_private() {
    let (_root, paths) = setup();
    let file = key_file(&paths);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "old\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        save_key(&paths, &NoKeyring, "abc123").unwrap(),
        Saved::File(file.clone())
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "abc123\n");
    let mode = fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
    assert_eq!(
        found(&paths, &[], &NoKeyring).unwrap().value,
        "abc123".to_owned()
    );
}

#[test]
fn removal_deletes_from_both() {
    let (_root, paths) = setup();
    save_key(&paths, &NoKeyring, "abc").unwrap();
    let keyring = FakeKeyring::with(Some("abc"));
    assert_eq!(
        remove_key(&paths, &keyring).unwrap(),
        Removed {
            keyring: true,
            file: true
        }
    );
    assert_eq!(
        remove_key(&paths, &keyring).unwrap(),
        Removed {
            keyring: false,
            file: false
        }
    );
    assert_eq!(
        remove_key(&paths, &NoKeyring).unwrap(),
        Removed {
            keyring: false,
            file: false
        }
    );
}

struct Answer {
    status: u16,
    seen: RefCell<Vec<(String, String)>>,
}

impl Transport for Answer {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn Read>> {
        bail!("unexpected unauthenticated request to {url}")
    }

    fn request(&self, url: &str, bearer: Option<&str>) -> anyhow::Result<Response> {
        self.seen
            .borrow_mut()
            .push((url.to_owned(), bearer.unwrap_or_default().to_owned()));
        Ok(Response {
            status: self.status,
            body: Box::new(io::empty()),
        })
    }
}

#[test]
fn a_key_is_checked_with_one_authenticated_request() {
    let answer = |status| Answer {
        status,
        seen: RefCell::new(Vec::new()),
    };
    let ok = answer(200);
    assert!(check_key(&ok, "abc").unwrap());
    assert_eq!(
        ok.seen.into_inner(),
        [(format!("{API}/search/autocomplete/wineo"), "abc".to_owned())]
    );
    assert!(!check_key(&answer(401), "abc").unwrap());
    assert!(!check_key(&answer(403), "abc").unwrap());
    let err = check_key(&answer(500), "abc").unwrap_err();
    assert_eq!(err.to_string(), "SteamGridDB answered with status 500");
}

use super::fake::FakeApi;
use serde_json::json;

fn image_json(id: u64) -> serde_json::Value {
    json!({ "id": id, "url": format!("https://cdn.example/{id}.png") })
}

#[test]
fn search_is_lenient_and_keeps_the_year_when_there_is_one() {
    let api = FakeApi::default();
    api.json(
        format!("{API}/search/autocomplete/Hades%20II"),
        json!([
            { "id": 1, "name": "Hades II", "release_date": 1_714_521_600, "verified": true },
            { "id": 2, "name": "Hades II Demo" },
            { "id": 3, "name": "No date", "release_date": null, "types": ["steam"] },
            { "name": "no id" },
            { "id": 4 },
        ]),
    );
    let games = Api::new(&api, "k").search("Hades II").unwrap();
    assert_eq!(
        games,
        [
            Game {
                id: 1,
                name: "Hades II".into(),
                year: Some(2024)
            },
            Game {
                id: 2,
                name: "Hades II Demo".into(),
                year: None
            },
            Game {
                id: 3,
                name: "No date".into(),
                year: None
            },
        ]
    );
    assert_eq!(api.seen.borrow()[0].1.as_deref(), Some("k"));
}

#[test]
fn game_looks_up_a_game_by_id() {
    let api = FakeApi::default();
    api.json(
        format!("{API}/games/id/1"),
        json!({
            "id": 1,
            "name": "Hades II",
            "release_date": 1_714_521_600,
            "verified": true
        }),
    );

    assert_eq!(
        Api::new(&api, "k").game(1).unwrap(),
        Some(Game {
            id: 1,
            name: "Hades II".into(),
            year: Some(2024),
        })
    );
    assert_eq!(api.urls(), [format!("{API}/games/id/1")]);
    assert_eq!(api.seen.borrow()[0].1.as_deref(), Some("k"));
}

#[test]
fn game_returns_none_when_the_id_is_not_found() {
    let api = FakeApi::default();
    api.answer(format!("{API}/games/id/404"), 404, || Box::new(io::empty()));
    assert_eq!(Api::new(&api, "k").game(404).unwrap(), None);

    let api = FakeApi::default();
    api.json(format!("{API}/games/id/404"), Value::Null);
    assert_eq!(Api::new(&api, "k").game(404).unwrap(), None);
}

#[test]
fn game_reports_a_rejected_key() {
    let api = FakeApi::default();
    api.answer(format!("{API}/games/id/1"), 401, || Box::new(io::empty()));

    let err = Api::new(&api, "k").game(1).unwrap_err();
    assert_eq!(err.to_string(), "SteamGridDB rejected the key");
}

#[test]
fn steam_game_looks_up_a_game_by_steam_app_id() {
    let api = FakeApi::default();
    api.json(
        format!("{API}/games/steam/1086940"),
        json!({
            "id": 123,
            "name": "Baldur's Gate 3",
            "release_date": 1_660_780_800
        }),
    );

    assert_eq!(
        Api::new(&api, "k").steam_game(1086940).unwrap(),
        Some(Game {
            id: 123,
            name: "Baldur's Gate 3".into(),
            year: Some(2022),
        })
    );
    assert_eq!(api.urls(), [format!("{API}/games/steam/1086940")]);
    assert_eq!(api.seen.borrow()[0].1.as_deref(), Some("k"));
}

#[test]
fn every_request_carries_the_hard_filters() {
    let grid = Api::images_url(9, Kind::Grid, 2, Filters::Ranked);
    assert_eq!(
        grid,
        format!(
            "{API}/grids/game/9?types=static&nsfw=false&humor=false&dimensions=600x900,342x482,660x930&page=2"
        )
    );
    for (kind, path) in [
        (Kind::Icon, "icons"),
        (Kind::Hero, "heroes"),
        (Kind::Logo, "logos"),
    ] {
        assert_eq!(
            Api::images_url(9, kind, 0, Filters::Ranked),
            format!("{API}/{path}/game/9?types=static&nsfw=false&humor=false&page=0")
        );
    }
}

#[test]
fn an_unfiltered_request_leaves_nothing_out() {
    for (kind, path) in [
        (Kind::Icon, "icons"),
        (Kind::Grid, "grids"),
        (Kind::Hero, "heroes"),
        (Kind::Logo, "logos"),
    ] {
        assert_eq!(
            Api::images_url(9, kind, 3, Filters::None),
            format!(
                "{API}/{path}/game/9?types=static,animated&nsfw=any&humor=any&epilepsy=any&page=3"
            )
        );
    }
}

#[test]
fn pages_are_walked_until_one_is_empty() {
    let api = FakeApi::default();
    api.json(
        Api::images_url(9, Kind::Hero, 0, Filters::Ranked),
        json!([image_json(1), image_json(2)]),
    );
    api.json(
        Api::images_url(9, Kind::Hero, 1, Filters::Ranked),
        json!([image_json(2), image_json(3), { "id": 4 }]),
    );
    api.json(
        Api::images_url(9, Kind::Hero, 2, Filters::Ranked),
        json!([]),
    );
    let images = Api::new(&api, "k")
        .images(9, Kind::Hero, Filters::Ranked)
        .unwrap();
    let ids: Vec<u64> = images.iter().map(|image| image.id).collect();
    assert_eq!(ids, [1, 2, 3]);
    assert_eq!(api.urls().len(), 3);
}

/// Cancels `token` once the first page has been asked for.
struct CancelAfterFirstPage<'a> {
    inner: &'a FakeApi,
    token: &'a Token,
}

impl Transport for CancelAfterFirstPage<'_> {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn Read>> {
        self.inner.get(url)
    }

    fn request(&self, url: &str, bearer: Option<&str>) -> anyhow::Result<Response> {
        let response = self.inner.request(url, bearer);
        self.token.cancel();
        response
    }
}

#[test]
fn a_cancel_stops_paging_between_pages() {
    let api = FakeApi::default();
    for page in 0..3 {
        api.json(
            Api::images_url(9, Kind::Hero, page, Filters::Ranked),
            json!([image_json(u64::from(page) + 1)]),
        );
    }
    let token = Token::new();
    let transport = CancelAfterFirstPage {
        inner: &api,
        token: &token,
    };
    let error = Api::new(&transport, "k")
        .images_with_cancel(9, Kind::Hero, Filters::Ranked, &token)
        .unwrap_err();
    assert!(
        matches!(error.downcast_ref(), Some(crate::error::Error::Canceled(_))),
        "{error:#}"
    );
    assert_eq!(api.urls().len(), 1, "no page after the cancel");
}

#[test]
fn paging_stops_at_the_page_limit() {
    let api = FakeApi::default();
    for page in 0..MAX_PAGES + 2 {
        api.json(
            Api::images_url(9, Kind::Logo, page, Filters::Ranked),
            json!([image_json(u64::from(page))]),
        );
    }
    let images = Api::new(&api, "k")
        .images(9, Kind::Logo, Filters::Ranked)
        .unwrap();
    assert_eq!(images.len(), MAX_PAGES as usize);
    assert_eq!(api.urls().len(), MAX_PAGES as usize);
}

#[test]
fn a_rejected_key_and_other_failures_are_errors() {
    let api = FakeApi::default();
    api.answer(
        Api::images_url(9, Kind::Icon, 0, Filters::Ranked),
        401,
        || Box::new(io::empty()),
    );
    let err = Api::new(&api, "k")
        .images(9, Kind::Icon, Filters::Ranked)
        .unwrap_err();
    assert_eq!(err.to_string(), "SteamGridDB rejected the key");
    api.json(format!("{API}/search/autocomplete/x"), json!(null));
    assert!(Api::new(&api, "k").search("x").unwrap().is_empty());
    let text = json!({ "success": false, "errors": ["Game not found"] }).to_string();
    api.answer(
        Api::images_url(8, Kind::Icon, 0, Filters::Ranked),
        200,
        move || Box::new(io::Cursor::new(text.clone().into_bytes())),
    );
    let err = Api::new(&api, "k")
        .images(8, Kind::Icon, Filters::Ranked)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "SteamGridDB refused the request: Game not found"
    );
}

#[test]
fn downloads_send_no_key_and_are_capped() {
    let api = FakeApi::default();
    api.bytes("https://cdn.example/a.png", b"png");
    assert_eq!(
        Api::new(&api, "k")
            .download("https://cdn.example/a.png")
            .unwrap(),
        b"png"
    );
    assert_eq!(api.seen.borrow()[0].1, None);
    api.answer("https://cdn.example/big.png", 200, || {
        Box::new(io::repeat(0).take(MAX_BYTES + 1))
    });
    let err = Api::new(&api, "k")
        .download_with_progress("https://cdn.example/big.png", |_, _| {}, &Token::new())
        .unwrap_err();
    assert!(err.to_string().contains("larger than 32 MiB"), "{err}");
    api.answer(
        "https://cdn.example/gone.png",
        404,
        || Box::new(io::empty()),
    );
    assert!(
        Api::new(&api, "k")
            .download("https://cdn.example/gone.png")
            .is_err()
    );
}

#[test]
fn downloads_enforce_declared_and_actual_size_limits() {
    let api = FakeApi::default();
    api.bytes_with_length(
        "https://cdn.example/declared-big.png",
        b"small body",
        MAX_BYTES + 1,
    );
    let err = Api::new(&api, "k")
        .download("https://cdn.example/declared-big.png")
        .unwrap_err();
    assert!(err.to_string().contains("larger than 32 MiB"), "{err}");

    api.answer_with_length("https://cdn.example/lying.png", 200, Some(1), || {
        Box::new(io::repeat(0).take(MAX_BYTES + 1))
    });
    let err = Api::new(&api, "k")
        .download("https://cdn.example/lying.png")
        .unwrap_err();
    assert!(err.to_string().contains("larger than 32 MiB"), "{err}");

    api.answer_with_length(
        "https://cdn.example/exact.png",
        200,
        Some(MAX_BYTES),
        || Box::new(io::repeat(0).take(MAX_BYTES)),
    );
    let bytes = Api::new(&api, "k")
        .download("https://cdn.example/exact.png")
        .unwrap();
    assert_eq!(bytes.len() as u64, MAX_BYTES);
}

struct ReadCounter(std::rc::Rc<std::cell::Cell<usize>>);

impl Read for ReadCounter {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        self.0.set(self.0.get() + 1);
        Ok(0)
    }
}

#[test]
fn canceled_before_the_first_chunk_does_not_read_the_body() {
    let api = FakeApi::default();
    let token = Token::new();
    let read_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let answer_token = token.clone();
    let answer_count = read_count.clone();
    api.answer("https://cdn.example/early-cancel.png", 200, move || {
        answer_token.cancel();
        Box::new(ReadCounter(answer_count.clone()))
    });
    let err = Api::new(&api, "k")
        .download_with_progress("https://cdn.example/early-cancel.png", |_, _| {}, &token)
        .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<Error>(),
        Some(Error::Canceled("download"))
    ));
    assert_eq!(read_count.get(), 0);
}

struct ShortReader {
    body: io::Cursor<Vec<u8>>,
    limit: usize,
}

impl Read for ShortReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = buffer.len().min(self.limit);
        self.body.read(&mut buffer[..count])
    }
}

struct InterruptedOnce {
    body: io::Cursor<Vec<u8>>,
    first_read: bool,
}

impl Read for InterruptedOnce {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.first_read {
            self.first_read = false;
            return Err(io::ErrorKind::Interrupted.into());
        }
        self.body.read(buffer)
    }
}

struct CancelAtEof {
    body: io::Cursor<Vec<u8>>,
    token: Token,
}

impl Read for CancelAtEof {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.body.position() == self.body.get_ref().len() as u64 {
            self.token.cancel();
            return Ok(0);
        }
        self.body.read(buffer)
    }
}

#[test]
fn interrupted_reads_are_retried() {
    let api = FakeApi::default();
    api.answer("https://cdn.example/interrupted.png", 200, || {
        Box::new(InterruptedOnce {
            body: io::Cursor::new(b"after interrupt".to_vec()),
            first_read: true,
        })
    });
    assert_eq!(
        Api::new(&api, "k")
            .download("https://cdn.example/interrupted.png")
            .unwrap(),
        b"after interrupt"
    );
}

#[test]
fn cancellation_during_the_eof_read_is_reported() {
    let api = FakeApi::default();
    let token = Token::new();
    let reader_token = token.clone();
    api.answer("https://cdn.example/eof-cancel.png", 200, move || {
        Box::new(CancelAtEof {
            body: io::Cursor::new(b"image".to_vec()),
            token: reader_token.clone(),
        })
    });
    let err = Api::new(&api, "k")
        .download_with_progress("https://cdn.example/eof-cancel.png", |_, _| {}, &token)
        .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<Error>(),
        Some(Error::Canceled("download"))
    ));
}

#[test]
fn downloads_report_progress_and_cancel_between_chunks() {
    let api = FakeApi::default();
    api.bytes("https://cdn.example/progress.png", b"image bytes");
    let mut progress = Vec::new();
    assert_eq!(
        Api::new(&api, "k")
            .download_with_progress(
                "https://cdn.example/progress.png",
                |received, total| progress.push((received, total)),
                &Token::new(),
            )
            .unwrap(),
        b"image bytes"
    );
    assert_eq!(progress, [(11, Some(11))]);

    let token = Token::new();
    api.answer("https://cdn.example/cancel.png", 200, || {
        Box::new(ShortReader {
            body: io::Cursor::new(b"first chunk, then more".to_vec()),
            limit: 5,
        })
    });
    let mut canceled_progress = Vec::new();
    let error = Api::new(&api, "k")
        .download_with_progress(
            "https://cdn.example/cancel.png",
            |received, total| {
                canceled_progress.push((received, total));
                token.cancel();
            },
            &token,
        )
        .unwrap_err();
    assert_eq!(canceled_progress, [(5, None)]);
    assert!(matches!(
        error.downcast_ref::<crate::error::Error>(),
        Some(crate::error::Error::Canceled("download"))
    ));
}

#[test]
fn image_fields_are_parsed_leniently() {
    let parsed = image(&json!({
        "id": 5, "url": "https://cdn.example/x.ICO?v=1", "style": ["official"],
        "thumb": "https://cdn.example/x-thumb.png",
        "author": {"name": "Ada"}, "nsfw": true, "humor": true, "type": "animated",
        "language": "DE", "width": 256, "height": 128, "score": -2, "mime": null,
    }))
    .unwrap();
    assert_eq!(
        parsed,
        Image {
            id: 5,
            url: "https://cdn.example/x.ICO?v=1".into(),
            thumb: Some("https://cdn.example/x-thumb.png".into()),
            author: Some("Ada".into()),
            style: None,
            nsfw: true,
            humor: true,
            animated: true,
            score: Some(-2),
            official: true,
            language: Some("de".into()),
            width: Some(256),
            height: Some(128),
            format: Some(Format::Ico),
        }
    );
    let bare = image(&json!({ "id": 6, "url": "https://cdn.example/y", "mime": "image/webp", "style": "custom" })).unwrap();
    assert_eq!(
        (
            bare.format,
            bare.official,
            bare.score,
            bare.thumb,
            bare.author,
            bare.style,
            bare.nsfw,
            bare.humor,
            bare.animated,
        ),
        (
            Some(Format::Webp),
            false,
            None,
            None,
            None,
            Some("custom".into()),
            false,
            false,
            false,
        )
    );
    let animated_mime = image(&json!({
        "id": 7, "url": "https://cdn.example/animated", "mime": "image/gif"
    }))
    .unwrap();
    assert!(animated_mime.animated);
    assert!(image(&json!({ "id": "7", "url": "u" })).is_none());
    assert!(image(&json!({ "id": 7 })).is_none());
}

#[test]
fn language_comes_from_the_first_set_variable() {
    let lang = |pairs: &[(&str, &str)]| language(vars(pairs));
    assert_eq!(lang(&[("LANG", "de_DE.UTF-8")]), "de");
    assert_eq!(lang(&[("LC_MESSAGES", "fr_FR"), ("LANG", "de_DE")]), "fr");
    assert_eq!(
        lang(&[("LC_ALL", "ja_JP.UTF-8"), ("LC_MESSAGES", "fr_FR")]),
        "ja"
    );
    assert_eq!(lang(&[("LC_ALL", ""), ("LANG", "pt_BR")]), "pt");
    assert_eq!(lang(&[("LANG", "C.UTF-8")]), "en");
    assert_eq!(lang(&[("LC_ALL", "POSIX"), ("LANG", "de_DE")]), "en");
    assert_eq!(lang(&[]), "en");
}

fn candidate(id: u64) -> Image {
    Image {
        id,
        url: format!("https://cdn.example/{id}"),
        ..Image::default()
    }
}

fn ranked(kind: Kind, images: Vec<Image>, language: &str) -> Vec<u64> {
    let mut images = images;
    rank(kind, &mut images, language);
    images.iter().map(|image| image.id).collect()
}

fn sized(mut image: Image, width: u64, height: u64) -> Image {
    image.width = Some(width);
    image.height = Some(height);
    image
}

#[test]
fn icons_rank_by_floor_style_language_format_score_size_then_id() {
    let official = |mut image: Image| {
        image.official = true;
        image
    };
    let lang = |mut image: Image, code: &str| {
        image.language = Some(code.into());
        image
    };
    let format = |mut image: Image, format| {
        image.format = Some(format);
        image
    };
    let score = |mut image: Image, score| {
        image.score = Some(score);
        image
    };
    let base = |id| sized(candidate(id), 256, 256);
    let images = vec![
        // Too small, even though it wins every later key.
        official(format(
            lang(score(sized(candidate(1), 512, 64), 99), "de"),
            Format::Png,
        )),
        base(2),
        official(base(3)),
        official(lang(base(4), "en")),
        official(lang(base(5), "de")),
        official(format(lang(base(6), "de"), Format::Ico)),
        official(format(lang(base(7), "de"), Format::Png)),
        official(score(format(lang(base(8), "de"), Format::Png), 5)),
        official(score(
            format(lang(sized(candidate(9), 512, 512), "de"), Format::Png),
            5,
        )),
        official(score(
            format(lang(sized(candidate(10), 512, 512), "de"), Format::Png),
            5,
        )),
    ];
    assert_eq!(
        ranked(Kind::Icon, images, "de"),
        [9, 10, 8, 7, 6, 5, 4, 3, 2, 1]
    );
}

#[test]
fn grids_heroes_and_logos_rank_by_their_own_keys() {
    let with = |id, language: Option<&str>, score: Option<i64>, size: Option<(u64, u64)>| Image {
        language: language.map(str::to_owned),
        score,
        width: size.map(|(width, _)| width),
        height: size.map(|(_, height)| height),
        ..candidate(id)
    };
    let grids = vec![
        with(1, None, Some(50), Some((660, 930))),
        with(2, Some("en"), Some(1), None),
        with(3, Some("fr"), None, None),
        with(4, Some("fr"), Some(0), Some((600, 900))),
        with(5, Some("fr"), Some(0), Some((342, 482))),
        with(6, Some("fr"), Some(0), Some((342, 482))),
    ];
    assert_eq!(ranked(Kind::Grid, grids.clone(), "fr"), [4, 5, 6, 3, 2, 1]);
    // An English user ranks `en` first and everything else together.
    assert_eq!(ranked(Kind::Grid, grids.clone(), "en"), [2, 1, 4, 5, 6, 3]);
    // Heroes ignore language entirely.
    assert_eq!(ranked(Kind::Hero, grids, "fr"), [1, 2, 4, 5, 6, 3]);

    let logo = |id, official: bool, language: &str, format: Option<Format>, score: i64| Image {
        official,
        language: Some(language.to_owned()),
        format,
        score: Some(score),
        ..candidate(id)
    };
    let logos = vec![
        logo(1, false, "de", Some(Format::Png), 99),
        logo(2, true, "en", Some(Format::Png), 99),
        logo(3, true, "de", None, 99),
        logo(4, true, "de", Some(Format::Webp), 0),
        logo(5, true, "de", Some(Format::Png), 0),
        logo(6, true, "de", Some(Format::Png), 1),
    ];
    assert_eq!(ranked(Kind::Logo, logos, "de"), [6, 5, 4, 3, 2, 1]);
}

fn scored(mut image: Image, score: i64) -> Image {
    image.score = Some(score);
    image
}

#[test]
fn oversized_logos_lose_to_any_normal_one() {
    let images = vec![
        scored(sized(candidate(1), 8192, 4482), 50),
        sized(candidate(2), 1600, 876),
    ];
    assert_eq!(ranked(Kind::Logo, images, "en"), [2, 1]);
}

#[test]
fn oversized_only_lists_still_yield_the_best_oversized() {
    let logos = vec![
        sized(candidate(1), 8192, 4482),
        scored(sized(candidate(2), 8192, 4482), 3),
    ];
    assert_eq!(ranked(Kind::Logo, logos, "en"), [2, 1]);
    let heroes = vec![
        sized(candidate(1), 5000, 1600),
        scored(sized(candidate(2), 6000, 2000), 3),
    ];
    assert_eq!(ranked(Kind::Hero, heroes, "en"), [2, 1]);
}

#[test]
fn oversized_is_judged_on_the_long_side_only_when_both_sizes_are_known() {
    let heroes = vec![
        sized(candidate(1), 4097, 100),
        sized(candidate(2), 4096, 4096),
        Image {
            width: Some(9000),
            ..candidate(3)
        },
    ];
    // 4096 is not oversized, and a missing height means unknown.
    assert_eq!(ranked(Kind::Hero, heroes, "en"), [2, 3, 1]);
}

#[test]
fn big_enough_logos_rank_by_score_not_size() {
    let images = vec![
        scored(sized(candidate(1), 3000, 1000), 1),
        scored(sized(candidate(2), 1200, 400), 5),
    ];
    assert_eq!(ranked(Kind::Logo, images, "en"), [2, 1]);
}

#[test]
fn big_enough_logos_beat_smaller_higher_scored_ones() {
    let images = vec![
        scored(sized(candidate(1), 1199, 600), 99),
        scored(sized(candidate(2), 1200, 100), 0),
    ];
    assert_eq!(ranked(Kind::Logo, images, "en"), [2, 1]);
}

#[test]
fn big_enough_logos_with_equal_scores_tie_on_id() {
    let images = vec![
        sized(candidate(3), 3000, 1000),
        sized(candidate(2), 1200, 400),
    ];
    assert_eq!(ranked(Kind::Logo, images, "en"), [2, 3]);
}

#[test]
fn too_small_logos_still_prefer_more_pixels_after_score() {
    let images = vec![
        sized(candidate(1), 400, 100),
        sized(candidate(2), 800, 200),
        candidate(3),
    ];
    assert_eq!(ranked(Kind::Logo, images, "en"), [2, 1, 3]);
}

#[test]
fn heroes_rank_big_enough_by_score_over_bigger() {
    let images = vec![
        scored(sized(candidate(1), 3840, 1240), 2),
        scored(sized(candidate(2), 1920, 620), 5),
        scored(sized(candidate(3), 1919, 620), 99),
    ];
    assert_eq!(ranked(Kind::Hero, images, "en"), [2, 1, 3]);
}

#[test]
fn unknown_dimensions_are_neither_oversized_nor_big_enough() {
    let images = vec![
        scored(sized(candidate(1), 8192, 4482), 99),
        scored(candidate(2), 99),
        scored(sized(candidate(3), 1600, 876), 1),
    ];
    // Known and big enough, then unknown, then oversized.
    assert_eq!(ranked(Kind::Logo, images.clone(), "en"), [3, 2, 1]);
    let heroes = vec![
        scored(sized(candidate(1), 8192, 4482), 99),
        scored(candidate(2), 99),
        scored(sized(candidate(3), 1920, 620), 1),
    ];
    assert_eq!(ranked(Kind::Hero, heroes, "en"), [3, 2, 1]);
}

#[test]
fn a_known_width_is_big_enough_without_a_height() {
    let mut wide = scored(candidate(1), 1);
    wide.width = Some(1600);
    let images = vec![wide, scored(sized(candidate(2), 800, 300), 99)];
    assert_eq!(ranked(Kind::Logo, images, "en"), [1, 2]);
}

#[test]
fn huge_reported_sizes_rank_without_overflowing() {
    let images = vec![
        sized(candidate(1), u64::MAX, u64::MAX),
        sized(candidate(2), 512, 512),
    ];
    assert_eq!(ranked(Kind::Icon, images, "en"), [1, 2]);
}

#[test]
fn icons_and_grids_still_prefer_the_biggest_regardless_of_size() {
    let images = vec![
        sized(candidate(1), 512, 512),
        sized(candidate(2), 8192, 8192),
        sized(candidate(3), 1024, 1024),
    ];
    assert_eq!(ranked(Kind::Icon, images.clone(), "en"), [2, 3, 1]);
    assert_eq!(ranked(Kind::Grid, images, "en"), [2, 3, 1]);
}
