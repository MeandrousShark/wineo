use std::ffi::OsString;

use jiff::Timestamp;

use super::*;
use crate::library::{Entry, SortKey};

fn paths(home: &Path, portable: Option<&Path>) -> Paths {
    let home = home.to_owned();
    Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        portable,
    )
    .unwrap()
}

fn test_entry(favorite: bool, hidden: bool, seconds: Option<i64>) -> Entry {
    Entry {
        favorite,
        hidden,
        added: seconds.map(|seconds| Timestamp::from_second(seconds).unwrap()),
    }
}

#[test]
fn portable_copies_transfer_library_entry_and_keep_destination_view_and_entries() {
    for native in [false, true] {
        let f = Fixture::new(native);
        let copied = test_entry(true, true, Some(1_792_254_000));
        let other = test_entry(false, true, Some(1_792_200_000));
        let old_game = test_entry(false, false, None);
        library::put(&f.from, "game", Some(&copied)).unwrap();
        library::set_sort(&f.to, SortKey::Added).unwrap();
        library::set_descending(&f.to, true).unwrap();
        library::set_favorites_first(&f.to, true).unwrap();
        library::put(&f.to, "other", Some(&other)).unwrap();
        library::put(&f.to, "game", Some(&old_game)).unwrap();

        let mut events = Vec::new();
        let environment = if native {
            let review = inspect_native(&f.from, &f.to, "game").unwrap();
            execute_native_with(&f.from, &f.to, &review, &Token::new(), |event| {
                events.push(event)
            })
            .unwrap()
        } else {
            let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
            execute_reviewed_with(&f.from, &f.to, &f.global, &review, &Token::new(), |event| {
                events.push(event)
            })
            .unwrap()
        };

        assert_eq!(environment.name, "game");
        let library = library::load(&f.to);
        assert_eq!(library.environments.get("game"), Some(&copied));
        assert_eq!(library.environments.get("other"), Some(&other));
        assert_eq!(library.view.sort, SortKey::Added);
        assert!(library.view.descending);
        assert!(library.view.favorites_first);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, CopyEvent::Warning(_)))
        );
    }
}

#[test]
fn portable_copies_warn_and_leave_a_broken_destination_library_unchanged() {
    for native in [false, true] {
        let f = Fixture::new(native);
        library::put(
            &f.from,
            "game",
            Some(&test_entry(true, true, Some(1_792_254_000))),
        )
        .unwrap();
        fs::create_dir_all(f.to.library_file().parent().unwrap()).unwrap();
        let broken = b"[view\nsort = \"added\"\n";
        fs::write(f.to.library_file(), broken).unwrap();

        let mut warnings = Vec::new();
        let environment = if native {
            let review = inspect_native(&f.from, &f.to, "game").unwrap();
            execute_native_with(&f.from, &f.to, &review, &Token::new(), |event| {
                if let CopyEvent::Warning(warning) = event {
                    warnings.push(warning);
                }
            })
            .unwrap()
        } else {
            let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
            execute_reviewed_with(&f.from, &f.to, &f.global, &review, &Token::new(), |event| {
                if let CopyEvent::Warning(warning) = event {
                    warnings.push(warning);
                }
            })
            .unwrap()
        };

        assert_eq!(environment.name, "game");
        assert!(f.to.environment_config("game").exists());
        assert_eq!(fs::read(f.to.library_file()).unwrap(), broken);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("Can't update library.toml: it can't be read: "));
        assert!(!warnings[0].contains(".:"), "{warnings:?}");
        assert!(warnings[0].contains("TOML parse error"), "{warnings:?}");
    }
}

#[test]
fn portable_copies_warn_and_leave_the_destination_alone_when_the_source_library_is_broken() {
    for native in [false, true] {
        let f = Fixture::new(native);
        fs::create_dir_all(f.from.library_file().parent().unwrap()).unwrap();
        fs::write(
            f.from.library_file(),
            "[environments.game]\nfavorite = true\n[broken\n",
        )
        .unwrap();
        library::put(
            &f.to,
            "game",
            Some(&test_entry(true, false, Some(1_792_254_000))),
        )
        .unwrap();
        let before = fs::read(f.to.library_file()).unwrap();

        let mut warnings = Vec::new();
        let mut collect = |event| {
            if let CopyEvent::Warning(warning) = event {
                warnings.push(warning);
            }
        };
        if native {
            let review = inspect_native(&f.from, &f.to, "game").unwrap();
            execute_native_with(&f.from, &f.to, &review, &Token::new(), &mut collect).unwrap();
        } else {
            let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
            execute_reviewed_with(
                &f.from,
                &f.to,
                &f.global,
                &review,
                &Token::new(),
                &mut collect,
            )
            .unwrap();
        }

        assert!(f.to.environment_config("game").exists());
        assert_eq!(fs::read(f.to.library_file()).unwrap(), before);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("Can't read library.toml: "),
            "{warnings:?}"
        );
        assert!(
            warnings[0].ends_with(
                "The copy of game didn't keep its favorite, hidden flag, or added date."
            ),
            "{warnings:?}"
        );
    }
}

#[test]
fn a_canceled_copy_publishes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let from = paths(root.path(), None);
    let usb = root.path().join("usb");
    let to = paths(root.path(), Some(&usb));
    let prefix = root.path().join("prefix");
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    fs::create_dir_all(from.environments_dir()).unwrap();
    fs::write(
        from.environment_config("game"),
        format!(
            "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
            prefix.display()
        ),
    )
    .unwrap();
    let global = GlobalConfig::default();
    let reviewed = inspect(&from, &to, &global, "game").unwrap();
    let canceled = Token::new();
    canceled.cancel();
    let err =
        execute_reviewed(&from, &to, &global, &reviewed, &canceled, |_| {}, |_| {}).unwrap_err();
    assert!(err.is::<Canceled>(), "{err}");
    assert!(!usb.join("prefixes/game").exists());
    assert!(!to.environment_config("game").exists());
    assert!(!usb.join("config").exists());
    assert!(inspect(&from, &to, &global, "game").is_ok());
}
struct Fixture {
    _root: tempfile::TempDir,
    from: Paths,
    to: Paths,
    global: GlobalConfig,
    prefix: PathBuf,
}

impl Fixture {
    fn new(native: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let from = paths(root.path(), None);
        let to = paths(root.path(), Some(&root.path().join("usb")));
        let prefix = root.path().join("prefix");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        fs::write(prefix.join("drive_c/data"), "data").unwrap();
        fs::create_dir_all(from.environments_dir()).unwrap();
        let text = if native {
            "kind = 'native'\n[targets.app]\nexe = '/opt/app'\ndesktop = true\n".to_owned()
        } else {
            format!(
                "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
                prefix.display()
            )
        };
        fs::write(from.environment_config("game"), text).unwrap();
        fs::create_dir_all(from.templates_dir()).unwrap();
        fs::write(from.templates_dir().join("work.toml"), "gamemode = true\n").unwrap();
        fs::write(from.global_config(), "# global\n").unwrap();
        fs::create_dir_all(from.art_dir("game").join("app")).unwrap();
        fs::write(from.art_dir("game").join("hero.png"), "art").unwrap();
        std::os::unix::fs::symlink("../hero.png", from.art_dir("game").join("app/link")).unwrap();
        Self {
            _root: root,
            from,
            to,
            global: GlobalConfig::default(),
            prefix,
        }
    }

    fn assert_unpublished(&self) {
        assert!(!self.to.environment_config("game").exists());
        assert!(!self.to.default_prefix_root().join("game").exists());
        assert!(!self.to.art_dir("game").exists());
        assert!(!self.to.templates_dir().exists());
        assert!(!self.to.global_config().parent().unwrap().exists());
        for directory in [
            self.to.default_prefix_root(),
            self.to.portable_root().unwrap().to_owned(),
        ] {
            if directory.exists() {
                for entry in fs::read_dir(directory).unwrap() {
                    let name = entry.unwrap().file_name();
                    assert!(!name.to_string_lossy().starts_with(".wineo-copy-"));
                    assert!(!name.to_string_lossy().starts_with(".wineo-art-"));
                }
            }
        }
    }

    fn assert_no_staging(&self) {
        for directory in [
            self.to.default_prefix_root(),
            self.to.portable_root().unwrap().to_owned(),
        ] {
            for entry in fs::read_dir(directory).unwrap() {
                let name = entry.unwrap().file_name();
                assert!(!name.to_string_lossy().starts_with(".wineo-copy-"));
                assert!(!name.to_string_lossy().starts_with(".wineo-art-"));
            }
        }
        assert!(!self.to.environment_config("game").exists());
    }
}

#[test]
fn inspection_cancels_wine_and_native_without_destination_writes() {
    for native in [false, true] {
        let f = Fixture::new(native);
        let cancel = Token::new();
        let mut progress = |event| {
            assert!(matches!(event, ImportProgress::Checking { .. }));
            cancel.cancel();
        };
        let err = if native {
            inspect_native_with(&f.from, &f.to, "game", &cancel, &mut progress).unwrap_err()
        } else {
            inspect_with(&f.from, &f.to, &f.global, "game", &cancel, &mut progress).unwrap_err()
        };
        assert!(err.is::<Canceled>(), "{err}");
        f.assert_unpublished();
        assert!(!f.to.portable_root().unwrap().exists());
    }
}

#[test]
fn copy_cancellation_cleans_prefix_template_and_art_staging() {
    for native in [false, true] {
        for part in ["prefix", "templates", "art"] {
            if native && part == "prefix" {
                continue;
            }
            let f = Fixture::new(native);
            let cancel = Token::new();
            let source = match part {
                "prefix" => f.prefix.clone(),
                "templates" => f.from.templates_dir(),
                _ => f.from.art_dir("game"),
            };
            let mut canceled = false;
            let mut events = |event| {
                assert_ne!(event, CopyEvent::Publishing);
                if let CopyEvent::Progress(ImportProgress::Copied { path }) = event
                    && path.starts_with(&source)
                {
                    canceled = true;
                    cancel.cancel();
                }
            };
            let err = if native {
                let review = inspect_native(&f.from, &f.to, "game").unwrap();
                execute_native_with(&f.from, &f.to, &review, &cancel, &mut events).unwrap_err()
            } else {
                let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
                execute_reviewed_with(&f.from, &f.to, &f.global, &review, &cancel, &mut events)
                    .unwrap_err()
            };
            assert!(canceled);
            assert!(err.is::<Canceled>(), "{err}");
            f.assert_unpublished();
            if native {
                assert!(inspect_native(&f.from, &f.to, "game").unwrap().first_copy);
            } else {
                assert!(
                    inspect(&f.from, &f.to, &f.global, "game")
                        .unwrap()
                        .first_copy
                );
            }
        }
    }
}

#[test]
fn execution_revalidation_is_cancellable_before_and_after_copy() {
    for native in [false, true] {
        for after_copy in [false, true] {
            let f = Fixture::new(native);
            let cancel = Token::new();
            let mut copying = false;
            let mut canceled = false;
            let mut events = |event| match event {
                CopyEvent::Progress(ImportProgress::Started { .. }) => copying = true,
                CopyEvent::Progress(ImportProgress::Checking { .. }) if copying == after_copy => {
                    canceled = true;
                    cancel.cancel();
                }
                CopyEvent::Publishing => panic!("published canceled review"),
                _ => {}
            };
            let err = if native {
                let review = inspect_native(&f.from, &f.to, "game").unwrap();
                execute_native_with(&f.from, &f.to, &review, &cancel, &mut events).unwrap_err()
            } else {
                let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
                execute_reviewed_with(&f.from, &f.to, &f.global, &review, &cancel, &mut events)
                    .unwrap_err()
            };
            assert!(canceled);
            assert!(err.is::<Canceled>(), "{err}");
            f.assert_unpublished();
        }
    }
}

#[test]
fn destination_appearing_during_review_is_not_replaced() {
    let f = Fixture::new(false);
    let mut checks = 0;
    let review = inspect_with(&f.from, &f.to, &f.global, "game", &Token::new(), |event| {
        if let ImportProgress::Checking { .. } = event {
            checks += 1;
        }
    })
    .unwrap();
    let destination = review.destination.clone();
    // The last check of the locked review runs after its destination
    // test, so creating it then is only caught by the copy itself.
    let mut seen = 0;
    let err = execute_reviewed(
        &f.from,
        &f.to,
        &f.global,
        &review,
        &Token::new(),
        |event| {
            if let ImportProgress::Checking { .. } = event {
                seen += 1;
                if seen == checks {
                    fs::create_dir_all(&destination).unwrap();
                    fs::write(destination.join("theirs"), "keep").unwrap();
                }
            }
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "portable copy destination appeared during review: {}",
            destination.display()
        )
    );
    assert_eq!(
        fs::read_to_string(destination.join("theirs")).unwrap(),
        "keep"
    );
    f.assert_no_staging();
}

#[test]
fn destination_appearing_during_publication_is_not_replaced() {
    let f = Fixture::new(false);
    let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
    let destination = review.destination.clone();
    let err = execute_reviewed_with(&f.from, &f.to, &f.global, &review, &Token::new(), |event| {
        if event == CopyEvent::Publishing {
            fs::create_dir(&destination).unwrap();
            fs::write(destination.join("theirs"), "keep").unwrap();
        }
    })
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "portable copy destination appeared during publication: {}",
            destination.display()
        )
    );
    assert_eq!(
        fs::read_to_string(destination.join("theirs")).unwrap(),
        "keep"
    );
    assert!(!f.to.art_dir("game").exists());
    assert!(!f.to.templates_dir().exists());
    f.assert_no_staging();
}

#[test]
fn late_cancel_preserves_success_and_native_copy_reports_real_progress() {
    for native in [false, true] {
        let f = Fixture::new(native);
        let cancel = Token::new();
        let mut events = Vec::new();
        let mut callback = |event| {
            if event == CopyEvent::Publishing {
                assert!(!f.to.environment_config("game").exists());
                assert!(!f.to.default_prefix_root().join("game").exists());
                assert!(!f.to.art_dir("game").exists());
                assert!(!f.to.templates_dir().exists());
                cancel.cancel();
            }
            events.push(event);
        };
        let result = if native {
            let review = inspect_native(&f.from, &f.to, "game").unwrap();
            execute_native_with(&f.from, &f.to, &review, &cancel, &mut callback)
        } else {
            let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
            execute_reviewed_with(&f.from, &f.to, &f.global, &review, &cancel, &mut callback)
        };
        assert_eq!(result.unwrap().name, "game");
        assert!(cancel.is_canceled());
        assert_eq!(
            events
                .iter()
                .filter(|event| **event == CopyEvent::Publishing)
                .count(),
            1
        );
        let boundary = events
            .iter()
            .position(|event| *event == CopyEvent::Publishing)
            .unwrap();
        assert!(matches!(
            events.last(),
            Some(CopyEvent::Progress(ImportProgress::Finished { .. }))
        ));
        for source in [f.from.templates_dir(), f.from.art_dir("game")] {
            assert!(events[..boundary].iter().any(|event| matches!(event,
                    CopyEvent::Progress(ImportProgress::Copied { path }) if path.starts_with(&source))));
        }
        assert_eq!(
            fs::read_link(f.to.art_dir("game").join("app/link")).unwrap(),
            Path::new("../hero.png")
        );
        let destination = fs::read_to_string(f.to.environment_config("game")).unwrap();
        if native {
            assert!(destination.contains("desktop = false"));
            let totals: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    CopyEvent::Progress(ImportProgress::Started { entries }) => Some(*entries),
                    _ => None,
                })
                .collect();
            assert_eq!(totals, [2, 4]);
            assert_eq!(
                events.last(),
                Some(&CopyEvent::Progress(ImportProgress::Finished {
                    entries: 6
                }))
            );
        }
        assert_eq!(
            fs::read_to_string(f.to.templates_dir().join("work.toml")).unwrap(),
            "gamemode = true\n"
        );
    }
}

#[test]
fn publish_failures_still_report_published_paths_after_late_cancel() {
    for native in [false, true] {
        let f = Fixture::new(native);
        let cancel = Token::new();
        let mut events = |event| {
            if event == CopyEvent::Publishing {
                // Racing art prevents publication after templates (and the Wine prefix).
                fs::create_dir_all(f.to.art_dir("game")).unwrap();
                cancel.cancel();
            }
        };
        let err = if native {
            let review = inspect_native(&f.from, &f.to, "game").unwrap();
            execute_native_with(&f.from, &f.to, &review, &cancel, &mut events).unwrap_err()
        } else {
            let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
            execute_reviewed_with(&f.from, &f.to, &f.global, &review, &cancel, &mut events)
                .unwrap_err()
        };
        assert!(!err.is::<Canceled>());
        let message = err.to_string();
        assert!(message.contains("copied files remain at"), "{message}");
        assert!(
            message.contains(&f.to.templates_dir().display().to_string()),
            "{message}"
        );
        if !native {
            assert!(
                message.contains(
                    &f.to
                        .default_prefix_root()
                        .join("game")
                        .display()
                        .to_string()
                ),
                "{message}"
            );
        }
        assert!(message.contains("move them aside and retry"), "{message}");
        assert!(!f.to.environment_config("game").exists());
    }
}

#[test]
fn a_prefix_change_after_review_is_refused_under_the_lock() {
    let f = Fixture::new(false);
    let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
    // Same size and mtime, so only the content fingerprint can see it.
    let data = f.prefix.join("drive_c/data");
    let modified = fs::metadata(&data).unwrap().modified().unwrap();
    fs::write(&data, "edit").unwrap();
    fs::File::options()
        .write(true)
        .open(&data)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let err = execute_reviewed(
        &f.from,
        &f.to,
        &f.global,
        &review,
        &Token::new(),
        |_| {},
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "the reviewed prefix changed; inspect it again before importing"
    );
    f.assert_unpublished();
}

#[test]
fn an_interrupted_restore_after_review_is_reported_as_one() {
    let f = Fixture::new(false);
    let review = inspect(&f.from, &f.to, &f.global, "game").unwrap();
    let marker = f.from.restore_marker("game");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, "{}").unwrap();
    let err = execute_reviewed(
        &f.from,
        &f.to,
        &f.global,
        &review,
        &Token::new(),
        |_| {},
        |_| {},
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("interrupted snapshot restore; run `wineo snapshot recover game`"),
        "{err}"
    );
    f.assert_unpublished();
}
