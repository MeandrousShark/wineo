use std::fs;
use std::time::Instant;

use iced::keyboard::{Key, Modifiers};
use wineo_core::runtime::State;

use super::*;
use crate::sort_control::tests::run_counting;
use crate::tests::card;

fn paths(root: &tempfile::TempDir) -> Paths {
    let home = root.path().to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
}

fn write_library(paths: &Paths, text: &str) {
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), text).unwrap();
}

fn library() -> Vec<Card> {
    let mut cards: Vec<Card> = ["alpha", "bravo", "charlie", "delta", "echo"]
        .into_iter()
        .map(|name| card(name, State::Inactive, 0))
        .collect();
    cards[1].display_title = "Bravo".to_owned();
    cards
}

/// Gives `name` a config, as an environment has.
fn make_config(paths: &Paths, name: &str) {
    let path = paths.environment_config(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "").unwrap();
}

/// An app on its own throwaway HOME with `library()` loaded.
fn loaded() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    for card in library() {
        make_config(&paths, &card.name);
    }
    let mut app = App::boot(paths).0;
    let _ = app.update(Message::Loaded(Ok(library())));
    (root, app)
}

fn shown(app: &App) -> Vec<&str> {
    app.filtered()
        .iter()
        .map(|card| card.name.as_str())
        .collect()
}

fn flag(app: &App, name: &str, flag: Flag) -> bool {
    flag.of(app.cards.iter().find(|card| card.name == name).unwrap())
}

fn toggle(app: &mut App, name: &str, flag: Flag) -> Task<Message> {
    app.update(Message::Mark(Event::Toggle(name.into(), flag)))
}

fn notice(app: &App) -> Option<&str> {
    app.feature_notice.as_ref().map(|(text, _)| text.as_str())
}

/// Runs the write in flight for `name`'s `flag` as its task would, and
/// hands the result back to the app.
fn finish(app: &mut App, name: &str, flag: Flag) -> Task<Message> {
    let pending = app.flag_writes.get(name, flag).expect("a write in flight");
    let saved = reply(app, name, pending.added, flag, pending.writing);
    app.update(saved)
}

/// The reply a write of `value` for this `name` and `added` gets from the
/// file as it is now.
fn reply(
    app: &App,
    name: &str,
    added: Option<jiff::Timestamp>,
    flag: Flag,
    value: bool,
) -> Message {
    let (result, on_disk) = save(&app.paths, &app.shown, name, added, flag, value);
    Message::Mark(Event::Saved(Saved {
        name: name.into(),
        added,
        flag,
        value,
        result,
        on_disk: Some(on_disk),
    }))
}

fn press_f(app: &mut App, modifiers: Modifiers, focused: bool, captured: bool) {
    let _ = app.update(Message::KeyWithFocus(
        Key::Character("f".into()),
        modifiers,
        if captured {
            iced::event::Status::Captured
        } else {
            iced::event::Status::Ignored
        },
        focused,
    ));
}

fn f(app: &mut App) {
    press_f(app, Modifiers::empty(), false, false);
}

fn is_saved(message: &Message) -> bool {
    matches!(message, Message::Mark(Event::Saved(..)))
}

fn saved(task: Task<Message>, writes: usize) -> Vec<Message> {
    run_counting(task, false, writes, is_saved)
        .messages
        .into_iter()
        .filter(is_saved)
        .collect()
}

// The action paths.

#[test]
fn the_labels_name_what_a_toggle_does() {
    assert_eq!(Flag::Favorite.action_label(false), "Add to favorites");
    assert_eq!(Flag::Favorite.action_label(true), "Remove from favorites");
    assert_eq!(Flag::Hidden.action_label(false), "Hide");
    assert_eq!(Flag::Hidden.action_label(true), "Unhide");
}

#[test]
fn a_context_menu_row_toggles_its_card_and_closes_the_menu() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Context("bravo".into()));
    assert_eq!(app.action_menu.as_deref(), Some("bravo"));
    let _ = app.view();
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    assert!(app.action_menu.is_none());
    assert!(flag(&app, "bravo", Flag::Favorite));
    assert!(app.flag_writes.get("bravo", Flag::Favorite).is_some());
    // The selection is untouched; a menu row doesn't select.
    assert!(app.selected.is_none());

    let _ = app.update(Message::Context("charlie".into()));
    let _ = toggle(&mut app, "charlie", Flag::Hidden);
    assert!(flag(&app, "charlie", Flag::Hidden));
    assert_eq!(shown(&app), ["alpha", "bravo", "delta", "echo"]);
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    let _ = finish(&mut app, "charlie", Flag::Hidden);
    let saved = wineo_core::library::load(&app.paths);
    assert!(saved.environments["bravo"].favorite);
    assert!(saved.environments["charlie"].hidden);
}

#[test]
fn the_panel_star_toggles_the_panel_card() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("bravo".into()));
    // A panel still sliding in takes no input.
    app.panel_motion.snap(true);
    let messages = crate::library_states::tests::press_control(&app, PANEL_FAVORITE_ID);
    assert!(matches!(
        messages.as_slice(),
        [Message::Mark(Event::Toggle(name, Flag::Favorite))] if name == "bravo"
    ));
    for message in messages {
        let _ = app.update(message);
    }
    assert!(flag(&app, "bravo", Flag::Favorite));
    // The star is drawn either way.
    let _ = app.view();
}

#[test]
fn f_toggles_the_selected_card() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("charlie".into()));
    f(&mut app);
    assert!(flag(&app, "charlie", Flag::Favorite));
    press_f(&mut app, Modifiers::SHIFT, false, false);
    assert!(!flag(&app, "charlie", Flag::Favorite));
    // The panel doesn't have to be open.
    let _ = app.update(Message::ClosePanel);
    f(&mut app);
    assert!(flag(&app, "charlie", Flag::Favorite));
}

#[test]
fn f_does_nothing_unless_the_selected_card_is_shown_and_the_keys_are_free() {
    let (_root, mut app) = loaded();
    let favorite = |app: &App| flag(app, "charlie", Flag::Favorite);

    f(&mut app);
    assert!(app.flag_writes.get("charlie", Flag::Favorite).is_none());
    assert!(!app.flag_writes.pending(), "nothing selected");

    let _ = app.update(Message::Select("charlie".into()));
    // The search box has the keys, so f is typed.
    press_f(&mut app, Modifiers::empty(), true, true);
    assert!(!favorite(&app));
    // Another control took it.
    press_f(&mut app, Modifiers::empty(), false, true);
    assert!(!favorite(&app));
    // Ctrl+F focuses the search; Alt and Logo are someone else's.
    for modifiers in [Modifiers::CTRL, Modifiers::ALT, Modifiers::LOGO] {
        press_f(&mut app, modifiers, false, false);
        assert!(!favorite(&app), "{modifiers:?}");
    }

    let _ = app.update(Message::Context("alpha".into()));
    f(&mut app);
    assert!(!favorite(&app), "the context menu is open");
    let _ = app.update(Message::CloseMenu);

    crate::sort_control::tests::open_sort(&mut app);
    assert!(app.sort_menu_shown());
    let _ = app.update(Message::Key(
        Key::Character("f".into()),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    f(&mut app);
    assert!(!favorite(&app), "the sort menu is open");
    let _ = app.update(Message::Sort(crate::sort_control::Event::CloseMenu));

    crate::filter_control::tests::open_filters(&mut app);
    assert!(app.filter_menu_shown());
    f(&mut app);
    assert!(!favorite(&app), "the filters popup is open");
    let _ = app.update(Message::Filter(crate::filter_control::Event::CloseMenu));

    let _ = app.update(Message::Add);
    assert!(app.flow.is_some());
    f(&mut app);
    assert!(!favorite(&app), "a dialog is open");
    app.flow = None;

    // A search that hides it keeps the selection, but F waits like Enter.
    let _ = app.update(Message::SearchChanged("alpha".into()));
    assert_eq!(app.selected.as_deref(), Some("charlie"));
    f(&mut app);
    assert!(!favorite(&app), "not shown");
    let _ = app.update(Message::SearchChanged(String::new()));
    f(&mut app);
    assert!(favorite(&app));
}

// Writes.

#[test]
fn a_toggle_writes_off_the_ui_thread_and_refreshes_the_cache() {
    let (_root, mut app) = loaded();
    let reads = app.shown.library_reads();
    let task = toggle(&mut app, "bravo", Flag::Favorite);
    // The card shows the choice at once; only the write waits.
    assert!(flag(&app, "bravo", Flag::Favorite));
    assert!(!app.paths.library_file().exists());
    let messages = saved(task, 1);
    assert!(matches!(
        messages.as_slice(),
        [Message::Mark(Event::Saved(Saved { name, flag: Flag::Favorite, value: true, result: Ok(library::Guarded::Set(true)), on_disk: Some(true), .. }))]
            if name == "bravo"
    ));
    assert_eq!(app.shown.library_reads(), reads + 1);
    for message in messages {
        assert!(saved(app.update(message), 0).is_empty());
    }
    assert!(!app.flag_writes.pending());
    assert!(flag(&app, "bravo", Flag::Favorite));
    assert!(wineo_core::library::load(&app.paths).environments["bravo"].favorite);
    assert!(app.feature_notice.is_none());
}

#[test]
fn a_failed_write_reverts_the_card_to_the_file_and_says_why() {
    let (_root, mut app) = loaded();
    // A directory where the file should be fails the write.
    fs::create_dir_all(app.paths.library_file()).unwrap();
    let reads = app.shown.library_reads();
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    assert!(flag(&app, "bravo", Flag::Favorite));
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    assert!(!flag(&app, "bravo", Flag::Favorite));
    assert_eq!(
        notice(&app),
        Some(
            "Couldn't add Bravo to favorites: failed to read ~/.config/wineo/library.toml: Is a directory (os error 21)"
        )
    );
    assert_eq!(app.shown.library_reads(), reads + 1);
    assert!(!app.flag_writes.pending());

    // A failed hide brings the card back.
    let _ = toggle(&mut app, "charlie", Flag::Hidden);
    assert!(!shown(&app).contains(&"charlie"));
    let _ = finish(&mut app, "charlie", Flag::Hidden);
    assert!(shown(&app).contains(&"charlie"));
    assert_eq!(
        notice(&app),
        Some(
            "Couldn't hide charlie: failed to read ~/.config/wineo/library.toml: Is a directory (os error 21)"
        )
    );
}

#[test]
fn a_file_broken_since_the_last_read_refuses_the_write_and_the_card_goes_back() {
    let (_root, mut app) = loaded();
    write_library(&app.paths, "favorite = true\n");
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    let _ = finish(&mut app, "alpha", Flag::Favorite);
    // The file broke after the window last read it.
    let broken = "[environments.alpha\nfavorite = true\n";
    write_library(&app.paths, broken);
    assert!(app.library_problem.is_none());
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    assert!(!flag(&app, "alpha", Flag::Favorite));
    let _ = finish(&mut app, "alpha", Flag::Favorite);
    // Reads of a broken file have no stars, so the card shows none.
    assert!(!flag(&app, "alpha", Flag::Favorite));
    assert_eq!(
        notice(&app),
        Some(
            "Couldn't remove alpha from favorites: Can't save to library.toml: it can't be read. Fix it, or repair it in Checkup."
        )
    );
    assert_eq!(
        fs::read_to_string(app.paths.library_file()).unwrap(),
        broken
    );
}

#[test]
fn a_dead_worker_reloads_the_library_rather_than_guess() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    let task = app.update(Message::Mark(Event::Saved(Saved {
        name: "alpha".into(),
        added: None,
        flag: Flag::Favorite,
        value: true,
        result: Err("worker panicked".into()),
        on_disk: None,
    })));
    assert!(app.refreshing, "a reload reads what the file has");
    drop(task);
    assert_eq!(
        notice(&app),
        Some("Couldn't add alpha to favorites: worker panicked")
    );
    assert!(!app.flag_writes.pending());
}

#[test]
fn rapid_toggles_end_with_the_last_state_written() {
    let (_root, mut app) = loaded();
    for _ in 0..4 {
        let _ = toggle(&mut app, "bravo", Flag::Favorite);
    }
    assert!(!flag(&app, "bravo", Flag::Favorite));
    // Only the first write started, and the window wants it undone.
    assert_eq!(
        app.flag_writes.get("bravo", Flag::Favorite),
        Some(Pending {
            added: None,
            writing: true,
            next: Some(false),
        })
    );
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    assert!(wineo_core::library::load(&app.paths).environments["bravo"].favorite);
    assert_eq!(
        app.flag_writes.get("bravo", Flag::Favorite),
        Some(Pending {
            added: None,
            writing: false,
            next: None,
        })
    );
    assert!(!flag(&app, "bravo", Flag::Favorite));
    // Back to what's being written: nothing more to write.
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    assert_eq!(
        app.flag_writes.get("bravo", Flag::Favorite).unwrap().next,
        None
    );
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    assert!(!app.flag_writes.pending());
    assert!(!wineo_core::library::load(&app.paths).environments["bravo"].favorite);
    assert!(!flag(&app, "bravo", Flag::Favorite));
}

#[test]
fn a_queued_toggle_is_written_through_the_returned_task() {
    let (_root, mut app) = loaded();
    let first = toggle(&mut app, "bravo", Flag::Hidden);
    let _ = toggle(&mut app, "bravo", Flag::Hidden);
    let messages = saved(first, 1);
    let mut again = Vec::new();
    for message in messages {
        again.extend(saved(app.update(message), 1));
    }
    assert_eq!(again.len(), 1);
    for message in again {
        let _ = app.update(message);
    }
    assert!(!app.flag_writes.pending());
    assert!(!wineo_core::library::load(&app.paths).environments["bravo"].hidden);
    assert!(!flag(&app, "bravo", Flag::Hidden));
}

#[test]
fn a_read_from_before_the_write_landed_keeps_the_choice_on_screen() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = app.update(Message::Loaded(Ok(library())));
    assert!(flag(&app, "bravo", Flag::Favorite));
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    assert!(flag(&app, "bravo", Flag::Favorite));
}

/// The library as the file has it once bravo is a favorite.
fn starred() -> Vec<Card> {
    let mut cards = library();
    cards[1].favorite = true;
    cards
}

#[test]
fn a_rescan_that_read_before_the_write_finished_is_dropped() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    // A rescan starts after the card changed and reads the old file.
    let stale = crate::rescan::Scan {
        revisions: app.shown.revisions(),
        library: Some(Ok(library())),
        ..Default::default()
    };
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    assert!(app.flag_writes.get("bravo", Flag::Favorite).is_none());
    let _ = app.update(Message::Rescanned(Box::new(stale)));
    assert!(flag(&app, "bravo", Flag::Favorite));

    // One that starts after the write is taken.
    let fresh = crate::rescan::Scan {
        revisions: app.shown.revisions(),
        library: Some(Ok(starred())),
        ..Default::default()
    };
    let fresh = app.shown.current(fresh);
    assert!(fresh.library.is_some());
    assert!(app.shown.changes(fresh, jiff::Timestamp::now()).is_none());
    assert!(wineo_core::library::load(&app.paths).environments["bravo"].favorite);
}

#[test]
fn a_reload_that_started_before_the_write_finished_starts_over() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = app.update(Message::Refresh);
    assert!(app.refreshing);
    let stale = app.library_generation;
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    assert_ne!(app.library_generation, stale, "a new read started");
    let _ = app.update(Message::LibraryRead(stale, Ok(library()), None));
    assert!(flag(&app, "bravo", Flag::Favorite));
    assert!(app.refreshing, "still waiting on the new read");
    let current = app.library_generation;
    let _ = app.update(Message::LibraryRead(current, Ok(starred()), None));
    assert!(!app.refreshing);
    assert!(flag(&app, "bravo", Flag::Favorite));
}

#[test]
fn a_failed_write_also_drops_older_reads() {
    let (_root, mut app) = loaded();
    fs::create_dir_all(app.paths.library_file()).unwrap();
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    // This read raced the optimistic star and saw it.
    let stale = crate::rescan::Scan {
        revisions: app.shown.revisions(),
        library: Some(Ok(starred())),
        ..Default::default()
    };
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    assert!(!flag(&app, "bravo", Flag::Favorite));
    let _ = app.update(Message::Rescanned(Box::new(stale)));
    assert!(!flag(&app, "bravo", Flag::Favorite));
}

#[test]
fn a_quiet_rescan_after_a_write_reports_nothing() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = toggle(&mut app, "delta", Flag::Hidden);
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    let _ = finish(&mut app, "delta", Flag::Hidden);
    let mut disk = library();
    disk[1].favorite = true;
    disk[3].hidden = true;
    let scan = crate::rescan::Scan {
        revisions: app.shown.revisions(),
        library: Some(Ok(disk.clone())),
        ..Default::default()
    };
    let scan = app.shown.current(scan);
    assert!(scan.library.is_some());
    assert!(app.shown.changes(scan, jiff::Timestamp::now()).is_none());
    let rebuilds = app.order.rebuilds;
    let _ = app.update(Message::Loaded(Ok(disk)));
    assert_eq!(app.order.rebuilds, rebuilds);
    let _ = app.view();
    assert_eq!(app.order.misses.get(), 0);
}

#[test]
fn every_change_goes_through_the_order_rebuild() {
    let (_root, mut app) = loaded();
    let rebuilds = app.order.rebuilds;
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    assert_eq!(app.order.rebuilds, rebuilds + 1);
    let _ = toggle(&mut app, "charlie", Flag::Hidden);
    assert_eq!(app.order.rebuilds, rebuilds + 2);
    assert_eq!(app.order.counts(app.order_inputs()).hidden, 1);
    let _ = app.view();
    assert_eq!(app.order.misses.get(), 0);
}

#[test]
fn closing_waits_for_a_pending_favorite() {
    let (_root, mut app) = loaded();
    let first = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let asked = Instant::now();
    let close = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(app.closing && app.close_quietly);
    assert!(!run_counting(close, false, 0, is_saved).exits);
    let due = app.close_wake.due().expect("the wait is bounded");
    assert!(due >= asked + crate::app_window::SORT_SAVE_WAIT);

    let messages = saved(first, 1);
    let mut done = Vec::new();
    for message in messages {
        done.push(app.update(message));
    }
    assert!(run_counting(Task::batch(done), false, 0, is_saved).exits);
    assert!(wineo_core::library::load(&app.paths).environments["bravo"].favorite);
}

#[test]
fn closing_waits_for_the_queued_toggle_too() {
    let (_root, mut app) = loaded();
    let first = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    let mut again = Vec::new();
    for message in saved(first, 1) {
        let ran = run_counting(app.update(message), false, 1, is_saved);
        assert!(!ran.exits, "the last state isn't written yet");
        again.extend(ran.messages.into_iter().filter(is_saved));
    }
    let mut done = Vec::new();
    for message in again {
        done.push(app.update(message));
    }
    assert!(run_counting(Task::batch(done), false, 0, is_saved).exits);
    assert!(!wineo_core::library::load(&app.paths).environments["bravo"].favorite);
}

#[test]
fn a_write_finishing_after_exit_changes_nothing() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let close = || Message::Window(iced::window::Event::CloseRequested);
    let _ = app.update(close());
    let _ = app.update(close());
    assert!(app.exiting);
    let task = finish(&mut app, "bravo", Flag::Favorite);
    assert!(saved(task, 0).is_empty(), "no rewrite after exit");
    assert!(!app.flag_writes.pending());
}

// Broken file.

#[test]
fn with_the_banner_showing_every_action_is_refused_and_nothing_changes() {
    let (_root, mut app) = loaded();
    let broken = "[environments.alpha\nfavorite = true\n";
    write_library(&app.paths, broken);
    app.problem_read(wineo_core::library::load(&app.paths).problem);
    assert!(app.library_banner().is_some());
    let refused = "Can't save to library.toml: it can't be read. Fix it, or repair it in Checkup.";

    let _ = app.update(Message::Context("bravo".into()));
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    assert_eq!(
        notice(&app),
        Some(format!("Couldn't add Bravo to favorites: {refused}").as_str())
    );
    let _ = toggle(&mut app, "charlie", Flag::Hidden);
    assert_eq!(
        notice(&app),
        Some(format!("Couldn't hide charlie: {refused}").as_str())
    );
    let _ = app.update(Message::Select("delta".into()));
    app.panel_motion.snap(true);
    let messages = crate::library_states::tests::press_control(&app, PANEL_FAVORITE_ID);
    assert_eq!(messages.len(), 1);
    for message in messages {
        let _ = app.update(message);
    }
    f(&mut app);
    assert_eq!(
        notice(&app),
        Some(format!("Couldn't add delta to favorites: {refused}").as_str())
    );

    assert!(!app.flag_writes.pending());
    assert!(app.cards.iter().all(|card| !card.favorite && !card.hidden));
    assert_eq!(shown(&app).len(), 5);
    assert_eq!(
        fs::read_to_string(app.paths.library_file()).unwrap(),
        broken
    );
}

#[test]
fn a_file_from_a_newer_wineo_refuses_with_its_own_message() {
    let (_root, mut app) = loaded();
    write_library(&app.paths, "schema_version = 2\n");
    app.problem_read(wineo_core::library::load(&app.paths).problem);
    let _ = toggle(&mut app, "alpha", Flag::Hidden);
    assert_eq!(
        notice(&app),
        Some(
            "Couldn't hide alpha: library.toml was written by a newer wineo; update wineo to use it."
        )
    );
    assert!(!flag(&app, "alpha", Flag::Hidden));
    assert!(!app.flag_writes.pending());
}

// Selection.

#[test]
fn hiding_the_selected_card_selects_the_one_in_its_place() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("charlie".into()));
    let _ = toggle(&mut app, "charlie", Flag::Hidden);
    assert_eq!(shown(&app), ["alpha", "bravo", "delta", "echo"]);
    assert_eq!(app.selected.as_deref(), Some("delta"));
    assert!(app.panel_open);

    // The last card hands over to the new last one.
    let _ = app.update(Message::Select("echo".into()));
    let _ = toggle(&mut app, "echo", Flag::Hidden);
    assert_eq!(app.selected.as_deref(), Some("delta"));
    assert!(app.panel_open);

    for name in ["alpha", "bravo", "delta"] {
        let _ = app.update(Message::Context(name.into()));
        let _ = toggle(&mut app, name, Flag::Hidden);
    }
    // Nothing left: no selection and no panel.
    assert!(shown(&app).is_empty());
    assert!(app.selected.is_none());
    assert!(!app.panel_open);
}

#[test]
fn hiding_another_card_leaves_the_selection_alone() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("charlie".into()));
    let _ = toggle(&mut app, "alpha", Flag::Hidden);
    assert_eq!(app.selected.as_deref(), Some("charlie"));
    assert!(app.panel_open);
}

#[test]
fn hiding_with_show_hidden_on_keeps_the_card_shown_and_selected() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Filter(
        crate::filter_control::Event::ToggleShowHidden,
    ));
    assert!(app.filters.show_hidden);
    let _ = app.update(Message::Select("charlie".into()));
    let _ = toggle(&mut app, "charlie", Flag::Hidden);
    assert_eq!(shown(&app).len(), 5);
    assert_eq!(app.selected.as_deref(), Some("charlie"));
    assert!(app.panel_open);
    // It can be unhidden from there.
    let _ = toggle(&mut app, "charlie", Flag::Hidden);
    assert!(!flag(&app, "charlie", Flag::Hidden));
    assert_eq!(app.selected.as_deref(), Some("charlie"));
}

#[test]
fn favoriting_resorts_only_with_favorites_first_and_keeps_the_selection() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("delta".into()));
    // Off, the order stays and nothing scrolls.
    let task = toggle(&mut app, "delta", Flag::Favorite);
    assert_eq!(shown(&app), ["alpha", "bravo", "charlie", "delta", "echo"]);
    assert!(run_counting(task, true, 1, is_saved).operations.is_empty());

    let _ = app.update(Message::Sort(
        crate::sort_control::Event::ToggleFavoritesFirst,
    ));
    assert_eq!(shown(&app)[0], "delta");
    let task = toggle(&mut app, "echo", Flag::Favorite);
    assert_eq!(shown(&app)[..2], ["delta", "echo"]);
    assert_eq!(app.selected.as_deref(), Some("delta"));
    assert!(app.panel_open);
    // The selected card lands where it's already in view, so nothing
    // scrolls.
    assert!(run_counting(task, true, 1, is_saved).operations.is_empty());
}

// The window's own view.

#[test]
fn no_action_writes_the_window_view() {
    let (_root, mut app) = loaded();
    // Another window saved a view after this one opened.
    let theirs = "[view]\nsort = \"playtime\"\ndescending = true\n";
    write_library(&app.paths, theirs);
    let query = app.query;
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    let _ = toggle(&mut app, "charlie", Flag::Hidden);
    let _ = finish(&mut app, "charlie", Flag::Hidden);
    let _ = app.update(Message::Select("delta".into()));
    f(&mut app);
    let _ = finish(&mut app, "delta", Flag::Favorite);
    assert_eq!(app.query, query);
    assert!(!app.view_writes.pending());
    let text = fs::read_to_string(app.paths.library_file()).unwrap();
    assert!(text.contains(theirs), "{text}");
    assert_eq!(text.matches("[view]").count(), 1);
    let saved = wineo_core::library::load(&app.paths);
    assert_eq!(saved.view.sort, wineo_core::library::SortKey::Playtime);
    assert!(saved.view.descending);
}

#[test]
fn a_first_write_creates_no_view() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    let text = fs::read_to_string(app.paths.library_file()).unwrap();
    assert!(!text.contains("[view]"), "{text}");
}

// Drawing.

#[test]
fn posters_mark_favorites_and_hidden_cards() {
    let (_root, mut app) = loaded();
    let plain = card("plain", State::Inactive, 0);
    assert!(poster_marks(&plain, 1.0).is_none());
    let mut starred = plain.clone();
    starred.favorite = true;
    assert!(poster_marks(&starred, 1.0).is_some());
    let mut both = starred.clone();
    both.hidden = true;
    assert!(poster_marks(&both, 1.0).is_some());

    // A marked card lays out at the same height as a plain one.
    use crate::layout_probe::layout;
    let size = iced::Size::new(400.0, 900.0);
    let height = |card: &Card| {
        let app = crate::tests::app();
        let mut view = app.card(card);
        layout(&mut view, size).bounds().height
    };
    assert_eq!(height(&both), height(&plain));
    assert_eq!(
        height(&both),
        crate::grid::card_height(&both, crate::grid_motion::GridLayout::default())
    );

    let _ = app.update(Message::Filter(
        crate::filter_control::Event::ToggleShowHidden,
    ));
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    let _ = toggle(&mut app, "bravo", Flag::Hidden);
    let _ = app.view();
}

// Environments that leave, or come back under the same name.

fn without(name: &str) -> Vec<Card> {
    library()
        .into_iter()
        .filter(|card| card.name != name)
        .collect()
}

/// What `rm` leaves behind: no config and no entry.
fn remove(app: &App, name: &str) {
    fs::remove_file(app.paths.environment_config(name)).unwrap();
    wineo_core::library::remove(&app.paths, name).unwrap();
}

fn entry(app: &App, name: &str) -> Option<wineo_core::library::Entry> {
    wineo_core::library::load(&app.paths)
        .environments
        .get(name)
        .cloned()
}

#[test]
fn a_card_that_leaves_drops_its_write_and_ignores_the_late_reply() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    remove(&app, "alpha");
    let _ = app.update(Message::Loaded(Ok(without("alpha"))));
    assert!(!app.flag_writes.pending(), "nothing left to wait for");
    // The write had landed before `rm`; its reply changes nothing and
    // starts nothing.
    let late = Message::Mark(Event::Saved(Saved {
        name: "alpha".into(),
        added: None,
        flag: Flag::Favorite,
        value: true,
        result: Ok(library::Guarded::Set(true)),
        on_disk: Some(true),
    }));
    assert!(saved(app.update(late), 0).is_empty());
    assert!(app.feature_notice.is_none());
    assert!(!app.refreshing);
    assert_eq!(entry(&app, "alpha"), None);
}

#[test]
fn a_write_for_a_removed_environment_recreates_nothing() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "alpha", Flag::Hidden);
    // A second toggle waits behind the first.
    let _ = toggle(&mut app, "alpha", Flag::Hidden);
    // `rm` ran before the write reached the file, and before any read
    // told the window.
    remove(&app, "alpha");
    let task = finish(&mut app, "alpha", Flag::Hidden);
    assert_eq!(entry(&app, "alpha"), None, "no stray entry");
    assert!(!app.flag_writes.pending());
    assert!(app.feature_notice.is_none());
    assert!(app.refreshing, "a reload shows it gone");
    // The queued toggle never started.
    assert!(saved(task, 0).is_empty());
}

#[test]
fn a_queued_toggle_never_starts_once_its_card_is_gone() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    let reply = reply(&app, "alpha", None, Flag::Favorite, true);
    remove(&app, "alpha");
    let _ = app.update(Message::Loaded(Ok(without("alpha"))));
    assert!(saved(app.update(reply), 0).is_empty());
    assert!(!app.flag_writes.pending());
    assert_eq!(entry(&app, "alpha"), None);
}

#[test]
fn a_recreated_environment_gets_none_of_the_old_choice() {
    let (_root, mut app) = loaded();
    let first = toggle(&mut app, "alpha", Flag::Favorite);
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    // `rm`, then `new alpha`, before the write ran: the new one has a fresh
    // entry stamped with when it was made.
    remove(&app, "alpha");
    make_config(&app.paths, "alpha");
    wineo_core::library::fresh(&app.paths, "alpha").unwrap();
    let added = entry(&app, "alpha").unwrap().added;
    assert!(added.is_some());
    let fresh = entry(&app, "alpha");

    // The old write reaches the file now and is refused.
    let messages = saved(first, 1);
    assert!(matches!(
        messages.as_slice(),
        [Message::Mark(Event::Saved(Saved {
            result: Ok(library::Guarded::Gone),
            ..
        }))]
    ));
    assert_eq!(entry(&app, "alpha"), fresh);

    // The window learns of the new one before the reply arrives.
    let mut cards = library();
    cards[0].added = added;
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert!(!flag(&app, "alpha", Flag::Favorite));
    assert!(!app.flag_writes.pending());
    for message in messages {
        assert!(saved(app.update(message), 0).is_empty());
    }
    assert!(app.feature_notice.is_none());
    assert_eq!(entry(&app, "alpha"), fresh);

    // The new one can be starred as usual.
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    let _ = finish(&mut app, "alpha", Flag::Favorite);
    let now = entry(&app, "alpha").unwrap();
    assert!(now.favorite);
    assert_eq!(now.added, added);
}

#[test]
fn a_reply_for_the_old_environment_is_ignored_by_the_new_one() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    // Recreated, seen by a read, and starred again, all before the old
    // write's reply.
    remove(&app, "alpha");
    make_config(&app.paths, "alpha");
    wineo_core::library::fresh(&app.paths, "alpha").unwrap();
    let added = entry(&app, "alpha").unwrap().added;
    let mut cards = library();
    cards[0].added = added;
    let _ = app.update(Message::Loaded(Ok(cards)));
    let _ = toggle(&mut app, "alpha", Flag::Favorite);
    assert_eq!(
        app.flag_writes.get("alpha", Flag::Favorite).unwrap().added,
        added
    );
    let old = Message::Mark(Event::Saved(Saved {
        name: "alpha".into(),
        added: None,
        flag: Flag::Favorite,
        value: true,
        result: Err("failed".into()),
        on_disk: Some(false),
    }));
    let _ = app.update(old);
    // The new write is still awaited, and the old failure isn't shown.
    assert!(app.flag_writes.get("alpha", Flag::Favorite).is_some());
    assert!(flag(&app, "alpha", Flag::Favorite));
    assert!(app.feature_notice.is_none());
    let _ = finish(&mut app, "alpha", Flag::Favorite);
    assert!(entry(&app, "alpha").unwrap().favorite);
}

#[test]
fn enter_does_nothing_while_the_context_menu_is_open() {
    let (_root, mut app) = loaded();
    let enter = |app: &mut App| {
        let _ = app.update(Message::KeyWithFocus(
            Key::Named(iced::keyboard::key::Named::Enter),
            Modifiers::empty(),
            iced::event::Status::Ignored,
            false,
        ));
    };
    // Closed panel: Enter would open it.
    let _ = app.update(Message::Select("bravo".into()));
    let _ = app.update(Message::ClosePanel);
    let _ = app.update(Message::Context("charlie".into()));
    enter(&mut app);
    assert!(!app.panel_open);
    assert_eq!(app.selected.as_deref(), Some("bravo"));
    assert_eq!(app.action_menu.as_deref(), Some("charlie"));

    // Open panel: Enter would launch.
    let _ = app.update(Message::CloseMenu);
    enter(&mut app);
    assert!(
        app.panel_open,
        "Enter opens the panel once the menu is closed"
    );
    let _ = app.update(Message::Context("charlie".into()));
    enter(&mut app);
    assert!(app.operation.is_none());
    assert!(app.panel_open);
    assert_eq!(app.selected.as_deref(), Some("bravo"));
}

#[test]
fn a_dropped_rescan_still_delivers_its_other_changes_next_time() {
    let (_root, mut app) = loaded();
    let _ = toggle(&mut app, "bravo", Flag::Favorite);
    // A rescan that started before the write finished also found a new
    // environment and a broken library.toml.
    let problem = wineo_core::library::Problem {
        reason: "expected `]`".into(),
        line: Some(1),
        newer_version: false,
    };
    let mut found = library();
    found.push(card("foxtrot", State::Inactive, 0));
    let stale = crate::rescan::Scan {
        revisions: app.shown.revisions(),
        library: Some(Ok(found.clone())),
        problem: Some(Some(problem.clone())),
        ..Default::default()
    };
    let _ = finish(&mut app, "bravo", Flag::Favorite);
    let _ = app.update(Message::Rescanned(Box::new(stale)));
    assert!(!shown(&app).contains(&"foxtrot"));
    assert!(app.library_problem.is_none());

    // The next scan reads the same things and they still count as changes.
    found[1].favorite = true;
    let next = crate::rescan::Scan {
        revisions: app.shown.revisions(),
        library: Some(Ok(found)),
        problem: Some(Some(problem.clone())),
        ..Default::default()
    };
    let next = app.shown.current(next);
    let next = app
        .shown
        .changes(next, jiff::Timestamp::now())
        .expect("the new environment and the banner are still news");
    let _ = app.update(Message::Rescanned(Box::new(next)));
    assert!(shown(&app).contains(&"foxtrot"));
    assert!(flag(&app, "bravo", Flag::Favorite));
    assert_eq!(app.library_problem, Some(problem));
    assert!(app.library_banner().is_some());
}
