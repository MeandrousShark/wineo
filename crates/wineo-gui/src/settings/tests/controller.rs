//! The Controller tab: the switch, saved to
//! `library.toml`'s `[input]`, and the pads connected now.

use super::*;
use crate::pad::{PadEvent, PadId, PadInfo};

fn save(paths: &Paths, page: &Page) -> load::Report {
    let form = &global_editor(page).form;
    load::save_global_page(paths, &form.edits(), form.library_edits(), None).unwrap()
}

fn controller(page: &Page) -> bool {
    global_editor(page).form.draft.input.controller
}

#[test]
fn the_switch_is_on_by_default_and_off_saves_to_the_library() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    assert!(controller(&page));
    let _ = page.update(&paths, Event::GlobalController(false));
    assert_eq!(page.changes(), 1);
    assert!(global_editor(&page).form.edits().is_empty());
    assert!(save(&paths, &page).failed.is_none());
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        "schema_version = 1\n\n[input]\ncontroller = false\n"
    );
    assert!(!paths.global_config().exists());

    let mut page = global_page(&paths);
    assert!(!controller(&page));
    assert!(!page.is_dirty());
    let _ = page.update(&paths, Event::GlobalController(true));
    assert_eq!(page.changes(), 1);
    let _ = page.update(&paths, Event::GlobalController(false));
    assert_eq!(page.changes(), 0);
}

#[test]
fn saving_with_the_switch_untouched_leaves_the_library_alone() {
    let (_root, paths) = setup();
    let page = global_page(&paths);
    assert!(global_editor(&page).form.library_edits().input.is_none());
    assert!(save(&paths, &page).failed.is_none());
    assert!(!paths.library_file().exists());
}

#[test]
fn the_tab_lists_the_pads_or_says_none_was_found() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::Tab(Tab::Controller));
    let texts = page_texts(&page, &paths);
    assert!(texts.iter().any(|text| text == view::CONTROLLER_SWITCH));
    assert!(texts.iter().any(|text| text == view::NO_CONTROLLER));
    // No heading repeats the tab's name: the switch follows the tab labels.
    assert_eq!(
        texts.iter().filter(|text| *text == "Controller").count(),
        1,
        "{texts:?}"
    );
    let after_tabs = texts
        .iter()
        .position(|text| text == Tab::Advanced.label())
        .unwrap();
    assert_eq!(texts[after_tabs + 1], view::CONTROLLER_SWITCH, "{texts:?}");

    page.pads = vec!["Xbox Wireless Controller".to_owned()];
    let texts = page_texts(&page, &paths);
    assert!(texts.iter().any(|text| text == "Xbox Wireless Controller"));
    assert!(!texts.iter().any(|text| text == view::NO_CONTROLLER));
}

#[test]
fn the_app_keeps_the_pads_and_the_saved_switch_current() {
    let (_root, paths) = setup();
    let mut app = App::boot(paths.clone()).0;
    let xbox = PadInfo {
        name: "Xbox Wireless Controller".to_owned(),
        vendor: Some(0x045e),
        product: Some(0x0b13),
    };
    let _ = app.update(crate::Message::Pad(PadEvent::Connected(
        PadId::Gilrs(0),
        xbox,
    )));
    assert!(app.input.controller);
    // Another window turned it off since this one started.
    wineo_core::library::set_input(
        &paths,
        &wineo_core::library::Input {
            controller: false,
            ..Default::default()
        },
    )
    .unwrap();
    let _ = app.open_global_settings();
    assert_eq!(
        app.settings.as_ref().unwrap().pads,
        ["Xbox Wireless Controller"]
    );
    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(crate::Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(&paths),
    )));
    assert!(!app.input.controller);
    let _ = app.update(crate::Message::Pad(PadEvent::Disconnected(PadId::Gilrs(0))));
    assert!(app.settings.as_ref().unwrap().pads.is_empty());

    // An edit isn't saved yet, so the app keeps what's on disk.
    let _ = app.update(crate::Message::Settings(Event::GlobalController(true)));
    assert!(!app.input.controller);
}

fn gamecube(page: &Page) -> bool {
    global_editor(page).form.draft.input.gamecube_adapter
}

fn controller_tab(paths: &Paths) -> Page {
    let mut page = global_page(paths);
    let _ = page.update(paths, Event::Tab(Tab::Controller));
    page
}

#[test]
fn the_gamecube_switch_is_off_on_a_library_with_no_key_and_sits_under_the_controller_one() {
    let (_root, paths) = setup();
    let page = controller_tab(&paths);
    assert!(!gamecube(&page));
    assert!(!page.is_dirty());
    assert_eq!(view::GAMECUBE_ADAPTER_SWITCH, "Use a GameCube adapter");
    let texts = page_texts(&page, &paths);
    let first = texts
        .iter()
        .position(|text| text == view::CONTROLLER_SWITCH)
        .unwrap();
    // Each switch row's blank source label sits between them, and the hint
    // switch sits between the two, since it has nothing to do with the
    // adapter.
    assert_eq!(texts[first + 2], view::HINT_BAR_SWITCH, "{texts:?}");
    assert_eq!(texts[first + 4], view::GAMECUBE_ADAPTER_SWITCH, "{texts:?}");

    // A file with only the controller switch reads the same.
    write_library_text(&paths, "schema_version = 1\n[input]\ncontroller = true\n");
    assert!(!gamecube(&controller_tab(&paths)));
}

fn write_library_text(paths: &Paths, text: &str) {
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), text).unwrap();
}

#[test]
fn flipping_the_gamecube_switch_enables_save_by_itself_and_saving_stores_the_key() {
    let (_root, paths) = setup();
    let mut page = controller_tab(&paths);
    assert!(!page.can_save());
    let _ = page.update(&paths, Event::GlobalGamecubeAdapter(true));
    assert!(page.is_dirty() && page.can_save());
    assert_eq!(page.changes(), 1);
    assert!(global_editor(&page).form.edits().is_empty());
    assert!(!global_editor(&page).form.library_edits().is_empty());

    // Flipped back before saving, it is no change at all.
    let _ = page.update(&paths, Event::GlobalGamecubeAdapter(false));
    assert!(!page.is_dirty() && !page.can_save());
    assert_eq!(page.changes(), 0);
    let _ = page.update(&paths, Event::GlobalGamecubeAdapter(true));

    assert!(save(&paths, &page).failed.is_none());
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        "schema_version = 1\n\n[input]\ngamecube_adapter = true\n"
    );
    assert!(!paths.global_config().exists());

    // Back off is stored as false, and the controller switch is untouched.
    let mut page = controller_tab(&paths);
    assert!(gamecube(&page) && controller(&page));
    let _ = page.update(&paths, Event::GlobalGamecubeAdapter(false));
    assert!(save(&paths, &page).failed.is_none());
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        "schema_version = 1\n\n[input]\ngamecube_adapter = false\n"
    );
}

#[test]
fn a_saved_gamecube_switch_is_not_an_outside_change() {
    let (_root, paths) = setup();
    let mut page = controller_tab(&paths);
    let _ = page.update(&paths, Event::GlobalGamecubeAdapter(true));
    let _ = page.update(&paths, Event::Save);
    assert!(page.saving);
    let report = save(&paths, &page);
    let _ = page.update(&paths, Event::Saved(1, Ok(report)));
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert!(!page.is_dirty());
    assert!(gamecube(&page));

    // The check that follows reads the file this page just wrote.
    let sequence = page.check_sequence + 1;
    page.check_sequence = sequence;
    page.checking = true;
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, load::Texts::read_global(&paths)),
    );
    assert!(!page.changed_on_disk && !page.loading());
    assert!(!page.is_dirty());
}

/// What a press and release at the toggler of the row labelled `label` sends.
fn click_switch(page: &Page, paths: &Paths, label: &str) -> Vec<crate::Message> {
    use crate::layout_probe::lay_out;
    use iced::advanced::widget::{Id, Operation};
    use std::sync::{Arc, Mutex};

    struct Find(String, Arc<Mutex<Option<iced::Rectangle>>>);
    impl Operation for Find {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn text(&mut self, _id: Option<&Id>, bounds: iced::Rectangle, text: &str) {
            if text == self.0 {
                *self.1.lock().unwrap() = Some(bounds);
            }
        }
    }
    let found = Arc::new(Mutex::new(None));
    let mut laid = lay_out(
        page.view(paths, crate::theme::Palette::DARK),
        iced::Size::new(900.0, 3000.0),
    );
    laid.run(Box::new(Find(label.to_owned(), found.clone())));
    let bounds = found.lock().unwrap().expect(label);
    // The toggler is at the left of its box, after the 180 wide label and
    // the 12 gap.
    let cursor = iced::mouse::Cursor::Available(iced::Point::new(
        bounds.x + 192.0 + 10.0,
        bounds.center_y(),
    ));
    let left = iced::mouse::Button::Left;
    let mut messages = laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(left)),
        cursor,
    );
    messages.extend(laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonReleased(left)),
        cursor,
    ));
    messages
}

#[test]
fn the_gamecube_switch_waits_for_the_controller_switch_like_the_remap_rows() {
    let (_root, paths) = setup();
    let label = view::GAMECUBE_ADAPTER_SWITCH;
    let mut page = controller_tab(&paths);

    let messages = click_switch(&page, &paths, label);
    assert!(
        matches!(
            messages.as_slice(),
            [crate::Message::Settings(Event::GlobalGamecubeAdapter(true))]
        ),
        "{messages:?}"
    );

    // The Controller switch off: the switch is as locked as a remap row.
    let _ = page.update(&paths, Event::GlobalController(false));
    assert!(click_switch(&page, &paths, label).is_empty());
    let _ = page.update(&paths, Event::GlobalController(true));
    assert_eq!(click_switch(&page, &paths, label).len(), 1);

    // A page that is saving locks it too, as it does every control.
    page.saving = true;
    assert!(click_switch(&page, &paths, label).is_empty());
}

#[test]
fn a_saving_page_and_an_environment_page_ignore_the_gamecube_event() {
    let (_root, paths) = setup();
    let mut saving = controller_tab(&paths);
    saving.saving = true;
    let _ = saving.update(&paths, Event::GlobalGamecubeAdapter(true));
    assert!(!gamecube(&saving) && !saving.is_dirty());

    write(&paths, GAME);
    let mut env = page(&paths);
    // A handled edit clears the notes, so they show the event was dropped.
    env.notes = vec!["Installed DXVK 2.4".to_owned()];
    let _ = env.update(&paths, Event::GlobalGamecubeAdapter(true));
    assert!(!env.is_dirty());
    assert_eq!(env.notes, ["Installed DXVK 2.4"]);
}

#[test]
fn the_gamecube_event_does_nothing_while_the_draft_controller_switch_is_off() {
    let (_root, paths) = setup();
    let mut page = controller_tab(&paths);
    let _ = page.update(&paths, Event::GlobalController(false));
    // A handled edit clears these, so they show the event was dropped.
    page.notes = vec!["Installed DXVK 2.4".to_owned()];
    page.error = Some("earlier".to_owned());
    let _ = page.update(&paths, Event::GlobalGamecubeAdapter(true));
    assert!(!gamecube(&page));
    assert_eq!(page.notes, ["Installed DXVK 2.4"]);
    assert_eq!(page.error.as_deref(), Some("earlier"));
    assert_eq!(page.changes(), 1, "only the Controller switch changed");

    let _ = page.update(&paths, Event::GlobalController(true));
    let _ = page.update(&paths, Event::GlobalGamecubeAdapter(true));
    assert!(gamecube(&page));
    assert!(page.notes.is_empty() && page.error.is_none());
    assert_eq!(page.changes(), 1, "only the adapter switch differs now");
}

#[test]
fn the_adapter_has_a_line_while_wineo_isnt_holding_it() {
    use crate::pad::adapter::State;
    assert_eq!(view::ADAPTER_FREE, "GameCube adapter");
    assert_eq!(
        view::ADAPTER_BUSY,
        "GameCube adapter: in use by another program."
    );
    assert_eq!(
        view::ADAPTER_NO_ACCESS,
        "GameCube adapter: no access. See the docs."
    );
    let (_root, paths) = setup();
    let mut page = controller_tab(&paths);
    let watched = [
        view::ADAPTER_FREE,
        view::ADAPTER_BUSY,
        view::ADAPTER_NO_ACCESS,
        view::NO_CONTROLLER,
        "Xbox Wireless Controller",
        "GameCube controller (port 1)",
    ];
    let shown = |page: &Page| -> Vec<String> {
        page_texts(page, &paths)
            .into_iter()
            .filter(|text| watched.contains(&text.as_str()))
            .collect()
    };
    for (state, line) in [
        (State::Free, view::ADAPTER_FREE),
        (State::Busy, view::ADAPTER_BUSY),
        (State::NoAccess, view::ADAPTER_NO_ACCESS),
    ] {
        page.adapter = state;
        assert_eq!(shown(&page), [line], "{state:?}");
        // After any pads, never in place of them.
        page.pads = vec!["Xbox Wireless Controller".to_owned()];
        assert_eq!(shown(&page), ["Xbox Wireless Controller", line]);
        page.pads.clear();
    }
    // Used by the kernel or gone: no line, and nothing found without pads.
    for state in [State::Kernel, State::Absent] {
        page.adapter = state;
        assert_eq!(shown(&page), [view::NO_CONTROLLER], "{state:?}");
    }
    // Held, its pads show instead.
    page.adapter = State::Held;
    page.pads = vec!["GameCube controller (port 1)".to_owned()];
    assert_eq!(shown(&page), ["GameCube controller (port 1)"]);
    // Held with every port empty: the plain line, not "No controller found.",
    // so the list doesn't flip as the window gains and loses focus.
    page.pads.clear();
    assert_eq!(shown(&page), ["GameCube adapter"]);
    // Any pad counts, not only the adapter's own.
    page.pads = vec!["Xbox Wireless Controller".to_owned()];
    assert_eq!(shown(&page), ["Xbox Wireless Controller"]);
}

#[test]
fn the_app_keeps_the_adapters_line_current() {
    use crate::pad::adapter::{Report, State};
    let (_root, paths) = setup();
    let mut app = App::boot(paths.clone()).0;
    let _ = app.open_global_settings();
    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(crate::Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(&paths),
    )));
    let _ = app.update(crate::Message::Settings(Event::GlobalGamecubeAdapter(true)));
    let _ = app.update(crate::Message::Adapter(Report::State(State::Busy)));
    assert_eq!(app.settings.as_ref().unwrap().adapter, State::Busy);
    // A page opened later starts from what the app last heard.
    let _ = app.update(crate::Message::Settings(Event::GlobalGamecubeAdapter(
        false,
    )));
    assert_eq!(app.settings.as_ref().unwrap().adapter, State::Absent);
    app.settings = None;
    app.input.gamecube_adapter = true;
    let _ = app.update(crate::Message::Adapter(Report::State(State::NoAccess)));
    let _ = app.open_global_settings();
    assert_eq!(app.settings.as_ref().unwrap().adapter, State::NoAccess);
}

fn hint_bar(page: &Page) -> bool {
    global_editor(page).form.draft.input.hint_bar
}

#[test]
fn the_hint_switch_is_on_on_a_library_with_no_key_and_is_worded_as_planned() {
    let (_root, paths) = setup();
    let page = controller_tab(&paths);
    assert!(hint_bar(&page));
    assert!(!page.is_dirty());
    assert_eq!(view::HINT_BAR_SWITCH, "Show button hints");

    // A file with only the controller switch reads the same, and so does a
    // wrong type.
    write_library_text(&paths, "schema_version = 1\n[input]\ncontroller = true\n");
    assert!(hint_bar(&controller_tab(&paths)));
    write_library_text(
        &paths,
        "schema_version = 1\n[input]\nhint_bar = \"false\"\n",
    );
    assert!(hint_bar(&controller_tab(&paths)));
    write_library_text(&paths, "schema_version = 1\n[input]\nhint_bar = false\n");
    assert!(!hint_bar(&controller_tab(&paths)));
}

#[test]
fn flipping_the_hint_switch_enables_save_by_itself_and_saving_stores_the_key() {
    let (_root, paths) = setup();
    let mut page = controller_tab(&paths);
    assert!(!page.can_save());
    let _ = page.update(&paths, Event::GlobalHintBar(false));
    assert!(page.is_dirty() && page.can_save());
    assert_eq!(page.changes(), 1);
    assert!(global_editor(&page).form.edits().is_empty());
    assert!(!global_editor(&page).form.library_edits().is_empty());

    // Flipped back before saving, it is no change at all.
    let _ = page.update(&paths, Event::GlobalHintBar(true));
    assert!(!page.is_dirty() && !page.can_save());
    assert_eq!(page.changes(), 0);
    let _ = page.update(&paths, Event::GlobalHintBar(false));

    assert!(save(&paths, &page).failed.is_none());
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        "schema_version = 1\n\n[input]\nhint_bar = false\n"
    );
    assert!(!paths.global_config().exists());

    // Back on is stored as true, and the other switches are untouched.
    let mut page = controller_tab(&paths);
    assert!(!hint_bar(&page) && controller(&page) && !gamecube(&page));
    let _ = page.update(&paths, Event::GlobalHintBar(true));
    assert!(save(&paths, &page).failed.is_none());
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        "schema_version = 1\n\n[input]\nhint_bar = true\n"
    );
}

#[test]
fn a_saved_hint_switch_is_not_an_outside_change() {
    let (_root, paths) = setup();
    let mut page = controller_tab(&paths);
    let _ = page.update(&paths, Event::GlobalHintBar(false));
    let _ = page.update(&paths, Event::Save);
    assert!(page.saving);
    let report = save(&paths, &page);
    let _ = page.update(&paths, Event::Saved(1, Ok(report)));
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert!(!page.is_dirty());
    assert!(!hint_bar(&page));

    // The check that follows reads the file this page just wrote.
    let sequence = page.check_sequence + 1;
    page.check_sequence = sequence;
    page.checking = true;
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, load::Texts::read_global(&paths)),
    );
    assert!(!page.changed_on_disk && !page.loading());
    assert!(!page.is_dirty());
}

#[test]
fn the_hint_switch_waits_for_the_controller_switch_like_the_adapter_one() {
    let (_root, paths) = setup();
    let label = view::HINT_BAR_SWITCH;
    let mut page = controller_tab(&paths);

    let messages = click_switch(&page, &paths, label);
    assert!(
        matches!(
            messages.as_slice(),
            [crate::Message::Settings(Event::GlobalHintBar(false))]
        ),
        "{messages:?}"
    );

    // The Controller switch off: the switch is as locked as a remap row.
    let _ = page.update(&paths, Event::GlobalController(false));
    assert!(click_switch(&page, &paths, label).is_empty());
    let _ = page.update(&paths, Event::GlobalController(true));
    assert_eq!(click_switch(&page, &paths, label).len(), 1);

    // A page that is saving locks it too, as it does every control.
    page.saving = true;
    assert!(click_switch(&page, &paths, label).is_empty());
}

#[test]
fn the_hint_event_does_nothing_while_the_draft_controller_switch_is_off() {
    let (_root, paths) = setup();
    let mut page = controller_tab(&paths);
    let _ = page.update(&paths, Event::GlobalController(false));
    // A handled edit clears these, so they show the event was dropped.
    page.notes = vec!["Installed DXVK 2.4".to_owned()];
    page.error = Some("earlier".to_owned());
    let _ = page.update(&paths, Event::GlobalHintBar(false));
    assert!(hint_bar(&page));
    assert_eq!(page.notes, ["Installed DXVK 2.4"]);
    assert_eq!(page.error.as_deref(), Some("earlier"));
    assert_eq!(page.changes(), 1, "only the Controller switch changed");

    let _ = page.update(&paths, Event::GlobalController(true));
    let _ = page.update(&paths, Event::GlobalHintBar(false));
    assert!(!hint_bar(&page));
    assert!(page.notes.is_empty() && page.error.is_none());
    assert_eq!(page.changes(), 1, "only the hint switch differs now");
}

#[test]
fn a_saving_page_and_an_environment_page_ignore_the_hint_event() {
    let (_root, paths) = setup();
    let mut saving = controller_tab(&paths);
    saving.saving = true;
    let _ = saving.update(&paths, Event::GlobalHintBar(false));
    assert!(hint_bar(&saving) && !saving.is_dirty());

    write(&paths, GAME);
    let mut env = page(&paths);
    env.notes = vec!["Installed DXVK 2.4".to_owned()];
    let _ = env.update(&paths, Event::GlobalHintBar(false));
    assert!(!env.is_dirty());
    assert_eq!(env.notes, ["Installed DXVK 2.4"]);
}
