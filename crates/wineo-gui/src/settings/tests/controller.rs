//! The General tab's Controller section: the switch, saved to
//! `library.toml`'s `[input]`, and the pads connected now.

use super::*;
use crate::pad::{PadEvent, PadInfo};

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
fn the_section_lists_the_pads_or_says_none_was_found() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let texts = page_texts(&page, &paths);
    assert!(texts.iter().any(|text| text == view::CONTROLLER_SWITCH));
    assert!(texts.iter().any(|text| text == view::NO_CONTROLLER));

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
    let _ = app.update(crate::Message::Pad(PadEvent::Connected(0, xbox)));
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
    let _ = app.update(crate::Message::Pad(PadEvent::Disconnected(0)));
    assert!(app.settings.as_ref().unwrap().pads.is_empty());

    // An edit isn't saved yet, so the app keeps what's on disk.
    let _ = app.update(crate::Message::Settings(Event::GlobalController(true)));
    assert!(!app.input.controller);
}
