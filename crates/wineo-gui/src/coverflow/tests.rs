use iced::keyboard::{Key, Modifiers, key::Named};
use iced::{Point, Rectangle, Size};
use wineo_core::runtime::State;

use super::*;
use crate::app_selection::Cursor;
use crate::grid_motion::{Header, ViewLayout};
use crate::tests::card;

const WIDE: Size = Size::new(1600.0, 900.0);

fn far(geometry: &Geometry, position: f32) -> Vec<f32> {
    geometry
        .covers
        .iter()
        .map(|cover| (cover.stop as f32 - position).abs())
        .collect()
}

fn cover(geometry: &Geometry, stop: usize) -> Cover {
    *geometry
        .covers
        .iter()
        .find(|cover| cover.stop == stop)
        .unwrap_or_else(|| panic!("stop {stop} drawn"))
}

#[test]
fn the_middle_cover_is_55_percent_of_the_height_and_centered_over_its_caption() {
    let geometry = geometry(10.0, 30, WIDE);
    let middle = cover(&geometry, 10);
    assert_eq!(middle.bounds.size(), Size::new(330.0, 495.0));
    assert_eq!(middle.bounds.center_x(), WIDE.width / 2.0);
    assert_eq!((middle.scale, middle.opacity), (1.0, 1.0));
    // The cover and its caption sit together in the middle of the height.
    let block = middle.bounds.height + CAPTION;
    assert_eq!(middle.bounds.y, ((WIDE.height - block) / 2.0).round());
    assert_eq!(middle.bounds.x.fract(), 0.0);
    assert_eq!(
        geometry.caption.y,
        middle.bounds.y + middle.bounds.height + CAPTION_GAP
    );
    assert_eq!(geometry.caption.center_x(), middle.bounds.center_x());
}

#[test]
fn neighbors_step_smaller_and_darker_to_half_size_overlapping_the_ones_nearer() {
    let geometry = geometry(10.0, 30, WIDE);
    // Four each side, drawn farthest first, with the middle last.
    assert_eq!(geometry.covers.len(), 9);
    let distances = far(&geometry, 10.0);
    assert!(
        distances.windows(2).all(|pair| pair[0] >= pair[1]),
        "{distances:?}"
    );
    assert_eq!(geometry.covers.last().map(|cover| cover.stop), Some(10));
    let middle = cover(&geometry, 10);
    for step in 1..=NEIGHBORS {
        let (left, right) = (cover(&geometry, 10 - step), cover(&geometry, 10 + step));
        let (inner_left, inner_right) = (
            cover(&geometry, 10 - step + 1),
            cover(&geometry, 10 + step - 1),
        );
        assert!(right.scale < inner_right.scale, "step {step}");
        assert!(right.opacity < inner_right.opacity, "step {step}");
        assert!(right.scale >= MIN_SCALE, "step {step}");
        assert_eq!((left.scale, left.opacity), (right.scale, right.opacity));
        // Mirrored about the middle, to the pixel.
        let mirror = (middle.bounds.center_x() - left.bounds.center_x())
            - (right.bounds.center_x() - middle.bounds.center_x());
        assert!(mirror.abs() <= 1.0, "step {step}: {mirror}");
        // Each tucks under the one nearer the middle.
        assert!(right.bounds.x < inner_right.bounds.x + inner_right.bounds.width);
        assert!(left.bounds.x + left.bounds.width > inner_left.bounds.x);
        assert!((right.bounds.center_y() - middle.bounds.center_y()).abs() <= 0.5);
    }
    assert_eq!(cover(&geometry, 14).scale, MIN_SCALE);
}

#[test]
fn between_two_stops_both_are_the_same_size_and_the_far_ones_fade_out() {
    let geometry = geometry(10.5, 30, WIDE);
    let (left, right) = (cover(&geometry, 10), cover(&geometry, 11));
    assert_eq!(left.bounds.size(), right.bounds.size());
    assert!(left.scale < 1.0);
    assert!(
        (WIDE.width / 2.0 - left.bounds.center_x() - (right.bounds.center_x() - WIDE.width / 2.0))
            .abs()
            <= 1.0
    );
    // 4.5 stops out is halfway through its fade; 5.5 isn't drawn.
    let edge = cover(&geometry, 6);
    assert!((edge.opacity - (1.0 - STEP_DIM * 4.0) * 0.5).abs() < 1e-4);
    assert!(geometry.covers.iter().all(|cover| cover.stop != 5));
    assert!(geometry.covers.iter().all(|cover| cover.stop != 16));
    // The draw order still runs from the farthest in.
    let distances = far(&geometry, 10.5);
    assert!(
        distances.windows(2).all(|pair| pair[0] >= pair[1]),
        "{distances:?}"
    );
}

#[test]
fn a_cover_moves_smoothly_as_the_position_moves() {
    let mut last = cover(&geometry(10.0, 30, WIDE), 11).bounds;
    for tenth in 1..=10 {
        let position = 10.0 + tenth as f32 / 10.0;
        let now = cover(&geometry(position, 30, WIDE), 11).bounds;
        assert!(now.center_x() <= last.center_x(), "{position}");
        assert!(last.center_x() - now.center_x() < 100.0, "{position}");
        assert!(now.width >= last.width, "{position}");
        last = now;
    }
    // Arriving in the middle, it's the middle cover.
    assert_eq!(last, cover(&geometry(11.0, 30, WIDE), 11).bounds);
}

#[test]
fn at_the_ends_one_side_is_empty() {
    let first = geometry(0.0, 30, WIDE);
    assert_eq!(first.covers.len(), NEIGHBORS + 1);
    assert!(first.covers.iter().all(|cover| cover.stop <= NEIGHBORS));
    assert_eq!(cover(&first, 0).bounds.center_x(), WIDE.width / 2.0);
    let last = geometry(29.0, 30, WIDE);
    assert_eq!(last.covers.len(), NEIGHBORS + 1);
    assert!(last.covers.iter().all(|cover| cover.stop >= 29 - NEIGHBORS));
}

#[test]
fn one_game_is_the_middle_alone_and_none_draws_nothing() {
    let one = geometry(0.0, 1, WIDE);
    assert_eq!(one.covers.len(), 1);
    assert_eq!(cover(&one, 0).scale, 1.0);
    assert_eq!(cover(&one, 0).bounds.center_x(), WIDE.width / 2.0);
    assert!(geometry(0.0, 0, WIDE).covers.is_empty());
}

#[test]
fn the_strip_scales_with_the_window_and_narrow_windows_clamp_by_width() {
    // A bigger window, a bigger cover.
    assert!(middle_size(Size::new(2560.0, 1400.0)).height > middle_size(WIDE).height);
    // A short window keeps the grid's usual poster.
    let floor = GridLayout::default().poster_height();
    assert_eq!(middle_size(Size::new(1600.0, 400.0)).height, floor);
    // Narrow, the width clamp holds it to 40% of the strip's width...
    let narrow = Size::new(500.0, 900.0);
    assert_eq!(middle_size(narrow).width, 200.0);
    // ...and wins over the poster floor in a very narrow one.
    let tiny = Size::new(300.0, 900.0);
    assert_eq!(middle_size(tiny), Size::new(120.0, 180.0));
    assert!(middle_size(tiny).height < floor);
    // Never taller than leaves room for the caption.
    let short = Size::new(1600.0, 280.0);
    assert!(middle_size(short).height + CAPTION <= short.height);
}

#[test]
fn a_page_is_the_covers_on_screen() {
    assert_eq!(on_screen(WIDE), 9);
    // A narrow strip shows fewer, and always the middle one.
    let narrow = on_screen(Size::new(500.0, 900.0));
    assert!((1..9).contains(&narrow), "{narrow}");
    assert_eq!(on_screen(Size::new(10.0, 10.0)) % 2, 1);
}

#[test]
fn the_large_cover_decodes_in_steps_and_only_past_the_grid_poster() {
    assert_eq!(cover_height(400.0), None);
    assert_eq!(cover_height(480.0), None);
    assert_eq!(cover_height(481.0), Some(720));
    assert_eq!(cover_height(720.0), Some(720));
    assert_eq!(cover_height(1000.0), Some(1200));
    assert_eq!(cover_height(4000.0), Some(1440));
}

#[test]
fn the_area_leaves_the_docked_panel_its_room() {
    let bounds = Rectangle::new(Point::new(10.0, 20.0), Size::new(1500.0, 700.0));
    assert_eq!(area(bounds, false), bounds);
    assert_eq!(
        area(bounds, true),
        Rectangle::new(
            Point::new(10.0, 20.0),
            Size::new(1500.0 - DOCK_WIDTH, 700.0)
        )
    );
}

fn tag(name: &str) -> SectionKey {
    SectionKey::Tag(name.to_owned())
}

fn header(name: &str, count: usize, collapsed: bool, at: usize) -> Header {
    Header {
        key: tag(name),
        count,
        collapsed,
        at,
    }
}

/// a: open with two games, b: collapsed with three, c: open with two.
fn strip_places() -> Places {
    let ids = ["a-0", "a-1", "c-0", "c-1"]
        .into_iter()
        .map(|name| SlotId::new(tag(&name[..1]), name))
        .collect();
    let headers = vec![
        header("a", 2, false, 0),
        header("b", 3, true, 2),
        header("c", 2, false, 2),
    ];
    Places::strip(
        ids,
        headers,
        &[Some(0), Some(7), Some(4)],
        GridLayout::default(),
    )
}

#[test]
fn the_strip_is_one_row_with_a_stop_for_each_card_and_collapsed_section() {
    let places = strip_places();
    assert_eq!(
        places.strip,
        vec![
            Stop::Card(0),
            Stop::Card(1),
            Stop::Collapsed {
                header: 1,
                first: Some(7)
            },
            Stop::Card(2),
            Stop::Card(3),
        ]
    );
    let xs: Vec<f32> = places.slots.iter().map(|slot| slot.x).collect();
    assert_eq!(xs, [0.0, 1.0, 3.0, 4.0]);
    assert!(places.slots.iter().all(|slot| slot.y == 0.0));
    let starts: Vec<f32> = places.headers.iter().map(|place| place.slot.x).collect();
    assert_eq!(starts, [0.0, 2.0, 3.0]);
    assert_eq!(places.mode, ViewLayout::Coverflow);
}

#[test]
fn left_and_right_move_one_stop_and_stop_at_the_ends() {
    let places = strip_places();
    assert_eq!(walk(&places, None, Direction::Right), Some(0));
    assert_eq!(walk(&places, Some(1), Direction::Right), Some(2));
    assert_eq!(walk(&places, Some(2), Direction::Right), Some(3));
    assert_eq!(walk(&places, Some(4), Direction::Right), Some(4));
    assert_eq!(walk(&places, Some(0), Direction::Left), Some(0));
    assert_eq!(walk(&places, Some(3), Direction::Left), Some(2));
}

#[test]
fn up_and_down_jump_to_the_first_stop_of_the_next_or_previous_section() {
    let places = strip_places();
    assert_eq!(walk(&places, Some(1), Direction::Down), Some(2));
    assert_eq!(walk(&places, Some(2), Direction::Down), Some(3));
    assert_eq!(walk(&places, Some(4), Direction::Down), Some(4));
    assert_eq!(walk(&places, Some(4), Direction::Up), Some(2));
    assert_eq!(walk(&places, Some(2), Direction::Up), Some(0));
    assert_eq!(walk(&places, Some(1), Direction::Up), Some(1));
    // Ungrouped, they do nothing.
    let flat = Places::strip(
        vec![SlotId::from("x"), SlotId::from("y")],
        Vec::new(),
        &[],
        GridLayout::default(),
    );
    assert_eq!(walk(&flat, Some(1), Direction::Up), Some(1));
    assert_eq!(walk(&flat, Some(0), Direction::Down), Some(0));
    assert_eq!(walk(&Places::default(), None, Direction::Down), None);
}

#[test]
fn a_page_moves_by_the_covers_given_and_stops_at_the_ends() {
    let places = strip_places();
    assert_eq!(page(&places, Some(0), Direction::Down, 3), Some(3));
    assert_eq!(page(&places, Some(3), Direction::Down, 3), Some(4));
    assert_eq!(page(&places, Some(3), Direction::Up, 2), Some(1));
    assert_eq!(page(&places, Some(1), Direction::Up, 9), Some(0));
    assert_eq!(page(&places, None, Direction::Down, 9), Some(0));
}

fn boot() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    (root, App::boot(paths).0)
}

/// Three tag sections of six games each, a, b, and c, in coverflow.
fn sectioned() -> (tempfile::TempDir, App) {
    let (root, mut app) = boot();
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
    let _ = app.update(Message::Loaded(Ok(cards)));
    in_coverflow(&mut app);
    (root, app)
}

fn flat(count: usize) -> (tempfile::TempDir, App) {
    let (root, mut app) = boot();
    let cards = (0..count)
        .map(|index| card(&format!("game-{index:02}"), State::Inactive, 0))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    in_coverflow(&mut app);
    (root, app)
}

/// Shows coverflow without saving it, with the strip measured at `WIDE`.
fn in_coverflow(app: &mut App) {
    let _ = app.switch_layout(ViewLayout::Coverflow);
    let _ = app.update(Message::Strip(Event::Measured(WIDE)));
}

fn press_with(app: &mut App, named: Named, modifiers: Modifiers) {
    let task = app.update(Message::KeyWithFocus(
        Key::Named(named),
        modifiers,
        iced::event::Status::Ignored,
        false,
    ));
    for message in crate::sort_control::tests::answers(app, task) {
        let _ = app.update(message);
    }
}

fn press(app: &mut App, named: Named) {
    press_with(app, named, Modifiers::empty());
}

fn selected(app: &App) -> &str {
    app.selected.as_deref().expect("a selection")
}

/// The game or collapsed section in the middle.
fn middle(app: &App) -> String {
    let places = app.grid_places();
    match places.strip[app.strip.target()] {
        Stop::Card(index) => places.ids[index].name().to_owned(),
        Stop::Collapsed { header, .. } => {
            format!("[{}]", places.headers[header].header.key.title())
        }
    }
}

fn select(app: &mut App, name: &str) {
    let _ = app.update(Message::Select(name.into()));
}

fn collapse(app: &mut App, section: &str) {
    let _ = app.update(Message::ToggleSection(tag(section)));
}

#[test]
fn with_nothing_selected_the_first_game_is_the_middle_and_the_selection() {
    let (_root, app) = flat(5);
    assert_eq!(selected(&app), "game-00");
    assert_eq!(middle(&app), "game-00");
    assert_eq!(app.strip.position(), 0.0);
    assert!(!app.panel_open);
}

#[test]
fn the_arrows_walk_the_strip_and_the_middle_follows_the_selection() {
    let (_root, mut app) = sectioned();
    select(&mut app, "b-2");
    assert_eq!(middle(&app), "b-2");
    press(&mut app, Named::ArrowRight);
    assert_eq!((selected(&app), middle(&app).as_str()), ("b-3", "b-3"));
    press(&mut app, Named::ArrowDown);
    assert_eq!(selected(&app), "c-0");
    press(&mut app, Named::ArrowDown);
    assert_eq!(selected(&app), "c-0");
    press(&mut app, Named::End);
    assert_eq!(selected(&app), "c-5");
    press(&mut app, Named::ArrowRight);
    assert_eq!(selected(&app), "c-5", "no wrap past the end");
    press(&mut app, Named::ArrowUp);
    assert_eq!(selected(&app), "b-0");
    press(&mut app, Named::Home);
    assert_eq!(selected(&app), "a-0");
    press(&mut app, Named::ArrowLeft);
    assert_eq!(selected(&app), "a-0", "no wrap past the start");
    press(&mut app, Named::ArrowUp);
    assert_eq!(selected(&app), "a-0");
    assert_eq!(app.strip.target(), 0);
}

#[test]
fn ungrouped_up_and_down_do_nothing() {
    let (_root, mut app) = flat(8);
    select(&mut app, "game-03");
    press(&mut app, Named::ArrowDown);
    assert_eq!(selected(&app), "game-03");
    press(&mut app, Named::ArrowUp);
    assert_eq!(selected(&app), "game-03");
}

#[test]
fn page_down_and_up_move_by_the_covers_on_screen() {
    let (_root, mut app) = flat(40);
    let covers = app.covers_per_screen();
    assert_eq!(covers, on_screen(app.strip_size()));
    assert!(covers > 1);
    press(&mut app, Named::PageDown);
    assert_eq!(app.strip.target(), covers);
    press(&mut app, Named::PageDown);
    assert_eq!(app.strip.target(), 2 * covers);
    press(&mut app, Named::PageUp);
    assert_eq!(app.strip.target(), covers);
    for _ in 0..10 {
        press(&mut app, Named::PageDown);
    }
    assert_eq!(selected(&app), "game-39");
    // A narrower strip shows fewer covers, so a page is shorter.
    let _ = app.update(Message::Strip(Event::Measured(Size::new(700.0, 900.0))));
    let narrow = app.covers_per_screen();
    assert!(narrow < covers, "{narrow} < {covers}");
    press(&mut app, Named::PageUp);
    assert_eq!(app.strip.target(), 39 - narrow);
}

#[test]
fn a_selection_filtered_out_hands_the_middle_to_the_first_game() {
    let (_root, mut app) = flat(12);
    select(&mut app, "game-07");
    let _ = app.update(Message::SearchChanged("game-1".into()));
    assert_eq!(selected(&app), "game-10");
    assert_eq!(app.strip.target(), 0);
    let _ = app.update(Message::SearchChanged(String::new()));
    // The new selection holds and the strip recenters on it.
    assert_eq!(selected(&app), "game-10");
    assert_eq!(app.strip.target(), 10);
}

#[test]
fn search_random_pick_and_rescan_recenter_the_strip() {
    let (_root, mut app) = flat(12);
    select(&mut app, "game-05");
    assert_eq!(app.strip.target(), 5);
    // Down from search lands on the first stop.
    let task = app.update(Message::KeyWithFocus(
        Key::Named(Named::ArrowDown),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        true,
    ));
    for message in crate::sort_control::tests::answers(&app, task) {
        let _ = app.update(message);
    }
    assert_eq!((selected(&app), app.strip.target()), ("game-00", 0));
    // A random pick selects some game; the middle is that game.
    let task = app.update(Message::Random(crate::random_pick::Event::Pick));
    for message in crate::sort_control::tests::answers(&app, task) {
        let _ = app.update(message);
    }
    let picked = selected(&app).to_owned();
    assert_eq!(middle(&app), picked);
    // A rescan that adds games before the selection moves its stop, and
    // the strip goes with it.
    let before = app.strip.target();
    let mut cards = app.cards.clone();
    cards.push(card("aaa-new", State::Inactive, 0));
    cards.push(card("aab-new", State::Inactive, 0));
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert_eq!(selected(&app), picked);
    assert_eq!(middle(&app), picked);
    assert_eq!(app.strip.target(), before + 2);
}

#[test]
fn every_section_collapsed_makes_a_strip_of_section_cards() {
    let (_root, mut app) = sectioned();
    for section in ["a", "b", "c"] {
        collapse(&mut app, section);
    }
    let places = app.grid_places();
    assert!(places.ids.is_empty());
    assert_eq!(places.strip.len(), 3);
    // The middle is the first card, with its first game selected and the
    // panel hidden by it.
    assert_eq!(middle(&app), "[a]");
    assert_eq!(selected(&app), "a-0");
    assert_eq!(app.cursor, Some(Cursor::Header(tag("a"))));
    press(&mut app, Named::ArrowRight);
    assert_eq!(middle(&app), "[b]");
    assert_eq!(selected(&app), "b-0");
    press(&mut app, Named::End);
    assert_eq!(middle(&app), "[c]");
    press(&mut app, Named::Home);
    assert_eq!(middle(&app), "[a]");
    press(&mut app, Named::ArrowDown);
    assert_eq!(middle(&app), "[b]");
    // A card keeps its place even when the selection goes.
    app.selected = None;
    let _ = app.update(Message::Strip(Event::Measured(WIDE)));
    assert_eq!(middle(&app), "[b]");
    assert_eq!(selected(&app), "b-0");
}

#[test]
fn a_game_selected_inside_a_collapsed_section_brings_its_card_to_the_middle() {
    let (_root, mut app) = sectioned();
    collapse(&mut app, "b");
    select(&mut app, "c-2");
    let _ = app.update(Message::OpenCard("b-3".into()));
    assert_eq!(middle(&app), "[b]");
    assert_eq!(selected(&app), "b-0");
    assert!(app.panel_hidden_by_card());
}

#[test]
fn enter_on_a_section_card_opens_it_on_its_first_game() {
    let (_root, mut app) = sectioned();
    collapse(&mut app, "b");
    select(&mut app, "a-5");
    press(&mut app, Named::ArrowRight);
    assert_eq!(middle(&app), "[b]");
    press(&mut app, Named::Enter);
    assert!(!app.collapsed.contains(&tag("b")));
    assert_eq!(selected(&app), "b-0");
    assert_eq!(middle(&app), "b-0");
    assert_eq!(app.cursor, Some(Cursor::Card(SlotId::new(tag("b"), "b-0"))));
}

#[test]
fn f_and_space_do_nothing_on_a_section_card() {
    let (_root, mut app) = sectioned();
    // b-0 shows in c too, so it's in view while b's card is in the middle.
    let mut cards = app.cards.clone();
    for card in &mut cards {
        if card.name == "b-0" {
            card.tags.push("c".into());
        }
    }
    let _ = app.update(Message::Loaded(Ok(cards)));
    collapse(&mut app, "b");
    select(&mut app, "a-5");
    press(&mut app, Named::ArrowRight);
    assert_eq!(middle(&app), "[b]");
    let task = app.update(Message::KeyWithFocus(
        Key::Character("f".into()),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
    for message in crate::sort_control::tests::answers(&app, task) {
        let _ = app.update(message);
    }
    assert!(app.cards.iter().all(|card| !card.favorite));
    press(&mut app, Named::Space);
    assert!(app.marked.is_empty());
    // On a game they work as anywhere.
    press(&mut app, Named::ArrowLeft);
    press(&mut app, Named::Space);
    assert!(app.marked.contains("a-5"));
}

#[test]
fn a_header_cursor_from_the_grid_becomes_its_card_or_its_first_game() {
    let (_root, mut app) = sectioned();
    let _ = app.switch_layout(ViewLayout::Grid);
    select(&mut app, "b-3");
    collapse(&mut app, "b");
    assert_eq!(app.cursor, Some(Cursor::Header(tag("b"))));
    let _ = app.switch_layout(ViewLayout::Coverflow);
    let _ = app.update(Message::Strip(Event::Measured(WIDE)));
    assert_eq!(middle(&app), "[b]");
    assert_eq!(selected(&app), "b-0");
    assert!(app.panel_hidden_by_card());
    // A header on an open section goes to its first game.
    let _ = app.switch_layout(ViewLayout::Grid);
    collapse(&mut app, "b");
    app.cursor = Some(Cursor::Header(tag("c")));
    app.selected = Some("c-4".into());
    let _ = app.switch_layout(ViewLayout::Coverflow);
    let _ = app.update(Message::Strip(Event::Measured(WIDE)));
    assert_eq!((selected(&app), middle(&app).as_str()), ("c-0", "c-0"));
    assert!(!app.panel_hidden_by_card());
}

#[test]
fn a_section_card_hides_the_panel_until_the_next_game_and_esc_leaves_it() {
    let (_root, mut app) = sectioned();
    collapse(&mut app, "b");
    select(&mut app, "a-5");
    assert!(app.panel_open);
    let room = app.strip_size();
    press(&mut app, Named::ArrowRight);
    assert_eq!(middle(&app), "[b]");
    assert!(app.panel_open, "hidden, not closed");
    assert!(app.panel_hidden_by_card());
    // The strip keeps the panel's room, so it doesn't move.
    assert_eq!(app.strip_size(), room);
    // Esc and closing the panel leave it for the next game.
    press(&mut app, Named::Escape);
    assert!(app.panel_open);
    let _ = app.update(Message::ClosePanel);
    assert!(app.panel_open);
    press(&mut app, Named::ArrowRight);
    assert_eq!(selected(&app), "c-0");
    assert!(app.panel_open && !app.panel_hidden_by_card());
    // On a game Esc closes it as usual.
    press(&mut app, Named::Escape);
    assert!(!app.panel_open);
}

#[test]
fn a_hidden_panel_draws_nothing_in_its_room() {
    let (_root, mut app) = sectioned();
    collapse(&mut app, "b");
    select(&mut app, "a-5");
    app.panel_motion.snap(true);
    // A pixel inside the panel's room, clear of the strip's covers.
    let size = app.window_size;
    let at =
        ((size.height * 0.85) as usize * size.width as usize + (size.width - 100.0) as usize) * 4;
    let pixel = |app: &App| {
        let mut view = app.view();
        crate::layout_probe::render(&mut view, size)[at..at + 4].to_vec()
    };
    let shown = pixel(&app);
    press(&mut app, Named::ArrowRight);
    assert!(app.panel_hidden_by_card());
    let hidden = pixel(&app);
    assert_ne!(shown, hidden, "the panel's surface is gone");
    press(&mut app, Named::ArrowRight);
    assert_eq!(pixel(&app), shown, "and back on the next game");
}

/// The bounds of the container or focus control with `id` in the window.
fn find(
    window: &mut crate::more_menu::tests::Window,
    app: &App,
    id: iced::widget::Id,
) -> Option<Rectangle> {
    struct Find(iced::widget::Id, Option<Rectangle>);
    impl iced::advanced::widget::Operation<Option<Rectangle>> for Find {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<Option<Rectangle>>),
        ) {
            operate(self);
        }
        fn container(&mut self, id: Option<&iced::widget::Id>, bounds: Rectangle) {
            if id == Some(&self.0) {
                self.1 = Some(bounds);
            }
        }
        fn focusable(
            &mut self,
            id: Option<&iced::widget::Id>,
            bounds: Rectangle,
            _state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            if id == Some(&self.0) {
                self.1 = Some(bounds);
            }
        }
        fn finish(&self) -> iced::advanced::widget::operation::Outcome<Option<Rectangle>> {
            iced::advanced::widget::operation::Outcome::Some(self.1)
        }
    }
    let found = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = found.clone();
    let mut laid = window.lay_out(app);
    laid.run(Box::new(iced::advanced::widget::operation::map(
        Find(id, None),
        move |bounds| *sink.lock().unwrap() = bounds,
    )));
    drop(laid);
    *found.lock().unwrap()
}

/// The strip widget's bounds in the window.
fn strip_bounds(window: &mut crate::more_menu::tests::Window, app: &App) -> Rectangle {
    find(
        window,
        app,
        iced::widget::Id::new(crate::coverflow_widget::ID),
    )
    .expect("the strip is on screen")
}

/// Where the cover at `stop` is drawn in the window now.
fn cover_at(window: &mut crate::more_menu::tests::Window, app: &App, stop: usize) -> Rectangle {
    let bounds = strip_bounds(window, app);
    let area = area(bounds, app.panel_open);
    let places = app.grid_places();
    let drawn = cover(
        &geometry(app.strip.position(), places.strip.len(), area.size()),
        stop,
    );
    drawn.bounds + iced::Vector::new(area.x, area.y)
}

#[test]
fn the_window_reports_the_strips_size_and_the_strip_fills_the_library() {
    let (_root, mut app) = flat(10);
    let mut window = crate::more_menu::tests::Window::new(&mut app);
    let bounds = strip_bounds(&mut window, &app);
    assert_eq!(app.strip.size(), bounds.size());
    assert!(bounds.width > 0.0 && bounds.height > 0.0);
}

#[test]
fn a_click_on_a_side_cover_centers_it_and_on_the_middle_selects_it() {
    let (_root, mut app) = flat(10);
    select(&mut app, "game-04");
    let _ = app.update(Message::ClosePanel);
    // Shut at once, so its closing slide doesn't cover the side covers.
    app.panel_motion.snap(false);
    let mut window = crate::more_menu::tests::Window::new(&mut app);
    let side = cover_at(&mut window, &app, 6);
    // Right of the nearer cover that overlaps it.
    window.click(
        &mut app,
        Point::new(side.x + side.width - 4.0, side.center_y()),
    );
    assert_eq!((selected(&app), app.strip.target()), ("game-06", 6));
    assert!(!app.panel_open, "a side cover only moves");
    let middle = cover_at(&mut window, &app, 6);
    window.click(&mut app, middle.center());
    assert!(app.panel_open, "the middle opens like a grid card");
    assert_eq!(selected(&app), "game-06");
}

/// A left press at `point`, and what the widgets publish for it.
fn press_at(
    window: &mut crate::more_menu::tests::Window,
    app: &mut App,
    point: Point,
) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut laid = window.lay_out(app);
    messages.extend(laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
        iced::mouse::Cursor::Available(point),
    ));
    let tree = laid.into_tree();
    *window = crate::more_menu::tests::Window::with_tree(tree);
    window.send(app, messages.clone());
    messages
}

fn double_clicked(messages: &[Message]) -> bool {
    messages
        .iter()
        .any(|message| matches!(message, Message::DoubleClick(_)))
}

#[test]
fn a_quick_second_click_on_another_middle_cover_isnt_a_double_click() {
    let (_root, mut app) = flat(10);
    select(&mut app, "game-04");
    let mut window = crate::more_menu::tests::Window::new(&mut app);
    // A side cover, then the middle it became.
    let side = cover_at(&mut window, &app, 5);
    let point = Point::new(side.x + side.width - 4.0, side.center_y());
    assert!(!double_clicked(&press_at(&mut window, &mut app, point)));
    assert_eq!(selected(&app), "game-05");
    // Pressing there again at once isn't a double click on the cover it
    // centered.
    assert!(!double_clicked(&press_at(&mut window, &mut app, point)));
    select(&mut app, "game-05");
    // The panel comes and goes with these presses, moving the strip, so
    // the middle is found again each time.
    let middle = |window: &mut crate::more_menu::tests::Window, app: &App| {
        cover_at(window, app, app.strip.target()).center()
    };
    let point = middle(&mut window, &app);
    assert!(!double_clicked(&press_at(&mut window, &mut app, point)));
    // The middle, then a key moves another one there: the same spot again
    // at once is a plain press on whatever is there now.
    press(&mut app, Named::ArrowRight);
    assert_eq!(selected(&app), "game-06");
    assert!(!double_clicked(&press_at(&mut window, &mut app, point)));
    select(&mut app, "game-06");
    let _ = app.update(Message::ClosePanel);
    app.panel_motion.snap(false);
    let point = middle(&mut window, &app);
    assert!(!double_clicked(&press_at(&mut window, &mut app, point)));
    // Twice on the same middle cover is a double click, though the first
    // press toggled the panel and moved the strip from under the pointer.
    let messages = press_at(&mut window, &mut app, point);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::DoubleClick(name) if name == "game-06")),
        "{messages:?}"
    );
}

#[test]
fn a_click_on_a_section_card_centers_it_then_opens_it() {
    let (_root, mut app) = sectioned();
    collapse(&mut app, "b");
    select(&mut app, "a-5");
    let mut window = crate::more_menu::tests::Window::new(&mut app);
    let card = cover_at(&mut window, &app, 6);
    window.click(
        &mut app,
        Point::new(card.x + card.width - 4.0, card.center_y()),
    );
    assert_eq!(middle(&app), "[b]");
    assert!(app.collapsed.contains(&tag("b")));
    let card = cover_at(&mut window, &app, 6);
    window.click(&mut app, card.center());
    assert!(!app.collapsed.contains(&tag("b")));
    assert_eq!(middle(&app), "b-0");
}

#[test]
fn the_wheel_moves_one_cover_per_notch_either_way() {
    let (_root, mut app) = flat(10);
    select(&mut app, "game-04");
    let mut window = crate::more_menu::tests::Window::new(&mut app);
    let point = strip_bounds(&mut window, &app).center();
    let mut wheel = |app: &mut App, delta| {
        window.event_at(
            app,
            iced::Event::Mouse(iced::mouse::Event::WheelScrolled { delta }),
            point,
        );
    };
    use iced::mouse::ScrollDelta::{Lines, Pixels};
    wheel(&mut app, Lines { x: 0.0, y: -1.0 });
    assert_eq!(selected(&app), "game-05");
    wheel(&mut app, Lines { x: 0.0, y: 2.0 });
    assert_eq!(selected(&app), "game-03");
    // Sideways counts too.
    wheel(&mut app, Lines { x: -1.0, y: 0.0 });
    assert_eq!(selected(&app), "game-04");
    // A touchpad's pixels add up to notches.
    wheel(&mut app, Pixels { x: 0.0, y: -40.0 });
    assert_eq!(selected(&app), "game-04");
    wheel(&mut app, Pixels { x: 0.0, y: -40.0 });
    assert_eq!(selected(&app), "game-05");
}

#[test]
fn the_label_above_the_strip_collapses_the_middle_section() {
    let (_root, mut app) = sectioned();
    select(&mut app, "b-2");
    let mut window = crate::more_menu::tests::Window::new(&mut app);
    let label = find(&mut window, &app, tag("b").header_id()).expect("the label is on screen");
    window.click(&mut app, label.center());
    assert!(app.collapsed.contains(&tag("b")));
    assert_eq!(middle(&app), "[b]");
    // Ungrouped there's no label.
    let (_root, flat_app) = flat(4);
    let places = flat_app.grid_places();
    assert!(places.headers.is_empty());
}

#[test]
fn the_zoom_and_titles_hide_and_the_zoom_keys_do_nothing() {
    let (_root, mut app) = flat(10);
    assert!(!app.zoom_allowed());
    let zoom = app.zoom;
    let _ = app.update(Message::Zoom(crate::zoom::Event::Key(crate::zoom::Key::In)));
    assert_eq!(app.zoom, zoom);
    let mut window = crate::more_menu::tests::Window::new(&mut app);
    for id in [
        crate::zoom_control::SLIDER_ID,
        crate::zoom_control::POSTER_ONLY_ID,
    ] {
        assert_eq!(window.bounds(&app, id), None, "{id}");
    }
}

fn art_card(name: &str) -> crate::library::Card {
    let mut card = card(name, State::Inactive, 0);
    card.art = Some(crate::tests::art_file(&format!("/tmp/{name}.png"), 1));
    card
}

fn with_art(count: usize, size: Size) -> (tempfile::TempDir, App) {
    let (root, mut app) = boot();
    let cards = (0..count)
        .map(|index| art_card(&format!("game-{index:02}")))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    let _ = app.switch_layout(ViewLayout::Coverflow);
    let _ = app.update(Message::Strip(Event::Measured(size)));
    (root, app)
}

fn art_of(app: &App, name: &str) -> crate::library::ArtFile {
    app.cards
        .iter()
        .find(|card| card.name == name)
        .and_then(|card| card.art.clone())
        .expect("art")
}

fn kinds(keys: &[(ArtKey, bool)], kind: fn(&artwork::Kind) -> bool) -> Vec<String> {
    let mut names: Vec<String> = keys
        .iter()
        .filter(|((_, held), _)| kind(held))
        .map(|((file, _), _)| {
            file.path
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn is_grid(kind: &artwork::Kind) -> bool {
    *kind == artwork::Kind::Grid
}

fn is_cover(kind: &artwork::Kind) -> bool {
    matches!(kind, artwork::Kind::Cover(_))
}

#[test]
fn the_strip_wants_the_drawn_posters_and_the_middle_three_large() {
    let (_root, mut app) = with_art(20, Size::new(2400.0, 1400.0));
    select(&mut app, "game-10");
    let keys = app.strip_art();
    // The nine on screen and one past each end.
    let posters: Vec<String> = (5..=15).map(|index| format!("game-{index:02}")).collect();
    assert_eq!(kinds(&keys, is_grid), posters);
    assert_eq!(kinds(&keys, is_cover), ["game-09", "game-10", "game-11"]);
    // The middle's poster comes first, then its large cover is wanted.
    assert_eq!(keys[0].0, (art_of(&app, "game-10"), artwork::Kind::Grid));
    // What the strip wants is what eviction protects.
    assert_eq!(
        app.visible_art,
        keys.iter().map(|(key, _)| key.clone()).collect()
    );
    // A small window decodes nothing larger than the grid poster.
    let _ = app.update(Message::Strip(Event::Measured(Size::new(900.0, 600.0))));
    assert!(kinds(&app.strip_art(), is_cover).is_empty());
}

#[test]
fn no_large_cover_is_asked_for_while_the_strip_moves() {
    let (_root, mut app) = with_art(20, Size::new(2400.0, 1400.0));
    select(&mut app, "game-10");
    app.strip.position = 9.4;
    let keys = app.strip_art();
    assert!(kinds(&keys, is_cover).is_empty(), "{keys:?}");
    assert!(!kinds(&keys, is_grid).is_empty());
}

#[test]
fn a_large_cover_that_left_the_middle_while_decoding_is_dropped() {
    let (_root, mut app) = with_art(20, Size::new(2400.0, 1400.0));
    select(&mut app, "game-10");
    let kind = app.cover_kind().expect("a large cover");
    let pixel = image::Handle::from_rgba(1, 1, vec![0; 4]);
    let stale = (art_of(&app, "game-02"), kind);
    app.art_loaded(stale.clone(), false, pixel.clone());
    assert!(!app.images.contains(&stale, false));
    let fresh = (art_of(&app, "game-10"), kind);
    app.art_loaded(fresh.clone(), false, pixel);
    assert!(app.images.contains(&fresh, false));
}

#[test]
fn a_larger_cover_already_held_is_kept_after_a_shrink() {
    let (_root, mut app) = with_art(20, Size::new(2400.0, 1400.0));
    select(&mut app, "game-10");
    let big = app.cover_kind().expect("a large cover");
    let pixel = image::Handle::from_rgba(1, 1, vec![0; 4]);
    let held = (art_of(&app, "game-10"), big);
    app.art_loaded(held.clone(), false, pixel);
    let _ = app.update(Message::Strip(Event::Measured(Size::new(1800.0, 1100.0))));
    let smaller = app.cover_kind().expect("still large");
    assert_ne!(smaller, big);
    let keys = app.strip_art();
    assert!(keys.iter().any(|(key, _)| *key == held), "{keys:?}");
    assert!(
        !keys
            .iter()
            .any(|(key, _)| *key == (art_of(&app, "game-10"), smaller))
    );
    // The two neighbors have none, so they want the size that fits now.
    assert!(
        keys.iter()
            .any(|(key, _)| *key == (art_of(&app, "game-11"), smaller))
    );
}

#[test]
fn the_middle_draws_its_large_cover_once_it_arrives_and_the_poster_until_then() {
    let (_root, mut app) = with_art(20, Size::new(2400.0, 1400.0));
    select(&mut app, "game-10");
    let card = app
        .cards
        .iter()
        .find(|card| card.name == "game-10")
        .cloned()
        .unwrap();
    let poster = image::Handle::from_rgba(2, 3, vec![0; 24]);
    app.images.insert(
        (art_of(&app, "game-10"), artwork::Kind::Grid),
        false,
        poster.clone(),
    );
    assert_eq!(app.strip_handle(&card, 10), Some(poster.clone()));
    let large = image::Handle::from_rgba(4, 6, vec![0; 96]);
    let kind = app.cover_kind().unwrap();
    app.art_loaded((art_of(&app, "game-10"), kind), false, large.clone());
    assert_eq!(app.strip_handle(&card, 10), Some(large));
    // Two stops out it's the poster, large or not.
    assert_eq!(app.strip_handle(&card, 12), Some(poster));
}

#[test]
fn the_middle_draws_its_art_and_the_side_covers_draw_darker() {
    let (_root, mut app) = with_art(9, WIDE);
    select(&mut app, "game-04");
    let _ = app.update(Message::ClosePanel);
    for card in app.cards.clone() {
        let key = (card.art.clone().unwrap(), artwork::Kind::Grid);
        app.images.insert(
            key,
            false,
            // About the size drawn: tiny-skia places a scaled image in
            // whole source pixels, so a tiny one lands far off.
            image::Handle::from_rgba(330, 495, [200, 0, 0, 255].repeat(330 * 495)),
        );
    }
    let mut element = app.strip_view();
    let pixels = crate::layout_probe::render(&mut element, WIDE);
    let at = |point: Point| {
        let index = (point.y as usize * WIDE.width as usize + point.x as usize) * 4;
        pixels[index..index + 4].to_vec()
    };
    // Ungrouped there's no label, so the strip has the whole height.
    let geometry = geometry(app.strip.position(), 9, WIDE);
    let middle = at(cover(&geometry, 4).bounds.center());
    assert_eq!(middle, [200, 0, 0, 255]);
    // The outermost cover's right part shows past its neighbor, darker.
    let side = cover(&geometry, 8).bounds;
    let side = at(Point::new(side.x + side.width * 0.8, side.center_y()));
    assert!(side[0] + 60 < middle[0] && side[0] > 0, "{side:?}");
}

#[test]
fn a_switch_into_coverflow_centers_the_kept_selection_and_back_shows_it() {
    let (_root, mut app) = flat(40);
    let _ = app.switch_layout(ViewLayout::Grid);
    app.viewport_width = 1000.0;
    app.viewport_height = 600.0;
    select(&mut app, "game-27");
    let _ = app.switch_layout(ViewLayout::Coverflow);
    assert_eq!(middle(&app), "game-27");
    assert_eq!(app.strip.target(), 27);
    let _ = app.switch_layout(ViewLayout::Grid);
    let places = app.grid_places();
    let slot = places.slots[places.first("game-27").unwrap()];
    assert!(
        slot.y >= app.scroll_offset,
        "{slot:?} at {}",
        app.scroll_offset
    );
    assert!(
        slot.y + slot.height <= app.scroll_offset + app.viewport_height,
        "{slot:?} at {}",
        app.scroll_offset
    );
}

#[test]
fn a_saved_coverflow_starts_on_the_first_game() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join(".config/wineo");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("library.toml"),
        "schema_version = 1\n[view]\nlayout = \"coverflow\"\n",
    )
    .unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let mut app = App::boot(paths).0;
    assert_eq!(app.layout, ViewLayout::Coverflow);
    let cards = (0..3)
        .map(|index| card(&format!("game-{index}"), State::Inactive, 0))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert_eq!(selected(&app), "game-0");
    assert_eq!(middle(&app), "game-0");
}

#[test]
fn esc_on_a_section_card_clears_the_search_and_leaves_the_panel() {
    let (_root, mut app) = sectioned();
    collapse(&mut app, "b");
    select(&mut app, "a-5");
    let _ = app.update(Message::SearchChanged("-".into()));
    press(&mut app, Named::ArrowRight);
    assert_eq!(middle(&app), "[b]");
    press(&mut app, Named::Escape);
    assert!(app.search.is_empty(), "Esc fell through to the search");
    assert!(app.panel_open);
    assert_eq!(middle(&app), "[b]");
}

#[test]
fn the_strip_takes_no_input_under_a_dialog() {
    let (_root, mut app) = flat(10);
    select(&mut app, "game-04");
    app.close_prompt = true;
    let places = app.grid_places();
    let id = StopId::of(&places, places.strip[6]).unwrap();
    let _ = app.update(Message::Strip(Event::Center(id.clone())));
    assert_eq!(selected(&app), "game-04");
    app.close_prompt = false;
    let _ = app.update(Message::Strip(Event::Center(id)));
    assert_eq!(selected(&app), "game-06");
}

#[test]
fn a_narrow_strip_holds_only_its_few_covers_and_one_past_each_end() {
    let (_root, mut app) = with_art(20, Size::new(500.0, 900.0));
    select(&mut app, "game-10");
    let shown = on_screen(app.strip_size());
    assert!(shown < 9, "{shown}");
    let side = shown / 2 + 1;
    let posters: Vec<String> = (10 - side..=10 + side)
        .map(|index| format!("game-{index:02}"))
        .collect();
    assert_eq!(kinds(&app.strip_art(), is_grid), posters);
    // At the start there's nothing before it to hold.
    select(&mut app, "game-00");
    assert_eq!(kinds(&app.strip_art(), is_grid)[0], "game-00");
    assert_eq!(kinds(&app.strip_art(), is_grid).len(), side + 1);
}

#[test]
fn large_covers_count_against_the_budget_and_age_out_once_past_the_middle() {
    let (_root, mut app) = with_art(30, Size::new(2400.0, 1400.0));
    let kind = app.cover_kind().expect("a large cover");
    let artwork::Kind::Cover(height) = kind else {
        unreachable!()
    };
    let width = u32::from(height) * 2 / 3;
    let cover = || {
        image::Handle::from_rgba(
            width,
            u32::from(height),
            vec![0; (width * u32::from(height) * 4) as usize],
        )
    };
    let one = (width * u32::from(height) * 4) as usize;
    // Room for four large covers.
    app.images = crate::art_cache::ArtCache::with_budget(4 * one);
    let mut held = Vec::new();
    for middle in [5, 15, 25] {
        select(&mut app, &format!("game-{middle:02}"));
        for stop in middle - 1..=middle + 1 {
            let key = (art_of(&app, &format!("game-{stop:02}")), kind);
            app.art_loaded(key.clone(), false, cover());
            held.push(key);
        }
    }
    assert!(app.images.bytes() <= 4 * one, "{}", app.images.bytes());
    // The middle three of the last stop stay; the first ones went.
    for key in &held[6..] {
        assert!(app.images.contains(key, false), "{key:?}");
    }
    assert!(!app.images.contains(&held[0], false));
    assert!(!app.images.contains(&held[3], false));
}

#[test]
fn a_large_cover_is_two_thirds_as_wide_as_its_step() {
    let mut png = Vec::new();
    ::image::DynamicImage::ImageRgba8(::image::RgbaImage::new(1000, 1500))
        .write_to(
            &mut std::io::Cursor::new(&mut png),
            ::image::ImageFormat::Png,
        )
        .unwrap();
    for (step, size) in [(720, (480, 720)), (1200, (800, 1200))] {
        let handle = artwork::decode_bytes(&png, artwork::Kind::Cover(step)).unwrap();
        let iced::widget::image::Handle::Rgba { width, height, .. } = handle else {
            panic!("decoded to pixels");
        };
        assert_eq!((width, height), size, "{step}");
    }
}

#[test]
fn the_display_scale_sizes_the_large_cover() {
    let (_root, mut app) = with_art(20, WIDE);
    assert_eq!(app.cover_kind(), Some(artwork::Kind::Cover(720)));
    let _ = app.update(Message::DisplayScale(2.0));
    assert_eq!(app.cover_kind(), Some(artwork::Kind::Cover(1200)));
    let _ = app.update(app.window_event(iced::window::Event::Rescaled(1.0)));
    assert_eq!(app.cover_kind(), Some(artwork::Kind::Cover(720)));
}

/// Ctrl and `key` as the window delivers it: to the widgets, then the app.
fn ctrl(window: &mut crate::more_menu::tests::Window, app: &mut App, key: &str) {
    let key = Key::Character(key.into());
    let event = iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key.clone(),
        physical_key: iced::keyboard::key::Physical::Unidentified(
            iced::keyboard::key::NativeCode::Unidentified,
        ),
        location: iced::keyboard::Location::Standard,
        modifiers: Modifiers::CTRL,
        text: None,
        repeat: false,
    });
    let mut laid = window.lay_out(app);
    let (mut messages, status) = laid.event_with_status(&event, iced::mouse::Cursor::Unavailable);
    drop(laid);
    messages.push(Message::Key(key, Modifiers::CTRL, status));
    window.send(app, messages);
}

#[test]
fn the_zoom_keys_do_nothing_in_coverflow_as_in_the_list() {
    let (_root, mut app) = flat(10);
    for (layout, moves) in [
        (ViewLayout::Grid, true),
        (ViewLayout::List, false),
        (ViewLayout::Coverflow, false),
    ] {
        let _ = app.switch_layout(layout);
        app.zoom = 100;
        let mut window = crate::more_menu::tests::Window::new(&mut app);
        ctrl(&mut window, &mut app, "=");
        assert_eq!(app.zoom != 100, moves, "{layout:?} in");
        app.zoom = 100;
        ctrl(&mut window, &mut app, "-");
        assert_eq!(app.zoom != 100, moves, "{layout:?} out");
        app.zoom = 150;
        ctrl(&mut window, &mut app, "0");
        assert_eq!(app.zoom != 150, moves, "{layout:?} reset");
    }
}

/// The strip drawn at `WIDE` with every game's art one solid red, and the
/// middle cover's bounds.
fn drawn(app: &mut App) -> (Vec<u8>, Rectangle) {
    for card in app.cards.clone() {
        let key = (card.art.clone().unwrap(), artwork::Kind::Grid);
        app.images.insert(
            key,
            false,
            // About the size drawn: tiny-skia places a scaled image in
            // whole source pixels, so a tiny one lands far off.
            image::Handle::from_rgba(330, 495, [200, 0, 0, 255].repeat(330 * 495)),
        );
    }
    let mut element = app.strip_view();
    let pixels = crate::layout_probe::render(&mut element, WIDE);
    let middle = cover(
        &geometry(app.strip.position(), app.cards.len(), WIDE),
        app.strip.target(),
    );
    (pixels, middle.bounds)
}

fn pixel(pixels: &[u8], point: Point) -> [u8; 4] {
    let index = (point.y as usize * WIDE.width as usize + point.x as usize) * 4;
    pixels[index..index + 4].try_into().unwrap()
}

/// Where a badge's chip sits on a cover `bounds` large, left or right.
fn chip(bounds: Rectangle, left: bool) -> Rectangle {
    let scale = bounds.height / GridLayout::default().poster_height();
    let (size, margin) = (
        crate::library_actions::MARK_SIZE * scale,
        crate::library_actions::MARK_MARGIN * scale,
    );
    let x = if left {
        bounds.x + margin
    } else {
        bounds.x + bounds.width - margin - size
    };
    Rectangle::new(Point::new(x, bounds.y + margin), Size::new(size, size))
}

#[test]
fn a_favorite_cover_shows_the_star_and_a_marked_one_the_check() {
    let (_root, mut app) = with_art(9, WIDE);
    select(&mut app, "game-04");
    let _ = app.update(Message::ClosePanel);
    let (plain, middle) = drawn(&mut app);
    let star = chip(middle, false);
    let check = chip(middle, true);
    let red = [200, 0, 0, 255];
    assert_eq!(pixel(&plain, star.center()), red);
    assert_eq!(pixel(&plain, check.center()), red);
    for card in &mut app.cards {
        if card.name == "game-04" {
            card.favorite = true;
        }
    }
    app.marked.insert("game-04".into());
    let (badged, _) = drawn(&mut app);
    // The star's white glyph at its chip's middle.
    let center = pixel(&badged, star.center());
    assert!(center[0] > 230 && center[1] > 230, "{center:?}");
    // The check's chip in the primary color, beside its glyph.
    let ring = pixel(
        &badged,
        Point::new(check.x + check.width * 0.15, check.center_y()),
    );
    let primary = app.palette.primary.into_rgba8();
    for channel in 0..3 {
        assert!(
            ring[channel].abs_diff(primary[channel]) < 30,
            "{ring:?} vs {primary:?}"
        );
    }
    // Scaled with the cover: twice the grid's 24 px at this size.
    assert!(star.width > 1.9 * crate::library_actions::MARK_SIZE);
}

#[test]
fn delete_does_nothing_on_a_section_card() {
    let (_root, mut app) = sectioned();
    // b-0 shows in c too, so Delete can't fall back on it being hidden.
    let mut cards = app.cards.clone();
    for card in &mut cards {
        if card.name == "b-0" {
            card.tags.push("c".into());
        }
    }
    let _ = app.update(Message::Loaded(Ok(cards)));
    collapse(&mut app, "b");
    select(&mut app, "a-5");
    press(&mut app, Named::ArrowRight);
    assert_eq!(middle(&app), "[b]");
    press(&mut app, Named::Delete);
    assert!(app.removal.is_none());
    // On a game it asks as anywhere.
    press(&mut app, Named::ArrowLeft);
    press(&mut app, Named::Delete);
    assert!(app.removal.is_some());
}
