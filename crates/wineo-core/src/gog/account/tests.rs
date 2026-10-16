use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use wineo_gog::ErrorKind;

use super::*;
use crate::gog::test_support::{
    FakeKeyring, Scripted, client, client_at, client_with_profiles, setup, tokens,
};
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
        username: None,
    }
}

fn seed(paths: &Paths, keyring: &dyn Keyring, refresh: &str, user_id: u64) {
    save(paths, keyring, &login_of(refresh, user_id)).unwrap();
}

fn open<'a>(paths: &Paths, keyring: &'a dyn Keyring, server: &Server) -> Session<'a> {
    Session::open(paths, keyring, client(server))
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

    let (login, source) = stored_login(&paths, &keyring).unwrap().unwrap();
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
    let (login, source) = stored_login(&paths, &NoKeyring).unwrap().unwrap();
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
    assert_eq!(stored_login(&paths, &NoKeyring).unwrap(), None);

    // Logging out again finds nothing to remove.
    let again = logout(&paths, &NoKeyring).unwrap();
    assert_eq!((again.user_id, again.cache_removed), (None, false));
}

#[test]
fn no_variable_overrides_the_saved_login() {
    assert_eq!(KEY_SPEC.env_var, None);
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "SAVED", USER);
    let value = r#"{"refresh_token":"FROMENV","user_id":"5"}"#;
    // Whatever variable is set, the saved login is the one found.
    let key = key_store::find_key(&KEY_SPEC, &paths, |_| Some(value.into()), &keyring)
        .unwrap()
        .unwrap();
    assert_eq!(key.source, KeySource::Keyring);
    assert_eq!(
        StoredLogin::from_secret(&key.value).unwrap(),
        login_of("SAVED", USER)
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
    let mut session = Session::open(&paths, &keyring, unreachable)
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

/// Saves a login the way another process's login or renewal would, bumping
/// the generation.
fn replace(paths: &Paths, keyring: &dyn Keyring, refresh: &str, user_id: u64) {
    store(paths, keyring, &login_of(refresh, user_id)).unwrap();
}

/// A shared keyring and a scripted server whose handler gets the request's
/// number (from 0), the paths, and the keyring.
fn scripted(
    paths: &Paths,
    keyring: &Arc<FakeKeyring>,
    handler: impl Fn(usize, &Paths, &FakeKeyring) -> Vec<u8> + Send + 'static,
) -> Scripted {
    let (paths, keyring) = (paths.clone(), Arc::clone(keyring));
    let count = AtomicUsize::new(0);
    Scripted::start(move |_| handler(count.fetch_add(1, Ordering::SeqCst), &paths, &keyring))
}

#[test]
fn switching_accounts_during_a_renewal_ends_the_session() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "OLD", USER);
    let server = scripted(&paths, &keyring, |_, paths, keyring| {
        // Another account logs in while this renewal is in flight.
        replace(paths, keyring, "NEWER", OTHER_USER);
        tokens("STALEACCESS", "STALE", USER, 3600)
    });
    let mut session = Session::open(&paths, &*keyring, client_at(&server.base))
        .unwrap()
        .unwrap();

    let error = session.access_token().unwrap_err();
    assert!(matches!(error, AccountError::Switched), "{error:?}");
    assert!(!error.is_temporary());
    assert_eq!(
        saved(&paths, &*keyring),
        Some(login_of("NEWER", OTHER_USER))
    );
    assert_eq!(session.user_id(), UserId::new(USER));
    assert!(session.access.is_none());

    // It stays ended, without asking GOG again.
    assert!(matches!(
        session.access_token().unwrap_err(),
        AccountError::Switched
    ));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_stale_invalid_grant_never_clears_a_newer_login() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "OLD", USER);
    let server = scripted(&paths, &keyring, |n, paths, keyring| {
        if n == 0 {
            // Another process renewed first, and GOG retired the old token.
            replace(paths, keyring, "NEWER", USER);
            invalid_grant()
        } else {
            tokens("A", "NEWER2", USER, 3600)
        }
    });
    let mut session = Session::open(&paths, &*keyring, client_at(&server.base))
        .unwrap()
        .unwrap();
    assert_eq!(session.access_token().unwrap().expose(), "A");
    let sent: Vec<_> = server.requests().iter().map(|l| sent_refresh(l)).collect();
    assert_eq!(sent, ["OLD", "NEWER"]);
    assert_eq!(saved(&paths, &*keyring), Some(login_of("NEWER2", USER)));
}

#[test]
fn a_renewal_racing_a_logout_brings_nothing_back() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "R1", USER);
    let server = scripted(&paths, &keyring, |_, paths, keyring| {
        logout(paths, keyring).unwrap();
        tokens("A", "R2", USER, 3600)
    });
    let mut session = Session::open(&paths, &*keyring, client_at(&server.base))
        .unwrap()
        .unwrap();

    let error = session.access_token().unwrap_err();
    assert!(matches!(error, AccountError::LoggedOut), "{error:?}");
    assert_eq!(saved(&paths, &*keyring), None);
    assert!(!key_store::key_file(&KEY_SPEC, &paths).exists());
    // The renewal's answer isn't usable either.
    assert!(session.access.is_none());
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn every_renewal_attempt_finding_a_newer_login_gives_up() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "R0", USER);
    let server = scripted(&paths, &keyring, |n, paths, keyring| {
        // Each attempt loses to another renewal of the same account.
        replace(paths, keyring, &format!("OTHER{n}"), USER);
        tokens("A", &format!("MINE{n}"), USER, 3600)
    });
    let mut session = Session::open(&paths, &*keyring, client_at(&server.base))
        .unwrap()
        .unwrap();

    let error = session.access_token().unwrap_err();
    assert!(matches!(error, AccountError::Changed), "{error:?}");
    assert!(error.is_temporary());
    let sent: Vec<_> = server.requests().iter().map(|l| sent_refresh(l)).collect();
    assert_eq!(sent, ["R0", "OTHER0", "OTHER1"]);
    // None of this session's answers was saved.
    assert_eq!(saved(&paths, &*keyring), Some(login_of("OTHER2", USER)));
}

fn owned_page_body(page: u32, total: u32) -> Vec<u8> {
    let body = serde_json::json!({
        "page": page,
        "totalPages": total,
        "products": [{"id": page, "title": "Game"}],
    });
    reply(200, &body.to_string())
}

#[test]
fn a_logout_during_a_call_discards_its_answer() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "R1", USER);
    let server = scripted(&paths, &keyring, |n, paths, keyring| {
        if n == 0 {
            tokens("A", "R2", USER, 3600)
        } else {
            // The request is already in flight when the logout lands.
            logout(paths, keyring).unwrap();
            owned_page_body(1, 1)
        }
    });
    let mut session = Session::open(&paths, &*keyring, client_at(&server.base))
        .unwrap()
        .unwrap();
    let error = session
        .call(|client, token| client.owned_page(token, 1))
        .unwrap_err();
    assert!(
        matches!(error, CallError::Account(AccountError::LoggedOut)),
        "{error:?}"
    );
    assert_eq!(server.requests().len(), 2);
    assert!(session.access.is_none());
}

#[test]
fn a_switch_during_a_call_discards_its_answer_but_a_same_account_renewal_does_not() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "R1", USER);
    let server = scripted(&paths, &keyring, |n, paths, keyring| match n {
        0 => tokens("A", "R2", USER, 3600),
        1 => {
            // Another process renews the same account: the answer stands.
            replace(paths, keyring, "R3", USER);
            owned_page_body(1, 1)
        }
        _ => {
            replace(paths, keyring, "THEIRS", OTHER_USER);
            owned_page_body(1, 1)
        }
    });
    let mut session = Session::open(&paths, &*keyring, client_at(&server.base))
        .unwrap()
        .unwrap();
    let page = session
        .call(|client, token| client.owned_page(token, 1))
        .unwrap();
    assert_eq!(page.products.len(), 1);
    assert_eq!(session.login, login_of("R3", USER));

    let error = session
        .call(|client, token| client.owned_page(token, 1))
        .unwrap_err();
    assert!(
        matches!(error, CallError::Account(AccountError::Switched)),
        "{error:?}"
    );
}

#[test]
fn a_failed_generation_bump_leaves_the_credentials_alone() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    replace(&paths, &keyring, "R1", USER);
    let cache = paths.gog_owned_cache(USER);
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    fs::write(&cache, "{}").unwrap();
    let server = Server::start(vec![tokens("A", "R2", USER, 3600)]);
    let mut session = open(&paths, &keyring, &server);

    let generation = paths.gog_credentials_generation();
    let locks = generation.parent().unwrap();
    fs::set_permissions(locks, fs::Permissions::from_mode(0o500)).unwrap();
    // Root ignores directory permissions, so there is nothing to test.
    let can_write_anyway = fs::write(locks.join("probe"), "").is_ok();
    let results = (!can_write_anyway).then(|| {
        (
            store(&paths, &keyring, &login_of("THEIRS", OTHER_USER)).is_err(),
            logout(&paths, &keyring).is_err(),
            session.access_token().is_err(),
        )
    });
    fs::set_permissions(locks, fs::Permissions::from_mode(0o700)).unwrap();
    let Some(results) = results else {
        return;
    };

    assert_eq!(results, (true, true, true));
    // None of the three changed anything: not the login, not the cache.
    assert_eq!(saved(&paths, &keyring), Some(login_of("R1", USER)));
    assert!(cache.exists());
    // The renewal's request went out, but its answer wasn't saved.
    assert_eq!(server.requests().len(), 1);

    // With the counter writable again, a change reaches the session.
    replace(&paths, &keyring, "THEIRS", OTHER_USER);
    assert!(matches!(
        session.access_token().unwrap_err(),
        AccountError::Switched
    ));
}

/// A session that has a fresh access token, from one renewal.
fn renewed_session<'a>(paths: &Paths, keyring: &'a FakeKeyring) -> (Server, Session<'a>) {
    seed(paths, keyring, "R1", USER);
    let server = Server::start(vec![tokens("ACCESS", "R2", USER, 3600)]);
    let mut session = open(paths, keyring, &server);
    session.access_token().unwrap();
    (server, session)
}

#[test]
fn an_existing_session_ends_after_a_logout() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    let (server, mut session) = renewed_session(&paths, &keyring);
    logout(&paths, &keyring).unwrap();
    let error = session.access_token().unwrap_err();
    assert!(matches!(error, AccountError::LoggedOut), "{error:?}");
    assert!(session.access.is_none());
    assert_eq!(server.requests().len(), 1);

    // Logging back in to the same account lets it carry on.
    replace(&paths, &keyring, "R9", USER);
    let server = Server::start(vec![tokens("FRESH", "R10", USER, 3600)]);
    session.client = client(&server);
    assert_eq!(session.access_token().unwrap().expose(), "FRESH");
}

#[test]
fn an_existing_session_ends_after_a_switch_and_never_follows_it() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    let (server, mut session) = renewed_session(&paths, &keyring);
    replace(&paths, &keyring, "THEIRS", OTHER_USER);
    let error = session.access_token().unwrap_err();
    assert!(matches!(error, AccountError::Switched), "{error:?}");
    assert_eq!(session.user_id(), UserId::new(USER));
    assert_eq!(server.requests().len(), 1);
    assert_eq!(
        saved(&paths, &keyring),
        Some(login_of("THEIRS", OTHER_USER))
    );
}

#[test]
fn an_existing_session_adopts_its_own_account_s_newer_login() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    let (server, mut session) = renewed_session(&paths, &keyring);
    replace(&paths, &keyring, "R9", USER);
    // Its own access token is still good, so GOG isn't asked.
    assert_eq!(session.access_token().unwrap().expose(), "ACCESS");
    assert_eq!(session.login, login_of("R9", USER));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn every_change_to_the_login_moves_the_generation() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    assert_eq!(read_generation(&paths).unwrap(), 0);
    let server = Server::start(vec![
        tokens("A", "R1", USER, 3600),
        tokens("B", "R2", USER, 60),
        invalid_grant(),
    ]);
    let mut session = login(&paths, &keyring, client(&server), CODE)
        .unwrap()
        .session;
    let after_login = read_generation(&paths).unwrap();
    assert!(after_login > 0);
    session.access = None;
    session.access_token().unwrap();
    let after_renewal = read_generation(&paths).unwrap();
    assert!(after_renewal > after_login);
    assert_eq!(session.generation, after_renewal);
    // Renewing again is refused, which clears the login.
    session.access_token().unwrap_err();
    let after_clear = read_generation(&paths).unwrap();
    assert!(after_clear > after_renewal);
    logout(&paths, &keyring).unwrap();
    assert!(read_generation(&paths).unwrap() > after_clear);
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
fn an_unreadable_login_is_never_shown_and_logout_still_removes_it() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    *keyring.0.lock().unwrap() = Some("SECRETJUNK not json".to_owned());
    let error = stored_login(&paths, &keyring).unwrap_err();
    assert!(matches!(error, AccountError::Unreadable));
    assert!(!format!("{error} {error:?}").contains("SECRETJUNK"));
    let error = Session::open(&paths, &keyring, client(&Server::start(vec![]))).unwrap_err();
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

#[test]
fn opening_waits_for_a_login_change_in_progress() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "RA", USER);
    let server = Server::start(Vec::new());
    let session_client = client(&server);
    // Another process has announced a switch but not saved it yet.
    let lock = CredentialsLock::acquire(&paths).unwrap();
    bump_generation(&paths, &lock).unwrap();
    std::thread::scope(|scope| {
        let (sent, received) = std::sync::mpsc::channel();
        let (paths_ref, keyring_ref) = (&paths, &keyring);
        let opener = scope.spawn(move || {
            let session = Session::open(paths_ref, keyring_ref, session_client)
                .unwrap()
                .unwrap();
            sent.send(()).unwrap();
            session.user_id()
        });
        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "opening must wait for the change instead of pairing the new generation with the old login"
        );
        save(&paths, &keyring, &login_of("RB", OTHER_USER)).unwrap();
        drop(lock);
        assert_eq!(opener.join().unwrap(), UserId::new(OTHER_USER));
    });
}

#[test]
fn opening_a_session_creates_no_lock_file() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "REFRESH", USER);
    let lock = paths.gog_credentials_lock();
    let server = Server::start(vec![]);
    let session = open(&paths, &keyring, &server);
    assert_eq!(session.user_id(), UserId::new(USER));
    assert!(!lock.exists());
}

fn profile(name: &str) -> Vec<u8> {
    reply(
        200,
        &serde_json::json!({"id": USER.to_string(), "username": name}).to_string(),
    )
}

fn is_profile(line: &str) -> bool {
    line.starts_with("GET /users/")
}

fn profile_requests(server: &Scripted) -> usize {
    server
        .requests()
        .iter()
        .filter(|line| is_profile(line))
        .count()
}

fn named_login(refresh: &str, name: &str) -> StoredLogin {
    StoredLogin {
        username: Some(name.to_owned()),
        ..login_of(refresh, USER)
    }
}

#[test]
fn logging_in_stores_the_username() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    let server = Scripted::start(|line| {
        if is_profile(line) {
            profile("Hunter42")
        } else {
            tokens("ACCESS1", "REFRESH1", USER, 3600)
        }
    });
    let logged_in = login(&paths, &keyring, client_with_profiles(&server.base), CODE).unwrap();
    assert_eq!(logged_in.username.as_deref(), Some("Hunter42"));
    assert_eq!(
        server.requests()[1],
        format!("GET /users/{USER} HTTP/1.1"),
        "the name is asked for after the code is accepted"
    );

    let (stored, _) = stored_login(&paths, &keyring).unwrap().unwrap();
    assert_eq!(stored, named_login("REFRESH1", "Hunter42"));
    assert_eq!(
        keyring.0.lock().unwrap().as_deref(),
        Some(r#"{"refresh_token":"REFRESH1","user_id":"48628349957132247","username":"Hunter42"}"#)
    );
}

#[test]
fn a_failed_name_lookup_still_logs_in_with_none() {
    for answer in [
        reply(503, "{}"),
        reply(404, "{}"),
        reply(200, r#"{"username":"  "}"#),
    ] {
        let (_root, paths) = setup();
        let keyring = FakeKeyring::default();
        let server = Scripted::start(move |line| {
            if is_profile(line) {
                answer.clone()
            } else {
                tokens("ACCESS1", "REFRESH1", USER, 3600)
            }
        });
        let logged_in = login(&paths, &keyring, client_with_profiles(&server.base), CODE).unwrap();
        assert_eq!(logged_in.username, None);
        assert_eq!(logged_in.user_id, UserId::new(USER));
        assert_eq!(saved(&paths, &keyring), Some(login_of("REFRESH1", USER)));
        // Nothing but the token and refresh fields, as before names existed.
        assert_eq!(
            keyring.0.lock().unwrap().as_deref(),
            Some(r#"{"refresh_token":"REFRESH1","user_id":"48628349957132247"}"#)
        );
    }
}

#[test]
fn a_login_saved_without_a_username_still_reads_and_is_not_rewritten() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    let old = r#"{"refresh_token":"OLD","user_id":"48628349957132247"}"#;
    *keyring.0.lock().unwrap() = Some(old.to_owned());
    let (stored, source) = stored_login(&paths, &keyring).unwrap().unwrap();
    assert_eq!(stored, login_of("OLD", USER));
    assert_eq!(source, KeySource::Keyring);
    // Opening a session only reads too.
    let server = Server::start(vec![]);
    let session = open(&paths, &keyring, &server);
    assert_eq!(session.login.username, None);
    assert_eq!(keyring.0.lock().unwrap().as_deref(), Some(old));
    assert!(!paths.gog_credentials_generation().exists());
}

#[test]
fn a_renewal_keeps_the_saved_username() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    save(&paths, &keyring, &named_login("R1", "Hunter42")).unwrap();
    let server = Scripted::start(|line| {
        if is_profile(line) {
            profile("Changed")
        } else {
            tokens("A2", "R2", USER, 3600)
        }
    });
    let mut session = Session::open(&paths, &keyring, client_with_profiles(&server.base))
        .unwrap()
        .unwrap();
    session.access_token().unwrap();
    assert_eq!(saved(&paths, &keyring), Some(named_login("R2", "Hunter42")));
    assert_eq!(profile_requests(&server), 0, "it is only asked for once");
}

#[test]
fn a_renewal_fills_in_a_missing_username_once_per_session() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    let server = Scripted::start(|line| {
        if is_profile(line) {
            profile("Hunter42")
        } else {
            // Expiring at once, so every call renews.
            tokens("A", "R", USER, 60)
        }
    });
    let mut session = Session::open(&paths, &keyring, client_with_profiles(&server.base))
        .unwrap()
        .unwrap();
    session.access_token().unwrap();
    let stored = saved(&paths, &keyring).unwrap();
    assert_eq!(stored.username.as_deref(), Some("Hunter42"));
    assert_eq!(stored.refresh_token, "R");
    assert_eq!(profile_requests(&server), 1);

    session.access_token().unwrap();
    assert_eq!(saved(&paths, &keyring).unwrap().username, stored.username);
    assert_eq!(profile_requests(&server), 1);
}

#[test]
fn a_failed_lookup_during_a_renewal_changes_nothing_and_is_not_repeated() {
    for answer in [reply(503, "{}"), reply(200, "not json"), reply(404, "{}")] {
        let (_root, paths) = setup();
        let keyring = FakeKeyring::default();
        seed(&paths, &keyring, "R1", USER);
        let server = Scripted::start(move |line| {
            if is_profile(line) {
                answer.clone()
            } else {
                tokens("A", "R", USER, 60)
            }
        });
        let mut session = Session::open(&paths, &keyring, client_with_profiles(&server.base))
            .unwrap()
            .unwrap();
        assert_eq!(session.access_token().unwrap().expose(), "A");
        assert_eq!(saved(&paths, &keyring), Some(login_of("R", USER)));
        assert_eq!(session.access_token().unwrap().expose(), "A");
        assert_eq!(profile_requests(&server), 1);
    }

    // GOG being unreachable for the lookup alone is no different.
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    let server = Server::start(vec![tokens("A", "R2", USER, 3600)]);
    let mut session = open(&paths, &keyring, &server);
    session.access_token().unwrap();
    assert_eq!(saved(&paths, &keyring), Some(login_of("R2", USER)));
}

#[test]
fn a_renewal_that_fails_never_asks_for_the_name() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    let server = Scripted::start(|line| {
        if is_profile(line) {
            profile("Hunter42")
        } else {
            invalid_grant()
        }
    });
    let mut session = Session::open(&paths, &keyring, client_with_profiles(&server.base))
        .unwrap()
        .unwrap();
    assert!(matches!(
        session.access_token().unwrap_err(),
        AccountError::LoggedOut
    ));
    assert_eq!(profile_requests(&server), 0);
    assert_eq!(saved(&paths, &keyring), None);
}

#[test]
fn a_name_found_for_a_stale_renewal_is_saved_with_the_retry_not_over_a_newer_login() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "OLD", USER);
    let (handler_paths, handler_keyring) = (paths.clone(), Arc::clone(&keyring));
    let tokens_seen = AtomicUsize::new(0);
    let server = Scripted::start(move |line| {
        if is_profile(line) {
            return profile("Hunter42");
        }
        if tokens_seen.fetch_add(1, Ordering::SeqCst) == 0 {
            // Another process renewed first, so this answer is stale.
            replace(&handler_paths, &*handler_keyring, "NEWER", USER);
            tokens("STALE", "STALE", USER, 3600)
        } else {
            tokens("A", "NEWER2", USER, 3600)
        }
    });
    let mut session = Session::open(&paths, &*keyring, client_with_profiles(&server.base))
        .unwrap()
        .unwrap();
    assert_eq!(session.access_token().unwrap().expose(), "A");
    let sent: Vec<_> = server
        .requests()
        .iter()
        .filter(|line| !is_profile(line))
        .map(|line| sent_refresh(line))
        .collect();
    assert_eq!(sent, ["OLD", "NEWER"]);
    assert_eq!(
        saved(&paths, &*keyring),
        Some(named_login("NEWER2", "Hunter42"))
    );
    assert_eq!(profile_requests(&server), 1);
}

#[test]
fn another_process_filling_the_name_is_not_a_newer_login() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "R1", USER);
    let (handler_paths, handler_keyring) = (paths.clone(), Arc::clone(&keyring));
    let server = Scripted::start(move |line| {
        if is_profile(line) {
            return profile("Mine");
        }
        // The same refresh token with only the name added.
        store(
            &handler_paths,
            &*handler_keyring,
            &named_login("R1", "Theirs"),
        )
        .unwrap();
        tokens("A", "R2", USER, 3600)
    });
    let mut session = Session::open(&paths, &*keyring, client_with_profiles(&server.base))
        .unwrap()
        .unwrap();
    assert_eq!(session.access_token().unwrap().expose(), "A");
    // One renewal, not a retry, and the name already saved is kept.
    let renewals = server
        .requests()
        .iter()
        .filter(|line| !is_profile(line))
        .count();
    assert_eq!(renewals, 1);
    assert_eq!(saved(&paths, &*keyring), Some(named_login("R2", "Theirs")));
}

#[test]
fn the_username_is_not_a_secret_but_the_refresh_token_stays_redacted() {
    let shown = format!("{:?}", named_login("REFRESHSECRET", "Hunter42"));
    assert!(shown.contains("Hunter42"), "{shown}");
    assert!(!shown.contains("REFRESHSECRET"), "{shown}");
}

/// Whether nobody holds the credentials lock right now, tried without
/// waiting so a held lock fails the test instead of hanging it.
fn lock_is_free(paths: &Paths) -> bool {
    let path = paths.gog_credentials_lock();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    flock(&file, FlockOperation::NonBlockingLockExclusive).is_ok()
}

/// A scripted GOG whose profile answers run `during` (with whether the
/// credentials lock was free) before replying with a name. Tokens come from
/// `token`, given the request's number from 0.
fn racing_profile(
    paths: &Paths,
    during: impl Fn() + Send + 'static,
    token: impl Fn(usize) -> Vec<u8> + Send + 'static,
) -> (Scripted, Arc<std::sync::atomic::AtomicBool>) {
    let free = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (seen, paths) = (Arc::clone(&free), paths.clone());
    let tokens_seen = AtomicUsize::new(0);
    let server = Scripted::start(move |line| {
        if is_profile(line) {
            if !lock_is_free(&paths) {
                seen.store(false, Ordering::SeqCst);
            }
            during();
            profile("Hunter42")
        } else {
            token(tokens_seen.fetch_add(1, Ordering::SeqCst))
        }
    });
    (server, free)
}

#[test]
fn a_logout_during_the_profile_request_is_not_undone_by_the_renewal() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "R1", USER);
    let (during_paths, during_keyring) = (paths.clone(), Arc::clone(&keyring));
    let (server, free) = racing_profile(
        &paths,
        move || {
            logout(&during_paths, &*during_keyring).unwrap();
        },
        |_| tokens("A", "R2", USER, 3600),
    );
    let mut session = Session::open(&paths, &*keyring, client_with_profiles(&server.base))
        .unwrap()
        .unwrap();
    let error = session.access_token().unwrap_err();
    assert!(matches!(error, AccountError::LoggedOut), "{error:?}");
    assert!(free.load(Ordering::SeqCst), "the lookup held the lock");
    assert_eq!(profile_requests(&server), 1);
    assert_eq!(saved(&paths, &*keyring), None);
    assert!(!key_store::key_file(&KEY_SPEC, &paths).exists());
    assert!(session.access.is_none());
}

#[test]
fn a_switch_during_the_profile_request_leaves_the_newer_login_alone() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "R1", USER);
    let (during_paths, during_keyring) = (paths.clone(), Arc::clone(&keyring));
    let (server, free) = racing_profile(
        &paths,
        move || replace(&during_paths, &*during_keyring, "THEIRS", OTHER_USER),
        |_| tokens("A", "R2", USER, 3600),
    );
    let mut session = Session::open(&paths, &*keyring, client_with_profiles(&server.base))
        .unwrap()
        .unwrap();
    let error = session.access_token().unwrap_err();
    assert!(matches!(error, AccountError::Switched), "{error:?}");
    assert!(free.load(Ordering::SeqCst), "the lookup held the lock");
    // Exactly what the other account saved, with nobody's name added.
    assert_eq!(
        saved(&paths, &*keyring),
        Some(login_of("THEIRS", OTHER_USER))
    );
    assert!(session.access.is_none());
}

#[test]
fn a_newer_login_during_the_profile_request_is_renewed_and_gets_the_name() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "R1", USER);
    let (during_paths, during_keyring) = (paths.clone(), Arc::clone(&keyring));
    let replaced = AtomicUsize::new(0);
    let (server, free) = racing_profile(
        &paths,
        move || {
            if replaced.fetch_add(1, Ordering::SeqCst) == 0 {
                replace(&during_paths, &*during_keyring, "NEWER", USER);
            }
        },
        |n| tokens("A", &format!("R{}", n + 2), USER, 3600),
    );
    let mut session = Session::open(&paths, &*keyring, client_with_profiles(&server.base))
        .unwrap()
        .unwrap();
    assert_eq!(session.access_token().unwrap().expose(), "A");
    assert!(free.load(Ordering::SeqCst), "the lookup held the lock");
    let sent: Vec<_> = server
        .requests()
        .iter()
        .filter(|line| !is_profile(line))
        .map(|line| sent_refresh(line))
        .collect();
    // The stale answer (R2) was thrown away, and the retry used the newer
    // login's token.
    assert_eq!(sent, ["R1", "NEWER"]);
    assert_eq!(profile_requests(&server), 1);
    assert_eq!(
        saved(&paths, &*keyring),
        Some(named_login("R3", "Hunter42"))
    );
}

#[test]
fn the_lookup_at_login_holds_no_lock_and_saves_nothing_before_it_ends() {
    let (_root, paths) = setup();
    let keyring = Arc::new(FakeKeyring::default());
    seed(&paths, &*keyring, "PREVIOUS", OTHER_USER);
    let (during_paths, during_keyring) = (paths.clone(), Arc::clone(&keyring));
    let during_saved = Arc::new(Mutex::new(None));
    let record = Arc::clone(&during_saved);
    let (server, free) = racing_profile(
        &paths,
        move || {
            *record.lock().unwrap() = saved(&during_paths, &*during_keyring);
        },
        |_| tokens("A", "R1", USER, 3600),
    );
    let logged_in = login(&paths, &*keyring, client_with_profiles(&server.base), CODE).unwrap();
    assert!(free.load(Ordering::SeqCst), "the lookup held the lock");
    assert_eq!(
        *during_saved.lock().unwrap(),
        Some(login_of("PREVIOUS", OTHER_USER)),
        "the login is saved only after the lookup"
    );
    assert_eq!(logged_in.username.as_deref(), Some("Hunter42"));
    assert_eq!(
        saved(&paths, &*keyring),
        Some(named_login("R1", "Hunter42"))
    );
}

#[test]
fn a_slow_profile_delays_a_renewal_and_a_login_by_no_more_than_its_deadline() {
    let slow = |line: &str| {
        if is_profile(line) {
            std::thread::sleep(Duration::from_millis(1500));
            profile("Hunter42")
        } else {
            tokens("A", "R2", USER, 3600)
        }
    };
    let quick = |server: &Scripted| {
        client_with_profiles(&server.base).with_profile_deadline(Duration::from_millis(200))
    };

    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    let server = Scripted::start(slow);
    let mut session = Session::open(&paths, &keyring, quick(&server))
        .unwrap()
        .unwrap();
    let started = Instant::now();
    session.access_token().unwrap();
    assert!(started.elapsed() < Duration::from_millis(1200));
    // The renewed token is saved without a name, and not looked up again.
    assert_eq!(saved(&paths, &keyring), Some(login_of("R2", USER)));

    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    let server = Scripted::start(slow);
    let started = Instant::now();
    let logged_in = login(&paths, &keyring, quick(&server), CODE).unwrap();
    assert!(started.elapsed() < Duration::from_millis(1200));
    assert_eq!(logged_in.username, None);
    assert_eq!(saved(&paths, &keyring), Some(login_of("R2", USER)));
}

#[test]
fn a_named_login_round_trips_through_the_plain_text_key_file() {
    let (_root, paths) = setup();
    let server = Scripted::start(|line| {
        if is_profile(line) {
            profile("Hunter42")
        } else {
            tokens("A", "R1", USER, 3600)
        }
    });
    let logged_in = login(&paths, &NoKeyring, client_with_profiles(&server.base), CODE).unwrap();
    let file = key_store::key_file(&KEY_SPEC, &paths);
    assert_eq!(logged_in.saved, Saved::File(file.clone()));
    assert_eq!(logged_in.username.as_deref(), Some("Hunter42"));
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains(r#""username":"Hunter42""#)
    );

    let (stored, source) = stored_login(&paths, &NoKeyring).unwrap().unwrap();
    assert_eq!(
        (stored, source),
        (named_login("R1", "Hunter42"), KeySource::File)
    );

    // A renewal from the file keeps the name, without asking for it.
    let mut session = Session::open(&paths, &NoKeyring, client_with_profiles(&server.base))
        .unwrap()
        .unwrap();
    session.access_token().unwrap();
    assert_eq!(
        stored_login(&paths, &NoKeyring)
            .unwrap()
            .unwrap()
            .0
            .username
            .as_deref(),
        Some("Hunter42")
    );
    assert_eq!(profile_requests(&server), 1);
}

const GAME: u64 = 1771589310;
const SECRET_WORD: &str = "open-sesame-9";

fn password(text: &str) -> Secret {
    Secret::new(text)
}

fn branch_file(paths: &Paths) -> std::path::PathBuf {
    branches::file(paths)
}

#[test]
fn branch_passwords_live_in_the_keyring_per_game_and_branch() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    let first = password(SECRET_WORD);
    assert_eq!(
        branches::save(&paths, &keyring, GAME, "beta", &first).unwrap(),
        Saved::Keyring
    );
    branches::save(
        &paths,
        &keyring,
        GAME,
        "Legacy (64-bit)",
        &password("legacy"),
    )
    .unwrap();
    branches::save(&paths, &keyring, 5, "beta", &password("other game")).unwrap();
    assert!(!branch_file(&paths).exists());

    let found = |game, branch| {
        branches::find(&paths, &keyring, game, branch)
            .unwrap()
            .map(|secret| secret.expose().to_owned())
    };
    assert_eq!(found(GAME, "beta").as_deref(), Some(SECRET_WORD));
    assert_eq!(found(5, "beta").as_deref(), Some("other game"));
    assert_eq!(found(GAME, "gamma"), None);
    let listed: Vec<_> = branches::list(&paths, &keyring, GAME)
        .unwrap()
        .into_iter()
        .map(|(branch, secret)| (branch, secret.expose().to_owned()))
        .collect();
    assert_eq!(
        listed,
        [
            ("Legacy (64-bit)".to_owned(), "legacy".to_owned()),
            ("beta".to_owned(), SECRET_WORD.to_owned())
        ]
    );

    // Entering one again replaces it.
    branches::save(&paths, &keyring, GAME, "beta", &password("changed")).unwrap();
    assert_eq!(found(GAME, "beta").as_deref(), Some("changed"));

    assert!(branches::remove(&paths, &keyring, GAME, "beta").unwrap());
    assert!(!branches::remove(&paths, &keyring, GAME, "beta").unwrap());
    assert_eq!(found(GAME, "beta"), None);
    assert_eq!(found(5, "beta").as_deref(), Some("other game"));
}

#[test]
fn without_a_keyring_branch_passwords_are_one_private_file() {
    let (_root, paths) = setup();
    let file = branch_file(&paths);
    assert_eq!(file.file_name().unwrap(), "gog-branches.key");
    let saved = branches::save(&paths, &NoKeyring, GAME, "beta", &password(SECRET_WORD)).unwrap();
    assert_eq!(saved, Saved::File(file.clone()));
    branches::save(&paths, &NoKeyring, GAME, "gamma", &password("g")).unwrap();
    branches::save(&paths, &NoKeyring, 5, "beta", &password("five")).unwrap();
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let find = |game, branch| {
        branches::find(&paths, &NoKeyring, game, branch)
            .unwrap()
            .map(|secret| secret.expose().to_owned())
    };
    assert_eq!(find(GAME, "beta").as_deref(), Some(SECRET_WORD));
    assert_eq!(branches::list(&paths, &NoKeyring, GAME).unwrap().len(), 2);

    branches::save(&paths, &NoKeyring, GAME, "beta", &password("changed")).unwrap();
    assert_eq!(find(GAME, "beta").as_deref(), Some("changed"));

    assert!(branches::remove(&paths, &NoKeyring, GAME, "beta").unwrap());
    assert_eq!(find(GAME, "beta"), None);
    assert_eq!(find(GAME, "gamma").as_deref(), Some("g"));
    assert!(branches::remove_all(&paths, &NoKeyring).unwrap());
    assert!(!file.exists());
    assert!(!branches::remove_all(&paths, &NoKeyring).unwrap());
}

#[test]
fn a_keyring_save_clears_the_file_copy() {
    let (_root, paths) = setup();
    branches::save(&paths, &NoKeyring, GAME, "beta", &password("from the file")).unwrap();
    let keyring = FakeKeyring::default();
    branches::save(&paths, &keyring, GAME, "beta", &password("in the keyring")).unwrap();
    // The only branch in the file, so the file goes with it.
    assert!(!branch_file(&paths).exists());
    assert_eq!(
        branches::find(&paths, &keyring, GAME, "beta")
            .unwrap()
            .unwrap()
            .expose(),
        "in the keyring"
    );
}

/// A keyring whose branch wipe fails, as a locked one can.
struct WipeFails(FakeKeyring);

impl Keyring for WipeFails {
    fn get(&self) -> Result<Option<String>, key_store::KeyringError> {
        self.0.get()
    }

    fn set(&self, key: &str) -> Result<(), key_store::KeyringError> {
        self.0.set(key)
    }

    fn delete(&self) -> Result<bool, key_store::KeyringError> {
        self.0.delete()
    }

    fn delete_all_branches(&self) -> Result<bool, key_store::KeyringError> {
        Err(key_store::KeyringError::Failed("locked".to_owned()))
    }
}

#[test]
fn logging_out_removes_every_branch_password() {
    for keyring in [
        Box::new(FakeKeyring::default()) as Box<dyn Keyring>,
        Box::new(NoKeyring),
    ] {
        let (_root, paths) = setup();
        seed(&paths, &*keyring, "R1", USER);
        branches::save(&paths, &*keyring, GAME, "beta", &password(SECRET_WORD)).unwrap();
        branches::save(&paths, &*keyring, 5, "gamma", &password("g")).unwrap();
        logout(&paths, &*keyring).unwrap();
        assert!(branches::list(&paths, &*keyring, GAME).unwrap().is_empty());
        assert!(branches::list(&paths, &*keyring, 5).unwrap().is_empty());
        assert!(!branch_file(&paths).exists());
    }
}

#[test]
fn a_logout_that_cannot_wipe_the_passwords_keeps_the_login() {
    let (_root, paths) = setup();
    let keyring = WipeFails(FakeKeyring::default());
    seed(&paths, &keyring, "R1", USER);
    branches::save(&paths, &keyring, GAME, "beta", &password(SECRET_WORD)).unwrap();
    let error = logout(&paths, &keyring).unwrap_err();
    assert!(matches!(error, AccountError::Store(_)), "{error:?}");
    assert_eq!(saved(&paths, &keyring), Some(login_of("R1", USER)));
    assert!(!format!("{error:?} {error}").contains(SECRET_WORD));
}

#[test]
fn logging_in_as_another_account_removes_them_but_the_same_account_keeps_them() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    branches::save(&paths, &keyring, GAME, "beta", &password(SECRET_WORD)).unwrap();

    // The same account logging in again, or renewing, keeps them.
    replace(&paths, &keyring, "R2", USER);
    assert_eq!(branches::list(&paths, &keyring, GAME).unwrap().len(), 1);

    replace(&paths, &keyring, "R3", OTHER_USER);
    assert!(branches::list(&paths, &keyring, GAME).unwrap().is_empty());

    // Passwords with no login to compare against don't carry over either.
    logout(&paths, &keyring).unwrap();
    branches::save(&paths, &keyring, GAME, "beta", &password(SECRET_WORD)).unwrap();
    replace(&paths, &keyring, "R4", USER);
    assert!(branches::list(&paths, &keyring, GAME).unwrap().is_empty());
}

#[test]
fn the_real_login_path_removes_them_for_another_account() {
    let (_root, paths) = setup();
    seed(&paths, &NoKeyring, "R1", USER);
    branches::save(&paths, &NoKeyring, GAME, "beta", &password(SECRET_WORD)).unwrap();
    let server = Server::start(vec![tokens("A", "REFRESH2", OTHER_USER, 3600)]);
    login(&paths, &NoKeyring, client(&server), CODE).unwrap();
    assert!(!branch_file(&paths).exists());
}

#[test]
fn a_refused_login_takes_the_passwords_with_it() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    branches::save(&paths, &keyring, GAME, "beta", &password(SECRET_WORD)).unwrap();
    let server = Server::start(vec![invalid_grant()]);
    let error = open(&paths, &keyring, &server).access_token().unwrap_err();
    assert!(matches!(error, AccountError::LoggedOut), "{error:?}");
    assert!(branches::list(&paths, &keyring, GAME).unwrap().is_empty());
}

#[test]
fn a_session_saves_and_forgets_passwords_only_while_its_account_is_current() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, "R1", USER);
    let server = Server::start(vec![]);
    let mut session = open(&paths, &keyring, &server);
    session
        .save_branch_password(GAME, "beta", &password(SECRET_WORD))
        .unwrap();
    assert_eq!(
        session
            .branch_password(GAME, "beta")
            .unwrap()
            .unwrap()
            .expose(),
        SECRET_WORD
    );
    assert_eq!(session.branch_passwords(GAME).unwrap().len(), 1);
    assert!(session.forget_branch_password(GAME, "beta").unwrap());
    assert!(!session.forget_branch_password(GAME, "beta").unwrap());

    // Once the account has left, nothing is saved for it.
    logout(&paths, &keyring).unwrap();
    let error = session
        .save_branch_password(GAME, "beta", &password(SECRET_WORD))
        .unwrap_err();
    assert!(matches!(error, AccountError::LoggedOut), "{error:?}");
    assert!(branches::list(&paths, &keyring, GAME).unwrap().is_empty());
    replace(&paths, &keyring, "R9", OTHER_USER);
    let error = session
        .save_branch_password(GAME, "beta", &password(SECRET_WORD))
        .unwrap_err();
    assert!(matches!(error, AccountError::Switched), "{error:?}");
}

#[test]
fn a_branch_password_is_redacted_in_debug() {
    let secret = password(SECRET_WORD);
    assert!(!format!("{secret:?} {secret:#?}").contains(SECRET_WORD));
}
