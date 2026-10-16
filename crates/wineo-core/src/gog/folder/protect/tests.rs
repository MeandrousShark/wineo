use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, Barrier};
use std::thread;

use super::super::{
    Committed, Journal, JournalKind, Platform, Target, Track, lock_for_update, state_path,
};
use super::*;
use crate::gog::test_support::setup;

const ID: u64 = 1094900565;
const NEVER: &dyn Fn() -> bool = &|| false;

/// A game folder with its state, under a canonical root.
fn game(paths: &Paths, root: &Path, owner: Option<&str>, mid_update: bool) -> PathBuf {
    let folder = fs::canonicalize(root).unwrap().join("Games/RimWorld");
    fs::create_dir_all(&folder).unwrap();
    let target = Target {
        gog_id: ID,
        platform: Platform::Linux,
        language: "en-US".to_owned(),
        track: Track::Installer,
        products: Vec::new(),
    };
    let state = FolderState {
        committed: Some(Committed {
            source: None,
            gog_id: ID,
            platform: target.platform,
            language: target.language.clone(),
            track: target.track,
            products: Vec::new(),
            files: Vec::new(),
        }),
        journal: mid_update.then(|| Journal {
            operation: "op".to_owned(),
            kind: JournalKind::Update,
            target,
            packages: Vec::new(),
            attempted: Vec::new(),
        }),
        ..FolderState::new(folder.clone(), owner.map(str::to_owned))
    };
    FolderLock::exclusive(paths, &folder, NEVER)
        .unwrap()
        .write(&state)
        .unwrap();
    folder
}

fn owner(paths: &Paths, folder: &Path) -> Option<String> {
    FolderLock::shared(paths, folder, NEVER)
        .unwrap()
        .read()
        .unwrap()
        .unwrap()
        .owner
}

fn refusal(result: Result<Vec<InUseGuard>>) -> FolderError {
    result
        .unwrap_err()
        .downcast::<FolderError>()
        .expect("a typed refusal")
}

#[test]
fn missing_paths_resolve_through_their_nearest_existing_ancestor() {
    let (root, _) = setup();
    let real = fs::canonicalize(root.path()).unwrap();
    fs::create_dir_all(real.join("a")).unwrap();
    std::os::unix::fs::symlink(real.join("a"), real.join("link")).unwrap();
    assert_eq!(resolve(&real.join("link/new/x")), real.join("a/new/x"));
    assert_eq!(resolve(&real.join("link/new/../y")), real.join("a/y"));
    assert_eq!(resolve(&real.join("a/../link")), real.join("a"));
}

#[test]
fn an_owner_mid_update_is_refused() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), Some("rimworld"), true);
    let error = refusal(protect(&paths, "rimworld", Some("1094900565"), &[], NEVER));
    assert_eq!(
        error.to_string(),
        "rimworld's game folder is mid-update. Finish it with wineo update rimworld."
    );
    // The refusal let go of the in-use lock.
    assert!(lock_for_update(&paths, &folder, "rimworld").is_ok());
}

#[test]
fn a_galaxy_install_names_the_install_command() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), Some("rimworld"), true);
    // With a config, the environment exists and so isn't an unfinished
    // install that never got one.
    let config = paths.environment_config("rimworld");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "kind = 'native'\n").unwrap();
    let mut state = FolderLock::shared(&paths, &folder, NEVER)
        .unwrap()
        .read()
        .unwrap()
        .unwrap();
    let journal = state.journal.as_mut().unwrap();
    journal.kind = JournalKind::Install;
    journal.target.track = Track::Galaxy;
    FolderLock::exclusive(&paths, &folder, NEVER)
        .unwrap()
        .write(&state)
        .unwrap();
    let message =
        "rimworld's GOG install didn't finish. Finish it with wineo import gog --game 1094900565.";
    let error = refusal(protect(&paths, "rimworld", Some("1094900565"), &[], NEVER));
    assert_eq!(error.to_string(), message);
    // A clone running from the folder is told which game to finish.
    let error = refusal(protect(&paths, "copy", None, &[folder], NEVER));
    assert_eq!(error.to_string(), message);
}

#[test]
fn an_owner_holds_the_in_use_lock_whatever_it_launches() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), Some("rimworld"), false);
    let held = protect(&paths, "rimworld", Some("1094900565"), &[], NEVER).unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].lock().folder(), folder);
    assert!(matches!(
        lock_for_update(&paths, &folder, "rimworld")
            .unwrap_err()
            .downcast_ref::<FolderError>(),
        Some(FolderError::InUse { .. })
    ));
    drop(held);
    assert!(lock_for_update(&paths, &folder, "rimworld").is_ok());
}

#[test]
fn another_environment_is_checked_by_the_paths_it_names() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), Some("rimworld"), true);
    let outside = fs::canonicalize(root.path()).unwrap();
    assert!(
        protect(&paths, "copy", None, &[outside.join("other")], NEVER)
            .unwrap()
            .is_empty()
    );
    for path in [folder.clone(), folder.join("Data/new.exe")] {
        let error = refusal(protect(&paths, "copy", None, &[path], NEVER));
        assert!(matches!(&error, FolderError::MidUpdate { name } if name == "rimworld"));
    }
}

#[test]
fn an_update_in_progress_refuses_a_launch() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), Some("rimworld"), false);
    let _updating = lock_for_update(&paths, &folder, "rimworld").unwrap();
    let error = refusal(protect(&paths, "rimworld", Some("1094900565"), &[], NEVER));
    assert_eq!(
        error.to_string(),
        "rimworld's game folder is being updated."
    );
}

#[test]
fn a_lost_state_is_refused() {
    let (_root, paths) = setup();
    let error = refusal(protect(&paths, "rimworld", Some("1094900565"), &[], NEVER));
    assert_eq!(
        error.to_string(),
        "rimworld's GOG state is missing. Migrate it again with wineo import gog --game 1094900565, or stop tracking it with wineo set rimworld --unset gog_id."
    );
}

#[test]
fn an_ownerless_folder_mid_update_says_to_migrate_again() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), None, false);
    assert_eq!(
        protect(&paths, "copy", None, &[folder.join("x")], NEVER)
            .unwrap()
            .len(),
        1
    );
    game(&paths, root.path(), None, true);
    let error = refusal(protect(&paths, "copy", None, &[folder.join("x")], NEVER));
    assert_eq!(
        error.to_string(),
        "GOG game 1094900565's folder is mid-update. Migrate it again with wineo import gog --game 1094900565 and finish it."
    );
    assert_eq!(owner(&paths, &folder), None);
}

#[test]
fn an_ownerless_folder_of_its_game_is_claimed_once() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), None, false);
    let exe = folder.join("RimWorldLinux");
    protect(
        &paths,
        "rimworld",
        Some("1094900565"),
        std::slice::from_ref(&exe),
        NEVER,
    )
    .unwrap();
    assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
    let written = fs::metadata(state_path(&paths, &folder)).unwrap().ino();
    protect(
        &paths,
        "rimworld",
        Some("1094900565"),
        std::slice::from_ref(&exe),
        NEVER,
    )
    .unwrap();
    assert_eq!(
        fs::metadata(state_path(&paths, &folder)).unwrap().ino(),
        written,
        "an owned folder is never written again"
    );
    let error = refusal(protect(&paths, "other", Some("1094900565"), &[exe], NEVER));
    assert!(matches!(&error, FolderError::OwnedByOther { owner, .. } if owner == "rimworld"));
    assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
}

#[test]
fn a_claimed_folder_mid_update_names_its_new_owner() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), None, true);
    let error = refusal(protect(
        &paths,
        "rimworld",
        Some("1094900565"),
        std::slice::from_ref(&folder),
        NEVER,
    ));
    assert!(matches!(&error, FolderError::MidUpdate { name } if name == "rimworld"));
    assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
}

#[test]
fn racing_claims_agree() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), None, false);
    let barrier = Arc::new(Barrier::new(2));
    let racers: Vec<_> = (0..2)
        .map(|_| {
            let (paths, folder, barrier) = (paths.clone(), folder.clone(), barrier.clone());
            thread::spawn(move || {
                barrier.wait();
                protect(&paths, "rimworld", Some("1094900565"), &[folder], NEVER)
                    .map(|held| held.len())
            })
        })
        .collect();
    for racer in racers {
        assert_eq!(racer.join().unwrap().unwrap(), 1);
    }
    assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
}

#[test]
fn only_an_ancestors_broken_state_refuses() {
    let (root, paths) = setup();
    let real = fs::canonicalize(root.path()).unwrap();
    let broken = real.join("Broken");
    let elsewhere = real.join("Elsewhere");
    fs::create_dir_all(&broken).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    fs::create_dir_all(paths.gog_folders_dir()).unwrap();
    fs::write(state_path(&paths, &broken), "not = [valid").unwrap();
    assert!(
        protect(&paths, "copy", None, &[elsewhere.join("x.exe")], NEVER)
            .unwrap()
            .is_empty()
    );
    let error = refusal(protect(
        &paths,
        "copy",
        None,
        &[broken.join("x.exe")],
        NEVER,
    ));
    assert!(matches!(&error, FolderError::UnreadableFolder { folder, .. } if *folder == broken));
    assert!(
        error.to_string().starts_with(&format!(
            "The GOG state for {} can't be checked: ",
            broken.display()
        )),
        "{error}"
    );
}

#[test]
fn precheck_refuses_what_protect_would_without_claiming_or_holding() {
    let (root, paths) = setup();
    let lost = refusal(precheck(&paths, "rimworld", Some("1094900565"), &[]).map(|()| Vec::new()));
    assert!(matches!(lost, FolderError::StateMissing { .. }));

    let folder = game(&paths, root.path(), None, false);
    let written = fs::metadata(state_path(&paths, &folder)).unwrap().ino();
    let touched = [folder.join("RimWorldLinux")];
    precheck(&paths, "rimworld", Some("1094900565"), &touched).unwrap();
    assert_eq!(owner(&paths, &folder), None);
    assert_eq!(
        fs::metadata(state_path(&paths, &folder)).unwrap().ino(),
        written
    );
    let updating = lock_for_update(&paths, &folder, "rimworld").unwrap();
    let error =
        refusal(precheck(&paths, "rimworld", Some("1094900565"), &touched).map(|()| Vec::new()));
    assert!(matches!(error, FolderError::Updating { .. }));
    drop(updating);

    game(&paths, root.path(), None, true);
    let error =
        refusal(precheck(&paths, "rimworld", Some("1094900565"), &touched).map(|()| Vec::new()));
    assert!(matches!(&error, FolderError::MidUpdate { name } if name == "rimworld"));
    let error = refusal(precheck(&paths, "copy", None, &touched).map(|()| Vec::new()));
    assert!(matches!(
        error,
        FolderError::OwnerlessMidUpdate { gog_id: ID }
    ));
    assert_eq!(owner(&paths, &folder), None);
    assert!(lock_for_update(&paths, &folder, "rimworld").is_ok());
}

fn arm(hook: impl FnMut() -> bool + 'static) {
    BETWEEN.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[test]
fn a_folder_that_becomes_a_game_while_locking_is_locked_too() {
    let (root, paths) = setup();
    let folder = fs::canonicalize(root.path())
        .unwrap()
        .join("Games/RimWorld");
    let touched = [folder.join("RimWorld.exe")];
    let (hook_paths, hook_root) = (paths.clone(), root.path().to_owned());
    arm(move || {
        game(&hook_paths, &hook_root, Some("rimworld"), false);
        false
    });
    let held = protect(&paths, "copy", None, &touched, NEVER).unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].lock().folder(), folder);
    assert!(lock_for_update(&paths, &folder, "rimworld").is_err());
}

#[test]
fn folders_that_keep_appearing_are_refused() {
    let (root, paths) = setup();
    let base = fs::canonicalize(root.path()).unwrap();
    let touched: Vec<PathBuf> = (0..ATTEMPTS + 1)
        .map(|n| base.join(format!("g{n}/game.exe")))
        .collect();
    let hook_paths = paths.clone();
    let mut next = touched.clone().into_iter();
    arm(move || {
        if let Some(exe) = next.next() {
            let folder = exe.parent().unwrap();
            fs::create_dir_all(folder).unwrap();
            FolderLock::exclusive(&hook_paths, folder, NEVER)
                .unwrap()
                .write(&FolderState::new(folder.to_owned(), Some("x".to_owned())))
                .unwrap();
        }
        true
    });
    let error = protect(&paths, "copy", None, &touched, NEVER).unwrap_err();
    BETWEEN.with(|slot| slot.borrow_mut().take());
    assert_eq!(
        error.to_string(),
        "copy's GOG folders changed while locking; retry"
    );
}

#[test]
fn a_second_environment_of_the_game_loses_the_claim() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), None, false);
    let touched = vec![folder.join("RimWorldLinux")];
    let (hook_paths, hook_touched) = (paths.clone(), touched.clone());
    // The winner claims after the loser found the folder ownerless.
    arm(move || {
        protect(
            &hook_paths,
            "rimworld",
            Some("1094900565"),
            &hook_touched,
            NEVER,
        )
        .unwrap();
        false
    });
    let error = refusal(protect(
        &paths,
        "other",
        Some("1094900565"),
        &touched,
        NEVER,
    ));
    assert!(matches!(&error, FolderError::OwnedByOther { owner, .. } if owner == "rimworld"));
    assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
}

#[test]
fn two_environments_racing_for_one_folder_leave_one_owner() {
    let (root, paths) = setup();
    let folder = game(&paths, root.path(), None, false);
    let barrier = Arc::new(Barrier::new(2));
    let racers: Vec<_> = ["rimworld", "other"]
        .into_iter()
        .map(|name| {
            let (paths, folder, barrier) = (paths.clone(), folder.clone(), barrier.clone());
            thread::spawn(move || {
                barrier.wait();
                protect(&paths, name, Some("1094900565"), &[folder], NEVER)
                    .map(|_| name)
                    .map_err(|error| error.downcast::<FolderError>().unwrap())
            })
        })
        .collect();
    let results: Vec<_> = racers
        .into_iter()
        .map(|racer| racer.join().unwrap())
        .collect();
    let winners: Vec<_> = results
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .collect();
    assert_eq!(winners.len(), 1, "{results:?}");
    assert_eq!(owner(&paths, &folder).as_deref(), Some(*winners[0]));
    assert!(results.iter().any(|result| matches!(
        result,
        Err(FolderError::OwnedByOther { owner, .. }) if owner == winners[0]
    )));
}

#[test]
fn a_portable_gog_id_names_no_game() {
    let (root, _) = setup();
    let home = root.path().to_owned();
    let portable = Paths::select(
        move |key| (key == "HOME").then(|| home.clone().into_os_string()),
        Some(&root.path().join("usb")),
    )
    .unwrap();
    assert!(
        protect(&portable, "rimworld", Some("1094900565"), &[], NEVER)
            .unwrap()
            .is_empty()
    );
}
