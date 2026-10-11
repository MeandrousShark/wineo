use std::os::unix::fs::PermissionsExt;

use wineo_gog::ErrorKind;

use super::*;
use crate::gog::test_support::{FakeKeyring, client, setup, tokens};
use crate::key_store::NoKeyring;
use crate::steam::api::fixture::{Server, refused, reply};

const USER: u64 = 48_628_349_957_132_247;
const OTHER_USER: u64 = 77;
const CODE: &str = "CODESECRET0123456789";
const LOGGED_OUT: &str = "Log in to GOG again: wineo import gog --login.";

fn login_of(refresh: &str, user_id: u64) -> StoredLogin {
    StoredLogin {
        refresh_token: refresh.to_owned(),
        user_id: UserId::new(user_id),
    }
}

fn seed(paths: &Paths, keyring: &dyn Keyring, refresh: &str, user_id: u64) {
    save(paths, keyring, &login_of(refresh, user_id)).unwrap();
}

fn open<'a>(paths: &Paths, keyring: &'a dyn Keyring, server: &Server) -> Session<'a> {
    Session::open(paths, |_| None, keyring, client(server))
        .unwrap()
        .unwrap()
}

fn saved(paths: &Paths, keyring: &dyn Keyring) -> Option<StoredLogin> {
    saved_login(paths, keyring).unwrap()
}

fn invalid_grant() -> Vec<u8> {
    reply(
        400,
        r#"{"error":"invalid_grant","error_description":"The refresh token is invalid."}"#,
    )
}

/// The refresh token a token request line sent.
fn sent_refresh(line: &str) -> String {
    let url = reqwest::Url::parse(&format!(
        "http://x{}",
        line.split(' ').nth(1).unwrap_or_default()
    ))
    .unwrap();
    url.query_pairs()
        .find(|(name, _)| name == "refresh_token")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

#[test]
fn logging_in_saves_the_login_to_the_keyring() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    let server = Server::start(vec![tokens("ACCESS1", "REFRESH1", USER, 3600)]);
    let pasted = format!("https://embed.gog.com/on_login_success?origin=client&code={CODE}");
    let logged_in = login(&paths, &keyring, client(&server), &pasted).unwrap();
    assert_eq!(logged_in.user_id, UserId::new(USER));
    assert_eq!(logged_in.saved, Saved::Keyring);
    assert!(server.requests()[0].contains(&format!("code={CODE}")));

    let (login, source) = stored_login(&paths, |_| None, &keyring).unwrap().unwrap();
    assert_eq!(login, login_of("REFRESH1", USER));
    assert_eq!(source, KeySource::Keyring);
    assert_eq!(
        keyring.0.lock().unwrap().as_deref(),
        Some(r#"{"refresh_token":"REFRESH1","user_id":"48628349957132247"}"#)
    );
    assert!(!key_store::key_file(&KEY_SPEC, &paths).exists());

    // The login's own access token is used without renewing.
    let mut session = logged_in.session;
    assert_eq!(session.access_token().unwrap().expose(), "ACCESS1");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn without_a_keyring_the_login_is_a_private_file_and_logout_deletes_it() {
    let (_root, paths) = setup();
    let server = Server::start(vec![tokens("A", "REFRESH1", USER, 3600)]);
    let logged_in = login(&paths, &NoKeyring, client(&server), CODE).unwrap();
    let file = key_store::key_file(&KEY_SPEC, &paths);
    assert_eq!(logged_in.saved, Saved::File(file.clone()));
    assert_eq!(file.file_name().unwrap(), "gog.key");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let (login, source) = stored_login(&paths, |_| None, &NoKeyring).unwrap().unwrap();
    assert_eq!(
        (login, source),
        (login_of("REFRESH1", USER), KeySource::File)
    );

    let cache = paths.gog_owned_cache(USER);
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    fs::write(&cache, "{}").unwrap();
    let other_cache = paths.gog_owned_cache(OTHER_USER);
    fs::write(&other_cache, "{}").unwrap();

    let out = logout(&paths, &NoKeyring).unwrap();
    assert_eq!(out.user_id, Some(UserId::new(USER)));
    assert!(out.removed.file && out.cache_removed);
    assert!(!file.exists());
    assert!(!cache.exists());
    assert!(other_cache.exists(), "only that account's cache goes");
    assert_eq!(stored_login(&paths, |_| None, &NoKeyring).unwrap(), None);

    // Logging out again finds nothing to remove.
    let again = logout(&paths, &NoKeyring).unwrap();
    assert_eq!((again.user_id, again.cache_removed), (None, false));
}

#[test]
fn the_variable_overrides_the_saved_login() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "SAVED", USER);
    let value = r#"{"refresh_token":"FROMENV","user_id":"5"}"#;
    let lookup = |name: &str| (name == LOGIN_VAR).then(|| OsString::from(value));
    let (login, source) = stored_login(&paths, lookup, &keyring).unwrap().unwrap();
    assert_eq!(
        (login, source),
        (login_of("FROMENV", 5), KeySource::Variable)
    );
}

#[test]
fn a_bad_paste_or_a_rejected_code_saves_nothing() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    let server = Server::start(vec![invalid_grant()]);
    let error = login(
        &paths,
        &keyring,
        client(&server),
        "https://example.com/SECRET",
    )
    .unwrap_err();
    assert!(matches!(error, AccountError::Paste(_)), "{error:?}");
    assert!(!format!("{error} {error:?}").contains("SECRET"));
    assert!(server.requests().is_empty());

    let error = login(&paths, &keyring, client(&server), CODE).unwrap_err();
    assert!(matches!(error, AccountError::CodeRejected), "{error:?}");
    assert!(!format!("{error} {error:?}").contains(CODE));
    assert_eq!(saved(&paths, &keyring), None);
}

#[test]
fn a_renewal_saves_the_new_refresh_token() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "REFRESH1", USER);
    let server = Server::start(vec![tokens("ACCESS2", "REFRESH2", USER, 3600)]);
    let mut session = open(&paths, &keyring, &server);
    assert_eq!(session.user_id(), UserId::new(USER));
    assert_eq!(session.access_token().unwrap().expose(), "ACCESS2");
    assert_eq!(sent_refresh(&server.requests()[0]), "REFRESH1");
    assert!(server.requests()[0].contains("grant_type=refresh_token"));
    assert_eq!(saved(&paths, &keyring), Some(login_of("REFRESH2", USER)));

    // Still fresh, so GOG isn't asked again.
    assert_eq!(session.access_token().unwrap().expose(), "ACCESS2");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_token_within_a_minute_of_expiring_is_renewed() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    let server = Server::start(vec![
        tokens("A2", "R2", USER, 60),
        tokens("A3", "R3", USER, 3600),
    ]);
    let mut session = open(&paths, &keyring, &server);
    assert_eq!(session.access_token().unwrap().expose(), "A2");
    assert_eq!(session.access_token().unwrap().expose(), "A3");
    let sent: Vec<_> = server.requests().iter().map(|l| sent_refresh(l)).collect();
    assert_eq!(sent, ["R1", "R2"]);
    assert_eq!(saved(&paths, &keyring), Some(login_of("R3", USER)));
}

#[test]
fn invalid_grant_or_401_clears_the_login() {
    for answer in [invalid_grant(), reply(401, "{}")] {
        let (_root, paths) = setup();
        let keyring = FakeKeyring::default();
        seed(&paths, &keyring, "R1", USER);
        let server = Server::start(vec![answer]);
        let error = open(&paths, &keyring, &server).access_token().unwrap_err();
        assert!(matches!(error, AccountError::LoggedOut), "{error:?}");
        assert_eq!(error.to_string(), LOGGED_OUT);
        assert!(!error.is_temporary());
        assert_eq!(saved(&paths, &keyring), None);
    }
}

#[test]
fn temporary_failures_keep_the_login() {
    let answers = [
        (reply(500, "{}"), ErrorKind::Server(500)),
        (reply(503, "{}"), ErrorKind::Server(503)),
        (reply(429, "{}"), ErrorKind::RateLimited),
    ];
    for (answer, kind) in answers {
        let (_root, paths) = setup();
        let keyring = FakeKeyring::default();
        seed(&paths, &keyring, "R1", USER);
        let server = Server::start(vec![answer]);
        let error = open(&paths, &keyring, &server).access_token().unwrap_err();
        match &error {
            AccountError::Temporary(inner) => assert_eq!(inner.kind(), kind),
            other => panic!("{other:?}"),
        }
        assert!(error.is_temporary());
        assert_eq!(saved(&paths, &keyring), Some(login_of("R1", USER)));
    }

    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    let unreachable = wineo_gog::Client::with_base("wineo-test", &refused()).unwrap();
    let mut session = Session::open(&paths, |_| None, &keyring, unreachable)
        .unwrap()
        .unwrap();
    assert!(session.access_token().unwrap_err().is_temporary());
    assert_eq!(saved(&paths, &keyring), Some(login_of("R1", USER)));
}

#[test]
fn a_malformed_answer_keeps_the_login() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    let server = Server::start(vec![reply(200, r#"{"access_token":"x"}"#)]);
    let error = open(&paths, &keyring, &server).access_token().unwrap_err();
    assert!(matches!(error, AccountError::Gog(_)), "{error:?}");
    assert_eq!(saved(&paths, &keyring), Some(login_of("R1", USER)));
}

#[test]
fn a_stale_renewal_never_overwrites_a_newer_login() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "OLD", USER);
    let server = Server::start(vec![
        tokens("STALEACCESS", "STALE", USER, 3600),
        tokens("NEWACCESS", "NEWER2", OTHER_USER, 3600),
    ]);
    let mut session = open(&paths, &keyring, &server);
    // Another process logs in to another account before this one renews.
    seed(&paths, &keyring, "NEWER", OTHER_USER);

    assert_eq!(session.access_token().unwrap().expose(), "NEWACCESS");
    let sent: Vec<_> = server.requests().iter().map(|l| sent_refresh(l)).collect();
    assert_eq!(sent, ["OLD", "NEWER"]);
    assert_eq!(
        saved(&paths, &keyring),
        Some(login_of("NEWER2", OTHER_USER))
    );
    assert_eq!(session.user_id(), UserId::new(OTHER_USER));
}

#[test]
fn a_stale_invalid_grant_never_clears_a_newer_login() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "OLD", USER);
    let server = Server::start(vec![invalid_grant(), tokens("A", "NEWER2", USER, 3600)]);
    let mut session = open(&paths, &keyring, &server);
    // Another process renewed first, and GOG retired the old token.
    seed(&paths, &keyring, "NEWER", USER);
    assert_eq!(session.access_token().unwrap().expose(), "A");
    assert_eq!(saved(&paths, &keyring), Some(login_of("NEWER2", USER)));
}

#[test]
fn a_renewal_racing_a_logout_brings_nothing_back() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    let server = Server::start(vec![tokens("A", "R2", USER, 3600)]);
    let mut session = open(&paths, &keyring, &server);
    logout(&paths, &keyring).unwrap();

    let error = session.access_token().unwrap_err();
    assert!(matches!(error, AccountError::LoggedOut), "{error:?}");
    assert_eq!(saved(&paths, &keyring), None);
    assert!(!key_store::key_file(&KEY_SPEC, &paths).exists());
    // The renewal's answer isn't usable either.
    assert!(session.access.is_none());
}

#[test]
fn concurrent_renewals_leave_one_consistent_login() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    let server = Server::start(vec![
        tokens("A2", "R2", USER, 3600),
        tokens("A3", "R3", USER, 3600),
        tokens("A4", "R4", USER, 3600),
        tokens("A5", "R5", USER, 3600),
    ]);
    let barrier = std::sync::Barrier::new(2);
    let logins: Vec<StoredLogin> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let (paths, keyring, server, barrier) = (&paths, &keyring, &server, &barrier);
                scope.spawn(move || {
                    let mut session = open(paths, keyring, server);
                    barrier.wait();
                    session.access_token().unwrap();
                    session.login.clone()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let stored = saved(&paths, &keyring).unwrap();
    assert!(logins.contains(&stored), "{stored:?} not in {logins:?}");
    assert_ne!(stored.refresh_token, "R1");
}

#[test]
fn a_login_from_the_variable_is_renewed_but_never_saved_or_cleared() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "SAVED", USER);
    let value = r#"{"refresh_token":"FROMENV","user_id":"5"}"#;
    let lookup = |name: &str| (name == LOGIN_VAR).then(|| OsString::from(value));

    let server = Server::start(vec![tokens("A", "ENV2", 5, 3600)]);
    let mut session = Session::open(&paths, lookup, &keyring, client(&server))
        .unwrap()
        .unwrap();
    assert_eq!(session.source(), KeySource::Variable);
    assert_eq!(session.access_token().unwrap().expose(), "A");
    assert_eq!(sent_refresh(&server.requests()[0]), "FROMENV");
    assert_eq!(saved(&paths, &keyring), Some(login_of("SAVED", USER)));

    let server = Server::start(vec![invalid_grant()]);
    let mut session = Session::open(&paths, lookup, &keyring, client(&server))
        .unwrap()
        .unwrap();
    assert!(matches!(
        session.access_token().unwrap_err(),
        AccountError::LoggedOut
    ));
    assert_eq!(saved(&paths, &keyring), Some(login_of("SAVED", USER)));
}

#[test]
fn an_unreadable_login_is_never_shown_and_logout_still_removes_it() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    *keyring.0.lock().unwrap() = Some("SECRETJUNK not json".to_owned());
    let error = stored_login(&paths, |_| None, &keyring).unwrap_err();
    assert!(matches!(error, AccountError::Unreadable));
    assert!(!format!("{error} {error:?}").contains("SECRETJUNK"));
    let error =
        Session::open(&paths, |_| None, &keyring, client(&Server::start(vec![]))).unwrap_err();
    assert!(matches!(error, AccountError::Unreadable));

    let out = logout(&paths, &keyring).unwrap();
    assert_eq!(out.user_id, None);
    assert!(out.removed.keyring);
    assert_eq!(*keyring.0.lock().unwrap(), None);
}

#[test]
fn no_secret_reaches_debug_or_any_error() {
    let login = login_of("REFRESHSECRET", USER);
    assert!(!format!("{login:?}").contains("REFRESHSECRET"));

    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "REFRESHSECRET", USER);
    let server = Server::start(vec![tokens("ACCESSSECRET", "REFRESHSECRET2", USER, 3600)]);
    let mut session = open(&paths, &keyring, &server);
    session.access_token().unwrap();
    let shown = format!("{session:?}");
    assert!(!shown.contains("SECRET"), "{shown}");

    let answers = [
        invalid_grant(),
        reply(401, "REFRESHSECRET"),
        reply(429, "REFRESHSECRET"),
        reply(500, "REFRESHSECRET"),
        reply(200, "{\"echo\":\"REFRESHSECRET\""),
    ];
    for answer in answers {
        let (_root, paths) = setup();
        let keyring = FakeKeyring::default();
        seed(&paths, &keyring, "REFRESHSECRET", USER);
        let server = Server::start(vec![answer]);
        let error = open(&paths, &keyring, &server).access_token().unwrap_err();
        let shown = format!("{error} {error:?} {error:#?}");
        for secret in ["SECRET", wineo_gog::CLIENT_ID, "127.0.0.1", "token="] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
    }
}
