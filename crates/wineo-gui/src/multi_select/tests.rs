use std::collections::BTreeSet;

use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::runtime::State;

use crate::tests::card;
use crate::{App, Message};

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    (root, App::boot(paths).0)
}

fn games(count: usize) -> Vec<crate::library::Card> {
    (0..count)
        .map(|index| card(&format!("game-{index}"), State::Inactive, 60))
        .collect()
}

/// An app showing `count` games, in the order of their names.
fn library(count: usize) -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    let _ = app.update(Message::Loaded(Ok(games(count))));
    (root, app)
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn hold(app: &mut App, modifiers: Modifiers) {
    let _ = app.update(Message::Modifiers(modifiers));
}

/// A press on `name` with `modifiers` held, as the card sends it.
fn click(app: &mut App, name: &str, modifiers: Modifiers) {
    hold(app, modifiers);
    let _ = app.update(Message::Select(name.into()));
    hold(app, Modifiers::empty());
}

/// Delivers what `task` sends back to the app, as the runtime does.
fn answer(app: &mut App, task: iced::Task<Message>) {
    for message in crate::sort_control::tests::answers(app, task) {
        let _ = app.update(message);
    }
}

fn press(app: &mut App, key: Key, modifiers: Modifiers, captured: bool) -> iced::Task<Message> {
    let status = if captured {
        iced::event::Status::Captured
    } else {
        iced::event::Status::Ignored
    };
    app.update(Message::KeyWithFocus(key, modifiers, status, false))
}

fn ctrl_a(app: &mut App, captured: bool) {
    let task = press(app, Key::Character("a".into()), Modifiers::CTRL, captured);
    answer(app, task);
}

fn esc(app: &mut App) {
    let task = press(app, Key::Named(Named::Escape), Modifiers::empty(), false);
    answer(app, task);
}

fn shown(app: &App) -> BTreeSet<String> {
    app.filtered()
        .iter()
        .map(|card| card.name.clone())
        .collect()
}

#[test]
fn ctrl_click_toggles_a_mark_and_leaves_the_selection_and_panel() {
    let (_root, mut app) = library(5);
    click(&mut app, "game-1", Modifiers::empty());
    assert!(app.panel_open);
    click(&mut app, "game-3", Modifiers::CTRL);
    assert_eq!(app.marked, names(&["game-3"]));
    assert_eq!(app.selected.as_deref(), Some("game-1"));
    assert!(app.panel_open);
    // Ctrl+click on the selected card marks it rather than closing the panel.
    click(&mut app, "game-1", Modifiers::CTRL);
    assert_eq!(app.marked, names(&["game-1", "game-3"]));
    assert!(app.panel_open);
    click(&mut app, "game-3", Modifiers::CTRL);
    assert_eq!(app.marked, names(&["game-1"]));
    assert_eq!(app.selected.as_deref(), Some("game-1"));

    // With the panel closed it stays closed.
    let (_root, mut app) = library(3);
    click(&mut app, "game-2", Modifiers::CTRL);
    assert_eq!(app.marked, names(&["game-2"]));
    assert!(app.selected.is_none());
    assert!(!app.panel_open);
}

#[test]
fn shift_click_marks_a_range_in_shown_order_and_replaces_the_last_range() {
    let (_root, mut app) = library(8);
    click(&mut app, "game-1", Modifiers::CTRL);
    click(&mut app, "game-3", Modifiers::SHIFT);
    assert_eq!(app.marked, names(&["game-1", "game-2", "game-3"]));
    // The next range from the same anchor replaces the last one.
    click(&mut app, "game-0", Modifiers::SHIFT);
    assert_eq!(app.marked, names(&["game-0", "game-1"]));
    // Ctrl+click keeps the range and moves the anchor.
    click(&mut app, "game-5", Modifiers::CTRL);
    click(&mut app, "game-7", Modifiers::SHIFT);
    assert_eq!(
        app.marked,
        names(&["game-0", "game-1", "game-5", "game-6", "game-7"])
    );
    // With Ctrl held too, the last range stays.
    click(&mut app, "game-3", Modifiers::SHIFT | Modifiers::CTRL);
    assert_eq!(
        app.marked,
        names(&[
            "game-0", "game-1", "game-3", "game-4", "game-5", "game-6", "game-7"
        ])
    );
    assert!(app.selected.is_none());
    assert!(!app.panel_open);
}

#[test]
fn shift_click_follows_the_sort_not_the_names() {
    let (_root, mut app) = library(5);
    let _ = app.update(Message::Sort(crate::sort_control::Event::FlipDirection));
    let order: Vec<String> = app.filtered().iter().map(|c| c.name.clone()).collect();
    assert_eq!(order.first().map(String::as_str), Some("game-4"));
    click(&mut app, "game-3", Modifiers::CTRL);
    click(&mut app, "game-1", Modifiers::SHIFT);
    assert_eq!(app.marked, names(&["game-1", "game-2", "game-3"]));
}

#[test]
fn shift_click_without_a_mark_starts_at_the_selection_then_at_itself() {
    let (_root, mut app) = library(6);
    click(&mut app, "game-2", Modifiers::empty());
    click(&mut app, "game-4", Modifiers::SHIFT);
    assert_eq!(app.marked, names(&["game-2", "game-3", "game-4"]));
    assert_eq!(app.selected.as_deref(), Some("game-2"));
    assert!(app.panel_open);

    let (_root, mut app) = library(6);
    click(&mut app, "game-3", Modifiers::SHIFT);
    assert_eq!(app.marked, names(&["game-3"]));
    click(&mut app, "game-5", Modifiers::SHIFT);
    assert_eq!(app.marked, names(&["game-3", "game-4", "game-5"]));
    assert!(app.selected.is_none());
}

#[test]
fn a_range_across_a_filter_never_marks_a_hidden_game() {
    let (_root, mut app) = library(5);
    app.cards[1].hidden = true;
    app.cards[2].hidden = true;
    app.reorder();
    click(&mut app, "game-0", Modifiers::CTRL);
    click(&mut app, "game-4", Modifiers::SHIFT);
    assert_eq!(app.marked, names(&["game-0", "game-3", "game-4"]));
    // Showing them again doesn't mark them either.
    let _ = app.update(Message::Filter(
        crate::filter_control::Event::ToggleShowHidden,
    ));
    assert_eq!(shown(&app).len(), 5);
    assert_eq!(app.marked, names(&["game-0", "game-3", "game-4"]));
}

#[test]
fn ctrl_a_marks_every_shown_game() {
    let (_root, mut app) = library(5);
    app.cards[2].hidden = true;
    app.reorder();
    click(&mut app, "game-1", Modifiers::empty());
    ctrl_a(&mut app, false);
    assert_eq!(app.marked, names(&["game-0", "game-1", "game-3", "game-4"]));
    assert_eq!(app.selected.as_deref(), Some("game-1"));
    assert!(app.panel_open);
}

#[test]
fn ctrl_a_the_log_or_search_takes_marks_nothing() {
    let (_root, mut app) = library(3);
    click(&mut app, "game-0", Modifiers::empty());
    app.log_view = true;
    assert!(app.panel_log_visible());
    // The focused log's editor captures Ctrl+A as its own select all.
    ctrl_a(&mut app, true);
    assert!(app.marked.is_empty());
    assert!(app.panel_log_visible());

    let (_root, mut app) = library(3);
    let task = app.update(Message::KeyWithFocus(
        Key::Character("a".into()),
        Modifiers::CTRL,
        iced::event::Status::Ignored,
        true,
    ));
    answer(&mut app, task);
    assert!(app.marked.is_empty());
}

#[test]
fn esc_closes_the_log_then_clears_marks_then_the_panel_then_the_search() {
    let (_root, mut app) = library(4);
    let _ = app.update(Message::SearchChanged("game".into()));
    click(&mut app, "game-1", Modifiers::empty());
    click(&mut app, "game-2", Modifiers::CTRL);
    app.log_view = true;
    assert!(app.panel_log_visible());

    esc(&mut app);
    assert!(!app.panel_log_visible());
    assert_eq!(app.marked, names(&["game-2"]));
    assert!(app.panel_open);

    esc(&mut app);
    assert!(app.marked.is_empty());
    assert!(app.panel_open);

    esc(&mut app);
    assert!(!app.panel_open);
    assert_eq!(app.search, "game");

    esc(&mut app);
    assert!(app.search.is_empty());
}

#[test]
fn a_filter_or_search_drops_the_marks_it_hides() {
    let (_root, mut app) = library(12);
    ctrl_a(&mut app, false);
    assert_eq!(app.marked.len(), 12);
    let _ = app.update(Message::SearchChanged("game-1".into()));
    assert_eq!(
        app.marked,
        names(&["game-1", "game-10", "game-11"]),
        "only the games the search shows keep their marks"
    );
    // Clearing the search doesn't bring them back.
    let _ = app.update(Message::SearchChanged(String::new()));
    assert_eq!(app.marked, names(&["game-1", "game-10", "game-11"]));

    let (_root, mut app) = library(3);
    app.cards[1].hidden = true;
    app.reorder();
    let _ = app.update(Message::Filter(
        crate::filter_control::Event::ToggleShowHidden,
    ));
    ctrl_a(&mut app, false);
    assert_eq!(app.marked.len(), 3);
    let _ = app.update(Message::Filter(
        crate::filter_control::Event::ToggleShowHidden,
    ));
    assert_eq!(app.marked, names(&["game-0", "game-2"]));
}

#[test]
fn a_removal_drops_only_its_mark() {
    let (_root, mut app) = library(4);
    ctrl_a(&mut app, false);
    let mut cards = games(4);
    cards.remove(2);
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert_eq!(app.marked, names(&["game-0", "game-1", "game-3"]));
}

#[test]
fn a_rescan_a_re_sort_and_a_tag_change_keep_the_marks() {
    let (_root, mut app) = library(4);
    click(&mut app, "game-1", Modifiers::CTRL);
    click(&mut app, "game-3", Modifiers::CTRL);
    let kept = names(&["game-1", "game-3"]);
    let _ = app.update(Message::Loaded(Ok(games(4))));
    assert_eq!(app.marked, kept);
    let _ = app.update(Message::Sort(crate::sort_control::Event::FlipDirection));
    assert_eq!(app.marked, kept);
    let mut cards = games(4);
    cards[1].tags = vec!["rpg".into()];
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert_eq!(app.marked, kept);
}

#[test]
fn a_ctrl_or_shift_double_click_never_launches() {
    for held in [Modifiers::CTRL, Modifiers::SHIFT] {
        let (_root, mut app) = library(3);
        hold(&mut app, held);
        let _ = app.update(Message::Select("game-1".into()));
        let _ = app.update(Message::Select("game-1".into()));
        let _ = app.update(Message::DoubleClick("game-1".into()));
        assert!(app.operation.is_none(), "{held:?}");
        assert!(!app.panel_open, "{held:?}");
    }
    // A plain double-click still launches.
    let (_root, mut app) = library(3);
    let _ = app.update(Message::Select("game-1".into()));
    let _ = app.update(Message::Select("game-1".into()));
    let _ = app.update(Message::DoubleClick("game-1".into()));
    assert!(app.operation.is_some());
}

/// Runs `task` in a window where `focus` has keyboard focus.
fn answers_with_focus(app: &App, task: iced::Task<Message>, focus: &'static str) -> Vec<Message> {
    use iced::futures::StreamExt;
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    laid.run(Box::new(
        iced::advanced::widget::operation::focusable::focus(iced::widget::Id::new(focus)),
    ));
    assert_eq!(laid.focused(), vec![iced::widget::Id::new(focus)]);
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let mut messages = Vec::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        while let Ok(Some(action)) =
            tokio::time::timeout(std::time::Duration::from_millis(500), stream.next()).await
        {
            match action {
                iced_runtime::Action::Output(message) => messages.push(message),
                iced_runtime::Action::Widget(operation) => laid.run(operation),
                _ => {}
            }
        }
    });
    messages
}

#[test]
fn space_marks_the_selected_card_only_when_no_control_has_focus() {
    let (_root, mut app) = library(3);
    click(&mut app, "game-1", Modifiers::empty());
    let space = || Key::Named(Named::Space);

    let task = press(&mut app, space(), Modifiers::empty(), false);
    answer(&mut app, task);
    assert_eq!(app.marked, names(&["game-1"]));
    let task = press(&mut app, space(), Modifiers::empty(), false);
    answer(&mut app, task);
    assert!(app.marked.is_empty());

    // A focused button that took the press captured it.
    let task = press(&mut app, space(), Modifiers::empty(), true);
    answer(&mut app, task);
    assert!(app.marked.is_empty());

    // One that didn't, like a held key's repeat, still keeps it.
    let task = press(&mut app, space(), Modifiers::empty(), false);
    let messages = answers_with_focus(&app, task, crate::random_pick::BUTTON_ID);
    assert!(
        matches!(messages.as_slice(), [Message::SpaceFocus(true)]),
        "{messages:?}"
    );
    for message in messages {
        let _ = app.update(message);
    }
    assert!(app.marked.is_empty());
    assert_eq!(app.selected.as_deref(), Some("game-1"));
}

#[test]
fn space_without_a_shown_selection_marks_nothing() {
    let (_root, mut app) = library(3);
    let task = press(
        &mut app,
        Key::Named(Named::Space),
        Modifiers::empty(),
        false,
    );
    answer(&mut app, task);
    assert!(app.marked.is_empty());
}

#[test]
fn a_plain_click_clears_the_marks_and_selects_as_before() {
    let (_root, mut app) = library(4);
    click(&mut app, "game-0", Modifiers::CTRL);
    click(&mut app, "game-2", Modifiers::SHIFT);
    assert_eq!(app.marked.len(), 3);
    click(&mut app, "game-3", Modifiers::empty());
    assert!(app.marked.is_empty());
    assert_eq!(app.selected.as_deref(), Some("game-3"));
    assert!(app.panel_open);
    // A second plain click on the selected card closes the panel, as ever.
    click(&mut app, "game-3", Modifiers::empty());
    assert!(!app.panel_open);
    // The anchor went with the marks.
    click(&mut app, "game-1", Modifiers::SHIFT);
    assert_eq!(app.marked, names(&["game-1", "game-2", "game-3"]));
}

#[test]
fn ctrl_enter_opens_the_panel_and_keeps_the_marks() {
    let (_root, mut app) = library(3);
    let task = press(
        &mut app,
        Key::Named(Named::ArrowRight),
        Modifiers::empty(),
        false,
    );
    answer(&mut app, task);
    assert_eq!(app.selected.as_deref(), Some("game-0"));
    click(&mut app, "game-2", Modifiers::CTRL);
    hold(&mut app, Modifiers::CTRL);
    let task = press(&mut app, Key::Named(Named::Enter), Modifiers::CTRL, false);
    answer(&mut app, task);
    assert!(app.panel_open);
    assert_eq!(app.selected.as_deref(), Some("game-0"));
    assert_eq!(app.marked, names(&["game-2"]));
}
