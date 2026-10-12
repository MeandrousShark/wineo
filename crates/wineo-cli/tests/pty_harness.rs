//! The pseudo-terminal harness itself: it must not leave commands running
//! or print secrets when a test gives up.

mod common;

use std::any::Any;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::Command;
use std::sync::Once;
use std::time::{Duration, Instant};

use common::pty::Pty;

thread_local! {
    static EXPECTED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f`, which is meant to panic, without the panic printing anything:
/// its message could hold the very secrets these tests check are hidden. Only
/// this thread's deliberate panics are silenced; any other panic still
/// reports as usual.
fn deliberately_panics<T>(f: impl FnOnce() -> T) -> Result<T, Box<dyn Any + Send>> {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let usual = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !EXPECTED.with(Cell::get) {
                usual(info);
            }
        }));
    });
    EXPECTED.with(|expected| expected.set(true));
    let result = catch_unwind(AssertUnwindSafe(f));
    EXPECTED.with(|expected| expected.set(false));
    result
}

fn gone(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Path::new(&format!("/proc/{pid}")).exists() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", script]);
    command
}

/// The message of a panic. Never shown by a failing assertion, only
/// searched.
fn panic_text(result: Result<impl Sized, Box<dyn Any + Send>>) -> String {
    let Err(payload) = result else {
        panic!("the test should have panicked");
    };
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_default()
}

/// Starts a shell that starts a `sleep` in the background and, when
/// `leader_waits` is false, exits at once. Returns the terminal and the
/// `sleep`'s process id.
fn with_descendant(leader_waits: bool) -> (Pty, u32) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("descendant");
    let tail = if leader_waits { "wait" } else { "exit 0" };
    let mut pty = Pty::spawn(shell(&format!(
        "sleep 300 & echo $! > '{}'; echo ready; {tail}",
        file.display()
    )));
    pty.expect("ready");
    let descendant = std::fs::read_to_string(&file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    (pty, descendant)
}

#[test]
fn a_timed_out_terminal_leaves_no_command_running() {
    let mut command = Command::new("sleep");
    command.arg("300");
    let pty = Pty::spawn(command);
    let pid = pty.id();
    let result = deliberately_panics(|| pty.finish_within(Duration::from_millis(200)));
    assert!(panic_text(result).contains("wineo didn't exit"));
    assert!(gone(pid), "the command is still running");
}

#[test]
fn a_dropped_terminal_kills_the_whole_group() {
    let (pty, descendant) = with_descendant(true);
    let child = pty.id();
    assert!(Path::new(&format!("/proc/{descendant}")).exists());
    drop(pty);
    assert!(gone(child), "the command is still running");
    assert!(gone(descendant), "what it started is still running");
}

#[test]
fn a_leader_that_exits_first_leaves_nothing_running_after_drop() {
    let (pty, descendant) = with_descendant(false);
    // Wait for the shell itself to be gone, with its `sleep` still there.
    let leader = pty.id();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Path::new(&format!("/proc/{leader}/stat")).exists()
        && !std::fs::read_to_string(format!("/proc/{leader}/stat"))
            .unwrap_or_default()
            .contains(") Z ")
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(pty);
    assert!(gone(descendant), "what the leader started is still running");
}

#[test]
fn a_leader_that_exits_first_leaves_nothing_running_after_finish_or_timeout() {
    let (pty, descendant) = with_descendant(false);
    let (status, _) = pty.finish();
    assert!(status.success());
    assert!(gone(descendant), "what the leader started survived finish");

    // The leader is gone by the time a timeout kills the group.
    let (pty, descendant) = with_descendant(false);
    std::thread::sleep(Duration::from_millis(200));
    let result = deliberately_panics(|| pty.finish_within(Duration::ZERO));
    // Whether it counted as in time or not, nothing may stay behind.
    drop(result);
    assert!(
        gone(descendant),
        "what the leader started survived the timeout"
    );
}

#[test]
fn finishing_keeps_to_its_deadline() {
    let started = Instant::now();
    let mut command = Command::new("sleep");
    command.arg("300");
    let pty = Pty::spawn(command);
    let result = deliberately_panics(|| pty.finish_within(Duration::from_millis(100)));
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_millis(1500));

    // A leader that exits with its terminal still held by a descendant
    // doesn't hold the call up past the deadline either.
    let started = Instant::now();
    let mut pty = Pty::spawn(shell("sleep 300 & echo ready; exit 0"));
    pty.expect("ready");
    let (status, seen) = pty.finish_within(Duration::from_millis(300));
    assert!(status.success());
    assert!(seen.contains("ready"));
    assert!(started.elapsed() < Duration::from_millis(1500));
}

#[test]
fn a_panicking_test_leaves_no_command_running() {
    let mut command = Command::new("sleep");
    command.arg("300");
    let pty = Pty::spawn(command);
    let pid = pty.id();
    let result = deliberately_panics(move || {
        let mut pty = pty;
        pty.expect_within("never printed", Duration::from_millis(300));
    });
    assert!(result.is_err());
    assert!(gone(pid), "the command is still running");
}

#[test]
fn a_timeout_hides_secrets_in_what_it_prints() {
    let mut pty = Pty::spawn(shell(
        "echo token ACCESS-SECRET-1 and GoodCode-x; sleep 300",
    ));
    pty.expect("token");
    let result =
        deliberately_panics(|| pty.expect_within("never printed", Duration::from_millis(300)));
    let text = panic_text(result);
    assert!(
        text.contains("<redacted>"),
        "the panic message wasn't redacted"
    );
    assert!(
        !text.contains("SECRET") && !text.contains("GoodCode"),
        "a secret was in the panic message"
    );

    let pty = Pty::spawn(shell("echo ACCESS-SECRET-2; sleep 300"));
    let result = deliberately_panics(|| {
        std::thread::sleep(Duration::from_millis(300));
        pty.finish_within(Duration::from_millis(100))
    });
    let text = panic_text(result);
    assert!(
        text.contains("<redacted>"),
        "the panic message wasn't redacted"
    );
    assert!(
        !text.contains("SECRET"),
        "a secret was in the panic message"
    );
}

#[test]
fn what_a_terminal_printed_hides_secrets_when_displayed() {
    let pty = Pty::spawn(shell("echo ACCESS-SECRET-3 done"));
    let (status, seen) = pty.finish();
    assert!(status.success());
    assert!(seen.raw().contains("ACCESS-SECRET-3"));
    let shown = format!("{seen} {seen:?}");
    assert!(!shown.contains("SECRET"), "a secret was displayed");
    assert!(shown.contains("<redacted>"));
}
