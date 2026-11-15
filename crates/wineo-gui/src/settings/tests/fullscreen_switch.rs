//! "Open coverflow in full screen" on the global page: the form's edit, the
//! save to `[view]`, and what a saved change does to the window.

use iced::window::Mode;
use iced_runtime::window::Action as WindowAction;
use wineo_core::library::{self, ViewLayout};

use super::*;
use crate::fullscreen::Owner;

fn saved_form(paths: &Paths) -> global::Form {
    let page = global_page(paths);
    global_editor(&page).form.clone()
}

#[test]
fn the_switch_is_a_library_edit_and_counts_as_a_change() {
    let (_root, paths) = setup();
    let mut form = saved_form(&paths);
    assert!(!form.draft.coverflow_fullscreen);
    assert_eq!(form.coverflow_fullscreen_edit(), None);
    assert_eq!(form.changes(), 0);

    form.draft.coverflow_fullscreen = true;
    assert_eq!(form.coverflow_fullscreen_edit(), Some(true));
    assert_eq!(form.changes(), 1);
    assert_eq!(form.library_edits().coverflow_fullscreen, Some(true));
    assert!(!form.library_edits().is_empty());
    assert!(form.edits().is_empty(), "config.toml has nothing to write");

    form.revert();
    assert_eq!(form.library_edits().coverflow_fullscreen, None);
    assert!(form.library_edits().is_empty());
}

#[test]
fn saving_writes_view_coverflow_fullscreen_and_nothing_else() {
    let (_root, paths) = setup();
    let mut form = saved_form(&paths);
    form.draft.coverflow_fullscreen = true;
    let report = load::save_global_page(&paths, &form.edits(), form.library_edits(), None).unwrap();
    assert!(report.failed.is_none());
    let saved = library::load(&paths);
    assert!(saved.view.coverflow_fullscreen);
    assert_eq!(saved.view.layout, ViewLayout::Grid);
    assert!(!paths.global_config().exists());

    // Back off through the same path.
    let mut form = saved_form(&paths);
    assert!(form.loaded.coverflow_fullscreen);
    form.draft.coverflow_fullscreen = false;
    let report = load::save_global_page(&paths, &form.edits(), form.library_edits(), None).unwrap();
    assert!(report.failed.is_none());
    assert!(!library::load(&paths).view.coverflow_fullscreen);
}

#[test]
fn the_page_event_edits_the_draft_and_marks_the_page_dirty() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    assert!(!page.is_dirty());
    let _ = page.update(&paths, Event::GlobalCoverflowFullscreen(true));
    assert!(global_editor(&page).form.draft.coverflow_fullscreen);
    assert!(page.is_dirty());
    let _ = page.update(&paths, Event::GlobalCoverflowFullscreen(false));
    assert!(!page.is_dirty());
}

#[test]
fn the_row_names_the_switch() {
    let (_root, paths) = setup();
    let app = open_global_app(&paths);
    let texts = super::custom_themes::texts(&app, &paths);
    assert!(
        texts
            .iter()
            .any(|text| text == "Open coverflow in full screen"),
        "{texts:?}"
    );
}

/// The window mode changes a task asks for.
fn modes(task: Task<Message>) -> Vec<Mode> {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut modes = Vec::new();
        while let Ok(Some(action)) =
            tokio::time::timeout(std::time::Duration::from_millis(300), stream.next()).await
        {
            if let iced_runtime::Action::Window(WindowAction::SetMode(_, mode)) = action {
                modes.push(mode);
            }
        }
        modes
    })
}

/// Saves the switch from the open page the way a click and Save would, and
/// returns the mode changes the whole round asked for.
fn flip_and_save(app: &mut App, on: bool) -> Vec<Mode> {
    use iced::futures::StreamExt;
    let _ = app.update(Message::Settings(Event::GlobalCoverflowFullscreen(on)));
    let task = app.update(Message::Settings(Event::Save));
    let mut asked = Vec::new();
    let mut pending = vec![task];
    for _ in 0..6 {
        let Some(task) = pending.pop() else { break };
        let Some(mut stream) = iced_runtime::task::into_stream(task) else {
            continue;
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let actions = runtime.block_on(async {
            let mut actions = Vec::new();
            while let Ok(Some(action)) =
                tokio::time::timeout(std::time::Duration::from_millis(300), stream.next()).await
            {
                actions.push(action);
            }
            actions
        });
        for action in actions {
            match action {
                iced_runtime::Action::Window(WindowAction::SetMode(_, mode)) => asked.push(mode),
                iced_runtime::Action::Output(message) => pending.push(app.update(message)),
                _ => {}
            }
        }
    }
    asked
}

#[test]
fn an_unsaved_flip_does_nothing_to_the_window() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    app.layout = ViewLayout::Coverflow;
    let task = app.update(Message::Settings(Event::GlobalCoverflowFullscreen(true)));
    assert!(modes(task).is_empty());
    assert!(!app.coverflow_fullscreen);
    assert_eq!(app.fullscreen, Owner::Nobody);
}

#[test]
fn saving_the_switch_on_in_coverflow_enters_and_saving_it_off_leaves() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    app.layout = ViewLayout::Coverflow;

    assert_eq!(flip_and_save(&mut app, true), [Mode::Fullscreen]);
    assert!(app.coverflow_fullscreen);
    assert_eq!(app.fullscreen, Owner::Switch);

    assert_eq!(flip_and_save(&mut app, false), [Mode::Windowed]);
    assert!(!app.coverflow_fullscreen);
    assert_eq!(app.fullscreen, Owner::Nobody);
}

#[test]
fn saving_the_switch_outside_coverflow_only_arms_it() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    assert_eq!(app.layout, ViewLayout::Grid);
    assert!(flip_and_save(&mut app, true).is_empty());
    assert!(app.coverflow_fullscreen);
    assert_eq!(app.fullscreen, Owner::Nobody);
}

#[test]
fn the_switch_never_touches_a_full_screen_the_user_owns() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    app.layout = ViewLayout::Coverflow;
    app.fullscreen = Owner::User;
    assert!(flip_and_save(&mut app, true).is_empty());
    assert_eq!(app.fullscreen, Owner::User);
    assert!(flip_and_save(&mut app, false).is_empty());
    assert_eq!(app.fullscreen, Owner::User);
}
