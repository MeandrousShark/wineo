use std::fs;
use std::path::PathBuf;

use wineo_core::epic::Version;

use super::*;
use crate::epic_fake::{CODE, Epic};
use crate::epic_legendary::{Event as OfferEvent, Failure};
use crate::theme::Palette;

fn context() -> Context {
    Context {
        generation: 1,
        idle: true,
    }
}

fn texts(row: &Settings) -> Vec<String> {
    crate::layout_probe::texts(
        super::super::view::epic_row(row, false, Palette::DARK),
        iced::Size::new(700.0, 600.0),
    )
}

fn missing() -> Missing {
    Missing {
        older_on_path: Some((PathBuf::from("/usr/bin/legendary"), Version::new(0, 20, 34))),
    }
}

fn found(epic: &Epic) -> Found {
    legendary::find(&epic.paths, &Token::new()).unwrap()
}

#[test]
fn the_status_says_who_is_logged_in_and_which_legendary_runs() {
    let epic = Epic::new();
    let token = Token::new();
    let status = status_with(&epic.paths, &token).unwrap();
    assert_eq!(status.label(), "Not logged in");
    assert_eq!(
        status.legendary_label(),
        "Legendary 0.21.1, from [epic] legendary (~/bin/legendary)"
    );
    assert_eq!(login_with(&epic.paths, CODE, &token).unwrap(), "REDACTED");
    let status = status_with(&epic.paths, &token).unwrap();
    assert_eq!(status.label(), "Logged in to Epic as REDACTED.");
    assert_eq!(
        logout_with(&epic.paths, &token).unwrap(),
        "Removed the Epic login."
    );
    assert!(!epic.logged_in());
    assert_eq!(
        logout_with(&epic.paths, &token).unwrap(),
        "No Epic login was saved."
    );
    assert!(epic.sweep().is_empty());
}

#[test]
fn without_legendary_the_status_says_what_log_in_offers_and_logout_still_works() {
    let epic = Epic::new();
    let token = Token::new();
    let status = status_from(&epic.paths, Found::Offer(missing()), &token).unwrap();
    assert_eq!(status.label(), "Not logged in");
    assert_eq!(
        status.legendary_label(),
        "Not installed. Log in offers to download it."
    );
    epic.log_in();
    let status = status_from(&epic.paths, Found::Offer(missing()), &token).unwrap();
    assert_eq!(
        status.label(),
        "A login is saved, but no Legendary can read it."
    );
    let python = wineo_core::epic::Error::Python.to_string();
    let status = status_from(&epic.paths, Found::Unusable(python.clone()), &token).unwrap();
    assert_eq!(status.legendary_label(), python);
    // No Legendary runs: wineo removes the saved login itself.
    assert_eq!(
        logout_from(&epic.paths, Found::Unusable(python), &token).unwrap(),
        "Removed the Epic login."
    );
    assert!(!epic.logged_in());
    assert!(epic.calls().is_empty());
}

#[test]
fn a_rejected_code_says_so_and_nothing_pasted_is_ever_shown() {
    let epic = Epic::new();
    let token = Token::new();
    let wrong = "0123456789SECRETwrong";
    let error = login_with(&epic.paths, wrong, &token).unwrap_err();
    assert!(
        error.starts_with("Epic didn't accept that login"),
        "{error}"
    );
    assert!(!error.contains(wrong));
    let error = login_with(&epic.paths, "not a code!", &token).unwrap_err();
    assert_eq!(
        error,
        "That isn't an Epic login code. Paste the code, or the whole page Epic showed."
    );
    let page = format!("{{\"authorizationCode\": \"{CODE}\"}}");
    assert_eq!(login_with(&epic.paths, &page, &token).unwrap(), "REDACTED");
    assert!(epic.calls().contains(&"auth --code <code>".to_owned()));
    assert!(epic.sweep().is_empty());
}

#[test]
fn opening_asks_the_browser_only_when_legendary_is_there() {
    let epic = Epic::new();
    let mut opened = Vec::new();
    let ready = ready_from(found(&epic), |url| opened.push(url.to_owned())).unwrap();
    assert!(matches!(ready, Ready::Open));
    assert_eq!(opened, [LOGIN_URL]);
    let mut opened = Vec::new();
    let ready = ready_from(Found::Offer(missing()), |url| opened.push(url.to_owned())).unwrap();
    assert!(matches!(ready, Ready::Offer(_)));
    let error = ready_from(Found::Unusable("no python".to_owned()), |url| {
        opened.push(url.to_owned())
    })
    .unwrap_err();
    assert_eq!(error, "no python");
    assert!(opened.is_empty());
    assert!(epic.sweep().is_empty());
}

#[test]
fn the_form_takes_the_code_and_forgets_it() {
    let epic = Epic::new();
    let paths = &epic.paths;
    let mut row = Settings::default();
    let _ = row.update(paths, context(), Event::LoginOpen);
    assert_eq!(row.busy, Some(Busy::Opening));
    let session = row.input_session.unwrap();
    let opening = row.busy_sequence.unwrap();
    assert!(texts(&row).contains(&"Opening Epic's login page...".to_owned()));
    let _ = row.update(
        paths,
        context(),
        Event::LoginReady(1, opening, Ok(Ready::Open)),
    );
    assert!(row.input_open);
    let shown = texts(&row);
    for line in [
        "Log in to Epic in your browser. If it doesn't open, go to:",
        LOGIN_URL,
        "Then paste what the page shows, or only its authorizationCode.",
        "Copy link",
        "Log in",
        "Cancel",
    ] {
        assert!(shown.contains(&line.to_owned()), "{line}: {shown:?}");
    }
    let _ = row.update(
        paths,
        context(),
        Event::LoginText(Secret::new(CODE.to_owned())),
    );
    assert!(!format!("{row:?}").contains(CODE));
    assert!(!texts(&row).iter().any(|text| text.contains(CODE)));
    let _ = row.update(paths, context(), Event::Login);
    assert_eq!(row.busy, Some(Busy::LoggingIn));
    assert_eq!(row.input.as_str(), "");
    let sequence = row.busy_sequence.unwrap();
    let _ = row.update(
        paths,
        context(),
        Event::LoggedIn(
            1,
            sequence,
            Some(session),
            Err("Epic didn't accept that login. Log in again and paste the new code.".to_owned()),
        ),
    );
    // A failed login keeps the form for another try, empty.
    assert!(row.input_open);
    assert!(
        texts(&row)
            .iter()
            .any(|text| text.contains("didn't accept"))
    );

    let _ = row.update(
        paths,
        context(),
        Event::LoginText(Secret::new(CODE.to_owned())),
    );
    let _ = row.update(paths, context(), Event::Login);
    let sequence = row.busy_sequence.unwrap();
    let _ = row.update(
        paths,
        context(),
        Event::LoggedIn(1, sequence, Some(session), Ok("REDACTED".to_owned())),
    );
    assert!(!row.input_open);
    assert_eq!(row.result, None, "the Account row says who");
    let status_sequence = row.sequence;
    let _ = row.update(
        paths,
        context(),
        Event::StatusLoaded(
            1,
            status_sequence,
            Ok(Status {
                account: Account::LoggedIn("REDACTED".to_owned()),
                legendary: Program::InUse("Legendary 0.21.1, downloaded by wineo".to_owned()),
            }),
        ),
    );
    let shown = texts(&row);
    assert_eq!(
        &shown[..4],
        [
            "Account",
            "Logged in to Epic as REDACTED.",
            "Legendary",
            "Legendary 0.21.1, downloaded by wineo"
        ]
    );
    assert!(shown.contains(&"Log out".to_owned()));
    assert!(!shown.contains(&"Log in".to_owned()));
}

#[test]
fn log_in_asks_to_download_legendary_and_no_leaves_things_as_they_were() {
    let epic = Epic::new();
    let paths = &epic.paths;
    let mut row = Settings::default();
    let _ = row.update(paths, context(), Event::LoginOpen);
    let opening = row.busy_sequence.unwrap();
    let _ = row.update(
        paths,
        context(),
        Event::LoginReady(1, opening, Ok(Ready::Offer(missing()))),
    );
    assert_eq!(row.busy, None);
    let shown = texts(&row);
    assert!(shown.contains(
        &"Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it?".to_owned()
    ));
    assert!(shown.contains(
        &"The legendary on PATH (/usr/bin/legendary) is 0.20.34, older than 0.21.1.".to_owned()
    ));
    // While it asks, Log in and Log out wait.
    assert!(!shown.contains(&"Log in".to_owned()));
    let _ = row.update(paths, context(), Event::Logout);
    assert_eq!(row.busy, None);
    let _ = row.update(paths, context(), Event::Offer(0, OfferEvent::Decline));
    assert!(row.offer.is_none());
    assert_eq!(row.busy, None);
    assert_eq!(row.error, None);
    assert!(row.input_session.is_none());
}

#[test]
fn a_download_shows_progress_and_then_opens_the_login_page() {
    let epic = Epic::new();
    let paths = &epic.paths;
    let mut row = Settings {
        offer: Some(Offer::new(&missing())),
        ..Settings::default()
    };
    let _ = row.update(paths, context(), Event::Offer(0, OfferEvent::Download));
    assert_eq!(row.busy, Some(Busy::Downloading));
    let sequence = row.busy_sequence.unwrap();
    let _ = row.update(
        paths,
        context(),
        Event::Offer(sequence, OfferEvent::Progress(1, 1_048_576, 3_093_622)),
    );
    let shown = texts(&row);
    assert!(
        shown.contains(&"Downloading Legendary 0.21.1: 1 MiB of 3 MiB".to_owned()),
        "{shown:?}"
    );
    // The bar says it, so there is no spinner line besides.
    assert!(!shown.contains(&"Downloading Legendary 0.21.1...".to_owned()));
    // An end for another download is ignored.
    let _ = row.update(
        paths,
        context(),
        Event::Offer(sequence + 9, OfferEvent::Done(1, Ok(()))),
    );
    assert_eq!(row.busy, Some(Busy::Downloading));
    let _ = row.update(
        paths,
        context(),
        Event::Offer(sequence, OfferEvent::Done(1, Ok(()))),
    );
    assert!(row.offer.is_none());
    assert_eq!(
        row.busy,
        Some(Busy::Opening),
        "looks again, then opens the page"
    );
}

#[test]
fn a_failed_download_shows_why_and_a_canceled_one_shows_nothing() {
    let epic = Epic::new();
    let paths = &epic.paths;
    let message = wineo_core::epic::Error::Gone {
        version: "0.21.1".to_owned(),
    }
    .to_string();
    let mut row = Settings {
        offer: Some(Offer::new(&missing())),
        ..Settings::default()
    };
    let _ = row.update(paths, context(), Event::Offer(0, OfferEvent::Download));
    let sequence = row.busy_sequence.unwrap();
    let _ = row.update(paths, context(), Event::Offer(sequence, OfferEvent::Cancel));
    assert!(texts(&row).contains(&"Canceling...".to_owned()));
    let _ = row.update(
        paths,
        context(),
        Event::Offer(sequence, OfferEvent::Done(1, Err(Failure::Canceled))),
    );
    assert!(row.offer.is_none());
    assert_eq!((row.busy, &row.error), (None, &None));

    row.offer = Some(Offer::new(&missing()));
    let _ = row.update(paths, context(), Event::Offer(0, OfferEvent::Download));
    let sequence = row.busy_sequence.unwrap();
    let _ = row.update(
        paths,
        context(),
        Event::Offer(
            sequence,
            OfferEvent::Done(1, Err(Failure::Failed(message.clone()))),
        ),
    );
    assert!(row.offer.is_none());
    assert_eq!(row.busy, None);
    let shown = texts(&row);
    assert!(shown.contains(&message), "{shown:?}");
    assert!(message.contains("Install Legendary yourself and set [epic] legendary"));
}

#[test]
fn a_download_left_behind_lands_its_result_without_an_offer() {
    let epic = Epic::new();
    let paths = &epic.paths;
    let mut row = Settings {
        busy: Some(Busy::Downloading),
        busy_sequence: Some(7),
        ..Settings::default()
    };
    assert!(texts(&row).contains(&"Downloading Legendary 0.21.1...".to_owned()));
    let _ = row.update(paths, context(), Event::LoginOpen);
    assert_eq!(row.busy, Some(Busy::Downloading), "nothing else starts");
    let _ = row.update(
        paths,
        context(),
        Event::Offer(
            7,
            OfferEvent::Done(3, Err(Failure::Failed("gone".to_owned()))),
        ),
    );
    assert_eq!(row.busy, None);
    assert_eq!(row.error.as_deref(), Some("gone"));
}

#[test]
fn log_out_shows_only_with_a_login_saved_and_a_locked_page_starts_nothing() {
    let epic = Epic::new();
    let paths = &epic.paths;
    let mut row = Settings::default();
    assert!(!row.shows_login());
    assert!(!row.shows_logout());
    row.status = Some(Status {
        account: Account::Unread,
        legendary: Program::Missing,
    });
    assert!(row.shows_login());
    assert!(row.shows_logout());
    row.status = Some(Status {
        account: Account::LoggedOut,
        legendary: Program::Missing,
    });
    assert!(!row.shows_logout());
    let locked = Context {
        generation: 1,
        idle: false,
    };
    let _ = row.update(paths, locked, Event::LoginOpen);
    let _ = row.update(paths, locked, Event::Logout);
    assert_eq!(row.busy, None);
    // A stale status never lands.
    let _ = row.update(
        paths,
        context(),
        Event::StatusLoaded(
            0,
            row.sequence,
            Ok(Status {
                account: Account::LoggedIn("x".to_owned()),
                legendary: Program::Missing,
            }),
        ),
    );
    assert_eq!(row.status.as_ref().unwrap().account, Account::LoggedOut);
    let _ = fs::metadata(paths.global_config()).unwrap();
}

#[test]
fn log_out_is_refused_in_a_portable_session_like_the_terminals() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| std::ffi::OsString::from(&home)),
        Some(&root.path().join("portable")),
    )
    .unwrap();
    let user = wineo_core::epic::state_dir(&paths).join("user.json");
    fs::create_dir_all(user.parent().unwrap()).unwrap();
    fs::write(&user, "{}\n").unwrap();
    assert_eq!(
        logout_from(&paths, Found::Offer(missing()), &Token::new()).unwrap_err(),
        "Epic games live in the host library. Run this without --portable."
    );
    assert!(user.exists(), "the host's login stays");
}
