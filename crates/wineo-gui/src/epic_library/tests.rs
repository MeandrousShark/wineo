use std::collections::HashMap;
use std::fs;

use wineo_core::epic::install;
use wineo_core::epic::update::DlcStatus;
use wineo_core::runtime::State;

use super::*;
use crate::Message;
use crate::flow::epic_update::tests::installed;
use crate::tests::{app, card as test_card};

fn status(name: &str, state: UpdateState) -> Status {
    Status {
        environment: name.to_owned(),
        app_name: "AppOffline".to_owned(),
        title: "Offline Game".to_owned(),
        installed: Some("1.0".to_owned()),
        available: Some("0.24".to_owned()),
        state,
        applicable: true,
        reason: None,
        unfinished: None,
        dlc: Vec::<DlcStatus>::new(),
    }
}

fn epic_card(unfinished: Option<Work>) -> Option<EpicCard> {
    Some(EpicCard {
        app: "AppOffline".to_owned(),
        unfinished,
        installed_version: Some("1.0".to_owned()),
        dlc: Vec::new(),
    })
}

#[test]
fn a_card_reads_work_left_from_wineos_record() {
    let epic = installed();
    let environment = Environment::load(&epic.paths, "offline-game").unwrap();
    assert_eq!(card(&epic.paths, &environment), epic_card(None));
    // The record's version is the one Legendary installed.
    epic.set_mode("AppOffline", "hang");
    let plan = match crate::flow::epic_update::plan_with(&epic.paths, "offline-game", &Token::new())
        .unwrap()
    {
        crate::flow::epic_update::Planning::Ready(plan) => plan,
        other => panic!("{other:?}"),
    };
    let token = Token::new();
    let (paths, worker_token) = (epic.paths.clone(), token.clone());
    let worker = std::thread::spawn(move || {
        crate::flow::epic_update::update_with(&paths, &plan, install::STALL, &worker_token, |_| {})
    });
    epic.wait_started("AppOffline");
    token.cancel();
    assert!(worker.join().unwrap().is_err());
    let card = card(&epic.paths, &environment).unwrap();
    assert_eq!(card.unfinished, Some(Work::Update));
    assert_eq!(card.finish_label(), "Finish update");
    assert_eq!(
        card.interrupted_note(),
        "Its last update was interrupted. Finish it before playing."
    );
    assert!(epic.sweep().is_empty());
}

#[test]
fn badges_say_update_available_finish_update_or_finish_install() {
    let available = status("offline-game", UpdateState::Available);
    let current = status("offline-game", UpdateState::UpToDate);
    assert_eq!(
        badge(epic_card(None).as_ref(), Some(&available)),
        Some(Badge::UpdateAvailable)
    );
    assert_eq!(badge(epic_card(None).as_ref(), Some(&current)), None);
    assert_eq!(badge(epic_card(None).as_ref(), None), None);
    assert_eq!(badge(None, Some(&available)), None, "not an Epic game");
    assert_eq!(
        badge(epic_card(Some(Work::Repair)).as_ref(), Some(&available)),
        Some(Badge::FinishUpdate)
    );
    assert_eq!(
        badge(epic_card(Some(Work::Install)).as_ref(), None),
        Some(Badge::FinishInstall)
    );
    assert_eq!(
        epic_card(Some(Work::Install)).unwrap().finish_label(),
        "Finish install"
    );
    let mut held = available.clone();
    held.applicable = false;
    assert!(!updatable(&held));
    assert_eq!(status_label(&available), "Update available");
    assert_eq!(status_label(&current), "Up to date");
}

#[test]
fn a_check_landing_after_a_newer_change_keeps_it() {
    let mut state = EpicState::default();
    let asked = state.asked_all();
    state.changed("offline-game");
    let generation = state.asked("offline-game");
    assert!(state.accept_status(
        "offline-game",
        generation,
        status("offline-game", UpdateState::UpToDate)
    ));
    // The startup check, asked before the change, lands late.
    state.accept_batch(&asked, vec![status("offline-game", UpdateState::Available)]);
    assert_eq!(
        state.status("offline-game").unwrap().state,
        UpdateState::UpToDate
    );
    // A game the next check no longer reports loses its status.
    let asked = state.asked_all();
    state.accept_batch(&asked, Vec::new());
    assert!(state.status("offline-game").is_none());
}

#[test]
fn the_background_check_asks_only_when_logged_in_and_marks_cards() {
    let epic = installed();
    let batch = check_with(&epic.paths, &Token::new()).unwrap().unwrap();
    assert_eq!(batch.statuses.len(), 1);
    assert_eq!(batch.statuses[0].state, UpdateState::Available);
    let calls = epic.calls().len();
    fs::remove_file(epic.state().join("user.json")).unwrap();
    assert_eq!(check_with(&epic.paths, &Token::new()).unwrap(), None);
    assert!(
        !epic.calls()[calls..]
            .iter()
            .any(|call| call.starts_with("list")),
        "logged out asks Epic nothing"
    );
    let mut app = app();
    let mut game = test_card("offline-game", State::Inactive, 0);
    game.epic = epic_card(None);
    app.cards = vec![game];
    let asked = app.epic_updates.asked_all();
    checked_since(&mut app, &asked, Ok(Some(batch)));
    assert!(app.epic_update_action("offline-game"));
    // A failure only goes to the terminal.
    checked_since(&mut app, &HashMap::new(), Err("no network".to_owned()));
    assert!(app.error.is_none());
    assert!(epic.sweep().is_empty());

    let fresh = crate::epic_fake::Epic::new();
    assert_eq!(
        check_with(&fresh.paths, &Token::new()).unwrap(),
        None,
        "no Epic game"
    );
    assert!(fresh.calls().is_empty());
}

#[test]
fn update_is_offered_and_finish_update_replaces_play() {
    let mut app = app();
    let mut ready = test_card("ready", State::Inactive, 0);
    ready.epic = epic_card(None);
    let mut broken = test_card("broken", State::Inactive, 0);
    broken.epic = epic_card(Some(Work::Update));
    let mut current = test_card("current", State::Inactive, 0);
    current.epic = epic_card(None);
    app.cards = vec![ready, broken, current];
    assert!(!app.epic_update_action("ready"), "nothing checked yet");
    app.epic_updates
        .insert("ready", status("ready", UpdateState::Available));
    app.epic_updates
        .insert("current", status("current", UpdateState::UpToDate));
    app.epic_updates
        .insert("broken", status("broken", UpdateState::Available));
    assert!(app.epic_update_action("ready"));
    assert!(!app.epic_update_action("current"));
    assert!(
        !app.epic_update_action("broken"),
        "Finish update comes first"
    );

    let target = Some("game");
    assert!(crate::panel::play_enabled(&app.cards[0], false, target));
    assert!(!crate::panel::play_enabled(&app.cards[1], false, target));

    let panel = |app: &crate::App, index: usize| {
        crate::layout_probe::texts(app.panel(&app.cards[index]), iced::Size::new(420.0, 900.0))
    };
    let shown = panel(&app, 0);
    assert!(shown.contains(&"Update available".to_owned()), "{shown:?}");
    assert!(shown.contains(&"Play".to_owned()) || shown.contains(&"Launch".to_owned()));
    let shown = panel(&app, 1);
    assert!(shown.contains(&"Finish update".to_owned()), "{shown:?}");
    assert!(
        shown.contains(&"Its last update was interrupted. Finish it before playing.".to_owned())
    );
    assert!(!shown.contains(&"Update available".to_owned()));
    let shown = panel(&app, 2);
    assert!(shown.contains(&"Up to date".to_owned()), "{shown:?}");

    // The context menu lays out with its Update row.
    app.action_menu = Some("ready".to_owned());
    let menu = crate::layout_probe::texts(app.view(), iced::Size::new(1200.0, 800.0));
    assert!(menu.contains(&"Update".to_owned()), "{menu:?}");
    app.action_menu = None;

    // Update opens the review; a game without one can't.
    let _ = app.update(Message::EpicUpdate("current".to_owned()));
    assert!(app.flow.is_none());
    let _ = app.update(Message::EpicUpdate("broken".to_owned()));
    assert!(matches!(app.flow, Some(crate::flow::Page::EpicUpdate(_))));
}

#[test]
fn a_record_changed_elsewhere_is_seen_on_the_next_rescan_and_checked_again() {
    let mut app = app();
    let mut before = test_card("offline-game", State::Inactive, 0);
    before.epic = epic_card(None);
    app.cards = vec![before.clone()];
    app.epic_updates.insert(
        "offline-game",
        status("offline-game", UpdateState::Available),
    );
    // Nothing new: the status stays.
    app.merge(vec![before.clone()]);
    assert!(app.epic_updates.status("offline-game").is_some());
    assert!(local_changes(&app.cards, &[before.clone()]).is_empty());
    // The terminal updated it: the stale status goes, and a check is asked.
    let mut after = before.clone();
    after.epic.as_mut().unwrap().installed_version = Some("1.1".to_owned());
    assert_eq!(
        local_changes(&app.cards, &[after.clone()]),
        ["offline-game"]
    );
    let mut dlc = before.clone();
    dlc.epic.as_mut().unwrap().dlc = vec![("Dlc".to_owned(), Some("1.0".to_owned()))];
    assert_eq!(local_changes(&app.cards, &[dlc]), ["offline-game"]);
    app.merge(vec![after]);
    assert!(app.epic_updates.status("offline-game").is_none());
    // A card seen for the first time has nothing to compare.
    let mut new = test_card("new-game", State::Inactive, 0);
    new.epic = epic_card(None);
    assert!(local_changes(&app.cards, &[new]).is_empty());
}

#[test]
fn leaving_finish_install_forgets_the_cards_old_update_status() {
    let epic = crate::epic_fake::Epic::new();
    let mut app = crate::App::boot(epic.paths.clone()).0;
    app.epic_updates.insert(
        "offline-game",
        status("offline-game", UpdateState::Available),
    );
    let game = crate::flow::epic_install::Game {
        app: "AppOffline".to_owned(),
        title: "Offline Game".to_owned(),
        unfinished: true,
    };
    let (page, _) =
        crate::flow::Page::epic_finish_install(epic.paths.clone(), "offline-game".to_owned(), game);
    app.flow = Some(page);
    let generation = app.flow_generation;
    let _ = app.update(Message::Flow(generation, crate::flow::Event::Back));
    assert!(app.epic_updates.status("offline-game").is_none());
}
