use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::gog::test_support::setup;

const NEVER: &dyn Fn() -> bool = &|| false;

fn full_state(folder: &Path) -> FolderState {
    FolderState {
        version: VERSION,
        owner: Some("rimworld".to_owned()),
        folder: folder.to_owned(),
        device: Some(42),
        committed: Some(Committed {
            gog_id: 1094900565,
            platform: Platform::Linux,
            language: "en-US".to_owned(),
            track: Track::Installer,
            products: vec![
                Product {
                    id: 1094900565,
                    version: Some("1.5.4104".to_owned()),
                    build: None,
                    language: None,
                    status: ProductStatus::Installed,
                },
                Product {
                    id: 1149556166,
                    version: None,
                    build: None,
                    language: None,
                    status: ProductStatus::Unavailable,
                },
            ],
            files: vec![
                WinningEntry {
                    path: "RimWorldLinux".to_owned(),
                    kind: EntryType::File,
                    size: Some(12),
                    md5: Some("0123456789abcdef0123456789abcdef".to_owned()),
                    target: None,
                    mode: 0o755,
                    product: 1094900565,
                },
                WinningEntry {
                    path: "Data/link".to_owned(),
                    kind: EntryType::Symlink,
                    size: None,
                    md5: None,
                    target: Some("../RimWorldLinux".to_owned()),
                    mode: 0o777,
                    product: 1149556166,
                },
                WinningEntry {
                    path: "Data".to_owned(),
                    kind: EntryType::Directory,
                    size: None,
                    md5: None,
                    target: None,
                    mode: 0o755,
                    product: 1094900565,
                },
            ],
        }),
        journal: Some(journal(1094900565)),
    }
}

fn journal(gog_id: u64) -> Journal {
    Journal {
        operation: "20261011T120000Z-ab12".to_owned(),
        kind: JournalKind::Update,
        target: Target {
            gog_id,
            platform: Platform::Linux,
            language: "en-US".to_owned(),
            track: Track::Installer,
            products: vec![Product {
                id: gog_id,
                version: Some("1.6".to_owned()),
                build: None,
                language: None,
                status: ProductStatus::Installed,
            }],
        },
        packages: vec![Package {
            product: gog_id,
            download_id: "en3installer0".to_owned(),
            file_id: "en3installer0".to_owned(),
            size: 123,
            md5: None,
        }],
        attempted: vec![Attempted {
            path: "RimWorldLinux".to_owned(),
            kind: EntryType::File,
            md5: Some("fedcba9876543210fedcba9876543210".to_owned()),
            target: None,
        }],
    }
}

fn game(folder: &Path, owner: Option<&str>, gog_id: u64) -> FolderState {
    FolderState {
        committed: Some(Committed {
            gog_id,
            platform: Platform::Linux,
            language: "en-US".to_owned(),
            track: Track::Installer,
            products: Vec::new(),
            files: Vec::new(),
        }),
        ..FolderState::new(folder.to_owned(), owner.map(str::to_owned))
    }
}

fn save(paths: &Paths, state: &FolderState) {
    FolderLock::exclusive(paths, &state.folder, NEVER)
        .unwrap()
        .write(state)
        .unwrap();
}

fn load(paths: &Paths, folder: &Path) -> Option<FolderState> {
    FolderLock::shared(paths, folder, NEVER)
        .unwrap()
        .read()
        .unwrap()
}

#[test]
fn a_state_from_before_product_languages_loads_unchanged() {
    let (_root, paths) = setup();
    let folder = paths.gog_folders_dir().parent().unwrap().join("game");
    let text = format!(
        "version = 1\nowner = \"rimworld\"\nfolder = \"{}\"\n\n[committed]\ngog_id = 1\nplatform = \"linux\"\nlanguage = \"en\"\ntrack = \"installer\"\n\n[[committed.products]]\nid = 1\nversion = \"1.0\"\nstatus = \"installed\"\n",
        folder.display()
    );
    let state = FolderState::parse(&text).unwrap();
    let product = &state.committed.as_ref().unwrap().products[0];
    assert_eq!(product.language, None);
    // Written again, only the folder's language is there.
    assert_eq!(state.to_toml().unwrap().matches("language =").count(), 1);

    // Reading it through its lock leaves the file as it was.
    let path = state_path(&paths, &folder);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &text).unwrap();
    assert_eq!(load(&paths, &folder), Some(state));
    assert_eq!(fs::read_to_string(&path).unwrap(), text);

    // A product's own language round-trips.
    let mut with = FolderState::parse(&text).unwrap();
    with.committed.as_mut().unwrap().products[0].language = Some("de".to_owned());
    let written = with.to_toml().unwrap();
    assert!(written.contains("language = \"de\""), "{written}");
    assert_eq!(FolderState::parse(&written).unwrap(), with);
}

#[test]
fn state_round_trips_through_toml() {
    let state = full_state(Path::new("/games/rimworld"));
    let text = state.to_toml().unwrap();
    assert_eq!(FolderState::parse(&text).unwrap(), state);
    assert!(
        text.starts_with("version = 1\nowner = \"rimworld\"\n"),
        "{text}"
    );
    assert!(text.contains("[committed]"), "{text}");
    assert!(text.contains("[journal]"), "{text}");
    assert!(text.contains("type = \"symlink\""), "{text}");

    let bare = FolderState::new(PathBuf::from("/games/new"), None);
    let text = bare.to_toml().unwrap();
    assert_eq!(text, "version = 1\nfolder = \"/games/new\"\n");
    assert_eq!(FolderState::parse(&text).unwrap(), bare);
}

#[test]
fn bad_states_are_refused() {
    let base = "version = 1\nfolder = \"/games/x\"\n";
    for text in [
        "version = 2\nfolder = \"/games/x\"\n",
        "version = 1\nfolder = \"games/x\"\n",
        "version = 1\nfolder = \"/games/x\"\nowner = \"bad name\"\n",
        "version = 1\nfolder = \"/games/x\"\nsurprise = 1\n",
    ] {
        assert!(FolderState::parse(text).is_err(), "{text}");
    }
    let committed =
        "[committed]\ngog_id = 1\nplatform = \"linux\"\nlanguage = \"en\"\ntrack = \"installer\"\n";
    for entry in [
        "path = \"a\"\ntype = \"file\"\nmode = 1\nproduct = 1\n",
        "path = \"a\"\ntype = \"symlink\"\nsize = 1\nmd5 = \"x\"\nmode = 1\nproduct = 1\n",
        "path = \"a\"\ntype = \"directory\"\ntarget = \"b\"\nmode = 1\nproduct = 1\n",
        "path = \"../a\"\ntype = \"directory\"\nmode = 1\nproduct = 1\n",
        "path = \"/a\"\ntype = \"directory\"\nmode = 1\nproduct = 1\n",
    ] {
        let text = format!("{base}{committed}[[committed.files]]\n{entry}");
        assert!(FolderState::parse(&text).is_err(), "{text}");
    }
    let ok = format!(
        "{base}{committed}[[committed.files]]\npath = \"a/b\"\ntype = \"directory\"\nmode = 493\nproduct = 1\n"
    );
    FolderState::parse(&ok).unwrap();
}

#[test]
fn gog_id_comes_from_the_commit_or_a_first_install_journal() {
    let mut state = full_state(Path::new("/g"));
    assert_eq!(state.gog_id(), Some(1094900565));
    assert!(state.is_game("1094900565"));
    assert!(!state.is_game("1"));
    state.committed = None;
    state.journal = Some(journal(7));
    assert_eq!(state.gog_id(), Some(7));
    state.journal = None;
    assert_eq!(state.gog_id(), None);
}

#[test]
fn writes_are_durable_and_keyed_by_the_canonical_path() {
    let (root, paths) = setup();
    let folder = root.path().join("games/rimworld");
    fs::create_dir_all(&folder).unwrap();
    let folder = canonical(&folder).unwrap();
    let state = full_state(&folder);
    save(&paths, &state);
    let file = state_path(&paths, &folder);
    assert!(file.starts_with(paths.gog_folders_dir()));
    assert_eq!(
        file.file_name().unwrap().to_str().unwrap(),
        format!("{}.toml", key(&folder))
    );
    assert_eq!(key(&folder).len(), 64);
    assert_eq!(load(&paths, &folder), Some(state.clone()));

    // A crash before the rename leaves the old state, and the temporary file
    // is never listed as a state.
    let mut changed = state.clone();
    changed.owner = None;
    set_failpoint(Some("write.before_rename"));
    let lock = FolderLock::exclusive(&paths, &folder, NEVER).unwrap();
    assert!(lock.write(&changed).is_err());
    drop(lock);
    assert_eq!(load(&paths, &folder), Some(state));
    let listed = list(&paths).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].1.is_ok());
}

#[test]
fn shared_locks_cannot_write() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    let lock = FolderLock::shared(&paths, &folder, NEVER).unwrap();
    assert!(lock.write(&FolderState::new(folder.clone(), None)).is_err());
    // Shared holders don't block each other.
    let _again = FolderLock::shared(&paths, &folder, NEVER).unwrap();
}

#[test]
fn lock_waits_can_be_canceled() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    let _held = FolderLock::exclusive(&paths, &folder, NEVER).unwrap();
    let canceled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&canceled);
    let (paths2, folder2) = (paths.clone(), folder.clone());
    let waiter = thread::spawn(move || {
        FolderLock::shared(&paths2, &folder2, &|| flag.load(Ordering::Relaxed)).map(|_| ())
    });
    thread::sleep(Duration::from_millis(120));
    canceled.store(true, Ordering::Relaxed);
    let error = waiter.join().unwrap().unwrap_err();
    assert!(matches!(
        error.downcast_ref::<FolderError>(),
        Some(FolderError::Canceled)
    ));
}

#[test]
fn locks_and_states_stay_on_the_host_with_a_portable_root() {
    let (root, paths) = setup();
    let home = root.path().to_owned();
    let portable = Paths::select(
        move |key| (key == "HOME").then(|| home.clone().into_os_string()),
        Some(&root.path().join("usb")),
    )
    .unwrap();
    let folder = root.path().join("g");
    assert_eq!(state_path(&paths, &folder), state_path(&portable, &folder));
    assert_eq!(
        paths.gog_folder_lock(&key(&folder)),
        portable.gog_folder_lock(&key(&folder))
    );
}

#[test]
fn commit_replaces_committed_and_drops_the_journal_in_one_write() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    fs::create_dir_all(&folder).unwrap();
    let mut state = full_state(&folder);
    state.device = None;
    save(&paths, &state);
    let next = Committed {
        products: Vec::new(),
        ..state.committed.clone().unwrap()
    };
    let lock = FolderLock::exclusive(&paths, &folder, NEVER).unwrap();
    // A failed commit leaves the journal in place.
    set_failpoint(Some("write.before"));
    assert!(lock.commit(next.clone()).is_err());
    assert!(lock.read().unwrap().unwrap().journal.is_some());
    let done = lock.commit(next.clone()).unwrap();
    drop(lock);
    assert_eq!(done.journal, None);
    assert_eq!(done.committed, Some(next));
    assert_eq!(done.device, Some(fs::metadata(&folder).unwrap().dev()));
    assert_eq!(done.owner.as_deref(), Some("rimworld"));
    assert_eq!(load(&paths, &folder), Some(done));
}

#[test]
fn find_matches_owner_and_gog_id() {
    let (root, paths) = setup();
    let a = root.path().join("a");
    let b = root.path().join("b");
    save(&paths, &game(&a, Some("rimworld"), 1));
    save(&paths, &game(&b, Some("other"), 1));
    assert_eq!(find(&paths, "rimworld", "1").unwrap().unwrap().folder, a);
    assert!(find(&paths, "rimworld", "2").unwrap().is_none());
    assert!(find(&paths, "nobody", "1").unwrap().is_none());
    assert_eq!(owned_by(&paths, "other", true).unwrap().len(), 1);
}

#[test]
fn overlapping_folders_are_refused_by_name() {
    let (root, paths) = setup();
    let games = root.path().join("games");
    save(&paths, &game(&games.join("rimworld"), Some("rimworld"), 1));
    save(&paths, &game(&games.join("orphan"), None, 9));
    check_overlap(&paths, &games.join("rimworld")).unwrap();
    check_overlap(&paths, &games.join("rimworld2")).unwrap();
    let inside = check_overlap(&paths, &games.join("rimworld/Mods")).unwrap_err();
    assert_eq!(
        inside.to_string(),
        format!(
            "{} overlaps rimworld's game folder {}.",
            games.join("rimworld/Mods").display(),
            games.join("rimworld").display()
        )
    );
    let around = check_overlap(&paths, &games).unwrap_err();
    assert!(matches!(
        around.downcast_ref::<FolderError>(),
        Some(FolderError::Overlap { .. })
    ));
    let orphan = check_overlap(&paths, &games.join("orphan/x")).unwrap_err();
    assert!(
        orphan.to_string().contains("GOG game 9's game folder"),
        "{orphan}"
    );
}

#[test]
fn reclaim_keeps_the_journal_and_refuses_another_owner() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    let mut state = full_state(&folder);
    state.owner = None;
    save(&paths, &state);
    let claimed = reclaim(&paths, &folder, "rimworld", NEVER).unwrap();
    assert_eq!(claimed.owner.as_deref(), Some("rimworld"));
    assert_eq!(claimed.journal, state.journal);
    assert_eq!(claimed.committed, state.committed);
    assert_eq!(load(&paths, &folder), Some(claimed.clone()));
    // Again by the same owner changes nothing.
    assert_eq!(
        reclaim(&paths, &folder, "rimworld", NEVER).unwrap(),
        claimed
    );

    let error = reclaim(&paths, &folder, "copy", NEVER).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("{} belongs to rimworld.", folder.display())
    );
    assert_eq!(load(&paths, &folder), Some(claimed));
    assert!(reclaim(&paths, &root.path().join("none"), "x", NEVER).is_err());
}

#[test]
fn release_keeps_everything_but_the_owner() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    let state = full_state(&folder);
    save(&paths, &state);
    release(&paths, &folder, NEVER).unwrap();
    let released = load(&paths, &folder).unwrap();
    assert_eq!(released.owner, None);
    assert_eq!(released.journal, state.journal);
    assert_eq!(released.committed, state.committed);
}

#[test]
fn forget_is_refused_while_owned() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    save(&paths, &game(&folder, Some("rimworld"), 1));
    let error = forget(&paths, &folder, NEVER).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<FolderError>(),
        Some(FolderError::OwnedByOther { .. })
    ));
    assert!(state_path(&paths, &folder).exists());
    release(&paths, &folder, NEVER).unwrap();
    assert!(forget(&paths, &folder, NEVER).unwrap());
    assert!(!state_path(&paths, &folder).exists());
    assert!(!forget(&paths, &folder, NEVER).unwrap());
    // The lock file stays, so later lockers share it.
    assert!(paths.gog_folder_lock(&key(&folder)).exists());
}

/// A fake filesystem: each listed path exists on its device.
fn devices(map: &[(&Path, u64)]) -> impl Fn(&Path) -> io::Result<Option<u64>> + use<> {
    let map: HashMap<PathBuf, u64> = map
        .iter()
        .map(|(path, device)| (path.to_path_buf(), *device))
        .collect();
    move |path| Ok(map.get(path).copied())
}

#[test]
fn missing_folders_are_only_dropped_when_the_drive_is_mounted() {
    let (_root, paths) = setup();
    let folder = Path::new("/mnt/games/rimworld");
    let mut state = game(folder, None, 1);
    state.device = Some(7);
    save(&paths, &state);

    // Unmounted: the mount point is on the root filesystem.
    let unmounted = devices(&[
        (Path::new("/"), 1),
        (Path::new("/mnt"), 1),
        (Path::new("/mnt/games"), 1),
    ]);
    assert!(!surely_deleted(&state, &unmounted));
    assert!(!remove_if_deleted(&paths, folder, &unmounted, NEVER).unwrap());
    assert!(state_path(&paths, folder).exists());

    // Remounted with the folder still there.
    let remounted = devices(&[
        (Path::new("/"), 1),
        (Path::new("/mnt/games"), 7),
        (folder, 7),
    ]);
    assert!(!surely_deleted(&state, &remounted));
    assert!(!remove_if_deleted(&paths, folder, &remounted, NEVER).unwrap());

    // Mounted, and the folder really is gone.
    let deleted = devices(&[(Path::new("/"), 1), (Path::new("/mnt/games"), 7)]);
    assert!(surely_deleted(&state, &deleted));

    // An owner, a journal, or an unknown device keeps it.
    for blocked in [
        FolderState {
            owner: Some("rimworld".to_owned()),
            ..state.clone()
        },
        FolderState {
            journal: Some(journal(1)),
            ..state.clone()
        },
        FolderState {
            device: None,
            ..state.clone()
        },
    ] {
        save(&paths, &blocked);
        assert!(!remove_if_deleted(&paths, folder, &deleted, NEVER).unwrap());
        assert!(state_path(&paths, folder).exists());
    }
    save(&paths, &state);
    assert!(remove_if_deleted(&paths, folder, &deleted, NEVER).unwrap());
    assert!(!state_path(&paths, folder).exists());
}

#[test]
fn release_missing_owner_keeps_the_journal() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    let state = full_state(&folder);
    save(&paths, &state);
    assert!(release_missing_owner(&paths, &folder, NEVER).unwrap());
    let released = load(&paths, &folder).unwrap();
    assert_eq!(released.owner, None);
    assert_eq!(released.journal, state.journal);

    let other = root.path().join("h");
    save(&paths, &game(&other, Some("here"), 2));
    let config = paths.environment_config("here");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "kind = 'native'\n").unwrap();
    assert!(!release_missing_owner(&paths, &other, NEVER).unwrap());
    assert_eq!(load(&paths, &other).unwrap().owner.as_deref(), Some("here"));
}

#[test]
fn lock_owned_holds_every_owned_folder() {
    let (root, paths) = setup();
    for (folder, owner) in [("a", "rimworld"), ("b", "rimworld"), ("c", "other")] {
        save(&paths, &game(&root.path().join(folder), Some(owner), 1));
    }
    let held = lock_owned(&paths, "rimworld", true, NEVER).unwrap();
    assert_eq!(held.len(), 2);
    for (lock, state) in &held {
        assert_eq!(lock.folder(), state.folder);
    }
    assert!(
        lock_owned(&paths, "nobody", true, NEVER)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_misfiled_state_is_reported() {
    let (root, paths) = setup();
    let folder = root.path().join("a");
    save(&paths, &game(&folder, None, 1));
    fs::rename(
        state_path(&paths, &folder),
        state_path(&paths, &root.path().join("b")),
    )
    .unwrap();
    let listed = list(&paths).unwrap();
    assert!(listed[0].1.is_err());
    assert!(
        FolderLock::shared(&paths, &root.path().join("b"), NEVER)
            .unwrap()
            .read()
            .is_err()
    );
}

#[test]
fn non_canonical_folders_are_refused() {
    let (_root, paths) = setup();
    for bad in [
        "games/x",
        "/games/./x",
        "/games/../x",
        "/games/x/",
        "/games//x",
    ] {
        let folder = Path::new(bad);
        assert!(check_folder(folder).is_err(), "{bad}");
        assert!(FolderLock::shared(&paths, folder, NEVER).is_err(), "{bad}");
        let text = format!("version = 1\nfolder = \"{bad}\"\n");
        assert!(FolderState::parse(&text).is_err(), "{bad}");
    }
    check_folder(Path::new("/games/x")).unwrap();
    check_folder(Path::new("/")).unwrap();
}

#[test]
fn a_write_cleans_up_only_its_own_folders_stale_temporaries() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    let other = root.path().join("h");
    save(&paths, &game(&folder, None, 1));
    let dir = paths.gog_folders_dir();
    let mine = dir.join(format!(".state-{}-crashed", key(&folder)));
    let theirs = dir.join(format!(".state-{}-crashed", key(&other)));
    fs::write(&mine, "half").unwrap();
    fs::write(&theirs, "half").unwrap();
    save(&paths, &game(&folder, Some("rimworld"), 1));
    assert!(!mine.exists());
    assert!(theirs.exists());
    // Forgetting cleans them up too.
    fs::write(&mine, "half").unwrap();
    release(&paths, &folder, NEVER).unwrap();
    assert!(forget(&paths, &folder, NEVER).unwrap());
    assert!(!mine.exists());
    assert!(theirs.exists());
}

#[test]
fn a_failure_after_the_write_leaves_the_new_state() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    save(&paths, &game(&folder, None, 1));
    let lock = FolderLock::exclusive(&paths, &folder, NEVER).unwrap();
    set_failpoint(Some("write.after"));
    assert!(lock.write(&game(&folder, Some("rimworld"), 1)).is_err());
    drop(lock);
    assert_eq!(
        load(&paths, &folder).unwrap().owner.as_deref(),
        Some("rimworld")
    );
}

#[test]
fn unreadable_states_are_skipped_only_when_not_strict() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    save(&paths, &game(&folder, Some("rimworld"), 1));
    let bad = paths.gog_folders_dir().join("bad.toml");
    fs::write(&bad, "version = 9\nfolder = '/x'\n").unwrap();
    assert_eq!(owned_by(&paths, "rimworld", false).unwrap().len(), 1);
    let error = owned_by(&paths, "rimworld", true).unwrap_err();
    let message = error.to_string();
    assert_eq!(
        message,
        format!(
            "rimworld's GOG state can't be checked: {} isn't a valid GOG folder state (unsupported GOG folder state version 9).",
            bad.display()
        )
    );
    assert_eq!(message.matches(bad.to_str().unwrap()).count(), 1);
    // A TOML error stays on one line.
    fs::write(&bad, "version = [").unwrap();
    let error = owned_by(&paths, "rimworld", true).unwrap_err().to_string();
    assert!(!error.contains('\n'), "{error}");
}

#[test]
fn may_track_game_fails_closed() {
    let (_root, paths) = setup();
    let config = paths.environment_config("x");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    assert!(!may_track_game(&paths, "x"));
    fs::write(&config, "kind = 'native'\n").unwrap();
    assert!(!may_track_game(&paths, "x"));
    fs::write(&config, "kind = 'native'\ngog_id = '1'\n").unwrap();
    assert!(may_track_game(&paths, "x"));
    fs::write(&config, "kind = [").unwrap();
    assert!(may_track_game(&paths, "x"));
}

#[test]
fn a_folder_that_cant_be_looked_at_is_never_surely_deleted() {
    let mut state = game(Path::new("/mnt/games/x"), None, 1);
    state.device = Some(7);
    let denied = |path: &Path| {
        if path == Path::new("/mnt/games") {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        } else if path == Path::new("/") {
            Ok(Some(7))
        } else {
            Ok(None)
        }
    };
    assert!(!surely_deleted(&state, &denied));
    let unreadable_folder = |_: &Path| Err(io::Error::from(io::ErrorKind::PermissionDenied));
    assert!(!surely_deleted(&state, &unreadable_folder));
}

#[test]
fn a_temporary_that_cant_be_removed_leaves_the_state_in_place() {
    let (root, paths) = setup();
    let folder = root.path().join("g");
    save(&paths, &game(&folder, None, 1));
    let stuck = paths
        .gog_folders_dir()
        .join(format!(".state-{}-stuck", key(&folder)));
    fs::create_dir(&stuck).unwrap();
    fs::write(stuck.join("inside"), "").unwrap();
    assert!(forget(&paths, &folder, NEVER).is_err());
    assert!(state_path(&paths, &folder).exists());
    fs::remove_dir_all(&stuck).unwrap();
    assert!(forget(&paths, &folder, NEVER).unwrap());
}

fn installing(folder: &Path, owner: Option<&str>, kind: JournalKind) -> FolderState {
    FolderState {
        journal: Some(Journal { kind, ..journal(1) }),
        ..FolderState::new(folder.to_owned(), owner.map(str::to_owned))
    }
}

#[test]
fn a_fresh_install_may_use_a_missing_or_empty_folder() {
    let (root, paths) = setup();
    let games = canonical(root.path()).unwrap();
    let missing = games.join("a/b/RimWorld");
    assert_eq!(
        check_fresh_install(&paths, &missing, "rimworld").unwrap(),
        missing
    );
    fs::create_dir(games.join("empty")).unwrap();
    assert_eq!(
        check_fresh_install(&paths, &games.join("empty"), "rimworld").unwrap(),
        games.join("empty")
    );
}

#[test]
fn a_fresh_install_resolves_symlinks_above_a_missing_folder() {
    let (root, paths) = setup();
    let real = canonical(root.path()).unwrap().join("real");
    fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, root.path().join("link")).unwrap();
    let found =
        check_fresh_install(&paths, &root.path().join("link/RimWorld"), "rimworld").unwrap();
    assert_eq!(found, real.join("RimWorld"));
    save(&paths, &game(&real.join("RimWorld"), Some("other"), 9));
    let error = check_fresh_install(&paths, &root.path().join("link/RimWorld/Mods"), "rimworld")
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<FolderError>(),
        Some(FolderError::Overlap { .. })
    ));
}

#[test]
fn a_fresh_install_refuses_a_folder_with_files() {
    let (root, paths) = setup();
    let folder = canonical(root.path()).unwrap().join("RimWorld");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("saved.txt"), "mine").unwrap();
    let error = check_fresh_install(&paths, &folder, "rimworld").unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("{} already exists and isn't empty.", folder.display())
    );
    let file = folder.join("saved.txt");
    assert!(check_fresh_install(&paths, &file, "rimworld").is_err());
}

#[test]
fn a_fresh_install_may_finish_its_own_install() {
    let (root, paths) = setup();
    let folder = canonical(root.path()).unwrap().join("RimWorld");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join(".wineo-partial"), "x").unwrap();
    save(
        &paths,
        &installing(&folder, Some("rimworld"), JournalKind::Install),
    );
    assert_eq!(
        check_fresh_install(&paths, &folder, "rimworld").unwrap(),
        folder
    );
}

#[test]
fn a_fresh_install_refuses_other_environments_and_other_journals() {
    let (root, paths) = setup();
    let folder = canonical(root.path()).unwrap().join("RimWorld");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("file"), "x").unwrap();
    for state in [
        installing(&folder, Some("other"), JournalKind::Install),
        installing(&folder, None, JournalKind::Install),
        installing(&folder, Some("rimworld"), JournalKind::Update),
        installing(&folder, Some("rimworld"), JournalKind::Reinstall),
        installing(&folder, Some("rimworld"), JournalKind::Repair),
        game(&folder, Some("rimworld"), 1),
    ] {
        save(&paths, &state);
        let error = check_fresh_install(&paths, &folder, "rimworld").unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<FolderError>(),
                Some(FolderError::NotEmpty { .. })
            ),
            "{state:?}: {error}"
        );
    }
}

#[test]
fn a_fresh_install_refuses_to_overlap_another_games_folder() {
    let (root, paths) = setup();
    let games = canonical(root.path()).unwrap().join("games");
    fs::create_dir_all(games.join("rimworld")).unwrap();
    save(&paths, &game(&games.join("rimworld"), Some("other"), 1));
    for folder in [games.join("rimworld/Mods"), games.clone()] {
        let error = check_fresh_install(&paths, &folder, "rimworld").unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<FolderError>(),
                Some(FolderError::Overlap { .. })
            ),
            "{error}"
        );
    }
    check_fresh_install(&paths, &games.join("rimworld2"), "rimworld").unwrap();
}
