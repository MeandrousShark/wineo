//! The General tab's Desktop section: Show a tray icon, Closing the window,
//! When something starts, and When everything stops, saved to `library.toml`'s `[desktop]`.

use wineo_core::library::{CloseWindow, Desktop, OnStart, OnStop};

use super::*;

fn save_desktop(paths: &Paths, page: &Page) -> load::Report {
    let form = &global_editor(page).form;
    load::save_global_page(paths, &form.edits(), form.library_edits(), None).unwrap()
}

fn desktop(page: &Page) -> Desktop {
    global_editor(page).form.draft.desktop
}

#[test]
fn the_desktop_section_reads_its_defaults_and_saves_to_the_library() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    assert_eq!(desktop(&page), Desktop::default());
    assert!(desktop(&page).tray);
    assert_eq!(desktop(&page).close, CloseWindow::Quit);

    let _ = page.update(&paths, Event::GlobalClose(CloseWindow::Tray));
    assert_eq!(page.changes(), 1);
    assert!(global_editor(&page).form.edits().is_empty());
    assert!(save_desktop(&paths, &page).failed.is_none());
    assert_eq!(
        wineo_core::library::load(&paths).desktop.close,
        CloseWindow::Tray
    );
    assert!(!paths.global_config().exists());

    let mut page = global_page(&paths);
    assert_eq!(desktop(&page).close, CloseWindow::Tray);
    assert!(!page.is_dirty());
    let _ = page.update(&paths, Event::GlobalTray(false));
    assert_eq!(page.changes(), 1);
    assert!(save_desktop(&paths, &page).failed.is_none());
    let saved = wineo_core::library::load(&paths).desktop;
    assert!(!saved.tray);
    // Turning the tray off leaves the close choice as saved.
    assert_eq!(saved.close, CloseWindow::Tray);
}

#[test]
fn saving_with_the_desktop_untouched_leaves_the_library_alone() {
    let (_root, paths) = setup();
    let page = global_page(&paths);
    assert!(global_editor(&page).form.library_edits().is_empty());
    assert!(save_desktop(&paths, &page).failed.is_none());
    assert!(!paths.library_file().exists());
}

#[test]
fn closing_the_window_is_locked_while_the_tray_is_off() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalTray(false));
    let _ = page.update(&paths, Event::GlobalClose(CloseWindow::Tray));
    assert_eq!(desktop(&page).close, CloseWindow::Quit);
    assert_eq!(page.changes(), 1);
    let _ = page.update(&paths, Event::GlobalTray(true));
    let _ = page.update(&paths, Event::GlobalClose(CloseWindow::Tray));
    assert_eq!(desktop(&page).close, CloseWindow::Tray);
    assert_eq!(page.changes(), 1);
}

#[test]
fn the_close_choice_reads_as_the_plan_says() {
    assert_eq!(
        super::super::view::CloseChoice::ALL.map(|choice| choice.to_string()),
        ["Quit", "Keep running in the tray"]
    );
    assert_eq!(super::super::view::NEEDS_TRAY, "Needs the tray icon.");
}

#[test]
fn reverting_puts_the_desktop_section_back() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalTray(false));
    let _ = page.update(&paths, Event::Revert);
    assert!(!page.is_dirty());
    assert_eq!(desktop(&page), Desktop::default());
}

#[test]
fn an_environment_page_ignores_the_desktop_section() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = Page::new("game".to_owned(), 1);
    let _ = page.reload(&paths, Mode::Replace);
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot(&paths, "game")));
    let _ = page.update(&paths, Event::GlobalTray(false));
    let _ = page.update(&paths, Event::GlobalClose(CloseWindow::Tray));
    let _ = page.update(&paths, Event::GlobalOnStart(OnStart::Hide));
    let _ = page.update(&paths, Event::GlobalOnStop(OnStop::Show));
    assert!(!page.is_dirty());
    assert_eq!(page.desktop(), None);
}

#[test]
fn another_windows_desktop_save_moves_only_untouched_controls() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalClose(CloseWindow::Tray));
    wineo_core::library::set_desktop(
        &paths,
        &Desktop {
            tray: false,
            close: CloseWindow::Quit,
            ..Desktop::default()
        },
    )
    .unwrap();
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, Texts::read_global(&paths)),
    );
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    let form = &global_editor(&page).form;
    assert!(!form.loaded.desktop.tray);
    assert!(!form.draft.desktop.tray, "the untouched switch follows");
    assert_eq!(
        form.draft.desktop.close,
        CloseWindow::Tray,
        "the edit stays"
    );
    assert_eq!(page.desktop(), Some(form.loaded.desktop));
}

#[test]
fn a_library_that_refuses_the_desktop_section_says_so_and_offers_checkup() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalTray(false));
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), "[desktop\nbroken").unwrap();
    let report = save_desktop(&paths, &page);
    assert!(report.failed.is_some());
    assert!(report.checkup);
}

#[test]
fn the_app_follows_the_desktop_the_settings_page_read() {
    let (_root, paths) = setup();
    wineo_core::library::set_desktop(
        &paths,
        &Desktop {
            tray: false,
            close: CloseWindow::Tray,
            ..Desktop::default()
        },
    )
    .unwrap();
    let mut app = App::boot(paths.clone()).0;
    assert!(!app.desktop.tray);
    wineo_core::library::set_desktop(&paths, &Desktop::default()).unwrap();
    let _ = app.open_global_settings();
    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(&paths),
    )));
    assert_eq!(app.desktop, Desktop::default());
    // An edit isn't saved yet, so the app keeps what's on disk.
    let _ = app.update(Message::Settings(Event::GlobalTray(false)));
    assert!(app.desktop.tray);
}

#[test]
fn when_something_starts_defaults_to_keep_and_saves_hide() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    assert_eq!(desktop(&page).on_start, OnStart::Keep);
    let _ = page.update(&paths, Event::GlobalOnStart(OnStart::Hide));
    assert_eq!(page.changes(), 1);
    assert!(global_editor(&page).form.edits().is_empty());
    assert!(save_desktop(&paths, &page).failed.is_none());
    let saved = wineo_core::library::load(&paths).desktop;
    assert_eq!(saved.on_start, OnStart::Hide);
    assert_eq!((saved.tray, saved.close), (true, CloseWindow::Quit));
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        "schema_version = 1\n\n[desktop]\non_start = \"hide\"\n"
    );

    let page = global_page(&paths);
    assert_eq!(desktop(&page).on_start, OnStart::Hide);
    assert!(!page.is_dirty());
}

#[test]
fn when_something_starts_is_locked_while_the_tray_is_off() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalTray(false));
    let _ = page.update(&paths, Event::GlobalOnStart(OnStart::Hide));
    assert_eq!(desktop(&page).on_start, OnStart::Keep);
    assert_eq!(page.changes(), 1);
    let _ = page.update(&paths, Event::GlobalTray(true));
    let _ = page.update(&paths, Event::GlobalOnStart(OnStart::Hide));
    assert_eq!(desktop(&page).on_start, OnStart::Hide);
    assert_eq!(page.changes(), 1);

    // Turning the tray off keeps the choice as made, locked.
    let _ = page.update(&paths, Event::GlobalTray(false));
    assert_eq!(desktop(&page).on_start, OnStart::Hide);
    let _ = page.update(&paths, Event::GlobalOnStart(OnStart::Keep));
    assert_eq!(desktop(&page).on_start, OnStart::Hide);
}

#[test]
fn the_on_start_choice_reads_as_the_plan_says() {
    assert_eq!(
        super::super::view::OnStartChoice::ALL.map(|choice| choice.to_string()),
        ["Keep the window open", "Hide to the tray"]
    );
}

#[test]
fn another_windows_on_start_save_moves_an_untouched_control() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalClose(CloseWindow::Tray));
    wineo_core::library::set_desktop(
        &paths,
        &Desktop {
            on_start: OnStart::Hide,
            ..Desktop::default()
        },
    )
    .unwrap();
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, Texts::read_global(&paths)),
    );
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    let form = &global_editor(&page).form;
    assert_eq!(form.draft.desktop.on_start, OnStart::Hide);
    assert_eq!(form.draft.desktop.close, CloseWindow::Tray);
}

#[test]
fn when_everything_stops_defaults_to_restore_and_saves_show() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    assert_eq!(desktop(&page).on_stop, OnStop::Restore);
    let _ = page.update(&paths, Event::GlobalOnStop(OnStop::Show));
    assert_eq!(page.changes(), 1);
    assert!(global_editor(&page).form.edits().is_empty());
    assert!(save_desktop(&paths, &page).failed.is_none());
    let saved = wineo_core::library::load(&paths).desktop;
    assert_eq!(saved.on_stop, OnStop::Show);
    assert_eq!(
        (saved.tray, saved.close, saved.on_start),
        (true, CloseWindow::Quit, OnStart::Keep)
    );
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        "schema_version = 1\n\n[desktop]\non_stop = \"show\"\n"
    );

    let page = global_page(&paths);
    assert_eq!(desktop(&page).on_stop, OnStop::Show);
    assert!(!page.is_dirty());
}

#[test]
fn when_everything_stops_is_locked_while_the_tray_is_off() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalTray(false));
    let _ = page.update(&paths, Event::GlobalOnStop(OnStop::Show));
    assert_eq!(desktop(&page).on_stop, OnStop::Restore);
    assert_eq!(page.changes(), 1);
    let _ = page.update(&paths, Event::GlobalTray(true));
    let _ = page.update(&paths, Event::GlobalOnStop(OnStop::Show));
    assert_eq!(desktop(&page).on_stop, OnStop::Show);
    assert_eq!(page.changes(), 1);

    // Turning the tray off keeps the choice as made, locked.
    let _ = page.update(&paths, Event::GlobalTray(false));
    assert_eq!(desktop(&page).on_stop, OnStop::Show);
    let _ = page.update(&paths, Event::GlobalOnStop(OnStop::Restore));
    assert_eq!(desktop(&page).on_stop, OnStop::Show);
}

#[test]
fn the_on_stop_choice_reads_as_the_plan_says() {
    assert_eq!(
        super::super::view::OnStopChoice::ALL.map(|choice| choice.to_string()),
        ["Bring back a hidden window", "Always show the window"]
    );
}

#[test]
fn another_windows_on_stop_save_moves_an_untouched_control() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalClose(CloseWindow::Tray));
    wineo_core::library::set_desktop(
        &paths,
        &Desktop {
            on_stop: OnStop::Show,
            ..Desktop::default()
        },
    )
    .unwrap();
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, Texts::read_global(&paths)),
    );
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    let form = &global_editor(&page).form;
    assert_eq!(form.draft.desktop.on_stop, OnStop::Show);
    assert_eq!(form.draft.desktop.close, CloseWindow::Tray);

    // A control the user changed keeps the edit.
    let mut page = global_page(&paths);
    assert_eq!(desktop(&page).on_stop, OnStop::Show);
    let _ = page.update(&paths, Event::GlobalOnStop(OnStop::Restore));
    wineo_core::library::set_desktop(&paths, &Desktop::default()).unwrap();
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, Texts::read_global(&paths)),
    );
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    let form = &global_editor(&page).form;
    assert_eq!(form.draft.desktop.on_stop, OnStop::Restore);
    assert_eq!(form.loaded.desktop.on_stop, OnStop::Restore);
}
