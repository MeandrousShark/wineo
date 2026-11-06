use std::collections::BTreeSet;
use std::time::Duration;

use iced::Size;
use iced::keyboard::{Key, Modifiers};
use wineo_core::library::SortKey;
use wineo_core::runtime::State;

use super::*;
use crate::layout_probe::{click_at, lay_out};
use crate::sort_control::{Event as SortEvent, save};
use crate::tests::card;
use crate::zoom::{Event as ZoomEvent, Key as ZoomKey};

fn boot_with(text: Option<&str>) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    if let Some(text) = text {
        let file = paths.library_file();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    (root, App::boot(paths).0)
}

/// An app in the grid showing `count` games, in the order of their names.
fn library(count: usize) -> (tempfile::TempDir, App) {
    let (root, mut app) = boot_with(None);
    let cards = (0..count)
        .map(|index| {
            card(
                &format!("game-{index:03}"),
                State::Inactive,
                60 * index as i64,
            )
        })
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    (root, app)
}

fn set(app: &mut App, layout: ViewLayout) -> Task<Message> {
    app.update(Message::Layout(Event::Set(layout)))
}

fn ctrl_l(app: &mut App, search_focused: bool, captured: bool) {
    let status = if captured {
        iced::event::Status::Captured
    } else {
        iced::event::Status::Ignored
    };
    let _ = app.update(Message::KeyWithFocus(
        Key::Character("l".into()),
        Modifiers::CTRL,
        status,
        search_focused,
    ));
}

/// Runs the layout write in flight and hands its result back.
fn finish_write(app: &mut App) {
    let layout = app.view_writes.layout.expect("a write in flight");
    let result = save(&app.paths, &app.shown, Field::Layout(layout));
    let _ = app.update(Message::Sort(SortEvent::Saved(
        Field::Layout(layout),
        result,
    )));
}

fn saved(app: &App) -> ViewLayout {
    wineo_core::library::load(&app.paths).view.layout
}

/// Whether the window lays out a focusable control with `id`.
fn focusable(app: &App, id: &'static str) -> bool {
    let mut laid = lay_out(app.view(), app.window_size);
    laid.run(Box::new(
        iced::advanced::widget::operation::focusable::focus(iced::widget::Id::new(id)),
    ));
    laid.focused() == vec![iced::widget::Id::new(id)]
}

fn settle_thumb(app: &mut App) {
    let generation = app.layout_thumb.generation();
    let _ = app.update(Message::Layout(Event::Tick(
        generation,
        Instant::now() + Duration::from_secs(1),
    )));
}

#[test]
fn either_side_sets_its_layout_and_saves_it() {
    let (_root, mut app) = library(4);
    let size = Size::new(200.0, 100.0);
    assert_eq!(
        click_at(toolbar(&app), size, &[2, 0]),
        ["Layout(Set(List))"]
    );
    assert_eq!(
        click_at(toolbar(&app), size, &[2, 1]),
        ["Layout(Set(Grid))"]
    );
    let _ = set(&mut app, ViewLayout::List);
    assert_eq!(app.layout, ViewLayout::List);
    finish_write(&mut app);
    assert_eq!(saved(&app), ViewLayout::List);
    assert!(app.view_writes.layout.is_none());
    // The side already chosen does nothing.
    let _ = set(&mut app, ViewLayout::List);
    assert!(app.view_writes.layout.is_none());
    let _ = set(&mut app, ViewLayout::Grid);
    assert_eq!(app.layout, ViewLayout::Grid);
    finish_write(&mut app);
    assert_eq!(saved(&app), ViewLayout::Grid);
    let text = std::fs::read_to_string(app.paths.library_file()).unwrap();
    assert!(text.contains("layout = \"grid\""), "{text}");
}

#[test]
fn ctrl_l_switches_both_ways_unless_something_else_took_it() {
    let (_root, mut app) = library(4);
    ctrl_l(&mut app, false, false);
    assert_eq!(app.layout, ViewLayout::List);
    finish_write(&mut app);
    assert_eq!(saved(&app), ViewLayout::List);
    // Search has no use for it.
    ctrl_l(&mut app, true, true);
    assert_eq!(app.layout, ViewLayout::Grid);
    finish_write(&mut app);
    assert_eq!(saved(&app), ViewLayout::Grid);
    // A focused control that took it keeps it.
    ctrl_l(&mut app, false, true);
    assert_eq!(app.layout, ViewLayout::Grid);
    // So does an open menu.
    let _ = app.update(Message::Sort(SortEvent::ToggleMenu));
    app.sort_menu = true;
    ctrl_l(&mut app, false, false);
    assert_eq!(app.layout, ViewLayout::Grid);
}

#[test]
fn the_saved_layout_is_read_at_launch_with_the_thumb_on_its_side() {
    let (_root, app) = boot_with(None);
    assert_eq!(app.layout, ViewLayout::Grid);
    assert_eq!(app.layout_thumb.progress(), 1.0);
    let (_root, app) = boot_with(Some("schema_version = 1\n[view]\nlayout = \"list\"\n"));
    assert_eq!(app.layout, ViewLayout::List);
    assert_eq!(app.layout_thumb.progress(), 0.0);
    assert!(!app.layout_thumb.active());
    let (_root, app) = boot_with(Some("schema_version = 1\n[view]\nlayout = \"table\"\n"));
    assert_eq!(app.layout, ViewLayout::Grid);
}

#[test]
fn the_thumb_slides_to_the_chosen_side() {
    let (_root, mut app) = library(4);
    let _ = set(&mut app, ViewLayout::List);
    assert!(app.layout_thumb.active());
    assert_eq!(
        app.layout_thumb.progress(),
        1.0,
        "it sets out from the grid"
    );
    let generation = app.layout_thumb.generation();
    let _ = app.update(Message::Layout(Event::Tick(
        generation,
        Instant::now() + Duration::from_millis(60),
    )));
    let midway = app.layout_thumb.progress();
    assert!(midway > 0.0 && midway < 1.0, "{midway}");
    settle_thumb(&mut app);
    assert!(!app.layout_thumb.active());
    assert_eq!(app.layout_thumb.progress(), 0.0);
    let _ = set(&mut app, ViewLayout::Grid);
    settle_thumb(&mut app);
    assert_eq!(app.layout_thumb.progress(), 1.0);
}

#[test]
fn reduced_motion_snaps_the_thumb() {
    let (_root, mut app) = library(4);
    app.reduced_motion = true;
    let _ = set(&mut app, ViewLayout::List);
    assert!(!app.layout_thumb.active());
    assert_eq!(app.layout_thumb.progress(), 0.0);
    let _ = set(&mut app, ViewLayout::Grid);
    assert!(!app.layout_thumb.active());
    assert_eq!(app.layout_thumb.progress(), 1.0);
}

#[test]
fn the_sides_take_keyboard_focus_and_say_what_they_do() {
    let (_root, app) = library(4);
    assert!(focusable(&app, LIST_ID));
    assert!(focusable(&app, GRID_ID));
    assert_eq!(label(ViewLayout::List), "Show as list");
    assert_eq!(label(ViewLayout::Grid), "Show as grid");
}

#[test]
fn the_zoom_leaves_the_toolbar_and_its_keys_do_nothing_in_the_list() {
    let (_root, mut app) = library(30);
    assert!(focusable(&app, crate::zoom_control::SLIDER_ID));
    assert!(focusable(&app, crate::zoom_control::POSTER_ONLY_ID));
    let _ = set(&mut app, ViewLayout::List);
    assert!(!focusable(&app, crate::zoom_control::SLIDER_ID));
    assert!(!focusable(&app, crate::zoom_control::POSTER_ONLY_ID));
    assert!(focusable(&app, LIST_ID), "the segment stays");
    assert!(!app.zoom_allowed());
    for key in [ZoomKey::In, ZoomKey::Out, ZoomKey::Reset] {
        let _ = app.update(Message::Zoom(ZoomEvent::Key(key)));
        assert_eq!(app.zoom, 100);
        assert!(!app.zooming.active());
    }
    let _ = app.update(Message::Zoom(ZoomEvent::TogglePosterOnly));
    assert!(!app.poster_only);
    let _ = app.update(Message::Zoom(ZoomEvent::Wheel(
        iced::mouse::ScrollDelta::Lines { x: 0.0, y: 3.0 },
    )));
    assert_eq!(app.zoom, 100);
    assert!(app.view_writes.zoom.is_none());
    // The way back finds the zoom where it was.
    let _ = set(&mut app, ViewLayout::Grid);
    assert!(app.zoom_allowed());
    let _ = app.update(Message::Zoom(ZoomEvent::Key(ZoomKey::In)));
    assert_eq!(app.zoom, 110);
}

#[test]
fn a_zoom_in_flight_ends_where_it_was_heading_before_the_list_shows() {
    let (_root, mut app) = library(120);
    // Five columns at 100% and at 110%, so stopping the tween only scales.
    app.viewport_width = 1000.0;
    app.snap_grid();
    assert_eq!(app.columns(), 5);
    app.scroll_offset = 3000.0;
    let _ = app.update(Message::Zoom(ZoomEvent::Key(ZoomKey::In)));
    assert!(app.zooming.active());
    let generation = app.zooming.generation();
    let _ = app.update(Message::Zoom(ZoomEvent::Frame(
        generation,
        Instant::now() + Duration::from_millis(40),
    )));
    let shown = app.zooming.shown().expect("mid-tween");
    assert!(shown > 100.0 && shown < 110.0, "{shown}");
    assert_eq!(app.columns(), 5);
    // The card at the top as drawn mid-tween keeps its place in the list,
    // on the top edge when it was clipped there.
    let (anchor, _) = app.find_anchor();
    let (name, offset) = anchor.unwrap();
    let _ = set(&mut app, ViewLayout::List);
    let places = app.grid_places();
    let slot = places.slots[places.index(&name).unwrap()];
    assert_eq!(slot.y - app.scroll_offset, offset.max(0.0), "{name}");
    assert!(!app.zooming.moving());
    assert_eq!(app.zoom, 110);
    let _ = set(&mut app, ViewLayout::Grid);
    assert_eq!(app.grid_layout(), crate::grid_motion::GridLayout::new(110));
}

#[test]
fn marks_sort_and_selection_hold_through_a_switch_both_ways() {
    let (_root, mut app) = library(40);
    let _ = app.update(Message::Sort(SortEvent::Pick(SortKey::Playtime)));
    let order: Vec<String> = app
        .filtered()
        .iter()
        .map(|card| card.name.clone())
        .collect();
    let _ = app.update(Message::Select("game-010".into()));
    app.modifiers = Modifiers::CTRL;
    let _ = app.update(Message::Select("game-003".into()));
    let _ = app.update(Message::Select("game-020".into()));
    app.modifiers = Modifiers::empty();
    let marked: BTreeSet<String> = ["game-003", "game-020"].map(String::from).into();
    let query = app.query;
    for layout in [ViewLayout::List, ViewLayout::Grid, ViewLayout::List] {
        let _ = set(&mut app, layout);
        assert_eq!(app.layout, layout);
        assert_eq!(app.marked, marked, "{layout:?}");
        assert_eq!(app.query, query, "{layout:?}");
        assert_eq!(app.selected.as_deref(), Some("game-010"), "{layout:?}");
        assert!(app.panel_open, "{layout:?}");
        let shown: Vec<String> = app
            .filtered()
            .iter()
            .map(|card| card.name.clone())
            .collect();
        assert_eq!(shown, order, "{layout:?}");
        assert!(!app.grid_motion.active(), "a switch snaps");
    }
}

#[test]
fn the_selected_card_stays_in_view_after_a_switch() {
    for start in [ViewLayout::Grid, ViewLayout::List] {
        let (_root, mut app) = library(120);
        if start == ViewLayout::List {
            let _ = set(&mut app, ViewLayout::List);
        }
        let name = "game-090";
        app.selected = Some(name.into());
        let _ = app.scroll_to_landing();
        assert!(app.scroll_offset > 0.0, "{start:?}");
        for layout in [ViewLayout::List, ViewLayout::Grid] {
            let _ = set(&mut app, layout);
            let places = app.grid_places();
            let slot = places.slots[places.index(name).unwrap()];
            let visible = app.viewport_height - app.bar_room();
            assert!(
                slot.y >= app.scroll_offset - 0.5
                    && slot.y + slot.height <= app.scroll_offset + visible + 0.5,
                "from {start:?} to {layout:?}: {slot:?} at {}",
                app.scroll_offset
            );
        }
    }
}

#[test]
fn the_header_takes_its_room_from_the_scroll() {
    let (_root, mut app) = library(4);
    let height = app.viewport_height;
    let _ = set(&mut app, ViewLayout::List);
    assert_eq!(
        app.viewport_height,
        height - crate::list_view::HEADER_HEIGHT
    );
    let _ = set(&mut app, ViewLayout::Grid);
    assert_eq!(app.viewport_height, height);
}

#[test]
fn the_switch_is_blocked_by_a_dialog() {
    let (_root, mut app) = library(4);
    app.close_prompt = true;
    let _ = set(&mut app, ViewLayout::List);
    assert_eq!(app.layout, ViewLayout::Grid);
}

#[test]
fn with_nothing_selected_the_first_card_in_view_keeps_its_place() {
    let (_root, mut app) = library(120);
    app.scroll_offset = 1500.0;
    let (anchor, _) = app.find_anchor();
    let (name, offset) = anchor.unwrap();
    let _ = set(&mut app, ViewLayout::List);
    let places = app.grid_places();
    let slot = places.slots[places.index(&name).unwrap()];
    assert_eq!(slot.y - app.scroll_offset, offset.max(0.0), "{name}");
    // Back to the grid, the row at the top keeps it.
    let (anchor, _) = app.find_anchor();
    let (name, offset) = anchor.unwrap();
    let _ = set(&mut app, ViewLayout::Grid);
    let places = app.grid_places();
    let slot = places.slots[places.index(&name).unwrap()];
    assert_eq!(slot.y - app.scroll_offset, offset.max(0.0), "{name}");
}

#[test]
fn a_clipped_first_card_lands_on_the_top_edge_going_to_the_list() {
    let (_root, mut app) = library(120);
    // The first row of posters is clipped by the top edge.
    app.scroll_offset = 200.0;
    let (anchor, reveal) = app.find_anchor();
    let (name, offset) = anchor.unwrap();
    assert!(offset < 0.0 && !reveal, "{name} at {offset}");
    let _ = set(&mut app, ViewLayout::List);
    let places = app.grid_places();
    let slot = places.slots[places.index(&name).unwrap()];
    assert!(
        slot.y >= app.scroll_offset
            && slot.y + slot.height <= app.scroll_offset + app.viewport_height,
        "{name}: {slot:?} at {}",
        app.scroll_offset
    );
}

#[test]
fn a_clipped_row_lands_on_the_top_edge_going_back_to_the_grid() {
    let (_root, mut app) = library(120);
    let _ = set(&mut app, ViewLayout::List);
    // The row under the top edge is clipped.
    app.scroll_offset = 100.0;
    let (anchor, reveal) = app.find_anchor();
    let (name, offset) = anchor.unwrap();
    assert!(offset < 0.0 && !reveal, "{name} at {offset}");
    let _ = set(&mut app, ViewLayout::Grid);
    let places = app.grid_places();
    let slot = places.slots[places.index(&name).unwrap()];
    assert!(
        slot.y >= app.scroll_offset
            && slot.y + slot.height <= app.scroll_offset + app.viewport_height,
        "{name}: {slot:?} at {}",
        app.scroll_offset
    );
}

/// Asserts `name`'s card is drawn whole above the action bar.
fn assert_whole_on_screen(app: &App, name: &str) {
    let places = app.grid_places();
    let slot = places.slots[places.index(name).unwrap()];
    let bottom = app.scroll_offset + app.viewport_height - app.bar_room();
    assert!(
        slot.y >= app.scroll_offset && slot.y + slot.height <= bottom,
        "{name}: {slot:?} in {}..{bottom}",
        app.scroll_offset
    );
}

#[test]
fn at_the_end_of_the_list_the_first_row_in_view_shows_whole_in_the_grid() {
    let (_root, mut app) = library(120);
    app.viewport_height = 400.0;
    let _ = set(&mut app, ViewLayout::List);
    let places = app.grid_places();
    app.scroll_offset = places.height + app.bar_room() - app.viewport_height;
    let (anchor, reveal) = app.find_anchor();
    let (name, _) = anchor.unwrap();
    assert!(!reveal);
    let _ = set(&mut app, ViewLayout::Grid);
    assert_whole_on_screen(&app, &name);
}

#[test]
fn a_selected_row_near_the_bottom_shows_whole_in_the_grid() {
    let (_root, mut app) = library(120);
    app.viewport_height = 400.0;
    let _ = set(&mut app, ViewLayout::List);
    app.scroll_offset = 2000.0;
    let places = app.grid_places();
    let bottom = app.scroll_offset + app.viewport_height - app.bar_room();
    // The last whole row in view, too low for its poster to fit there.
    let index = (0..places.slots.len())
        .rev()
        .find(|&index| places.slots[index].y + places.slots[index].height <= bottom)
        .unwrap();
    let name = places.names[index].clone();
    app.selected = Some(name.clone());
    let (anchor, reveal) = app.find_anchor();
    let (_, offset) = anchor.unwrap();
    assert!(
        !reveal && offset > app.viewport_height / 2.0,
        "{name} at {offset}"
    );
    let _ = set(&mut app, ViewLayout::Grid);
    assert_whole_on_screen(&app, &name);
}
