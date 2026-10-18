use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use super::*;
use crate::process;

fn runtime(root: &Path) -> Runtime {
    let home = root.to_owned();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    Runtime::new(&paths, "notepad")
}

fn run_state(status: RunStatus) -> RunState {
    RunState {
        status,
        lost: false,
        phase: None,
        mode: RunMode::Detached,
        target: "notepad".to_owned(),
        runner: None,
        prefix: None,
        native: false,
        launcher: None,
        pid: 4242,
        group: Some(4242),
        boot_id: None,
        leader_start: None,
        started_at: "2026-09-16T21:02:11Z".parse().unwrap(),
        running_at: None,
        heartbeat_at: None,
        program_exited_at: None,
        ended_at: None,
        exit_code: None,
        requested_target: None,
        args: Vec::new(),
        env: BTreeMap::new(),
    }
}

#[test]
fn lock_is_exclusive_until_dropped() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let first = runtime.try_lock().unwrap();
    assert!(first.is_some());
    assert!(runtime.try_lock().unwrap().is_none());
    drop(first);
    assert!(runtime.try_lock().unwrap().is_some());
}

#[test]
fn run_lock_releases_even_if_a_forked_child_still_holds_the_fd() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let lock = runtime.try_lock().unwrap().unwrap();
    // Stands in for a forked child's copy of the fd, kept alive past the drop.
    let clone = lock.file.try_clone().unwrap();
    drop(lock);
    assert!(runtime.try_lock().unwrap().is_some());
    drop(clone);
}

#[test]
fn is_running_probe_releases_its_lock_even_if_a_forked_child_still_holds_the_fd() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let file = runtime.open_lock().unwrap();
    // Stands in for a forked child's copy of the fd, kept alive past the probe.
    let clone = file.try_clone().unwrap();
    assert!(runtime.probe(&file).unwrap());
    drop(file);
    let fresh = runtime.open_lock().unwrap();
    assert!(
        runtime
            .flock(&fresh, FlockOperation::NonBlockingLockExclusive)
            .unwrap()
    );
    drop(clone);
}

#[test]
fn dropping_and_retaking_the_run_lock_survives_concurrent_forks() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let stop = Arc::new(AtomicBool::new(false));
    let forkers: Vec<_> = (0..4)
        .map(|_| {
            let stop = stop.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = std::process::Command::new("true").status();
                }
            })
        })
        .collect();
    for _ in 0..500 {
        let lock = runtime.try_lock().unwrap();
        assert!(lock.is_some(), "the previous drop should have freed it");
        drop(lock);
    }
    stop.store(true, Ordering::Relaxed);
    for forker in forkers {
        forker.join().unwrap();
    }
}

#[test]
fn probes_never_mistake_each_other_for_a_run() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    drop(runtime.try_lock().unwrap());
    let until = Instant::now() + Duration::from_millis(300);
    let probers: Vec<_> = (0..4)
        .map(|_| {
            let runtime = runtime.clone();
            thread::spawn(move || {
                let mut seen_running = false;
                while Instant::now() < until {
                    seen_running |= runtime.is_running().unwrap();
                }
                seen_running
            })
        })
        .collect();
    for prober in probers {
        assert!(!prober.join().unwrap());
    }
}

#[test]
fn a_new_run_gets_the_lock_while_something_probes() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    drop(runtime.try_lock().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let prober = {
        let (runtime, stop) = (runtime.clone(), stop.clone());
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                runtime.is_running().unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        })
    };
    for _ in 0..50 {
        assert!(runtime.try_lock().unwrap().is_some());
    }
    stop.store(true, Ordering::Relaxed);
    prober.join().unwrap();
}

#[test]
fn lock_records_the_holder_pid_until_released() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    assert_eq!(runtime.holder_pid().unwrap(), None);
    assert!(!runtime.is_running().unwrap());

    let lock = runtime.try_lock().unwrap().unwrap();
    assert_eq!(runtime.holder_pid().unwrap(), Some(std::process::id()));
    assert!(runtime.is_running().unwrap());
    // Probing must not disturb the holder's pid.
    assert_eq!(runtime.holder_pid().unwrap(), Some(std::process::id()));

    drop(lock);
    assert_eq!(runtime.holder_pid().unwrap(), None);
    assert!(!runtime.is_running().unwrap());
}

#[test]
fn state_round_trips() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    assert_eq!(runtime.read_state().unwrap(), None);

    let state = RunState {
        status: RunStatus::Stopped,
        mode: RunMode::Attached,
        program_exited_at: Some("2026-09-16T21:04:59Z".parse().unwrap()),
        ended_at: Some("2026-09-16T21:05:00Z".parse().unwrap()),
        exit_code: Some(0),
        requested_target: Some("notepad".to_owned()),
        args: vec!["-x".to_owned()],
        env: BTreeMap::from([("DXVK_HUD".to_owned(), "1".to_owned())]),
        prefix: None,
        boot_id: None,
        leader_start: None,
        ..run_state(RunStatus::Running)
    };
    runtime.write_state(&state).unwrap();
    assert_eq!(runtime.read_state().unwrap(), Some(state));
    assert_eq!(
        runtime.dir(),
        root.path().join(".local/state/wineo/notepad")
    );
}

#[test]
fn state_without_newer_fields_still_parses() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    fs::create_dir_all(runtime.dir()).unwrap();
    fs::write(
            runtime.dir().join("state.toml"),
            "status = \"exited\"\nmode = \"attached\"\ntarget = \"notepad\"\npid = 1\nstarted_at = \"2026-09-16T21:02:11Z\"\n",
        )
        .unwrap();
    let state = runtime.read_state().unwrap().unwrap();
    assert_eq!(state.status, RunStatus::Exited);
    assert_eq!(state.requested_target, None);
    assert_eq!(state.group, None);
}

/// `state.toml` files from before proxied runs read as the kind they
/// always were.
#[test]
fn older_state_files_keep_their_kind() {
    let base = "status = \"running\"\nmode = \"detached\"\ntarget = \"game\"\npid = 1\nstarted_at = \"2026-09-16T21:02:11Z\"\n";
    for (extra, kind) in [
        ("native = true\n", RunKind::Native),
        ("native = false\n", RunKind::Wine),
        ("", RunKind::Wine),
    ] {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        fs::create_dir_all(runtime.dir()).unwrap();
        fs::write(runtime.dir().join("state.toml"), format!("{base}{extra}")).unwrap();
        let state = runtime.read_state().unwrap().unwrap();
        assert_eq!(state.kind(), kind, "{extra:?}");
        assert_eq!(state.launcher, None, "{extra:?}");
    }
}

#[test]
fn a_proxied_run_round_trips_its_launcher() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let state = RunState {
        launcher: Some(Launcher::Steam),
        ..run_state(RunStatus::Running)
    };
    runtime.write_state(&state).unwrap();
    let text = fs::read_to_string(runtime.dir().join("state.toml")).unwrap();
    assert!(text.contains("launcher = \"steam\""), "{text}");
    assert!(!text.contains("native"), "{text}");
    let read = runtime.read_state().unwrap().unwrap();
    assert_eq!(read.kind(), RunKind::Proxied(Launcher::Steam));
    assert_eq!(read, state);
}

/// A lost Steam supervisor releases the run while leaving Steam's live tree alone.
#[test]
fn a_lost_proxied_run_is_released_without_signaling() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut run = run_state(RunStatus::Running);
    run.launcher = Some(Launcher::Steam);
    run.pid = pid;
    run.group = Some(pid);
    run.boot_id = process::boot_id();
    run.leader_start = process::start_time(pid);
    run.heartbeat_at = Some("2026-09-16T21:03:00Z".parse().unwrap());
    assert_eq!(run.group_ownership(), GroupOwnership::Owned);
    runtime.write_state(&run).unwrap();
    assert!(!runtime.is_running().unwrap());
    let stats_log = root.path().join("stats.jsonl");
    let cleaned = runtime
        .clean_up_lost_run(Duration::from_secs(1), &stats_log)
        .unwrap();
    assert!(
        matches!(cleaned, LostRunCleanup::Gone { warning: None }),
        "{cleaned:?}"
    );
    assert_eq!(
        runtime.read_state().unwrap().unwrap().status,
        RunStatus::Stopped
    );
    assert_eq!(
        runtime.read_state().unwrap().unwrap().ended_at,
        run.heartbeat_at
    );
    assert!(runtime.read_state().unwrap().unwrap().lost);
    // Steam's own total is the playtime, so no session is logged for it.
    assert!(!stats_log.exists());
    let again = runtime
        .clean_up_lost_run(Duration::from_secs(1), &stats_log)
        .unwrap();
    assert!(matches!(again, LostRunCleanup::NotLost), "{again:?}");
    assert!(!stats_log.exists());
    assert!(child.try_wait().unwrap().is_none());
    assert!(runtime.try_lock().unwrap().is_some());
    runtime.check_quiet("game").unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!stats_log.exists());
}

#[test]
fn a_starting_steam_run_keeps_the_legacy_running_status_on_disk() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let mut run = run_state(RunStatus::Running);
    run.launcher = Some(Launcher::Steam);
    run.starting();
    let lock = runtime.try_lock().unwrap().unwrap();
    runtime.write_state(&run).unwrap();
    let text = fs::read_to_string(runtime.state_path()).unwrap();
    assert!(text.contains("status = \"running\""), "{text}");
    assert!(text.contains("phase = \"starting\""), "{text}");
    assert_eq!(runtime.status().unwrap().state, State::Starting);
    drop(lock);
}

#[test]
fn overrides_round_trip_through_state() {
    let overrides = Overrides {
        target: Some("bridge".to_owned()),
        env: BTreeMap::from([("A".to_owned(), "1".to_owned())]),
        args: vec!["-nosplash".to_owned()],
    };
    let mut state = RunState::running(
        RunMode::Detached,
        "bridge".to_owned(),
        7,
        Some(7),
        &overrides,
    );
    assert_eq!(state.status, RunStatus::Running);
    let kept = state.overrides();
    assert_eq!(kept.target, overrides.target);
    assert_eq!(kept.env, overrides.env);
    assert_eq!(kept.args, overrides.args);

    state.program_exited(Some(3), true);
    assert!(state.program_exited_at.is_some());
    assert_eq!(state.exit_code, Some(3));
    assert_eq!(state.ended_at, None);
    state.finish(RunStatus::Failed);
    assert_eq!(state.status, RunStatus::Failed);
    assert!(state.ended_at.is_some());
}

#[test]
fn status_follows_lock_and_state() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let status = runtime.status().unwrap();
    assert_eq!(
        (status.state, status.run, status.lost),
        (State::Inactive, None, false)
    );

    for (recorded, state) in [
        (RunStatus::Exited, State::Exited),
        (RunStatus::Failed, State::Failed),
        (RunStatus::Stopped, State::Stopped),
    ] {
        runtime.write_state(&run_state(recorded)).unwrap();
        let status = runtime.status().unwrap();
        assert_eq!(status.state, state);
        assert!(!status.lost);
        assert_eq!(status.run.unwrap().status, recorded);
    }

    // A new run holds the lock before it writes its state.
    let lock = runtime.try_lock().unwrap().unwrap();
    let status = runtime.status().unwrap();
    assert_eq!((status.state, status.run), (State::Running, None));

    runtime.write_state(&run_state(RunStatus::Running)).unwrap();
    let status = runtime.status().unwrap();
    assert_eq!(status.state, State::Running);
    assert!(status.run.is_some());

    // The holder died without finishing its state.
    drop(lock);
    let status = runtime.status().unwrap();
    assert_eq!(status.state, State::Failed);
    assert!(status.lost);
    assert!(status.run.is_some());
}

#[test]
fn check_quiet_refuses_runs_lost_runs_and_unresolved_operations() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    runtime.check_quiet("notepad").unwrap();

    let lock = runtime.try_lock().unwrap().unwrap();
    assert!(matches!(
        runtime.check_quiet("notepad"),
        Err(Error::Running { .. })
    ));
    runtime.write_state(&run_state(RunStatus::Running)).unwrap();
    drop(lock);
    assert!(matches!(
        runtime.check_quiet("notepad"),
        Err(Error::LostRun(_))
    ));

    runtime.write_state(&run_state(RunStatus::Stopped)).unwrap();
    runtime
        .write_operation(&OperationState {
            operation: Operation::Install,
            program: "setup.exe".to_owned(),
            runner: Runner::fake("wine"),
            prefix: root.path().join("prefix"),
            pid: None,
            group: None,
            boot_id: None,
            leader_start: None,
            started_at: Timestamp::now(),
        })
        .unwrap();
    assert!(matches!(
        runtime.check_quiet("notepad"),
        Err(Error::Unresolved(_))
    ));
    runtime.clear_operation().unwrap();
    runtime.check_quiet("notepad").unwrap();

    // An unreadable state file cannot make the lost-run block disappear.
    fs::write(runtime.dir().join("state.toml"), "garbage [").unwrap();
    assert!(matches!(
        runtime.check_quiet("notepad"),
        Err(Error::LostRun(_))
    ));
    assert!(matches!(
        runtime.check_resolved("notepad"),
        Err(Error::LostRun(_))
    ));
    assert!(runtime.state_path().is_file());
}

#[test]
fn rotating_keeps_one_previous_log() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    writeln!(runtime.rotate_log().unwrap(), "first").unwrap();
    assert!(!runtime.previous_log_path().exists());
    writeln!(runtime.rotate_log().unwrap(), "second").unwrap();
    writeln!(runtime.rotate_log().unwrap(), "third").unwrap();
    assert_eq!(fs::read_to_string(runtime.log_path()).unwrap(), "third\n");
    assert_eq!(
        fs::read_to_string(runtime.previous_log_path()).unwrap(),
        "second\n"
    );
}

#[test]
fn exit_codes_follow_shell_convention() {
    assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
    assert_eq!(exit_code(ExitStatus::from_raw(9)), 137);
}

#[test]
fn a_lost_run_is_logged_exactly_once_even_across_a_failed_state_write() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let mut run = run_state(RunStatus::Running);
    run.native = true;
    // A boot id that never matches the current one, so ownership resolves
    // to `Gone` without touching any real process.
    run.boot_id = Some("an-earlier-boot".to_owned());
    run.leader_start = Some(1);
    run.heartbeat_at = Some("2026-09-16T21:03:00Z".parse().unwrap());
    runtime.write_state(&run).unwrap();
    let stats_log = root.path().join("stats.jsonl");

    // The runtime dir can't be written to, so the state write inside
    // cleanup fails before anything is appended.
    let dir = runtime.dir().to_owned();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let failed = runtime.clean_up_lost_run(Duration::from_secs(5), &stats_log);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(failed.is_err(), "{failed:?}");
    assert!(!stats_log.exists());
    assert_eq!(
        runtime.read_state().unwrap().unwrap().status,
        RunStatus::Running,
        "a failed write must not mark the run finished"
    );

    // A retry now succeeds, and logs the lost session exactly once.
    let cleaned = runtime
        .clean_up_lost_run(Duration::from_secs(5), &stats_log)
        .unwrap();
    assert!(matches!(cleaned, LostRunCleanup::Gone { warning: None }));
    let log = fs::read_to_string(&stats_log).unwrap();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("\"end\":\"lost\""), "{log}");
    assert!(
        log.contains("\"ended_at\":\"2026-09-16T21:03:00Z\""),
        "{log}"
    );
}
