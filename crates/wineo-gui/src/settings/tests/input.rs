//! What the global page tracks of `library.toml` beyond the tables it edits
//! yet: `[view] coverflow_fullscreen` and `[input]` with its buttons.

use std::collections::BTreeMap;

use wineo_core::library::{self, Input, PadAction, PadButton, ViewLayout};

use super::*;

fn input(controller: bool, buttons: &[(PadAction, PadButton)]) -> Input {
    Input {
        controller,
        buttons: buttons.iter().copied().collect::<BTreeMap<_, _>>(),
    }
}

fn write_library_text(paths: &Paths, text: &str) {
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), text).unwrap();
}

#[test]
fn the_global_page_reads_coverflow_fullscreen_and_input() {
    let (_root, paths) = setup();
    let texts = load::Texts::read_global(&paths);
    assert_eq!(texts.coverflow_fullscreen, Some(false));
    assert_eq!(texts.input, Some(Input::default()));

    write_library_text(
        &paths,
        "schema_version = 1\n[view]\ncoverflow_fullscreen = true\n[input]\ncontroller = false\n[input.buttons]\nfavorite = \"rb\"\n",
    );
    let page = global_page(&paths);
    let values = &global_editor(&page).form.loaded;
    assert!(values.coverflow_fullscreen);
    assert_eq!(
        values.input,
        input(false, &[(PadAction::Favorite, PadButton::Rb)])
    );
    assert_eq!(global_editor(&page).form.draft.input, values.input);
    assert!(!page.is_dirty());
}

#[test]
fn a_change_to_either_names_library_toml_and_a_toolbar_write_is_no_change() {
    let (_root, paths) = setup();
    let before = load::Texts::read_global(&paths);

    // The toolbar writes `[view]` all the time: sort, zoom, layout.
    library::set_layout(&paths, ViewLayout::Coverflow).unwrap();
    library::set_zoom(&paths, 120).unwrap();
    assert_eq!(load::Texts::read_global(&paths), before);

    library::set_coverflow_fullscreen(&paths, true).unwrap();
    let after = load::Texts::read_global(&paths);
    assert_ne!(after, before);
    assert_eq!(ChangedFile::between(&after, &before), ChangedFile::Library);

    library::set_input(&paths, &input(false, &[])).unwrap();
    let controller = load::Texts::read_global(&paths);
    assert_eq!(
        ChangedFile::between(&controller, &after),
        ChangedFile::Library
    );

    library::set_input(&paths, &input(false, &[(PadAction::Mark, PadButton::Rs)])).unwrap();
    let buttons = load::Texts::read_global(&paths);
    assert_eq!(
        ChangedFile::between(&buttons, &controller),
        ChangedFile::Library
    );
}

#[test]
fn another_windows_change_to_either_rebases_an_untouched_form() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalArch(wineo_core::config::Arch::Win32));

    library::set_coverflow_fullscreen(&paths, true).unwrap();
    library::set_input(
        &paths,
        &input(false, &[(PadAction::Favorite, PadButton::Rb)]),
    )
    .unwrap();
    let texts = load::Texts::read_global(&paths);
    let generation = page.generation;
    page.drop_checks();
    let _ = page.update(
        &paths,
        Event::Checked(generation, page.check_sequence, texts),
    );
    let _ = page.update(
        &paths,
        Event::Loaded(generation, load::snapshot_global(&paths)),
    );
    let form = &global_editor(&page).form;
    assert!(form.draft.coverflow_fullscreen);
    assert!(!form.draft.input.controller);
    assert_eq!(
        form.draft.input.buttons.get(&PadAction::Favorite),
        Some(&PadButton::Rb)
    );
    // The edit to an unrelated field stays, and nothing asks about disk.
    assert_eq!(form.draft.arch, wineo_core::config::Arch::Win32);
    assert_eq!(page.changes(), 1);
    assert!(!page.changed_on_disk);
}

#[test]
fn rebase_keeps_a_field_the_form_changed_and_follows_the_ones_it_didnt() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let View::Global(editor) = &mut page.view else {
        panic!("global form expected");
    };
    // No control edits these yet, so the draft is set directly.
    editor.form.draft.coverflow_fullscreen = true;
    editor.form.draft.input.controller = false;

    let mut values = editor.form.loaded.clone();
    values.coverflow_fullscreen = false;
    values.input.controller = true;
    values.input.buttons.insert(PadAction::Mark, PadButton::Rs);
    editor.form.rebase(values);

    // Edited, so kept; the buttons weren't, so they follow.
    assert!(editor.form.draft.coverflow_fullscreen);
    assert!(!editor.form.draft.input.controller);
    assert_eq!(
        editor.form.draft.input.buttons.get(&PadAction::Mark),
        Some(&PadButton::Rs)
    );
    // The new base is what the form compares against from here on.
    assert!(!editor.form.loaded.coverflow_fullscreen);
    assert!(editor.form.loaded.input.controller);
}

#[test]
fn rebase_keeps_remaps_the_form_changed() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let View::Global(editor) = &mut page.view else {
        panic!("global form expected");
    };
    editor
        .form
        .draft
        .input
        .buttons
        .insert(PadAction::Favorite, PadButton::Rb);

    let mut values = editor.form.loaded.clone();
    values.input.buttons.insert(PadAction::Mark, PadButton::Rs);
    editor.form.rebase(values);

    assert_eq!(
        editor.form.draft.input.buttons,
        BTreeMap::from([(PadAction::Favorite, PadButton::Rb)])
    );
    assert_eq!(
        editor.form.loaded.input.buttons,
        BTreeMap::from([(PadAction::Mark, PadButton::Rs)])
    );
}

/// A rebase for `library.toml` that lands after `config.toml` moved too
/// keeps General's base, but still records what it read of the library, so
/// once the config is back the next check finds nothing new.
#[test]
fn a_rebase_that_finds_the_config_moved_still_records_both() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(
        &paths,
        Event::GlobalField(global::Field::StopTimeout, "7".to_owned()),
    );
    assert!(page.is_dirty());
    library::set_coverflow_fullscreen(&paths, true).unwrap();
    library::set_input(&paths, &input(false, &[])).unwrap();
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, load::Texts::read_global(&paths)),
    );
    assert!(page.loading(), "a library change rebases");
    // Another window writes the config while the rebase reads.
    fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
    fs::write(paths.global_config(), "stop_timeout = 9\n").unwrap();
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert!(!page.loading());
    // And takes it back.
    fs::remove_file(paths.global_config()).unwrap();
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, load::Texts::read_global(&paths)),
    );
    assert!(!page.loading(), "nothing changed since the rebase read");
    assert!(!page.changed_on_disk);
}
