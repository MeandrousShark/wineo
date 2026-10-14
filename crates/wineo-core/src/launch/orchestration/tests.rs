use crate::test_script::write_script;
use std::ffi::OsString;
use std::fs;

use super::*;

#[test]
fn launcher_cleanup_after_writes_to_the_callers_output() {
    for stage in ["prepare", "start", "prepared"] {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let config = paths.environment_config("app");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        let exe = if stage == "prepare" {
            root.path().join("missing")
        } else {
            PathBuf::from("/bin/sh")
        };
        fs::write(
                config,
                format!(
                    "kind = 'native'\n[hooks]\nbefore = 'echo before'\nafter = 'echo after; echo cleanup-error >&2'\n[targets.app]\nexe = '{}'\n",
                    exe.display()
                ),
            )
            .unwrap();
        let mut capture = tempfile::tempfile().unwrap();
        let output = hooks::Output::File(capture.try_clone().unwrap());
        match stage {
            "prepare" => assert!(
                prepare(
                    &paths,
                    "app",
                    &Overrides::default(),
                    &Token::new(),
                    output,
                    |_| {},
                )
                .is_err()
            ),
            "start" => assert!(
                start(
                    &paths,
                    "app",
                    Overrides::default(),
                    &root.path().join("missing-supervisor"),
                    &Token::new(),
                    output,
                    Output::Inherit,
                    |_| {},
                )
                .is_err()
            ),
            "prepared" => {
                let mut prepared = prepare(
                    &paths,
                    "app",
                    &Overrides::default(),
                    &Token::new(),
                    output,
                    |_| {},
                )
                .unwrap();
                prepared.failed(&paths, None, &Token::new());
            }
            _ => unreachable!(),
        }
        use std::io::{Read, Seek};
        capture.rewind().unwrap();
        let mut captured = String::new();
        capture.read_to_string(&mut captured).unwrap();
        assert_eq!(captured, "before\nafter\ncleanup-error\n", "{stage}");
    }
}

#[test]
fn reconcile_uses_the_component_build_locked_by_the_launch() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let runner_bin = paths.default_runner_root().join("fake/bin");
    fs::create_dir_all(&runner_bin).unwrap();
    for name in ["wine", "wineserver"] {
        let path = runner_bin.join(name);
        write_script(&path, "#!/bin/sh\nexit 0\n");
    }
    let prefix = paths.default_prefix_root().join("app");
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let install_dxvk = |name: &str| {
        for arch in ["x64", "x32"] {
            let dir = paths.default_dxvk_root().join(name).join(arch);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("dxgi.dll"), name).unwrap();
        }
    };
    install_dxvk("1.0");
    let path = paths.environment_config("app");
    let environment = Environment {
            name: "app".to_owned(),
            path: path.clone(),
            config: crate::config::EnvironmentConfig::parse(
                &path,
                &format!(
                    "prefix = '{}'\nrunner = 'fake'\narch = 'win64'\ndxvk = 'latest'\n[targets.app]\nexe = 'app'\n",
                    prefix.display()
                ),
            )
            .unwrap(),
        };
    let global = GlobalConfig::default();
    let launch = Launch::target(&paths, &global, &environment, &Overrides::default()).unwrap();
    install_dxvk("2.0");
    reconcile(&paths, &global, &environment, &launch, |_| {}).unwrap();
    assert_eq!(
        install::installed(
            launch.host.runner().unwrap(),
            &prefix,
            crate::components::Component::Dxvk
        )
        .unwrap()
        .as_deref(),
        Some("1.0")
    );
    assert!(
        crate::build_lock::try_exclusive(&paths, crate::build_lock::Kind::Dxvk, "1.0")
            .unwrap()
            .is_none()
    );
}

#[test]
fn failed_preparation_waits_for_same_name_shared_users_before_after() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let locks = Locks::new(&paths);
    let held = locks.acquire(Operation::Launch, "app", &[]).unwrap();
    let captured = root.path().join("after");
    let after = AfterHook {
        steam: None,
        paths: paths.clone(),
        environment: "app".to_owned(),
        target: "app".to_owned(),
        exe: "missing".to_owned(),
        wine: None,
        command: format!("printf '%s\\n' \"$WINEO_END\" >> '{}'", captured.display()),
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        failed_after(
            &paths,
            "app",
            Some(&after),
            None,
            1,
            None,
            &Token::new(),
            hooks::Output::Inherit,
        );
        sender.send(()).unwrap();
    });
    let result = receiver.recv_timeout(Duration::from_millis(250));
    let admission = locks.acquire(Operation::Launch, "app", &[]);
    let blocked = matches!(
        admission,
        Err(Error::Busy {
            holder: Some(Operation::Hook(Phase::After)),
            ..
        })
    );
    drop(admission);
    drop(held);
    worker.join().unwrap();
    assert!(
        matches!(result, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
        "after must wait for the existing shared user"
    );
    assert!(
        blocked,
        "new shared users must not enter while cleanup waits"
    );
    assert_eq!(fs::read_to_string(captured).unwrap(), "failed\n");
}

#[test]
fn launcher_reservation_blocks_exclusive_operations_between_before_and_failed_after() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let locks = Locks::new(&paths);
    let reservation = locks.reserve_launch("app").unwrap();
    let before = reservation
        .acquire(Operation::Hook(Phase::Before), &[])
        .unwrap();
    let context = hooks::Context {
        steam: None,
        environment: "app",
        target: "app",
        exe: "missing",
        wine: None,
        hook: Hook::Before,
    };
    assert!(matches!(
        hooks::run(
            "true",
            &paths,
            &context,
            &before,
            &Token::new(),
            Duration::from_secs(1),
            hooks::Output::Inherit,
        ).unwrap(),
        hooks::Outcome::Finished(status) if status.success()
    ));
    drop(before);
    let config = paths.environment_config("app");
    let environment = Environment {
        name: "app".to_owned(),
        path: config.clone(),
        config: crate::config::EnvironmentConfig::parse(
            &config,
            &format!(
                "kind = 'native'\n[targets.app]\nexe = '{}'\n",
                root.path().join("missing").display()
            ),
        )
        .unwrap(),
    };
    assert!(
        prepare_target_reserved(
            &paths,
            &GlobalConfig::default(),
            &environment,
            &Overrides::default(),
            &Token::new(),
            Some(&reservation),
            |_| {},
        )
        .is_err()
    );
    // Early preparation failures have no operation guard to protect cleanup.
    for operation in [Operation::Snapshot, Operation::Winetricks] {
        assert!(matches!(
            locks.acquire(operation, "app", &[]),
            Err(Error::Busy { .. })
        ));
    }
    let captured = root.path().join("after");
    let after = AfterHook {
        steam: None,
        paths: paths.clone(),
        environment: "app".to_owned(),
        target: "app".to_owned(),
        exe: "missing".to_owned(),
        wine: None,
        command: format!("printf '%s\\n' \"$WINEO_END\" >> '{}'", captured.display()),
    };
    failed_after(
        &paths,
        "app",
        Some(&after),
        None,
        1,
        None,
        &Token::new(),
        hooks::Output::Inherit,
    );
    assert_eq!(fs::read_to_string(captured).unwrap(), "failed\n");
    assert!(matches!(
        locks.acquire(Operation::Snapshot, "app", &[]),
        Err(Error::Busy { .. })
    ));
    drop(reservation);
    locks.acquire(Operation::Snapshot, "app", &[]).unwrap();
}

#[test]
fn detached_supervisor_failures_run_after_once_in_the_launching_process() {
    for reply in [None, Some("failed spawn failed"), Some("no report")] {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let captured = root.path().join("after");
        let command = format!(
            "printf '%s|%s|%s|%s\\n' \"$WINEO_END\" \"$WINEO_EXIT_CODE\" \"$WINEO_SECONDS\" \"$PPID\" >> '{}'",
            captured.display()
        );
        let config = paths.environment_config("app");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
                &config,
                format!(
                    "kind = 'native'\n[hooks]\nbefore = 'true'\nafter = {}\n[targets.app]\nexe = '/bin/sh'\n",
                    toml::Value::String(command)
                ),
            ).unwrap();
        let exe = root.path().join("supervisor");
        if let Some(reply) = reply {
            write_script(
                &exe,
                format!("#!/bin/sh\ncat >/dev/null\nprintf '{reply}\\n'\n"),
            );
        }
        assert!(
            start(
                &paths,
                "app",
                Overrides::default(),
                &exe,
                &Token::new(),
                hooks::Output::Inherit,
                Output::Inherit,
                |_| {},
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(captured).unwrap(),
            format!("failed||0|{}\n", std::process::id())
        );
        assert!(Locks::new(&paths).busy_with("app").unwrap().is_none());
        assert!(!Runtime::new(&paths, "app").is_running().unwrap());
    }
}

#[test]
fn detached_start_reports_supervisor_outcomes_and_event_order() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    let config = paths.environment_config("app");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "kind = 'native'\n[targets.app]\nexe = '/bin/sh'\n").unwrap();

    for (reply, expected) in [
        ("started 42", StartOutcome::Started),
        ("already-running", StartOutcome::AlreadyRunning),
    ] {
        let exe = root.path().join("supervisor");
        write_script(
            &exe,
            format!("#!/bin/sh\ncat >/dev/null\nprintf '{reply}\\n'\n"),
        );
        let mut events = Vec::new();
        let outcome = start(
            &paths,
            "app",
            Overrides::default(),
            &exe,
            &Token::new(),
            hooks::Output::Inherit,
            Output::Inherit,
            |event| events.push(event),
        )
        .unwrap();
        assert_eq!(outcome, expected);
        assert!(matches!(events.first(), Some(LaunchEvent::Starting)));
        assert_eq!(
            events
                .iter()
                .any(|event| matches!(event, LaunchEvent::Started)),
            expected == StartOutcome::Started
        );
    }
}
