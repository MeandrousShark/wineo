use std::path::PathBuf;

use wineo_core::epic::Version;

use super::*;
use crate::epic_fake::Epic;

fn missing(older: bool) -> Missing {
    Missing {
        older_on_path: older
            .then(|| (PathBuf::from("/usr/bin/legendary"), Version::new(0, 20, 34))),
    }
}

fn texts(offer: &Offer) -> Vec<String> {
    crate::layout_probe::texts(offer.view(Palette::DARK), iced::Size::new(600.0, 300.0))
}

#[test]
fn the_offer_asks_in_the_terminals_words_and_names_an_older_copy() {
    let offer = Offer::new(&missing(true));
    assert_eq!(
        texts(&offer),
        [
            "The legendary on PATH (/usr/bin/legendary) is 0.20.34, older than 0.21.1.",
            "Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it?",
            "No",
            "Yes",
        ]
    );
    assert_eq!(
        texts(&Offer::new(&missing(false)))[0],
        "Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it?"
    );
}

#[test]
fn no_declines_and_yes_downloads_with_progress_and_cancel() {
    let epic = Epic::new();
    let mut offer = Offer::new(&missing(false));
    assert_eq!(
        offer.update(&epic.paths, Event::Decline).1,
        Outcome::Declined
    );

    let mut offer = Offer::new(&missing(false));
    let (_task, outcome) = offer.update(&epic.paths, Event::Download);
    // The task isn't run, so nothing is fetched.
    assert_eq!(outcome, Outcome::Pending);
    assert!(offer.token.is_some());
    assert_eq!(
        offer.stage(),
        Stage::Downloading {
            received: 0,
            size: own_copy::PINNED.size
        }
    );
    // A second yes while it downloads starts nothing.
    let _ = offer.update(&epic.paths, Event::Download);
    assert_eq!(offer.generation, 1);
    let _ = offer.update(&epic.paths, Event::Progress(1, 1_500_000, 3_093_622));
    assert_eq!(
        texts(&offer),
        ["Downloading Legendary 0.21.1: 1.4 MiB of 3 MiB", "Cancel"]
    );
    // A count from an earlier download is ignored.
    let _ = offer.update(&epic.paths, Event::Progress(0, 9, 10));
    assert_eq!(
        offer.stage(),
        Stage::Downloading {
            received: 1_500_000,
            size: 3_093_622
        }
    );
    let _ = offer.update(&epic.paths, Event::Cancel);
    assert!(offer.token.as_ref().unwrap().is_canceled());
    assert_eq!(texts(&offer), ["Canceling...", "Cancel"]);
    let (_, outcome) = offer.update(&epic.paths, Event::Done(1, Err(Failure::Canceled)));
    assert_eq!(outcome, Outcome::Declined);
    assert!(offer.token.is_none());
}

#[test]
fn a_finished_download_is_ready_and_a_failure_says_why() {
    let epic = Epic::new();
    let mut offer = Offer::new(&missing(false));
    let _ = offer.update(&epic.paths, Event::Download);
    assert_eq!(
        offer.update(&epic.paths, Event::Done(0, Ok(()))).1,
        Outcome::Pending,
        "an earlier download's end is ignored"
    );
    assert_eq!(
        offer.update(&epic.paths, Event::Done(1, Ok(()))).1,
        Outcome::Ready
    );

    let mut offer = Offer::new(&missing(false));
    let _ = offer.update(&epic.paths, Event::Download);
    let message = wineo_core::epic::Error::Mismatch {
        version: "0.21.1".to_owned(),
    }
    .to_string();
    assert_eq!(
        offer
            .update(
                &epic.paths,
                Event::Done(1, Err(Failure::Failed(message.clone())))
            )
            .1,
        Outcome::Failed(message)
    );
}

#[test]
fn finding_uses_the_setting_and_says_which_legendary_is_in_use() {
    let epic = Epic::new();
    let found = find(&epic.paths, &Token::new()).unwrap();
    let Found::Usable(legendary) = found else {
        panic!("{found:?}");
    };
    assert_eq!(
        in_use(&epic.paths, &legendary),
        "Legendary 0.21.1, from [epic] legendary (~/bin/legendary)"
    );
    assert!(epic.sweep().is_empty());
}

#[test]
fn a_setting_that_is_too_old_is_the_error_and_nothing_is_offered() {
    let epic = Epic::version("0.20.34");
    let error = find(&epic.paths, &Token::new()).unwrap_err();
    assert_eq!(
        error,
        "[epic] legendary is set to ~/bin/legendary, which is Legendary 0.20.34. wineo needs 0.21.1 or newer."
    );
    assert!(epic.sweep().is_empty());
}

#[test]
fn without_python_nothing_is_offered_and_the_terminals_reason_shows() {
    let found = offerable(missing(false), Err(wineo_core::epic::Error::Python));
    let Found::Unusable(why) = found else {
        panic!("{found:?}");
    };
    assert_eq!(why, "Epic needs Python 3.10 or newer to run Legendary.");
    let found = offerable(missing(true), Ok(PathBuf::from("/usr/bin/python3")));
    assert!(matches!(found, Found::Offer(missing) if missing.older_on_path.is_some()));
}
