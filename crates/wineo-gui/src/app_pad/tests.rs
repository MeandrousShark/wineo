//! The pad in the app, all on made-up events: the gate, the filter, and
//! what each press stands for.

use std::time::Duration;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::window;
use wineo_core::runtime::State;

use super::*;
use crate::pad::{Axis, PadEvent, PadInfo, Raw};
use crate::tests::card;

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

/// A library of three games with an Xbox pad connected and the gate shut,
/// as at launch.
fn closed_app() -> App {
    let mut app = crate::tests::app();
    let cards = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(|name| card(name, State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    app.viewport_width = 1000.0;
    app.snap_grid();
    let _ = app.update(Message::Pad(PadEvent::Connected(0, xbox())));
    app
}

/// `closed_app` after the window's first real focus, with the cursor on
/// the first game and its panel shut.
fn ready() -> App {
    let mut app = closed_app();
    focus(&mut app);
    move_to(&mut app, "alpha");
    app
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
/// view. Anything that waits, like a wakeup, is cut off.
fn outputs(app: &App, task: Task<Message>) -> Vec<Message> {
    use iced::futures::StreamExt;
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
    runtime.block_on(async {
        while let Ok(Some(action)) =
            tokio::time::timeout(Duration::from_millis(20), stream.next()).await
        {
            match action {
                iced_runtime::Action::Output(message) => messages.push(message),
                iced_runtime::Action::Widget(operation) => laid
                    .get_or_insert_with(|| {
                        crate::layout_probe::lay_out(app.view(), app.window_size)
                    })
                    .run(operation),
                _ => {}
            }
        }
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
    let mut app = closed_app();
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
    let mut app = closed_app();
    let _ = app.update(Message::Pressed);
    move_to(&mut app, "beta");
    tap(&mut app, Raw::South);
    assert!(app.panel_open);
}

#[test]
fn losing_focus_hiding_and_closing_shut_the_gate() {
    let mut app = ready();
    let id = app.main_window.unwrap();
    let _ = app.update(Message::Window(id, window::Event::Unfocused));
    assert!(!app.pad_gate_open());
    tap(&mut app, Raw::South);
    assert!(!app.panel_open);

    // The tray's reopen says focused without a Focused event, which a
    // compositor that refused the raise never sends.
    let mut app = ready();
    let _ = app.hide_window();
    assert!(!app.pad_gate);
    let _ = app.open_window();
    assert!(app.focused);
    assert!(app.main_window.is_some());
    assert!(!app.pad_gate_open());
    tap(&mut app, Raw::South);
    assert!(!app.panel_open);

    let mut app = ready();
    app.closing = true;
    assert!(!app.pad_gate_open());
    tap(&mut app, Raw::South);
    assert!(!app.panel_open);
}

#[test]
fn a_launch_or_a_steam_request_shuts_the_gate() {
    use wineo_core::steam::AppState;
    let mut app = ready();
    app.merge(vec![crate::tests::steam_card("game", AppState::Installed)]);
    let _ = app.update(Message::Select("game".into()));
    let _ = app.update(Message::Play);
    assert!(app.operation.is_some());
    assert!(!app.pad_gate_open());
    // Only a real focus opens it again.
    focus(&mut app);
    assert!(app.pad_gate_open());

    let mut app = ready();
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
fn library_presses_become_their_keyboard_twins() {
    let mut app = ready();
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
    let mut app = ready();
    let task = app.update(Message::Pad(PadEvent::Stick(0, Axis::X, 0.9)));
    settle(&mut app, task);
    assert_eq!(app.selected.as_deref(), Some("beta"));
    let task = app.update(Message::Pad(PadEvent::Stick(0, Axis::X, 0.0)));
    settle(&mut app, task);
    assert_eq!(app.selected.as_deref(), Some("beta"));
}

#[test]
fn a_held_direction_repeats_through_its_wakeup() {
    let mut app = ready();
    let task = app.update(Message::Pad(PadEvent::Pressed(0, Raw::DPadRight)));
    settle(&mut app, task);
    assert_eq!(app.selected.as_deref(), Some("beta"));
    let due = app.pads.repeat_due().expect("a repeat is due");
    assert_eq!(app.pad_wake.due(), Some(due));
    // A stale wakeup does nothing.
    let stale = app.pad_wake.generation().wrapping_sub(1);
    settle_update(&mut app, Message::PadRepeat(stale));
    assert_eq!(app.selected.as_deref(), Some("beta"));
    std::thread::sleep(due.saturating_duration_since(std::time::Instant::now()));
    let generation = app.pad_wake.generation();
    settle_update(&mut app, Message::PadRepeat(generation));
    assert_eq!(app.selected.as_deref(), Some("gamma"));
    let _ = app.update(Message::Pad(PadEvent::Released(0, Raw::DPadRight)));
    assert_eq!(app.pad_wake.due(), None);
}

fn settle_update(app: &mut App, message: Message) {
    let task = app.update(message);
    settle(app, task);
}

#[test]
fn a_nintendo_pad_confirms_with_its_printed_a() {
    let mut app = ready();
    let _ = app.update(Message::Pad(PadEvent::Connected(1, switch_pro())));
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
    let mut app = ready();
    tap(&mut app, Raw::South);
    assert!(app.panel_open);
    tap(&mut app, Raw::East);
    assert!(!app.panel_open);

    // And goes back out of a flow, which nothing else on the pad reaches.
    let mut app = ready();
    let _ = app.update(Message::Add);
    assert!(app.flow.is_some());
    tap(&mut app, Raw::South);
    assert!(app.flow.is_some());
    tap(&mut app, Raw::East);
    assert!(app.flow.is_none());

    // And out of settings, through the page's own Back.
    let mut app = ready();
    let _ = app.update(Message::OpenSettings);
    assert!(app.settings.is_some());
    tap(&mut app, Raw::East);
    assert!(app.settings.is_none());
}

#[test]
fn start_sends_full_screens_key_but_not_in_the_close_prompt() {
    assert!(matches!(
        start_message(),
        Message::Key(Key::Named(Named::F11), modifiers, iced::event::Status::Ignored)
            if modifiers.is_empty()
    ));
    let mut app = ready();
    app.close_prompt = true;
    tap(&mut app, Raw::Start);
    assert!(app.close_prompt);
    // B still answers the prompt, as Esc does.
    tap(&mut app, Raw::East);
    assert!(!app.close_prompt);
}

/// Each overlay, set one at a time. A non-B press must do nothing with any
/// of them open, so an overlay added later fails here until it's named in
/// `library_overlay_open`.
type Open = fn(&mut App);

fn overlays() -> Vec<(&'static str, Open)> {
    vec![
        ("flow", |app| {
            let _ = app.update(Message::Add);
        }),
        ("settings", |app| {
            let _ = app.update(Message::OpenSettings);
        }),
        ("art page", |app| app.art_page = Some("alpha".into())),
        ("tool page", |app| {
            app.tool_page = Some(("alpha".into(), crate::tools_page::Kind::Exec));
        }),
        ("portable page", |app| {
            app.portable_page = Some("alpha".into())
        }),
        ("sort menu", |app| app.sort_menu = true),
        ("filter menu", |app| app.filter_menu = true),
        ("random menu", |app| app.random_menu = true),
        ("more menu", |app| app.more_menu = true),
        ("search drop-down", |app| {
            app.search_icon = true;
            app.search_open = true;
        }),
        ("tag menu", |app| {
            app.marked.insert("alpha".into());
            app.tag_menu = true;
        }),
        ("context menu", |app| app.action_menu = Some("alpha".into())),
        ("version picker", |app| {
            app.panel_open = true;
            app.version = Some(crate::version_picker::Picker::for_test("alpha".into()));
        }),
        ("panel log", |app| {
            app.panel_open = true;
            app.log_view = true;
        }),
        ("checkup results", |app| {
            let _ = app.update(Message::Checkup(crate::checkup::Event::Open(
                crate::checkup::Scope::Environment("alpha".into()),
            )));
        }),
        ("close prompt", |app| app.close_prompt = true),
        ("remove dialog", |app| {
            let _ = app.update(Message::Remove("alpha".into()));
        }),
        ("hook stop confirm", |app| {
            app.hook_stop_confirm = Some("alpha".into());
        }),
    ]
}

#[test]
fn every_overlay_drops_a_non_back_press() {
    // Nothing open: the same presses act, so the table isn't vacuous.
    let mut app = ready();
    assert!(!app.library_overlay_open());
    assert!(app.pad_drives_library());
    tap(&mut app, Raw::DPadRight);
    assert_eq!(app.selected.as_deref(), Some("beta"));

    for (name, open) in overlays() {
        let mut app = ready();
        open(&mut app);
        assert!(app.library_overlay_open(), "{name} isn't open");
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
    let mut app = ready();
    app.tool_page = Some(("alpha".into(), crate::tools_page::Kind::Exec));
    let task = app.update(Message::Pad(PadEvent::Pressed(0, Raw::South)));
    assert!(outputs(&app, task).is_empty());

    let mut app = ready();
    let _ = app.update(Message::Add);
    let task = app.update(Message::Pad(PadEvent::Pressed(0, Raw::South)));
    assert!(outputs(&app, task).is_empty());
}

#[test]
fn a_focused_text_field_takes_no_pad_press() {
    // The field check finds the focused search, and a press with it
    // focused does nothing, even once the key path runs on.
    let mut app = ready();
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
    let app = ready();
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
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let mut messages = Vec::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        while let Ok(Some(action)) =
            tokio::time::timeout(Duration::from_millis(20), stream.next()).await
        {
            match action {
                iced_runtime::Action::Output(message) => messages.push(message),
                iced_runtime::Action::Widget(operation) => laid.run(operation),
                _ => {}
            }
        }
    });
    messages
}

#[test]
fn turning_the_controller_off_ignores_presses() {
    let mut app = ready();
    app.input.controller = false;
    tap(&mut app, Raw::South);
    assert!(!app.panel_open);
    assert!(app.pad_hint.is_none());
    // The pad list stays current for the settings page.
    let _ = app.update(Message::Pad(PadEvent::Connected(1, switch_pro())));
    assert_eq!(app.pads.names().len(), 2);
}

#[test]
fn the_reader_starts_with_the_switch_and_then_stays() {
    let mut app = crate::tests::app();
    assert!(app.input.controller);
    assert!(app.pad_reading);
    app.input.controller = false;
    app.sync_input();
    assert!(app.pad_reading);

    let mut app = crate::tests::app();
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
    let mut app = ready();
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

#[test]
fn the_hints_name_the_bound_buttons_in_the_pads_labels() {
    let input = wineo_core::library::Input::default();
    let hints = crate::pad::glyph::hints(&input.bindings(), false);
    let words: Vec<_> = hints.iter().map(|(_, word)| *word).collect();
    assert_eq!(words, ["Open", "Back", "Favorite", "Mark", "Random"]);
    assert_eq!(
        crate::pad::glyph::hints(&input.bindings(), true)[0].1,
        "Play"
    );
    use crate::pad::Style;
    use crate::pad::glyph::{Glyph, glyph};
    use wineo_core::library::PadButton;
    // The same printed A looks like its own pad's.
    assert_ne!(
        glyph(Style::Xbox, PadButton::A),
        glyph(Style::PlayStation, PadButton::A)
    );
    assert_eq!(
        glyph(Style::Xbox, PadButton::A),
        glyph(Style::Nintendo, PadButton::A)
    );
    assert_eq!(glyph(Style::Nintendo, PadButton::Lt), Glyph::Label("ZL"));
    assert_eq!(glyph(Style::PlayStation, PadButton::Rb), Glyph::Label("R1"));
}
