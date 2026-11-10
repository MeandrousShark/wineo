//! A font switch while the app runs. Each scenario runs in a test process of
//! its own, since a remap changes the one font system every test measures
//! with.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use iced::widget::{pick_list, text};
use iced::{Element, Font, Size};

use super::fixtures::{WIDE, install_wide};
use super::*;
use crate::layout_probe::{lay_out, relay_out, render_with, renderer};

const CHILD: &str = "WINEO_GUI_FONT_SWITCH_CHILD";

/// Gives this process the font system and adds `WIDE`.
fn setup() {
    live::turn_on();
    install_wide();
}

fn switch_to(family: Option<&str>) {
    remap(resolve(family));
}

fn label() -> Element<'static, ()> {
    text("Interface size").into()
}

/// A text label laid out once keeps its widget state, and so its shaped
/// paragraph, across a switch, as a window's widgets do between updates.
fn label_width_follows_the_family() {
    let size = Size::new(800.0, 100.0);
    let laid = lay_out(label(), size);
    let noto = laid.node.bounds().width;
    let mut tree = laid.into_tree();

    switch_to(Some(WIDE));
    assert_eq!(current(), WIDE);
    let laid = relay_out(label(), tree, size);
    let wide = laid.node.bounds().width;
    assert!(wide > noto * 1.8, "{noto} then {wide}");
    tree = laid.into_tree();

    switch_to(None);
    assert_eq!(current(), BUILT_IN);
    let laid = relay_out(label(), tree, size);
    assert_eq!(laid.node.bounds().width, noto);
    tree = laid.into_tree();

    // Without the version bump the paragraph keeps its old shaping, which
    // is why `remap` loads an empty buffer. If iced starts reshaping on its
    // own, this fails and the bump can go.
    font_system()
        .write()
        .expect("font system lock")
        .raw()
        .db_mut()
        .set_sans_serif_family(WIDE);
    let laid = relay_out(label(), tree, size);
    assert_eq!(laid.node.bounds().width, noto);
    switch_to(None);
}

/// A card's state row wraps in the card's width, so a wider family makes it
/// taller, and the places the grid keeps must follow.
fn card_height_follows_the_family() {
    use crate::layout_probe::layout;
    use wineo_core::runtime::State;
    let mut app = crate::tests::app();
    let mut card = crate::tests::card("game", State::Inactive, 60);
    card.playtime = "1234 hours 56 minutes".to_owned();
    app.merge(vec![card]);
    let height = |app: &crate::App| {
        let places = app.grid_places();
        assert_eq!(places.slots.len(), 1);
        let mut view = app.card(&app.cards[0]);
        let built = layout(&mut view, Size::new(400.0, 900.0)).bounds().height;
        assert_eq!(places.slots[0].height, built, "the places match the card");
        built
    };
    let noto = height(&app);
    let kept = app.grid_places();

    switch_to(Some(WIDE));
    let wide = height(&app);
    assert!(wide > noto, "{noto} then {wide}");
    assert!(!std::rc::Rc::ptr_eq(&kept, &app.grid_places()));

    switch_to(None);
    assert_eq!(height(&app), noto);
}

fn pick(font: Option<Font>) -> Element<'static, ()> {
    let list = pick_list(["Follow the system"], Some("Follow the system"), |_| ()).width(300);
    match font {
        Some(font) => list.font(font).into(),
        None => crate::ui::PickListStyle::wineo_style(list, crate::theme::Palette::DARK).into(),
    }
}

/// A pick list's label draws through the renderer's text cache, whose key
/// has the font but not the font system's version. Each renderer here draws
/// one list every frame, as a window redraws one that stays on screen.
fn cached_text_follows_the_family() {
    let size = Size::new(320.0, 48.0);
    let mut generic = renderer();
    let mut styled = renderer();
    let generic_noto = render_with(&mut generic, &mut pick(Some(Font::DEFAULT)), size);
    let styled_noto = render_with(&mut styled, &mut pick(None), size);

    switch_to(Some(WIDE));
    let generic_wide = render_with(&mut generic, &mut pick(Some(Font::DEFAULT)), size);
    let styled_wide = render_with(&mut styled, &mut pick(None), size);
    // The generic family keeps its old glyphs, which is why the app's pick
    // lists name the family. If iced fixes this, this fails and they can
    // go back to the generic one.
    assert!(
        generic_wide == generic_noto,
        "the cache kept the old glyphs"
    );
    assert!(
        styled_wide != styled_noto,
        "the styled list follows the switch"
    );
    // What a fresh renderer draws is what the styled one shows.
    let fresh = render_with(&mut renderer(), &mut pick(None), size);
    assert!(fresh == styled_wide);

    switch_to(None);
    assert!(render_with(&mut styled, &mut pick(None), size) == styled_noto);
}

#[test]
fn child_process_scenario() {
    let Ok(scenario) = std::env::var(CHILD) else {
        return;
    };
    setup();
    match scenario.as_str() {
        "label" => label_width_follows_the_family(),
        "card" => card_height_follows_the_family(),
        "cached" => cached_text_follows_the_family(),
        other => panic!("unknown scenario {other}"),
    }
}

fn run_child(scenario: &str) {
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fonts::switch_tests::child_process_scenario",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, scenario)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = crate::test_script::KillOnDrop::new(child);
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the child never ended"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{scenario}: {}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_laid_out_label_changes_width_on_a_switch_and_back() {
    run_child("label");
}

#[test]
fn a_card_height_follows_a_switch() {
    run_child("card");
}

#[test]
fn text_from_the_renderer_cache_follows_a_switch() {
    run_child("cached");
}
