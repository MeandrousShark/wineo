use std::time::{Duration, Instant};

use iced::Task;
use wineo_core::runtime::State;

use super::*;
use crate::App;
use crate::layout_probe::lay_out;
use crate::zoom::{Event as Zoom, Key};

#[test]
fn only_a_readable_false_reduces_motion() {
    assert!(reduced_from::<()>(Ok(false)));
    assert!(!reduced_from::<()>(Ok(true)));
    assert!(!reduced_from(Err("no portal")));
}

#[test]
fn the_setting_overrides_the_system_unless_it_follows_it() {
    for system_reduced in [false, true] {
        let motion = |setting| Motion {
            setting,
            system_reduced,
        };
        assert_eq!(motion(Setting::System).reduced(), system_reduced);
        assert!(!motion(Setting::Full).reduced());
        assert!(motion(Setting::Reduced).reduced());
    }
}

#[test]
fn the_dropdown_labels_each_setting_in_order() {
    let labels: Vec<String> = Choice::ALL.iter().map(Choice::to_string).collect();
    assert_eq!(labels, ["Follow the system", "Full", "Reduced"]);
}

fn library(count: usize) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let mut app = App::boot(paths).0;
    let cards = (0..count)
        .map(|index| crate::tests::card(&format!("game-{index:03}"), State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    (root, app)
}

/// Reduced motion the way the desktop asks for it.
fn reduced(count: usize) -> (tempfile::TempDir, App) {
    let (root, mut app) = library(count);
    let _ = app.update(Message::SystemMotion(true));
    assert!(app.motion.reduced());
    (root, app)
}

/// Where card `index` is drawn now, from the viewport's top.
fn top(app: &App, index: usize) -> f32 {
    app.grid_motion.drawn(&app.grid_places(), index).y - app.scroll_offset
}

fn zoom_frame(app: &mut App, at: Instant) -> Task<Message> {
    let generation = app.zooming.generation();
    app.update(Message::Zoom(Zoom::Frame(generation, at)))
}

fn grid_tick(app: &mut App, at: Instant) -> Task<Message> {
    app.update(Message::GridTick(app.grid_motion.generation(), at))
}

fn located(app: &App, task: Task<Message>, name: &str) -> bool {
    crate::sort_control::tests::answers(app, task)
        .iter()
        .any(|message| matches!(message, Message::GridCardLocated(found, ..) if found == name))
}

#[test]
fn follow_the_system_follows_the_desktop_switch() {
    let (_root, mut app) = library(4);
    assert!(
        !app.motion.reduced(),
        "full until the portal says otherwise"
    );
    let _ = app.update(Message::SystemMotion(true));
    assert!(app.motion.reduced());
    let _ = app.update(Message::SystemMotion(false));
    assert!(!app.motion.reduced());

    // A choice of its own ignores the switch.
    app.appearance.motion = Setting::Full;
    let _ = app.update(Message::SystemMotion(true));
    assert!(!app.motion.reduced());
    app.appearance.motion = Setting::Reduced;
    let _ = app.update(Message::SystemMotion(false));
    assert!(app.motion.reduced());
}

#[test]
fn reduced_motion_lands_a_zoom_on_its_first_frame_and_keeps_the_place() {
    let (_root, mut app) = reduced(120);
    app.scroll_offset = top(&app, 21) + app.scroll_offset - 120.0;
    app.selected = Some(app.filtered()[21].name.clone());
    for step in [Key::In, Key::In, Key::Out, Key::Out, Key::Out] {
        let _ = app.update(Message::Zoom(Zoom::Key(step)));
        // Started, not skipped, so its frame does the place keeping.
        assert!(app.zooming.active());
        let _ = zoom_frame(&mut app, Instant::now());
        assert!(!app.zooming.moving(), "{step:?}");
        assert!(!app.zooming.anchoring(), "{step:?}");
        assert_eq!(app.zooming.shown(), None);
        assert_eq!(top(&app, 21), 120.0, "{step:?}");
        // A column change springs, which lands on its first tick too.
        if app.grid_motion.active() {
            let _ = grid_tick(&mut app, Instant::now());
            assert!(!app.grid_motion.active(), "{step:?}");
            assert!(app.grid_motion.anchor.is_none());
            assert_eq!(top(&app, 21), 120.0, "{step:?}");
        }
    }
    assert_eq!(app.zoom, 90);
}

#[test]
fn reduced_motion_scrolls_an_off_screen_selection_once_when_the_zoom_lands() {
    let (_root, mut app) = reduced(120);
    let name = app.filtered()[100].name.clone();
    app.selected = Some(name.clone());
    let _ = app.update(Message::Zoom(Zoom::Key(Key::In)));
    let mut task = zoom_frame(&mut app, Instant::now());
    if app.grid_motion.active() {
        assert!(!located(&app, task, &name), "waits for the grid");
        task = grid_tick(&mut app, Instant::now());
    }
    assert!(located(&app, task, &name));
    assert!(app.grid_scroll_after_settle.is_none());
    assert!(!app.grid_motion.active() && !app.zooming.moving());
}

#[test]
fn reduced_motion_lands_the_dock_on_its_first_tick_keeping_the_place_and_the_scroll() {
    let (_root, mut app) = reduced(120);
    app.scroll_offset = top(&app, 21) + app.scroll_offset - 120.0;
    let name = app.filtered()[21].name.clone();
    let columns = app.columns();
    let _ = app.update(Message::Select(name.clone()));
    assert!(app.panel_open);
    assert_ne!(app.columns(), columns, "the dock changes the columns");
    // The spring is set out, not skipped, so the dock's anchor is taken
    // from where the cards were drawn.
    assert!(app.grid_motion.active());
    assert!(app.grid_motion.anchor.is_some());
    let _ = app.scroll_selection();
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some(name.as_str()));

    let task = grid_tick(&mut app, Instant::now());
    assert!(!app.grid_motion.active());
    assert!(app.grid_motion.anchor.is_none());
    assert_eq!(top(&app, 21), 120.0);
    assert!(located(&app, task, &name));
    assert!(app.grid_scroll_after_settle.is_none());
}

#[test]
fn full_motion_still_springs_and_tweens() {
    let (_root, mut app) = library(120);
    // Full wins over a desktop asking for less.
    app.appearance.motion = Setting::Full;
    let _ = app.update(Message::SystemMotion(true));
    assert!(!app.motion.reduced());
    let name = app.filtered()[21].name.clone();
    let _ = app.update(Message::Select(name));
    let _ = grid_tick(&mut app, Instant::now() + Duration::from_millis(16));
    assert!(app.grid_motion.active(), "springs over several frames");
    let _ = crate::tests::settle_grid(&mut app);
    assert!(!app.grid_motion.active());

    let (_root, mut app) = library(120);
    app.appearance.motion = Setting::Full;
    let _ = app.update(Message::SystemMotion(true));
    let _ = app.update(Message::Zoom(Zoom::Key(Key::In)));
    let start = Instant::now();
    let _ = zoom_frame(&mut app, start + Duration::from_millis(80));
    assert!(app.zooming.active(), "tweens over several frames");
}

#[test]
fn reduced_motion_keeps_the_fades_and_drops_the_movement() {
    let (_root, mut app) = reduced(40);
    // Cards a search brings back fade in where they land.
    let _ = app.update(Message::SearchChanged("game-00".to_owned()));
    let _ = crate::tests::settle_grid(&mut app);
    let _ = app.update(Message::SearchChanged(String::new()));
    let entering = app.grid_places().ids[30].clone();
    assert_eq!(app.grid_motion.opacity(&entering), 0.0);
    let _ = grid_tick(&mut app, Instant::now() + Duration::from_millis(16));
    let opacity = app.grid_motion.opacity(&entering);
    assert!(opacity > 0.0 && opacity < 1.0, "{opacity}");
    assert!(app.grid_motion.active(), "still fading");
    assert_eq!(app.grid_motion.offset(&entering), iced::Vector::ZERO);

    // The details fade as the zoom crosses 90%, without the drift.
    let _ = crate::tests::settle_grid(&mut app);
    for step in [Key::Out, Key::Out] {
        let _ = app.update(Message::Zoom(Zoom::Key(step)));
        let _ = zoom_frame(&mut app, Instant::now());
    }
    assert!(!app.details_full());
    assert!(app.details_fade.active());
    let start = Instant::now();
    let generation = app.details_fade.generation();
    let _ = app.update(Message::DetailsTick(generation, start));
    let _ = app.update(Message::DetailsTick(
        generation,
        start + Duration::from_millis(80),
    ));
    let progress = app.details_progress();
    assert!(progress > 0.0 && progress < 1.0, "{progress}");
    assert_eq!(app.details_drift(progress), 0.0);
    app.appearance.motion = Setting::Full;
    let _ = app.update(Message::SystemMotion(true));
    assert!(app.details_drift(progress) > 0.0);

    // The panel still fades in, though it no longer slides.
    let (_root, mut app) = reduced(4);
    let name = app.filtered()[0].name.clone();
    let _ = app.update(Message::Select(name));
    assert!(app.panel_motion.active());
    assert!(!app.panel_motion.interactive());
}

#[test]
fn the_spinner_keeps_turning_with_reduced_motion() {
    let (_root, mut app) = reduced(4);
    app.change = Some(crate::removal::Change::Trash(
        crate::trash_page::Change::Prune { ids: Vec::new() },
    ));
    let mut laid = lay_out(app.sidebar(), app.window_size);
    assert_eq!(
        laid.redraw(Instant::now()),
        iced::window::RedrawRequest::NextFrame
    );
}
