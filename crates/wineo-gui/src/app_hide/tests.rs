//! The hide state machine, without windows: what each start does to the
//! window, and what brings it back.

use std::collections::BTreeSet;

use iced::window;
use wineo_core::launch::StartOutcome;
use wineo_core::library::{CloseWindow, Desktop, OnStart, OnStop};

use crate::panel::PanelError;
use crate::sort_control::tests::run_counting;
use crate::tray::Event;
use crate::tray::tests::{example, with_tray};
use crate::{App, Message};
use wineo_core::runtime::State;

/// The example library with the tray up and Hide to the tray on.
fn hiding() -> App {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.on_start = OnStart::Hide;
    app
}

fn started(app: &mut App, name: &str, outcome: StartOutcome) {
    let _ = app.update(Message::StartFinished(name.into(), Ok(outcome)));
}

/// A tray Play that goes through, answered with `outcome`.
fn tray_play(app: &mut App, name: &str, outcome: StartOutcome) {
    let _ = app.update(Message::Tray(Event::Play(name.into())));
    assert!(app.operation.is_some(), "{name} should start");
    started(app, name, outcome);
}

/// Play in the window on `name`'s panel, answered with `outcome`.
fn window_play(app: &mut App, name: &str, outcome: StartOutcome) {
    let _ = app.update(Message::OpenCard(name.into()));
    let _ = app.update(Message::Play);
    assert!(app.operation.is_some(), "{name} should start");
    started(app, name, outcome);
}

/// A reading from the light poll as it stands now.
fn polled(app: &mut App, running: &[&str]) -> iced::Task<Message> {
    let generation = app.tray.poll_generation;
    let running: BTreeSet<String> = running.iter().map(|&name| name.to_owned()).collect();
    app.update(Message::TrayPolled(generation, running))
}

/// A rescan or reload that finds `name` in `state`.
fn loaded(app: &mut App, name: &str, state: State) {
    let mut cards = app.cards.clone();
    cards
        .iter_mut()
        .find(|card| card.name == name)
        .unwrap()
        .state = state;
    let _ = app.update(Message::Loaded(Ok(cards)));
}

/// Always show the window, on the example library with the window open
/// and focused, as when the user is looking at it.
fn showing() -> App {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.on_stop = OnStop::Show;
    app.focused = true;
    assert!(app.main_window.is_some());
    app
}

fn hidden_for(app: &App) -> Vec<&str> {
    app.tray.hidden_for.iter().map(String::as_str).collect()
}

#[test]
fn a_start_from_the_window_hides_it_and_records_the_environment() {
    let mut app = hiding();
    let generation = app.tray.poll_generation;
    window_play(&mut app, "stardew", StartOutcome::Started);
    assert_eq!(app.main_window, None);
    assert!(!app.focused);
    assert!(!app.exiting && !app.closing);
    assert_eq!(hidden_for(&app), ["stardew"]);
    assert_eq!(app.tray.poll_generation, generation + 1);
}

#[test]
fn a_start_from_the_trays_play_hides_the_window() {
    let mut app = hiding();
    tray_play(&mut app, "notepad", StartOutcome::Started);
    assert_eq!(app.main_window, None);
    assert_eq!(hidden_for(&app), ["notepad"]);
}

#[test]
fn only_a_start_wineo_runs_itself_hides() {
    for outcome in [
        StartOutcome::AlreadyRunning,
        StartOutcome::SteamAlreadyRunning,
        StartOutcome::SteamRequested,
        StartOutcome::SteamAdopted,
    ] {
        let mut app = hiding();
        let main = app.main_window;
        window_play(&mut app, "stardew", outcome);
        assert_eq!(app.main_window, main, "{outcome:?}");
        assert!(app.tray.hidden_for.is_empty(), "{outcome:?}");

        let mut app = hiding();
        let main = app.main_window;
        tray_play(&mut app, "stardew", outcome);
        assert_eq!(app.main_window, main, "{outcome:?}");
        assert!(app.tray.hidden_for.is_empty(), "{outcome:?}");
    }
}

#[test]
fn a_failed_start_leaves_the_window() {
    let mut app = hiding();
    let main = app.main_window;
    let _ = app.update(Message::OpenCard("stardew".into()));
    let _ = app.update(Message::Play);
    let _ = app.update(Message::StartFinished(
        "stardew".into(),
        Err(PanelError {
            text: "it would not start".into(),
            cleanup: false,
            output: None,
        }),
    ));
    assert_eq!(app.main_window, main);
    assert!(app.tray.hidden_for.is_empty());
    assert!(app.panel_error.is_some());
}

#[test]
fn keep_the_window_open_is_the_default_and_never_hides() {
    let mut app = example();
    with_tray(&mut app);
    assert_eq!(app.desktop.on_start, OnStart::Keep);
    let main = app.main_window;
    window_play(&mut app, "stardew", StartOutcome::Started);
    assert_eq!(app.main_window, main);
    assert!(app.tray.hidden_for.is_empty());
}

#[test]
fn a_canceled_start_that_still_started_leaves_the_window() {
    let mut app = hiding();
    let main = app.main_window;
    let _ = app.update(Message::OpenCard("stardew".into()));
    let _ = app.update(Message::Play);
    let _ = app.update(Message::CancelStart);
    started(&mut app, "stardew", StartOutcome::Started);
    assert_eq!(app.main_window, main);
    assert!(app.tray.hidden_for.is_empty());
}

#[test]
fn a_start_while_hidden_adds_to_the_set_and_stays_hidden() {
    let mut app = hiding();
    window_play(&mut app, "stardew", StartOutcome::Started);
    let generation = app.tray.poll_generation;
    tray_play(&mut app, "notepad", StartOutcome::Started);
    assert_eq!(app.main_window, None);
    assert_eq!(hidden_for(&app), ["notepad", "stardew"]);
    assert_eq!(app.tray.poll_generation, generation + 1);

    // An answer that isn't a start of its own adds nothing.
    tray_play(&mut app, "celeste", StartOutcome::SteamRequested);
    assert_eq!(app.main_window, None);
    assert_eq!(hidden_for(&app), ["notepad", "stardew"]);
}

#[test]
fn the_window_comes_back_once_everything_it_hid_for_has_stopped() {
    let mut app = hiding();
    let _ = app.update(Message::OpenCard("celeste".into()));
    let size = iced::Size::new(900.0, 700.0);
    let _ = app.update(app.window_event(window::Event::Resized(size)));
    app.scroll_offset = 140.0;
    let _ = app.update(Message::Play);
    started(&mut app, "celeste", StartOutcome::Started);
    tray_play(&mut app, "notepad", StartOutcome::Started);
    assert_eq!(app.main_window, None);

    // Both run, then one stops: still hidden.
    let _ = polled(&mut app, &["celeste", "hollow", "notepad"]);
    assert_eq!(app.main_window, None);
    let _ = polled(&mut app, &["hollow", "notepad"]);
    assert_eq!(app.main_window, None);
    assert_eq!(hidden_for(&app), ["celeste", "notepad"]);

    // The last one stops. Something it didn't hide for still runs.
    let _ = polled(&mut app, &["hollow"]);
    assert!(app.main_window.is_some());
    assert!(app.focused);
    assert!(app.tray.hidden_for.is_empty());
    assert_eq!(app.window_size, size);
    assert_eq!(app.scroll_offset, 140.0);
    assert_eq!(app.selected.as_deref(), Some("celeste"));
}

#[test]
fn a_run_over_before_the_first_reading_still_brings_the_window_back() {
    let mut app = hiding();
    window_play(&mut app, "stardew", StartOutcome::Started);
    // The restarted poll's first reading arrives even though it never saw
    // stardew run.
    let _ = polled(&mut app, &[]);
    assert!(app.main_window.is_some());
}

#[test]
fn a_reading_from_before_the_hide_changes_nothing() {
    let mut app = hiding();
    let before = app.tray.poll_generation;
    window_play(&mut app, "stardew", StartOutcome::Started);
    let _ = app.update(Message::TrayPolled(before, BTreeSet::new()));
    assert_eq!(app.main_window, None);
    assert_eq!(hidden_for(&app), ["stardew"]);
}

#[test]
fn the_users_open_ends_the_hide() {
    for open in [Event::Open, Event::Add] {
        let mut app = hiding();
        window_play(&mut app, "stardew", StartOutcome::Started);
        let _ = app.update(Message::Tray(open.clone()));
        assert!(app.main_window.is_some(), "{open:?}");
        assert!(app.tray.hidden_for.is_empty(), "{open:?}");

        // The game stopping changes nothing.
        let main = app.main_window;
        let _ = polled(&mut app, &[]);
        assert_eq!(app.main_window, main, "{open:?}");
        // Add opened its chooser, which would block the next Play.
        app.flow = None;

        // The next start hides as usual.
        window_play(&mut app, "notepad", StartOutcome::Started);
        assert_eq!(app.main_window, None, "{open:?}");
        assert_eq!(hidden_for(&app), ["notepad"], "{open:?}");
    }
}

#[test]
fn a_blocked_tray_play_while_hidden_is_the_users_open() {
    let mut app = hiding();
    window_play(&mut app, "stardew", StartOutcome::Started);
    // Another start in flight blocks the next Play, which shows the card.
    let _ = app.update(Message::Tray(Event::Play("notepad".into())));
    let _ = app.update(Message::Tray(Event::Play("celeste".into())));
    assert!(app.main_window.is_some());
    assert!(app.tray.hidden_for.is_empty());
    assert_eq!(app.selected.as_deref(), Some("celeste"));
}

#[test]
fn losing_the_tray_while_hidden_brings_the_window_back_at_once() {
    let mut app = hiding();
    window_play(&mut app, "stardew", StartOutcome::Started);
    let _ = app.update(Message::Tray(Event::Available(false)));
    assert!(app.main_window.is_some());
    assert!(app.tray.hidden_for.is_empty());
    assert!(!app.tray_shown());

    // A tray that comes back doesn't hide it again.
    let main = app.main_window;
    let _ = app.update(Message::Tray(Event::Available(true)));
    assert_eq!(app.main_window, main);
}

#[test]
fn losing_the_tray_brings_back_a_window_keep_running_closed() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);
    let _ = app.update(Message::Tray(Event::Available(false)));
    assert!(app.main_window.is_some());
}

#[test]
fn losing_the_tray_with_the_window_up_leaves_it() {
    let mut app = hiding();
    let main = app.main_window;
    let _ = app.update(Message::Tray(Event::Available(false)));
    assert_eq!(app.main_window, main);
}

#[test]
fn with_no_tray_at_the_start_the_window_stays() {
    // The host is gone, the icon is off, or there's no tray service.
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
            on_start: OnStart::Hide,
            ..Desktop::default()
        };
        let main = app.main_window;
        window_play(&mut app, "stardew", StartOutcome::Started);
        assert_eq!(app.main_window, main, "{service} {tray} {available}");
        assert!(app.tray.hidden_for.is_empty());
    }
}

#[test]
fn a_cli_start_seen_only_through_the_poll_hides_nothing() {
    let mut app = hiding();
    let main = app.main_window;
    let _ = app.update(app.window_event(window::Event::Unfocused));
    let _ = polled(&mut app, &["hollow", "stardew"]);
    assert_eq!(app.main_window, main);
    assert!(app.tray.hidden_for.is_empty());
    let _ = polled(&mut app, &[]);
    assert_eq!(app.main_window, main);
}

#[test]
fn a_cli_stop_of_what_the_window_hid_for_brings_it_back() {
    let mut app = hiding();
    tray_play(&mut app, "stardew", StartOutcome::Started);
    // A CLI start doesn't join the set, so its run keeps nothing hidden.
    let _ = polled(&mut app, &["stardew", "notepad"]);
    assert_eq!(app.main_window, None);
    let _ = polled(&mut app, &["notepad"]);
    assert!(app.main_window.is_some());
}

/// The window closed by the user with Keep running, the way a close
/// request does it.
fn closed_by_the_user(app: &mut App) {
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);
    assert!(app.tray.hidden_for.is_empty());
}

#[test]
fn a_start_while_keep_running_has_the_window_closed_leaves_it_closed() {
    let mut app = hiding();
    assert_eq!(app.desktop.on_stop, OnStop::Restore);
    closed_by_the_user(&mut app);
    let generation = app.tray.poll_generation;
    tray_play(&mut app, "stardew", StartOutcome::Started);
    assert_eq!(app.main_window, None);
    assert!(app.tray.hidden_for.is_empty());
    assert_eq!(app.tray.poll_generation, generation);
    let _ = polled(&mut app, &["stardew"]);
    let _ = polled(&mut app, &[]);
    assert_eq!(app.main_window, None);
}

#[test]
fn with_always_show_a_start_while_keep_running_has_the_window_closed_joins_the_set() {
    let mut app = hiding();
    app.desktop.on_stop = OnStop::Show;
    closed_by_the_user(&mut app);
    tray_play(&mut app, "stardew", StartOutcome::Started);
    assert_eq!(app.main_window, None);
    assert_eq!(hidden_for(&app), ["stardew"]);
    let _ = polled(&mut app, &[]);
    assert!(app.main_window.is_some());
    assert!(app.tray.hidden_for.is_empty());
}

#[test]
fn always_show_brings_back_a_user_closed_window_with_the_window_kept_open() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.on_stop = OnStop::Show;
    assert_eq!(app.desktop.on_start, OnStart::Keep);
    closed_by_the_user(&mut app);
    let generation = app.tray.poll_generation;
    tray_play(&mut app, "stardew", StartOutcome::Started);
    assert_eq!(app.main_window, None);
    assert_eq!(hidden_for(&app), ["stardew"]);
    assert_eq!(app.tray.poll_generation, generation + 1);

    // It waits for everything started from the app.
    tray_play(&mut app, "notepad", StartOutcome::Started);
    let _ = polled(&mut app, &["notepad"]);
    assert_eq!(app.main_window, None);
    let _ = polled(&mut app, &[]);
    assert!(app.main_window.is_some());
    assert!(app.focused);
    assert!(app.tray.hidden_for.is_empty());
}

#[test]
fn restore_with_the_window_kept_open_never_joins() {
    let mut app = example();
    with_tray(&mut app);
    assert_eq!(app.desktop.on_stop, OnStop::Restore);
    let main = app.main_window;
    window_play(&mut app, "stardew", StartOutcome::Started);
    tray_play(&mut app, "notepad", StartOutcome::Started);
    assert_eq!(app.main_window, main);
    assert!(app.tray.hidden_for.is_empty());
    // Closing it mid-run leaves it closed when the run stops.
    closed_by_the_user(&mut app);
    let _ = polled(&mut app, &[]);
    assert_eq!(app.main_window, None);
}

#[test]
fn always_show_opens_a_window_the_user_closed_mid_run() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.on_stop = OnStop::Show;
    let main = app.main_window;
    window_play(&mut app, "stardew", StartOutcome::Started);
    // The window stays up for a start, but the run is remembered.
    assert_eq!(app.main_window, main);
    assert_eq!(hidden_for(&app), ["stardew"]);
    // The rescan sees it run, so it's still running when the window closes.
    loaded(&mut app, "stardew", State::Running);
    assert_eq!(hidden_for(&app), ["stardew"]);

    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);
    let _ = polled(&mut app, &["stardew"]);
    assert_eq!(app.main_window, None);
    let _ = polled(&mut app, &[]);
    assert!(app.main_window.is_some());
    assert!(app.focused);
    assert!(app.tray.hidden_for.is_empty());
}

#[test]
fn always_show_leaves_a_window_that_was_open_the_whole_time() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.on_stop = OnStop::Show;
    let main = app.main_window;
    window_play(&mut app, "stardew", StartOutcome::Started);
    tray_play(&mut app, "notepad", StartOutcome::Started);
    assert_eq!(app.main_window, main);
    let _ = polled(&mut app, &[]);
    assert_eq!(app.main_window, main);
    assert!(app.tray.hidden_for.is_empty());
}

#[test]
fn a_window_a_start_hid_comes_back_under_either_stop_choice() {
    for on_stop in OnStop::ALL {
        let mut app = hiding();
        app.desktop.on_stop = on_stop;
        window_play(&mut app, "stardew", StartOutcome::Started);
        assert_eq!(app.main_window, None, "{on_stop:?}");
        // A start while it's hidden joins the set under either choice.
        tray_play(&mut app, "notepad", StartOutcome::Started);
        assert_eq!(hidden_for(&app), ["notepad", "stardew"], "{on_stop:?}");
        let _ = polled(&mut app, &["notepad"]);
        assert_eq!(app.main_window, None, "{on_stop:?}");
        let _ = polled(&mut app, &[]);
        assert!(app.main_window.is_some(), "{on_stop:?}");
    }
}

#[test]
fn the_users_open_ends_a_return_always_show_was_waiting_for() {
    let mut app = hiding();
    app.desktop.on_stop = OnStop::Show;
    closed_by_the_user(&mut app);
    tray_play(&mut app, "stardew", StartOutcome::Started);
    assert_eq!(hidden_for(&app), ["stardew"]);
    let _ = app.update(Message::Tray(Event::Open));
    assert!(app.main_window.is_some());
    assert!(app.tray.hidden_for.is_empty());
    let main = app.main_window;
    let _ = polled(&mut app, &[]);
    assert_eq!(app.main_window, main);
}

#[test]
fn losing_the_tray_brings_back_a_window_always_show_was_waiting_for() {
    let mut app = hiding();
    app.desktop.on_stop = OnStop::Show;
    closed_by_the_user(&mut app);
    tray_play(&mut app, "stardew", StartOutcome::Started);
    let _ = app.update(Message::Tray(Event::Available(false)));
    assert!(app.main_window.is_some());
    assert!(app.tray.hidden_for.is_empty());
}

#[test]
fn losing_the_tray_brings_back_a_user_closed_window_with_restore() {
    let mut app = hiding();
    closed_by_the_user(&mut app);
    tray_play(&mut app, "stardew", StartOutcome::Started);
    let _ = app.update(Message::Tray(Event::Available(false)));
    assert!(app.main_window.is_some());
}

#[test]
fn always_show_without_a_tray_joins_nothing() {
    let mut app = example();
    app.tray.service = true;
    app.tray.available = false;
    app.desktop = Desktop {
        close: CloseWindow::Tray,
        on_stop: OnStop::Show,
        ..Desktop::default()
    };
    app.main_window = None;
    started(&mut app, "stardew", StartOutcome::Started);
    assert!(app.tray.hidden_for.is_empty());
}

#[test]
fn only_a_start_wineo_runs_itself_joins_a_window_always_show_waits_for() {
    for outcome in [
        StartOutcome::AlreadyRunning,
        StartOutcome::SteamAlreadyRunning,
        StartOutcome::SteamRequested,
        StartOutcome::SteamAdopted,
    ] {
        let mut app = hiding();
        app.desktop.on_stop = OnStop::Show;
        closed_by_the_user(&mut app);
        tray_play(&mut app, "stardew", outcome);
        assert!(app.tray.hidden_for.is_empty(), "{outcome:?}");
    }
}

#[test]
fn a_start_with_the_close_question_up_leaves_the_window() {
    let mut app = hiding();
    let _ = app.update(Message::OpenCard("stardew".into()));
    let _ = app.update(Message::Play);
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert!(app.close_prompt);
    let main = app.main_window;
    started(&mut app, "stardew", StartOutcome::Started);
    assert_eq!(app.main_window, main);
    assert!(app.tray.hidden_for.is_empty());
}

#[test]
fn a_hidden_windows_last_presses_are_ignored() {
    let mut app = hiding();
    window_play(&mut app, "stardew", StartOutcome::Started);
    let _ = app.update(Message::Pressed);
    assert!(!app.focused);
}

#[test]
fn keep_running_still_quits_with_no_tray_host_while_hide_is_on() {
    let mut app = hiding();
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(Message::Tray(Event::Available(false)));
    let close = app.update(app.window_event(window::Event::CloseRequested));
    assert!(app.exiting);
    assert!(run_counting(close, false, 0, |_| false).exits);
}

#[test]
fn coming_back_shows_a_tray_stops_held_error() {
    let mut app = hiding();
    // No panel is open, so nothing else claims the window when it's back.
    tray_play(&mut app, "stardew", StartOutcome::Started);
    let _ = app.update(Message::Tray(Event::Stop("hollow".into())));
    assert_eq!(app.main_window, None);
    let _ = app.update(Message::StopFinished(
        "hollow".into(),
        Err(PanelError {
            text: "it would not stop".into(),
            cleanup: false,
            output: None,
        }),
    ));
    assert!(app.tray.held_error.is_some());
    let _ = polled(&mut app, &[]);
    assert!(app.main_window.is_some());
    assert!(app.tray.held_error.is_none());
    assert_eq!(app.selected.as_deref(), Some("hollow"));
    assert_eq!(
        app.panel_error.as_ref().map(|error| error.text.as_str()),
        Some("it would not stop")
    );
}

/// Plays `stardew` in the open window, and has the rescan see it run.
fn playing_in_the_window(app: &mut App) {
    window_play(app, "stardew", StartOutcome::Started);
    loaded(app, "stardew", State::Running);
    assert_eq!(hidden_for(app), ["stardew"]);
    assert!(app.main_window.is_some() && app.focused);
}

#[test]
fn a_run_that_stops_in_the_window_does_not_reopen_it_after_a_close() {
    // Through the window's Stop, and through a rescan that finds it stopped.
    for by_stop in [true, false] {
        let mut app = showing();
        playing_in_the_window(&mut app);
        if by_stop {
            let _ = app.update(Message::StopFinished("stardew".into(), Ok(Vec::new())));
        } else {
            loaded(&mut app, "stardew", State::Inactive);
        }
        assert!(app.tray.hidden_for.is_empty(), "{by_stop}");
        let main = app.main_window;
        let _ = polled(&mut app, &[]);
        assert_eq!(app.main_window, main, "{by_stop}");

        closed_by_the_user(&mut app);
        // The poll starts once the window is gone, and its first reading
        // finds nothing running.
        let _ = polled(&mut app, &[]);
        assert_eq!(app.main_window, None, "{by_stop}");
    }
}

#[test]
fn an_entry_no_card_vouches_for_is_dropped_at_the_close() {
    let mut app = showing();
    playing_in_the_window(&mut app);
    app.tray.hidden_for.clear();
    app.tray.hidden_for.insert("gone".into());
    closed_by_the_user(&mut app);
    let _ = polled(&mut app, &[]);
    assert_eq!(app.main_window, None);
}

#[test]
fn a_failed_stop_keeps_what_is_still_running_pending() {
    let mut app = showing();
    playing_in_the_window(&mut app);
    let _ = app.update(Message::StopFinished(
        "stardew".into(),
        Err(PanelError {
            text: "it would not stop".into(),
            cleanup: false,
            output: None,
        }),
    ));
    assert_eq!(hidden_for(&app), ["stardew"]);
}

#[test]
fn a_run_still_going_at_the_close_brings_the_window_back_when_it_stops() {
    let mut app = showing();
    playing_in_the_window(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);
    assert_eq!(hidden_for(&app), ["stardew"]);
    let _ = polled(&mut app, &["stardew"]);
    assert_eq!(app.main_window, None);
    let _ = polled(&mut app, &[]);
    assert!(app.main_window.is_some());
}

#[test]
fn switching_to_restore_with_a_run_pending_leaves_the_closed_window_closed() {
    let mut app = showing();
    playing_in_the_window(&mut app);
    // Another window saved Bring back a hidden window meanwhile.
    app.desktop.on_stop = OnStop::Restore;
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);
    assert!(app.tray.hidden_for.is_empty());
    let _ = polled(&mut app, &["stardew"]);
    let _ = polled(&mut app, &[]);
    assert_eq!(app.main_window, None);

    // And with it stopped before the close.
    let mut app = showing();
    playing_in_the_window(&mut app);
    loaded(&mut app, "stardew", State::Inactive);
    app.desktop.on_stop = OnStop::Restore;
    closed_by_the_user(&mut app);
    let _ = polled(&mut app, &[]);
    assert_eq!(app.main_window, None);
}

#[test]
fn a_stop_from_the_tray_while_hidden_leaves_the_set_to_the_poll() {
    let mut app = showing();
    closed_by_the_user(&mut app);
    tray_play(&mut app, "stardew", StartOutcome::Started);
    let _ = app.update(Message::StopFinished("stardew".into(), Ok(Vec::new())));
    assert_eq!(hidden_for(&app), ["stardew"]);
    let _ = polled(&mut app, &[]);
    assert!(app.main_window.is_some());
}

#[test]
fn always_show_joins_nothing_while_the_close_question_closing_or_exiting() {
    for flag in ["prompt", "closing", "exiting"] {
        for open in [true, false] {
            let mut app = showing();
            if !open {
                closed_by_the_user(&mut app);
            }
            match flag {
                "prompt" => app.close_prompt = true,
                "closing" => app.closing = true,
                _ => app.exiting = true,
            }
            started(&mut app, "stardew", StartOutcome::Started);
            assert!(app.tray.hidden_for.is_empty(), "{flag} {open}");
        }
    }
}

#[test]
fn a_close_request_arriving_after_a_hide_leaves_what_it_hid_for() {
    let mut app = hiding();
    assert_eq!(app.desktop.on_stop, OnStop::Restore);
    app.desktop.close = CloseWindow::Tray;
    // The request was queued for the window before the hide took it.
    let queued = Message::Window(app.main_window.unwrap(), window::Event::CloseRequested);
    window_play(&mut app, "stardew", StartOutcome::Started);
    assert_eq!(app.main_window, None);
    let _ = app.update(queued);
    assert_eq!(hidden_for(&app), ["stardew"]);
    // `update` drops a message for a window that's gone, so the guard in
    // the close itself is checked directly.
    app.closing_to_the_tray();
    assert_eq!(hidden_for(&app), ["stardew"]);
    let _ = polled(&mut app, &[]);
    assert!(app.main_window.is_some());
}
