//! The pad in the app, all on made-up events: the gate, the filter, and
//! what each press stands for.

use std::time::Duration;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::window;
use wineo_core::runtime::State;

use super::*;
use crate::pad::{Axis, PadEvent, PadId, PadInfo, Raw};
use crate::test_tasks::each_ready;
use crate::tests::card;

mod hints;

fn xbox() -> PadInfo {
    PadInfo {
        name: "Xbox Wireless Controller".to_owned(),
        vendor: Some(0x045e),
        product: Some(0x0b13),
    }
}

fn switch_pro() -> PadInfo {
    PadInfo {
        name: "Nintendo Switch Pro Controller".to_owned(),
        vendor: Some(0x057e),
        product: Some(0x2009),
    }
}

/// An app on its own HOME, since presses write `library.toml` (a favorite,
/// the layout) and the shared test HOME would carry that to other tests.
fn booted() -> (tempfile::TempDir, App) {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| path.as_os_str().to_owned())
    })
    .unwrap();
    (home, App::boot(paths).0)
}

/// A library of three games with an Xbox pad connected and the gate shut,
/// as at launch.
fn closed_app() -> (tempfile::TempDir, App) {
    let (home, mut app) = booted();
    let cards = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(|name| card(name, State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    app.viewport_width = 1000.0;
    app.snap_grid();
    let _ = app.update(Message::Pad(PadEvent::Connected(PadId::Gilrs(0), xbox())));
    (home, app)
}

/// `closed_app` after the window's first real focus, with the cursor on
/// the first game and its panel shut.
fn ready() -> (tempfile::TempDir, App) {
    let (home, mut app) = closed_app();
    focus(&mut app);
    move_to(&mut app, "alpha");
    (home, app)
}

fn focus(app: &mut App) {
    let id = app.main_window.expect("the main window");
    let _ = app.update(Message::Window(id, window::Event::Focused));
}

fn move_to(app: &mut App, name: &str) {
    let _ = app.update(Message::Select(name.to_owned()));
    let _ = app.update(Message::ClosePanel);
    assert_eq!(app.selected.as_deref(), Some(name));
    assert!(!app.panel_open);
}

/// What `task` sends, with its widget operations run in the app's laid out
/// view. Anything that waits, like a wakeup, is left unrun.
fn outputs(app: &App, task: Task<Message>) -> Vec<Message> {
    if task.units() == 0 {
        return Vec::new();
    }
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    // Laid out only once an operation needs it, since most presses send none.
    let mut laid = None;
    let mut messages = Vec::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    each_ready(&runtime, &mut stream, |action| match action {
        iced_runtime::Action::Output(message) => messages.push(message),
        iced_runtime::Action::Widget(operation) => laid
            .get_or_insert_with(|| crate::layout_probe::lay_out(app.view(), app.window_size))
            .run(operation),
        _ => {}
    });
    messages
}

/// Runs the key path a press starts: the text field check, the page's
/// search check, and Space's focus check.
fn settle(app: &mut App, task: Task<Message>) {
    for message in outputs(app, task) {
        if matches!(
            message,
            Message::PadKey(..) | Message::KeyWithFocus(..) | Message::SpaceFocus(_)
        ) {
            let next = app.update(message);
            settle(app, next);
        }
    }
}

fn tap_on(app: &mut App, pad: usize, raw: Raw) {
    let pad = PadId::Gilrs(pad);
    let task = app.update(Message::Pad(PadEvent::Pressed(pad, raw)));
    settle(app, task);
    let task = app.update(Message::Pad(PadEvent::Released(pad, raw)));
    settle(app, task);
}

fn tap(app: &mut App, raw: Raw) {
    tap_on(app, 0, raw);
}

fn favorite(app: &App, name: &str) -> bool {
    app.cards
        .iter()
        .any(|card| card.name == name && card.favorite)
}

#[test]
fn the_gate_starts_closed_and_opens_on_a_real_focus_or_press() {
    let (_home, mut app) = closed_app();
    // `focused` starts true on purpose; the gate doesn't.
    assert!(app.focused);
    assert!(!app.pad_gate_open());
    move_to(&mut app, "alpha");
    tap(&mut app, Raw::South);
    assert!(!app.panel_open);
    assert!(app.pad_hint.is_none());

    focus(&mut app);
    tap(&mut app, Raw::South);
    assert!(app.panel_open);

    // A press in the window counts as focus too.
    let (_home, mut app) = closed_app();
    let _ = app.update(Message::Pressed);
    move_to(&mut app, "beta");
    tap(&mut app, Raw::South);
    assert!(app.panel_open);
}

#[test]
fn losing_focus_hiding_and_closing_shut_the_gate() {
    let (_home, mut app) = ready();
    let id = app.main_window.unwrap();
    let _ = app.update(Message::Window(id, window::Event::Unfocused));
    assert!(!app.pad_gate_open());
    tap(&mut app, Raw::South);
    assert!(!app.panel_open);

    // The tray's reopen says focused without a Focused event, which a
    // compositor that refused the raise never sends.
    let (_home, mut app) = ready();
    let _ = app.hide_window();
    assert!(!app.pad_gate.is_open());
    let _ = app.open_window();
    assert!(app.focused);
    assert!(app.main_window.is_some());
    assert!(!app.pad_gate_open());
    tap(&mut app, Raw::South);
    assert!(!app.panel_open);

    let (_home, mut app) = ready();
    app.closing = true;
    assert!(!app.pad_gate_open());
    tap(&mut app, Raw::South);
    assert!(!app.panel_open);
}

#[test]
fn a_launch_or_a_steam_request_shuts_the_gate() {
    use wineo_core::steam::AppState;
    let (_home, mut app) = ready();
    app.merge(vec![crate::tests::steam_card("game", AppState::Installed)]);
    let _ = app.update(Message::Select("game".into()));
    let _ = app.update(Message::Play);
    assert!(app.operation.is_some());
    assert!(!app.pad_gate_open());
    // Only a real focus opens it again.
    focus(&mut app);
    assert!(app.pad_gate_open());

    let (_home, mut app) = ready();
    app.merge(vec![crate::tests::steam_card(
        "owned",
        AppState::NotInstalled,
    )]);
    let _ = app.update(Message::Select("owned".into()));
    let _ = app.update(Message::Install);
    assert!(app.steam_request.is_some());
    assert!(!app.pad_gate_open());
}

#[test]
fn a_tool_run_shuts_the_gate() {
    // Every route to a run goes through the page's own start, whatever the
    // kind; Run program's check needs a real runner, so winetricks stands
    // in.
    use crate::tools_page::{Event, Kind};
    for start in [Event::OpenWinetricks, Event::InstallMissing] {
        let (home, mut app) = ready();
        for card in &mut app.cards {
            card.prefix = Some(home.path().join("prefix"));
        }
        let _ = app.update(Message::OpenTool("alpha".into(), Kind::Winetricks));
        assert!(app.pad_gate_open());
        let _ = app.update(Message::Tool("alpha".into(), Kind::Winetricks, start));
        assert!(app.tool_pages[&("alpha".to_owned(), Kind::Winetricks)].busy());
        assert!(!app.pad_gate_open());
        // Only a real focus opens it again.
        tap(&mut app, Raw::East);
        assert!(app.tool_page.is_some());
        focus(&mut app);
        assert!(app.pad_gate_open());
        // Later events from the running tool don't shut it again.
        let _ = app.update(Message::Tool(
            "alpha".into(),
            Kind::Winetricks,
            Event::Program("regedit".into()),
        ));
        assert!(app.tool_pages[&("alpha".to_owned(), Kind::Winetricks)].busy());
        assert!(app.pad_gate_open());
    }
}

#[test]
fn enter_on_a_tool_page_starts_the_run_and_shuts_the_gate() {
    use crate::tools_page::{Event, Kind};
    let (home, mut app) = ready();
    let start = std::time::Instant::now();
    app.pad_clock = Some(start);
    for card in &mut app.cards {
        card.prefix = Some(home.path().join("prefix"));
    }
    let _ = app.update(Message::OpenTool("alpha".into(), Kind::Winetricks));
    for event in [Event::Verb("corefonts".into()), Event::AddVerb] {
        let _ = app.update(Message::Tool("alpha".into(), Kind::Winetricks, event));
    }
    assert!(app.pad_gate_open());
    // Enter with no field focused goes through the page's own submit, not
    // `tool_update`.
    let _ = app.update(Message::Key(
        Key::Named(Named::Enter),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    assert!(app.tool_pages[&("alpha".to_owned(), Kind::Winetricks)].busy());
    assert!(!app.pad_gate_open());
    // The same Enter's trailing press doesn't open it again.
    let _ = app.update(Message::Pressed);
    assert!(!app.pad_gate_open());
    app.pad_clock = Some(start + PRESS_GRACE);
    let _ = app.update(Message::Pressed);
    assert!(app.pad_gate_open());
}

#[test]
fn a_tool_page_edit_or_refused_run_leaves_the_gate_open() {
    use crate::tools_page::{Event, Kind};
    let (home, mut app) = ready();
    for card in &mut app.cards {
        card.prefix = Some(home.path().join("prefix"));
    }
    let _ = app.update(Message::OpenTool("alpha".into(), Kind::Exec));
    let _ = app.update(Message::Tool(
        "alpha".into(),
        Kind::Exec,
        Event::Program("winecfg".into()),
    ));
    assert!(app.pad_gate_open());
    let _ = app.update(Message::Tool(
        "alpha".into(),
        Kind::Exec,
        Event::Program(String::new()),
    ));
    // The page refuses an empty program, so nothing starts.
    let _ = app.update(Message::Tool("alpha".into(), Kind::Exec, Event::Submit));
    let page = &app.tool_pages[&("alpha".to_owned(), Kind::Exec)];
    assert_eq!(page.error(), Some("Choose a program."));
    assert!(!page.busy());
    assert!(app.pad_gate_open());
}

#[test]
fn the_gate_ignores_a_press_inside_the_grace_after_a_close() {
    let start = std::time::Instant::now();
    let mut gate = Gate::default();
    assert!(!gate.is_open());
    // Nothing closed it yet, so the first press opens it.
    gate.press(start);
    assert!(gate.is_open());
    gate.close(start);
    gate.press(start);
    gate.press(start + PRESS_GRACE - Duration::from_millis(1));
    assert!(!gate.is_open());
    gate.press(start + PRESS_GRACE);
    assert!(gate.is_open());
    // A real focus opens it at once.
    gate.close(start);
    gate.focus();
    assert!(gate.is_open());
}

/// A launch route's own message, then the trailing press of the same
/// input, all at one pinned instant.
fn launch_then_its_press(app: &mut App, launch: Message) {
    let _ = app.update(launch);
    assert!(!app.pad_gate_open(), "the launch shuts the gate");
    let _ = app.update(Message::Pressed);
}

fn steam_ready(name: &str, install: wineo_core::steam::AppState) -> (tempfile::TempDir, App) {
    let (home, mut app) = ready();
    app.merge(vec![crate::tests::steam_card(name, install)]);
    app.pad_clock = Some(std::time::Instant::now());
    (home, app)
}

#[test]
fn a_launchs_own_trailing_press_leaves_the_gate_shut() {
    use wineo_core::steam::AppState;
    // A double click on the card.
    let (_home, mut app) = steam_ready("game", AppState::Installed);
    launch_then_its_press(&mut app, Message::DoubleClick("game".into()));
    assert!(app.operation.is_some());
    assert!(!app.pad_gate_open(), "double click");

    // Enter or Space on the focused Play button.
    let (_home, mut app) = steam_ready("game", AppState::Installed);
    let _ = app.update(Message::Select("game".into()));
    launch_then_its_press(&mut app, Message::Play);
    assert!(!app.pad_gate_open(), "Play");

    // Enter on the focused Install button, a Steam handoff.
    let (_home, mut app) = steam_ready("owned", AppState::NotInstalled);
    let _ = app.update(Message::Select("owned".into()));
    launch_then_its_press(&mut app, Message::Install);
    assert!(app.steam_request.is_some());
    assert!(!app.pad_gate_open(), "Install");

    // Enter in a tool page's field, which submits it.
    use crate::tools_page::{Event, Kind};
    let (home, mut app) = ready();
    app.pad_clock = Some(std::time::Instant::now());
    for card in &mut app.cards {
        card.prefix = Some(home.path().join("prefix"));
    }
    let _ = app.update(Message::OpenTool("alpha".into(), Kind::Winetricks));
    for event in [Event::Verb("corefonts".into()), Event::AddVerb] {
        let _ = app.update(Message::Tool("alpha".into(), Kind::Winetricks, event));
    }
    launch_then_its_press(
        &mut app,
        Message::Tool("alpha".into(), Kind::Winetricks, Event::Submit),
    );
    assert!(!app.pad_gate_open(), "tool run");
}

#[test]
fn after_a_launch_focus_reopens_at_once_and_a_later_press_after_the_grace() {
    use wineo_core::steam::AppState;
    let (_home, mut app) = steam_ready("game", AppState::Installed);
    let start = app.pad_clock.unwrap();
    launch_then_its_press(&mut app, Message::DoubleClick("game".into()));
    // A compositor that never sends focus: a press just inside the grace
    // still counts as the launch's, and the next one after it opens.
    app.pad_clock = Some(start + PRESS_GRACE - Duration::from_millis(1));
    let _ = app.update(Message::Pressed);
    assert!(!app.pad_gate_open());
    app.pad_clock = Some(start + PRESS_GRACE);
    let _ = app.update(Message::Pressed);
    assert!(app.pad_gate_open());

    // A real focus doesn't wait.
    let (_home, mut app) = steam_ready("game", AppState::Installed);
    launch_then_its_press(&mut app, Message::DoubleClick("game".into()));
    focus(&mut app);
    assert!(app.pad_gate_open());
}

#[test]
fn library_presses_become_their_keyboard_twins() {
    let (_home, mut app) = ready();
    tap(&mut app, Raw::DPadRight);
    assert_eq!(app.selected.as_deref(), Some("beta"));
    tap(&mut app, Raw::DPadLeft);
    assert_eq!(app.selected.as_deref(), Some("alpha"));
    // RT is the last game, LT the first.
    tap(&mut app, Raw::RightTrigger);
    assert_eq!(app.selected.as_deref(), Some("gamma"));
    tap(&mut app, Raw::LeftTrigger);
    assert_eq!(app.selected.as_deref(), Some("alpha"));
    // Y favorites, X marks, A opens.
    tap(&mut app, Raw::North);
    assert!(favorite(&app, "alpha"));
    tap(&mut app, Raw::West);
    assert!(app.marked.contains("alpha"));
    tap(&mut app, Raw::South);
    assert!(app.panel_open);
    // View cycles the layout, like Ctrl+L.
    let before = app.layout;
    tap(&mut app, Raw::Select);
    assert_ne!(app.layout, before);
}

#[test]
fn the_stick_moves_like_the_dpad() {
    let (_home, mut app) = ready();
    let task = app.update(Message::Pad(PadEvent::Stick(PadId::Gilrs(0), Axis::X, 0.9)));
    settle(&mut app, task);
    assert_eq!(app.selected.as_deref(), Some("beta"));
    let task = app.update(Message::Pad(PadEvent::Stick(PadId::Gilrs(0), Axis::X, 0.0)));
    settle(&mut app, task);
    assert_eq!(app.selected.as_deref(), Some("beta"));
}

#[test]
fn a_held_direction_repeats_through_its_wakeup() {
    let (_home, mut app) = ready();
    app.pad_clock = Some(std::time::Instant::now());
    let task = app.update(Message::Pad(PadEvent::Pressed(
        PadId::Gilrs(0),
        Raw::DPadRight,
    )));
    settle(&mut app, task);
    assert_eq!(app.selected.as_deref(), Some("beta"));
    let due = app.pads.repeat_due().expect("a repeat is due");
    assert_eq!(app.pad_wake.due(), Some(due));
    // A stale wakeup does nothing.
    let stale = app.pad_wake.generation().wrapping_sub(1);
    settle_update(&mut app, Message::PadRepeat(stale));
    assert_eq!(app.selected.as_deref(), Some("beta"));
    // Just before it's due, a wakeup moves nothing.
    app.pad_clock = Some(due - Duration::from_millis(1));
    let generation = app.pad_wake.generation();
    settle_update(&mut app, Message::PadRepeat(generation));
    assert_eq!(app.selected.as_deref(), Some("beta"));
    app.pad_clock = Some(due);
    let generation = app.pad_wake.generation();
    settle_update(&mut app, Message::PadRepeat(generation));
    assert_eq!(app.selected.as_deref(), Some("gamma"));
    let _ = app.update(Message::Pad(PadEvent::Released(
        PadId::Gilrs(0),
        Raw::DPadRight,
    )));
    assert_eq!(app.pad_wake.due(), None);
}

#[test]
fn a_held_repeat_waits_for_the_last_presss_field_check() {
    let (_home, mut app) = ready();
    let start = std::time::Instant::now();
    app.pad_clock = Some(start);
    let press = app.update(Message::Pad(PadEvent::Pressed(
        PadId::Gilrs(0),
        Raw::DPadRight,
    )));
    assert!(app.pad_checking);
    // The repeat comes due before the check answers: nothing new is asked.
    app.pad_clock = Some(start + crate::pad::REPEAT_AFTER);
    let generation = app.pad_wake.generation();
    let repeat = app.update(Message::PadRepeat(generation));
    assert!(
        !outputs(&app, repeat)
            .iter()
            .any(|message| matches!(message, Message::PadKey(..)))
    );
    // The first press still lands, and the next repeat asks again.
    settle(&mut app, press);
    assert!(!app.pad_checking);
    assert_eq!(app.selected.as_deref(), Some("beta"));
    app.pad_clock = Some(start + crate::pad::REPEAT_AFTER + crate::pad::REPEAT_EVERY);
    let generation = app.pad_wake.generation();
    settle_update(&mut app, Message::PadRepeat(generation));
    assert_eq!(app.selected.as_deref(), Some("gamma"));
}

#[test]
fn a_field_check_answer_meets_the_gate_and_overlays_again() {
    let (_home, mut app) = ready();
    let press = app.update(Message::Pad(PadEvent::Pressed(PadId::Gilrs(0), Raw::South)));
    let answers = outputs(&app, press);
    assert!(matches!(answers.as_slice(), [Message::PadKey(_, _, false)]));
    // Focus left while the check ran.
    let id = app.main_window.unwrap();
    let _ = app.update(Message::Window(id, window::Event::Unfocused));
    for message in answers.clone() {
        let task = app.update(message);
        settle(&mut app, task);
    }
    assert!(!app.panel_open);

    // Or a menu opened.
    focus(&mut app);
    app.sort_menu = true;
    for message in answers {
        let task = app.update(message);
        settle(&mut app, task);
    }
    assert!(!app.panel_open);
    assert!(app.sort_menu);
}

#[test]
fn back_and_start_skip_the_field_check() {
    let (_home, mut app) = ready();
    let _ = app.update(Message::OpenCard("alpha".into()));
    assert!(app.panel_open);
    // B goes straight to Esc, as mouse Back does even with a field
    // focused: no pad check is asked, only the page's own Esc path.
    let back = app.update(Message::Pad(PadEvent::Pressed(PadId::Gilrs(0), Raw::East)));
    assert!(!app.pad_checking);
    let answers = outputs(&app, back);
    assert!(
        !answers
            .iter()
            .any(|message| matches!(message, Message::PadKey(..)))
    );
    for message in answers {
        let task = app.update(message);
        settle(&mut app, task);
    }
    assert!(!app.panel_open);
    let start = app.update(Message::Pad(PadEvent::Pressed(PadId::Gilrs(0), Raw::Start)));
    assert!(!app.pad_checking);
    assert!(
        !outputs(&app, start)
            .iter()
            .any(|message| matches!(message, Message::PadKey(..)))
    );
}

fn settle_update(app: &mut App, message: Message) {
    let task = app.update(message);
    settle(app, task);
}

#[test]
fn a_nintendo_pad_confirms_with_its_printed_a() {
    let (_home, mut app) = ready();
    let _ = app.update(Message::Pad(PadEvent::Connected(
        PadId::Gilrs(1),
        switch_pro(),
    )));
    // The bottom button is B there, which backs out instead.
    tap_on(&mut app, 1, Raw::South);
    assert!(!app.panel_open);
    tap_on(&mut app, 1, Raw::East);
    assert!(app.panel_open);
    assert_eq!(app.pad_hint, Some(crate::pad::Style::Nintendo));
    // The Xbox pad beside it keeps its own labels.
    tap(&mut app, Raw::East);
    assert!(!app.panel_open);
    assert_eq!(app.pad_hint, Some(crate::pad::Style::Xbox));
}

#[test]
fn back_is_the_mouse_back_button() {
    let mouse = crate::event_message(
        iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Back)),
        iced::event::Status::Captured,
    );
    let (
        Some(Message::Key(mouse_key, mouse_modifiers, mouse_status)),
        Message::Key(key, modifiers, status),
    ) = (mouse, back_message())
    else {
        panic!("both are keys");
    };
    assert_eq!(
        (key, modifiers, status),
        (mouse_key, mouse_modifiers, mouse_status)
    );

    // It closes the panel in the Library.
    let (_home, mut app) = ready();
    tap(&mut app, Raw::South);
    assert!(app.panel_open);
    tap(&mut app, Raw::East);
    assert!(!app.panel_open);

    // And goes back out of a flow, which nothing else on the pad reaches.
    let (_home, mut app) = ready();
    let _ = app.update(Message::Add);
    assert!(app.flow.is_some());
    tap(&mut app, Raw::South);
    assert!(app.flow.is_some());
    tap(&mut app, Raw::East);
    assert!(app.flow.is_none());

    // And out of settings, through the page's own Back.
    let (_home, mut app) = ready();
    let _ = app.update(Message::OpenSettings);
    assert!(app.settings.is_some());
    tap(&mut app, Raw::East);
    assert!(app.settings.is_none());
}

#[test]
fn start_toggles_full_screen_anywhere_but_the_close_prompt() {
    assert!(matches!(
        start_message(),
        Message::Key(Key::Named(Named::F11), modifiers, iced::event::Status::Ignored)
            if modifiers.is_empty()
    ));
    use crate::fullscreen::Owner;
    let (_home, mut app) = ready();
    assert_eq!(app.fullscreen, Owner::Nobody);
    tap(&mut app, Raw::Start);
    assert_eq!(app.fullscreen, Owner::User);
    tap(&mut app, Raw::Start);
    assert_eq!(app.fullscreen, Owner::Nobody);
    // It skips the filter, so it works over a page too.
    let _ = app.update(Message::OpenSettings);
    assert!(app.settings.is_some());
    tap(&mut app, Raw::Start);
    assert_eq!(app.fullscreen, Owner::User);
    tap(&mut app, Raw::Start);
    assert_eq!(app.fullscreen, Owner::Nobody);

    let (_home, mut app) = ready();
    app.close_prompt = true;
    tap(&mut app, Raw::Start);
    assert_eq!(app.fullscreen, Owner::Nobody);
    assert!(app.close_prompt);
    // B still answers the prompt, as Esc does.
    tap(&mut app, Raw::East);
    assert!(!app.close_prompt);
}

/// Each overlay, set one at a time, and which of the shared parts it
/// belongs to. A non-B press must do nothing with any of them open, so an
/// overlay added later fails here until it's named in
/// `library_overlay_open`. The parts are `handle_key`'s own guards, so a
/// row in the wrong one would change the keyboard too.
type Open = fn(&mut App);

/// `dialog_open` takes every key; `panel_takes_keys` takes the library's
/// letters and Enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Dialog,
    Panel,
    Other,
}

fn overlays() -> Vec<(&'static str, Part, Open)> {
    vec![
        ("flow", Part::Dialog, |app| {
            let _ = app.update(Message::Add);
        }),
        ("settings", Part::Other, |app| {
            let _ = app.update(Message::OpenSettings);
        }),
        ("art page", Part::Other, |app| {
            app.art_page = Some("alpha".into())
        }),
        ("tool page", Part::Other, |app| {
            app.tool_page = Some(("alpha".into(), crate::tools_page::Kind::Exec));
        }),
        ("portable page", Part::Other, |app| {
            app.portable_page = Some("alpha".into())
        }),
        ("sort menu", Part::Other, |app| app.sort_menu = true),
        ("filter menu", Part::Other, |app| app.filter_menu = true),
        ("random menu", Part::Other, |app| app.random_menu = true),
        ("more menu", Part::Other, |app| app.more_menu = true),
        ("search drop-down", Part::Other, |app| {
            app.search_icon = true;
            app.search_open = true;
        }),
        ("tag menu", Part::Other, |app| {
            app.marked.insert("alpha".into());
            app.tag_menu = true;
        }),
        ("context menu", Part::Panel, |app| {
            app.action_menu = Some("alpha".into())
        }),
        ("version picker", Part::Panel, |app| {
            app.panel_open = true;
            app.version = Some(crate::version_picker::Picker::for_test("alpha".into()));
        }),
        ("panel log", Part::Other, |app| {
            app.panel_open = true;
            app.log_view = true;
        }),
        ("checkup results", Part::Panel, |app| {
            let _ = app.update(Message::Checkup(crate::checkup::Event::Open(
                crate::checkup::Scope::Environment("alpha".into()),
            )));
        }),
        ("checkup under the log", Part::Other, |app| {
            let _ = app.update(Message::Checkup(crate::checkup::Event::Open(
                crate::checkup::Scope::Environment("alpha".into()),
            )));
            app.log_view = true;
        }),
        ("close prompt", Part::Dialog, |app| app.close_prompt = true),
        ("remove dialog", Part::Dialog, |app| {
            let _ = app.update(Message::Remove("alpha".into()));
        }),
        ("hook stop confirm", Part::Dialog, |app| {
            app.hook_stop_confirm = Some("alpha".into());
        }),
    ]
}

#[test]
fn every_overlay_drops_a_non_back_press() {
    // Nothing open: the same presses act, so the table isn't vacuous.
    let (_home, mut app) = ready();
    assert!(!app.library_overlay_open());
    assert!(app.pad_drives_library());
    tap(&mut app, Raw::DPadRight);
    assert_eq!(app.selected.as_deref(), Some("beta"));

    for (name, part, open) in overlays() {
        let (_home, mut app) = ready();
        open(&mut app);
        assert!(app.library_overlay_open(), "{name} isn't open");
        assert_eq!(app.dialog_open(), part == Part::Dialog, "{name}");
        assert_eq!(app.panel_takes_keys(), part == Part::Panel, "{name}");
        assert!(!app.pad_drives_library(), "{name}");
        let selected = app.selected.clone();
        let panel = app.panel_open;
        let marked = app.marked.clone();
        for raw in [
            Raw::DPadRight,
            Raw::South,
            Raw::West,
            Raw::North,
            Raw::RightTrigger,
            Raw::RightBumper,
        ] {
            tap(&mut app, raw);
        }
        assert_eq!(app.selected, selected, "{name}");
        assert_eq!(app.panel_open, panel, "{name}");
        assert_eq!(app.marked, marked, "{name}");
        assert!(!favorite(&app, "alpha"), "{name}");
    }
}

#[test]
fn a_is_dropped_on_pages_and_flows_that_take_enter() {
    // A tool page runs its tool on Enter, and the Add flow's Choose page
    // takes Enter too; A must reach neither.
    let (_home, mut app) = ready();
    app.tool_page = Some(("alpha".into(), crate::tools_page::Kind::Exec));
    let task = app.update(Message::Pad(PadEvent::Pressed(PadId::Gilrs(0), Raw::South)));
    assert!(outputs(&app, task).is_empty());

    let (_home, mut app) = ready();
    let _ = app.update(Message::Add);
    let task = app.update(Message::Pad(PadEvent::Pressed(PadId::Gilrs(0), Raw::South)));
    assert!(outputs(&app, task).is_empty());
}

#[test]
fn a_focused_text_field_takes_no_pad_press() {
    // The field check finds the focused search, and a press with it
    // focused does nothing, even once the key path runs on.
    let (_home, mut app) = ready();
    assert_eq!(text_focus_after(&app, crate::SEARCH_ID), Some(true));
    let task = app.update(Message::PadKey(
        Key::Named(Named::Enter),
        Modifiers::empty(),
        true,
    ));
    settle(&mut app, task);
    assert!(!app.panel_open);
    // The same press without a field focused opens the card.
    let task = app.update(Message::PadKey(
        Key::Named(Named::Enter),
        Modifiers::empty(),
        false,
    ));
    settle(&mut app, task);
    assert!(app.panel_open);

    // A focused button isn't a text field.
    let (_home, app) = ready();
    assert_eq!(
        text_focus_after(&app, crate::filter_control::FILTER_BUTTON_ID),
        Some(false)
    );
}

/// What the text field check says once `id` has focus, run in one laid out
/// tree so the focus is still there.
fn text_focus_after(app: &App, id: &'static str) -> Option<bool> {
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    let focus = iced::widget::operation::focus::<Message>(iced::widget::Id::new(id));
    assert!(drive_in(&mut laid, focus).is_empty());
    let found = drive_in(
        &mut laid,
        iced::advanced::widget::operate(TextFocus::default())
            .map(|focused| Message::PadKey(Key::Named(Named::Enter), Modifiers::empty(), focused)),
    );
    match found.as_slice() {
        [Message::PadKey(_, _, focused)] => Some(*focused),
        _ => None,
    }
}

/// `outputs` in a tree that's already laid out, so focus set by one task
/// is there for the next.
fn drive_in(
    laid: &mut crate::layout_probe::Laid<'_, Message>,
    task: Task<Message>,
) -> Vec<Message> {
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let mut messages = Vec::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    each_ready(&runtime, &mut stream, |action| match action {
        iced_runtime::Action::Output(message) => messages.push(message),
        iced_runtime::Action::Widget(operation) => laid.run(operation),
        _ => {}
    });
    messages
}

#[test]
fn turning_the_controller_off_ignores_presses() {
    let (_home, mut app) = ready();
    app.input.controller = false;
    tap(&mut app, Raw::South);
    assert!(!app.panel_open);
    assert!(app.pad_hint.is_none());
    // The pad list stays current for the settings page.
    let _ = app.update(Message::Pad(PadEvent::Connected(
        PadId::Gilrs(1),
        switch_pro(),
    )));
    assert_eq!(app.pads.names().len(), 2);
}

#[test]
fn the_reader_starts_with_the_switch_and_then_stays() {
    let (_home, mut app) = booted();
    assert!(app.input.controller);
    assert!(app.pad_reading);
    app.input.controller = false;
    app.sync_input();
    assert!(app.pad_reading);

    let (_home, mut app) = booted();
    app.pad_reading = false;
    app.input.controller = false;
    app.sync_input();
    assert!(!app.pad_reading);
    app.input.controller = true;
    app.sync_input();
    assert!(app.pad_reading);
}

#[test]
fn the_hint_bar_follows_the_last_pad_and_hides_on_mouse_or_keys() {
    let (_home, mut app) = ready();
    assert!(app.pad_hint_view().is_none());
    tap(&mut app, Raw::DPadDown);
    assert_eq!(app.pad_hint, Some(crate::pad::Style::Xbox));
    assert!(app.pad_hint_view().is_some());
    assert!(app.bar_room() > 0.0);
    let _ = app.update(Message::PointerMoved);
    assert!(app.pad_hint.is_none());
    assert_eq!(app.bar_room(), 0.0);

    tap(&mut app, Raw::DPadDown);
    assert!(app.pad_hint.is_some());
    let _ = app.update(Message::Pressed);
    assert!(app.pad_hint.is_none());

    // Only over the Library itself.
    tap(&mut app, Raw::DPadDown);
    app.art_page = Some("alpha".into());
    assert!(app.pad_hint.is_some());
    assert!(app.pad_hint_view().is_none());
    app.art_page = None;
    assert!(app.pad_hint_view().is_some());
}

/// Each hint's word, in the bar's order.
fn words(hints: &[crate::pad::glyph::Hint]) -> Vec<&'static str> {
    hints.iter().map(|hint| hint.word).collect()
}

#[test]
fn the_hints_name_the_bound_buttons_in_the_pads_labels() {
    use crate::pad::glyph::hints;
    let input = wineo_core::library::Input::default();
    assert_eq!(
        words(&hints(&input.bindings(), false, false)),
        [
            "Open",
            "Back",
            "Favorite",
            "Mark",
            "Random",
            "Page",
            "Layout",
            "Full screen"
        ]
    );
    // With a game's panel open, Layout and Full screen leave; Page stays.
    assert_eq!(
        words(&hints(&input.bindings(), true, false)),
        ["Play", "Back", "Favorite", "Mark", "Random", "Page"]
    );
    // A section's header or collapsed card has nothing to favorite or mark.
    assert_eq!(
        words(&hints(&input.bindings(), false, true)),
        ["Open", "Back", "Random", "Page", "Layout", "Full screen"]
    );
    use crate::pad::Style;
    use crate::pad::glyph::{Glyph, glyph};
    use wineo_core::library::PadButton;
    // The same printed A looks like its own pad's.
    assert_ne!(
        glyph(Style::Xbox, PadButton::A),
        glyph(Style::PlayStation, PadButton::A)
    );
    assert_ne!(
        glyph(Style::Xbox, PadButton::A),
        glyph(Style::Nintendo, PadButton::A)
    );
    let text = |style, button| match glyph(style, button) {
        Glyph::Label { text, .. } => text,
        Glyph::Face { .. } => "face",
    };
    assert_eq!(text(Style::Nintendo, PadButton::Lt), "ZL");
    assert_eq!(text(Style::PlayStation, PadButton::Rb), "R1");
}

#[test]
fn a_remap_changes_what_a_press_does_and_what_the_hints_say() {
    use wineo_core::library::{PadAction, PadButton};
    let (_home, mut app) = ready();
    // Confirm on X: X opens the card, and A, now Mark's, marks it.
    app.input.buttons.insert(PadAction::Confirm, PadButton::X);
    app.input.buttons.insert(PadAction::Mark, PadButton::A);
    tap(&mut app, Raw::South);
    assert!(app.marked.contains("alpha"));
    assert!(!app.panel_open);
    tap(&mut app, Raw::West);
    assert!(app.panel_open);
    let hints = crate::pad::glyph::hints(&app.input.bindings(), true, false);
    let hint = |button, word| crate::pad::glyph::Hint {
        buttons: vec![button],
        word,
    };
    assert_eq!(hints[0], hint(PadButton::X, "Play"));
    assert!(hints.contains(&hint(PadButton::A, "Mark")));
}

#[test]
fn a_remap_names_the_printed_button_on_a_nintendo_pad_too() {
    use wineo_core::library::{PadAction, PadButton};
    let (_home, mut app) = ready();
    let _ = app.update(Message::Pad(PadEvent::Connected(
        PadId::Gilrs(1),
        switch_pro(),
    )));
    // Confirm on `b` and Back on `a`: the button printed B, which sits at
    // the bottom of a Nintendo pad, now confirms.
    app.input.buttons.insert(PadAction::Confirm, PadButton::B);
    app.input.buttons.insert(PadAction::Back, PadButton::A);
    tap_on(&mut app, 1, Raw::East);
    assert!(!app.panel_open);
    tap_on(&mut app, 1, Raw::South);
    assert!(app.panel_open);
    // And on an Xbox pad, B is to the right.
    tap(&mut app, Raw::South);
    assert!(!app.panel_open);
    tap(&mut app, Raw::East);
    assert!(app.panel_open);
}

/// Coverflow over sections a, b, and c of two games each, with b collapsed,
/// a pad connected and the gate open, and the cursor on a-0 with its panel
/// shut.
fn coverflow_ready() -> (tempfile::TempDir, App) {
    let (home, mut app) = booted();
    app.group = wineo_core::library::GroupBy::Tag;
    let cards = ["a-0", "a-1", "b-0", "b-1", "c-0", "c-1"]
        .into_iter()
        .map(|name| {
            let mut card = card(name, State::Inactive, 60);
            card.tags = vec![name[..1].to_owned()];
            card
        })
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    let _ = app.switch_layout(crate::grid_motion::ViewLayout::Coverflow);
    let _ = app.update(Message::Strip(crate::coverflow::Event::Measured(
        iced::Size::new(1600.0, 900.0),
    )));
    let _ = app.update(Message::ToggleSection(crate::slot::SectionKey::Tag(
        "b".into(),
    )));
    let _ = app.update(Message::Pad(PadEvent::Connected(PadId::Gilrs(0), xbox())));
    focus(&mut app);
    move_to(&mut app, "a-0");
    (home, app)
}

#[test]
fn in_coverflow_the_dpad_walks_the_strip_and_a_opens_a_section_card() {
    let (_home, mut app) = coverflow_ready();
    assert_eq!(app.strip.target(), 0);
    tap(&mut app, Raw::DPadRight);
    assert_eq!(app.selected.as_deref(), Some("a-1"));
    assert_eq!(app.strip.target(), 1);
    tap(&mut app, Raw::DPadRight);
    assert_eq!(app.strip.target(), 2);
    assert!(app.header_cursor().is_some(), "on b's card");
    tap(&mut app, Raw::South);
    assert!(
        !app.collapsed
            .contains(&crate::slot::SectionKey::Tag("b".into()))
    );
    assert_eq!(app.selected.as_deref(), Some("b-0"));
    assert_eq!(app.strip.target(), 2);
    assert!(app.header_cursor().is_none());
}

#[test]
fn on_a_coverflow_card_the_hints_drop_favorite_and_mark_and_a_opens() {
    let (_home, mut app) = coverflow_ready();
    // The panel is open on a-1, then the card hides it.
    tap(&mut app, Raw::DPadRight);
    tap(&mut app, Raw::South);
    assert!(app.panel_open);
    let words = |app: &App| words(&app.pad_hints());
    assert!(words(&app).contains(&"Play"));
    assert!(words(&app).contains(&"Favorite"));
    tap(&mut app, Raw::DPadRight);
    assert!(app.panel_hidden_by_card());
    let on_card = words(&app);
    assert!(on_card.contains(&"Open"), "{on_card:?}");
    assert!(!on_card.contains(&"Play"), "{on_card:?}");
    assert!(!on_card.contains(&"Favorite") && !on_card.contains(&"Mark"));
    assert!(app.pad_hint_view().is_some(), "the bar shows in coverflow");
}

#[test]
fn a_remap_moves_the_coverflow_strip_and_opens_cards() {
    use wineo_core::library::{PadAction, PadButton};
    let (_home, mut app) = coverflow_ready();
    // Confirm on X, Last on A, and Mark, whose X that was, on RT.
    app.input.buttons.insert(PadAction::Confirm, PadButton::X);
    app.input.buttons.insert(PadAction::Last, PadButton::A);
    app.input.buttons.insert(PadAction::Mark, PadButton::Rt);
    // The collapsed section's card opens with the remapped confirm.
    tap(&mut app, Raw::DPadRight);
    tap(&mut app, Raw::DPadRight);
    assert!(app.header_cursor().is_some(), "on b's card");
    tap(&mut app, Raw::West);
    assert_eq!(app.selected.as_deref(), Some("b-0"));
    assert!(app.header_cursor().is_none());
    // A now jumps the strip to its end, and X opens the game there.
    tap(&mut app, Raw::South);
    assert_eq!(app.selected.as_deref(), Some("c-1"));
    assert_eq!(app.strip.target(), 5);
    assert!(!app.panel_open);
    tap(&mut app, Raw::West);
    assert!(app.panel_open);
}

#[test]
fn a_remapped_page_button_moves_the_strip_a_screenful() {
    use wineo_core::library::{PadAction, PadButton};
    // What PageDown does from the first cover.
    let (_home, mut keyboard) = coverflow_ready();
    let _ = keyboard.update(Message::KeyWithFocus(
        Key::Named(Named::PageDown),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
    let screenful = keyboard.strip.target();
    assert!(screenful > 1, "a screenful is more than one cover");

    // Next screenful on A, with confirm taking A's place on RB.
    let (_home, mut app) = coverflow_ready();
    app.input.buttons.insert(PadAction::PageRight, PadButton::A);
    app.input.buttons.insert(PadAction::Confirm, PadButton::Rb);
    tap(&mut app, Raw::South);
    assert_eq!(app.strip.target(), screenful);
    assert_eq!(app.selected, keyboard.selected);
    assert!(!app.panel_open);
    // RB, the old page button, now opens the game in the middle.
    assert!(app.header_cursor().is_none(), "a game is in the middle");
    tap(&mut app, Raw::RightBumper);
    assert!(app.panel_open);
    assert_eq!(app.strip.target(), screenful);
}

// The GameCube adapter's hold, asked of plain functions, since an iced
// subscription can't be looked into.

use crate::pad::adapter::{self, Report};

/// `ready` with both switches saved on.
fn adapter_ready() -> (tempfile::TempDir, App) {
    let (home, mut app) = ready();
    app.input.gamecube_adapter = true;
    let _ = app.update(Message::PointerMoved);
    (home, app)
}

/// What a new reader would start from.
fn told(app: &App) -> bool {
    *app.adapter.link().wanted.borrow()
}

fn adapter_pad(app: &mut App, event: PadEvent) -> Task<Message> {
    app.update(Message::Adapter(Report::Pad(event)))
}

const PORT1: PadId = PadId::Adapter(0);

#[test]
fn the_adapter_is_wanted_with_both_switches_on_and_the_gate_open() {
    let (_home, app) = ready();
    assert!(app.pad_gate_open());
    // Off by default: no reader, so nothing watches USB.
    assert!(!app.adapter_subscribed());
    assert!(!app.adapter_wanted() && !told(&app));

    let (_home, mut app) = adapter_ready();
    assert!(app.adapter_subscribed());
    assert!(app.adapter_wanted() && told(&app));

    app.input.controller = false;
    let _ = app.update(Message::PointerMoved);
    assert!(!app.adapter_subscribed());
    assert!(!told(&app));
    app.input.controller = true;
    let _ = app.update(Message::PointerMoved);
    assert!(told(&app));
}

#[test]
fn closing_exiting_a_closed_window_focus_and_starts_let_go() {
    for case in ["closing", "exiting", "window", "unfocused", "play", "tool"] {
        let (home, mut app) = adapter_ready();
        assert!(told(&app), "{case}");
        match case {
            // Either alone ends the reader, before any update reacts.
            "closing" => {
                app.closing = true;
                assert!(!app.adapter_subscribed() && !app.adapter_wanted());
            }
            "exiting" => {
                app.exiting = true;
                assert!(!app.adapter_subscribed() && !app.adapter_wanted());
            }
            "window" => app.main_window = None,
            "unfocused" => {
                let id = app.main_window.unwrap();
                let _ = app.update(Message::Window(id, window::Event::Unfocused));
            }
            "play" => {
                let _ = app.update(Message::Play);
                assert!(app.operation.is_some());
            }
            _ => {
                use crate::tools_page::{Event, Kind};
                for card in &mut app.cards {
                    card.prefix = Some(home.path().join("prefix"));
                }
                let _ = app.update(Message::OpenTool("alpha".into(), Kind::Winetricks));
                let _ = app.update(Message::Tool(
                    "alpha".into(),
                    Kind::Winetricks,
                    Event::OpenWinetricks,
                ));
            }
        }
        let _ = app.update(Message::PointerMoved);
        assert!(!app.adapter_wanted(), "{case}");
        assert!(!told(&app), "{case}");
        // Closing or exiting drops the reader too; the rest keep it.
        let subscribed = !matches!(case, "closing" | "exiting");
        assert_eq!(app.adapter_subscribed(), subscribed, "{case}");
    }
}

#[test]
fn a_start_while_the_adapter_is_held_waits_on_its_state() {
    let (_home, mut app) = adapter_ready();
    // Not held: the start has nothing to wait on.
    let task = app.update(Message::Play);
    assert!(app.operation.is_some());
    assert_eq!(app.adapter.state.receiver_count(), 0);
    drop(task);

    let (_home, mut app) = adapter_ready();
    let _ = app.update(Message::Adapter(Report::State(adapter::State::Held)));
    app.adapter.state.send_replace(adapter::State::Held);
    assert_eq!(app.adapter.state.receiver_count(), 0);
    // The start's task holds the state, never run here, so nothing launches.
    let task = app.update(Message::Play);
    assert!(app.operation.is_some());
    assert_eq!(app.adapter.state.receiver_count(), 1);
    drop(task);
    assert_eq!(app.adapter.state.receiver_count(), 0);
}

#[test]
fn the_first_focus_before_the_reader_exists_still_takes() {
    let (_home, mut app) = closed_app();
    app.input.gamecube_adapter = true;
    let _ = app.update(Message::PointerMoved);
    assert!(app.adapter_subscribed() && !told(&app));
    focus(&mut app);
    // A reader made after the focus starts wanted.
    assert!(told(&app));
}

#[test]
fn the_reader_is_told_only_when_the_answer_changes() {
    let (_home, mut app) = adapter_ready();
    let mut reader = app.adapter.wanted.subscribe();
    assert!(*reader.borrow_and_update());
    for _ in 0..3 {
        let _ = app.update(Message::PointerMoved);
        tap(&mut app, Raw::DPadDown);
    }
    assert!(!reader.has_changed().unwrap());
    let id = app.main_window.unwrap();
    let _ = app.update(Message::Window(id, window::Event::Unfocused));
    assert!(reader.has_changed().unwrap());
    assert!(!*reader.borrow_and_update());
    let _ = app.update(Message::PointerMoved);
    assert!(!reader.has_changed().unwrap());
}

#[test]
fn the_adapters_pads_go_with_its_reader_and_late_reports_are_dropped() {
    let (_home, mut app) = adapter_ready();
    let _ = app.update(Message::Adapter(Report::State(adapter::State::Held)));
    let _ = adapter_pad(&mut app, PadEvent::Connected(PORT1, adapter::port_info(0)));
    assert_eq!(
        app.pads.names(),
        ["Xbox Wireless Controller", "GameCube controller (port 1)"]
    );
    assert_eq!(app.adapter.shown, adapter::State::Held);
    app.adapter.state.send_replace(adapter::State::Held);

    // The switch off ends the subscription, which can't say so itself.
    app.input.gamecube_adapter = false;
    let _ = app.update(Message::PointerMoved);
    assert_eq!(app.pads.names(), ["Xbox Wireless Controller"]);
    assert_eq!(app.adapter.shown, adapter::State::Absent);
    assert_eq!(*app.adapter.state.borrow(), adapter::State::Absent);
    assert!(app.adapter.held().is_none());
    // What the dropped reader sent before it went is stale.
    let _ = adapter_pad(&mut app, PadEvent::Connected(PORT1, adapter::port_info(0)));
    let _ = app.update(Message::Adapter(Report::State(adapter::State::Busy)));
    assert_eq!(app.pads.names(), ["Xbox Wireless Controller"]);
    assert_eq!(app.adapter.shown, adapter::State::Absent);
    // Even a press, which would act before anything else could drop it.
    let task = adapter_pad(&mut app, PadEvent::Pressed(PORT1, Raw::South));
    settle(&mut app, task);
    assert!(!app.panel_open);
}

#[test]
fn a_gamecube_a_confirms_and_l_r_z_and_the_c_stick_follow_the_map() {
    use wineo_core::library::{PadAction, PadButton};
    let (_home, mut app) = adapter_ready();
    let _ = adapter_pad(&mut app, PadEvent::Connected(PORT1, adapter::port_info(0)));
    let task = adapter_pad(&mut app, PadEvent::Pressed(PORT1, Raw::South));
    settle(&mut app, task);
    assert!(app.panel_open);
    assert_eq!(app.pad_hint, Some(crate::pad::Style::GameCube));

    let action = |app: &App, raw| {
        let button = crate::pad::label(crate::pad::Style::GameCube, raw).unwrap();
        app.input.bindings().action(button)
    };
    assert_eq!(action(&app, Raw::LeftBumper), Some(PadAction::PageLeft));
    assert_eq!(action(&app, Raw::RightBumper), Some(PadAction::PageRight));
    assert_eq!(action(&app, Raw::Select), Some(PadAction::Layout));
    assert_eq!(action(&app, Raw::LeftTrigger), Some(PadAction::First));
    assert_eq!(action(&app, Raw::RightTrigger), Some(PadAction::Last));
    assert_eq!(action(&app, Raw::Start), Some(PadAction::Fullscreen));
    // A remap moves them like any button.
    app.input.buttons.insert(PadAction::First, PadButton::Ls);
    app.input.buttons.insert(PadAction::Random, PadButton::Lt);
    assert_eq!(action(&app, Raw::LeftTrigger), Some(PadAction::Random));
    app.input
        .buttons
        .insert(PadAction::Favorite, PadButton::View);
    let _ = app.update(Message::ClosePanel);
    let task = adapter_pad(&mut app, PadEvent::Pressed(PORT1, Raw::Select));
    settle(&mut app, task);
    assert!(favorite(&app, "alpha"));
}

#[test]
fn the_hint_bar_leaves_out_what_a_gamecube_pad_lacks() {
    use wineo_core::library::{PadAction, PadButton};
    let (_home, mut app) = adapter_ready();
    let words = |app: &App| words(&app.pad_hints());
    app.pad_hint = Some(crate::pad::Style::Xbox);
    assert!(words(&app).contains(&"Random"));
    app.pad_hint = Some(crate::pad::Style::GameCube);
    assert_eq!(
        words(&app),
        [
            "Open",
            "Back",
            "Favorite",
            "Mark",
            "Page",
            "Layout",
            "Full screen"
        ]
    );
    // Back once it's on a button the pad has, the C-stick included.
    app.input.buttons.insert(PadAction::Layout, PadButton::Ls);
    app.input.buttons.insert(PadAction::Random, PadButton::View);
    assert!(words(&app).contains(&"Random"));
    app.input.buttons.clear();
    app.input.buttons.insert(PadAction::Last, PadButton::Ls);
    app.input.buttons.insert(PadAction::Random, PadButton::Rt);
    assert!(words(&app).contains(&"Random"));
    app.input.buttons.clear();
    app.input.buttons.insert(PadAction::First, PadButton::Ls);
    app.input.buttons.insert(PadAction::Random, PadButton::Lt);
    assert!(words(&app).contains(&"Random"));
    app.input.buttons.insert(PadAction::Confirm, PadButton::Rs);
    assert!(!words(&app).contains(&"Open"));
}
