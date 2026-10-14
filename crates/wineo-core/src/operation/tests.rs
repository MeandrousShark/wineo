use std::os::unix::fs::symlink;

use super::*;

#[test]
fn the_after_hook_backs_off_to_the_cap() {
    let mut retry = LOCK_RETRY;
    let mut seen = Vec::new();
    for _ in 0..9 {
        seen.push(retry.as_millis());
        retry = drain_backoff(retry);
    }
    assert_eq!(seen, [5, 10, 20, 40, 80, 160, 250, 250, 250]);
}

fn locks(root: &Path) -> Locks {
    Locks::at(root.join("locks"))
}

#[test]
fn operations_serialize_as_json_strings_and_round_trip() {
    for operation in [
        Operation::Create,
        Operation::Remove,
        Operation::Winetricks,
        Operation::Configure,
        Operation::Launch,
        Operation::Install,
        Operation::Cleanup,
        Operation::Snapshot,
        Operation::Copy,
        Operation::Trash,
        Operation::Migrate,
        Operation::Update,
        Operation::Hook(Phase::Before),
        Operation::Hook(Phase::After),
    ] {
        let json = serde_json::to_string(&operation).unwrap();
        assert_eq!(json, format!("\"{}\"", operation.as_str()));
        assert_eq!(serde_json::from_str::<Operation>(&json).unwrap(), operation);
        let state = crate::runtime::OperationState {
            operation,
            program: "setup.exe".to_owned(),
            runner: crate::runner::Runner::fake("wine"),
            prefix: PathBuf::from("/tmp/prefix"),
            pid: None,
            group: None,
            boot_id: None,
            leader_start: None,
            started_at: jiff::Timestamp::now(),
        };
        let text = toml::to_string(&state).unwrap();
        assert!(text.contains(&format!("operation = \"{}\"", operation.as_str())));
        let restored: crate::runtime::OperationState = toml::from_str(&text).unwrap();
        assert_eq!(restored.operation, operation);
        assert_eq!(restored.program, state.program);
        assert_eq!(restored.prefix, state.prefix);
    }
    assert!(serde_json::from_str::<Operation>("\"unknown\"").is_err());
}

#[test]
fn operation_state_reads_legacy_toml_representations() {
    #[derive(Deserialize)]
    struct Record {
        operation: Operation,
    }
    assert_eq!(
        toml::from_str::<Record>("operation = 'install'")
            .unwrap()
            .operation,
        Operation::Install
    );
    for phase in [Phase::Before, Phase::After] {
        let text = format!("[operation]\nhook = '{}'\n", phase.as_str());
        assert_eq!(
            toml::from_str::<Record>(&text).unwrap().operation,
            Operation::Hook(phase)
        );
    }
}

#[test]
fn hook_locks_are_exclusive_and_busy_messages_name_the_phase() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("prefix");
    for phase in [Phase::Before, Phase::After] {
        let held = locks
            .acquire(Operation::Hook(phase), "hades", &[&prefix])
            .unwrap();
        assert_eq!(
            locks.busy_with("hades").unwrap(),
            Some(Operation::Hook(phase))
        );
        for operation in [
            Operation::Launch,
            Operation::Install,
            Operation::Hook(phase),
        ] {
            let err = locks.acquire(operation, "hades", &[&prefix]).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("hades is busy running its {} hook", phase.as_str())
            );
            assert!(
                matches!(err, Error::Busy { holder: Some(Operation::Hook(found)), .. } if found == phase)
            );
        }
        let alias = locks
            .acquire(Operation::Launch, "other", &[&prefix])
            .unwrap();
        let other_hook = locks
            .acquire(Operation::Hook(phase), "third", &[&prefix])
            .unwrap();
        drop(other_hook);
        drop(alias);
        drop(held);
        assert_eq!(locks.busy_with("hades").unwrap(), None);
    }
}

#[test]
fn after_handoff_does_not_wait_for_another_environment_sharing_the_prefix() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("prefix");
    let other = locks
        .acquire(Operation::Launch, "other", &[&prefix])
        .unwrap();
    let launch = locks
        .acquire(Operation::Launch, "hades", &[&prefix])
        .unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker_locks = locks.clone();
    let worker = thread::spawn(move || {
        let _ = sender.send(worker_locks.after_launch(launch, "hades"));
    });
    let result = receiver.recv_timeout(std::time::Duration::from_secs(5));
    if result.is_err() {
        drop(other);
        worker.join().unwrap();
        panic!("after handoff waited for another environment's prefix lock");
    }
    let after = result.unwrap().unwrap();
    worker.join().unwrap();
    assert_eq!(
        locks.busy_with("hades").unwrap(),
        Some(Operation::Hook(Phase::After))
    );
    assert!(
        locks
            .acquire(Operation::Launch, "hades", &[&prefix])
            .is_err()
    );
    let alias = locks
        .acquire(Operation::Launch, "alias", &[&prefix])
        .unwrap();
    drop(alias);
    drop(after);
    drop(other);
}

#[test]
fn hook_group_records_identity_and_ignores_stale_contents() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let held = locks
        .acquire(Operation::Hook(Phase::Before), "hades", &[])
        .unwrap();
    let running = Identity {
        group: 123,
        boot_id: "boot".to_owned(),
        leader_start: 456,
    };
    held.record_hook(Phase::Before, Some(&running)).unwrap();
    assert_eq!(
        locks.hook_group("hades").unwrap(),
        Some((Phase::Before, running))
    );
    drop(held);
    assert!(locks.hook_group("hades").unwrap().is_none());
    fs::write(locks.name_path("hades"), "hook-before 1 123 boot 456\n").unwrap();
    assert!(locks.hook_group("hades").unwrap().is_none());
}

#[test]
fn hook_stop_requests_match_the_phase_and_ignore_an_old_before_request() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let guard = locks
        .acquire(Operation::Hook(Phase::After), "app", &[])
        .unwrap();
    let running = Identity {
        group: 123,
        boot_id: "boot".to_owned(),
        leader_start: 456,
    };
    guard.record_hook(Phase::After, Some(&running)).unwrap();
    let path = locks.name_path("app");
    let original = fs::read_to_string(&path).unwrap();
    assert!(
        locks
            .request_hook_stop("app", Phase::Before, false)
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    // Stand in for before finishing between the phase check and the append.
    writeln!(
        OpenOptions::new().append(true).open(&path).unwrap(),
        "cancel-before"
    )
    .unwrap();
    let token = crate::cancel::Token::new();
    guard.hook_request(&token).unwrap();
    assert!(!token.is_canceled());
    assert_eq!(
        locks.request_hook_stop("app", Phase::After, true).unwrap(),
        Some(running)
    );
    guard.hook_request(&token).unwrap();
    assert!(token.is_forced());
}

#[test]
fn pinned_after_force_ignores_a_stale_request_on_a_later_run() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let guard = locks
        .acquire(Operation::Hook(Phase::After), "app", &[])
        .unwrap();
    let first = Identity {
        group: 123,
        boot_id: "boot".to_owned(),
        leader_start: 456,
    };
    let later = Identity {
        group: 124,
        boot_id: "boot".to_owned(),
        leader_start: 789,
    };
    guard.record_hook(Phase::After, Some(&first)).unwrap();
    guard.record_hook(Phase::After, Some(&later)).unwrap();
    let path = locks.name_path("app");
    writeln!(
        OpenOptions::new().append(true).open(&path).unwrap(),
        "force-after-for {} {} {}",
        first.group,
        first.boot_id,
        first.leader_start
    )
    .unwrap();
    let token = crate::cancel::Token::new();
    guard.hook_request(&token).unwrap();
    assert!(!token.is_forced());
    assert!(!locks.request_hook_force_for("app", &first).unwrap());
    assert!(locks.request_hook_force_for("app", &later).unwrap());
    guard.hook_request(&token).unwrap();
    assert!(token.is_forced());
}

#[test]
fn hook_operations_round_trip_in_toml() {
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Record {
        operation: Operation,
    }
    for phase in [Phase::Before, Phase::After] {
        let record = Record {
            operation: Operation::Hook(phase),
        };
        let text = toml::to_string(&record).unwrap();
        assert_eq!(toml::from_str::<Record>(&text).unwrap(), record);
    }
}

#[test]
fn launches_share_an_environment() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("p");
    let first = locks
        .acquire(Operation::Launch, "notepad", &[&prefix])
        .unwrap();
    let second = locks
        .acquire(Operation::Launch, "notepad", &[&prefix])
        .unwrap();
    drop((first, second));
}

#[test]
fn an_exclusive_operation_excludes_everything_else() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("p");

    let held = locks
        .acquire(Operation::Winetricks, "notepad", &[&prefix])
        .unwrap();
    for operation in [
        Operation::Launch,
        Operation::Remove,
        Operation::Winetricks,
        Operation::Configure,
    ] {
        let err = locks.acquire(operation, "notepad", &[&prefix]).unwrap_err();
        assert!(
            matches!(
                &err,
                Error::Busy {
                    holder: Some(Operation::Winetricks),
                    ..
                }
            ),
            "{operation:?}: {err}"
        );
    }
    drop(held);
    drop(
        locks
            .acquire(Operation::Remove, "notepad", &[&prefix])
            .unwrap(),
    );
}

#[test]
fn a_launch_blocks_an_exclusive_operation() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("p");
    let held = locks
        .acquire(Operation::Launch, "notepad", &[&prefix])
        .unwrap();
    let err = locks
        .acquire(Operation::Remove, "notepad", &[&prefix])
        .unwrap_err();
    assert!(
        matches!(
            &err,
            Error::Busy {
                holder: Some(Operation::Launch),
                ..
            }
        ),
        "{err}"
    );
    drop(held);
}

#[test]
fn different_environments_do_not_block_each_other() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let held = locks
        .acquire(
            Operation::Winetricks,
            "notepad",
            &[&root.path().join("one")],
        )
        .unwrap();
    drop(
        locks
            .acquire(Operation::Remove, "other", &[&root.path().join("two")])
            .unwrap(),
    );
    drop(held);
}

#[test]
fn two_names_for_one_prefix_still_exclude_each_other() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("prefix");
    fs::create_dir_all(&prefix).unwrap();
    let alias = root.path().join("alias");
    symlink(&prefix, &alias).unwrap();

    let held = locks
        .acquire(Operation::Winetricks, "one", &[&prefix])
        .unwrap();
    let err = locks
        .acquire(Operation::Remove, "two", &[&alias])
        .unwrap_err();
    assert!(
        matches!(&err, Error::Busy { subject, .. } if subject.starts_with("prefix ")),
        "{err}"
    );
    drop(held);
    drop(locks.acquire(Operation::Remove, "two", &[&alias]).unwrap());
}

#[test]
fn a_prefix_that_does_not_exist_yet_still_gets_one_lock() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let missing = root.path().join("later/prefix");
    let alias = root.path().join("later/../later/prefix");
    assert_eq!(locks.prefix_path(&missing), locks.prefix_path(&alias));
}

#[test]
fn busy_with_reports_only_exclusive_holders() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("p");
    assert_eq!(locks.busy_with("notepad").unwrap(), None);

    let launch = locks
        .acquire(Operation::Launch, "notepad", &[&prefix])
        .unwrap();
    assert_eq!(locks.busy_with("notepad").unwrap(), None);
    drop(launch);

    let held = locks
        .acquire(Operation::Winetricks, "notepad", &[&prefix])
        .unwrap();
    assert_eq!(
        locks.busy_with("notepad").unwrap(),
        Some(Operation::Winetricks)
    );
    drop(held);
    assert_eq!(locks.busy_with("notepad").unwrap(), None);
}

#[test]
fn a_failed_acquire_holds_nothing() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("p");
    // Holds the prefix under another name, so the name lock is free and the
    // prefix lock is not. The name lock must not stay held after the failure.
    let held = locks
        .acquire(Operation::Winetricks, "other", &[&prefix])
        .unwrap();
    assert!(
        locks
            .acquire(Operation::Remove, "notepad", &[&prefix])
            .is_err()
    );
    drop(locks.acquire(Operation::Remove, "notepad", &[]).unwrap());
    drop(held);
}

#[test]
fn relabeling_keeps_the_locks_and_changes_what_others_see() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("p");
    let mut held = locks
        .acquire(Operation::Create, "notepad", &[&prefix])
        .unwrap();
    held.relabel(Operation::Install).unwrap();
    assert_eq!(
        locks.busy_with("notepad").unwrap(),
        Some(Operation::Install)
    );
    let err = locks
        .acquire(Operation::Launch, "notepad", &[&prefix])
        .unwrap_err();
    assert!(
        matches!(
            &err,
            Error::Busy {
                holder: Some(Operation::Install),
                ..
            }
        ),
        "{err}"
    );
    drop(held);
    assert_eq!(locks.busy_with("notepad").unwrap(), None);
}

#[test]
fn relabel_never_lets_go_of_the_lock() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("p");
    let mut held = locks
        .acquire(Operation::Create, "notepad", &[&prefix])
        .unwrap();
    assert!(
        locks
            .acquire(Operation::Remove, "notepad", &[&prefix])
            .is_err()
    );
    held.relabel(Operation::Install).unwrap();
    // Still held right after relabeling: it never let go in between.
    assert!(
        locks
            .acquire(Operation::Remove, "notepad", &[&prefix])
            .is_err()
    );
    drop(held);
    assert!(
        locks
            .acquire(Operation::Remove, "notepad", &[&prefix])
            .is_ok()
    );
}

#[test]
fn guard_releases_its_locks_even_if_a_forked_child_still_holds_the_fd() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("p");
    let held = locks
        .acquire(Operation::Winetricks, "notepad", &[&prefix])
        .unwrap();
    // Stand in for a forked child's copies of the fds, kept alive past the drop.
    let clones: Vec<File> = held
        .files
        .iter()
        .map(|(file, _)| file.try_clone().unwrap())
        .collect();
    drop(held);
    assert!(
        locks
            .acquire(Operation::Remove, "notepad", &[&prefix])
            .is_ok()
    );
    drop(clones);
}

#[test]
fn config_guard_releases_its_lock_even_if_a_forked_child_still_holds_the_fd() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let config = root.path().join("environments/notepad.toml");
    let held = locks.config(&config).unwrap();
    // Stands in for a forked child's copy of the fd, kept alive past the drop.
    let clone = held.file.try_clone().unwrap();
    drop(held);
    assert!(locks.config(&config).is_ok());
    drop(clone);
}

#[test]
fn probe_releases_its_lock_even_if_a_forked_child_still_holds_the_fd() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("probe.lock");
    let file = open(&path).unwrap();
    // Stands in for a forked child's copy of the fd, kept alive past the probe.
    let clone = file.try_clone().unwrap();
    assert!(probe(&file, &path).unwrap());
    drop(file);
    let fresh = open(&path).unwrap();
    assert!(take(&fresh, &path, false, 1).unwrap());
    drop(clone);
}

#[test]
fn a_config_lock_excludes_another_writer_of_the_same_file() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let config = root.path().join("environments/notepad.toml");
    let held = locks.config(&config).unwrap();
    let err = locks.config_lock(&config, 2).unwrap_err();
    assert!(
        matches!(&err, Error::Busy { subject, .. } if subject.starts_with("config ")),
        "{err}"
    );
    // Another file has a lock of its own.
    drop(
        locks
            .config(&root.path().join("environments/other.toml"))
            .unwrap(),
    );
    drop(held);
    drop(locks.config(&config).unwrap());
}

#[test]
fn a_config_write_waits_for_the_one_before_it() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let config = root.path().join("config.toml");
    let held = locks.config(&config).unwrap();
    let waiter = {
        let (locks, config) = (locks.clone(), config.clone());
        thread::spawn(move || locks.config(&config).map(drop))
    };
    thread::sleep(Duration::from_millis(50));
    drop(held);
    waiter.join().unwrap().unwrap();
}

// `set` has to be able to change a target while a game runs, which holds
// the environment's operation lock the whole time.
#[test]
fn config_locks_are_independent_of_operation_locks() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    let prefix = root.path().join("p");
    let config = root.path().join("environments/notepad.toml");
    let held = locks
        .acquire(Operation::Winetricks, "notepad", &[&prefix])
        .unwrap();
    drop(locks.config(&config).unwrap());
    drop(held);
}

#[test]
fn locks_live_outside_the_runtime_dirs() {
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| std::ffi::OsString::from("/home/me")))
            .unwrap();
    let locks = Locks::new(&paths);
    assert_eq!(locks.dir, Path::new("/home/me/.local/state/wineo/.locks"));
    // `.locks` is not a valid environment name, so it can never be one.
    assert!(!crate::name::is_valid(".locks"));
}

#[test]
fn interrupted_restore_blocks_ordinary_operation_after_locking() {
    let root = tempfile::tempdir().unwrap();
    let locks = locks(root.path());
    fs::create_dir_all(&locks.dir).unwrap();
    fs::write(
        locks.dir.join("snapshot-restore-game-restore.json"),
        "marker",
    )
    .unwrap();
    let prefix = root.path().join("prefix");
    assert!(
        locks
            .acquire(Operation::Launch, "game", &[&prefix])
            .is_err()
    );
    assert!(
        locks
            .acquire(Operation::Remove, "game", &[&prefix])
            .is_err()
    );
    drop(
        locks
            .acquire(Operation::Snapshot, "game", &[&prefix])
            .unwrap(),
    );
}

fn rooted(root: &Path, portable: Option<&Path>) -> Paths {
    let home = root.to_owned();
    Paths::select(
        move |key| (key == "HOME").then(|| home.clone().into_os_string()),
        portable,
    )
    .unwrap()
}

#[test]
fn prefix_locks_are_shared_across_roots_and_name_locks_are_not() {
    let root = tempfile::tempdir().unwrap();
    let xdg = Locks::new(&rooted(root.path(), None));
    let portable = Locks::new(&rooted(root.path(), Some(&root.path().join("usb"))));
    let prefix = root.path().join("prefix");
    let other = root.path().join("other");

    let held = xdg
        .acquire(Operation::Winetricks, "game", &[&prefix])
        .unwrap();
    let err = portable
        .acquire(Operation::Remove, "else", &[&prefix])
        .unwrap_err();
    assert!(
        matches!(&err, Error::Busy { subject, .. } if subject.starts_with("prefix ")),
        "{err}"
    );
    // The same name in another root is a different environment.
    drop(
        portable
            .acquire(Operation::Remove, "game", &[&other])
            .unwrap(),
    );
    drop(held);
}

#[test]
fn a_portable_restore_marker_is_found_inside_the_folder() {
    let root = tempfile::tempdir().unwrap();
    let usb = root.path().join("usb");
    let paths = rooted(root.path(), Some(&usb));
    let marker = paths.restore_marker("game");
    assert_eq!(
        marker,
        usb.join("prefixes/.wineo-snapshots/game/restore.json")
    );
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, "marker").unwrap();
    let locks = Locks::new(&paths);
    let prefix = usb.join("prefixes/game");
    assert!(
        locks
            .acquire(Operation::Launch, "game", &[&prefix])
            .is_err()
    );
    drop(
        Locks::new(&rooted(root.path(), None))
            .acquire(Operation::Launch, "game", &[&root.path().join("xdg")])
            .unwrap(),
    );
}

#[test]
fn the_same_path_always_hashes_the_same() {
    assert_eq!(fnv1a(b"/home/me/prefix"), fnv1a(b"/home/me/prefix"));
    assert_ne!(fnv1a(b"/home/me/prefix"), fnv1a(b"/home/me/other"));
    assert_eq!(label("Photoshop-2026"), "Photoshop-2026");
    assert_eq!(label("weird name!"), "weirdname");
}
