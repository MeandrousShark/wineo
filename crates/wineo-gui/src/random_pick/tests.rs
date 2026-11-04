use std::collections::BTreeSet;

use iced::keyboard::{Key, Modifiers};
use wineo_core::runtime::State;

use super::*;
use crate::keys::{Action, action};
use crate::tests::card;

fn entry(name: &str, launchable: bool) -> Entry<'_> {
    Entry { name, launchable }
}

/// What `choose` gives for each of `seeds`.
fn picks<'a>(
    shown: &[Entry<'a>],
    current: Option<&str>,
    seeds: std::ops::Range<u64>,
) -> BTreeSet<&'a str> {
    seeds
        .filter_map(|seed| choose(shown, current, &mut fastrand::Rng::with_seed(seed)))
        .collect()
}

#[test]
fn a_seeded_pick_is_repeatable_and_reaches_every_game() {
    let shown = [entry("a", true), entry("b", true), entry("c", true)];
    for seed in 0..20 {
        let first = choose(&shown, None, &mut fastrand::Rng::with_seed(seed));
        let again = choose(&shown, None, &mut fastrand::Rng::with_seed(seed));
        assert_eq!(first, again);
        let index = fastrand::Rng::with_seed(seed).usize(..shown.len());
        assert_eq!(first, Some(shown[index].name));
    }
    assert_eq!(picks(&shown, None, 0..100), BTreeSet::from(["a", "b", "c"]));
}

#[test]
fn cards_that_cannot_launch_are_skipped() {
    let shown = [entry("a", false), entry("b", true), entry("c", false)];
    assert_eq!(picks(&shown, None, 0..100), BTreeSet::from(["b"]));
}

#[test]
fn any_card_is_a_candidate_when_none_can_launch() {
    let shown = [entry("a", false), entry("b", false), entry("c", false)];
    assert_eq!(picks(&shown, None, 0..100), BTreeSet::from(["a", "b", "c"]));
}

#[test]
fn the_current_selection_is_never_picked_while_there_is_a_choice() {
    let shown = [entry("a", true), entry("b", true), entry("c", true)];
    assert_eq!(picks(&shown, Some("a"), 0..100), BTreeSet::from(["b", "c"]));
    // Among the fallback cards too.
    let shown = [entry("a", false), entry("b", false)];
    assert_eq!(picks(&shown, Some("a"), 0..100), BTreeSet::from(["b"]));
}

#[test]
fn the_current_selection_is_kept_when_it_is_the_only_choice() {
    let shown = [entry("a", true)];
    assert_eq!(picks(&shown, None, 0..10), BTreeSet::from(["a"]));
    assert_eq!(picks(&shown, Some("a"), 0..10), BTreeSet::from(["a"]));
    // The only card that can launch is the current one, so a card that
    // can't launch doesn't take its place.
    let shown = [entry("a", true), entry("b", false)];
    assert_eq!(picks(&shown, Some("a"), 0..10), BTreeSet::from(["a"]));
}

#[test]
fn nothing_shown_picks_nothing() {
    assert_eq!(choose(&[], None, &mut fastrand::Rng::with_seed(1)), None);
    assert_eq!(
        choose(&[], Some("a"), &mut fastrand::Rng::with_seed(1)),
        None
    );
}

#[test]
fn a_game_shown_twice_is_drawn_once() {
    let twice = [
        entry("a", true),
        entry("a", true),
        entry("a", true),
        entry("b", true),
    ];
    let once = [entry("a", true), entry("b", true)];
    // The same draw lands on the same game, so "a" isn't three times as
    // likely.
    for seed in 0..100 {
        assert_eq!(
            choose(&twice, None, &mut fastrand::Rng::with_seed(seed)),
            choose(&once, None, &mut fastrand::Rng::with_seed(seed)),
        );
    }
}

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    (root, App::boot(paths).0)
}

/// An app showing `count` games, in the order of their names.
fn library(count: usize) -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    let cards = (0..count)
        .map(|index| card(&format!("game-{index:03}"), State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    (root, app)
}

/// Marks every card but `name` as unable to launch, so a pick has one
/// answer.
fn only_launchable(app: &mut App, name: &str) {
    for card in &mut app.cards {
        if card.name != name {
            card.error = Some("unreadable".into());
        }
    }
}

fn shown(app: &App) -> Vec<String> {
    app.filtered()
        .iter()
        .map(|card| card.name.clone())
        .collect()
}

/// Delivers what `task` sends back to the app, as the runtime does.
fn answer(app: &mut App, task: Task<Message>) {
    for message in crate::sort_control::tests::answers(app, task) {
        let _ = app.update(message);
    }
}

fn key_with(app: &mut App, key: &str, modifiers: Modifiers, focused: bool, captured: bool) {
    let status = if captured {
        iced::event::Status::Captured
    } else {
        iced::event::Status::Ignored
    };
    let task = app.update(Message::KeyWithFocus(
        Key::Character(key.into()),
        modifiers,
        status,
        focused,
    ));
    answer(app, task);
}

fn press_r(app: &mut App) {
    key_with(app, "r", Modifiers::empty(), false, false);
}

fn pick(app: &mut App) {
    let task = app.update(Message::Random(Event::Pick));
    answer(app, task);
}

fn nothing_happened(app: &App) -> bool {
    app.selected.is_none() && !app.panel_open
}

#[test]
fn r_is_only_a_key_when_search_does_not_have_it() {
    let r = Key::Character("r".into());
    let plain =
        |modifiers, search_focused| action(&r, modifiers, search_focused, false, false, true);
    assert_eq!(plain(Modifiers::empty(), false), Action::PickRandom);
    // Shift is allowed, so Caps Lock doesn't matter.
    assert_eq!(plain(Modifiers::SHIFT, false), Action::PickRandom);
    assert_eq!(plain(Modifiers::empty(), true), Action::None);
    for held in [Modifiers::CTRL, Modifiers::ALT, Modifiers::LOGO] {
        assert_eq!(plain(held, false), Action::None);
    }
    let capital = Key::Character("R".into());
    assert_eq!(
        action(&capital, Modifiers::SHIFT, false, false, false, true),
        Action::PickRandom
    );
}

#[test]
fn r_picks_a_game_and_opens_its_panel() {
    let (_root, mut app) = library(5);
    only_launchable(&mut app, "game-003");
    assert!(nothing_happened(&app));
    press_r(&mut app);
    assert_eq!(app.selected.as_deref(), Some("game-003"));
    assert!(app.panel_open);
    // Picking never launches.
    assert!(app.operation.is_none());
    assert!(app.cards.iter().all(|card| card.state == State::Inactive));
}

#[test]
fn the_button_picks_the_same_way() {
    let (_root, mut app) = library(5);
    only_launchable(&mut app, "game-002");
    let messages = crate::library_states::tests::press_control(&app, BUTTON_ID);
    assert!(matches!(
        messages.as_slice(),
        [Message::Random(Event::Pick)]
    ));
    for message in messages {
        let task = app.update(message);
        answer(&mut app, task);
    }
    assert_eq!(app.selected.as_deref(), Some("game-002"));
    assert!(app.panel_open);
}

#[test]
fn the_pick_scrolls_into_view_once_the_panel_has_docked() {
    let (_root, mut app) = library(60);
    let last = shown(&app).pop().unwrap();
    only_launchable(&mut app, &last);
    assert_eq!(app.scroll_offset, 0.0);
    pick(&mut app);
    assert_eq!(app.selected.as_deref(), Some(last.as_str()));
    assert!(app.panel_open);
    assert!(app.grid_motion.active());
    // The scroll waits for the dock's motion to end.
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some(last.as_str()));
    let task = crate::tests::settle_grid(&mut app);
    answer(&mut app, task);
    assert!(app.grid_scroll_after_settle.is_none());
    assert!(app.scroll_offset > 0.0, "the grid stayed at the top");
}

#[test]
fn a_pick_with_the_panel_already_open_scrolls_at_once() {
    let (_root, mut app) = library(60);
    let _ = app.update(Message::Select("game-000".into()));
    let _ = crate::tests::settle_grid(&mut app);
    assert!(app.panel_open);
    assert!(!app.grid_motion.active());
    let last = shown(&app).pop().unwrap();
    only_launchable(&mut app, &last);
    pick(&mut app);
    assert_eq!(app.selected.as_deref(), Some(last.as_str()));
    assert!(app.panel_open);
    assert!(app.scroll_offset > 0.0);
}

#[test]
fn a_pick_skips_cards_that_cannot_launch_by_the_panels_rule() {
    let (_root, mut app) = library(6);
    app.cards[0].error = Some("unreadable".into());
    app.cards[1].busy = true;
    app.cards[2].state = State::Running;
    app.cards[3] = crate::tests::steam_card("game-003", wineo_core::steam::AppState::NotInstalled);
    for seed in 0..50 {
        let name = app
            .random_choice(&mut fastrand::Rng::with_seed(seed))
            .unwrap();
        assert!(["game-004", "game-005"].contains(&name.as_str()), "{name}");
    }
}

#[test]
fn a_pick_falls_back_to_any_shown_card_when_none_can_launch() {
    let (_root, mut app) = library(4);
    for card in &mut app.cards {
        card.error = Some("unreadable".into());
    }
    let seen: BTreeSet<_> = (0..100)
        .filter_map(|seed| app.random_choice(&mut fastrand::Rng::with_seed(seed)))
        .collect();
    assert_eq!(seen.len(), 4);
}

#[test]
fn a_pick_never_repeats_the_selection_while_there_is_a_choice() {
    let (_root, mut app) = library(3);
    let _ = app.update(Message::Select("game-001".into()));
    let seen: BTreeSet<_> = (0..100)
        .filter_map(|seed| app.random_choice(&mut fastrand::Rng::with_seed(seed)))
        .collect();
    assert_eq!(
        seen,
        BTreeSet::from(["game-000".to_owned(), "game-002".to_owned()])
    );
}

#[test]
fn a_pick_only_draws_from_the_shown_cards() {
    let (_root, mut app) = library(4);
    app.cards[1].hidden = true;
    app.cards[2].hidden = true;
    app.reorder();
    let draws = |app: &App| -> BTreeSet<String> {
        (0..100)
            .filter_map(|seed| app.random_choice(&mut fastrand::Rng::with_seed(seed)))
            .collect()
    };
    assert_eq!(
        draws(&app),
        BTreeSet::from(["game-000".to_owned(), "game-003".to_owned()])
    );
    let _ = app.update(Message::Filter(
        crate::filter_control::Event::ToggleShowHidden,
    ));
    assert_eq!(draws(&app).len(), 4);
}

#[test]
fn the_only_card_stays_selected_and_open_when_it_is_picked_again() {
    let (_root, mut app) = library(1);
    pick(&mut app);
    assert_eq!(app.selected.as_deref(), Some("game-000"));
    assert!(app.panel_open);
    // A pick opens the panel, it never toggles it shut.
    pick(&mut app);
    assert_eq!(app.selected.as_deref(), Some("game-000"));
    assert!(app.panel_open);
    // Selected but closed, it opens.
    app.close_panel();
    assert!(!app.panel_open);
    pick(&mut app);
    assert_eq!(app.selected.as_deref(), Some("game-000"));
    assert!(app.panel_open);
}

#[test]
fn nothing_shown_disables_the_button_and_r_does_nothing() {
    let (_root, mut app) = library(3);
    let _ = app.update(Message::SearchChanged("no such game".into()));
    assert!(app.filtered().is_empty());
    assert!(crate::library_states::tests::press_control(&app, BUTTON_ID).is_empty());
    press_r(&mut app);
    assert!(nothing_happened(&app));
    // A message that gets through anyway has nothing to pick.
    pick(&mut app);
    assert!(nothing_happened(&app));

    let (_root, mut app) = library(0);
    press_r(&mut app);
    assert!(nothing_happened(&app));
}

#[test]
fn the_button_is_on_with_cards_shown() {
    let (_root, app) = library(3);
    assert_eq!(
        crate::library_states::tests::press_control(&app, BUTTON_ID).len(),
        1
    );
}

#[test]
fn r_is_typed_into_search_when_it_has_focus() {
    let (_root, mut app) = library(3);
    key_with(&mut app, "r", Modifiers::empty(), true, false);
    assert!(nothing_happened(&app));
}

#[test]
fn r_with_ctrl_alt_or_logo_is_ignored() {
    let (_root, mut app) = library(3);
    for held in [Modifiers::CTRL, Modifiers::ALT, Modifiers::LOGO] {
        key_with(&mut app, "r", held, false, false);
        assert!(nothing_happened(&app));
    }
}

#[test]
fn r_is_ignored_when_something_else_took_the_key() {
    let (_root, mut app) = library(3);
    key_with(&mut app, "r", Modifiers::empty(), false, true);
    assert!(nothing_happened(&app));
}

#[test]
fn r_is_ignored_while_a_popup_is_open() {
    let (_root, mut app) = library(3);
    let task = app.update(Message::Sort(crate::sort_control::Event::ToggleMenu));
    answer(&mut app, task);
    assert!(app.sort_menu_shown());
    press_r(&mut app);
    assert!(nothing_happened(&app));
    let _ = app.update(Message::Sort(crate::sort_control::Event::ToggleMenu));
    let task = app.update(Message::Filter(crate::filter_control::Event::ToggleMenu));
    answer(&mut app, task);
    assert!(app.filter_menu_shown());
    press_r(&mut app);
    assert!(nothing_happened(&app));
}

#[test]
fn the_pick_is_blocked_by_a_dialog() {
    let (_root, mut app) = library(3);
    app.close_prompt = true;
    pick(&mut app);
    assert!(nothing_happened(&app));
    press_r(&mut app);
    assert!(nothing_happened(&app));
}
