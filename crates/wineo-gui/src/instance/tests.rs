use std::collections::HashMap;
use std::ffi::OsString;

use iced::window;
use wineo_core::library::CloseWindow;

use super::*;
use crate::sort_control::tests::run_counting;
use crate::tray::tests::{example, with_tray};

mod bus;

fn paths(vars: &[(&str, &str)]) -> Paths {
    let vars: HashMap<String, OsString> = vars
        .iter()
        .map(|(key, value)| (key.to_string(), OsString::from(value)))
        .collect();
    Paths::from_lookup(|key| vars.get(key).cloned()).unwrap()
}

#[test]
fn the_default_root_has_the_plain_name() {
    assert_eq!(bus_name(&paths(&[("HOME", "/home/me")])), NAME);
    let named = paths(&[
        ("HOME", "/home/me"),
        ("XDG_DATA_HOME", "/home/me/.local/share"),
    ]);
    assert_eq!(bus_name(&named), NAME);
    // Two homes are two users, each with a bus of their own.
    assert_eq!(bus_name(&paths(&[("HOME", "/home/other")])), NAME);
}

#[test]
fn other_roots_append_a_hash_that_never_changes() {
    let portable = bus_name(&paths(&[
        ("HOME", "/home/me"),
        ("WINEO_PORTABLE_DIR", "/games/wineo"),
    ]));
    let data = bus_name(&paths(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "/games")]));
    let other = bus_name(&paths(&[
        ("HOME", "/home/me"),
        ("WINEO_PORTABLE_DIR", "/games/other"),
    ]));
    // Pinned, so a change of algorithm or input shows up here rather than as
    // two apps for one root after an update.
    assert_eq!(portable, "io.github.MeandrousShark.wineo.Gui_8174faa4c3c3");
    assert_eq!(data, "io.github.MeandrousShark.wineo.Gui_f7cda781271d");
    // Both are /games/wineo, as a portable folder and as a data dir.
    assert_ne!(portable, data);
    assert_ne!(other, portable);
    // The same root is the same name whatever the home.
    let elsewhere = paths(&[
        ("HOME", "/home/other"),
        ("WINEO_PORTABLE_DIR", "/games/wineo"),
    ]);
    assert_eq!(bus_name(&elsewhere), portable);
}

#[test]
fn every_name_is_a_valid_bus_name() {
    for vars in [
        vec![("HOME", "/home/me")],
        vec![("HOME", "/home/me"), ("WINEO_PORTABLE_DIR", "/games/wineo")],
        vec![
            ("HOME", "/home/me"),
            ("WINEO_PORTABLE_DIR", "/ünïcode/ földer/\u{1F3AE}"),
        ],
        vec![
            ("HOME", "/home/me"),
            ("XDG_DATA_HOME", "/9/starts-with-a-digit"),
        ],
    ] {
        let name = bus_name(&paths(&vars));
        zbus::names::WellKnownName::try_from(name.as_str()).unwrap();
        let last = name.rsplit('.').next().unwrap();
        assert!(last.starts_with("Gui"), "{name}");
        assert!(
            last.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "{name}"
        );
    }
}

#[test]
fn show_brings_back_a_window_hidden_for_a_start() {
    let mut app = example();
    with_tray(&mut app);
    let _ = app.update(Message::OpenCard("celeste".into()));
    app.tray.hidden_for.insert("stardew".into());
    let _ = app.hide_window();
    assert_eq!(app.main_window, None);

    let _ = app.update(Message::Show);
    assert!(app.main_window.is_some());
    assert!(app.focused);
    // A user's show ends the hide, so the game stopping changes nothing.
    assert!(app.tray.hidden_for.is_empty());
    assert_eq!(app.selected.as_deref(), Some("celeste"));
}

#[test]
fn show_brings_back_a_window_closed_with_keep_running() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let first = app.main_window.unwrap();
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);

    let _ = app.update(Message::Show);
    let second = app.main_window.unwrap();
    assert_ne!(second, first);
    assert!(!app.exiting);
}

#[test]
fn show_raises_an_open_window() {
    let mut app = example();
    let main = app.main_window;
    let _ = app.update(Message::Show);
    assert_eq!(app.main_window, main);
}

#[test]
fn show_leaves_a_flow_as_it_is() {
    let mut app = example();
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(Message::Add);
    assert!(app.flow.is_some());
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);

    let _ = app.update(Message::Show);
    assert!(app.main_window.is_some());
    assert!(app.flow.is_some());
}

#[test]
fn show_leaves_a_close_prompt_up() {
    let mut app = example();
    let _ = app.start_named("stardew".into());
    let main = app.main_window;
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert!(app.close_prompt);

    let _ = app.update(Message::Show);
    assert!(app.close_prompt);
    assert!(!app.exiting && !app.closing);
    assert_eq!(app.main_window, main);
}

#[test]
fn a_show_after_the_exit_started_does_nothing() {
    let mut app = example();
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert!(app.exiting);
    let task = app.update(Message::Show);
    assert!(!run_counting(task, false, 0, |_| false).exits);
}

/// An `Instance` with no bus behind it, and where its release requests go.
pub(crate) fn instance() -> (Instance, mpsc::UnboundedReceiver<oneshot::Sender<()>>) {
    let (release, releases) = mpsc::unbounded_channel();
    let instance = Instance {
        shows: watch::channel(0).1,
        leaving: Arc::default(),
        release,
    };
    (instance, releases)
}

#[test]
fn an_exit_refuses_shows_and_lets_the_name_go_as_it_starts() {
    let (instance, mut releases) = instance();
    let mut app = example();
    app.instance = Some(instance.clone());
    let quit = app.update(app.window_event(window::Event::CloseRequested));
    assert!(app.exiting);
    // Both happen in the update that starts the exit, before the window
    // closes and the watchdog waits.
    assert!(instance.leaving.load(Ordering::SeqCst));
    let done = releases.try_recv().expect("the release was asked for");
    let _ = done.send(());
    assert!(run_counting(quit, false, 0, |_| false).exits);
}

#[test]
fn an_exit_goes_on_when_the_bus_never_answers() {
    let (instance, mut releases) = instance();
    let mut app = example();
    app.instance = Some(instance);
    let quit = app.update(app.window_event(window::Event::CloseRequested));
    // Held and never answered, like a bus thread that's stuck.
    let _held = releases.try_recv().unwrap();
    let started = std::time::Instant::now();
    assert!(exits_within(quit, crate::app_window::LEAVE_WAIT * 3));
    assert!(started.elapsed() >= crate::app_window::LEAVE_WAIT);
}

/// Runs `task` until it exits or `patience` runs out, which `run_counting`
/// can't wait out since it stops at the first quiet half second.
fn exits_within(task: iced::Task<Message>, patience: std::time::Duration) -> bool {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return false;
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let deadline = tokio::time::Instant::now() + patience;
            while let Ok(Some(action)) = tokio::time::timeout_at(deadline, stream.next()).await {
                if matches!(action, iced_runtime::Action::Exit) {
                    return true;
                }
            }
            false
        })
}

/// Whether `instance` refuses shows and has asked once for the name to go.
fn left(instance: &Instance, releases: &mut mpsc::UnboundedReceiver<oneshot::Sender<()>>) -> bool {
    let refused = instance.leaving.load(Ordering::SeqCst);
    let asked = releases.try_recv().is_ok();
    assert!(
        releases.try_recv().is_err(),
        "the release was asked for twice"
    );
    refused && asked
}

/// A sort flip leaves a `library.toml` write pending, which a close waits on.
fn with_pending_write(app: &mut crate::App) {
    let _ = app.update(Message::Sort(crate::sort_control::Event::FlipDirection));
    assert!(app.library_writes_pending());
}

#[test]
fn a_close_waiting_on_library_writes_lets_the_name_go_and_refuses_shows() {
    let (instance, mut releases) = instance();
    let mut app = example();
    app.instance = Some(instance.clone());
    with_pending_write(&mut app);
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert!(app.closing && !app.exiting);
    // Free for a relaunch to own while this app waits to exit.
    assert!(left(&instance, &mut releases));

    let main = app.main_window;
    let _ = app.update(Message::Show);
    assert_eq!(app.main_window, main);
    assert!(app.closing);
}

#[test]
fn a_close_waiting_on_canceled_work_lets_the_name_go_and_refuses_shows() {
    let (instance, mut releases) = instance();
    let mut app = example();
    app.instance = Some(instance.clone());
    let _ = app.start_named("stardew".into());
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert!(app.close_prompt);
    // The question can still be answered with Keep working, so nothing's
    // let go yet.
    assert!(!instance.leaving.load(Ordering::SeqCst));
    assert!(releases.try_recv().is_err());

    let _ = app.update(Message::CancelAndClose);
    assert!(app.closing && !app.exiting, "the exit waits for the stop");
    assert!(left(&instance, &mut releases));
    let main = app.main_window;
    let _ = app.update(Message::Show);
    assert_eq!(app.main_window, main);
}

#[test]
fn a_quiet_close_from_the_tray_keeps_a_hidden_window_hidden_on_a_show() {
    let (instance, mut releases) = instance();
    let mut app = example();
    app.instance = Some(instance.clone());
    with_tray(&mut app);
    app.desktop.close = CloseWindow::Tray;
    let _ = app.update(app.window_event(window::Event::CloseRequested));
    assert_eq!(app.main_window, None);
    with_pending_write(&mut app);
    let _ = app.update(Message::Tray(crate::tray::Event::Quit));
    assert!(app.closing && app.close_quietly && !app.exiting);
    assert!(left(&instance, &mut releases));

    let _ = app.update(Message::Show);
    assert_eq!(app.main_window, None);
}
