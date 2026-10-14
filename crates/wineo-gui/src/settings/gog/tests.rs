use std::cell::RefCell;
use std::ffi::OsString;

use wineo_core::key_store::NoKeyring;

use super::*;
use crate::gog_fake::{Server, reply, tokens};
use crate::settings::steam_key::tests::TestKeyring;

const CODE: &str = "PASTEDCODEabcdef0123456789";

fn pasted() -> String {
    format!("https://embed.gog.com/on_login_success?origin=client&code={CODE}")
}

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

fn keyring() -> TestKeyring {
    TestKeyring(RefCell::new(None))
}

#[test]
fn logging_in_says_which_account_and_logging_out_what_was_removed() {
    let (_root, paths) = setup();
    let keyring = keyring();
    assert_eq!(status_with(&paths, &keyring).unwrap(), Status::LoggedOut);
    assert_eq!(Status::LoggedOut.label(), "Not logged in");

    let server = Server::start(|_| tokens("REFRESH1", 48151623));
    let done = login_with(&paths, &keyring, server.client(), &pasted()).unwrap();
    // This server has no profile to give, so it falls back to the number.
    assert_eq!(done.status.label(), "Logged in to GOG as account 48151623.");
    assert!(server.requests()[0].contains(CODE));
    let status = status_with(&paths, &keyring).unwrap();
    assert_eq!(status, done.status);
    assert_eq!(status.label(), "Logged in to GOG as account 48151623.");
    assert_eq!(status.warning(), None);

    let out = logout_with(&paths, &keyring).unwrap();
    assert_eq!(out.result, "Removed the GOG login.");
    assert_eq!(status_with(&paths, &keyring).unwrap(), Status::LoggedOut);
    let again = logout_with(&paths, &keyring).unwrap();
    assert_eq!(again.result, "No GOG login was saved.");
}

#[test]
fn the_status_shows_the_name_stored_with_the_login_without_asking_gog() {
    let (_root, paths) = setup();
    let keyring = keyring();
    let server = Server::start(|line| {
        if line.starts_with("GET /users/") {
            reply(200, r#"{"id":"48151623","username":"Hunter42"}"#)
        } else {
            tokens("REFRESH1", 48151623)
        }
    });
    let done = login_with(&paths, &keyring, server.client(), &pasted()).unwrap();
    assert_eq!(done.status.label(), "Logged in to GOG as Hunter42.");
    let requests = server.requests().len();

    let status = status_with(&paths, &keyring).unwrap();
    assert_eq!(status, done.status);
    assert_eq!(status.label(), "Logged in to GOG as Hunter42.");
    assert_eq!(
        server.requests().len(),
        requests,
        "a status check is offline"
    );
}

#[test]
fn the_account_row_shows_the_name_in_place_of_the_number() {
    let row = Settings {
        status: Some(Status::LoggedIn {
            user_id: 9,
            username: Some("Hunter42".to_owned()),
            plain_text: None,
        }),
        ..Settings::default()
    };
    let texts = super::super::view::gog_texts(&row);
    assert_eq!(texts[1], "Logged in to GOG as Hunter42.");
    assert!(!texts.iter().any(|text| text.contains("account 9")));
}

#[test]
fn without_a_keyring_the_login_is_plain_text_with_a_warning() {
    let (_root, paths) = setup();
    let server = Server::start(|_| tokens("REFRESH1", 7));
    let done = login_with(&paths, &NoKeyring, server.client(), &pasted()).unwrap();
    let warning = done.status.warning().unwrap();
    assert!(warning.contains("plain text"), "{warning}");
    assert!(warning.contains(&key_store::key_file(&KEY_SPEC, &paths).display().to_string()));
    assert_eq!(status_with(&paths, &NoKeyring).unwrap(), done.status);
}

#[test]
fn failed_logins_never_show_the_pasted_address() {
    let (_root, paths) = setup();
    let keyring = keyring();
    let rejected = Server::start(|_| {
        reply(
            400,
            r#"{"error":"invalid_grant","error_description":"bad code"}"#,
        )
    });
    let down = Server::start(|_| reply(503, "{}"));
    let errors = [
        login_with(&paths, &keyring, rejected.client(), &pasted()).unwrap_err(),
        login_with(&paths, &keyring, down.client(), &pasted()).unwrap_err(),
        login_with(
            &paths,
            &keyring,
            rejected.client(),
            &format!("https://example.com/?code={CODE}"),
        )
        .unwrap_err(),
    ];
    assert_eq!(
        errors[0],
        "GOG didn't accept that login. Log in again and paste the new address."
    );
    for error in &errors {
        assert!(!error.contains(CODE), "{error}");
        assert!(!error.contains("on_login_success"), "{error}");
    }
    assert_eq!(status_with(&paths, &keyring).unwrap(), Status::LoggedOut);
}

#[test]
fn an_unreadable_login_is_an_error_status() {
    let (_root, paths) = setup();
    let keyring = TestKeyring(RefCell::new(Some("not a login".to_owned())));
    let error = status_with(&paths, &keyring).unwrap_err();
    assert!(error.contains("can't be read"), "{error}");
    assert!(!error.contains("not a login"));
}

fn context() -> Context {
    Context {
        generation: 1,
        idle: true,
    }
}

#[test]
fn the_row_takes_the_address_and_forgets_it() {
    let (_root, paths) = setup();
    let mut row = Settings::default();
    let _ = row.update(&paths, context(), Event::LoginOpen);
    assert_eq!(row.busy, Some(Busy::Opening));
    let session = row.busy_sequence.unwrap();
    let url = "https://auth.gog.com/auth?client_id=1".to_owned();
    let _ = row.update(
        &paths,
        context(),
        Event::LoginReady(1, session, Ok(url.clone())),
    );
    assert!(row.input_open);
    assert_eq!(row.login_url.as_deref(), Some(url.as_str()));
    let texts = super::super::view::gog_texts(&row);
    assert!(texts.contains(&url));
    assert!(texts.contains(&"Address".to_owned()));

    let _ = row.update(&paths, context(), Event::LoginText(Secret::new(pasted())));
    assert!(!format!("{row:?}").contains(CODE));
    assert!(
        !super::super::view::gog_texts(&row)
            .iter()
            .any(|text| text.contains(CODE))
    );
    let _ = row.update(&paths, context(), Event::Login);
    assert_eq!(row.busy, Some(Busy::LoggingIn));
    assert_eq!(row.input.as_str(), "");
    let sequence = row.busy_sequence.unwrap();
    let event = Event::LoggedIn(
        1,
        sequence,
        Some(session),
        Err("GOG didn't accept that login. Log in again and paste the new address.".to_owned()),
    );
    assert!(!format!("{event:?}").contains(CODE));
    let _ = row.update(&paths, context(), event);
    // A failed login keeps the form for another try, empty.
    assert!(row.input_open);
    assert_eq!(row.input.as_str(), "");
    assert!(row.error.as_deref().unwrap().contains("didn't accept"));

    let _ = row.update(&paths, context(), Event::LoginText(Secret::new(pasted())));
    let _ = row.update(&paths, context(), Event::Login);
    let sequence = row.busy_sequence.unwrap();
    let _ = row.update(
        &paths,
        context(),
        Event::LoggedIn(
            1,
            sequence,
            Some(session),
            Ok(LoggedIn {
                status: Status::LoggedIn {
                    user_id: 9,
                    username: None,
                    plain_text: None,
                },
            }),
        ),
    );
    assert!(!row.input_open);
    assert!(row.login_url.is_none());
    let texts = super::super::view::gog_texts(&row);
    assert_eq!(texts[1], "Logged in to GOG as account 9.");
    // The Account row says it, so no result line repeats it.
    assert!(row.result.is_none());
    assert_eq!(
        texts
            .iter()
            .filter(|text| text.starts_with("Logged in to GOG"))
            .count(),
        1,
        "{texts:?}"
    );
    assert!(texts.contains(&"Log out".to_owned()));
    assert!(row.logged_in());
}

#[test]
fn the_row_shows_logged_out_and_error_states() {
    let (_root, paths) = setup();
    let mut row = Settings::default();
    assert_eq!(super::super::view::gog_texts(&row)[1], "Checking...");
    let sequence = row.sequence;
    let _ = row.update(
        &paths,
        context(),
        Event::StatusLoaded(1, sequence, Ok(Status::LoggedOut)),
    );
    let texts = super::super::view::gog_texts(&row);
    assert_eq!(texts[1], "Not logged in");
    assert!(!row.logged_in());
    // Nobody to log out, so only Log in is offered.
    assert!(texts.contains(&"Log in".to_owned()));
    assert!(!texts.contains(&"Log out".to_owned()));

    let _ = row.update(
        &paths,
        context(),
        Event::StatusLoaded(
            1,
            sequence,
            Err(
                "The saved GOG login can't be read. Log in again: wineo import gog --login."
                    .to_owned(),
            ),
        ),
    );
    let texts = super::super::view::gog_texts(&row);
    assert!(texts.iter().any(|text| text.contains("can't be read")));

    // A stale status never lands.
    let _ = row.update(
        &paths,
        context(),
        Event::StatusLoaded(
            0,
            sequence,
            Ok(Status::LoggedIn {
                user_id: 1,
                username: None,
                plain_text: None,
            }),
        ),
    );
    assert_eq!(row.status, Some(Status::LoggedOut));
}

#[test]
fn log_in_and_log_out_each_show_only_when_they_apply() {
    let (_root, paths) = setup();
    let mut row = Settings::default();
    // Still checking: nothing to log in to or out of yet.
    assert!(!row.shows_logout());
    assert!(!row.shows_login());
    assert!(!super::super::view::gog_texts(&row).contains(&"Log out".to_owned()));
    let sequence = row.sequence;
    let _ = row.update(
        &paths,
        context(),
        Event::StatusLoaded(
            1,
            sequence,
            Ok(Status::LoggedIn {
                user_id: 9,
                username: None,
                plain_text: None,
            }),
        ),
    );
    assert!(row.shows_logout());
    assert!(super::super::view::gog_texts(&row).contains(&"Log out".to_owned()));
    assert!(!row.shows_login());
    assert!(!super::super::view::gog_texts(&row).contains(&"Log in".to_owned()));
    let _ = row.update(&paths, context(), Event::Logout);
    assert_eq!(row.busy, Some(Busy::LoggingOut));
    assert!(super::super::view::gog_texts(&row).contains(&"Log out".to_owned()));
    // Even if the status were already gone, the button stays while it runs.
    row.status = Some(Status::LoggedOut);
    assert!(row.shows_logout());
    assert!(row.shows_login());

    // A failed check with nothing known still offers a way in.
    let mut row = Settings {
        status_error: Some("GOG login check failed".to_owned()),
        ..Settings::default()
    };
    assert!(row.shows_login());
    row.status = Some(Status::LoggedOut);
    assert!(super::super::view::gog_texts(&row).contains(&"Log in".to_owned()));
}

#[test]
fn a_busy_or_locked_page_starts_nothing() {
    let (_root, paths) = setup();
    let mut row = Settings::default();
    let locked = Context {
        generation: 1,
        idle: false,
    };
    let _ = row.update(&paths, locked, Event::LoginOpen);
    let _ = row.update(&paths, locked, Event::Logout);
    assert_eq!(row.busy, None);
}
