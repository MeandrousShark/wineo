//! Which copy of the selected game the cursor is on.

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use iced::{Point, Rectangle, Size, Vector};
use wineo_core::runtime::State;

use super::{Cursor, cursor};
use crate::grid_motion::{CardBounds, GridLayout, Places, ViewLayout};
use crate::library_actions::Flag;
use crate::slot::{SectionKey, SlotId};
use crate::tests::{card, settle_grid};
use crate::{App, Message};

fn flat(name: &str) -> SlotId {
    SlotId::new(SectionKey::All, name)
}

fn other(name: &str) -> SlotId {
    SlotId::new(SectionKey::Test("rpg"), name)
}

fn places(ids: &[SlotId]) -> Places {
    let layout = GridLayout::default();
    Places::new(
        ids.to_vec(),
        &vec![layout.poster_height(); ids.len()],
        layout.card_width(),
        3,
        0.0,
        layout,
        ViewLayout::Grid,
    )
}

#[test]
fn the_cursor_is_its_copy_else_the_selected_games_first() {
    let shown = places(&[flat("hades"), other("hades"), flat("celeste")]);
    assert_eq!(cursor(&shown, None, None), None);
    assert_eq!(cursor(&shown, Some("hades"), None), Some(0));
    assert_eq!(
        cursor(&shown, Some("hades"), Some(&other("hades"))),
        Some(1)
    );
    assert_eq!(cursor(&shown, Some("hades"), Some(&flat("hades"))), Some(0));
    // A copy of another game, or one no longer shown, is stale.
    assert_eq!(
        cursor(&shown, Some("celeste"), Some(&other("hades"))),
        Some(2)
    );
    assert_eq!(
        cursor(&shown, Some("celeste"), Some(&other("celeste"))),
        Some(2)
    );
    assert_eq!(cursor(&shown, Some("absent"), None), None);
    // The first copy is the first in shown order, whatever its section.
    let rpg_first = places(&[other("hades"), flat("hades")]);
    assert_eq!(cursor(&rpg_first, Some("hades"), None), Some(0));
}

fn fixture(count: usize) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let mut app = App::boot(paths).0;
    let cards = (0..count)
        .map(|index| card(&format!("game-{index:02}"), State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    (root, app)
}

fn key(app: &mut App, named: Named) {
    let _ = app.update(Message::KeyWithFocus(
        Key::Named(named),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
}

#[test]
fn a_press_puts_the_cursor_on_its_slot_and_a_marking_press_doesnt() {
    let (_root, mut app) = fixture(6);
    let _ = app.update(Message::Press(flat("game-03")));
    assert_eq!(app.selected.as_deref(), Some("game-03"));
    assert_eq!(app.card_cursor(), Some(&flat("game-03")));
    assert_eq!(app.selected_slot(), Some(flat("game-03")));
    // With nothing marked yet, a range runs from the cursor's slot.
    app.modifiers = Modifiers::SHIFT;
    let _ = app.update(Message::Press(flat("game-05")));
    app.modifiers = Modifiers::CTRL;
    let _ = app.update(Message::Press(flat("game-01")));
    app.modifiers = Modifiers::empty();
    assert_eq!(app.card_cursor(), Some(&flat("game-03")));
    assert_eq!(app.selected.as_deref(), Some("game-03"));
    let marked: Vec<_> = app.marked.iter().map(String::as_str).collect();
    assert_eq!(marked, ["game-01", "game-03", "game-04", "game-05"]);
}

#[test]
fn arrows_move_the_cursor_from_its_copy_or_the_first() {
    let (_root, mut app) = fixture(6);
    let _ = app.update(Message::Press(flat("game-02")));
    key(&mut app, Named::ArrowRight);
    assert_eq!(app.selected.as_deref(), Some("game-03"));
    assert_eq!(app.card_cursor(), Some(&flat("game-03")));
    // A copy the grid doesn't show falls back to the game's first.
    app.cursor = Some(Cursor::Card(other("game-03")));
    assert_eq!(app.selected_slot(), Some(flat("game-03")));
    key(&mut app, Named::ArrowRight);
    assert_eq!(app.card_cursor(), Some(&flat("game-04")));
    // With nothing selected the first slot takes the cursor.
    app.selected = None;
    app.cursor = None;
    key(&mut app, Named::ArrowRight);
    assert_eq!(app.card_cursor(), Some(&flat("game-00")));
}

#[test]
fn another_copy_of_the_open_games_card_moves_the_cursor_and_keeps_the_panel() {
    let (_root, mut app) = fixture(4);
    let _ = app.update(Message::Press(flat("game-01")));
    assert!(app.panel_open);
    let _ = app.update(Message::Press(other("game-01")));
    assert!(app.panel_open);
    assert_eq!(app.selected.as_deref(), Some("game-01"));
    assert_eq!(app.card_cursor(), Some(&other("game-01")));
    // Selecting the game by name, as a double-click does, keeps the copy.
    let _ = app.update(Message::Select("game-01".into()));
    assert_eq!(app.card_cursor(), Some(&other("game-01")));
    // The copy the cursor is on closes it, as a card always has.
    let _ = app.update(Message::Press(flat("game-01")));
    let _ = app.update(Message::Press(flat("game-01")));
    assert!(!app.panel_open);
}

#[test]
fn selecting_another_game_by_any_path_forgets_the_copy() {
    let (_root, mut app) = fixture(4);
    let _ = app.update(Message::Press(other("game-01")));
    assert_eq!(app.card_cursor(), Some(&other("game-01")));
    let _ = app.update(Message::OpenCard("game-02".into()));
    assert_eq!(app.card_cursor(), None);
    // Coming back to the game starts on its first copy.
    let _ = app.update(Message::OpenCard("game-01".into()));
    assert_eq!(app.selected_slot(), Some(flat("game-01")));
}

fn below(app: &App) -> CardBounds {
    CardBounds {
        card: Rectangle::new(Point::new(32.0, 5000.0), Size::new(160.0, 300.0)),
        viewport: Rectangle::new(Point::new(0.0, 50.0), Size::new(app.viewport_width, 600.0)),
        offset: Vector::ZERO,
    }
}

#[test]
fn a_scroll_finds_the_cursors_copy_and_ignores_a_reply_for_another() {
    let (_root, mut app) = fixture(40);
    let _ = app.update(Message::Press(flat("game-30")));
    let _ = settle_grid(&mut app);
    let task = app.scroll_selection();
    let asked: Vec<_> = crate::sort_control::tests::answers(&app, task)
        .into_iter()
        .filter_map(|message| match message {
            Message::GridCardLocated(slot, ..) => Some(slot),
            _ => None,
        })
        .collect();
    assert_eq!(asked, [flat("game-30")]);
    let offset = app.scroll_offset;
    let revision = app.grid_revision();
    let _ = app.update(Message::GridCardLocated(
        other("game-30"),
        revision,
        below(&app),
    ));
    assert_eq!(app.scroll_offset, offset);
    let _ = app.update(Message::GridCardLocated(
        flat("game-30"),
        revision,
        below(&app),
    ));
    assert_ne!(app.scroll_offset, offset);
}

#[test]
fn hiding_the_selected_game_moves_the_cursor_to_the_next_slot() {
    let (_root, mut app) = fixture(5);
    let _ = app.update(Message::Press(flat("game-02")));
    let _ = app.toggle_flag("game-02", Flag::Hidden);
    assert_eq!(app.selected.as_deref(), Some("game-03"));
    assert_eq!(app.card_cursor(), Some(&flat("game-03")));
}

// The grid below shows one game twice, in tags "a" and "b"; see
// `crate::tests::show_copy`.

fn in_a(name: &str) -> SlotId {
    SlotId::new(SectionKey::Tag("a".into()), name)
}

fn in_b(name: &str) -> SlotId {
    SlotId::new(SectionKey::Tag("b".into()), name)
}

#[test]
fn hiding_a_later_copy_counts_the_earlier_copy_that_leaves_too() {
    // game-00, game-01, game-00 again, game-02, game-03, game-04, game-05
    let (_root, mut app) = fixture(6);
    let copy = crate::tests::show_copy(&mut app, "game-00", 2);
    let _ = app.update(Message::Press(copy.clone()));
    assert_eq!(app.selected_slot(), Some(copy));
    let _ = app.toggle_flag("game-00", Flag::Hidden);
    // The first card after the cursor that stays.
    assert_eq!(app.selected.as_deref(), Some("game-02"));
    assert_eq!(app.card_cursor(), Some(&in_b("game-02")));
}

#[test]
fn a_bulk_hide_counts_every_leaving_copy_before_the_cursor() {
    // game-00, game-01, game-02, game-00 again, game-03, game-04
    let (_root, mut app) = fixture(5);
    let copy = crate::tests::show_copy(&mut app, "game-00", 3);
    let _ = app.update(Message::Press(copy));
    app.marked.insert("game-00".into());
    app.marked.insert("game-01".into());
    let _ = app.flag_marked(Flag::Hidden, true);
    assert_eq!(app.selected.as_deref(), Some("game-03"));
    assert_eq!(app.card_cursor(), Some(&in_b("game-03")));
}

#[test]
fn arrows_walk_across_both_copies_of_a_game() {
    // game-00, game-00 again, game-01, ...
    let (_root, mut app) = fixture(4);
    let copy = crate::tests::show_copy(&mut app, "game-00", 1);
    let _ = app.update(Message::Press(in_a("game-00")));
    assert!(app.panel_open);
    key(&mut app, Named::ArrowRight);
    assert_eq!(app.selected.as_deref(), Some("game-00"));
    assert_eq!(app.card_cursor(), Some(&copy));
    assert_eq!(app.selected_slot(), Some(copy.clone()));
    assert!(app.panel_open);
    key(&mut app, Named::ArrowRight);
    assert_eq!(app.card_cursor(), Some(&in_b("game-01")));
    key(&mut app, Named::ArrowLeft);
    assert_eq!(app.selected_slot(), Some(copy));
    key(&mut app, Named::ArrowLeft);
    assert_eq!(app.selected_slot(), Some(in_a("game-00")));
    assert!(app.panel_open);
}

#[test]
fn a_scroll_reply_for_the_other_shown_copy_is_ignored() {
    let (_root, mut app) = fixture(40);
    let copy = crate::tests::show_copy(&mut app, "game-30", 36);
    let _ = app.update(Message::Press(copy.clone()));
    let _ = settle_grid(&mut app);
    let task = app.scroll_selection();
    let asked: Vec<_> = crate::sort_control::tests::answers(&app, task)
        .into_iter()
        .filter_map(|message| match message {
            Message::GridCardLocated(slot, ..) => Some(slot),
            _ => None,
        })
        .collect();
    assert_eq!(asked, std::slice::from_ref(&copy));
    let offset = app.scroll_offset;
    let revision = app.grid_revision();
    let _ = app.update(Message::GridCardLocated(
        in_a("game-30"),
        revision,
        below(&app),
    ));
    assert_eq!(app.scroll_offset, offset);
    let _ = app.update(Message::GridCardLocated(copy, revision, below(&app)));
    assert_ne!(app.scroll_offset, offset);
}

#[test]
fn a_layout_switch_keeps_the_cursors_copy_in_place() {
    let (_root, mut app) = fixture(40);
    let copy = crate::tests::show_copy(&mut app, "game-05", 6);
    let _ = app.update(Message::Press(copy.clone()));
    let _ = settle_grid(&mut app);
    // The first copy is partly in view too, and isn't the one kept.
    let places = app.grid_places();
    app.scroll_offset = places.slots[5].y + 10.0;
    let (anchor, reveal) = app.find_anchor();
    let (slot, offset) = anchor.expect("an anchor");
    assert_eq!(slot, copy);
    assert!(!reveal);
    let _ = app.update(Message::Layout(crate::layout_switch::Event::Set(
        ViewLayout::List,
    )));
    let places = app.grid_places();
    let at = places.index(&copy).unwrap();
    assert_eq!(places.slots[at].y - app.scroll_offset, offset.max(0.0));
}
