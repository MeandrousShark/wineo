//! Home, End, PageUp, PageDown, and F11: the keys a controller's shoulder
//! and trigger buttons share, which work in every layout.

use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::runtime::State;

use super::*;
use crate::grid_motion::ViewLayout;
use crate::tests::card;

fn app_with(count: usize, layout: ViewLayout) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let mut app = App::boot(paths).0;
    let cards = (0..count)
        .map(|index| card(&format!("game-{index:03}"), State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    app.layout = layout;
    app.viewport_width = 1000.0;
    app.snap_grid();
    (root, app)
}

fn press_with(
    app: &mut App,
    named: Named,
    modifiers: Modifiers,
    captured: bool,
    search_focused: bool,
) {
    let status = if captured {
        iced::event::Status::Captured
    } else {
        iced::event::Status::Ignored
    };
    let task = app.update(Message::KeyWithFocus(
        Key::Named(named),
        modifiers,
        status,
        search_focused,
    ));
    for message in crate::sort_control::tests::answers(app, task) {
        let _ = app.update(message);
    }
}

fn press(app: &mut App, named: Named) {
    press_with(app, named, Modifiers::empty(), false, false);
}

fn select(app: &mut App, name: &str) {
    let _ = app.update(Message::Select(name.into()));
}

fn selected(app: &App) -> &str {
    app.selected.as_deref().expect("a selection")
}

/// The window's height so that exactly `rows` rows of the tallest slot
/// fit, with a few pixels to spare that are less than another row.
fn fit_rows(app: &mut App, rows: usize) {
    let tallest = app
        .grid_places()
        .slots
        .iter()
        .map(|slot| slot.height)
        .fold(0.0, f32::max);
    let gap = crate::grid_motion::row_gap(app.layout);
    app.viewport_height = rows as f32 * (tallest + gap) - gap + 3.0;
}

#[test]
fn the_keys_map_to_their_actions_and_wait_for_the_grid() {
    let named = |named, modifiers, search_focused| {
        action(
            &Key::Named(named),
            modifiers,
            search_focused,
            false,
            false,
            true,
        )
    };
    let none = Modifiers::empty();
    assert_eq!(named(Named::Home, none, false), Action::First);
    assert_eq!(named(Named::End, none, false), Action::Last);
    assert_eq!(
        named(Named::PageUp, none, false),
        Action::Page(Direction::Up)
    );
    assert_eq!(
        named(Named::PageDown, none, false),
        Action::Page(Direction::Down)
    );
    assert_eq!(named(Named::F11, none, false), Action::ToggleFullscreen);
    // Shift is allowed, so Caps Lock and a held Shift don't matter.
    assert_eq!(named(Named::Home, Modifiers::SHIFT, false), Action::First);
    // A focused search field keeps them for its own cursor.
    for key in [
        Named::Home,
        Named::End,
        Named::PageUp,
        Named::PageDown,
        Named::F11,
    ] {
        assert_eq!(named(key, none, true), Action::None, "{key:?}");
    }
    // Ctrl, Alt, and the logo key make other shortcuts.
    for modifiers in [Modifiers::CTRL, Modifiers::ALT, Modifiers::LOGO] {
        assert_eq!(named(Named::Home, modifiers, false), Action::None);
        assert_eq!(named(Named::PageDown, modifiers, false), Action::None);
    }
}

#[test]
fn home_and_end_go_to_the_first_and_last_game_in_the_grid() {
    let (_root, mut app) = app_with(40, ViewLayout::Grid);
    select(&mut app, "game-017");
    press(&mut app, Named::End);
    assert_eq!(selected(&app), "game-039");
    press(&mut app, Named::Home);
    assert_eq!(selected(&app), "game-000");
    // Already there.
    press(&mut app, Named::Home);
    assert_eq!(selected(&app), "game-000");
}

#[test]
fn home_and_end_go_to_the_first_and_last_game_in_the_list() {
    let (_root, mut app) = app_with(40, ViewLayout::List);
    select(&mut app, "game-017");
    press(&mut app, Named::End);
    assert_eq!(selected(&app), "game-039");
    press(&mut app, Named::Home);
    assert_eq!(selected(&app), "game-000");
}

#[test]
fn with_nothing_selected_the_keys_start_from_the_ends() {
    for (key, expected) in [
        (Named::Home, "game-000"),
        (Named::End, "game-039"),
        (Named::PageDown, "game-000"),
        (Named::PageUp, "game-000"),
    ] {
        let (_root, mut app) = app_with(40, ViewLayout::Grid);
        assert_eq!(app.selected, None);
        press(&mut app, key);
        assert_eq!(selected(&app), expected, "{key:?}");
    }
}

#[test]
fn page_down_and_up_move_the_rows_that_fit_in_the_list() {
    let (_root, mut app) = app_with(40, ViewLayout::List);
    fit_rows(&mut app, 5);
    select(&mut app, "game-002");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "game-007");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "game-012");
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "game-007");
    // The first page up stops at the top, and the next stays there.
    press(&mut app, Named::PageUp);
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "game-000");
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "game-000");
    // The last page down stops at the bottom, and doesn't wrap.
    select(&mut app, "game-037");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "game-039");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "game-039");
}

#[test]
fn the_page_is_the_rows_that_fit_so_a_taller_window_moves_further() {
    let (_root, mut app) = app_with(60, ViewLayout::List);
    for rows in [1, 2, 7] {
        fit_rows(&mut app, rows);
        select(&mut app, "game-010");
        press(&mut app, Named::PageDown);
        assert_eq!(selected(&app), format!("game-{:03}", 10 + rows), "{rows}");
    }
    // A window shorter than a row still moves one.
    app.viewport_height = 10.0;
    select(&mut app, "game-010");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "game-011");
}

/// The grid with the panel open on `game-001`, as every key test selects
/// it, and how many columns that leaves.
fn grid_on_game_one(count: usize, layout: ViewLayout) -> (tempfile::TempDir, App, usize) {
    let (root, mut app) = app_with(count, layout);
    app.viewport_width = 1500.0;
    app.snap_grid();
    select(&mut app, "game-001");
    assert!(app.panel_open);
    app.snap_grid();
    let columns = app.columns();
    assert!(columns >= 3, "{columns}");
    (root, app, columns)
}

fn game(index: usize) -> String {
    format!("game-{index:03}")
}

#[test]
fn page_down_and_up_keep_the_column_in_the_grid() {
    let (_root, mut app, columns) = grid_on_game_one(80, ViewLayout::Grid);
    fit_rows(&mut app, 3);
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), game(1 + 3 * columns));
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), game(1 + 6 * columns));
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), game(1 + 3 * columns));
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), game(1));
    // The top row is as far as it goes; no wrapping to the bottom.
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), game(1));
}

#[test]
fn page_down_stops_at_the_last_row_and_a_short_row_takes_the_nearest_card() {
    // The last row is partly filled.
    let (_root, mut app, columns) = grid_on_game_one(4 * 4 + 1, ViewLayout::Grid);
    let count = app.cards.len();
    fit_rows(&mut app, 2);
    let last = count - 1;
    select(&mut app, &game(last - columns));
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), game(last));
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), game(last));
}

#[test]
fn coverflow_pages_by_the_covers_on_screen() {
    let (_root, mut app) = app_with(80, ViewLayout::Coverflow);
    let _ = app.update(Message::Strip(crate::coverflow::Event::Measured(
        iced::Size::new(1600.0, 900.0),
    )));
    select(&mut app, "game-001");
    let covers = app.covers_per_screen();
    assert!(covers > 1);
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), game(1 + covers));
    press(&mut app, Named::End);
    assert_eq!(selected(&app), game(79));
    press(&mut app, Named::Home);
    assert_eq!(selected(&app), game(0));
}

#[test]
fn the_keys_do_nothing_while_search_has_focus_or_something_captured_them() {
    for layout in [ViewLayout::Grid, ViewLayout::List] {
        for key in [Named::Home, Named::End, Named::PageUp, Named::PageDown] {
            let (_root, mut app) = app_with(40, layout);
            fit_rows(&mut app, 3);
            select(&mut app, "game-010");
            press_with(&mut app, key, Modifiers::empty(), false, true);
            assert_eq!(
                selected(&app),
                "game-010",
                "search focused {layout:?} {key:?}"
            );
            press_with(&mut app, key, Modifiers::empty(), true, false);
            assert_eq!(selected(&app), "game-010", "captured {layout:?} {key:?}");
            // Control makes another shortcut.
            press_with(&mut app, key, Modifiers::CTRL, false, false);
            assert_eq!(selected(&app), "game-010", "ctrl {layout:?} {key:?}");
        }
    }
}

#[test]
fn the_keys_leave_the_panel_open_and_a_hidden_selection_alone() {
    let (_root, mut app) = app_with(40, ViewLayout::List);
    select(&mut app, "game-010");
    assert!(app.panel_open);
    press(&mut app, Named::End);
    assert_eq!(selected(&app), "game-039");
    assert!(app.panel_open);
    // Filtered to nothing, there's nowhere to go and the selection stays.
    let _ = app.update(Message::SearchChanged("no such game".into()));
    assert!(app.grid_places().ids.is_empty());
    for key in [Named::Home, Named::End, Named::PageUp, Named::PageDown] {
        press(&mut app, key);
        assert_eq!(selected(&app), "game-039", "{key:?}");
    }
}

#[test]
fn f11_is_taken_and_does_nothing_yet() {
    let (_root, mut app) = app_with(10, ViewLayout::Grid);
    select(&mut app, "game-003");
    let layout = app.layout;
    press(&mut app, Named::F11);
    assert_eq!(selected(&app), "game-003");
    assert_eq!(app.layout, layout);
    assert!(app.panel_open);
}

#[test]
fn a_page_is_the_rows_that_fit_whole_and_the_gap_between_them_counts() {
    let (_root, mut app) = app_with(80, ViewLayout::List);
    let tallest = app
        .grid_places()
        .slots
        .iter()
        .map(|slot| slot.height)
        .fold(0.0, f32::max);
    let gap = crate::grid_motion::row_gap(ViewLayout::List);
    // Exactly six rows fit with the gaps between them, and a pixel less
    // leaves five.
    for (height, rows) in [
        (6.0 * (tallest + gap) - gap, 6),
        (6.0 * (tallest + gap) - gap - 1.0, 5),
    ] {
        app.viewport_height = height;
        select(&mut app, "game-010");
        press(&mut app, Named::PageDown);
        assert_eq!(selected(&app), game(10 + rows), "{height}");
    }
}

#[test]
fn a_jump_that_changes_the_selection_resets_what_belongs_to_the_old_one() {
    let (_root, mut app) = app_with(40, ViewLayout::List);
    select(&mut app, "game-010");
    app.panel_note = Some("boom".into());
    press(&mut app, Named::End);
    assert_eq!(selected(&app), "game-039");
    assert_eq!(app.panel_note, None);
    // Landing where it already is changes nothing, so that stays.
    app.panel_note = Some("again".into());
    press(&mut app, Named::End);
    assert_eq!(app.panel_note.as_deref(), Some("again"));
}

/// Three tag sections of six games each, a, b, and c, in that order.
fn sectioned(layout: ViewLayout) -> App {
    let mut app = crate::tests::app();
    app.viewport_width = 1000.0;
    app.viewport_height = 800.0;
    app.layout = layout;
    app.group = wineo_core::library::GroupBy::Tag;
    let cards = ["a", "b", "c"]
        .into_iter()
        .flat_map(|tag| {
            (0..6).map(move |index| {
                let mut card = card(&format!("{tag}-{index}"), State::Inactive, 0);
                card.tags = vec![tag.to_owned()];
                card
            })
        })
        .collect();
    app.merge(cards);
    app.reorder();
    app.snap_grid();
    app
}

fn tag(name: &str) -> crate::slot::SectionKey {
    crate::slot::SectionKey::Tag(name.to_owned())
}

fn shown_order(app: &App) -> Vec<String> {
    app.grid_places()
        .ids
        .iter()
        .map(|id| id.name().to_owned())
        .collect()
}

fn header_on(app: &mut App, section: &str, selected: &str) {
    app.selected = Some(selected.into());
    app.cursor = Some(crate::app_selection::Cursor::Header(tag(section)));
}

#[test]
fn with_sections_home_and_end_go_to_the_first_and_last_game_of_the_library() {
    for layout in [ViewLayout::List, ViewLayout::Grid] {
        let mut app = sectioned(layout);
        assert_eq!(shown_order(&app).len(), 18);
        select(&mut app, "b-2");
        press(&mut app, Named::End);
        assert_eq!(selected(&app), "c-5", "{layout:?}");
        press(&mut app, Named::Home);
        assert_eq!(selected(&app), "a-0", "{layout:?}");
    }
}

#[test]
fn a_page_crosses_a_section_boundary_as_the_arrows_do() {
    let mut app = sectioned(ViewLayout::List);
    fit_rows(&mut app, 3);
    select(&mut app, "a-4");
    // Two rows left in a, so the third lands in b.
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "b-1");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "b-4");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "c-1");
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "b-4");
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "b-1");
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "a-4");
    // The ends stop the page without wrapping, across all three sections.
    press(&mut app, Named::PageUp);
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "a-0");
    select(&mut app, "c-4");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "c-5");
}

#[test]
fn a_grid_page_crosses_into_the_next_section_at_the_nearest_column() {
    let mut app = sectioned(ViewLayout::Grid);
    app.viewport_width = 1500.0;
    app.snap_grid();
    press(&mut app, Named::Home);
    press(&mut app, Named::ArrowRight);
    press(&mut app, Named::ArrowRight);
    assert_eq!(selected(&app), "a-2");
    // Each section is one row of six when the window is wide enough.
    assert!(app.columns() >= 6, "{}", app.columns());
    fit_rows(&mut app, 1);
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "b-2");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "c-2");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "c-2", "the last row stays");
    fit_rows(&mut app, 2);
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "a-2");
}

#[test]
fn a_collapsed_section_has_no_games_to_land_on() {
    let mut app = sectioned(ViewLayout::List);
    let _ = app.update(Message::ToggleSection(tag("b")));
    assert_eq!(shown_order(&app).len(), 12);
    fit_rows(&mut app, 3);
    select(&mut app, "a-4");
    // a-5, then c-0 and c-1, skipping b.
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "c-1");
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "a-4");
    // The ends skip a collapsed section at either end too.
    let _ = app.update(Message::ToggleSection(tag("c")));
    press(&mut app, Named::End);
    assert_eq!(selected(&app), "a-5");
    let _ = app.update(Message::ToggleSection(tag("c")));
    let _ = app.update(Message::ToggleSection(tag("b")));
    let _ = app.update(Message::ToggleSection(tag("a")));
    press(&mut app, Named::Home);
    assert_eq!(selected(&app), "b-0");
}

#[test]
fn every_section_collapsed_leaves_nothing_to_jump_to() {
    let mut app = sectioned(ViewLayout::List);
    select(&mut app, "b-2");
    for section in ["a", "b", "c"] {
        let _ = app.update(Message::ToggleSection(tag(section)));
    }
    assert!(shown_order(&app).is_empty());
    for key in [Named::Home, Named::End, Named::PageUp, Named::PageDown] {
        press(&mut app, key);
        assert_eq!(selected(&app), "b-2", "{key:?}");
    }
}

#[test]
fn from_a_header_cursor_a_page_starts_at_the_header_and_home_and_end_leave_it() {
    let mut app = sectioned(ViewLayout::List);
    fit_rows(&mut app, 3);
    // Down from a header goes to the first card under it, so the third row
    // is the third card of its section.
    header_on(&mut app, "b", "a-0");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "b-2");
    assert_eq!(app.header_cursor(), None);
    // Up goes to the row above the header, the last card of the section
    // before it.
    header_on(&mut app, "b", "a-0");
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "a-3");
    assert_eq!(app.header_cursor(), None);
    header_on(&mut app, "b", "b-3");
    press(&mut app, Named::Home);
    assert_eq!(selected(&app), "a-0");
    assert_eq!(app.header_cursor(), None);
    header_on(&mut app, "b", "b-3");
    press(&mut app, Named::End);
    assert_eq!(selected(&app), "c-5");
    assert_eq!(app.header_cursor(), None);
}

#[test]
fn from_the_header_of_a_collapsed_section_a_page_goes_past_it() {
    let mut app = sectioned(ViewLayout::List);
    fit_rows(&mut app, 3);
    select(&mut app, "b-2");
    let _ = app.update(Message::ToggleSection(tag("b")));
    // The cursor moved to the header and the selection stays on its game.
    assert_eq!(app.header_cursor(), Some(&tag("b")));
    assert_eq!(selected(&app), "b-2");
    press(&mut app, Named::PageDown);
    assert_eq!(selected(&app), "c-2");
    assert_eq!(app.header_cursor(), None);

    // And Up goes to the row above the header.
    let mut app = sectioned(ViewLayout::List);
    fit_rows(&mut app, 3);
    select(&mut app, "b-2");
    let _ = app.update(Message::ToggleSection(tag("b")));
    assert_eq!(app.header_cursor(), Some(&tag("b")));
    press(&mut app, Named::PageUp);
    assert_eq!(selected(&app), "a-3");
}
