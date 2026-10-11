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
                    status: ProductStatus::Installed,
                },
                Product {
                    id: 1149556166,
                    version: None,
                    build: None,
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
    assert_eq!(owned_by(&paths, "other").unwrap().len(), 1);
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
    assert!(!surely_deleted(&state, &unmounted).unwrap());
    assert!(!remove_if_deleted(&paths, folder, &unmounted, NEVER).unwrap());
    assert!(state_path(&paths, folder).exists());

    // Remounted with the folder still there.
    let remounted = devices(&[
        (Path::new("/"), 1),
        (Path::new("/mnt/games"), 7),
        (folder, 7),
    ]);
    assert!(!surely_deleted(&state, &remounted).unwrap());
    assert!(!remove_if_deleted(&paths, folder, &remounted, NEVER).unwrap());

    // Mounted, and the folder really is gone.
    let deleted = devices(&[(Path::new("/"), 1), (Path::new("/mnt/games"), 7)]);
    assert!(surely_deleted(&state, &deleted).unwrap());

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
    let held = lock_owned(&paths, "rimworld", NEVER).unwrap();
    assert_eq!(held.len(), 2);
    for (lock, state) in &held {
        assert_eq!(lock.folder(), state.folder);
    }
    assert!(lock_owned(&paths, "nobody", NEVER).unwrap().is_empty());
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
