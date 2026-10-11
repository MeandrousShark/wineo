use super::*;
use crate::gog::account::{self, Session};
use crate::gog::test_support::{FakeKeyring, client, setup, tokens};
use crate::key_store::{KeySpec, Keyring};
use crate::steam::api::fixture::{Server, reply};

const USER: u64 = 1001;
const OTHER_USER: u64 = 2002;
const T: i64 = 1_800_000_000;

fn seed(paths: &Paths, keyring: &dyn Keyring, user_id: u64) {
    let secret = format!(r#"{{"refresh_token":"R-{user_id}","user_id":"{user_id}"}}"#);
    let spec: &KeySpec = &account::KEY_SPEC;
    crate::key_store::save_key(spec, paths, keyring, &secret).unwrap();
}

fn page(page: u32, total: u32, games: &[(u64, &str)]) -> Vec<u8> {
    let products: Vec<_> = games
        .iter()
        .map(|(id, title)| {
            serde_json::json!({
                "id": id,
                "title": title,
                "image": "//images.gog.com/x",
                "worksOn": {"Windows": true, "Mac": false, "Linux": false},
            })
        })
        .collect();
    let body = serde_json::json!({"page": page, "totalPages": total, "products": products});
    reply(200, &body.to_string())
}

fn titles(owned: &Owned) -> Vec<&str> {
    owned.games.iter().map(|game| game.title.as_str()).collect()
}

fn run(
    paths: &Paths,
    keyring: &FakeKeyring,
    replies: Vec<Vec<u8>>,
    refresh: bool,
    now: i64,
) -> (Server, Result<Owned, LoadError>) {
    let server = Server::start(replies);
    let mut session = Session::open(paths, keyring, client(&server))
        .unwrap()
        .unwrap();
    let result = load_at(&mut session, refresh, now);
    (server, result)
}

fn token() -> Vec<u8> {
    tokens("ACCESS", "R-next", USER, 3600)
}

#[test]
fn every_page_is_fetched_and_cached() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let (server, owned) = run(
        &paths,
        &keyring,
        vec![
            token(),
            page(1, 2, &[(1, "Alpha")]),
            page(2, 2, &[(2, "Beta")]),
        ],
        false,
        T,
    );
    let owned = owned.unwrap();
    assert_eq!(titles(&owned), ["Alpha", "Beta"]);
    assert_eq!(owned.warning, None);
    let requests = server.requests();
    assert!(requests[1].contains("/account/getFilteredProducts?mediaType=1&page=1"));
    assert!(requests[2].contains("page=2"));

    let cache = fs::read_to_string(paths.gog_owned_cache(USER)).unwrap();
    assert!(cache.contains("\"fetched_at\":1800000000"));
    assert!(cache.contains(&format!("\"user_id\":\"{USER}\"")));
    assert!(!cache.contains("ACCESS") && !cache.contains("R-"));
}

#[test]
fn a_fresh_cache_is_used_without_asking_gog_at_all() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let (_, first) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(1, "Alpha")])],
        false,
        T,
    );
    first.unwrap();
    let (quiet, cached) = run(&paths, &keyring, vec![], false, T + 3599);
    assert_eq!(titles(&cached.unwrap()), ["Alpha"]);
    // Not even the token endpoint.
    assert!(quiet.requests().is_empty());
}

#[test]
fn a_stale_cache_and_a_forced_refresh_both_fetch() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let (_, first) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(1, "Alpha")])],
        false,
        T,
    );
    first.unwrap();

    let (stale, owned) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(2, "Beta")])],
        false,
        T + 3600,
    );
    assert_eq!(titles(&owned.unwrap()), ["Beta"]);
    assert_eq!(stale.requests().len(), 2);

    let (forced, owned) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(3, "Gamma")])],
        true,
        T + 3601,
    );
    assert_eq!(titles(&owned.unwrap()), ["Gamma"]);
    assert_eq!(forced.requests().len(), 2);
}

#[test]
fn a_failed_page_keeps_the_last_complete_list_with_a_warning() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let (_, first) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(1, "Alpha")])],
        false,
        T,
    );
    first.unwrap();
    let before = fs::read(paths.gog_owned_cache(USER)).unwrap();

    let (_, owned) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 2, &[(9, "Partial")]), reply(500, "{}")],
        true,
        T + 10,
    );
    let owned = owned.unwrap();
    assert_eq!(titles(&owned), ["Alpha"]);
    assert_eq!(
        owned.warning.as_deref(),
        Some(
            "Couldn't refresh owned GOG games: GOG owned games request failed: GOG answered with status 500"
        )
    );
    assert_eq!(fs::read(paths.gog_owned_cache(USER)).unwrap(), before);
}

#[test]
fn a_failed_fetch_without_a_cache_is_an_error() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let (_, owned) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 2, &[(1, "A")]), reply(200, "{")],
        false,
        T,
    );
    let error = owned.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Couldn't get owned GOG games: GOG owned games request failed: GOG's answer isn't valid JSON"
    );
    assert!(!paths.gog_owned_cache(USER).exists());
}

#[test]
fn a_temporary_renewal_failure_falls_back_but_logged_out_never_does() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let (_, first) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(1, "Alpha")])],
        false,
        T,
    );
    first.unwrap();

    let (_, owned) = run(&paths, &keyring, vec![reply(503, "{}")], true, T + 10);
    let owned = owned.unwrap();
    assert_eq!(titles(&owned), ["Alpha"]);
    assert!(
        owned
            .warning
            .unwrap()
            .starts_with("Couldn't refresh owned GOG games: GOG token request failed")
    );

    let (_, owned) = run(
        &paths,
        &keyring,
        vec![reply(400, r#"{"error":"invalid_grant"}"#)],
        true,
        T + 20,
    );
    let error = owned.unwrap_err();
    assert!(
        matches!(error, LoadError::Account(AccountError::LoggedOut)),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "Log in to GOG again: wineo import gog --login."
    );
}

#[test]
fn caches_are_per_account() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let (_, first) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(1, "Mine")])],
        false,
        T,
    );
    first.unwrap();

    // Another account logs in; the first one's fresh cache isn't used.
    seed(&paths, &keyring, OTHER_USER);
    let (server, owned) = run(
        &paths,
        &keyring,
        vec![
            tokens("A", "R", OTHER_USER, 3600),
            page(1, 1, &[(2, "Theirs")]),
        ],
        false,
        T + 1,
    );
    assert_eq!(titles(&owned.unwrap()), ["Theirs"]);
    assert_eq!(server.requests().len(), 2);
    assert!(paths.gog_owned_cache(USER).exists());
    assert!(
        paths
            .gog_owned_cache(OTHER_USER)
            .ends_with(format!("wineo/gog-owned-{OTHER_USER}.json"))
    );
}

#[test]
fn a_cache_naming_another_account_is_not_used() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let foreign = Cache {
        fetched_at: T,
        user_id: UserId::new(OTHER_USER),
        games: Vec::new(),
    };
    write_cache(&paths.gog_owned_cache(USER), &foreign).unwrap();
    let (server, owned) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(1, "Mine")])],
        false,
        T + 1,
    );
    assert_eq!(titles(&owned.unwrap()), ["Mine"]);
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn logging_out_deletes_the_cache() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let (_, first) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(1, "Alpha")])],
        false,
        T,
    );
    first.unwrap();
    assert!(paths.gog_owned_cache(USER).exists());
    let out = account::logout(&paths, &keyring).unwrap();
    assert!(out.cache_removed);
    assert!(!paths.gog_owned_cache(USER).exists());
    assert!(
        Session::open(&paths, &keyring, client(&Server::start(vec![])))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_cache_that_cannot_be_written_is_a_warning() {
    let (root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    fs::create_dir_all(root.path().join(".cache")).unwrap();
    fs::write(root.path().join(".cache/wineo"), "in the way").unwrap();
    let (_, owned) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(1, "Alpha")])],
        false,
        T,
    );
    let owned = owned.unwrap();
    assert_eq!(titles(&owned), ["Alpha"]);
    assert!(
        owned
            .warning
            .unwrap()
            .starts_with("Couldn't save owned GOG games: ")
    );
}

#[test]
fn a_refused_access_token_is_renewed_once_and_the_fetch_retried() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let (server, owned) = run(
        &paths,
        &keyring,
        vec![
            tokens("A1", "R-second", USER, 3600),
            reply(401, "{}"),
            tokens("A2", "R-third", USER, 3600),
            page(1, 1, &[(1, "Alpha")]),
        ],
        false,
        T,
    );
    let owned = owned.unwrap();
    assert_eq!(titles(&owned), ["Alpha"]);
    assert_eq!(owned.warning, None);
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    // The forced renewal used the refresh token the first one saved.
    assert!(
        requests[2].contains("refresh_token=R-second"),
        "{}",
        requests[2]
    );
    let (login, _) = account::stored_login(&paths, &keyring).unwrap().unwrap();
    assert_eq!(login.refresh_token, "R-third");
}

#[test]
fn a_second_refusal_falls_back_to_the_cache_and_keeps_the_login() {
    let (_root, paths) = setup();
    let keyring = FakeKeyring::default();
    seed(&paths, &keyring, USER);
    let (_, first) = run(
        &paths,
        &keyring,
        vec![token(), page(1, 1, &[(1, "Alpha")])],
        false,
        T,
    );
    first.unwrap();

    let refused = || {
        vec![
            tokens("A1", "R-a", USER, 3600),
            reply(401, "{}"),
            tokens("A2", "R-b", USER, 3600),
            reply(401, "{}"),
        ]
    };
    let (server, owned) = run(&paths, &keyring, refused(), true, T + 10);
    let owned = owned.unwrap();
    assert_eq!(titles(&owned), ["Alpha"]);
    assert_eq!(
        owned.warning.as_deref(),
        Some(
            "Couldn't refresh owned GOG games: GOG owned games request failed: GOG rejected the login"
        )
    );
    assert_eq!(server.requests().len(), 4);
    assert!(account::stored_login(&paths, &keyring).unwrap().is_some());

    // Without a cache it's the request's error, and the login stays.
    remove_cache(&paths, UserId::new(USER)).unwrap();
    let (_, owned) = run(&paths, &keyring, refused(), true, T + 20);
    match owned.unwrap_err() {
        LoadError::Fetch(error) => {
            assert_eq!(error.kind(), wineo_gog::ErrorKind::Unauthorized)
        }
        other => panic!("{other:?}"),
    }
    assert!(account::stored_login(&paths, &keyring).unwrap().is_some());
}
