use super::*;

fn placeholder() -> image::Handle {
    image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255])
}

const AREA: Area = Area {
    floor: 700.0,
    width: 900.0,
    columns: 3,
    drop: 700.0,
};

#[test]
fn ten_clicks_inside_three_seconds_complete_a_burst() {
    let start = Instant::now();
    let mut clicks = Clicks::default();
    for index in 0..9 {
        assert!(!clicks.record(start + Duration::from_millis(index * 200)));
    }
    assert!(clicks.record(start + Duration::from_millis(1800)));
    // The burst starts over rather than firing on every later click.
    assert!(!clicks.record(start + Duration::from_millis(1900)));
}

#[test]
fn slow_clicks_never_complete_a_burst() {
    let start = Instant::now();
    let mut clicks = Clicks::default();
    for index in 0..30 {
        assert!(!clicks.record(start + Duration::from_millis(index * 350)));
    }
}

#[test]
fn cards_fall_to_rest_above_the_floor_and_go_quiet() {
    let start = Instant::now();
    let mut collapse = Collapse::default();
    collapse.fall(start, 6, 0..4, AREA, placeholder(), 7);
    let generation = collapse.generation();
    assert!(collapse.animating());
    let slot = Point::new(16.0, 16.0);
    assert!(matches!(collapse.look(0, slot), Look::Normal(_)));
    collapse.tick(generation, start + Duration::from_millis(400));
    assert!(matches!(collapse.look(0, slot), Look::Loose { .. }));
    assert_eq!(collapse.look(5, slot), Look::Hidden);
    assert_eq!(collapse.caption_alpha(), 0.0);
    collapse.tick(generation, start + Duration::from_secs(4));
    assert!(collapse.resting());
    assert!(!collapse.animating());
    for index in 0..4 {
        let Look::Loose { position, .. } = collapse.look(index, slot) else {
            panic!("card {index} should lie in the heap");
        };
        let bottom = position.y + FRAME + 2.0 * HALF_POSTER.y;
        assert!(bottom > AREA.floor - 2.0 * HALF_POSTER.y - 60.0);
        assert!(position.y + FRAME + HALF_POSTER.y < AREA.floor);
        assert!(position.x >= 0.0 && position.x + 160.0 <= AREA.width);
    }
}

#[test]
fn rebuild_flies_back_to_the_slots_and_ends_idle() {
    let start = Instant::now();
    let mut collapse = Collapse::default();
    collapse.fall(start, 3, 0..3, AREA, placeholder(), 1);
    let generation = collapse.generation();
    collapse.tick(generation, start + Duration::from_secs(4));
    collapse.restore(start + Duration::from_secs(4));
    assert!(collapse.animating());
    let slot = Point::new(192.0, 16.0);
    collapse.tick(generation, start + Duration::from_millis(4010));
    assert!(matches!(collapse.look(1, slot), Look::Loose { .. }));
    assert_eq!(collapse.caption_alpha(), 0.0);
    collapse.tick(generation, start + Duration::from_millis(4299));
    let Look::Normal(offset) = collapse.look(1, slot) else {
        panic!("a card nearly home turns back into the real card");
    };
    assert!(offset.x.abs() < 1.0 && offset.y.abs() < 1.0);
    assert!(collapse.caption_alpha() > 0.9);
    collapse.tick(generation, start + Duration::from_millis(4400));
    assert!(collapse.idle());
    assert_eq!(collapse.look(1, slot), Look::Normal(Vector::ZERO));
}

#[test]
fn faded_cards_return_when_restored() {
    let start = Instant::now();
    let mut collapse = Collapse::default();
    collapse.fade(start);
    let generation = collapse.generation();
    collapse.tick(generation, start + Duration::from_secs(1));
    assert!(collapse.alpha() > 0.0 && collapse.alpha() < 1.0);
    collapse.tick(generation, start + FADE_OUT);
    assert!(collapse.gone());
    assert!(!collapse.animating());
    assert_eq!(collapse.look(0, Point::ORIGIN), Look::Hidden);
    collapse.restore(start + Duration::from_secs(5));
    assert!(collapse.animating());
    collapse.tick(generation, start + Duration::from_secs(6));
    assert!(collapse.idle());
    assert_eq!(collapse.alpha(), 1.0);
}

#[test]
fn stale_ticks_are_ignored() {
    let start = Instant::now();
    let mut collapse = Collapse::default();
    collapse.fade(start);
    let stale = collapse.generation();
    collapse.reset();
    collapse.fade(start);
    collapse.tick(stale, start + FADE_OUT);
    assert!(!collapse.gone());
}

mod app {
    use std::time::{Duration, Instant};

    use iced::Size;
    use iced::keyboard::Modifiers;
    use wineo_core::runtime::State;

    use crate::sidebar::Section;
    use crate::tests::{app, card};
    use crate::{App, Message};

    fn library(count: usize) -> App {
        let mut app = app();
        app.cards = (1..=count)
            .map(|index| card(&format!("game-{index}"), State::Inactive, 0))
            .collect();
        app.grid_motion.snap(false, count);
        app
    }

    fn click_library(app: &mut App, times: usize) {
        for _ in 0..times {
            let _ = app.update(Message::SectionClicked(Section::Library));
        }
    }

    fn tick(app: &mut App, after: Duration) {
        let generation = app.collapse.generation();
        let _ = app.update(Message::CollapseTick(generation, Instant::now() + after));
    }

    #[test]
    fn nine_clicks_change_nothing_and_ten_collapse() {
        let mut app = library(6);
        click_library(&mut app, 9);
        assert!(app.collapse.idle());
        click_library(&mut app, 1);
        assert!(!app.collapse.idle());
        assert!(app.collapse.animating());
        assert_eq!(app.section, Section::Library);
        assert!(!app.panel_open);
    }

    #[test]
    fn keyboard_shortcuts_never_count() {
        let mut app = library(6);
        for _ in 0..12 {
            crate::shortcuts::tests::press(&mut app, "1", Modifiers::CTRL);
        }
        assert_eq!(app.section, Section::Library);
        assert!(app.collapse.idle());
    }

    #[test]
    fn fallen_cards_rest_quietly_and_a_click_rebuilds_without_opening() {
        let mut app = library(6);
        click_library(&mut app, 10);
        tick(&mut app, Duration::from_secs(4));
        assert!(app.collapse.resting());
        // The subscription only asks for frames while this is true.
        assert!(!app.collapse.animating());
        let _ = app.update(Message::Select("game-2".into()));
        let _ = app.update(Message::DoubleClick("game-2".into()));
        assert!(!app.panel_open);
        assert!(app.selected.is_none());
        assert!(app.operation.is_none());
        let _ = app.update(Message::Rebuild);
        assert!(app.collapse.animating());
        tick(&mut app, Duration::from_secs(5));
        assert!(app.collapse.idle());
        assert!(!app.panel_open);
        let _ = app.update(Message::Select("game-2".into()));
        assert!(app.panel_open);
    }

    #[test]
    fn the_heap_and_the_empty_library_stay_until_clicked() {
        let mut app = library(6);
        click_library(&mut app, 10);
        tick(&mut app, Duration::from_secs(4));
        tick(&mut app, Duration::from_secs(60));
        let _ = app.update(Message::Refresh);
        assert!(app.collapse.resting());
        assert!(!app.collapse.animating());
        let _ = app.update(Message::Rebuild);
        tick(&mut app, Duration::from_secs(61));
        assert!(app.collapse.idle());

        app.reduced_motion = true;
        click_library(&mut app, 10);
        tick(&mut app, Duration::from_secs(4));
        tick(&mut app, Duration::from_secs(60));
        assert!(app.collapse.gone());
        assert!(!app.collapse.animating());
    }

    #[test]
    fn leaving_the_library_or_filtering_restores_at_once() {
        let mut app = library(6);
        click_library(&mut app, 10);
        let _ = app.update(Message::Section(Section::Trash));
        assert!(app.collapse.idle());
        let _ = app.update(Message::Section(Section::Library));
        click_library(&mut app, 10);
        assert!(!app.collapse.idle());
        let _ = app.update(Message::SearchChanged("game-1".into()));
        assert!(app.collapse.idle());
        let _ = app.update(Message::SearchChanged(String::new()));
        click_library(&mut app, 10);
        let _ = app.update(Message::Window(iced::window::Event::Resized(Size::new(
            900.0, 700.0,
        ))));
        assert!(app.collapse.idle());
        click_library(&mut app, 10);
        assert!(!app.collapse.idle());
        let mut cards = app.cards.clone();
        cards.push(card("game-new", State::Inactive, 0));
        let _ = app.update(Message::Loaded(Ok(cards)));
        assert!(app.collapse.idle());
        click_library(&mut app, 10);
        assert!(!app.collapse.idle());
        app.scroll_offset = 40.0;
        app.on_scroll_offset(40.0);
        assert!(!app.collapse.idle());
        app.on_scroll_offset(120.0);
        assert!(app.collapse.idle());
    }

    #[test]
    fn a_far_scrolled_heap_settles_as_fast_as_one_at_the_top() {
        let mut top = library(200);
        click_library(&mut top, 10);
        let mut deep = library(200);
        deep.scroll_offset = 6000.0;
        click_library(&mut deep, 10);
        tick(&mut top, Duration::from_secs(3));
        tick(&mut deep, Duration::from_secs(3));
        assert!(top.collapse.resting());
        assert!(deep.collapse.resting());
    }

    #[test]
    fn an_open_panel_closes_before_the_drop() {
        let mut app = library(6);
        let _ = app.update(Message::Select("game-1".into()));
        assert!(app.panel_open);
        click_library(&mut app, 10);
        assert!(!app.panel_open);
        assert!(!app.panel_motion.active());
        assert!(!app.grid_motion.active());
        assert!(!app.collapse.idle());
        assert_eq!(app.selected.as_deref(), Some("game-1"));
    }

    #[test]
    fn reduced_motion_fades_out_and_the_empty_library_brings_it_back() {
        let mut app = library(6);
        app.reduced_motion = true;
        click_library(&mut app, 10);
        tick(&mut app, Duration::from_secs(1));
        assert!(app.collapse.alpha() > 0.0 && app.collapse.alpha() < 1.0);
        tick(&mut app, Duration::from_secs(4));
        assert!(app.collapse.gone());
        assert!(!app.collapse.animating());
        assert!(app.collapse.faded());
        // The library area's own click handler sends this.
        let _ = app.update(Message::Rebuild);
        assert!(app.collapse.animating());
        tick(&mut app, Duration::from_secs(6));
        assert!(app.collapse.idle());
        assert_eq!(app.collapse.alpha(), 1.0);
    }

    #[test]
    fn collapse_moves_only_drawn_cards_never_the_grid() {
        use crate::layout_probe::layout;
        let mut app = library(6);
        let size = Size::new(app.viewport_width, 10_000.0);
        let idle = layout(&mut app.grid(), size);
        click_library(&mut app, 10);
        tick(&mut app, Duration::from_secs(4));
        let fallen = layout(&mut app.grid(), size);
        assert_eq!(fallen.size(), idle.size());
        let floor = app.scroll_offset + app.viewport_height;
        for (before, after) in idle.children().iter().zip(fallen.children()) {
            let poster_center = after.bounds().y + 2.0 + 118.0;
            assert!(after.bounds().y > before.bounds().y);
            assert!(poster_center < floor && poster_center > floor - 300.0);
        }
        assert_eq!(app.scroll_offset, 0.0);
    }
}
