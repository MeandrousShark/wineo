//! The details fade as the app drives it: the identifier and state row
//! fading and drifting through a 90% crossing on their own frame clock,
//! drawn in a band clipped above the card below.

use std::time::{Duration, Instant};

use iced::Size;
use iced::advanced::subscription::into_recipes;
use wineo_core::runtime::State;

use super::{GAP, GridState};
use crate::details_fade::DRIFT;
use crate::layout_probe::{lay_out, render};
use crate::library::Card;
use crate::tests::{card, settle_grid};
use crate::zoom::{Event, Key, ease_out};
use crate::{App, Message};

/// An app on its own throwaway HOME at `zoom`, as if saved there, with
/// `count` running cards, so each one's state dot shows bright.
fn fixture(zoom: u16, count: usize) -> (tempfile::TempDir, App) {
    let cards = (0..count)
        .map(|index| card(&format!("game-{index:02}"), State::Running, 60))
        .collect();
    fixture_with(zoom, cards)
}

fn fixture_with(zoom: u16, cards: Vec<Card>) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let mut app = App::boot(paths).0;
    app.zoom = zoom;
    let _ = app.update(Message::Loaded(Ok(cards)));
    let _ = settle_grid(&mut app);
    assert!(settled(&app));
    (root, app)
}

/// The test's frame clock, which the details fade reads from.
struct Clock(Instant);

impl Clock {
    /// Starts at the first frame after an update that set out a crossing.
    fn start(app: &mut App) -> Self {
        let now = Instant::now();
        let _ = app.update(Message::DetailsTick(app.details_fade.generation(), now));
        Self(now)
    }

    fn tick(&mut self, app: &mut App, millis: u64) {
        self.0 += Duration::from_millis(millis);
        let _ = app.update(Message::DetailsTick(app.details_fade.generation(), self.0));
    }
}

fn zoom(app: &mut App, event: Event) {
    let _ = app.update(Message::Zoom(event));
}

fn progress(app: &App) -> f32 {
    app.details_progress()
}

fn near(value: f32, expected: f32) -> bool {
    (value - expected).abs() < 0.01
}

/// The eased value `millis` into a crossing from `from` to `to`.
fn eased(from: f32, to: f32, millis: u64) -> f32 {
    from + (to - from) * ease_out(millis as f32 / 160.0)
}

fn frames(app: &App) -> usize {
    into_recipes(app.details_fade.subscription()).len()
}

fn settled(app: &App) -> bool {
    !app.details_fade.active() && frames(app) == 0
}

fn grid_size(app: &App) -> Size {
    Size::new(app.viewport_width, 2000.0)
}

/// The grid's state after a layout: the details' nodes and their clips.
fn laid_state(app: &App) -> (iced::advanced::layout::Node, GridState) {
    let laid = lay_out(app.grid(), grid_size(app));
    let node = laid.node.clone();
    let tree = laid.into_tree();
    let state = tree.state.downcast_ref::<GridState>();
    (
        node,
        GridState {
            details: state.details.clone(),
            bands: state.bands.clone(),
            looks: state.looks.clone(),
            ..GridState::default()
        },
    )
}

/// The red, green, and blue summed over `area` of the rendered grid, so
/// a fainter drawing sums lower.
fn ink(pixels: &[u8], width: f32, area: iced::Rectangle) -> u64 {
    let width = width as usize;
    let mut sum = 0;
    for y in area.y as usize..(area.y + area.height) as usize {
        for x in area.x as usize..(area.x + area.width) as usize {
            let at = (y * width + x) * 4;
            sum += u64::from(pixels[at]) + u64::from(pixels[at + 1]) + u64::from(pixels[at + 2]);
        }
    }
    sum
}

/// The first card's state dot, where it's drawn at the details' drift.
fn dot_ink(app: &App) -> u64 {
    let (_, state) = laid_state(app);
    let Some(details) = state.details.first() else {
        return 0;
    };
    let details = details.bounds();
    let drift = crate::details_fade::DetailsFade::drift(progress(app));
    let dot = iced::Rectangle::new(
        iced::Point::new(details.x, details.y + drift + (details.height - 8.0) / 2.0),
        Size::new(8.0, 8.0),
    );
    let size = grid_size(app);
    ink(&render(&mut app.grid(), size), size.width, dot)
}

/// The first card's poster and title, down to where its details start.
fn card_ink(app: &App) -> u64 {
    let (node, state) = laid_state(app);
    let mut card = node.children()[0].bounds();
    if let Some(details) = state.details.first() {
        card.height = card.height.min(details.bounds().y - card.y);
    }
    let size = grid_size(app);
    ink(&render(&mut app.grid(), size), size.width, card)
}

#[test]
fn a_crossing_out_fades_and_drifts_the_details_while_title_cards_keep_them() {
    let (_root, mut app) = fixture(90, 3);
    let full = dot_ink(&app);
    assert!(full > 0);
    assert_eq!(progress(&app), 1.0);
    zoom(&mut app, Event::Key(Key::Out));
    assert_eq!(app.grid_layout().text(), super::CardText::Title);
    assert!(app.details_fade.active());
    assert!(frames(&app) > 0);
    let mut clock = Clock::start(&mut app);
    assert_eq!(progress(&app), 1.0);
    clock.tick(&mut app, 40);
    let at = progress(&app);
    assert!(near(at, eased(1.0, 0.0, 40)), "{at}");
    // A Title card still builds its details while they fade, drifting down.
    let (_, state) = laid_state(&app);
    assert_eq!(state.details.len(), 3);
    let faint = dot_ink(&app);
    assert!(faint < full && faint > 0, "{faint} {full}");
    clock.tick(&mut app, 120);
    assert_eq!(progress(&app), 0.0);
    assert!(settled(&app));
    let (_, state) = laid_state(&app);
    assert!(state.details.is_empty());
}

#[test]
fn a_crossing_in_fades_the_details_up_into_place() {
    let (_root, mut app) = fixture(80, 3);
    assert_eq!(progress(&app), 0.0);
    let (_, state) = laid_state(&app);
    assert!(state.details.is_empty());
    zoom(&mut app, Event::Key(Key::In));
    assert_eq!(app.grid_layout().text(), super::CardText::Full);
    let mut clock = Clock::start(&mut app);
    // Full cards build them at once, from nothing and a full drift below.
    assert_eq!(progress(&app), 0.0);
    assert_eq!(
        crate::details_fade::DetailsFade::drift(progress(&app)),
        DRIFT
    );
    let (_, state) = laid_state(&app);
    assert_eq!(state.details.len(), 3);
    assert_eq!(dot_ink(&app), 0);
    clock.tick(&mut app, 60);
    let at = progress(&app);
    assert!(near(at, eased(0.0, 1.0, 60)), "{at}");
    let mid = dot_ink(&app);
    clock.tick(&mut app, 100);
    assert!(settled(&app));
    assert_eq!(crate::details_fade::DetailsFade::drift(progress(&app)), 0.0);
    let full = dot_ink(&app);
    assert!(mid > 0 && mid < full, "{mid} {full}");
}

#[test]
fn a_reversal_mid_crossing_carries_on_from_where_the_details_are() {
    let (_root, mut app) = fixture(90, 3);
    zoom(&mut app, Event::Key(Key::Out));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 40);
    let at = progress(&app);
    zoom(&mut app, Event::Key(Key::In));
    assert_eq!(progress(&app), at);
    assert!(app.details_fade.active());
    // The next frame sets out again from there.
    let mut clock = Clock::start(&mut app);
    assert_eq!(progress(&app), at);
    clock.tick(&mut app, 80);
    let now = progress(&app);
    assert!(near(now, eased(at, 1.0, 80)), "{now}");
    clock.tick(&mut app, 80);
    assert_eq!(progress(&app), 1.0);
    assert!(settled(&app));
}

#[test]
fn a_card_entering_mid_crossing_joins_at_the_same_progress() {
    let (_root, mut app) = fixture(80, 4);
    let _ = app.update(Message::SearchChanged("game-0".into()));
    let _ = app.update(Message::SearchChanged("game-01".into()));
    let _ = settle_grid(&mut app);
    zoom(&mut app, Event::Key(Key::In));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 50);
    let at = progress(&app);
    assert!(at > 0.0 && at < 1.0);
    // A filter brings the others back, and they draw at the shared value.
    let _ = app.update(Message::SearchChanged(String::new()));
    assert_eq!(progress(&app), at);
    let (_, state) = laid_state(&app);
    assert_eq!(state.details.len(), 4);
    // A sort reorders them mid-crossing without restarting it.
    let _ = app.update(Message::Sort(crate::sort_control::Event::FlipDirection));
    assert_eq!(progress(&app), at);
    clock.tick(&mut app, 50);
    let now = progress(&app);
    assert!(near(now, eased(0.0, 1.0, 100)), "{now}");
    clock.tick(&mut app, 60);
    assert!(settled(&app));
}

#[test]
fn the_title_keeps_its_strength_through_a_crossing() {
    for (from, key) in [(90, Key::Out), (80, Key::In)] {
        let (_root, mut app) = fixture(from, 3);
        zoom(&mut app, Event::Key(key));
        // The zoom's own frames aren't sent, so only the details change.
        let mut clock = Clock::start(&mut app);
        let first = card_ink(&app);
        assert!(first > 0);
        for _ in 0..4 {
            clock.tick(&mut app, 40);
            assert_eq!(card_ink(&app), first, "{from}");
        }
        assert!(settled(&app));
    }
}

#[test]
fn the_band_reaches_the_card_below_by_index_or_the_old_slot_without_one() {
    let (_root, mut app) = fixture(90, 8);
    // A second row that's partly filled, so some cards have none below.
    let columns = app.columns();
    assert!(columns > 4 && columns < 8, "{columns}");
    // Steady full text clips the same way a crossing does.
    for crossing in [false, true] {
        if crossing {
            zoom(&mut app, Event::Key(Key::Out));
            // The rows spring up to their Title places.
            let _ = settle_grid(&mut app);
            let mut clock = Clock::start(&mut app);
            clock.tick(&mut app, 40);
        }
        let (node, state) = laid_state(&app);
        let cards: Vec<_> = node.children().iter().map(|card| card.bounds()).collect();
        assert_eq!(state.details.len(), 8);
        for (index, card) in cards.iter().enumerate() {
            let band = state.bands[index].expect("a band");
            assert_eq!((band.x, band.y, band.width), (card.x, card.y, card.width));
            let details = state.details[index].bounds();
            let own = details.y + details.height + GAP;
            let bottom = match cards.get(index + columns) {
                Some(below) => below.y.min(own),
                None => own,
            };
            assert_eq!(band.y + band.height, bottom, "{crossing} {index}");
        }
        if crossing {
            // The Title rows sit higher, so the second row cuts the first
            // row's details short.
            let details = state.details[0].bounds();
            assert!(cards[columns].y < details.y + details.height + GAP);
        }
    }
}

#[test]
fn the_details_finish_when_the_zoom_stops_early_or_the_library_is_left() {
    let (_root, mut app) = fixture(90, 3);
    zoom(&mut app, Event::Key(Key::Out));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 40);
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    assert!(!app.zooming.moving());
    assert!(frames(&app) > 0);
    let before = into_recipes(app.subscription()).len();
    clock.tick(&mut app, 60);
    assert!(near(progress(&app), eased(1.0, 0.0, 100)));
    clock.tick(&mut app, 60);
    assert_eq!(progress(&app), 0.0);
    assert!(settled(&app));
    assert_eq!(into_recipes(app.subscription()).len(), before - 1);
}

#[test]
fn the_details_finish_when_a_flow_opens_mid_crossing() {
    let (_root, mut app) = fixture(80, 3);
    zoom(&mut app, Event::Key(Key::In));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 40);
    app.flow = Some(crate::flow::Page::choose(app.paths.clone()));
    assert!(frames(&app) > 0);
    let before = into_recipes(app.subscription()).len();
    clock.tick(&mut app, 60);
    assert!(near(progress(&app), eased(0.0, 1.0, 100)));
    clock.tick(&mut app, 60);
    assert_eq!(progress(&app), 1.0);
    assert!(settled(&app));
    assert_eq!(into_recipes(app.subscription()).len(), before - 1);
}

#[test]
fn a_drag_across_90_sets_out_and_turns_back() {
    let (_root, mut app) = fixture(100, 3);
    zoom(&mut app, Event::Drag(85.0));
    assert!(app.details_fade.active());
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 40);
    let at = progress(&app);
    assert!(at < 1.0);
    zoom(&mut app, Event::Drag(95.0));
    assert_eq!(progress(&app), at);
    zoom(&mut app, Event::DragEnded);
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 160);
    assert_eq!(progress(&app), 1.0);
    assert!(settled(&app));
}

#[test]
fn the_poster_only_toggle_snaps_the_details() {
    let (_root, mut app) = fixture(100, 3);
    zoom(&mut app, Event::TogglePosterOnly);
    assert_eq!(progress(&app), 0.0);
    assert!(settled(&app));
    // Poster-only cards build no details.
    let (_, state) = laid_state(&app);
    assert!(state.details.is_empty());
    zoom(&mut app, Event::TogglePosterOnly);
    assert_eq!(progress(&app), 1.0);
    assert!(settled(&app));
    let (_, state) = laid_state(&app);
    assert_eq!(state.details.len(), 3);

    // A toggle mid-crossing ends the crossing at the new target.
    let (_root, mut app) = fixture(90, 3);
    zoom(&mut app, Event::Key(Key::Out));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 40);
    assert!(progress(&app) > 0.0 && progress(&app) < 1.0);
    zoom(&mut app, Event::TogglePosterOnly);
    assert_eq!(progress(&app), 0.0);
    assert!(settled(&app));
    let (_root, mut app) = fixture(90, 3);
    zoom(&mut app, Event::Key(Key::Out));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 40);
    zoom(&mut app, Event::Key(Key::In));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 40);
    let _ = app.update(Message::Zoom(Event::TogglePosterOnly));
    assert_eq!(progress(&app), 0.0);
    assert!(settled(&app));
    zoom(&mut app, Event::TogglePosterOnly);
    assert_eq!(progress(&app), 1.0);
    assert!(settled(&app));
}

#[test]
fn a_long_frame_gap_finishes_the_crossing() {
    let (_root, mut app) = fixture(90, 3);
    zoom(&mut app, Event::Key(Key::Out));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 10_000);
    assert_eq!(progress(&app), 0.0);
    assert!(settled(&app));
}

#[test]
fn a_saved_zoom_below_90_shows_no_crossing_at_launch() {
    let (_root, app) = fixture(80, 3);
    assert_eq!(progress(&app), 0.0);
    assert!(settled(&app));
}

#[test]
fn a_wrapping_state_row_stays_one_line_only_while_the_details_fade() {
    let mut steam = card("game-00", State::SteamRunning, 60);
    steam.playtime = "587 h 17 m".into();
    let (_root, mut app) = fixture_with(90, vec![steam.clone()]);
    let line = crate::grid::context_line();
    // (details height, card height, and the card's height from `card_height`)
    let measure = |app: &App| {
        let (node, state) = laid_state(app);
        let details = state.details.first().map(|details| details.bounds().height);
        let card = node.children()[0].bounds().height;
        (
            details,
            card,
            crate::grid::card_height(&steam, app.grid_layout()),
        )
    };
    let (wrapped, card, height) = measure(&app);
    let wrapped = wrapped.unwrap();
    assert!(wrapped > line + 1.0, "{wrapped}");
    assert_eq!(card, height);

    zoom(&mut app, Event::Key(Key::Out));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 40);
    let (details, card, height) = measure(&app);
    assert_eq!(details, Some(line));
    assert_eq!(card, height);
    clock.tick(&mut app, 120);
    assert!(settled(&app));
    let (details, card, height) = measure(&app);
    assert_eq!(details, None);
    assert_eq!(card, height);

    zoom(&mut app, Event::Key(Key::In));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 40);
    let (details, card, height) = measure(&app);
    assert_eq!(details, Some(line));
    assert_eq!(card, height);
    clock.tick(&mut app, 120);
    assert!(settled(&app));
    let (details, card, height) = measure(&app);
    assert_eq!(details, Some(wrapped));
    assert_eq!(card, height);
}
