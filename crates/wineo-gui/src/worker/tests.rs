use super::*;
use std::time::Duration;

#[test]
fn finished_after_hook_reports_a_panel_note() {
    let messages = after_stop_messages(launch::StopOutcome::NotRunning {
        messages: Vec::new(),
    });
    assert_eq!(messages, vec![AFTER_HOOK_ENDED_NOTE]);
}

#[test]
fn capture_is_only_created_for_the_selected_before_hook() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let config = paths.environment_config("app");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "schema_version = 1\nkind = 'native'\n[targets.app]\nexe = '/missing'\n",
    )
    .unwrap();
    assert!(hook_capture(&paths, "app", "app").unwrap().is_none());
    std::fs::write(&config, "schema_version = 1\nkind = 'native'\n[hooks]\nbefore = 'echo output; echo error >&2; exit 3'\n[targets.app]\nexe = '/missing'\n[targets.disabled]\nexe = '/missing'\nhooks = false\n").unwrap();
    assert!(hook_capture(&paths, "app", "disabled").unwrap().is_none());
    let capture = hook_capture(&paths, "app", "app").unwrap().unwrap();
    let error = launch::prepare(
        &paths,
        "app",
        &Overrides {
            target: Some("app".into()),
            ..Overrides::default()
        },
        &Token::new(),
        wineo_core::hooks::Output::File(capture.try_clone().unwrap()),
        |_| {},
    )
    .err()
    .expect("before hook should fail");
    let mut capture = capture.finish().unwrap();
    let mapped = launch_error(&error, Some(&mut capture), None);
    assert_eq!(
        mapped.text,
        "Launch canceled: the before hook exited with code 3."
    );
    assert_eq!(mapped.output.as_deref(), Some("output\nerror"));
}

#[test]
fn failed_before_shows_only_last_twenty_output_lines() {
    let mut capture = tempfile::tempfile().unwrap();
    for index in 0..30 {
        writeln!(capture, "line {index}\r").unwrap();
    }
    write!(capture, "partial").unwrap();
    let error = anyhow::Error::new(wineo_core::Error::BeforeHookFailed(7));
    let mapped = launch_error(&error, Some(&mut capture), None);
    assert_eq!(
        mapped.text,
        "Launch canceled: the before hook exited with code 7."
    );
    let output = mapped.output.unwrap();
    assert_eq!(output.lines().count(), 20);
    assert!(output.starts_with("line 11\n"));
    assert!(output.ends_with("line 29\npartial"));
    assert!(!output.contains('\r'));
}

#[test]
fn failed_launch_shows_cleanup_output_after_before_and_keeps_twenty_lines() {
    for before in [None, Some("echo before")] {
        for lines in [1, 25] {
            let root = tempfile::tempdir().unwrap();
            let paths = Paths::from_lookup(|key| {
                (key == "HOME").then(|| root.path().as_os_str().to_owned())
            })
            .unwrap();
            let config = paths.environment_config("app");
            std::fs::create_dir_all(config.parent().unwrap()).unwrap();
            let before_config = before
                .map(|command| format!("before = '{command}'\n"))
                .unwrap_or_default();
            std::fs::write(
                config,
                format!(
                    "kind = 'native'\n[hooks]\n{before_config}after = 'i=0; while [ $i -lt {lines} ]; do echo cleanup-$i >&2; i=$((i+1)); done'\n[targets.app]\nexe = '{}'\n",
                    root.path().join("missing").display()
                ),
            )
            .unwrap();
            let capture = hook_capture(&paths, "app", "app").unwrap().unwrap();
            let mut after_output_start = None;
            let error = launch::start(
                &paths,
                "app",
                Overrides::default(),
                &root.path().join("unused-supervisor"),
                &Token::new(),
                wineo_core::hooks::Output::File(capture.try_clone().unwrap()),
                Output::Inherit,
                |event| {
                    if matches!(event, launch::LaunchEvent::AfterHook(_)) {
                        after_output_start = Some(capture.position().unwrap());
                    }
                },
            )
            .unwrap_err();
            let mut capture = capture.finish().unwrap();
            let mapped = launch_error(&error, Some(&mut capture), after_output_start);
            assert_eq!(mapped.text, PanelError::from_error(&error).text);
            let mut expected = Vec::new();
            if before.is_some() {
                expected.push("before".to_owned());
            }
            expected.extend((0..lines).map(|index| format!("cleanup-{index}")));
            let expected = expected[expected.len().saturating_sub(20)..].join("\n");
            assert_eq!(mapped.output.as_deref(), Some(expected.as_str()));
        }
    }
}

#[test]
fn silent_cleanup_keeps_successful_before_output_hidden() {
    let mut capture = tempfile::tempfile().unwrap();
    writeln!(capture, "successful before output").unwrap();
    let after_output_start = Some(capture.metadata().unwrap().len());
    let error = anyhow::anyhow!("missing executable");
    assert!(
        launch_error(&error, Some(&mut capture), after_output_start)
            .output
            .is_none()
    );
}

#[test]
fn empty_capture_and_other_errors_have_no_hook_output() {
    let mut capture = tempfile::tempfile().unwrap();
    let failure = anyhow::Error::new(wineo_core::Error::BeforeHookFailed(1));
    assert!(
        launch_error(&failure, Some(&mut capture), None)
            .output
            .is_none()
    );
    writeln!(capture, "successful before output").unwrap();
    let error = anyhow::anyhow!("missing executable");
    assert!(
        launch_error(&error, Some(&mut capture), None)
            .output
            .is_none()
    );
}

#[test]
fn capture_bounds_long_and_invalid_utf8_output() {
    let mut capture = tempfile::tempfile().unwrap();
    capture.write_all(&vec![0xff; 100_000]).unwrap();
    writeln!(capture, "\nlast line").unwrap();
    let output = captured_tail(&mut capture).unwrap();
    assert!(output.ends_with("last line"));
    assert!(output.chars().count() <= 64 * 1024);
}

#[test]
fn hook_capture_stays_bounded_while_a_chatty_hook_runs() {
    use std::os::unix::fs::FileTypeExt;

    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let config = paths.environment_config("app");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "kind = 'native'\n[hooks]\nbefore = 'yes x | head -c 4194304; echo last line; exit 8'\n[targets.app]\nexe = '/missing'\n",
    )
    .unwrap();
    let capture = hook_capture(&paths, "app", "app").unwrap().unwrap();
    assert!(
        capture
            .writer
            .as_ref()
            .unwrap()
            .metadata()
            .unwrap()
            .file_type()
            .is_socket()
    );
    let error = launch::prepare(
        &paths,
        "app",
        &Overrides {
            target: Some("app".into()),
            ..Overrides::default()
        },
        &Token::new(),
        wineo_core::hooks::Output::File(capture.try_clone().unwrap()),
        |_| {},
    )
    .err()
    .expect("before hook should fail");
    capture.position().unwrap();
    let state = capture.state.lock().unwrap();
    assert!(state.bytes.len() <= HOOK_CAPTURE_LIMIT);
    assert!(state.bytes.capacity() <= HOOK_CAPTURE_LIMIT);
    assert!(state.total > 4 * 1024 * 1024);
    drop(state);
    let mut snapshot = capture.finish().unwrap();
    assert_eq!(snapshot.bytes.len(), HOOK_CAPTURE_LIMIT);
    assert!(snapshot.tail().unwrap().ends_with("last line"));
    assert!(
        launch_error(&error, Some(&mut snapshot), None)
            .output
            .unwrap()
            .ends_with("last line")
    );
}

/// Voluntary context switches of the capture reader thread called `name`.
fn reader_wakeups(name: &str) -> u64 {
    let mut total = 0;
    for task in std::fs::read_dir("/proc/self/task").unwrap().flatten() {
        let path = task.path();
        if std::fs::read_to_string(path.join("comm")).is_ok_and(|comm| comm.trim() == name)
            && let Ok(status) = std::fs::read_to_string(path.join("status"))
        {
            total += status
                .lines()
                .find_map(|line| line.strip_prefix("voluntary_ctxt_switches:"))
                .and_then(|count| count.trim().parse::<u64>().ok())
                .unwrap_or(0);
        }
    }
    total
}

#[test]
fn an_idle_capture_reader_does_not_wake() {
    let capture = HookCapture::named("idle-capture").unwrap();
    thread::sleep(Duration::from_millis(100));
    let before = reader_wakeups("idle-capture");
    thread::sleep(Duration::from_secs(1));
    let woke = reader_wakeups("idle-capture") - before;
    eprintln!("reader wakeups in 1 s idle: {woke}");
    drop(capture);
    assert!(woke < 5, "reader woke {woke} times while idle");
}

#[test]
fn the_reader_picks_up_output_without_being_asked() {
    let capture = HookCapture::new().unwrap();
    let mut writer = capture.try_clone().unwrap();
    writer.write_all(b"hello\n").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    // Looks at the shared state only, so position() cannot do the read.
    while capture.state.lock().unwrap().total < 6 {
        assert!(std::time::Instant::now() < deadline, "reader never woke");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(capture.state.lock().unwrap().bytes, b"hello\n");
}

#[test]
fn position_counts_everything_written_before_the_call() {
    let capture = HookCapture::new().unwrap();
    let mut writer = capture.try_clone().unwrap();
    let mut written = 0;
    for round in 0..200 {
        let line = format!("line {round}\n");
        writer.write_all(line.as_bytes()).unwrap();
        written += line.len() as u64;
        assert_eq!(capture.position().unwrap(), written);
    }
    let snapshot = capture.finish().unwrap();
    assert_eq!(snapshot.total, written);
}

/// Runs `end` on another thread and fails instead of hanging.
fn finishes_promptly(end: impl FnOnce() + Send + 'static) {
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        end();
        let _ = sender.send(());
    });
    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("the reader stayed blocked");
}

#[test]
fn finish_ends_the_reader_while_a_grandchild_holds_the_write_end() {
    let capture = HookCapture::new().unwrap();
    let mut held = capture.try_clone().unwrap();
    held.write_all(b"before\n").unwrap();
    // Let the reader block in its wait first.
    thread::sleep(Duration::from_millis(50));
    held.write_all(b"last\n").unwrap();
    finishes_promptly(move || {
        let mut snapshot = capture.finish().unwrap();
        assert_eq!(snapshot.size().unwrap(), 12);
        assert_eq!(snapshot.tail().unwrap(), "before\nlast");
    });
    drop(held);
}

#[test]
fn dropping_the_capture_ends_the_reader_while_a_grandchild_holds_the_write_end() {
    let capture = HookCapture::new().unwrap();
    let held = capture.try_clone().unwrap();
    thread::sleep(Duration::from_millis(50));
    finishes_promptly(move || drop(capture));
    drop(held);
}

#[test]
fn capture_finish_drains_pending_output() {
    for _ in 0..32 {
        let capture = HookCapture::new().unwrap();
        let mut writer = capture.try_clone().unwrap();
        writer.write_all(b"before\n").unwrap();
        drop(writer);
        let mut snapshot = capture.finish().unwrap();
        assert_eq!(snapshot.size().unwrap(), 7);
        assert_eq!(snapshot.tail().unwrap(), "before");
    }
}

#[test]
fn silent_cleanup_after_chatty_before_hides_before_output() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let config = paths.environment_config("app");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        format!(
            "kind = 'native'\n[hooks]\nbefore = 'yes x | head -c 4194304; echo before-final'\nafter = 'true'\n[targets.app]\nexe = '{}'\n",
            root.path().join("missing").display()
        ),
    )
    .unwrap();
    let capture = hook_capture(&paths, "app", "app").unwrap().unwrap();
    let mut after_output_start = None;
    let error = launch::start(
        &paths,
        "app",
        Overrides::default(),
        &root.path().join("unused-supervisor"),
        &Token::new(),
        wineo_core::hooks::Output::File(capture.try_clone().unwrap()),
        Output::Inherit,
        |event| {
            if matches!(event, launch::LaunchEvent::AfterHook(_)) {
                after_output_start = Some(capture.position().unwrap());
            }
        },
    )
    .unwrap_err();
    let mut snapshot = capture.finish().unwrap();
    let before_end = after_output_start.expect("cleanup should run");
    assert!(before_end > 4 * 1024 * 1024);
    assert_eq!(snapshot.total, before_end);
    assert!(snapshot.tail().unwrap().ends_with("before-final"));
    assert!(
        launch_error(&error, Some(&mut snapshot), after_output_start)
            .output
            .is_none()
    );
}

#[test]
fn capture_load_failure_defers_to_launch() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let config = paths.environment_config("app");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(config, "not valid toml = [").unwrap();
    assert!(hook_capture(&paths, "app", "app").unwrap().is_none());
    assert!(
        launch::prepare(
            &paths,
            "app",
            &Overrides::default(),
            &Token::new(),
            wineo_core::hooks::Output::Inherit,
            |_| {},
        )
        .is_err()
    );
}
