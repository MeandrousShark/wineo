//! The Controller section's remap rows: picking a button for an action,
//! swaps, Reset, the capture's cancels, and hand-edited files.

use std::time::{Duration, Instant};

use wineo_core::library::{PadAction, PadButton};

use super::*;

fn save(paths: &Paths, page: &Page) -> load::Report {
    let form = &global_editor(page).form;
    load::save_global_page(paths, &form.edits(), form.library_edits(), None).unwrap()
}

fn form(page: &mut Page) -> &mut global::Form {
    match &mut page.view {
        View::Global(editor) => &mut editor.form,
        _ => panic!("global form expected"),
    }
}

fn button(page: &Page, action: PadAction) -> Option<PadButton> {
    global_editor(page)
        .form
        .draft
        .input
        .bindings()
        .button(action)
}

fn write_library(paths: &Paths, text: &str) {
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), text).unwrap();
}

fn capture(page: &mut Page, action: PadAction) {
    page.toggle_capture(action, Instant::now() + CAPTURE_FOR);
    assert_eq!(page.capturing(), Some(action));
}

#[test]
fn a_free_button_moves_the_action_and_saving_writes_only_that() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    capture(&mut page, PadAction::Favorite);
    assert!(page.take_capture(PadButton::Rs));
    assert_eq!(page.capturing(), None);
    assert_eq!(button(&page, PadAction::Favorite), Some(PadButton::Rs));
    assert_eq!(button(&page, PadAction::Confirm), Some(PadButton::A));
    assert_eq!(page.changes(), 1);
    assert!(save(&paths, &page).failed.is_none());
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        "schema_version = 1\n\n[input.buttons]\nfavorite = \"rs\"\n"
    );
    // No capture waiting, a press changes nothing.
    let mut page = global_page(&paths);
    assert!(!page.take_capture(PadButton::A));
    assert!(!page.is_dirty());
}

#[test]
fn a_button_in_use_swaps_the_two_actions() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    form(&mut page).assign(PadAction::Favorite, PadButton::A);
    assert_eq!(button(&page, PadAction::Favorite), Some(PadButton::A));
    assert_eq!(button(&page, PadAction::Confirm), Some(PadButton::Y));
    // Every other action keeps its own.
    for action in PadAction::ALL {
        if !matches!(action, PadAction::Favorite | PadAction::Confirm) {
            assert_eq!(button(&page, action), Some(action.default_button()));
        }
    }
    // Its own button again changes nothing; swapping back is no change.
    form(&mut page).assign(PadAction::Favorite, PadButton::A);
    assert_eq!(button(&page, PadAction::Favorite), Some(PadButton::A));
    form(&mut page).assign(PadAction::Favorite, PadButton::Y);
    assert!(global_editor(&page).form.draft.input.buttons.is_empty());
    assert!(!page.is_dirty());
}

#[test]
fn back_and_any_other_button_can_be_picked_for_an_action() {
    // The capture takes whatever is pressed, B included.
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    capture(&mut page, PadAction::Confirm);
    assert!(page.take_capture(PadButton::B));
    assert_eq!(button(&page, PadAction::Confirm), Some(PadButton::B));
    assert_eq!(button(&page, PadAction::Back), Some(PadButton::A));
}

#[test]
fn reset_puts_every_action_back_and_saving_drops_the_table() {
    let (_root, paths) = setup();
    write_library(
        &paths,
        "schema_version = 1\n\n[input.buttons]\nfavorite = \"rs\"\nconfirm = \"x\"\n",
    );
    let mut page = global_page(&paths);
    assert_eq!(button(&page, PadAction::Favorite), Some(PadButton::Rs));
    let _ = page.update(&paths, Event::ResetButtons);
    for action in PadAction::ALL {
        assert_eq!(button(&page, action), Some(action.default_button()));
    }
    assert_eq!(page.changes(), 1);
    assert!(save(&paths, &page).failed.is_none());
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        "schema_version = 1\n"
    );
}

#[test]
fn a_hand_edited_duplicate_shows_unbound_and_a_pick_fixes_it() {
    let (_root, paths) = setup();
    // Favorite asks for Y, which confirm took first, and its own default
    // is Y too, so it's left without one.
    write_library(
        &paths,
        "schema_version = 1\n\n[input.buttons]\nconfirm = \"y\"\nfavorite = \"y\"\n",
    );
    let mut page = global_page(&paths);
    assert_eq!(button(&page, PadAction::Confirm), Some(PadButton::Y));
    assert_eq!(button(&page, PadAction::Favorite), None);
    let texts = page_texts(&page, &paths);
    assert_eq!(
        texts.iter().filter(|text| *text == view::UNBOUND).count(),
        1,
        "{texts:?}"
    );
    // Picking B for it moves Back, which takes its own default if free,
    // else the first free button: A, which confirm left.
    form(&mut page).assign(PadAction::Favorite, PadButton::B);
    let bindings = global_editor(&page).form.draft.input.bindings();
    assert_eq!(bindings.button(PadAction::Favorite), Some(PadButton::B));
    assert_eq!(bindings.button(PadAction::Back), Some(PadButton::A));
    assert_eq!(bindings.button(PadAction::Confirm), Some(PadButton::Y));
    assert!(
        PadAction::ALL
            .iter()
            .all(|action| bindings.button(*action).is_some())
    );
}

#[test]
fn unknown_actions_and_buttons_read_as_unset() {
    let (_root, paths) = setup();
    write_library(
        &paths,
        "schema_version = 1\n\n[input.buttons]\nteleport = \"a\"\nfavorite = \"zz\"\nmark = \"rs\"\n",
    );
    let page = global_page(&paths);
    assert_eq!(button(&page, PadAction::Favorite), Some(PadButton::Y));
    assert_eq!(button(&page, PadAction::Mark), Some(PadButton::Rs));
    assert!(
        !page_texts(&page, &paths)
            .iter()
            .any(|text| text == view::UNBOUND)
    );
    assert!(!page.is_dirty());
}

#[test]
fn the_rows_show_each_action_its_button_and_the_waiting_one() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let texts = page_texts(&page, &paths);
    for action in PadAction::ALL {
        assert!(
            texts.iter().any(|text| text == view::action_label(action)),
            "{action:?}"
        );
    }
    // Labels that are text, in Xbox terms until a pad says otherwise.
    for label in ["LB", "RB", "LT", "RT", "View", "Menu", "LS"] {
        assert!(texts.iter().any(|text| text == label), "{label}");
    }
    assert!(texts.iter().any(|text| text == view::RESET_BUTTONS));
    assert!(!texts.iter().any(|text| text == view::CAPTURE_PROMPT));
    page.pad_style = crate::pad::Style::PlayStation;
    let texts = page_texts(&page, &paths);
    assert!(texts.iter().any(|text| text == "L1"));
    capture(&mut page, PadAction::PageLeft);
    let texts = page_texts(&page, &paths);
    assert!(texts.iter().any(|text| text == view::CAPTURE_PROMPT));
    assert!(!texts.iter().any(|text| text == "L1"));
}

#[test]
fn the_same_row_again_stops_waiting_and_a_locked_page_never_waits() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    capture(&mut page, PadAction::Mark);
    page.toggle_capture(PadAction::Mark, Instant::now() + Duration::from_secs(5));
    assert_eq!(page.capturing(), None);
    // Another row moves the wait there.
    capture(&mut page, PadAction::Mark);
    capture(&mut page, PadAction::Random);

    let mut page = global_page(&paths);
    page.saving = true;
    page.toggle_capture(PadAction::Mark, Instant::now() + CAPTURE_FOR);
    assert_eq!(page.capturing(), None);

    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::Tab(Tab::Appearance));
    page.toggle_capture(PadAction::Mark, Instant::now() + CAPTURE_FOR);
    assert_eq!(page.capturing(), None);
}

#[test]
fn anything_the_user_does_on_the_page_ends_the_wait() {
    let (_root, paths) = setup();
    let cancels = [
        Event::Tab(Tab::Appearance),
        Event::Gog(gog::Event::LoginOpen),
        Event::Gog(gog::Event::Login),
        Event::Gog(gog::Event::CopyLink),
        Event::Gog(gog::Event::Logout),
        Event::Save,
        Event::Revert,
        Event::Reload,
        Event::GlobalController(false),
        Event::ResetButtons,
        Event::MenuOpened,
    ];
    for event in cancels {
        let mut page = global_page(&paths);
        capture(&mut page, PadAction::Favorite);
        let name = format!("{event:?}");
        let _ = page.update(&paths, event);
        assert_eq!(page.capturing(), None, "{name}");
    }
    // A background answer leaves it waiting.
    let mut page = global_page(&paths);
    capture(&mut page, PadAction::Favorite);
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, load::Texts::read_global(&paths)),
    );
    let _ = page.update(&paths, Event::Scrolled(Tab::General, 40.0));
    // A GOG answer that came back on its own, too.
    let _ = page.update(
        &paths,
        Event::Gog(gog::Event::StatusLoaded(1, 0, Ok(gog::Status::LoggedOut))),
    );
    assert_eq!(page.capturing(), Some(PadAction::Favorite));
}

#[test]
fn the_holder_of_a_picked_button_takes_its_own_default_when_free() {
    let (_root, paths) = setup();
    // Favorite is left unbound, and Mark sits on RS, off its default X.
    write_library(
        &paths,
        "schema_version = 1\n\n[input.buttons]\nconfirm = \"y\"\nfavorite = \"y\"\nmark = \"rs\"\n",
    );
    let mut page = global_page(&paths);
    assert_eq!(button(&page, PadAction::Favorite), None);
    form(&mut page).assign(PadAction::Favorite, PadButton::Rs);
    assert_eq!(button(&page, PadAction::Favorite), Some(PadButton::Rs));
    assert_eq!(button(&page, PadAction::Mark), Some(PadButton::X));
}

#[test]
fn a_hand_written_default_is_no_change_after_a_swap_back_or_reset() {
    let (_root, paths) = setup();
    // Favorite on Y is its own default, written out by hand.
    write_library(
        &paths,
        "schema_version = 1\n\n[input.buttons]\nfavorite = \"y\"\n",
    );
    let mut page = global_page(&paths);
    assert!(!page.is_dirty());
    form(&mut page).assign(PadAction::Favorite, PadButton::A);
    assert_eq!(page.changes(), 1);
    form(&mut page).assign(PadAction::Favorite, PadButton::Y);
    assert_eq!(page.changes(), 0);
    assert!(global_editor(&page).form.library_edits().input.is_none());
    let _ = page.update(&paths, Event::ResetButtons);
    assert_eq!(page.changes(), 0);
    assert!(global_editor(&page).form.library_edits().is_empty());
}

mod app {
    //! The capture in the app: pad presses go to the waiting row first.

    use iced::keyboard::{Key, Modifiers, key::Named};
    use iced::window;

    use super::*;
    use crate::Message;
    use crate::pad::{PadEvent, PadInfo, Raw};

    fn xbox() -> PadInfo {
        PadInfo {
            name: "Xbox Wireless Controller".to_owned(),
            vendor: Some(0x045e),
            product: Some(0x0b13),
        }
    }

    /// Global settings open and loaded, a pad connected, the window
    /// focused, and the pad clock pinned.
    fn app_in(paths: &Paths) -> (App, Instant) {
        let mut app = App::boot(paths.clone()).0;
        let start = Instant::now();
        app.pad_clock = Some(start);
        let _ = app.update(Message::Pad(PadEvent::Connected(0, xbox())));
        let id = app.main_window.unwrap();
        let _ = app.update(Message::Window(id, window::Event::Focused));
        let _ = app.open_global_settings();
        let generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot_global(paths),
        )));
        (app, start)
    }

    fn page(app: &App) -> &Page {
        app.settings.as_ref().unwrap()
    }

    fn press(app: &mut App, raw: Raw) {
        let _ = app.update(Message::Pad(PadEvent::Pressed(0, raw)));
        let _ = app.update(Message::Pad(PadEvent::Released(0, raw)));
    }

    fn start_capture(app: &mut App, action: PadAction) {
        let _ = app.update(Message::Settings(Event::CaptureButton(action)));
        assert_eq!(page(app).capturing(), Some(action));
    }

    #[test]
    fn the_next_pad_button_goes_to_the_waiting_row_b_included() {
        let (_root, paths) = setup();
        let (mut app, _) = app_in(&paths);
        start_capture(&mut app, PadAction::Favorite);
        // A D-pad press isn't a button, so it keeps waiting.
        press(&mut app, Raw::DPadLeft);
        assert_eq!(page(&app).capturing(), Some(PadAction::Favorite));
        press(&mut app, Raw::West);
        assert_eq!(page(&app).capturing(), None);
        assert_eq!(button(page(&app), PadAction::Favorite), Some(PadButton::X));
        assert_eq!(button(page(&app), PadAction::Mark), Some(PadButton::Y));

        // B is taken for the row too, rather than going back.
        start_capture(&mut app, PadAction::Confirm);
        press(&mut app, Raw::East);
        assert_eq!(button(page(&app), PadAction::Confirm), Some(PadButton::B));
        assert_eq!(button(page(&app), PadAction::Back), Some(PadButton::A));
        assert!(app.settings.is_some());

        // Start too, which doesn't also toggle full screen.
        start_capture(&mut app, PadAction::Random);
        press(&mut app, Raw::Start);
        assert_eq!(
            button(page(&app), PadAction::Random),
            Some(PadButton::Start)
        );
        assert_eq!(app.fullscreen, crate::fullscreen::Owner::Nobody);
        // With nothing waiting, Start does what the saved map says, since
        // the page's picks are only a draft until saved.
        press(&mut app, Raw::Start);
        assert_eq!(app.fullscreen, crate::fullscreen::Owner::User);
    }

    #[test]
    fn with_the_switch_saved_off_the_drafts_switch_starts_the_reader_and_the_rows() {
        let (_root, paths) = setup();
        write_library(
            &paths,
            "schema_version = 1\n\n[input]\ncontroller = false\n",
        );
        let (mut app, _) = app_in(&paths);
        assert!(!app.input.controller);
        assert!(!app.pad_reading, "off at start, the reader waits");
        // Off in the draft too: the rows don't wait for a pad.
        let _ = app.update(Message::Settings(Event::CaptureButton(PadAction::Favorite)));
        assert_eq!(page(&app).capturing(), None);

        // On in the draft, before any save.
        let _ = app.update(Message::Settings(Event::GlobalController(true)));
        assert!(app.pad_reading);
        assert!(!app.input.controller, "the saved switch is still off");
        start_capture(&mut app, PadAction::Favorite);
        press(&mut app, Raw::West);
        assert_eq!(page(&app).capturing(), None);
        assert_eq!(button(page(&app), PadAction::Favorite), Some(PadButton::X));

        // Off in the draft again: the reader stays, the rows stop.
        let _ = app.update(Message::Settings(Event::GlobalController(false)));
        assert!(app.pad_reading);
        let _ = app.update(Message::Settings(Event::CaptureButton(PadAction::Mark)));
        assert_eq!(page(&app).capturing(), None);
    }

    #[test]
    fn a_waiting_row_hears_no_pad_until_the_gate_opens() {
        let (_root, paths) = setup();
        let (mut app, _) = app_in(&paths);
        // As at launch: no real focus or press yet.
        app.pad_gate = crate::app_pad::Gate::default();
        assert!(!app.pad_gate_open());
        start_capture(&mut app, PadAction::Favorite);
        press(&mut app, Raw::West);
        assert_eq!(page(&app).capturing(), Some(PadAction::Favorite));
        assert_eq!(button(page(&app), PadAction::Favorite), Some(PadButton::Y));
    }

    #[test]
    fn the_rows_open_in_the_connected_pads_labels() {
        let (_root, paths) = setup();
        let mut app = App::boot(paths.clone()).0;
        let dualsense = PadInfo {
            name: "DualSense Wireless Controller".to_owned(),
            vendor: Some(0x054c),
            product: Some(0x0ce6),
        };
        let _ = app.update(Message::Pad(PadEvent::Connected(0, dualsense)));
        let _ = app.open_global_settings();
        assert_eq!(page(&app).pad_style, crate::pad::Style::PlayStation);
        let generation = page(&app).generation;
        let _ = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot_global(&paths),
        )));
        let texts = page_texts(page(&app), &paths);
        assert!(texts.iter().any(|text| text == "L1"), "{texts:?}");
        assert!(!texts.iter().any(|text| text == "LB"));
    }

    #[test]
    fn the_wait_gives_up_after_five_seconds_on_the_pad_clock() {
        let (_root, paths) = setup();
        let (mut app, start) = app_in(&paths);
        start_capture(&mut app, PadAction::Favorite);
        assert_eq!(app.capture_wake.due(), Some(start + CAPTURE_FOR));
        app.pad_clock = Some(start + CAPTURE_FOR - Duration::from_millis(1));
        let generation = app.capture_wake.generation();
        let _ = app.update(Message::CaptureTimeout(generation));
        // Early, it re-arms and keeps waiting.
        assert_eq!(page(&app).capturing(), Some(PadAction::Favorite));
        app.pad_clock = Some(start + CAPTURE_FOR);
        let generation = app.capture_wake.generation();
        let _ = app.update(Message::CaptureTimeout(generation));
        assert_eq!(page(&app).capturing(), None);
        assert_eq!(app.capture_wake.due(), None);
        press(&mut app, Raw::West);
        assert!(!page(&app).is_dirty());
    }

    #[test]
    fn esc_losing_focus_or_the_pad_leaving_ends_the_wait() {
        let (_root, paths) = setup();
        let (mut app, _) = app_in(&paths);
        start_capture(&mut app, PadAction::Favorite);
        let _ = app.update(Message::Key(
            Key::Named(Named::Escape),
            Modifiers::empty(),
            iced::event::Status::Ignored,
        ));
        assert_eq!(page(&app).capturing(), None);

        start_capture(&mut app, PadAction::Favorite);
        let id = app.main_window.unwrap();
        let _ = app.update(Message::Window(id, window::Event::Unfocused));
        assert_eq!(page(&app).capturing(), None);
        let _ = app.update(Message::Window(id, window::Event::Focused));

        start_capture(&mut app, PadAction::Favorite);
        let _ = app.update(Message::Pad(PadEvent::Disconnected(0)));
        assert_eq!(page(&app).capturing(), None);
        assert!(!page(&app).is_dirty());
    }

    #[test]
    fn any_click_or_key_after_the_starting_one_ends_the_wait() {
        let (_root, paths) = setup();
        let (mut app, start) = app_in(&paths);
        // Any mouse button or key in the window is a press, whatever is
        // under it: a text field, the space outside the column, the
        // header.
        for event in [
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                iced::mouse::Button::Right,
            )),
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: Key::Character("a".into()),
                modified_key: Key::Character("a".into()),
                physical_key: iced::keyboard::key::Physical::Unidentified(
                    iced::keyboard::key::NativeCode::Unidentified,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers: Modifiers::empty(),
                text: None,
                repeat: false,
            }),
        ] {
            let id = app.main_window.unwrap();
            let pressed = crate::window_or_press(event.clone(), id);
            assert!(matches!(pressed, Some(Message::Pressed)), "{event:?}");

            // The click or Enter that started it arrives after the row's
            // own message, inside the grace: it keeps waiting.
            app.pad_clock = Some(start);
            start_capture(&mut app, PadAction::Favorite);
            let _ = app.update(pressed.clone().unwrap());
            app.pad_clock = Some(start + crate::app_pad::PRESS_GRACE - Duration::from_millis(1));
            let _ = app.update(pressed.clone().unwrap());
            assert_eq!(
                page(&app).capturing(),
                Some(PadAction::Favorite),
                "{event:?}"
            );
            // Any later press ends it.
            app.pad_clock = Some(start + crate::app_pad::PRESS_GRACE);
            let _ = app.update(pressed.unwrap());
            assert_eq!(page(&app).capturing(), None, "{event:?}");
        }
        // A tab switch too.
        app.pad_clock = Some(start);
        start_capture(&mut app, PadAction::Favorite);
        let _ = app.update(Message::Settings(Event::Tab(Tab::Appearance)));
        assert_eq!(page(&app).capturing(), None);
    }

    #[test]
    fn a_saved_remap_reaches_the_live_map_and_the_pads_style_the_rows() {
        let (_root, paths) = setup();
        let (mut app, _) = app_in(&paths);
        assert_eq!(
            app.input.bindings().action(PadButton::X),
            Some(PadAction::Mark)
        );
        // Another window, or this page's save, writes a remap.
        write_library(
            &paths,
            "schema_version = 1\n\n[input.buttons]\nconfirm = \"x\"\nmark = \"a\"\n",
        );
        // The page's next check finds it and rebases, as it would live.
        let generation = page(&app).generation;
        let sequence = start_check(app.settings.as_mut().unwrap());
        let _ = app.update(Message::Settings(Event::Checked(
            generation,
            sequence,
            load::Texts::read_global(&paths),
        )));
        assert!(page(&app).loading());
        let _ = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot_global(&paths),
        )));
        assert_eq!(
            app.input.bindings().action(PadButton::X),
            Some(PadAction::Confirm)
        );
        assert_eq!(
            app.input.bindings().action(PadButton::A),
            Some(PadAction::Mark)
        );

        // A pad press styles the rows in its own labels.
        let nintendo = PadInfo {
            name: "Nintendo Switch Pro Controller".to_owned(),
            vendor: Some(0x057e),
            product: Some(0x2009),
        };
        let _ = app.update(Message::Pad(PadEvent::Connected(1, nintendo)));
        assert_eq!(page(&app).pad_style, crate::pad::Style::Xbox);
        let _ = app.update(Message::Pad(PadEvent::Pressed(1, Raw::LeftBumper)));
        assert_eq!(page(&app).pad_style, crate::pad::Style::Nintendo);
    }
}
