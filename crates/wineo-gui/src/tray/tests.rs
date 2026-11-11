use std::ffi::OsString;

use iced::window;
use jiff::Timestamp;
use wineo_core::library::{CloseWindow, Desktop};
use wineo_core::paths::Paths;
use wineo_core::runtime::State;

use super::*;
use crate::library::Card;
use crate::panel::PanelError;
use crate::sort_control::tests::run_counting;
use crate::tests::card;
use crate::{App, Message, OperationKind};

pub(crate) mod bus;

fn at(second: i64) -> Option<Timestamp> {
    Some(Timestamp::from_second(second).unwrap())
}

fn played(name: &str, title: &str, second: i64) -> Card {
    let mut card = card(name, State::Inactive, 60);
    card.display_title = title.to_owned();
    card.last_played_at = at(second);
    card
}

fn running(name: &str, title: &str, second: i64) -> Card {
    let mut card = played(name, title, second);
    card.state = State::Running;
    card
}

fn with_cards(cards: Vec<Card>) -> App {
    let mut app = crate::tests::app();
    app.cards = cards;
    app.reorder();
    app
}

/// The plan's example library: three to play, one running, and the rest
/// passed over for a reason each.
pub(crate) fn example() -> App {
    let mut hidden = played("secret", "Secret Game", 900);
    hidden.hidden = true;
    let mut never = card("fresh", State::Inactive, 0);
    never.display_title = "Fresh Install".to_owned();
    with_cards(vec![
        played("stardew", "Stardew Valley", 500),
        played("notepad", "Notepad++", 400),
        played("celeste", "Celeste", 300),
        played("old", "Old Game", 100),
        running("hollow", "Hollow Knight", 800),
        hidden,
        never,
    ])
}

/// The tray is up and a host shows it, as `run()` and the watcher would
/// leave it.
pub(crate) fn with_tray(app: &mut App) {
    app.tray.service = true;
    app.tray.available = true;
}

fn error(text: &str) -> PanelError {
    PanelError {
        text: text.to_owned(),
        cleanup: false,
        output: None,
    }
}

fn labels(app: &App) -> Vec<String> {
    app.tray_menu().labels()
}

#[test]
fn names_are_cut_at_forty_characters_and_underscores_doubled() {
    assert_eq!(menu_text("Celeste"), "Celeste");
    assert_eq!(menu_text("snake_case_game"), "snake__case__game");
    let exact = "a".repeat(40);
    assert_eq!(menu_text(&exact), exact);
    let long = format!("{} tail", "b".repeat(40));
    let cut = menu_text(&long);
    assert_eq!(cut, format!("{}…", "b".repeat(39)));
    assert_eq!(cut.chars().count(), 40);
    // A space at the cut doesn't sit before the ellipsis.
    let spaced = format!("{} {}", "c".repeat(38), "d".repeat(10));
    assert_eq!(menu_text(&spaced), format!("{}…", "c".repeat(38)));
    // Cut first, then escaped, so the limit counts what's shown.
    let under = "_".repeat(45);
    assert_eq!(menu_text(&under), format!("{}…", "__".repeat(39)));
    // Whole characters, never half of one.
    let accents = "é".repeat(41);
    assert_eq!(menu_text(&accents), format!("{}…", "é".repeat(39)));
}

#[test]
fn the_menu_reads_as_the_plan_shows() {
    let app = example();
    assert_eq!(
        labels(&app),
        [
            "Open wineo",
            "Add to library...",
            "-",
            "Play Stardew Valley",
            "Play Notepad++",
            "Play Celeste",
            "-",
            "Stop Hollow Knight",
            "-",
            "Quit",
        ]
    );
    let entries = app.tray_menu().entries();
    assert!(entries.contains(&Entry::Item(
        "Play Stardew Valley".into(),
        Event::Play("stardew".into())
    )));
    assert!(entries.contains(&Entry::Item(
        "Stop Hollow Knight".into(),
        Event::Stop("hollow".into())
    )));
    assert_eq!(entries[0], Entry::Item(OPEN.into(), Event::Open));
    assert_eq!(entries[1], Entry::Item(ADD.into(), Event::Add));
    assert_eq!(entries.last(), Some(&Entry::Item(QUIT.into(), Event::Quit)));
}

#[test]
fn an_empty_library_leaves_only_the_fixed_rows() {
    let app = with_cards(Vec::new());
    assert_eq!(
        labels(&app),
        ["Open wineo", "Add to library...", "-", "Quit"]
    );
    // Nothing runs, so the Stop section is gone too.
    let app = with_cards(vec![played("celeste", "Celeste", 300)]);
    assert_eq!(
        labels(&app),
        [
            "Open wineo",
            "Add to library...",
            "-",
            "Play Celeste",
            "-",
            "Quit"
        ]
    );
    let app = with_cards(vec![running("hollow", "Hollow Knight", 0)]);
    assert_eq!(
        labels(&app),
        [
            "Open wineo",
            "Add to library...",
            "-",
            "Stop Hollow Knight",
            "-",
            "Quit"
        ]
    );
}

#[test]
fn play_rows_skip_hidden_trashed_running_and_never_played() {
    let mut app = example();
    let play: Vec<String> = app
        .tray_menu()
        .play
        .into_iter()
        .map(|row| row.name)
        .collect();
    assert_eq!(play, ["stardew", "notepad", "celeste"]);

    // Showing hidden games in the library doesn't put one in the tray.
    app.filters.show_hidden = true;
    app.reorder();
    assert!(!app.tray_menu().play.iter().any(|row| row.name == "secret"));

    // A game just moved to the trash keeps its card until the reload.
    let root = tempfile::tempdir().unwrap();
    let paths = temp_paths(&root);
    let config = paths.environment_config("stardew");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "kind = 'native'\n").unwrap();
    let entry = wineo_core::trash::remove(&paths, "stardew", false).unwrap();
    app.toast = Some(crate::removal::Toast::new(entry));
    let play: Vec<String> = app
        .tray_menu()
        .play
        .into_iter()
        .map(|row| row.name)
        .collect();
    assert_eq!(play, ["notepad", "celeste", "old"]);
}

#[test]
fn play_rows_refill_when_one_starts() {
    let mut app = example();
    let _ = app.start_named("stardew".into());
    assert!(app.operation.as_ref().is_some_and(|operation| {
        operation.name == "stardew" && operation.kind == OperationKind::Start
    }));
    let menu = app.tray_menu();
    let play: Vec<&str> = menu.play.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(play, ["notepad", "celeste", "old"]);

    // Once it runs, it moves to the Stop rows, in the poll's word too.
    app.operation = None;
    app.tray.polled = Some(["hollow".to_owned(), "stardew".to_owned()].into());
    let menu = app.tray_menu();
    let play: Vec<&str> = menu.play.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(play, ["notepad", "celeste", "old"]);
    let stop: Vec<&str> = menu.stop.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(stop, ["hollow", "stardew"]);
}

#[test]
fn stop_rows_follow_the_library_sort_and_stop_at_eight() {
    let mut cards: Vec<Card> = (0..10)
        .map(|index| running(&format!("r{index:02}"), &format!("Run {index:02}"), 0))
        .collect();
    cards[3].hidden = true;
    let mut app = with_cards(cards);
    let menu = app.tray_menu();
    let stop: Vec<&str> = menu.stop.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(
        stop,
        ["r00", "r01", "r02", "r03", "r04", "r05", "r06", "r07"]
    );
    assert!(menu.more);
    let labels = menu.labels();
    let more = labels.iter().position(|label| label == MORE).unwrap();
    assert_eq!(labels[more - 1], "Stop Run 07");
    assert_eq!(labels[more + 1], "-");
    assert!(
        menu.entries()
            .contains(&Entry::Item(MORE.to_owned(), Event::Open))
    );

    app.query.descending = true;
    let stop: Vec<String> = app
        .tray_menu()
        .stop
        .into_iter()
        .map(|row| row.name)
        .collect();
    assert_eq!(stop[0], "r09");
    assert_eq!(stop[7], "r02");

    // A search or filter narrows the library, not the tray.
    app.query.descending = false;
    app.search = "r09".to_owned();
    app.reorder();
    assert_eq!(app.tray_menu().stop.len(), STOP_ROWS);

    app.cards.truncate(8);
    assert!(!app.tray_menu().more);
}

fn temp_paths(root: &tempfile::TempDir) -> Paths {
    let home = root.path().join("home");
    Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
        .unwrap()
}

#[test]
fn the_poll_stands_in_for_the_cards_and_rereads_a_stopped_play_time() {
    let root = tempfile::tempdir().unwrap();
    let paths = temp_paths(&root);
    let config = paths.environment_config("hollow");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "schema_version = 1\nkind = 'native'\n").unwrap();
    wineo_core::stats::append(
        &paths.stats_log("hollow"),
        &wineo_core::stats::Session::new(
            "game".into(),
            Timestamp::from_second(4_000).unwrap(),
            Timestamp::from_second(5_000).unwrap(),
            wineo_core::stats::End::Exited,
        ),
    )
    .unwrap();
    // Whatever core calls the last play time, the cards use it too.
    let ended = last_played(&paths, "hollow").unwrap();
    let (mut app, _) = App::boot(paths);
    app.cards = example().cards;
    app.reorder();
    let before = app.tray_menu();
    assert_eq!(before.stop.len(), 1);

    // The poll no longer sees it run, so it's read again.
    let task = app.update(Message::TrayPolled(0, BTreeSet::new()));
    assert!(app.tray_menu().stop.is_empty());
    let ran = run_counting(task, false, 1, |message| {
        matches!(message, Message::TrayPlayed(..))
    });
    let read: Vec<&Message> = ran
        .messages
        .iter()
        .filter(|message| matches!(message, Message::TrayPlayed(..)))
        .collect();
    assert!(
        matches!(read[..], [Message::TrayPlayed(name, Some(when))] if name == "hollow" && *when == ended),
        "{read:?}"
    );
    let _ = app.update(Message::TrayPlayed("hollow".into(), Some(ended)));
    let play: Vec<String> = app
        .tray_menu()
        .play
        .into_iter()
        .map(|row| row.name)
        .collect();
    assert_eq!(play, ["hollow", "stardew", "notepad"]);

    // The same set again stops nothing.
    let quiet = app.tray_polled(0, BTreeSet::new());
    assert!(iced_runtime::task::into_stream(quiet).is_none());

    // A full read is newer than both.
    let cards = app.cards.clone();
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert_eq!(app.tray.polled, None);
    assert!(app.tray.played.is_empty());
    assert_eq!(app.tray_menu(), before);
}

#[test]
fn the_menu_reaches_the_service_only_when_it_changes() {
    let mut app = example();
    let mut receiver = app.tray.menu.subscribe();
    app.tray.service = true;
    let _ = app.update(Message::Refresh);
    assert!(receiver.has_changed().unwrap());
    assert_eq!(*receiver.borrow_and_update(), app.tray_menu());
    let _ = app.update(Message::Refresh);
    assert!(!receiver.has_changed().unwrap());
    app.cards[0].hidden = true;
    let _ = app.update(Message::Refresh);
    assert!(receiver.has_changed().unwrap());
}

#[test]
fn a_tray_play_starts_through_the_same_slot_without_touching_the_panel() {
    let mut app = example();
    let _ = app.update(Message::OpenCard("celeste".into()));
    app.panel_error = Some(error("celeste's own error"));
    let _ = app.update(Message::Tray(Event::Play("stardew".into())));
    assert!(app.operation.as_ref().is_some_and(|operation| {
        operation.name == "stardew" && operation.kind == OperationKind::Start
    }));
    assert_eq!(app.selected.as_deref(), Some("celeste"));
    assert!(app.panel_error.is_some());
}

#[test]
fn a_blocked_tray_play_shows_its_card_with_the_panel_open() {
    let mut app = example();
    let _ = app.start_named("stardew".into());
    let _ = app.update(Message::Tray(Event::Play("notepad".into())));
    assert_eq!(app.operation.as_ref().unwrap().name, "stardew");
    assert_eq!(app.selected.as_deref(), Some("notepad"));
    assert!(app.panel_open);
    assert!(app.feature_notice.is_none());

    // Asking again for the card already open leaves its panel open.
    let _ = app.update(Message::Tray(Event::Play("notepad".into())));
    assert!(app.panel_open);
}

#[test]
fn a_tray_play_with_no_default_target_waits_for_the_panel() {
    let mut choice = played("suite", "Office Suite", 700);
    choice.targets = vec!["writer".into(), "calc".into()];
    choice.default_target = None;
    let mut app = with_cards(vec![choice]);
    let _ = app.update(Message::Tray(Event::Play("suite".into())));
    assert!(app.operation.is_none());
    assert_eq!(app.selected.as_deref(), Some("suite"));
    assert!(app.panel_open);
    // With the target picked there, Play from the tray goes through.
    let _ = app.update(Message::TargetPicked("calc".into()));
    let _ = app.update(Message::Tray(Event::Play("suite".into())));
    assert!(app.operation.is_some());
}

#[test]
fn a_blocked_tray_click_over_a_page_keeps_the_page_with_a_notice() {
    let mut app = example();
    let _ = app.start_named("stardew".into());
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    let _ = app.update(Message::Tray(Event::Play("notepad".into())));
    assert_eq!(app.section, crate::sidebar::Section::Trash);
    assert_eq!(app.selected, None);
    assert_eq!(
        app.feature_notice.as_ref().map(|(text, _)| text.as_str()),
        Some(PLAY_BLOCKED)
    );
    let _ = app.update(Message::Tray(Event::Stop("hollow".into())));
    assert_eq!(
        app.feature_notice.as_ref().map(|(text, _)| text.as_str()),
        Some(STOP_BLOCKED)
    );
    assert_eq!(app.operation.as_ref().unwrap().name, "stardew");

    // A flow is never thrown away either.
    let mut app = example();
    let _ = app.update(Message::Add);
    assert!(app.flow.is_some());
    let _ = app.start_named("stardew".into());
    let _ = app.update(Message::Tray(Event::Play("notepad".into())));
    assert!(app.flow.is_some());
    assert_eq!(app.selected, None);
}

#[test]
fn a_play_that_can_start_goes_through_over_a_page() {
    let mut app = example();
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    let _ = app.update(Message::Tray(Event::Play("stardew".into())));
    assert!(app.operation.is_some());
    assert_eq!(app.section, crate::sidebar::Section::Trash);
    assert!(app.feature_notice.is_none());
}

#[test]
fn a_tray_stop_is_a_normal_stop_through_the_same_slot() {
    let mut app = example();
    let _ = app.update(Message::Tray(Event::Stop("hollow".into())));
    let operation = app.operation.as_ref().unwrap();
    assert_eq!(
        (operation.name.as_str(), operation.kind),
        ("hollow", OperationKind::Stop)
    );
    assert!(app.force_stop_at.is_some());
    assert_eq!(app.tray.stopping.as_deref(), Some("hollow"));
    assert_eq!(app.selected, None);

    // While it runs, another Stop shows its card instead.
    let mut second = running("other", "Other Game", 0);
    second.display_title = "Other Game".into();
    app.cards.push(second);
    app.reorder();
    let _ = app.update(Message::Tray(Event::Stop("other".into())));
    assert_eq!(app.operation.as_ref().unwrap().name, "hollow");
    assert_eq!(app.selected.as_deref(), Some("other"));
    assert!(app.panel_open);
}

#[test]
fn a_tray_stop_leaves_hooks_to_the_panel() {
    let mut app = example();
    let card = app
        .cards
        .iter_mut()
        .find(|card| card.name == "hollow")
        .unwrap();
    card.busy = true;
    card.busy_holder = Some(wineo_core::operation::Operation::Hook(
        wineo_core::hooks::Phase::After,
    ));
    let _ = app.update(Message::Tray(Event::Stop("hollow".into())));
    assert!(app.operation.is_none());
    assert!(app.hook_stop_confirm.is_none());
    assert_eq!(app.selected.as_deref(), Some("hollow"));
    assert!(app.panel_open);
}

#[test]
fn a_tray_stops_error_waits_until_the_window_shows() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);
    let _ = app.update(Message::Tray(Event::Stop("hollow".into())));
    let _ = app.update(Message::StopFinished(
        "hollow".into(),
        Err(error("it would not stop")),
    ));
    assert!(app.operation.is_none());
    assert_eq!(app.tray.stopping, None);
    assert!(app.panel_error.is_none());
    assert!(app.tray.held_error.is_some());

    let _ = app.update(Message::Tray(Event::Open));
    assert!(app.main_window.is_some());
    assert_eq!(app.selected.as_deref(), Some("hollow"));
    assert!(app.panel_open);
    assert_eq!(
        app.panel_error.as_ref().map(|error| error.text.as_str()),
        Some("it would not stop")
    );
    assert!(app.tray.held_error.is_none());
}

#[test]
fn a_tray_stops_error_shows_at_once_in_an_open_panel() {
    let mut app = example();
    let _ = app.update(Message::OpenCard("hollow".into()));
    let _ = app.update(Message::Tray(Event::Stop("hollow".into())));
    let _ = app.update(Message::StopFinished(
        "hollow".into(),
        Err(error("it would not stop")),
    ));
    assert!(app.panel_error.is_some());
    assert!(app.tray.held_error.is_none());
}

#[test]
fn a_held_error_waits_out_a_page_and_a_new_stop_replaces_it() {
    let mut app = example();
    let _ = app.update(Message::Tray(Event::Stop("hollow".into())));
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    let _ = app.update(Message::StopFinished(
        "hollow".into(),
        Err(error("it would not stop")),
    ));
    let _ = app.update(Message::Tray(Event::Open));
    assert_eq!(app.section, crate::sidebar::Section::Trash);
    assert!(app.tray.held_error.is_some());

    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    let _ = app.update(Message::Tray(Event::Stop("hollow".into())));
    assert!(app.tray.held_error.is_none());
}

#[test]
fn keep_running_hides_the_window_and_open_brings_it_back_where_it_was() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(Message::OpenCard("celeste".into()));
    let first = app.main_window.unwrap();
    let size = iced::Size::new(900.0, 700.0);
    let _ = app.update(app.window_event(window::Event::Resized(size)));
    let hide = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);
    assert!(!app.focused);
    assert!(!app.exiting && !app.closing && !app.close_prompt);
    assert!(!run_counting(hide, false, 0, |_| false).exits);

    // Its last presses still arrive, and don't count as focus.
    let _ = app.update(Message::Pressed);
    assert!(!app.focused);
    // Nor do events from the window that's gone.
    let _ = app.update(Message::Window(first, window::Event::Focused));
    assert!(!app.focused);

    let _ = app.update(Message::Tray(Event::Open));
    let second = app.main_window.unwrap();
    assert_ne!(second, first);
    assert!(app.focused);
    assert_eq!(app.window_size, size);
    assert_eq!(app.selected.as_deref(), Some("celeste"));
    assert!(app.panel_open);
}

#[test]
fn a_window_hidden_in_the_tray_gets_no_minimum_update_and_reopens_with_the_current_one() {
    use iced_runtime::window::Action as Window;
    let mut app = example();
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);

    // The size changes, however it changes, with no window to tell.
    app.appearance.interface_size = 150;
    let task = app.update(Message::Pressed);
    if let Some(stream) = iced_runtime::task::into_stream(task) {
        use iced::futures::StreamExt;
        let actions = iced::futures::executor::block_on(stream.collect::<Vec<_>>());
        assert!(
            !actions.iter().any(|action| matches!(
                action,
                iced_runtime::Action::Window(Window::SetMinSize(..))
            )),
            "no window, no update"
        );
    }

    let task = app.update(Message::Tray(Event::Open));
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let mut opened = None;
    while let Some(action) =
        iced::futures::executor::block_on(iced::futures::StreamExt::next(&mut stream))
    {
        if let iced_runtime::Action::Window(Window::Open(id, settings, _)) = action {
            opened = Some((id, settings));
            break;
        }
    }
    let (id, settings) = opened.expect("the tray's Open opens the window");
    assert_eq!(Some(id), app.main_window);
    assert_eq!(settings.min_size, Some(iced::Size::new(600.0, 450.0)));
}

#[test]
fn open_raises_a_window_that_is_already_up() {
    let mut app = example();
    let main = app.main_window.unwrap();
    let _ = app.update(Message::Tray(Event::Open));
    assert_eq!(app.main_window, Some(main));
}

#[test]
fn keep_running_hides_over_foreground_work_without_asking() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let _ = app.start_named("stardew".into());
    assert!(app.foreground_busy());
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);
    assert!(!app.close_prompt && !app.closing);
    assert!(app.operation.is_some());
}

#[test]
fn keep_running_quits_anyway_without_a_tray_to_come_back_from() {
    for (service, tray, available) in [
        (true, true, false),
        (true, false, true),
        (false, true, true),
    ] {
        let mut app = example();
        app.tray.service = service;
        app.tray.available = available;
        app.desktop = Desktop {
            tray,
            close: CloseWindow::Tray,
            ..Desktop::default()
        };
        let close = app.update(app.window_event(window::Event::CloseRequested));
        assert!(app.exiting, "{service} {tray} {available}");
        assert!(run_counting(close, false, 0, |_| false).exits);
    }
}

#[test]
fn quit_is_the_default_close() {
    let mut app = example();
    with_tray(&mut app);
    assert_eq!(app.desktop.close, CloseWindow::Quit);
    let close = app.update(app.window_event(window::Event::CloseRequested));
    assert!(app.exiting);
    assert!(run_counting(close, false, 0, |_| false).exits);
}

#[test]
fn the_trays_quit_always_quits() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let quit = app.update(Message::Tray(Event::Quit));
    assert!(app.exiting);
    assert!(run_counting(quit, false, 0, |_| false).exits);
}

#[test]
fn the_trays_quit_asks_over_foreground_work_in_the_window() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let _ = app.start_named("stardew".into());
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);
    let _ = app.update(Message::Tray(Event::Quit));
    assert!(app.close_prompt);
    assert!(!app.exiting);
    assert!(app.main_window.is_some());
}

#[test]
fn add_opens_the_chooser_and_leaves_a_flow_alone() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    let _ = app.update(Message::Tray(Event::Add));
    assert!(app.main_window.is_some());
    assert!(matches!(app.flow, Some(ref page) if page.rows().is_some()));

    let mut app = example();
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    let _ = app.update(Message::Tray(Event::Add));
    assert!(app.flow.is_none());
    assert_eq!(app.section, crate::sidebar::Section::Trash);
}

#[test]
fn the_host_coming_and_going_is_live_state() {
    let mut app = example();
    app.tray.service = true;
    app.desktop.close = CloseWindow::Tray;
    assert!(!app.tray_shown());
    let _ = app.update(Message::Tray(Event::Available(true)));
    assert!(app.tray_shown() && app.keeps_running());
    let _ = app.update(Message::Tray(Event::Available(false)));
    assert!(!app.keeps_running());
}

#[test]
fn tray_clicks_drop_while_the_app_exits() {
    let mut app = example();
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert!(app.exiting);
    let _ = app.update(Message::Tray(Event::Play("stardew".into())));
    assert!(app.operation.is_none());
}

#[test]
fn the_pixmaps_are_32_and_64_px_argb() {
    let icons = crate::tray_icons();
    let sizes: Vec<(i32, i32)> = icons.iter().map(|icon| (icon.width, icon.height)).collect();
    assert_eq!(sizes, [(32, 32), (64, 64)]);
    for icon in &icons {
        assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
        // Alpha leads, and the art isn't see-through all over.
        assert!(icon.data.chunks(4).any(|pixel| pixel[0] == 255));
        assert!(icon.data.chunks(4).any(|pixel| pixel[0] == 0));
    }
}

#[test]
fn nothing_in_process_reaches_d_bus() {
    let app = crate::tests::app();
    assert!(!app.tray.service);
}

fn hold(app: &mut App, name: &str) {
    app.tray.held_error = Some((name.to_owned(), error("it would not stop")));
}

fn shown_error(app: &App) -> Option<&str> {
    app.panel_error.as_ref().map(|error| error.text.as_str())
}

#[test]
fn a_held_error_waits_through_a_blocked_click_on_another_card() {
    let mut app = example();
    let _ = app.start_named("stardew".into());
    hold(&mut app, "hollow");
    let _ = app.update(Message::Tray(Event::Play("notepad".into())));
    assert_eq!(app.selected.as_deref(), Some("notepad"));
    assert_eq!(shown_error(&app), None);
    assert!(app.tray.held_error.is_some());
    // Whatever opens its card's panel shows it, a click on the card too.
    let _ = app.update(Message::OpenCard("hollow".into()));
    assert_eq!(shown_error(&app), Some("it would not stop"));
    assert!(app.tray.held_error.is_none());
}

#[test]
fn a_blocked_click_on_the_held_cards_open_panel_shows_its_error() {
    let mut app = example();
    let _ = app.start_named("stardew".into());
    let _ = app.update(Message::OpenCard("hollow".into()));
    hold(&mut app, "hollow");
    let _ = app.update(Message::Tray(Event::Stop("hollow".into())));
    assert_eq!(shown_error(&app), Some("it would not stop"));
}

#[test]
fn a_held_error_waits_through_add_and_the_flow() {
    let mut app = example();
    hold(&mut app, "hollow");
    let _ = app.update(Message::Tray(Event::Add));
    assert!(app.flow.is_some());
    assert!(app.tray.held_error.is_some());
    // Leaving the chooser.
    let _ = app.flow_update(app.flow_generation, crate::flow::Event::Back);
    assert!(app.flow.is_none());
    assert!(app.tray.held_error.is_some());
    let _ = app.update(Message::OpenCard("hollow".into()));
    assert_eq!(shown_error(&app), Some("it would not stop"));

    // Finishing it, which opens the new card's panel instead.
    let mut app = example();
    hold(&mut app, "hollow");
    let _ = app.update(Message::Tray(Event::Add));
    let mut cards = app.cards.clone();
    cards.push(card("new", State::Inactive, 0));
    // As the flow's last step does before its read comes back.
    app.flow = None;
    let _ = app.flow_returned(
        crate::flow::Action::Library {
            name: Some("new".into()),
            launch: false,
        },
        Ok(cards),
    );
    assert_eq!(app.selected.as_deref(), Some("new"));
    assert_eq!(shown_error(&app), None);
    assert!(app.tray.held_error.is_some());
    let _ = app.update(Message::OpenCard("hollow".into()));
    assert_eq!(shown_error(&app), Some("it would not stop"));
}

#[test]
fn a_held_error_drops_with_its_card() {
    let mut app = example();
    hold(&mut app, "hollow");
    let cards: Vec<Card> = app
        .cards
        .iter()
        .filter(|card| card.name != "hollow")
        .cloned()
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert!(app.tray.held_error.is_none());

    // Gone from the cards some other way, it drops when the window shows.
    let mut app = example();
    hold(&mut app, "hollow");
    app.cards.retain(|card| card.name != "hollow");
    app.reorder();
    let _ = app.update(Message::Tray(Event::Open));
    assert!(app.tray.held_error.is_none());
    assert_eq!(app.selected, None);
}

/// The card `example` just moved to the trash, as the Undo toast says.
fn trash(app: &mut App, root: &tempfile::TempDir, name: &str) {
    let paths = temp_paths(root);
    let config = paths.environment_config(name);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "kind = 'native'\n").unwrap();
    let entry = wineo_core::trash::remove(&paths, name, false).unwrap();
    app.toast = Some(crate::removal::Toast::new(entry));
}

#[test]
fn a_stale_click_for_a_card_that_is_gone_only_shows_the_window() {
    let root = tempfile::tempdir().unwrap();
    for event in [Event::Play("renamed".into()), Event::Stop("renamed".into())] {
        let mut app = example();
        with_tray(&mut app);
        app.desktop.close = CloseWindow::Tray;
        let _ = app.update(app.window_event(window::Event::CloseRequested));
        let _ = app.update(Message::Tray(event.clone()));
        assert!(app.main_window.is_some(), "{event:?}");
        assert!(app.operation.is_none());
        assert_eq!(app.selected, None);
        assert!(!app.panel_open);
        assert!(app.feature_notice.is_none());
    }
    // Just trashed: the card is still there until the reload.
    let mut app = example();
    trash(&mut app, &root, "stardew");
    let _ = app.update(Message::Tray(Event::Play("stardew".into())));
    assert!(app.operation.is_none());
    assert_eq!(app.selected, None);
    let mut app = example();
    trash(&mut app, &root, "hollow");
    let _ = app.update(Message::Tray(Event::Stop("hollow".into())));
    assert!(app.operation.is_none());
    assert_eq!(app.selected, None);
}

#[test]
fn a_stale_play_for_a_hidden_card_shows_it_instead_of_starting() {
    let mut app = example();
    let _ = app.update(Message::Tray(Event::Play("secret".into())));
    assert!(app.operation.is_none());
    assert_eq!(app.selected.as_deref(), Some("secret"));
    assert!(app.panel_open);
    // Its own panel's Play still starts it.
    let _ = app.update(Message::Play);
    assert!(app.operation.is_some());
}

#[test]
fn focus_coming_back_releases_a_held_error_only_into_a_free_panel() {
    // Another card's panel is open, say from a blocked tray click.
    let mut app = example();
    let _ = app.update(app.window_event(window::Event::Unfocused));
    let _ = app.update(Message::OpenCard("notepad".into()));
    hold(&mut app, "hollow");
    let _ = app.update(app.window_event(window::Event::Focused));
    assert_eq!(app.selected.as_deref(), Some("notepad"));
    assert_eq!(shown_error(&app), None);
    assert!(app.tray.held_error.is_some());

    // No panel open, so the held card's opens with it.
    let mut app = example();
    let _ = app.update(app.window_event(window::Event::Unfocused));
    hold(&mut app, "hollow");
    let _ = app.update(app.window_event(window::Event::Focused));
    assert_eq!(app.selected.as_deref(), Some("hollow"));
    assert!(app.panel_open);
    assert_eq!(shown_error(&app), Some("it would not stop"));
    assert!(app.tray.held_error.is_none());
}
