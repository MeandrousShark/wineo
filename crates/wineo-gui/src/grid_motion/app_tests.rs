//! The grid's motion as the app drives it: what moves, what snaps, the
//! widget it draws, the scroll that waits for it, and the art it loads.

use std::time::{Duration, Instant};

use iced::advanced::layout::Node;
use iced::advanced::subscription::into_recipes;
use iced::advanced::widget::{Id, Operation};
use iced::mouse::{self, Cursor};
use iced::{Point, Rectangle, Size, Task, Vector};
use wineo_core::runtime::State;

use crate::layout_probe::{bounds_at, lay_out, layout, relay_out};
use crate::library::Card;
use crate::tests::{art_file, card, settle_grid};
use crate::{App, Message, artwork};

fn cards(names: &[String]) -> Vec<Card> {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let mut card = card(name, State::Inactive, 60);
            card.art = Some(art_file(&format!("/tmp/{name}.png"), index as u64));
            card
        })
        .collect()
}

fn fixture(names: &[String]) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let mut app = App::boot(paths).0;
    let _ = app.update(Message::Loaded(Ok(cards(names))));
    assert!(!app.grid_motion.active());
    (root, app)
}

fn named(prefix: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("{prefix}-{index:02}"))
        .collect()
}

fn shown(app: &App) -> Vec<String> {
    app.filtered()
        .iter()
        .map(|card| card.name.clone())
        .collect()
}

fn frames(app: &App) -> usize {
    into_recipes(app.grid_motion.subscription()).len()
}

fn grid_size(app: &App) -> Size {
    Size::new(app.viewport_width, f32::INFINITY)
}

fn grid_node(app: &App) -> Node {
    layout(&mut app.grid(), grid_size(app))
}

fn moved(at: Point) -> iced::Event {
    iced::Event::Mouse(mouse::Event::CursorMoved { position: at })
}

fn pressed() -> iced::Event {
    iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
}

fn search(app: &mut App, text: &str) -> Task<Message> {
    app.update(Message::SearchChanged(text.into()))
}

fn flip(app: &mut App) -> Task<Message> {
    app.update(Message::Sort(crate::sort_control::Event::FlipDirection))
}

/// The ids of the card containers an operation can reach.
fn card_ids(app: &App) -> Vec<Id> {
    type Found = std::sync::Arc<std::sync::Mutex<Vec<Id>>>;
    struct Ids(Found);
    impl Operation for Ids {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn container(&mut self, id: Option<&Id>, _bounds: Rectangle) {
            self.0.lock().unwrap().extend(id.cloned());
        }
    }
    let found = Found::default();
    let mut laid = lay_out(app.grid(), grid_size(app));
    laid.run(Box::new(Ids(found.clone())));
    found.lock().unwrap().clone()
}

fn located(app: &App, task: Task<Message>, name: &str) -> usize {
    crate::sort_control::tests::answers(app, task)
        .iter()
        .filter(|message| matches!(message, Message::GridCardLocated(found, ..) if found == name))
        .count()
}

#[test]
fn the_widget_puts_cards_where_the_app_measures_them() {
    let (_root, mut app) = fixture(&named("game", 12));
    for open in [false, true] {
        if open {
            let _ = app.update(Message::Select("game-00".into()));
            let _ = settle_grid(&mut app);
            assert!(app.panel_open);
        }
        let places = app.grid_places();
        let node = grid_node(&app);
        for (index, slot) in places.slots.iter().enumerate() {
            assert_eq!(bounds_at(&node, &[index]), *slot, "{index}, open {open}");
        }
        assert_eq!(node.bounds().height, places.height);
    }
}

#[test]
fn hover_follows_a_card_to_its_new_place() {
    let (_root, mut app) = fixture(&named("game", 10));
    let first = shown(&app)[0].clone();
    let mut laid = lay_out(app.grid(), grid_size(&app));
    let at = bounds_at(&laid.node, &[0]).center();
    let messages = laid.event_at(&moved(at), Cursor::Available(at));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Hover(Some(name)) if *name == first)),
    );
    let tree = laid.into_tree();
    for message in messages {
        let _ = app.update(message);
    }
    let _ = flip(&mut app);
    let _ = settle_grid(&mut app);
    let now_first = shown(&app)[0].clone();
    assert_ne!(now_first, first);
    // The card now under the pointer was never hovered, and the one that
    // was has moved out from under it.
    let mut laid = relay_out(app.grid(), tree, grid_size(&app));
    let messages = laid.event_at(&moved(at), Cursor::Available(at));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Hover(Some(name)) if *name == now_first)),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Hover(None))),
        "{messages:?}"
    );
}

#[test]
fn leaving_cards_stay_where_they_were_take_no_input_and_hold_the_height() {
    let mut names = named("drop", 9);
    names.push("keep-00".into());
    let (_root, mut app) = fixture(&names);
    assert_eq!(app.columns(), 5);
    let before = app.grid_places();
    let _ = app.update(Message::Hover(Some("drop-01".into())));
    let _ = search(&mut app, "keep");
    assert_eq!(shown(&app), ["keep-00"]);
    assert!(app.grid_motion.active());
    // A leaving card never sends its exit, so its hover goes now.
    assert_eq!(app.hovered, None);

    let node = grid_node(&app);
    // "keep-00" springs from where it was, and the leaving cards follow in
    // the order they were shown.
    assert_eq!(bounds_at(&node, &[0]), before.slots[9]);
    for index in 0..9 {
        assert_eq!(bounds_at(&node, &[1 + index]), before.slots[index]);
    }
    // The scroll height holds until the fades end.
    assert_eq!(node.bounds().height, before.height);
    assert!(app.grid_places().height < before.height);

    // A leaving card answers neither the pointer nor a scroll to it.
    let over = before.slots[1].center();
    let mut laid = lay_out(app.grid(), grid_size(&app));
    assert!(
        laid.event_at(&moved(over), Cursor::Available(over))
            .is_empty()
    );
    assert!(
        laid.event_at(&pressed(), Cursor::Available(over))
            .is_empty()
    );
    drop(laid);
    let ids = card_ids(&app);
    assert!(ids.contains(&Id::from("library-card:keep-00".to_owned())));
    assert!(!ids.contains(&Id::from("library-card:drop-01".to_owned())));

    let _ = settle_grid(&mut app);
    assert!(app.grid_motion.leaving().is_empty());
    let node = grid_node(&app);
    assert_eq!(node.children().len(), 1);
    assert_eq!(node.bounds().height, app.grid_places().height);
}

#[test]
fn a_resize_snaps_a_transition() {
    let (_root, mut app) = fixture(&named("game", 12));
    let _ = app.update(Message::Select("game-07".into()));
    assert!(app.grid_motion.active());
    let _ = app.update(Message::Window(iced::window::Event::Resized(Size::new(
        1400.0, 900.0,
    ))));
    assert!(!app.grid_motion.active());
    assert_eq!(frames(&app), 0);
    for name in shown(&app) {
        assert_eq!(app.grid_motion.offset(&name), Vector::ZERO);
    }
}

#[test]
fn a_rescan_mid_transition_settles_it_first() {
    let mut names = named("drop", 9);
    names.push("keep-00".into());
    let (_root, mut app) = fixture(&names);
    let _ = search(&mut app, "keep");
    assert!(app.grid_motion.active());
    assert!(!app.grid_motion.leaving().is_empty());
    names.push("keep-01".into());
    app.merge(cards(&names));
    assert_eq!(shown(&app), ["keep-00", "keep-01"]);
    assert!(!app.grid_motion.active());
    assert!(app.grid_motion.leaving().is_empty());
    assert_eq!(app.grid_motion.offset("keep-00"), Vector::ZERO);
    assert_eq!(app.grid_motion.opacity("keep-01"), 1.0);
}

/// A search leaves 20 of 120 cards fading out, with a selection scroll
/// waiting for them.
fn waiting_on_a_fade() -> (tempfile::TempDir, App) {
    let names: Vec<String> = (0..120).map(|index| format!("game-{index:03}")).collect();
    let (root, mut app) = fixture(&names);
    let _ = search(&mut app, "game-1");
    assert_eq!(shown(&app).len(), 20);
    assert!(app.grid_motion.active());
    app.selected = Some("game-119".into());
    let _ = app.scroll_selection();
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some("game-119"));
    (root, app)
}

#[test]
fn a_waiting_scroll_resolves_once_when_the_last_motion_ends() {
    type End = fn(&mut App) -> Task<Message>;
    let ends: [(&str, End); 3] = [
        ("settle", settle_grid),
        ("resize", |app| {
            app.update(Message::Window(iced::window::Event::Resized(Size::new(
                1300.0, 800.0,
            ))))
        }),
        ("rescan", |app| {
            let mut names: Vec<String> = (0..120).map(|index| format!("game-{index:03}")).collect();
            names.push("game-120".into());
            app.merge_checked(cards(&names))
        }),
    ];
    for (end, run) in ends {
        let (_root, mut app) = waiting_on_a_fade();
        let task = run(&mut app);
        assert!(!app.grid_motion.active(), "{end}");
        assert!(app.grid_scroll_after_settle.is_none(), "{end}");
        assert_eq!(located(&app, task, "game-119"), 1, "{end}");
        // Nothing is left to fire later.
        let _ = app.update(Message::Select("game-100".into()));
        let task = settle_grid(&mut app);
        assert_eq!(located(&app, task, "game-119"), 0, "{end}");
    }
}

#[test]
fn a_waiting_scroll_drops_when_the_grid_is_covered_left_or_reordered() {
    type End = fn(&mut App) -> Task<Message>;
    let ends: [(&str, End); 3] = [
        ("cover", |app| app.update(Message::Add)),
        ("leave", |app| {
            app.update(Message::Section(crate::sidebar::Section::Trash))
        }),
        ("search", |app| search(app, "game-11")),
    ];
    for (end, run) in ends {
        let (_root, mut app) = waiting_on_a_fade();
        let task = run(&mut app);
        assert!(app.grid_scroll_after_settle.is_none(), "{end}");
        assert_eq!(located(&app, task, "game-119"), 0, "{end}");
        let task = settle_grid(&mut app);
        assert_eq!(located(&app, task, "game-119"), 0, "{end}");
    }
}

#[test]
fn art_loads_what_is_in_view_then_where_cards_head_then_what_fades() {
    // Sorted by name: five that stay in the first row, fifty that leave,
    // then ten that rise from far below into rows one and two.
    let mut names = named("a-keep", 5);
    names.extend(named("drop", 50));
    names.extend(named("keep", 10));
    let (_root, mut app) = fixture(&names);
    assert_eq!(
        shown(&app)[..6],
        [
            "a-keep-00",
            "a-keep-01",
            "a-keep-02",
            "a-keep-03",
            "a-keep-04",
            "drop-00"
        ]
    );
    let _ = search(&mut app, "keep");
    assert_eq!(shown(&app).len(), 15);
    let order: Vec<String> = app
        .grid_art()
        .into_iter()
        .map(|((art, kind), _)| {
            assert_eq!(kind, artwork::Kind::Grid);
            art.path.file_stem().unwrap().to_string_lossy().into_owned()
        })
        .collect();
    let first: Vec<&str> = order.iter().map(String::as_str).take(15).collect();
    assert_eq!(
        first[..5],
        [
            "a-keep-00",
            "a-keep-01",
            "a-keep-02",
            "a-keep-03",
            "a-keep-04"
        ]
    );
    assert!(
        first[5..].iter().all(|name| name.starts_with("keep-")),
        "{order:?}"
    );
    let rest = &order[15..];
    assert!(!rest.is_empty());
    assert!(
        rest.iter().all(|name| name.starts_with("drop-")),
        "{order:?}"
    );
    // Only the leaving cards near the view, not all fifty.
    assert!(rest.len() < 50, "{}", rest.len());
    assert!(app.visible_art.len() == order.len());

    // Once settled, only what's in view stays wanted.
    let _ = settle_grid(&mut app);
    assert!(
        app.grid_art()
            .iter()
            .all(|((art, _), _)| !art.path.to_string_lossy().contains("drop-"))
    );
}

#[test]
fn every_kind_of_transition_stops_asking_for_frames() {
    let (_root, mut app) = fixture(&named("game", 12));
    let _ = app.update(Message::Sort(
        crate::sort_control::Event::ToggleFavoritesFirst,
    ));
    let _ = settle_grid(&mut app);
    type Change = fn(&mut App) -> Task<Message>;
    let changes: [(&str, Change); 6] = [
        ("dock", |app| app.update(Message::Select("game-03".into()))),
        ("undock", |app| app.update(Message::ClosePanel)),
        ("resort", flip),
        ("filter", |app| search(app, "game-0")),
        ("favorite", |app| {
            app.update(Message::Mark(crate::library_actions::Event::Toggle(
                "game-08".into(),
                crate::library_actions::Flag::Favorite,
            )))
        }),
        ("unfilter", |app| search(app, "")),
    ];
    for (change, run) in changes {
        let _ = run(&mut app);
        assert!(app.grid_motion.active(), "{change}");
        assert_eq!(frames(&app), 1, "{change}");
        let _ = settle_grid(&mut app);
        assert!(!app.grid_motion.active(), "{change}");
        assert_eq!(frames(&app), 0, "{change}");
    }
}

#[test]
fn ticks_follow_a_dock_until_it_settles() {
    let (_root, mut app) = fixture(&named("game", 12));
    let _ = app.update(Message::Select("game-07".into()));
    let generation = app.grid_motion.generation();
    let start = Instant::now();
    let _ = app.update(Message::GridTick(
        generation,
        start + Duration::from_millis(120),
    ));
    assert!(app.grid_motion.active());
    assert_ne!(app.grid_motion.offset("game-07"), Vector::ZERO);
    let _ = app.update(Message::GridTick(
        generation,
        start + Duration::from_secs(1),
    ));
    assert!(!app.grid_motion.active());
    assert_eq!(app.grid_motion.offset("game-07"), Vector::ZERO);
}

#[test]
fn the_poster_only_toggle_moves_cards_only_when_their_rows_change() {
    let (_root, mut app) = fixture(&named("game", 12));
    let generation = app.grid_motion.generation();
    let _ = app.update(Message::Zoom(crate::zoom::Event::TogglePosterOnly));
    assert!(app.poster_only);
    // It gives the cards their slots again, which don't change yet.
    assert_ne!(app.grid_motion.generation(), generation);
    assert!(!app.grid_motion.active());
    assert!(app.grid_motion.anchor.is_none());
    assert_eq!(frames(&app), 0);
}

#[test]
fn a_resort_scrolls_an_off_screen_selection_once_to_where_it_lands() {
    let (_root, mut app) = fixture(&named("game", 60));
    app.selected = Some("game-00".into());
    assert_eq!(app.scroll_offset, 0.0);
    let task = flip(&mut app);
    assert_eq!(shown(&app).last().map(String::as_str), Some("game-00"));
    assert!(app.grid_motion.active());
    // Straight to the bottom of its new slot, the way an arrow key would
    // bring it just into view, while it's still drawn at the top.
    let places = app.grid_places();
    let slot = places.slots[places.index("game-00").unwrap()];
    let landing = slot.y + slot.height - app.viewport_height;
    assert!(landing > 0.0);
    assert_eq!(app.scroll_offset, landing);
    assert!(app.grid_motion.offset("game-00") != Vector::ZERO);
    assert!(app.grid_scroll_after_settle.is_none());
    assert_eq!(located(&app, task, "game-00"), 0);
    // Nothing scrolls again when the cards settle.
    let task = settle_grid(&mut app);
    assert_eq!(located(&app, task, "game-00"), 0);
    assert_eq!(app.scroll_offset, landing);
}

#[test]
fn leaving_cards_keep_the_size_they_left_with_through_a_zoom() {
    let mut names = named("drop", 9);
    names.push("keep-00".into());
    let (_root, mut app) = fixture(&names);
    let _ = search(&mut app, "keep");
    let left = app.grid_motion.leaving()[0].clone();
    assert_eq!(left.bounds.width, 160.0);
    let _ = app.update(Message::Zoom(crate::zoom::Event::Drag(150.0)));
    assert_eq!(app.zoom, 150);
    assert!(!app.grid_motion.leaving().is_empty());
    let node = grid_node(&app);
    assert_eq!(bounds_at(&node, &[0]).width, 240.0);
    assert_eq!(bounds_at(&node, &[1]), left.bounds);
    // Its poster is the one it left with, not one squeezed from 150%.
    assert_eq!(
        bounds_at(&node, &[1, 0]).size(),
        Size::new(160.0, super::GridLayout::default().poster_height())
    );
}

#[test]
fn places_are_measured_again_only_when_their_inputs_change() {
    let (_root, mut app) = fixture(&named("game", 12));
    let first = app.grid_places();
    // Reading again, as every held frame does, reuses the measure.
    assert!(std::rc::Rc::ptr_eq(&first, &app.grid_places()));
    type Change = fn(&mut App);
    let changes: [(&str, Change); 4] = [
        ("order", |app| {
            let _ = flip(app);
        }),
        ("columns", |app| {
            let _ = app.update(Message::Select("game-03".into()));
        }),
        ("zoom", |app| {
            let _ = app.update(Message::Zoom(crate::zoom::Event::Drag(130.0)));
        }),
        ("cards", |app| app.merge(cards(&named("game", 13)))),
    ];
    for (change, run) in changes {
        let before = app.grid_places();
        run(&mut app);
        let after = app.grid_places();
        assert!(!std::rc::Rc::ptr_eq(&before, &after), "{change}");
        assert!(std::rc::Rc::ptr_eq(&after, &app.grid_places()), "{change}");
    }
}
