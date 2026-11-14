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

/// The focusable ids under `element` with their bounds, in Tab's order.
fn tab_stops(element: Element<'_, Message>) -> Vec<(iced::widget::Id, iced::Rectangle)> {
    use std::sync::{Arc, Mutex};
    struct Stops(Arc<Mutex<Vec<(iced::widget::Id, iced::Rectangle)>>>);
    impl iced::advanced::widget::Operation for Stops {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }
        fn focusable(
            &mut self,
            id: Option<&iced::widget::Id>,
            bounds: iced::Rectangle,
            _state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            self.0
                .lock()
                .unwrap()
                .push((id.expect("an id").clone(), bounds));
        }
    }
    let mut laid = lay_out(element, Size::new(200.0, 100.0));
    let found = Arc::new(Mutex::new(Vec::new()));
    laid.run(Box::new(Stops(found.clone())));
    found.lock().unwrap().clone()
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
fn ctrl_l_switches_on_unless_something_else_took_it() {
    let (_root, mut app) = library(4);
    ctrl_l(&mut app, false, false);
    assert_eq!(app.layout, ViewLayout::Coverflow);
    finish_write(&mut app);
    assert_eq!(saved(&app), ViewLayout::Coverflow);
    // Search has no use for it.
    ctrl_l(&mut app, true, true);
    assert_eq!(app.layout, ViewLayout::List);
    finish_write(&mut app);
    assert_eq!(saved(&app), ViewLayout::List);
    // A focused control that took it keeps it.
    ctrl_l(&mut app, false, true);
    assert_eq!(app.layout, ViewLayout::List);
    // So does an open menu.
    let _ = app.update(Message::Sort(SortEvent::ToggleMenu));
    app.sort_menu = true;
    ctrl_l(&mut app, false, false);
    assert_eq!(app.layout, ViewLayout::List);
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
    app.motion.system_reduced = true;
    let _ = set(&mut app, ViewLayout::List);
    assert!(!app.layout_thumb.active());
    assert_eq!(app.layout_thumb.progress(), 0.0);
    let _ = set(&mut app, ViewLayout::Grid);
    assert!(!app.layout_thumb.active());
    assert_eq!(app.layout_thumb.progress(), 1.0);
}

#[test]
fn the_segment_is_one_tab_stop_and_the_sides_say_what_they_do() {
    let (_root, app) = library(4);
    assert!(focusable(&app, ID));
    let stops = tab_stops(toolbar(&app));
    assert_eq!(stops.len(), 1, "one stop, not one per side");
    assert_eq!(stops[0].0, iced::widget::Id::new(ID));
    assert_eq!(stops[0].1.size(), Size::new(WIDTH, ui::BUTTON_HEIGHT));
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
    assert!(focusable(&app, ID), "the segment stays");
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
    assert_eq!(slot.y - app.scroll_offset, offset.max(0.0), "{name:?}");
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
            let slot = places.slots[places.index(&name.into()).unwrap()];
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
    assert_eq!(slot.y - app.scroll_offset, offset.max(0.0), "{name:?}");
    // Back to the grid, the row at the top keeps it.
    let (anchor, _) = app.find_anchor();
    let (name, offset) = anchor.unwrap();
    let _ = set(&mut app, ViewLayout::Grid);
    let places = app.grid_places();
    let slot = places.slots[places.index(&name).unwrap()];
    assert_eq!(slot.y - app.scroll_offset, offset.max(0.0), "{name:?}");
}

#[test]
fn a_clipped_first_card_lands_on_the_top_edge_going_to_the_list() {
    let (_root, mut app) = library(120);
    // The first row of posters is clipped by the top edge.
    app.scroll_offset = 200.0;
    let (anchor, reveal) = app.find_anchor();
    let (name, offset) = anchor.unwrap();
    assert!(offset < 0.0 && !reveal, "{name:?} at {offset}");
    let _ = set(&mut app, ViewLayout::List);
    let places = app.grid_places();
    let slot = places.slots[places.index(&name).unwrap()];
    assert!(
        slot.y >= app.scroll_offset
            && slot.y + slot.height <= app.scroll_offset + app.viewport_height,
        "{name:?}: {slot:?} at {}",
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
    assert!(offset < 0.0 && !reveal, "{name:?} at {offset}");
    let _ = set(&mut app, ViewLayout::Grid);
    let places = app.grid_places();
    let slot = places.slots[places.index(&name).unwrap()];
    assert!(
        slot.y >= app.scroll_offset
            && slot.y + slot.height <= app.scroll_offset + app.viewport_height,
        "{name:?}: {slot:?} at {}",
        app.scroll_offset
    );
}

/// Asserts `name`'s card is drawn whole above the action bar.
fn assert_whole_on_screen(app: &App, name: &str) {
    let places = app.grid_places();
    let slot = places.slots[places.index(&name.into()).unwrap()];
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
    assert_whole_on_screen(&app, name.name());
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
    let name = places.ids[index].name().to_owned();
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

/// Hands `named` to the window's focused segment and applies what it sent.
fn press_on_segment(app: &mut App, named: iced::keyboard::key::Named) -> iced::event::Status {
    let mut laid = lay_out(app.view(), app.window_size);
    laid.run(Box::new(
        iced::advanced::widget::operation::focusable::focus(iced::widget::Id::new(ID)),
    ));
    let (messages, status) = laid.event_with_status(
        &crate::sort_control::tests::press(named),
        iced::mouse::Cursor::Unavailable,
    );
    drop(laid);
    for message in messages {
        let _ = app.update(message);
    }
    status
}

#[test]
fn left_steps_back_and_right_space_and_enter_step_forward_without_wrapping() {
    use iced::keyboard::key::Named;
    let (_root, mut app) = library(4);
    // The sides run list, grid, coverflow, and a key that has nowhere to go
    // still takes the press, so it never reaches the library's selection.
    for (named, start, expected) in [
        (Named::ArrowLeft, ViewLayout::Coverflow, ViewLayout::Grid),
        (Named::ArrowLeft, ViewLayout::Grid, ViewLayout::List),
        (Named::ArrowLeft, ViewLayout::List, ViewLayout::List),
        (Named::ArrowRight, ViewLayout::List, ViewLayout::Grid),
        (Named::ArrowRight, ViewLayout::Grid, ViewLayout::Coverflow),
        (
            Named::ArrowRight,
            ViewLayout::Coverflow,
            ViewLayout::Coverflow,
        ),
        (Named::Space, ViewLayout::List, ViewLayout::Grid),
        (Named::Space, ViewLayout::Grid, ViewLayout::Coverflow),
        (Named::Space, ViewLayout::Coverflow, ViewLayout::Coverflow),
        (Named::Enter, ViewLayout::List, ViewLayout::Grid),
        (Named::Enter, ViewLayout::Grid, ViewLayout::Coverflow),
        (Named::Enter, ViewLayout::Coverflow, ViewLayout::Coverflow),
    ] {
        let _ = set(&mut app, start);
        assert_eq!(
            press_on_segment(&mut app, named),
            iced::event::Status::Captured,
            "{named:?} from {start:?}"
        );
        assert_eq!(app.layout, expected, "{named:?} from {start:?}");
    }
}

#[test]
fn an_unfocused_segment_ignores_the_keys() {
    let (_root, app) = library(4);
    let mut laid = lay_out(app.view(), app.window_size);
    for named in [
        iced::keyboard::key::Named::ArrowLeft,
        iced::keyboard::key::Named::ArrowRight,
    ] {
        let messages = laid.event(&crate::sort_control::tests::press(named));
        assert!(
            messages
                .iter()
                .all(|message| !matches!(message, Message::Layout(_)))
        );
    }
}

#[test]
fn a_click_picks_a_side_without_focusing_the_segment() {
    let (_root, app) = library(4);
    let mut laid = lay_out(toolbar(&app), Size::new(200.0, 100.0));
    let left = iced::mouse::Button::Left;
    laid.run(Box::new(
        iced::advanced::widget::operation::focusable::focus(iced::widget::Id::new(ID)),
    ));
    assert_eq!(laid.focused(), vec![iced::widget::Id::new(ID)]);
    // Text has no size without a real renderer, so aim at each laid-out side.
    for (side, expected) in [(0, "Set(List)"), (1, "Set(Grid)")] {
        let bounds = crate::layout_probe::bounds_at(&laid.node, &[2, side]);
        let cursor = iced::mouse::Cursor::Available(bounds.center());
        let mut messages = laid.event_at(
            &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(left)),
            cursor,
        );
        messages.extend(laid.event_at(
            &iced::Event::Mouse(iced::mouse::Event::ButtonReleased(left)),
            cursor,
        ));
        assert_eq!(messages.len(), 1);
        assert!(
            format!("{:?}", messages[0]).contains(expected),
            "{messages:?}"
        );
        assert!(laid.focused().is_empty(), "a click leaves no focus");
    }
}

#[test]
fn a_focused_segment_keeps_the_arrows_from_the_librarys_selection() {
    use iced::keyboard::key::Named;
    let (_root, mut app) = library(4);
    assert_eq!(app.selected, None);
    for named in [Named::ArrowLeft, Named::ArrowRight] {
        let status = press_on_segment(&mut app, named);
        assert_eq!(status, iced::event::Status::Captured);
        // The app sees the key as captured, as the runtime hands it over.
        let _ = app.update(Message::KeyWithFocus(
            Key::Named(named),
            Modifiers::empty(),
            status,
            false,
        ));
        assert_eq!(app.selected, None, "{named:?} moved the selection");
    }
    // The same arrow, uncaptured, does move it, so the check above bites.
    let _ = app.update(Message::KeyWithFocus(
        Key::Named(Named::ArrowRight),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
    assert!(app.selected.is_some());
}

#[test]
fn the_switch_has_three_sides_in_order_and_each_sets_its_layout() {
    let (_root, app) = library(4);
    assert_eq!(WIDTH, 96.0);
    let size = Size::new(200.0, 100.0);
    for (side, expected) in [
        (0, "Layout(Set(List))"),
        (1, "Layout(Set(Grid))"),
        (2, "Layout(Set(Coverflow))"),
    ] {
        assert_eq!(click_at(toolbar(&app), size, &[2, side]), [expected]);
    }
    assert_eq!(label(ViewLayout::Coverflow), "Show as coverflow");
    let stops = tab_stops(toolbar(&app));
    assert_eq!(stops.len(), 1, "still one stop");
    assert_eq!(stops[0].1.size(), Size::new(WIDTH, ui::BUTTON_HEIGHT));
}

#[test]
fn ctrl_l_cycles_the_sides_left_to_right_and_wraps_saving_each() {
    let (_root, mut app) = library(4);
    for (expected, text) in [
        (ViewLayout::Coverflow, "coverflow"),
        (ViewLayout::List, "list"),
        (ViewLayout::Grid, "grid"),
        (ViewLayout::Coverflow, "coverflow"),
    ] {
        ctrl_l(&mut app, false, false);
        assert_eq!(app.layout, expected);
        finish_write(&mut app);
        assert_eq!(saved(&app), expected);
        let file = std::fs::read_to_string(app.paths.library_file()).unwrap();
        assert!(file.contains(&format!("layout = \"{text}\"")), "{file}");
    }
    // Search has no use for it, and a control that took it keeps it.
    ctrl_l(&mut app, true, true);
    assert_eq!(app.layout, ViewLayout::List);
    ctrl_l(&mut app, false, true);
    assert_eq!(app.layout, ViewLayout::List);
}

#[test]
fn the_thumb_rests_at_three_stops_and_slides_between_them() {
    assert_eq!(stop(ViewLayout::List), 0.0);
    assert_eq!(stop(ViewLayout::Grid), 1.0);
    assert_eq!(stop(ViewLayout::Coverflow), 2.0);

    let (_root, mut app) = library(4);
    assert_eq!(app.layout_thumb.progress(), 1.0);
    let _ = set(&mut app, ViewLayout::Coverflow);
    assert!(app.layout_thumb.active());
    assert_eq!(
        app.layout_thumb.progress(),
        1.0,
        "it sets out from the grid"
    );
    let generation = app.layout_thumb.generation();
    let _ = app.update(Message::Layout(Event::Tick(
        generation,
        Instant::now() + Duration::from_millis(80),
    )));
    let midway = app.layout_thumb.progress();
    assert!(midway > 1.0 && midway < 2.0, "{midway}");
    settle_thumb(&mut app);
    assert!(!app.layout_thumb.active());
    assert_eq!(app.layout_thumb.progress(), 2.0);
    // Back one stop, not through the one it passed.
    let _ = set(&mut app, ViewLayout::Grid);
    settle_thumb(&mut app);
    assert_eq!(app.layout_thumb.progress(), 1.0);

    app.motion.system_reduced = true;
    let _ = set(&mut app, ViewLayout::Coverflow);
    assert!(!app.layout_thumb.active());
    assert_eq!(app.layout_thumb.progress(), 2.0);
    let _ = set(&mut app, ViewLayout::List);
    assert_eq!(app.layout_thumb.progress(), 0.0);
}

#[test]
fn a_saved_coverflow_boots_with_the_thumb_on_the_third_side() {
    let (_root, app) = boot_with(Some("schema_version = 1\n[view]\nlayout = \"coverflow\"\n"));
    assert_eq!(app.layout, ViewLayout::Coverflow);
    assert_eq!(app.layout_thumb.progress(), 2.0);
    assert!(!app.layout_thumb.active());
}

#[test]
fn coverflow_draws_as_the_grid_until_its_widget_lands() {
    let (_root, mut app) = library(40);
    app.viewport_width = 1000.0;
    app.snap_grid();
    let grid_places = app.grid_places();
    let grid_columns = app.columns();
    let grid_width = app.slot_width();
    let _ = set(&mut app, ViewLayout::Coverflow);
    assert_eq!(app.layout, ViewLayout::Coverflow);
    let places = app.grid_places();
    assert_eq!(places.ids, grid_places.ids);
    assert_eq!(places.slots, grid_places.slots);
    assert_eq!(app.columns(), grid_columns);
    assert_eq!(app.slot_width(), grid_width);
    assert_eq!(app.header_room(), 0.0);
    // The posters' zoom stays available, since the posters show.
    assert!(app.zoom_allowed());
    let _ = app.update(Message::Zoom(ZoomEvent::Key(ZoomKey::In)));
    assert_eq!(app.zoom, 110);
    // The marks, sort, and selection hold through it like any switch.
    let _ = app.update(Message::Select("game-010".into()));
    let _ = set(&mut app, ViewLayout::List);
    let _ = set(&mut app, ViewLayout::Coverflow);
    assert_eq!(app.selected.as_deref(), Some("game-010"));
}

#[test]
fn the_thumb_is_drawn_over_the_chosen_side() {
    let (_root, mut app) = library(4);
    app.motion.system_reduced = true;
    let size = Size::new(200.0, 100.0);
    for (layout, side) in [
        (ViewLayout::Grid, 1.0),
        (ViewLayout::Coverflow, 2.0),
        (ViewLayout::List, 0.0),
    ] {
        let _ = set(&mut app, layout);
        let node = crate::layout_probe::layout(&mut toolbar(&app), size);
        let track = crate::layout_probe::bounds_at(&node, &[0]);
        let thumb = crate::layout_probe::bounds_at(&node, &[1, 0]);
        assert_eq!(thumb.width, WIDTH / 3.0, "{layout:?}");
        assert_eq!(thumb.x - track.x, side * WIDTH / 3.0, "{layout:?}");
    }
}

#[test]
fn coverflow_keeps_the_zoom_controls_as_the_grid_does() {
    let (_root, mut app) = library(4);
    for layout in [ViewLayout::Grid, ViewLayout::Coverflow] {
        let _ = set(&mut app, layout);
        assert!(
            focusable(&app, crate::zoom_control::SLIDER_ID),
            "{layout:?}"
        );
        assert!(
            focusable(&app, crate::zoom_control::POSTER_ONLY_ID),
            "{layout:?}"
        );
    }
    let _ = set(&mut app, ViewLayout::List);
    assert!(!focusable(&app, crate::zoom_control::SLIDER_ID));
    // The slots are the grid's width and gap, not the list's.
    let layout = crate::grid_motion::GridLayout::new(100);
    for mode in [ViewLayout::Grid, ViewLayout::Coverflow] {
        assert_eq!(
            crate::grid_motion::row_gap(mode),
            crate::grid_motion::row_gap(ViewLayout::Grid)
        );
        assert_eq!(
            crate::grid_motion::width_for_viewport(1000.0, false, layout, mode),
            layout.card_width()
        );
    }
    assert_ne!(
        crate::grid_motion::row_gap(ViewLayout::Coverflow),
        crate::grid_motion::row_gap(ViewLayout::List)
    );
}
