//! The pseudo-terminal harness itself: it must not leave commands running
//! or print secrets when a test gives up.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use common::pty::Pty;

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

fn panic_text(result: Result<impl Sized, Box<dyn std::any::Any + Send>>) -> String {
    let payload = result.err().expect("the test should have panicked");
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

#[test]
fn a_timed_out_terminal_leaves_no_command_running() {
    let mut command = Command::new("sleep");
    command.arg("300");
    let pty = Pty::spawn(command);
    let pid = pty.id();
    let result = catch_unwind(AssertUnwindSafe(|| {
        pty.finish_within(Duration::from_millis(200))
    }));
    assert!(panic_text(result).contains("wineo didn't exit"));
    assert!(gone(pid), "the command is still running");
}

#[test]
fn a_dropped_terminal_kills_the_whole_group() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("grandchild");
    let mut pty = Pty::spawn(shell(&format!(
        "sleep 300 & echo $! > '{}'; echo ready; wait",
        file.display()
    )));
    pty.expect("ready");
    let child = pty.id();
    let grandchild: u32 = std::fs::read_to_string(&file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(Path::new(&format!("/proc/{grandchild}")).exists());
    drop(pty);
    assert!(gone(child), "the command is still running");
    assert!(gone(grandchild), "what it started is still running");
}

#[test]
fn a_panicking_test_leaves_no_command_running() {
    let mut command = Command::new("sleep");
    command.arg("300");
    let pty = Pty::spawn(command);
    let pid = pty.id();
    let result = catch_unwind(AssertUnwindSafe(move || {
        let mut pty = pty;
        pty.expect_within("never printed", Duration::from_millis(300));
    }));
    assert!(result.is_err());
    assert!(gone(pid), "the command is still running");
}

#[test]
fn a_timeout_hides_secrets_in_what_it_prints() {
    let mut pty = Pty::spawn(shell(
        "echo token ACCESS-SECRET-1 and GoodCode-x; sleep 300",
    ));
    pty.expect("token");
    let result = catch_unwind(AssertUnwindSafe(|| {
        pty.expect_within("never printed", Duration::from_millis(300))
    }));
    let text = panic_text(result);
    assert!(text.contains("<redacted>"), "{text}");
    assert!(
        !text.contains("SECRET") && !text.contains("GoodCode"),
        "{text}"
    );

    let pty = Pty::spawn(shell("echo ACCESS-SECRET-2; sleep 300"));
    let result = catch_unwind(AssertUnwindSafe(|| {
        std::thread::sleep(Duration::from_millis(300));
        pty.finish_within(Duration::from_millis(100))
    }));
    let text = panic_text(result);
    assert!(text.contains("<redacted>"), "{text}");
    assert!(!text.contains("SECRET"), "{text}");
}

#[test]
fn what_a_terminal_printed_hides_secrets_when_displayed() {
    let pty = Pty::spawn(shell("echo ACCESS-SECRET-3 done"));
    let (status, seen) = pty.finish();
    assert!(status.success());
    assert!(seen.raw().contains("ACCESS-SECRET-3"));
    assert!(!format!("{seen} {seen:?}").contains("SECRET"));
    assert!(format!("{seen}").contains("<redacted>"));
}
