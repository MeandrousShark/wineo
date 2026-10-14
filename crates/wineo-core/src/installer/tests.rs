use std::ffi::OsString;
use std::path::PathBuf;

use super::*;
use crate::runner::Runner;

fn paths(root: &Path) -> Paths {
    let home = root.to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
        .unwrap()
}

fn runtime(root: &Path) -> Runtime {
    Runtime::new(&paths(root), "hades")
}

fn installer(root: &Path, wine: &str) -> Installer {
    let locks = Locks::at(root.join("locks"));
    let prefix = root.join("prefix");
    Installer {
        guard: locks
            .acquire(Operation::Install, "hades", &[&prefix])
            .unwrap(),
        build_guard: None,
        launch: Launch {
            host: Host::Wine {
                runner: Runner::wine_build(
                    "fake".to_owned(),
                    PathBuf::from(wine),
                    PathBuf::from("/bin/true"),
                ),
                prefix,
            },
            target: None,
            args: vec!["setup.exe".to_owned()],
            cwd: root.to_owned(),
            env: BTreeMap::new(),
            wrappers: Default::default(),
            wayland: Default::default(),
            gameid: None,
            store: None,
            build_guards: Vec::new(),
        },
        runtime: runtime(root),
        program: "setup.exe".to_owned(),
    }
}

#[test]
fn unresolved_cleanup_holds_recorded_runner_and_component_locks() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let dir = paths.default_runner_root().join("recorded/bin");
    fs::create_dir_all(&dir).unwrap();
    for program in ["wine", "wineserver"] {
        std::os::unix::fs::symlink("/bin/true", dir.join(program)).unwrap();
    }
    let runner = Runners::new(&paths, &global)
        .unwrap()
        .resolve("recorded")
        .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("hades"),
        format!(
            "prefix = {:?}\nrunner = 'different'\narch = 'win64'\ndxvk = '2.0'\nvkd3d = '3.0'\n",
            root.path().join("prefix").display().to_string()
        ),
    )
    .unwrap();
    let record = OperationState {
        operation: Operation::Install,
        program: "setup.exe".to_owned(),
        runner,
        prefix: root.path().join("prefix"),
        pid: None,
        group: None,
        boot_id: None,
        leader_start: None,
        started_at: Timestamp::now(),
    };
    Runtime::new(&paths, "hades")
        .write_operation(&record)
        .unwrap();
    let references = unresolved_builds(&paths).unwrap();
    assert_eq!(
        references,
        vec![(
            "hades".to_owned(),
            build_lock::Kind::Runner,
            "recorded".to_owned()
        )]
    );
    let cleaned = clean_up_with(&paths, &global, "hades", Duration::from_secs(1), |target| {
        assert_eq!(target.runner.name(), "recorded");
        for (kind, build) in [
            (build_lock::Kind::Runner, "recorded"),
            (build_lock::Kind::Dxvk, "2.0"),
            (build_lock::Kind::Vkd3d, "3.0"),
        ] {
            assert!(build_lock::try_exclusive(&paths, kind, build)?.is_none());
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(cleaned, Some(Cleaned::Unresolved));
    assert!(
        build_lock::try_exclusive(&paths, build_lock::Kind::Runner, "recorded")
            .unwrap()
            .is_some()
    );
    fs::write(paths.environment_config("hades"), "invalid = [TOML").unwrap();
    assert_eq!(unresolved_builds(&paths).unwrap(), references);
    fs::remove_file(paths.environment_config("hades")).unwrap();
    assert_eq!(
        unresolved_builds(&paths).unwrap(),
        vec![(
            "hades".to_owned(),
            build_lock::Kind::Runner,
            "recorded".to_owned()
        )]
    );
}

#[test]
fn an_installer_that_cannot_start_leaves_no_record() {
    let root = tempfile::tempdir().unwrap();
    let err = installer(root.path(), "/nonexistent/wine")
        .run(
            &Token::new(),
            Duration::from_secs(1),
            Output::Inherit,
            |_| {},
        )
        .unwrap_err();
    assert!(err.to_string().contains("failed to start wine"), "{err}");
    assert!(runtime(root.path()).read_operation().unwrap().is_none());
}

#[test]
fn a_finished_installer_clears_its_record() {
    let root = tempfile::tempdir().unwrap();
    let outcome = installer(root.path(), "/bin/true")
        .run(
            &Token::new(),
            Duration::from_secs(1),
            Output::Inherit,
            |_| {},
        )
        .unwrap();
    assert!(
        matches!(outcome, Outcome::Finished { exit_code: Some(0) }),
        "{outcome:?}"
    );
    assert!(runtime(root.path()).read_operation().unwrap().is_none());
}

#[test]
fn a_token_cancels_an_installer_without_a_signal() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefix");
    fs::create_dir(&prefix).unwrap();
    let wine = root.path().join("wine");
    crate::test_script::write_script(
        &wine,
        "#!/bin/sh\nif [ \"$1\" = wineboot ]; then\n  touch \"$WINEPREFIX/stopped\"\n  exit 0\nfi\ntouch \"$WINEPREFIX/started\"\nwhile [ ! -e \"$WINEPREFIX/stopped\" ]; do sleep 0.02; done\n",
    );
    let installer = installer(root.path(), wine.to_str().unwrap());
    let started = prefix.join("started");
    let token = Token::new();
    let request = token.clone();
    let marker = started.clone();
    let canceler = thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !marker.exists() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        request.cancel();
    });
    let outcome = installer
        .run(&token, Duration::from_millis(100), Output::Inherit, |_| {})
        .unwrap();
    canceler.join().unwrap();
    assert!(started.exists());
    assert!(matches!(outcome, Outcome::Canceled { .. }), "{outcome:?}");
    assert!(runtime(root.path()).read_operation().unwrap().is_none());
}

#[test]
fn a_post_spawn_identity_write_failure_stops_the_live_child_before_returning() {
    let root = tempfile::tempdir().unwrap();
    let mut installer = installer(root.path(), "/bin/sleep");
    installer.launch.args = vec!["30".to_owned()];
    let mut writes = 0;
    let mut identity = None;
    let error = installer
        .run_with_writer(
            &Token::new(),
            Duration::from_secs(1),
            Output::Inherit,
            |_| {},
            |runtime, state| {
                writes += 1;
                if writes == 1 {
                    runtime.write_operation(state)
                } else {
                    identity = Some((state.group, state.boot_id.clone(), state.leader_start));
                    Err(Error::Unsettled(
                        "injected identity write failure".to_owned(),
                    ))
                }
            },
        )
        .unwrap_err();

    assert_eq!(error.to_string(), "injected identity write failure");
    assert_eq!(writes, 2);
    assert!(runtime(root.path()).read_operation().unwrap().is_none());
    let (Some(group), Some(boot), Some(leader_start)) = identity.unwrap() else {
        panic!("the launched child's full identity should be available");
    };
    assert_eq!(
        process::Identity {
            group,
            boot_id: boot,
            leader_start
        }
        .ownership(),
        process::GroupOwnership::Gone
    );
}

#[test]
fn a_post_spawn_identity_write_failure_keeps_unsettled_work_blocked() {
    let root = tempfile::tempdir().unwrap();
    let mut installer = installer(root.path(), "/bin/sh");
    let started = root.path().join("child-started");
    installer.launch.args = vec![
        "-c".to_owned(),
        format!("sleep 1 & echo ready > '{}'; wait", started.display()),
    ];
    let mut writes = 0;
    let mut group = None;
    let outcome = installer
        .run_with_writer(
            &Token::new(),
            Duration::from_secs(1),
            Output::Inherit,
            |_| {},
            |runtime, state| {
                writes += 1;
                if writes == 1 {
                    runtime.write_operation(state)
                } else {
                    group = state.group;
                    let deadline = std::time::Instant::now() + Duration::from_secs(2);
                    while !started.exists() && std::time::Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(Error::Unsettled(
                        "injected identity write failure".to_owned(),
                    ))
                }
            },
        )
        .unwrap();

    assert!(matches!(outcome, Outcome::Unresolved { .. }), "{outcome:?}");
    let runtime = runtime(root.path());
    assert!(runtime.read_operation().unwrap().is_some());
    assert!(matches!(
        runtime.check_resolved("hades"),
        Err(Error::Unresolved(_))
    ));
    let group = group.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while process::group_has_live_members(group) != Some(false)
        && std::time::Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(process::group_has_live_members(group), Some(false));
}

#[test]
fn output_can_go_to_a_log_that_each_run_replaces() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("state/install.log");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, "the previous run\n").unwrap();
    // `echo` stands in for wine, so what it prints is the arguments.
    installer(root.path(), "/bin/echo")
        .run(
            &Token::new(),
            Duration::from_secs(1),
            Output::Log(&log),
            |_| {},
        )
        .unwrap();
    assert_eq!(fs::read_to_string(&log).unwrap(), "setup.exe\n");
}

#[test]
fn records_round_trip_and_are_what_refuses_other_work() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    assert!(runtime.check_resolved("hades").is_ok());
    let record = OperationState {
        operation: Operation::Install,
        program: "setup.exe".to_owned(),
        runner: Runner::fake("wine"),
        prefix: root.path().join("prefix"),
        pid: Some(42),
        group: Some(42),
        boot_id: None,
        leader_start: None,
        started_at: Timestamp::now(),
    };
    runtime.write_operation(&record).unwrap();
    let read = runtime.read_operation().unwrap().unwrap();
    assert_eq!(read.group, Some(42));
    assert_eq!(read.runner.name(), "wine");
    assert!(matches!(
        runtime.check_resolved("hades"),
        Err(Error::Unresolved(_))
    ));

    // Garbage still counts, since it says something was left unresolved.
    fs::write(runtime.dir().join("operation.toml"), "not toml [").unwrap();
    assert!(matches!(
        runtime.check_resolved("hades"),
        Err(Error::Unresolved(_))
    ));
    runtime.clear_operation().unwrap();
    runtime.clear_operation().unwrap();
    assert!(runtime.check_resolved("hades").is_ok());
}

#[test]
fn cleanup_keeps_an_old_identity_unresolved_and_blocked() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runtime = Runtime::new(&paths, "hades");
    let record = OperationState {
        operation: Operation::Install,
        program: "setup.exe".to_owned(),
        runner: Runner::fake("wine"),
        prefix: root.path().join("prefix-alias"),
        pid: Some(1234),
        group: Some(1234),
        boot_id: None,
        leader_start: None,
        started_at: Timestamp::now(),
    };
    runtime.write_operation(&record).unwrap();

    let result = clean_up(
        &paths,
        &GlobalConfig::default(),
        "hades",
        Duration::from_secs(1),
    )
    .unwrap();

    assert_eq!(result, Some(Cleaned::Unresolved));
    assert!(runtime.read_operation().unwrap().is_some());
    assert!(matches!(
        runtime.check_resolved("hades"),
        Err(Error::Unresolved(_))
    ));
}

#[test]
fn cleanup_does_not_remove_an_unreadable_operation_record() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runtime = Runtime::new(&paths, "hades");
    fs::create_dir_all(runtime.dir()).unwrap();
    let text = "not toml [";
    fs::write(runtime.dir().join("operation.toml"), text).unwrap();

    let result = clean_up(
        &paths,
        &GlobalConfig::default(),
        "hades",
        Duration::from_secs(1),
    );

    assert!(matches!(result, Err(Error::StateSyntax { .. })));
    assert_eq!(
        fs::read_to_string(runtime.dir().join("operation.toml")).unwrap(),
        text
    );
}
