use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use iced::Task;
use wineo_core::cancel::Token;
use wineo_core::paths::Paths;

use super::*;
use crate::sort_control::Event;
use crate::test_script::KillOnDrop;
use crate::{App, Message, Operation, OperationKind};

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    (root, App::boot(paths).0)
}

/// Whether running `task` asks the window to exit.
pub(crate) fn exits(task: Task<Message>) -> bool {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return false;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        while let Ok(Some(action)) =
            tokio::time::timeout(Duration::from_millis(500), stream.next()).await
        {
            if matches!(action, iced_runtime::Action::Exit) {
                return true;
            }
        }
        false
    })
}

fn close() -> Message {
    Message::Window(iced::window::Event::CloseRequested)
}

fn running(kind: OperationKind, after_hook: bool) -> Operation {
    Operation {
        name: "game".into(),
        kind,
        token: Some(Token::new()),
        cancelled: false,
        status: "Working...".into(),
        install_done: None,
        installing: false,
        after_hook,
    }
}

fn said(app: &App) -> String {
    app.watchdog
        .started()
        .expect("the watchdog started")
        .to_owned()
}

#[test]
fn the_bound_is_ten_seconds() {
    assert_eq!(EXIT_BOUND, Duration::from_secs(10));
}

#[test]
fn the_line_names_what_was_abandoned() {
    let work = |names: &[&str]| {
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };
    let bound = EXIT_BOUND;
    assert_eq!(
        abandoned_line(bound, &work(&["a library write"])),
        "wineo-gui: gave up after 10 s on a library write."
    );
    assert_eq!(
        abandoned_line(bound, &work(&["a library write", "a Heroic import"])),
        "wineo-gui: gave up after 10 s on a library write and a Heroic import."
    );
    assert_eq!(
        abandoned_line(bound, &work(&["a", "b", "c"])),
        "wineo-gui: gave up after 10 s on a, b, and c."
    );
    assert_eq!(
        abandoned_line(bound, &[]),
        "wineo-gui: gave up after 10 s on background work that isn't tracked."
    );
}

#[test]
fn the_watchdog_expires_once_after_the_bound() {
    let (sent, received) = mpsc::channel();
    let mut watchdog = Watchdog::new(Duration::from_millis(50), move |line| {
        sent.send(line.to_owned()).unwrap();
    });
    assert!(watchdog.start(&["a library write".to_owned()]));
    // A second start is ignored, so there is one thread and the first line.
    assert!(!watchdog.start(&["a tool".to_owned()]));
    let line = received.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(line.contains("a library write") && !line.contains("a tool"));
    assert_eq!(watchdog.started(), Some(line.as_str()));
    assert!(received.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn the_watchdog_stays_quiet_inside_the_bound() {
    let (sent, received) = mpsc::channel();
    let mut watchdog = Watchdog::new(Duration::from_secs(30), move |line| {
        sent.send(line.to_owned()).unwrap();
    });
    assert!(watchdog.start(&[]));
    assert!(received.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn a_close_with_nothing_tracked_still_starts_the_watchdog() {
    let (_root, mut app) = fixture();
    assert!(app.watchdog.started().is_none());
    assert!(exits(app.update(close())));
    assert_eq!(
        said(&app),
        "wineo-gui: gave up after 0 s on background work that isn't tracked."
    );
}

#[test]
fn a_stuck_library_write_is_named_when_the_wait_runs_out() {
    let (_root, mut app) = fixture();
    let _stalled = app.update(Message::Sort(Event::FlipDirection));
    assert!(app.view_writes.pending());
    assert!(!exits(app.update(close())));
    assert!(app.watchdog.started().is_none(), "still waiting");
    let over = app.update(Message::CloseWaitOver(app.close_wake.generation()));
    assert!(exits(over));
    assert!(said(&app).ends_with("on a library write."));
}

#[test]
fn a_stuck_hook_is_named_and_force_stop_closes_the_window() {
    let (_root, mut app) = fixture();
    app.operation = Some(running(OperationKind::Start, true));
    assert!(!exits(app.update(close())));
    assert!(app.close_prompt);

    // A plain cancel keeps waiting, with no watchdog running.
    assert!(!exits(app.update(Message::CancelAndClose)));
    assert!(app.closing && app.foreground_busy());
    assert!(app.watchdog.started().is_none());

    let token = app.operation.as_ref().unwrap().token.clone().unwrap();
    assert!(exits(app.update(Message::ForceCloseWork)));
    assert!(token.is_forced(), "work is canceled before the window goes");
    assert!(app.foreground_busy(), "the hook is still stuck");
    assert!(said(&app).ends_with("on the after hook of game."));
}

#[test]
fn a_stuck_start_is_named_by_its_kind() {
    for (kind, text) in [
        (OperationKind::Start, "starting game"),
        (OperationKind::Stop, "stopping game"),
    ] {
        let (_root, mut app) = fixture();
        app.operation = Some(running(kind, false));
        assert!(exits(app.update(Message::ForceCloseWork)));
        assert!(said(&app).ends_with(&format!("on {text}.")));
    }
}

#[test]
fn every_stuck_thing_is_listed_once() {
    let (_root, mut app) = fixture();
    let _stalled = app.update(Message::Sort(Event::FlipDirection));
    app.operation = Some(running(OperationKind::Start, true));
    assert!(exits(app.update(Message::ForceCloseWork)));
    assert!(said(&app).ends_with("on a library write and the after hook of game."));
}

#[test]
fn a_second_exit_keeps_the_first_watchdog() {
    let (_root, mut app) = fixture();
    app.operation = Some(running(OperationKind::Start, true));
    assert!(exits(app.update(Message::ForceCloseWork)));
    let first = said(&app);
    app.operation = None;
    assert!(exits(app.exit_now()));
    assert!(exits(app.update(Message::CancelAndClose)));
    assert_eq!(said(&app), first);
}

#[test]
fn cancel_and_close_with_nothing_waiting_starts_the_watchdog() {
    let (_root, mut app) = fixture();
    assert!(exits(app.update(Message::CancelAndClose)));
    assert!(app.watchdog.started().is_some());
}

#[test]
fn work_ending_during_a_close_exits_through_the_watchdog() {
    // The settle path: the last foreground work ended while closing.
    let (_root, mut app) = fixture();
    app.operation = Some(running(OperationKind::Start, false));
    assert!(!exits(app.update(Message::CancelAndClose)));
    let finished = app.update(Message::StartFinished(
        "game".into(),
        Err(crate::panel::PanelError {
            text: "canceled".into(),
            cleanup: false,
            output: None,
        }),
    ));
    assert!(exits(finished));
    assert!(app.watchdog.started().is_some());
}

#[test]
fn a_flow_reload_arriving_during_a_close_exits_through_the_watchdog() {
    let (_root, mut app) = fixture();
    app.closing = true;
    let task = app.flow_returned(crate::flow::Action::Choose, Ok(Vec::new()));
    assert!(exits(task));
    assert!(app.watchdog.started().is_some());
}

#[test]
fn any_update_while_closing_with_nothing_left_exits_through_the_watchdog() {
    let (_root, mut app) = fixture();
    app.closing = true;
    assert!(exits(app.update(Message::ScrollIdle(0))));
    assert!(app.watchdog.started().is_some());
}

/// Child modes for the process tests below: the test binary re-runs itself
/// with this set, so a real `process::exit` ends a child and not the suite.
const CHILD: &str = "WINEO_EXIT_WATCHDOG_CHILD";

/// Mimics iced: blocking work outlives the window, and dropping the runtime
/// waits for it. Without a watchdog the child ends with status 3 instead.
fn child(stuck: bool) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let blocking = runtime.spawn_blocking(move || {
        if stuck {
            std::thread::sleep(Duration::from_secs(20));
        }
    });
    let bound = Duration::from_millis(if stuck { 300 } else { 5_000 });
    let mut watchdog = Watchdog::new(bound, exit_process);
    watchdog.start(&["a library write".to_owned()]);
    if !stuck {
        // The work finishes in time, so dropping the runtime returns.
        runtime.block_on(blocking).unwrap();
    }
    drop(runtime);
    std::process::exit(3);
}

#[test]
fn child_process_scenario() {
    match std::env::var(CHILD).as_deref() {
        Ok("stuck") => child(true),
        Ok("fast") => child(false),
        _ => {}
    }
}

fn run_child(mode: &str) -> (std::process::Output, Duration) {
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "exit_watchdog::tests::child_process_scenario",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = KillOnDrop::new(child);
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the child never ended"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let took = started.elapsed();
    (child.wait_with_output().unwrap(), took)
}

#[test]
fn the_process_ends_when_blocking_work_holds_the_runtime() {
    let (output, took) = run_child("stuck");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(took < Duration::from_secs(10), "{took:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("wineo-gui: gave up after 0 s on a library write."),
        "{stderr}"
    );
}

#[test]
fn a_fast_exit_prints_nothing() {
    let (output, took) = run_child("fast");
    // The child's own status for "the runtime dropped and nothing fired".
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("wineo-gui:"));
}
