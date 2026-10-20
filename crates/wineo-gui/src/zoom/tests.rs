use std::sync::{Arc, Mutex};

use iced::keyboard::{Location, Modifiers, key::Named};
use iced::mouse::Cursor;
use iced::{Point, Rectangle, Size};
use wineo_core::runtime::State;

use super::*;
use crate::grid_motion;
use crate::layout_probe::{Laid, bounds_at, lay_out, layout};
use crate::sort_control::{Writes, save};
use crate::zoom_control::{POSTER_ONLY_ID, SLIDER_ID};

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    (root, App::boot(paths).0)
}

fn cards(count: usize) -> Vec<crate::library::Card> {
    (0..count)
        .map(|index| crate::tests::card(&format!("game-{index:03}"), State::Inactive, 60))
        .collect()
}

/// An app with enough cards to scroll at every zoom.
fn library(count: usize) -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    let _ = app.update(Message::Loaded(Ok(cards(count))));
    (root, app)
}

fn zoom(app: &mut App, event: Event) -> Task<Message> {
    app.update(Message::Zoom(event))
}

/// Runs the tween to its end.
fn settle(app: &mut App) -> Task<Message> {
    let Some(tween) = app.zooming.tween else {
        return Task::none();
    };
    let generation = app.zooming.generation;
    zoom(app, Event::Frame(generation, tween.start + TWEEN))
}

/// A frame `ms` into the tween.
fn frame(app: &mut App, ms: u64) -> Task<Message> {
    let start = app.zooming.tween.expect("a tween").start;
    let generation = app.zooming.generation;
    zoom(
        app,
        Event::Frame(generation, start + Duration::from_millis(ms)),
    )
}

/// Hands `event` to the laid out view with the cursor at `at`, then the
/// messages it sent to the app.
fn send(app: &mut App, event: &iced::Event, at: Cursor) -> Vec<Message> {
    let mut laid = lay_out(app.view(), app.window_size);
    let messages = laid.event_at(event, at);
    drop(laid);
    for message in messages.clone() {
        let _ = app.update(message);
    }
    messages
}

/// Runs the zoom write in flight, as its task would.
fn finish_write(app: &mut App) {
    let written = Field::Zoom(app.view_writes.zoom.expect("a zoom write"));
    let result = save(&app.paths, &app.shown, written);
    let _ = app.update(Message::Sort(crate::sort_control::Event::Saved(
        written, result,
    )));
}

fn key_event(key: &str, modifiers: Modifiers, location: Location) -> iced::Event {
    let key = keyboard::Key::Character(key.into());
    iced::Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key.clone(),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location,
        modifiers,
        text: match key {
            keyboard::Key::Character(text) => Some(text),
            _ => None,
        },
        repeat: false,
    })
}

fn bounds(laid: &mut Laid<'_, Message>, id: &'static str) -> Rectangle {
    let found = Arc::new(Mutex::new(None));
    let sink = found.clone();
    laid.run(Box::new(iced::advanced::widget::operation::map(
        crate::library_menu::bounds_of(id),
        move |bounds| *sink.lock().unwrap() = bounds,
    )));
    found.lock().unwrap().expect(id)
}

/// Every focusable id in tree order, which is Tab's order.
fn focus_order(laid: &mut Laid<'_, Message>) -> Vec<String> {
    struct Order(Arc<Mutex<Vec<String>>>);
    impl iced::advanced::widget::Operation for Order {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }
        fn focusable(
            &mut self,
            id: Option<&iced::widget::Id>,
            _bounds: Rectangle,
            _state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            self.0
                .lock()
                .unwrap()
                .push(id.map_or_else(String::new, |id| format!("{id:?}")));
        }
    }
    let found = Arc::new(Mutex::new(Vec::new()));
    laid.run(Box::new(Order(found.clone())));
    found.lock().unwrap().clone()
}

fn id_text(id: &str) -> String {
    format!("{:?}", iced::widget::Id::from(id.to_owned()))
}

fn focus(laid: &mut Laid<'_, Message>, id: &'static str) {
    laid.run(Box::new(
        iced::advanced::widget::operation::focusable::focus(iced::widget::Id::new(id)),
    ));
}

/// Runs `task`'s widget operations against `laid`.
fn drive(laid: &mut Laid<'_, Message>, task: Task<Message>) {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        while let Ok(Some(action)) =
            tokio::time::timeout(Duration::from_millis(500), stream.next()).await
        {
            if let iced_runtime::Action::Widget(operation) = action {
                laid.run(operation);
            }
        }
    });
}

fn grid_offset(laid: &mut Laid<'_, Message>) -> f32 {
    laid.offsets().into_iter().fold(0.0_f32, f32::max)
}

fn wheel(delta: ScrollDelta) -> iced::Event {
    iced::Event::Mouse(iced::mouse::Event::WheelScrolled { delta })
}

fn held(modifiers: Modifiers) -> iced::Event {
    iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers))
}

const NOTCH_UP: ScrollDelta = ScrollDelta::Lines { x: 0.0, y: 1.0 };
const NOTCH_DOWN: ScrollDelta = ScrollDelta::Lines { x: 0.0, y: -1.0 };

/// A point over the grid, below the toolbar, in the default window.
fn over_grid() -> Point {
    Point::new(crate::sidebar::WIDTH + 300.0, 400.0)
}

#[test]
fn every_zoom_shortcut_maps_to_its_step() {
    let ctrl = Modifiers::CTRL;
    let shift = Modifiers::CTRL | Modifiers::SHIFT;
    for (character, modifiers, expected) in [
        ("=", ctrl, Some(Key::In)),
        ("=", shift, Some(Key::In)),
        ("+", ctrl, Some(Key::In)),
        ("+", shift, Some(Key::In)),
        ("-", ctrl, Some(Key::Out)),
        ("-", shift, Some(Key::Out)),
        ("0", ctrl, Some(Key::Reset)),
        ("=", Modifiers::empty(), None),
        ("-", Modifiers::SHIFT, None),
        ("0", Modifiers::empty(), None),
        ("=", ctrl | Modifiers::ALT, None),
        ("-", ctrl | Modifiers::LOGO, None),
        ("9", ctrl, None),
    ] {
        assert_eq!(
            key(&keyboard::Key::Character(character.into()), modifiers),
            expected,
            "{character} {modifiers:?}"
        );
    }
    // The keypad's + and - arrive as the same characters.
    let (_root, mut app) = library(4);
    for (character, expected) in [("+", 110), ("-", 100)] {
        let event = key_event(character, ctrl, Location::Numpad);
        let _ = send(&mut app, &event, Cursor::Unavailable);
        assert_eq!(app.zoom, expected, "keypad {character}");
    }
}

#[test]
fn zoom_keys_step_and_reset_from_where_the_grid_is_heading() {
    let (_root, mut app) = library(4);
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert_eq!(app.zoom, 110);
    // Repeated steps count from the target, not the tween's frame.
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert_eq!(app.zoom, 120);
    assert!(app.zooming.shown().unwrap() < 110.0);
    let _ = zoom(&mut app, Event::Key(Key::Out));
    assert_eq!(app.zoom, 110);
    let _ = zoom(&mut app, Event::Key(Key::Reset));
    assert_eq!(app.zoom, 100);
    for _ in 0..20 {
        let _ = zoom(&mut app, Event::Key(Key::In));
    }
    assert_eq!(app.zoom, 200);
    for _ in 0..20 {
        let _ = zoom(&mut app, Event::Key(Key::Out));
    }
    assert_eq!(app.zoom, 60);
}

#[test]
fn steps_go_to_the_next_multiple_of_ten_and_clamp() {
    assert_eq!(step(87, true), 90);
    assert_eq!(step(87, false), 80);
    assert_eq!(step(90, true), 100);
    assert_eq!(step(90, false), 80);
    assert_eq!(step(100, false), 90);
    assert_eq!(step(195, true), 200);
    assert_eq!(step(200, true), 200);
    assert_eq!(step(65, false), 60);
    assert_eq!(step(60, false), 60);

    // From a drag that let go at 87.
    let (_root, mut app) = library(4);
    let _ = zoom(&mut app, Event::Drag(87.0));
    let _ = zoom(&mut app, Event::DragEnded);
    assert_eq!(app.zoom, 87);
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert_eq!(app.zoom, 90);
    let _ = settle(&mut app);
    let _ = zoom(&mut app, Event::Drag(87.0));
    let _ = zoom(&mut app, Event::DragEnded);
    let _ = zoom(&mut app, Event::Step(false));
    assert_eq!(app.zoom, 80);
}

#[test]
fn a_drag_snaps_at_five_points_and_lets_go_past_seven() {
    assert_eq!(snap(95.0, false), (100, true));
    assert_eq!(snap(105.0, false), (100, true));
    assert_eq!(snap(94.9, false), (95, false));
    assert_eq!(snap(106.0, false), (106, false));
    assert_eq!(snap(107.0, true), (100, true));
    assert_eq!(snap(93.0, true), (100, true));
    assert_eq!(snap(107.1, true), (107, false));
    assert_eq!(snap(92.9, true), (93, false));
    assert_eq!(snap(10.0, false), (60, false));
    assert_eq!(snap(400.0, false), (200, false));

    let (_root, mut app) = library(4);
    let mut shown = Vec::new();
    for raw in [120.0, 106.0, 104.0, 106.5, 107.0, 107.5, 106.5, 105.0] {
        let _ = zoom(&mut app, Event::Drag(raw));
        shown.push(app.zoom);
    }
    assert_eq!(shown, [120, 106, 100, 100, 100, 108, 107, 100]);
    // The grid follows a drag directly, with no tween.
    assert!(!app.zooming.active());
}

#[test]
fn a_drag_takes_over_from_a_tween_at_once() {
    let (_root, mut app) = library(4);
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert!(app.zooming.active());
    let _ = zoom(&mut app, Event::Drag(150.0));
    assert!(!app.zooming.active());
    assert_eq!(app.zoom, 150);
    assert_eq!(app.grid_layout(), grid_motion::GridLayout::new(150));
    // Keys wait for the drag to end.
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert_eq!(app.zoom, 150);
}

#[test]
fn a_zoom_key_in_the_focused_search_zooms_and_leaves_focus_and_text_alone() {
    let (_root, mut app) = library(4);
    app.search = "game".into();
    let mut laid = lay_out(app.view(), app.window_size);
    focus(&mut laid, crate::SEARCH_ID);
    let mut sent = Vec::new();
    for character in ["=", "+", "-", "0"] {
        let messages = laid.event(&key_event(character, Modifiers::CTRL, Location::Standard));
        assert!(
            matches!(messages.as_slice(), [Message::Zoom(Event::Key(_))]),
            "{character}: {messages:?}"
        );
        sent.extend(messages);
        assert_eq!(laid.focused(), [iced::widget::Id::new(crate::SEARCH_ID)]);
    }
    // A toolbar control keeps its focus too.
    for id in [crate::sort_control::DIRECTION_BUTTON_ID, SLIDER_ID] {
        focus(&mut laid, id);
        let messages = laid.event(&key_event("=", Modifiers::CTRL, Location::Standard));
        assert!(matches!(
            messages.as_slice(),
            [Message::Zoom(Event::Key(Key::In))]
        ));
        assert_eq!(laid.focused(), [iced::widget::Id::new(id)]);
    }
    drop(laid);
    for message in sent {
        let _ = app.update(message);
    }
    assert_eq!(app.search, "game");
    assert_eq!(app.zoom, 100);
    assert!(app.zooming.active());
}

#[test]
fn zoom_keys_do_nothing_while_a_menu_popup_flow_or_dialog_has_the_keys() {
    let (_root, mut app) = library(4);
    let name = app.cards[0].name.clone();
    let opened: [fn(&mut App, &str); 4] = [
        |app, _| app.sort_menu = true,
        |app, _| app.filter_menu = true,
        |app, name| app.action_menu = Some(name.to_owned()),
        |app, _| app.flow = Some(crate::flow::Page::choose(app.paths.clone())),
    ];
    for open in opened {
        open(&mut app, &name);
        assert!(!app.zoom_allowed());
        // The key reaches whatever has the keys, not the zoom.
        let mut laid = lay_out(app.view(), app.window_size);
        let messages = laid.event(&key_event("=", Modifiers::CTRL, Location::Standard));
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, Message::Zoom(_))),
            "{messages:?}"
        );
        drop(laid);
        let _ = zoom(&mut app, Event::Key(Key::In));
        let _ = zoom(&mut app, Event::Wheel(NOTCH_UP));
        let _ = zoom(&mut app, Event::Drag(150.0));
        assert_eq!(app.zoom, 100);
        app.close_library_menus();
        app.flow = None;
    }
    app.hook_stop_confirm = Some(name);
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert_eq!(app.zoom, 100);
}

#[test]
fn the_toolbar_reads_add_search_sort_filters_then_the_zoom_controls() {
    let (_root, app) = library(4);
    let mut laid = lay_out(app.view(), app.window_size);
    let order = focus_order(&mut laid);
    let at = |id: &str| order.iter().position(|found| *found == id_text(id));
    let ids = [
        crate::SEARCH_ID,
        crate::sort_control::SORT_BUTTON_ID,
        crate::sort_control::DIRECTION_BUTTON_ID,
        crate::filter_control::FILTER_BUTTON_ID,
        SLIDER_ID,
        POSTER_ONLY_ID,
    ];
    let places: Vec<_> = ids.iter().map(|id| at(id).expect(id)).collect();
    assert!(places.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    // Left to right in the same order, with Add before the search.
    let xs: Vec<f32> = ids.iter().map(|id| bounds(&mut laid, id).x).collect();
    assert!(xs.windows(2).all(|pair| pair[0] < pair[1]), "{xs:?}");
    let slider = bounds(&mut laid, SLIDER_ID);
    assert_eq!(slider.size(), Size::new(96.0, 32.0));
    assert_eq!(
        bounds(&mut laid, POSTER_ONLY_ID).size(),
        Size::new(32.0, 32.0)
    );
}

#[test]
fn tab_reaches_the_slider_after_the_filter_tab_chips() {
    let (_root, mut app) = library(4);
    app.filters.stores.insert("Steam");
    app.reorder();
    let mut laid = lay_out(app.view(), app.window_size);
    let redraw = iced::Event::Window(iced::window::Event::RedrawRequested(Instant::now()));
    let messages = laid.event(&redraw);
    drop(laid);
    for message in messages {
        let _ = app.update(message);
    }
    assert!(app.filter_tab().is_some());
    let mut laid = lay_out(app.view(), app.window_size);
    focus(&mut laid, crate::filter_control::FILTER_BUTTON_ID);
    let mut seen = Vec::new();
    for _ in 0..4 {
        drive(&mut laid, crate::filter_tab::step(&app, false));
        seen.push(format!("{:?}", laid.focused()));
    }
    let slider = seen
        .iter()
        .position(|focused| focused.contains(SLIDER_ID))
        .expect("the slider in Tab's order");
    // The tab's x for Steam, then its clear-all, then the slider.
    assert_eq!(slider, 2, "{seen:?}");
    assert!(seen[slider + 1].contains(POSTER_ONLY_ID), "{seen:?}");
}

#[test]
fn the_search_box_shrinks_to_its_minimum_before_the_toolbar_clips() {
    let (_root, app) = library(4);
    let width = |window: f32| {
        let mut laid = lay_out(app.view(), Size::new(window, 800.0));
        bounds(&mut laid, crate::SEARCH_ID).width
    };
    assert_eq!(width(1200.0), crate::grid::SEARCH_WIDTH);
    assert_eq!(width(1600.0), crate::grid::SEARCH_WIDTH);
    let roomy = width(1200.0);
    let narrower = width(1000.0);
    assert!(narrower < roomy && narrower > crate::grid::SEARCH_MIN_WIDTH);
    // A narrower window takes the same pixels off the search box.
    assert_eq!(width(990.0), narrower - 10.0);
    assert_eq!(width(700.0), crate::grid::SEARCH_MIN_WIDTH);
    assert_eq!(width(400.0), crate::grid::SEARCH_MIN_WIDTH);
}

#[test]
fn the_slider_steps_on_its_arrows_only_while_focused() {
    let (_root, mut app) = library(4);
    let mut laid = lay_out(app.view(), app.window_size);
    let right = crate::sort_control::tests::press(Named::ArrowRight);
    let left = crate::sort_control::tests::press(Named::ArrowLeft);
    assert!(
        !laid
            .event(&right)
            .iter()
            .any(|message| matches!(message, Message::Zoom(_)))
    );
    focus(&mut laid, SLIDER_ID);
    let up = laid.event(&right);
    assert!(matches!(up.as_slice(), [Message::Zoom(Event::Step(true))]));
    let down = laid.event(&left);
    assert!(matches!(
        down.as_slice(),
        [Message::Zoom(Event::Step(false))]
    ));
    drop(laid);
    for message in up {
        let _ = app.update(message);
    }
    assert_eq!(app.zoom, 110);
    for message in down {
        let _ = app.update(message);
    }
    assert_eq!(app.zoom, 100);
    // The arrows the slider takes don't also move the grid's selection.
    let _ = app.update(Message::KeyWithFocus(
        keyboard::Key::Named(Named::ArrowRight),
        Modifiers::empty(),
        iced::event::Status::Captured,
        false,
    ));
    assert_eq!(app.selected, None);
}

#[test]
fn ctrl_wheel_zooms_over_the_grid_and_the_slider_and_the_plain_wheel_scrolls() {
    let (_root, mut app) = library(60);
    let mut laid = lay_out(app.view(), app.window_size);
    let slider = bounds(&mut laid, SLIDER_ID);
    // Plain wheel: the grid scrolls, and the slider does nothing.
    let _ = laid.event_at(&held(Modifiers::empty()), Cursor::Available(over_grid()));
    let messages = laid.event_at(&wheel(NOTCH_DOWN), Cursor::Available(over_grid()));
    assert!(
        messages
            .iter()
            .all(|message| !matches!(message, Message::Zoom(_)))
    );
    assert!(grid_offset(&mut laid) > 0.0);
    // Moving there leaves the grid, which is all that happens.
    let messages = laid.event_at(&wheel(NOTCH_DOWN), Cursor::Available(slider.center()));
    assert!(
        matches!(messages.as_slice(), [Message::Zoom(Event::PointerLeft)]),
        "{messages:?}"
    );
    drop(laid);

    let mut laid = lay_out(app.view(), app.window_size);
    let _ = laid.event_at(&held(Modifiers::CTRL), Cursor::Available(over_grid()));
    let mut sent = Vec::new();
    for at in [over_grid(), slider.center()] {
        let messages = laid.event_at(&wheel(NOTCH_UP), Cursor::Available(at));
        assert!(
            matches!(messages.as_slice(), [Message::Zoom(Event::Wheel(_))]),
            "{messages:?}"
        );
        sent.extend(messages);
    }
    assert_eq!(grid_offset(&mut laid), 0.0);
    drop(laid);
    for message in sent {
        let _ = app.update(message);
    }
    assert_eq!(app.zoom, 120);
}

#[test]
fn ctrl_wheel_over_the_panel_or_the_filter_tab_does_what_the_wheel_does_there() {
    let (_root, mut app) = library(60);
    for card in &mut app.cards {
        card.tags = vec!["game".into()];
    }
    let _ = app.update(Message::Filter(crate::filter_control::Event::ToggleTag(
        "game".into(),
    )));
    let redraw = iced::Event::Window(iced::window::Event::RedrawRequested(Instant::now()));
    let _ = send(&mut app, &redraw, Cursor::Unavailable);
    let probe = crate::layout_probe::probe(&mut app.view(), app.window_size);
    let tab = bounds_at(&probe.node, &[1, 0]);
    let mut laid = lay_out(app.view(), app.window_size);
    let _ = laid.event_at(&held(Modifiers::CTRL), Cursor::Available(tab.center()));
    let messages = laid.event_at(&wheel(NOTCH_DOWN), Cursor::Available(tab.center()));
    assert!(
        matches!(
            messages.as_slice(),
            [Message::Filter(crate::filter_control::Event::Wheel(_))]
        ),
        "{messages:?}"
    );
    drop(laid);

    // The docked panel scrolls itself, or nothing, but never zooms.
    let name = app.filtered()[0].name.clone();
    let _ = app.update(Message::Select(name));
    let _ = app.update(Message::GridTick(
        app.grid_motion.generation(),
        Instant::now() + Duration::from_secs(1),
    ));
    app.panel_motion.snap(true);
    let mut laid = lay_out(app.view(), app.window_size);
    let panel = Point::new(app.window_size.width - 200.0, 400.0);
    let _ = laid.event_at(&held(Modifiers::CTRL), Cursor::Available(panel));
    let messages = laid.event_at(&wheel(NOTCH_UP), Cursor::Available(panel));
    assert!(
        messages
            .iter()
            .all(|message| !matches!(message, Message::Zoom(_))),
        "{messages:?}"
    );
    assert_eq!(app.zoom, 100);
}

#[test]
fn ctrl_wheel_at_either_end_of_the_range_is_still_taken() {
    let (_root, mut app) = library(60);
    for (zoom_at, delta) in [(200, NOTCH_UP), (60, NOTCH_DOWN)] {
        app.zoom = zoom_at;
        let mut laid = lay_out(app.view(), app.window_size);
        let _ = laid.event_at(&held(Modifiers::CTRL), Cursor::Available(over_grid()));
        let messages = laid.event_at(&wheel(delta), Cursor::Available(over_grid()));
        assert_eq!(grid_offset(&mut laid), 0.0);
        drop(laid);
        for message in messages {
            let _ = app.update(message);
        }
        assert_eq!(app.zoom, zoom_at);
        assert!(!app.zooming.active());
        assert!(app.view_writes.zoom.is_none());
    }
}

#[test]
fn trackpad_pixels_add_up_to_steps_and_reset() {
    let pixels = |y| Event::Wheel(ScrollDelta::Pixels { x: 0.0, y });
    let (_root, mut app) = library(4);
    let _ = zoom(&mut app, pixels(30.0));
    assert_eq!(app.zoom, 100);
    let _ = zoom(&mut app, pixels(40.0));
    assert_eq!(app.zoom, 110);
    // What's left over (10) still counts toward the next step.
    let _ = zoom(&mut app, pixels(50.0));
    assert_eq!(app.zoom, 120);

    // Releasing Ctrl drops a partial step.
    let _ = zoom(&mut app, pixels(40.0));
    let _ = app.update(Message::Modifiers(Modifiers::empty()));
    let _ = zoom(&mut app, pixels(40.0));
    assert_eq!(app.zoom, 120);
    // So does turning back.
    let _ = zoom(&mut app, pixels(-40.0));
    assert_eq!(app.zoom, 120);
    let _ = zoom(&mut app, pixels(-30.0));
    assert_eq!(app.zoom, 110);
    // And leaving the grid.
    let _ = zoom(&mut app, pixels(-40.0));
    let _ = zoom(&mut app, Event::PointerLeft);
    let _ = zoom(&mut app, pixels(-40.0));
    assert_eq!(app.zoom, 110);
    // A wheel's notches step once each, fast turns included.
    let _ = zoom(
        &mut app,
        Event::Wheel(ScrollDelta::Lines { x: 0.0, y: 2.0 }),
    );
    assert_eq!(app.zoom, 130);
}

/// The layout's top of each shown card, as the grid places them.
fn drawn_tops(app: &App) -> (Vec<f32>, f32) {
    let node = layout(
        &mut app.grid(),
        Size::new(app.viewport_width, f32::INFINITY),
    );
    let tops = (0..app.filtered().len())
        .map(|index| bounds_at(&node, &[index]).y)
        .collect();
    (tops, node.bounds().height)
}

fn index_of(app: &App, name: &str) -> usize {
    app.filtered()
        .iter()
        .position(|card| card.name == name)
        .unwrap()
}

#[test]
fn the_anchor_card_keeps_its_place_through_a_zoom() {
    let (_root, mut app) = library(120);
    let (tops, _) = drawn_tops(&app);
    // The selected card is in view, a little below the viewport's top.
    app.scroll_offset = tops[21] - 120.0;
    app.selected = Some(app.filtered()[21].name.clone());
    for steps in [Key::In, Key::In, Key::Out, Key::Out, Key::Out] {
        let _ = zoom(&mut app, Event::Key(steps));
        // Every frame, not just the last, holds the anchor.
        for ms in [40, 80, 120, 160] {
            let _ = frame(&mut app, ms);
            let (tops, _) = drawn_tops(&app);
            assert_eq!(
                tops[21] - app.scroll_offset,
                120.0,
                "{:?} at {ms} ms",
                app.grid_layout()
            );
        }
    }
    assert_eq!(app.zoom, 90);
    assert!(app.zooming.gesture.is_none());
}

#[test]
fn the_selected_card_anchors_when_any_of_it_is_on_screen() {
    let (_root, mut app) = library(120);
    let (tops, _) = drawn_tops(&app);
    app.scroll_offset = tops[20] + 100.0;
    app.selected = Some(app.filtered()[25].name.clone());
    let offset = tops[25] - app.scroll_offset;
    // Card 20 is the first in view, but the selection is in view too.
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert_eq!(
        app.zooming.gesture.as_ref().unwrap().anchor,
        Some((app.filtered()[25].name.clone(), offset))
    );
    let _ = settle(&mut app);
    let (tops, _) = drawn_tops(&app);
    assert_eq!(tops[25] - app.scroll_offset, offset);
}

#[test]
fn the_anchor_clamps_to_the_scroll_range() {
    let (_root, mut app) = library(120);
    // At the bottom, zooming out shortens the grid under the anchor.
    let (_, height) = drawn_tops(&app);
    app.scroll_offset = height - app.viewport_height;
    let _ = zoom(&mut app, Event::Key(Key::Out));
    let _ = settle(&mut app);
    let (_, height) = drawn_tops(&app);
    assert_eq!(app.scroll_offset, height - app.viewport_height);

    // At the top, zooming out moves the second row's card into the first
    // row, where it can't come back down to its old place.
    app.scroll_offset = 0.0;
    let _ = zoom(&mut app, Event::Key(Key::Reset));
    let _ = settle(&mut app);
    app.selected = Some(app.filtered()[app.columns()].name.clone());
    let _ = zoom(&mut app, Event::Key(Key::Out));
    let _ = settle(&mut app);
    // The column change springs the cards on past the tween.
    let _ = crate::tests::settle_grid(&mut app);
    assert_eq!(app.scroll_offset, 0.0);
    assert_eq!(drawn_tops(&app).0[app.columns() - 1], 16.0);
}

#[test]
fn a_card_taller_than_the_viewport_aligns_its_top() {
    let (_root, mut app) = library(40);
    app.viewport_height = 200.0;
    let (tops, _) = drawn_tops(&app);
    app.scroll_offset = tops[10] - 20.0;
    app.selected = Some(app.filtered()[10].name.clone());
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = settle(&mut app);
    let (tops, _) = drawn_tops(&app);
    assert_eq!(app.scroll_offset, tops[10]);
}

#[test]
fn input_that_moves_the_grid_ends_the_anchoring() {
    let (_root, mut app) = library(120);
    let (tops, _) = drawn_tops(&app);
    app.scroll_offset = tops[21] - 120.0;
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert!(app.zooming.anchoring());
    // The grid hands on a scroll or a press only while anchoring.
    let mut laid = lay_out(app.view(), app.window_size);
    let messages = laid.event_at(&wheel(NOTCH_DOWN), Cursor::Available(over_grid()));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Zoom(Event::Interrupted)))
    );
    drop(laid);
    for message in messages {
        let _ = app.update(message);
    }
    assert!(!app.zooming.anchoring());
    let offset = app.scroll_offset;
    let _ = settle(&mut app);
    assert_eq!(app.scroll_offset, offset);
    let mut laid = lay_out(app.view(), app.window_size);
    let messages = laid.event_at(&wheel(NOTCH_DOWN), Cursor::Available(over_grid()));
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, Message::Zoom(_)))
    );
    drop(laid);

    // An arrow key does too.
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert!(app.zooming.anchoring());
    let _ = app.update(Message::KeyWithFocus(
        keyboard::Key::Named(Named::ArrowRight),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
    assert!(!app.zooming.anchoring());
}

#[test]
fn an_off_screen_selection_scrolls_into_view_when_the_zoom_ends() {
    let (_root, mut app) = library(120);
    let name = app.filtered()[100].name.clone();
    app.selected = Some(name.clone());
    let _ = zoom(&mut app, Event::Key(Key::In));
    // The first card in view anchors instead.
    assert_eq!(
        app.zooming.gesture.as_ref().unwrap().anchor,
        Some((app.filtered()[0].name.clone(), 16.0))
    );
    let early = frame(&mut app, 80);
    assert!(!located(&app, early, &name));
    let task = settle(&mut app);
    assert!(located(&app, task, &name));
    // A selection already in view stays where the anchor holds it.
    let (_root, mut app) = library(120);
    app.selected = Some(app.filtered()[2].name.clone());
    let _ = zoom(&mut app, Event::Key(Key::In));
    let task = settle(&mut app);
    let selected = app.selected.clone().unwrap();
    assert!(!located(&app, task, &selected));
    assert_eq!(index_of(&app, &selected), 2);
}

/// Whether `task` asks where the card `name` is, which is how the grid
/// scrolls the selection into view.
fn located(app: &App, task: Task<Message>, name: &str) -> bool {
    crate::sort_control::tests::answers(app, task)
        .iter()
        .any(|message| matches!(message, Message::GridCardLocated(found, ..) if found == name))
}

#[test]
fn the_tween_runs_frames_only_until_it_ends() {
    let recipes =
        |app: &App| iced::advanced::subscription::into_recipes(app.zooming.subscription()).len();
    let (_root, mut app) = library(4);
    assert_eq!(recipes(&app), 0);
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert_eq!(recipes(&app), 1);
    let start = app.zooming.tween.unwrap().start;
    let _ = frame(&mut app, 80);
    let halfway = app.zooming.shown().unwrap();
    assert!(halfway > 100.0 && halfway < 110.0, "{halfway}");
    // A frame from before the latest step is ignored.
    let stale = app.zooming.generation;
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = zoom(
        &mut app,
        Event::Frame(stale, start + Duration::from_secs(1)),
    );
    assert!(app.zooming.active());
    let _ = settle(&mut app);
    assert!(!app.zooming.active());
    assert_eq!(app.grid_layout(), grid_motion::GridLayout::new(120));
    assert_eq!(recipes(&app), 0);
    // Leaving the Library settles a tween at its target.
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    assert!(!app.zooming.active());
    assert_eq!(app.zoom, 130);
}

#[test]
fn a_step_saves_the_target_and_never_a_frame() {
    let (_root, mut app) = library(4);
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert_eq!(app.view_writes.zoom, Some(110));
    // A step while the write runs waits for it, then saves the latest.
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = frame(&mut app, 80);
    assert_eq!(app.view_writes.zoom, Some(110));
    finish_write(&mut app);
    assert_eq!(app.view_writes.zoom, Some(120));
    finish_write(&mut app);
    assert!(app.view_writes.zoom.is_none());
    let saved = wineo_core::library::load(&app.paths);
    assert_eq!(saved.view.zoom, 120);
    assert!(app.feature_notice.is_none());
}

#[test]
fn a_drag_saves_once_when_it_ends() {
    let (_root, mut app) = library(4);
    for raw in [120.0, 130.5, 141.2, 150.0] {
        let _ = zoom(&mut app, Event::Drag(raw));
        assert_eq!(app.view_writes.zoom, None);
    }
    let _ = zoom(&mut app, Event::DragEnded);
    assert_eq!(app.view_writes.zoom, Some(150));
    finish_write(&mut app);
    assert_eq!(wineo_core::library::load(&app.paths).view.zoom, 150);
    // A second release saves nothing.
    let _ = zoom(&mut app, Event::DragEnded);
    assert_eq!(app.view_writes.zoom, None);
}

#[test]
fn closing_waits_for_an_unsaved_zoom() {
    // A drag the close interrupts.
    let (_root, mut app) = library(4);
    let _ = zoom(&mut app, Event::Drag(150.0));
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(!app.zooming.dragging());
    assert_eq!(app.view_writes.zoom, Some(150));
    assert!(app.closing && app.close_quietly && !app.exiting);
    finish_write(&mut app);
    assert_eq!(wineo_core::library::load(&app.paths).view.zoom, 150);

    // A step whose write is still running.
    let (_root, mut app) = library(4);
    let _ = zoom(&mut app, Event::Key(Key::Out));
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(app.closing && app.close_quietly && !app.exiting);
    let Writes { zoom, .. } = app.view_writes;
    assert_eq!(zoom, Some(90));
}

#[test]
fn the_toggle_saves_poster_only_and_boot_reads_it() {
    let (root, mut app) = library(4);
    assert!(!app.poster_only);
    let _ = zoom(&mut app, Event::TogglePosterOnly);
    assert!(app.poster_only);
    let written = Field::PosterOnly(app.view_writes.poster_only.expect("a write"));
    let result = save(&app.paths, &app.shown, written);
    let _ = app.update(Message::Sort(crate::sort_control::Event::Saved(
        written, result,
    )));
    assert!(wineo_core::library::load(&app.paths).view.poster_only);
    let app = App::boot(app.paths.clone()).0;
    assert!(app.poster_only);
    drop(root);
}

/// Presses Tab the way the app does, against the view laid out over
/// `tree`, and returns the tree and what has focus after.
fn press_tab(
    app: &mut App,
    tree: iced::advanced::widget::Tree,
) -> (iced::advanced::widget::Tree, Vec<iced::widget::Id>) {
    let task = app.update(Message::KeyWithFocus(
        keyboard::Key::Named(Named::Tab),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
    let mut laid = crate::layout_probe::relay_out(app.view(), tree, app.window_size);
    drive(&mut laid, task);
    let focused = laid.focused();
    (laid.into_tree(), focused)
}

fn busy() -> crate::Operation {
    crate::Operation {
        name: "game-000".into(),
        kind: crate::OperationKind::Start,
        token: None,
        cancelled: false,
        status: String::new(),
        install_done: None,
        installing: false,
        after_hook: false,
    }
}

#[test]
fn tab_cycles_the_toolbar_from_add_in_its_order() {
    let (_root, mut app) = library(4);
    let order = [
        crate::grid::ADD_ID,
        crate::SEARCH_ID,
        crate::sort_control::SORT_BUTTON_ID,
        crate::sort_control::DIRECTION_BUTTON_ID,
        crate::filter_control::FILTER_BUTTON_ID,
        SLIDER_ID,
        POSTER_ONLY_ID,
    ];
    let mut laid = lay_out(app.view(), app.window_size);
    focus(&mut laid, crate::grid::ADD_ID);
    let mut tree = laid.into_tree();
    let mut seen = Vec::new();
    for _ in 0..order.len() + 1 {
        let (next, focused) = press_tab(&mut app, tree);
        tree = next;
        seen.push(focused);
    }
    let expected: Vec<Vec<iced::widget::Id>> = order[1..]
        .iter()
        .map(|id| vec![iced::widget::Id::new(id)])
        .collect();
    assert_eq!(seen[..expected.len()], expected[..], "{seen:?}");
    // Past the toggle focus leaves the toolbar, and the next Tab starts
    // over at Add.
    assert_eq!(seen[expected.len()], Vec::<iced::widget::Id>::new());
    assert_eq!(
        seen[expected.len() + 1],
        [iced::widget::Id::new(crate::grid::ADD_ID)]
    );
}

#[test]
fn enter_and_space_on_add_send_what_a_click_does() {
    let (_root, app) = library(4);
    let mut laid = lay_out(app.view(), app.window_size);
    focus(&mut laid, crate::grid::ADD_ID);
    for key in [Named::Enter, Named::Space] {
        let messages = laid.event(&crate::sort_control::tests::press(key));
        assert!(
            matches!(messages.as_slice(), [Message::Add]),
            "{messages:?}"
        );
    }
}

#[test]
fn a_disabled_add_takes_no_focus_and_tab_skips_it() {
    let (_root, mut app) = library(4);
    let mut laid = lay_out(app.view(), app.window_size);
    focus(&mut laid, crate::grid::ADD_ID);
    assert_eq!(laid.focused(), [iced::widget::Id::new(crate::grid::ADD_ID)]);
    let tree = laid.into_tree();
    // Busy work disables Add while it has focus; the focus goes, and Tab
    // starts the cycle at the search box.
    app.operation = Some(busy());
    let mut laid = crate::layout_probe::relay_out(app.view(), tree, app.window_size);
    assert!(laid.focused().is_empty());
    assert!(
        laid.event(&crate::sort_control::tests::press(Named::Enter))
            .iter()
            .all(|message| !matches!(message, Message::Add))
    );
    focus(&mut laid, crate::grid::ADD_ID);
    assert!(laid.focused().is_empty());
    let mut tree = laid.into_tree();
    let mut seen = Vec::new();
    for _ in 0..9 {
        let (next, focused) = press_tab(&mut app, tree);
        tree = next;
        seen.push(focused);
    }
    assert_eq!(
        seen[0],
        [iced::widget::Id::new(crate::SEARCH_ID)],
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .flatten()
            .all(|id| *id != iced::widget::Id::new(crate::grid::ADD_ID))
    );
    assert_eq!(seen[6], Vec::<iced::widget::Id>::new(), "{seen:?}");
    assert_eq!(
        seen[7],
        [iced::widget::Id::new(crate::SEARCH_ID)],
        "{seen:?}"
    );
}

#[test]
fn a_drag_saves_only_its_release_even_when_a_step_write_finishes_during_it() {
    let (_root, mut app) = library(4);
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert_eq!(app.view_writes.zoom, Some(110));
    for raw in [130.0, 140.0] {
        let _ = zoom(&mut app, Event::Drag(raw));
    }
    // The step's write finishing starts nothing for the values in between.
    finish_write(&mut app);
    assert_eq!(app.view_writes.zoom, None);
    assert_eq!(wineo_core::library::load(&app.paths).view.zoom, 110);
    let _ = zoom(&mut app, Event::Drag(150.0));
    let _ = zoom(&mut app, Event::DragEnded);
    assert_eq!(app.view_writes.zoom, Some(150));
    finish_write(&mut app);
    assert_eq!(app.view_writes.zoom, None);
    assert_eq!(wineo_core::library::load(&app.paths).view.zoom, 150);

    // A release while the step's write still runs saves the release after it.
    let _ = settle(&mut app);
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = zoom(&mut app, Event::Drag(130.0));
    let _ = zoom(&mut app, Event::Drag(170.0));
    let _ = zoom(&mut app, Event::DragEnded);
    assert_eq!(app.view_writes.zoom, Some(160));
    finish_write(&mut app);
    assert_eq!(app.view_writes.zoom, Some(170));
    finish_write(&mut app);
    assert_eq!(wineo_core::library::load(&app.paths).view.zoom, 170);

    // So does a close that interrupts the drag.
    let _ = zoom(&mut app, Event::Key(Key::Out));
    let _ = zoom(&mut app, Event::Drag(80.0));
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(app.closing && !app.exiting);
    finish_write(&mut app);
    assert_eq!(app.view_writes.zoom, Some(80));
    finish_write(&mut app);
    assert_eq!(wineo_core::library::load(&app.paths).view.zoom, 80);
}

#[test]
fn ctrl_held_before_the_grid_was_built_still_zooms() {
    let (_root, mut app) = library(60);
    // Ctrl went down, then Ctrl+2 and Ctrl+1 left and came back, so the
    // grid and the slider are new while Ctrl stays down.
    let _ = app.update(Message::Modifiers(Modifiers::CTRL));
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    let mut laid = lay_out(app.view(), app.window_size);
    let slider = bounds(&mut laid, SLIDER_ID);
    let mut sent = Vec::new();
    for at in [over_grid(), slider.center()] {
        let messages = laid.event_at(&wheel(NOTCH_UP), Cursor::Available(at));
        assert!(
            messages
                .iter()
                .any(|message| matches!(message, Message::Zoom(Event::Wheel(_)))),
            "{messages:?}"
        );
        sent.extend(messages);
    }
    assert_eq!(grid_offset(&mut laid), 0.0);
    drop(laid);
    for message in sent {
        let _ = app.update(message);
    }
    assert_eq!(app.zoom, 120);

    // Losing focus with Ctrl down doesn't leave it stuck.
    let _ = app.update(Message::Window(iced::window::Event::Unfocused));
    assert_eq!(app.modifiers, Modifiers::empty());
    let mut laid = lay_out(app.view(), app.window_size);
    let messages = laid.event_at(&wheel(NOTCH_DOWN), Cursor::Available(over_grid()));
    assert!(
        messages
            .iter()
            .all(|message| !matches!(message, Message::Zoom(Event::Wheel(_))))
    );
    assert!(grid_offset(&mut laid) > 0.0);
    drop(laid);
    // The grid built before the focus left lets Ctrl go too.
    let _ = app.update(Message::Modifiers(Modifiers::CTRL));
    let mut laid = lay_out(app.view(), app.window_size);
    let _ = laid.event(&iced::Event::Window(iced::window::Event::Unfocused));
    let messages = laid.event_at(&wheel(NOTCH_DOWN), Cursor::Available(over_grid()));
    assert!(
        messages
            .iter()
            .all(|message| !matches!(message, Message::Zoom(Event::Wheel(_))))
    );
}

#[test]
fn a_selection_located_before_a_zoom_frame_is_dropped() {
    let (_root, mut app) = library(120);
    let name = app.filtered()[60].name.clone();
    app.selected = Some(name.clone());
    let before = app.grid_revision();
    let stale = crate::grid_motion::CardBounds {
        card: Rectangle::new(Point::new(32.0, 5000.0), Size::new(160.0, 300.0)),
        viewport: Rectangle::new(Point::new(0.0, 50.0), Size::new(800.0, 600.0)),
        offset: iced::Vector::ZERO,
    };
    // Steps and drags that change the drawn grid each move the revision.
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = frame(&mut app, 40);
    assert_ne!(app.grid_revision(), before);
    let offset = app.scroll_offset;
    let _ = app.update(Message::GridCardLocated(name.clone(), before, stale));
    assert_eq!(app.scroll_offset, offset);
    let during = app.grid_revision();
    let _ = frame(&mut app, 80);
    assert_ne!(app.grid_revision(), during);
    let _ = app.update(Message::GridCardLocated(name.clone(), during, stale));
    assert_eq!(app.scroll_offset, offset);
    let _ = settle(&mut app);
    let dragged = app.grid_revision();
    let _ = zoom(&mut app, Event::Drag(150.0));
    assert_ne!(app.grid_revision(), dragged);
    let _ = zoom(&mut app, Event::DragEnded);
    let offset = app.scroll_offset;
    let _ = app.update(Message::GridCardLocated(name.clone(), dragged, stale));
    assert_eq!(app.scroll_offset, offset);
    // One measured now still scrolls.
    let _ = app.update(Message::GridCardLocated(name, app.grid_revision(), stale));
    assert_eq!(app.scroll_offset, 5000.0 - 50.0 + 300.0 - 600.0);
}

#[test]
fn an_arrow_during_a_tween_scrolls_its_selection_once_the_tween_ends() {
    let (_root, mut app) = library(120);
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = frame(&mut app, 40);
    app.selected = Some(app.filtered()[99].name.clone());
    let _ = app.update(Message::KeyWithFocus(
        keyboard::Key::Named(Named::ArrowRight),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
    let name = app.selected.clone().unwrap();
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some(name.as_str()));
    let task = settle(&mut app);
    assert!(located(&app, task, &name));
}

fn arrow_right(app: &mut App) {
    let _ = app.update(Message::KeyWithFocus(
        keyboard::Key::Named(Named::ArrowRight),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
}

#[test]
fn an_arrow_before_a_drag_cuts_the_tween_short_scrolls_once_the_cards_settle() {
    let (_root, mut app) = library(120);
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = frame(&mut app, 40);
    app.selected = Some(app.filtered()[99].name.clone());
    arrow_right(&mut app);
    let name = app.selected.clone().unwrap();
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some(name.as_str()));
    // The drag removes the tween but keeps the scroll waiting.
    let _ = zoom(&mut app, Event::Drag(150.0));
    assert!(!app.zooming.active());
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some(name.as_str()));
    let release = zoom(&mut app, Event::DragEnded);
    // 150% has fewer columns, so the cards still spring after the release.
    assert!(app.grid_motion.active());
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some(name.as_str()));
    let settled = crate::tests::settle_grid(&mut app);
    assert!(app.grid_scroll_after_settle.is_none());
    assert_eq!(
        crate::sort_control::tests::answers(&app, Task::batch([release, settled]))
            .iter()
            .filter(
                |message| matches!(message, Message::GridCardLocated(found, ..) if *found == name)
            )
            .count(),
        1
    );
    // Nothing is left for a later panel close to scroll back to.
    app.scroll_offset = 0.0;
    let _ = app.update(Message::Select(name.clone()));
    let _ = app.update(Message::ClosePanel);
    assert!(app.grid_scroll_after_settle.is_none());
}

#[test]
fn leaving_the_library_mid_tween_drops_an_arrows_waiting_scroll() {
    let (_root, mut app) = library(120);
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = frame(&mut app, 40);
    app.selected = Some(app.filtered()[99].name.clone());
    arrow_right(&mut app);
    assert!(app.grid_scroll_after_settle.is_some());
    let task = app.update(Message::Section(crate::sidebar::Section::Trash));
    assert!(!app.zooming.moving());
    assert!(app.grid_scroll_after_settle.is_none());
    let name = app.selected.clone().unwrap();
    assert!(!located(&app, task, &name));
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert!(app.grid_scroll_after_settle.is_none());
}

#[test]
fn a_resize_during_the_dock_resolves_an_arrows_waiting_scroll_once() {
    let (_root, mut app) = library(120);
    let first = app.filtered()[0].name.clone();
    let _ = app.update(Message::Select(first));
    assert!(app.panel_open && app.grid_motion.active());
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = frame(&mut app, 40);
    app.selected = Some(app.filtered()[98].name.clone());
    arrow_right(&mut app);
    let name = app.selected.clone().unwrap();
    // The zoom ends while the dock still moves, which keeps the scroll.
    let task = settle(&mut app);
    assert!(app.grid_motion.active());
    assert!(!located(&app, task, &name));
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some(name.as_str()));
    // The resize snaps the dock, which is the last motion to end.
    let task = app.update(Message::Window(iced::window::Event::Resized(Size::new(
        1300.0, 800.0,
    ))));
    assert!(!app.grid_motion.active());
    assert!(app.grid_scroll_after_settle.is_none());
    assert_eq!(
        crate::sort_control::tests::answers(&app, task)
            .iter()
            .filter(
                |message| matches!(message, Message::GridCardLocated(found, ..) if *found == name)
            )
            .count(),
        1
    );
    // A later panel close and open has nothing left to scroll back to.
    app.scroll_offset = 0.0;
    let _ = app.update(Message::ClosePanel);
    let _ = app.update(Message::GridTick(
        app.grid_motion.generation(),
        Instant::now() + Duration::from_secs(1),
    ));
    let close_open = app.update(Message::Select(name.clone()));
    let tick = app.update(Message::GridTick(
        app.grid_motion.generation(),
        Instant::now() + Duration::from_secs(1),
    ));
    assert!(app.grid_scroll_after_settle.is_none());
    assert!(!located(&app, Task::batch([close_open, tick]), &name));
}

fn arrow_during_a_zoom(change: fn(&mut App)) -> (tempfile::TempDir, App, String) {
    let (root, mut app) = library(120);
    for card in &mut app.cards {
        card.tags = vec!["game".into()];
    }
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = frame(&mut app, 40);
    app.selected = Some(app.filtered()[98].name.clone());
    arrow_right(&mut app);
    let name = app.selected.clone().unwrap();
    assert!(app.grid_scroll_after_settle.is_some());
    change(&mut app);
    // The selection is still shown, and the zoom is still moving.
    assert!(app.selected_shown() && app.zooming.moving());
    (root, app, name)
}

#[test]
fn a_search_during_a_zoom_drops_an_arrows_waiting_scroll() {
    let (_root, mut app, name) = arrow_during_a_zoom(|app| {
        let _ = app.update(Message::SearchChanged("game".into()));
    });
    assert!(app.grid_scroll_after_settle.is_none());
    let task = settle(&mut app);
    assert!(!located(&app, task, &name));
}

#[test]
fn a_sort_or_filter_during_a_zoom_replaces_the_arrows_scroll_with_its_own() {
    let changes: [fn(&mut App); 2] = [
        |app| {
            let _ = app.update(Message::Sort(crate::sort_control::Event::FlipDirection));
        },
        |app| {
            let _ = app.update(Message::Filter(crate::filter_control::Event::ToggleTag(
                "game".into(),
            )));
        },
    ];
    for change in changes {
        // A new order keeps the shown selection in view, which waits for
        // the zoom like any scroll, and happens once.
        let (_root, mut app, name) = arrow_during_a_zoom(change);
        assert_eq!(app.grid_scroll_after_settle.as_deref(), Some(name.as_str()));
        let task = settle(&mut app);
        // A sort's cards are still moving after the tween; a filter that
        // keeps every card moves none.
        let settled = if app.grid_motion.active() {
            assert_eq!(app.grid_scroll_after_settle.as_deref(), Some(name.as_str()));
            crate::tests::settle_grid(&mut app)
        } else {
            Task::none()
        };
        assert!(app.grid_scroll_after_settle.is_none());
        assert_eq!(
            crate::sort_control::tests::answers(&app, Task::batch([task, settled]))
                .iter()
                .filter(
                    |message| matches!(message, Message::GridCardLocated(found, ..) if *found == name)
                )
                .count(),
            1
        );
    }
}

fn close_to(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn a_zoom_started_mid_dock_keeps_the_place_as_drawn() {
    let (_root, mut app) = library(120);
    let (tops, _) = drawn_tops(&app);
    app.scroll_offset = tops[21] - 120.0;
    let name = app.filtered()[21].name.clone();
    let _ = app.update(Message::Select(name.clone()));
    assert!(app.panel_open && app.grid_motion.active());
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(100 + ms);
    let _ = app.update(Message::GridTick(app.grid_motion.generation(), at(0)));
    // Scrolled so the moving card is in view, where it anchors.
    app.scroll_offset = drawn_tops(&app).0[21] - 120.0;
    let drawn = 120.0;
    // The dock's target layout puts the card somewhere else, which is what
    // the anchor used to read.
    let target = app.grid_places().slots[21].y - app.scroll_offset;
    assert!((drawn - target).abs() > 20.0, "{drawn} vs {target}");

    let _ = zoom(&mut app, Event::Key(Key::In));
    let anchor = app
        .zooming
        .gesture
        .as_ref()
        .unwrap()
        .anchor
        .clone()
        .unwrap();
    assert_eq!(anchor.0, name);
    assert!(close_to(anchor.1, drawn), "{anchor:?} vs {drawn}");
    let generation = app.zooming.generation;
    // The tween starts on the dock's clock, so neither depends on how long
    // the test takes.
    app.zooming.tween.as_mut().unwrap().start = at(0);
    for ms in [20, 80, 160] {
        let _ = app.update(Message::GridTick(app.grid_motion.generation(), at(ms)));
        let _ = zoom(&mut app, Event::Frame(generation, at(ms)));
        let top = drawn_tops(&app).0[21] - app.scroll_offset;
        assert!(close_to(top, drawn), "{top} vs {drawn} at {ms} ms");
    }
    assert!(!app.zooming.moving());
    // The cards the dock and the zoom left moving still keep the place
    // until they stop.
    assert!(app.grid_motion.active());
    let _ = app.update(Message::GridTick(app.grid_motion.generation(), at(200)));
    let top = drawn_tops(&app).0[21] - app.scroll_offset;
    assert!(close_to(top, drawn), "{top} vs {drawn}");
    let _ = crate::tests::settle_grid(&mut app);
    assert!(!app.grid_motion.active());
    assert!(app.grid_motion.anchor.is_none());
    let top = drawn_tops(&app).0[21] - app.scroll_offset;
    assert!(close_to(top, drawn), "{top} vs {drawn}");
}

#[test]
fn a_zoom_springs_only_when_the_column_count_changes_and_then_goes_idle() {
    let (_root, mut app) = library(120);
    let frames = |app: &App| {
        iced::advanced::subscription::into_recipes(app.grid_motion.subscription()).len()
    };
    let mut changed = 0;
    while app.zoom < ZOOM_MAX {
        let columns = app.columns();
        let _ = zoom(&mut app, Event::Key(Key::In));
        let _ = settle(&mut app);
        if app.columns() == columns {
            assert!(!app.grid_motion.active(), "{}%", app.zoom);
        } else {
            changed += 1;
            assert!(app.grid_motion.active(), "{}%", app.zoom);
            assert_eq!(frames(&app), 1);
            let _ = crate::tests::settle_grid(&mut app);
        }
        assert_eq!(frames(&app), 0, "{}%", app.zoom);
    }
    assert!(changed > 0);
}

#[test]
fn a_search_during_a_zoom_ends_its_anchoring_and_its_reveal() {
    let (_root, mut app) = library(120);
    let (tops, _) = drawn_tops(&app);
    app.scroll_offset = tops[40];
    // Off screen, so the gesture would reveal it at the end.
    let name = app.filtered()[110].name.clone();
    app.selected = Some(name.clone());
    let _ = zoom(&mut app, Event::Key(Key::In));
    assert!(app.zooming.anchoring());
    assert!(app.zooming.gesture.as_ref().unwrap().reveal);
    let _ = frame(&mut app, 40);
    // Every card still matches, the anchor included.
    let _ = app.update(Message::SearchChanged("game-".into()));
    assert_eq!(app.filtered().len(), 120);
    assert_eq!(app.scroll_offset, 0.0);
    assert!(!app.zooming.anchoring());
    let next = frame(&mut app, 80);
    assert_eq!(app.scroll_offset, 0.0);
    let end = settle(&mut app);
    let settled = crate::tests::settle_grid(&mut app);
    assert_eq!(app.scroll_offset, 0.0);
    assert!(!located(&app, Task::batch([next, end, settled]), &name));
}

#[test]
fn a_resort_during_a_zoom_scrolls_an_off_screen_selection_once_at_the_end() {
    let (_root, mut app) = library(120);
    let name = app.filtered()[100].name.clone();
    app.selected = Some(name.clone());
    let _ = zoom(&mut app, Event::Key(Key::In));
    let _ = frame(&mut app, 40);
    // Nothing asks where the card is drawn mid-motion, so no early reply
    // can scroll to its old place.
    let resort = app.update(Message::Sort(crate::sort_control::Event::FlipDirection));
    assert!(app.selected_shown());
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some(name.as_str()));
    assert!(!located(&app, resort, &name));
    let end = settle(&mut app);
    let settled = crate::tests::settle_grid(&mut app);
    assert!(app.grid_scroll_after_settle.is_none());
    assert_eq!(
        crate::sort_control::tests::answers(&app, Task::batch([end, settled]))
            .iter()
            .filter(
                |message| matches!(message, Message::GridCardLocated(found, ..) if *found == name)
            )
            .count(),
        1
    );
}
