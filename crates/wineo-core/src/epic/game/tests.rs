use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::gog::test_support::setup;

const NEVER: &dyn Fn() -> bool = &|| false;
const APP: &str = "AppOffline";

fn folder(root: &Path) -> PathBuf {
    let folder = root.join("Games/Offline Game");
    fs::create_dir_all(&folder).unwrap();
    folder
}

fn write(paths: &Paths, record: &Record) {
    RecordLock::exclusive(paths, &record.app_name, NEVER)
        .unwrap()
        .write(paths, record)
        .unwrap();
}

fn installed(folder: &Path) -> Record {
    Record {
        installed_version: Some("1.0.2-win".to_owned()),
        can_run_offline: true,
        dlc: vec![Dlc {
            app_name: "0000000000000000000000000000000c".to_owned(),
            version: Some("3".to_owned()),
        }],
        offline: Some(OfflineStart {
            executable: "OfflineGame.exe".to_owned(),
            arguments: vec!["-nosplash".to_owned()],
            working_directory: None,
        }),
        ..Record::new(APP, folder.to_owned())
    }
}

/// An environment config naming `epic_id`, as an install leaves it.
fn environment(paths: &Paths, name: &str, epic_id: &str) {
    let config = paths.environment_config(name);
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        format!("prefix = '/p/{name}'\nrunner = 'wine'\narch = 'win64'\nepic_id = '{epic_id}'\n"),
    )
    .unwrap();
}

fn refusal(result: Result<impl std::fmt::Debug>) -> String {
    result.unwrap_err().to_string()
}

#[test]
fn a_record_round_trips_and_leaves_no_temporary_file() {
    let (root, paths) = setup();
    let record = installed(&folder(root.path()));
    write(&paths, &record);
    assert_eq!(peek(&paths, APP).unwrap(), Some(record.clone()));
    let names: Vec<String> = fs::read_dir(paths.epic_games_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, [format!("{}.toml", key(APP))]);
    let text = fs::read_to_string(record_path(&paths, APP)).unwrap();
    assert!(
        text.starts_with("version = 1\napp_name = \"AppOffline\"\n"),
        "{text}"
    );
    assert!(!text.contains("in_flight"), "{text}");
}

#[test]
fn the_file_name_is_a_hash_so_any_app_name_is_safe() {
    let (root, paths) = setup();
    for app in ["Dlc Game", "../escape", "a/b"] {
        let record = Record::new(app, folder(root.path()));
        write(&paths, &record);
        assert_eq!(peek(&paths, app).unwrap(), Some(record));
        assert!(record_path(&paths, app).starts_with(paths.epic_games_dir()));
    }
    assert_eq!(list(&paths).unwrap().len(), 3);
    assert!(!root.path().join("escape").exists());
}

#[test]
fn a_record_written_as_an_install_begins_reads_with_defaults() {
    let text = "app_name = 'AppOffline'\nfolder = '/games/Offline'\n\n[in_flight]\nwork = 'install'\napp = 'AppOffline'\n";
    let record = Record::parse(text).unwrap();
    assert_eq!(record.version, VERSION);
    assert_eq!(record.installed_version, None);
    assert!(!record.can_run_offline);
    assert!(record.dlc.is_empty());
    assert_eq!(record.offline, None);
    assert_eq!(
        record.in_flight,
        Some(InFlight {
            work: Work::Install,
            app: Some(APP.to_owned()),
        })
    );
}

#[test]
fn fields_this_wineo_doesnt_know_are_ignored() {
    let text = "version = 1\napp_name = 'AppOffline'\nfolder = '/games/Offline'\nfuture = true\n\n[[dlc]]\napp_name = 'Extra'\nsize = 3\n\n[offline]\nexecutable = 'a.exe'\n\n[in_flight]\nwork = 'move'\n";
    let record = Record::parse(text).unwrap();
    assert_eq!(
        record.dlc,
        [Dlc {
            app_name: "Extra".to_owned(),
            version: None
        }]
    );
    assert_eq!(record.offline.unwrap().arguments, Vec::<String>::new());
    // A mark this wineo can't name still blocks launches.
    assert_eq!(
        record.in_flight,
        Some(InFlight {
            work: Work::Unknown,
            app: None
        })
    );
    let bare = Record::parse("app_name = 'A'\nfolder = '/g'\n[in_flight]\n").unwrap();
    assert_eq!(bare.in_flight.unwrap().work, Work::Unknown);
}

#[test]
fn a_record_that_cant_be_trusted_is_refused() {
    for (text, message) in [
        (
            "version = 2\napp_name = 'A'\nfolder = '/g'\n",
            "Epic game record version 2 is from a newer wineo",
        ),
        ("app_name = ''\nfolder = '/g'\n", "Invalid epic_id: empty."),
        (
            "app_name = 'A'\nfolder = 'g'\n",
            "Epic game folder g isn't a canonical path",
        ),
        (
            "app_name = 'A'\nfolder = '/g/../h'\n",
            "Epic game folder /g/../h isn't a canonical path",
        ),
        ("folder = '/g'\n", "missing field `app_name`"),
        ("app_name = 'A'\n", "missing field `folder`"),
    ] {
        assert_eq!(
            Record::parse(text).unwrap_err().to_string(),
            message,
            "{text}"
        );
    }
}

#[test]
fn a_record_under_another_games_name_is_refused() {
    let (root, paths) = setup();
    write(&paths, &Record::new(APP, folder(root.path())));
    fs::rename(record_path(&paths, APP), record_path(&paths, "Other")).unwrap();
    assert!(refusal(peek(&paths, "Other")).ends_with("names AppOffline, not Other"));
    let listed = list(&paths).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(
        listed[0]
            .1
            .as_ref()
            .unwrap_err()
            .to_string()
            .ends_with("names AppOffline, which isn't its key")
    );
}

#[test]
fn a_write_cut_off_by_a_kill_leaves_the_old_record_and_is_cleaned_up() {
    let (root, paths) = setup();
    let record = installed(&folder(root.path()));
    write(&paths, &record);
    // What a run killed between the temporary file and the rename leaves.
    let stale = paths
        .epic_games_dir()
        .join(format!("{}abc123", temp_prefix(APP)));
    fs::write(&stale, "app_name = 'half").unwrap();
    assert_eq!(list(&paths).unwrap().len(), 1);
    assert_eq!(peek(&paths, APP).unwrap(), Some(record.clone()));
    // Another game's write leaves it alone; this game's next one removes it.
    write(&paths, &Record::new("Other", folder(root.path())));
    assert!(stale.exists());
    write(&paths, &record);
    assert!(!stale.exists());
}

#[test]
fn writes_need_the_exclusive_lock_and_the_right_game() {
    let (root, paths) = setup();
    let record = Record::new(APP, folder(root.path()));
    let shared = RecordLock::shared(&paths, APP, NEVER).unwrap();
    assert_eq!(
        refusal(shared.write(&paths, &record)),
        "Epic game record changed under a shared lock"
    );
    drop(shared);
    let other = RecordLock::exclusive(&paths, "Other", NEVER).unwrap();
    assert_eq!(
        refusal(other.write(&paths, &record)),
        "record for AppOffline written under the wrong lock"
    );
    assert_eq!(peek(&paths, APP).unwrap(), None);
}

#[test]
fn a_waiting_record_lock_can_be_canceled() {
    let (_root, paths) = setup();
    let _held = RecordLock::exclusive(&paths, APP, NEVER).unwrap();
    let error = RecordLock::shared(&paths, APP, &|| true).err().unwrap();
    assert!(matches!(error.downcast_ref(), Some(GameError::Canceled)));
}

#[test]
fn marks_are_written_and_cleared_and_removing_forgets_the_game() {
    let (root, paths) = setup();
    let mut record = Record::new(APP, folder(root.path()));
    let lock = RecordLock::exclusive(&paths, APP, NEVER).unwrap();
    lock.mark(&paths, &mut record, Work::Install, APP).unwrap();
    assert_eq!(
        lock.read(&paths).unwrap().unwrap().in_flight,
        Some(InFlight {
            work: Work::Install,
            app: Some(APP.to_owned())
        })
    );
    lock.clear_mark(&paths, &mut record).unwrap();
    assert_eq!(lock.read(&paths).unwrap().unwrap().in_flight, None);
    assert!(lock.remove(&paths).unwrap());
    assert!(!lock.remove(&paths).unwrap());
    assert_eq!(peek(&paths, APP).unwrap(), None);
}

#[test]
fn owners_are_the_environments_naming_the_game() {
    let (_root, paths) = setup();
    environment(&paths, "fowl", APP);
    environment(&paths, "zed", APP);
    environment(&paths, "other", "Other");
    let broken = paths.environment_config("broken");
    fs::write(&broken, "epic_id = 'AppOffline'\nnot toml").unwrap();
    assert_eq!(owners(&paths, APP).unwrap(), ["fowl", "zed"]);
    assert_eq!(epic_id_of(&paths, "other").as_deref(), Some("Other"));
    assert_eq!(epic_id_of(&paths, "broken"), None);
    assert!(owners(&paths, "Nobody").unwrap().is_empty());
}

#[test]
fn work_refuses_while_a_game_runs_and_while_other_work_holds_the_folder() {
    let (root, paths) = setup();
    let folder = folder(root.path());
    let running = in_use_lock(&paths, &folder)
        .unwrap()
        .try_shared("fowl-copy")
        .unwrap()
        .unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        refusal(lock_for(&paths, &folder, "fowl", Work::Update)),
        "fowl-copy is running from this game folder. Stop it before updating fowl."
    );
    assert_eq!(
        refusal(lock_for(&paths, &folder, "fowl", Work::Repair)),
        "fowl-copy is running from this game folder. Stop it before repairing fowl."
    );
    assert_eq!(
        refusal(lock_for(&paths, &folder, "Offline Game", Work::Install)),
        "fowl-copy is running from this game folder. Stop it before installing Offline Game."
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    drop(running);

    // Two installs of one game: the second never waits.
    let installing = lock_for(&paths, &folder, "Offline Game", Work::Install).unwrap();
    let error = lock_for(&paths, &folder, "Offline Game", Work::Install).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(GameError::InUse { holders, .. }) if holders.is_empty()
    ));
    assert_eq!(
        error.to_string(),
        "Something else is using Offline Game's game folder. Try again once it's done."
    );
    drop(installing);
    assert!(lock_for(&paths, &folder, "Offline Game", Work::Install).is_ok());
}

#[test]
fn the_folder_lock_is_keyed_by_folder_and_reopens_for_a_supervisor() {
    let (root, paths) = setup();
    let folder = folder(root.path());
    let lock = in_use_lock(&paths, &folder).unwrap();
    let text = toml::to_string(&lock).unwrap();
    let reopened: InUseLock = toml::from_str(&text).unwrap();
    let _held = lock.try_shared("fowl").unwrap().unwrap();
    assert!(reopened.try_exclusive().unwrap().is_none());
    // Another folder, and GOG's lock on this one, are separate.
    assert!(lock_for(&paths, &root.path().join("Games/Other"), "x", Work::Update).is_ok());
    assert!(
        crate::gog::folder::lock_for_update(&paths, &folder, "x").is_ok(),
        "Epic and GOG share a folder lock"
    );
    assert!(in_use_lock(&paths, Path::new("relative")).is_err());
}

mod launch {
    use super::*;

    #[test]
    fn an_environment_without_a_record_is_refused() {
        let (_root, paths) = setup();
        environment(&paths, "fowl", APP);
        let message = "fowl's Epic record is missing. Repair it with wineo update fowl --repair.";
        assert_eq!(refusal(precheck(&paths, "fowl", Some(APP), &[])), message);
        assert_eq!(
            refusal(protect(&paths, "fowl", Some(APP), &[], NEVER)),
            message
        );
    }

    #[test]
    fn an_unreadable_record_is_refused() {
        let (_root, paths) = setup();
        let path = record_path(&paths, APP);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "app_name = 'AppOffline'\n").unwrap();
        let message = refusal(protect(&paths, "fowl", Some(APP), &[], NEVER));
        assert!(
            message.starts_with("fowl's Epic record can't be checked: ")
                && message.ends_with("missing field `folder`)."),
            "{message}"
        );
    }

    #[test]
    fn a_mark_left_by_a_killed_run_refuses_until_the_work_finishes() {
        let (root, paths) = setup();
        let folder = folder(root.path());
        environment(&paths, "fowl", APP);
        for (work, message) in [
            (
                Work::Install,
                "fowl's Epic install didn't finish. Finish it with wineo import epic --game AppOffline.",
            ),
            (
                Work::Update,
                "fowl's game folder is mid-update. Finish it with wineo update fowl.",
            ),
            (
                Work::Repair,
                "fowl's game folder is mid-repair. Finish it with wineo update fowl --repair.",
            ),
            (
                Work::Unknown,
                "fowl's game folder has Epic work that didn't finish. Repair it with wineo update fowl --repair.",
            ),
        ] {
            // What a killed run leaves: its mark on disk, and its folder lock
            // gone with its process.
            let working = lock_for(&paths, &folder, "fowl", work).unwrap();
            let mut record = installed(&folder);
            RecordLock::exclusive(&paths, APP, NEVER)
                .unwrap()
                .mark(&paths, &mut record, work, APP)
                .unwrap();
            drop(working);

            assert_eq!(refusal(precheck(&paths, "fowl", Some(APP), &[])), message);
            let error = protect(&paths, "fowl", Some(APP), &[], NEVER).unwrap_err();
            assert_eq!(error.to_string(), message);
            assert!(matches!(
                error.downcast_ref(),
                Some(GameError::Unfinished { work: found, .. }) if *found == work
            ));
            // The refusal held nothing, so the same work can run again.
            let resumed = lock_for(&paths, &folder, "fowl", work).unwrap();
            RecordLock::exclusive(&paths, APP, NEVER)
                .unwrap()
                .clear_mark(&paths, &mut record)
                .unwrap();
            drop(resumed);
            assert_eq!(
                protect(&paths, "fowl", Some(APP), &[], NEVER)
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[test]
    fn a_launch_during_work_is_refused_naming_the_work() {
        let (root, paths) = setup();
        let folder = folder(root.path());
        environment(&paths, "fowl", APP);
        let mut record = installed(&folder);
        write(&paths, &record);
        let working = lock_for(&paths, &folder, "fowl", Work::Repair).unwrap();
        // Before the mark is written, the work is taken for an update.
        assert_eq!(
            refusal(protect(&paths, "fowl", Some(APP), &[], NEVER)),
            "fowl's Epic game is being updated."
        );
        RecordLock::exclusive(&paths, APP, NEVER)
            .unwrap()
            .mark(&paths, &mut record, Work::Repair, APP)
            .unwrap();
        let error = precheck(&paths, "fowl", Some(APP), &[]).unwrap_err();
        assert!(matches!(error.downcast_ref(), Some(GameError::Busy { .. })));
        assert_eq!(error.to_string(), "fowl's Epic game is being repaired.");
        drop(working);
    }

    #[test]
    fn a_running_game_holds_its_folder_until_it_exits() {
        let (root, paths) = setup();
        let folder = folder(root.path());
        environment(&paths, "fowl", APP);
        write(&paths, &installed(&folder));
        let guards = protect(&paths, "fowl", Some(APP), &[], NEVER).unwrap();
        assert_eq!(guards.len(), 1);
        assert_eq!(guards[0].lock().folder(), folder);
        assert_eq!(
            refusal(lock_for(&paths, &folder, "fowl", Work::Update)),
            "fowl is running from this game folder. Stop it before updating fowl."
        );
        drop(guards);
        assert!(lock_for(&paths, &folder, "fowl", Work::Update).is_ok());
    }

    #[test]
    fn a_copy_running_from_the_folder_holds_it_too() {
        let (root, paths) = setup();
        let folder = folder(root.path());
        environment(&paths, "fowl", APP);
        write(&paths, &installed(&folder));
        let exe = folder.join("OfflineGame.exe");
        let outside = root.path().join("Games/Offline");
        let guards = protect(&paths, "copy", None, &[outside.clone(), exe.clone()], NEVER).unwrap();
        assert_eq!(guards.len(), 1);
        assert_eq!(
            refusal(lock_for(&paths, &folder, "fowl", Work::Update)),
            "copy is running from this game folder. Stop it before updating fowl."
        );
        drop(guards);
        // A path beside the folder, sharing only a prefix of its name, isn't in it.
        assert!(
            protect(&paths, "copy", None, &[outside], NEVER)
                .unwrap()
                .is_empty()
        );

        let mut record = installed(&folder);
        RecordLock::exclusive(&paths, APP, NEVER)
            .unwrap()
            .mark(&paths, &mut record, Work::Update, APP)
            .unwrap();
        assert_eq!(
            refusal(protect(
                &paths,
                "copy",
                None,
                std::slice::from_ref(&exe),
                NEVER
            )),
            "fowl's game folder is mid-update. Finish it with wineo update fowl."
        );
        let _working = lock_for(&paths, &folder, "fowl", Work::Update).unwrap();
        assert_eq!(
            refusal(precheck(&paths, "copy", None, &[exe])),
            "fowl's Epic game is being updated."
        );
    }

    #[test]
    fn an_ownerless_mark_names_the_game_and_folder() {
        let (root, paths) = setup();
        let folder = folder(root.path());
        let exe = folder.join("OfflineGame.exe");
        let mut record = Record::new(APP, folder.clone());
        let lock = RecordLock::exclusive(&paths, APP, NEVER).unwrap();
        lock.mark(&paths, &mut record, Work::Install, APP).unwrap();
        assert_eq!(
            refusal(precheck(&paths, "copy", None, std::slice::from_ref(&exe))),
            format!(
                "Epic game AppOffline's install in {} didn't finish. Finish it with wineo import epic --game AppOffline.",
                folder.display()
            )
        );
        lock.mark(&paths, &mut record, Work::Repair, APP).unwrap();
        drop(lock);
        assert_eq!(
            refusal(protect(&paths, "copy", None, &[exe], NEVER)),
            format!(
                "Epic game AppOffline's folder {} is mid-repair. Finish it from the environment that owns it.",
                folder.display()
            )
        );
    }

    #[test]
    fn a_broken_record_elsewhere_doesnt_block_a_plain_launch() {
        let (root, paths) = setup();
        let dir = paths.epic_games_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("broken.toml"), "not toml").unwrap();
        let exe = root.path().join("notepad.exe");
        assert!(
            protect(&paths, "notepad", None, &[exe], NEVER)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_portable_root_owns_no_epic_game() {
        let (root, _) = setup();
        let home = root.path().to_owned();
        let portable = Paths::select(
            move |key| (key == "HOME").then(|| home.clone().into_os_string()),
            Some(&root.path().join("usb")),
        )
        .unwrap();
        assert!(
            protect(&portable, "fowl", Some(APP), &[], NEVER)
                .unwrap()
                .is_empty()
        );
        assert!(precheck(&portable, "fowl", Some(APP), &[]).is_ok());
    }
}

#[test]
fn a_restore_is_refused_while_another_environment_has_the_game() {
    let (_root, paths) = setup();
    assert!(claim_for_restore(&paths, "fowl", APP).is_ok());
    environment(&paths, "fowl", APP);
    assert!(claim_for_restore(&paths, "fowl", APP).is_ok());
    fs::remove_file(paths.environment_config("fowl")).unwrap();
    environment(&paths, "copy", APP);
    let error = claim_for_restore(&paths, "fowl", APP).err().unwrap();
    assert!(matches!(
        error.downcast_ref(),
        Some(GameError::Taken { other, .. }) if other == "copy"
    ));
    assert_eq!(
        error.to_string(),
        "fowl's Epic game AppOffline belongs to copy now."
    );
    // The claim holds the record lock, so an install can't check in between.
    let claim = claim_for_restore(&paths, "other", "Other").unwrap();
    let error = RecordLock::exclusive(&paths, "Other", &|| true)
        .err()
        .unwrap();
    assert!(matches!(error.downcast_ref(), Some(GameError::Canceled)));
    drop(claim);
}
