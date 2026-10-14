use std::collections::BTreeMap;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use super::*;
use crate::operation::{Locks, Operation};
use crate::process::GroupOwnership;
use rustix::process::Pid;

const POLL: Duration = Duration::from_millis(20);

fn paths(root: &Path, portable: bool) -> Paths {
    Paths::select(
        |key| (key == "HOME").then(|| root.as_os_str().to_owned()),
        portable.then_some(root),
    )
    .unwrap()
}

fn context<'a>(wine: Option<(&'a Runner, &'a Path)>, hook: Hook) -> Context<'a> {
    Context {
        steam: None,
        environment: "hades",
        target: "editor",
        exe: "C:\\Games\\Hades.exe",
        wine,
        hook,
    }
}

fn variables(paths: &Paths, context: &Context<'_>) -> BTreeMap<String, String> {
    let mut command = Command::new("sh");
    // Seed this command rather than mutating shared process environment.
    for name in VARIABLES.into_iter().chain([PORTABLE_VAR]) {
        command.env(name, "stale");
    }
    command.env("HOOK_INHERITED", "kept");
    configure(&mut command, paths, context, "env");
    let output = command.output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

#[test]
fn hooks_before_variables_for_wine_proton_and_native() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path(), false);
    let wine = Runner::fake("wine");
    let proton = Runner::fake_proton("proton");
    let prefix = root.path().join("prefix");
    for (runner, expected) in [
        (Some(&wine), Some(prefix.clone())),
        (Some(&proton), Some(prefix.join("pfx"))),
        (None, None),
    ] {
        let context = context(
            runner.map(|runner| (runner, prefix.as_path())),
            Hook::Before,
        );
        let vars = variables(&paths, &context);
        assert_eq!(vars["WINEO_ENVIRONMENT"], "hades");
        assert_eq!(vars["WINEO_TARGET"], "editor");
        assert_eq!(vars["WINEO_EXE"], context.exe);
        assert_eq!(vars["WINEO_HOOK"], "before");
        assert_eq!(vars["HOOK_INHERITED"], "kept");
        assert_eq!(
            vars.get("WINEO_PREFIX"),
            expected
                .as_ref()
                .map(|path| path.to_str().unwrap().to_owned())
                .as_ref()
        );
        for key in [
            "WINEPREFIX",
            "WINEO_END",
            "WINEO_EXIT_CODE",
            "WINEO_SECONDS",
            PORTABLE_VAR,
        ] {
            assert!(!vars.contains_key(key), "{key}");
        }
    }
}

#[test]
fn hooks_after_variables_and_portable_root() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path(), true);
    for (end, exit_code) in [
        (End::Exited, Some(0)),
        (End::Failed, Some(7)),
        (End::Stopped, None),
    ] {
        let context = context(
            None,
            Hook::After(Ending {
                end,
                exit_code,
                seconds: 42,
            }),
        );
        let vars = variables(&paths, &context);
        assert_eq!(vars["WINEO_HOOK"], "after");
        assert_eq!(vars["WINEO_END"], end.as_str());
        assert_eq!(
            vars["WINEO_EXIT_CODE"],
            exit_code.map(|code| code.to_string()).unwrap_or_default()
        );
        assert_eq!(vars["WINEO_SECONDS"], "42");
        assert_eq!(
            vars[PORTABLE_VAR],
            paths.portable_root().unwrap().to_str().unwrap()
        );
        assert!(!vars.contains_key("WINEO_PREFIX"));
        assert!(!vars.contains_key("WINEPREFIX"));
    }
}

#[test]
fn hooks_run_in_home_capture_output_and_report_nonzero_exit() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path(), false);
    let locks = Locks::new(&paths);
    let guard = locks
        .acquire(Operation::Hook(Phase::Before), "hades", &[])
        .unwrap();
    let output = root.path().join("output");
    let outcome = run(
        "pwd; echo stderr >&2; read line || echo stdin-null; exit 7",
        &paths,
        &context(None, Hook::Before),
        &guard,
        &Token::new(),
        Duration::from_secs(1),
        Output::File(File::create(&output).unwrap()),
    )
    .unwrap();
    let Outcome::Finished(status) = outcome else {
        panic!("hook was canceled");
    };
    assert_eq!(status.code(), Some(7));
    assert_eq!(
        fs::read_to_string(output).unwrap(),
        format!("{}\nstderr\nstdin-null\n", paths.home().display())
    );
    assert!(locks.hook_group("hades").unwrap().is_none());
}

fn wait_ready(root: &Path, locks: &Locks) -> Identity {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if root.join("ready").exists()
            && let Some((phase, running)) = locks.hook_group("hades").unwrap()
        {
            assert_eq!(phase, Phase::Before);
            return running;
        }
        assert!(Instant::now() < deadline, "hook never became ready");
        thread::sleep(POLL);
    }
}

fn interrupted(
    text: &str,
    timeout: Duration,
    stop: impl FnOnce(&Token, &Identity),
) -> (ExitStatus, Duration) {
    let root = Arc::new(tempfile::tempdir().unwrap());
    let paths = paths(root.path(), false);
    let locks = Locks::new(&paths);
    let token = Token::new();
    let task = {
        let (root, paths, locks, token, text) = (
            root.clone(),
            paths.clone(),
            locks.clone(),
            token.clone(),
            text.to_owned(),
        );
        thread::spawn(move || {
            let _root = root;
            let guard = locks
                .acquire(Operation::Hook(Phase::Before), "hades", &[])
                .unwrap();
            run(
                &text,
                &paths,
                &context(None, Hook::Before),
                &guard,
                &token,
                timeout,
                Output::Inherit,
            )
            .unwrap()
        })
    };
    let running = wait_ready(root.path(), &locks);
    assert_eq!(running.ownership(), GroupOwnership::Owned);
    let started = Instant::now();
    stop(&token, &running);
    let outcome = join_with_watchdog(task, &running);
    let status = match outcome {
        Outcome::Canceled(status) => {
            assert!(token.is_canceled());
            status
        }
        Outcome::Finished(status) => {
            assert!(!token.is_canceled());
            status
        }
    };
    let elapsed = started.elapsed();
    assert_eq!(process::group_has_live_members(running.group), Some(false));
    assert!(locks.hook_group("hades").unwrap().is_none());
    (status, elapsed)
}

fn join_with_watchdog<T>(task: thread::JoinHandle<T>, running: &Identity) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !task.is_finished() {
        if Instant::now() >= deadline {
            let _ = running.kill_now();
            panic!("hook worker did not finish within five seconds");
        }
        thread::sleep(POLL);
    }
    task.join().unwrap()
}

#[test]
fn hooks_cancel_kills_background_sleep() {
    interrupted(
        "sleep 60 & echo ready > ready; wait",
        Duration::from_secs(1),
        |token, _| token.cancel(),
    );
}

#[test]
fn hooks_cancel_escalates_after_timeout_for_term_trap() {
    let timeout = Duration::from_millis(200);
    let (status, elapsed) = interrupted(
        "trap '' TERM; sleep 60 & echo ready > ready; wait",
        timeout,
        |token, _| token.cancel(),
    );
    assert_eq!(status.signal(), Some(9));
    assert!(elapsed >= timeout);
    assert!(elapsed < Duration::from_secs(5));
}

#[test]
fn hooks_cancel_keeps_identity_when_leader_exits_before_child() {
    interrupted(
        "trap 'exit 0' TERM; sh -c 'trap \"\" TERM; echo ready > ready; sleep 60' & wait",
        Duration::from_millis(100),
        |token, _| token.cancel(),
    );
}

#[test]
fn hooks_force_token_kills_now() {
    let (status, elapsed) = interrupted(
        "trap '' TERM; sleep 60 & echo ready > ready; wait",
        Duration::from_secs(60),
        |token, _| token.force(),
    );
    assert_eq!(status.signal(), Some(9));
    assert!(elapsed < Duration::from_secs(5));
}

#[test]
fn hooks_recorded_group_can_be_killed_by_another_caller() {
    // The shell spins rather than waiting on `sleep`: members are killed
    // before the leader, and a `wait` would let the shell exit 0 on its own
    // in between.
    let (status, elapsed) = interrupted(
        "trap '' TERM; sleep 60 & echo ready > ready; while :; do :; done",
        Duration::from_secs(60),
        |_, running| {
            assert_eq!(running.kill_now().unwrap(), GroupOwnership::Owned);
        },
    );
    assert_eq!(status.signal(), Some(9), "{status:?}");
    assert!(elapsed < Duration::from_secs(5));
}

#[test]
fn hooks_cancel_term_trap_exiting_zero_is_canceled() {
    let (status, _) = interrupted(
        "trap 'exit 0' TERM; echo ready > ready; while :; do :; done",
        Duration::from_secs(1),
        |token, _| token.cancel(),
    );
    assert!(status.success());
}

#[test]
fn hooks_kill_now_rejects_stale_identity_and_leaves_live_child() {
    let child = Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let mut spawned = Spawned {
        child,
        finished: false,
    };
    let running = Identity {
        group: spawned.child.id(),
        boot_id: process::boot_id().unwrap(),
        leader_start: process::start_time(spawned.child.id()).unwrap(),
    };
    for stale in [
        Identity {
            boot_id: "wrong-boot".to_owned(),
            ..running.clone()
        },
        Identity {
            leader_start: running.leader_start + 1,
            ..running.clone()
        },
    ] {
        assert_eq!(stale.kill_now().unwrap(), GroupOwnership::Gone);
        assert!(spawned.child.try_wait().unwrap().is_none());
        assert_eq!(running.ownership(), GroupOwnership::Owned);
    }
}

#[test]
fn hooks_cancel_escalates_when_process_inspection_is_unknown() {
    let root = Arc::new(tempfile::tempdir().unwrap());
    let child = Command::new("sh")
        .args(["-c", "trap '' TERM; sleep 60 & echo ready > ready; wait"])
        .current_dir(root.path())
        .process_group(0)
        .spawn()
        .unwrap();
    let mut spawned = Spawned {
        child,
        finished: false,
    };
    let running = Identity {
        group: spawned.child.id(),
        boot_id: process::boot_id().unwrap(),
        leader_start: process::start_time(spawned.child.id()).unwrap(),
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while !root.path().join("ready").exists() {
        assert!(Instant::now() < deadline, "hook never became ready");
        thread::sleep(POLL);
    }
    let token = Token::new();
    token.cancel();
    let timeout = Duration::from_millis(100);
    let started = Instant::now();
    let task = thread::spawn(move || {
        let _root = root;
        // Killed, but with nothing to inspect it can't be called stopped.
        let unconfirmed = wait_for_exit(&spawned.child, &token, timeout, |_| None);
        assert!(
            matches!(&unconfirmed, Err(Error::Unsettled(message)) if message == UNCONFIRMED),
            "{unconfirmed:?}"
        );
        let status = spawned.child.wait().unwrap();
        spawned.finished = true;
        drop(spawned);
        status
    });
    let status = join_with_watchdog(task, &running);
    assert_eq!(status.signal(), Some(9));
    assert!(started.elapsed() >= timeout);
    let deadline = Instant::now() + Duration::from_secs(5);
    while process::group_has_live_members(running.group) != Some(false) {
        assert!(Instant::now() < deadline, "hook child survived escalation");
        thread::sleep(POLL);
    }
}

#[test]
fn hooks_normal_exit_leaves_background_child_running() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path(), false);
    let locks = Locks::new(&paths);
    let guard = locks
        .acquire(Operation::Hook(Phase::Before), "hades", &[])
        .unwrap();
    let outcome = run(
        "sleep 60 & echo $! > child; echo $$ > group; exit 0",
        &paths,
        &context(None, Hook::Before),
        &guard,
        &Token::new(),
        Duration::from_secs(1),
        Output::Inherit,
    )
    .unwrap();
    let child: u32 = fs::read_to_string(root.path().join("child"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    struct Cleanup(u32);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ =
                rustix::process::kill_process(Pid::from_raw(self.0 as i32).unwrap(), Signal::KILL);
        }
    }
    let _cleanup = Cleanup(child);
    assert!(matches!(outcome, Outcome::Finished(status) if status.success()));
    assert!(process::start_time(child).is_some());
    assert!(rustix::process::test_kill_process(Pid::from_raw(child as i32).unwrap()).is_ok());
    let group = fs::read_to_string(root.path().join("group"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(process::group_has_live_members(group), Some(true));
    assert!(locks.hook_group("hades").unwrap().is_none());
}

#[test]
fn hooks_wrong_operation_guard_returns_error_before_spawning() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path(), false);
    let locks = Locks::new(&paths);
    for operation in [Operation::Launch, Operation::Hook(Phase::After)] {
        let guard = locks.acquire(operation, "hades", &[]).unwrap();
        assert!(matches!(
            run(
                "touch started",
                &paths,
                &context(None, Hook::Before),
                &guard,
                &Token::new(),
                Duration::ZERO,
                Output::Inherit,
            ),
            Err(Error::Unsettled(_))
        ));
        assert!(!root.path().join("started").exists());
    }
}

#[test]
fn hooks_already_canceled_token_starts_nothing() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path(), false);
    let locks = Locks::new(&paths);
    let guard = locks
        .acquire(Operation::Hook(Phase::Before), "hades", &[])
        .unwrap();
    let token = Token::new();
    token.cancel();
    assert!(matches!(
        run(
            "touch started",
            &paths,
            &context(None, Hook::Before),
            &guard,
            &token,
            Duration::ZERO,
            Output::Inherit
        ),
        Err(Error::Canceled("hook"))
    ));
    assert!(!root.path().join("started").exists());
}
