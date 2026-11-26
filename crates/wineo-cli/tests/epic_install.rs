//! `wineo import epic --game` and `--forget` against the fake Legendary:
//! the review, the runs and their progress, cancel and resume, the shared
//! memory a killed download leaves, two installs at once, and `wineo rm`.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::{Output, Stdio};

use common::epic::Epic;
use common::pty::Pty;
use common::{KillOnDrop, stderr, stdout};
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
use serde_json::Value;

const APP: &str = "AppOffline";

/// Logged in, with the fake on `PATH`.
fn epic() -> Epic {
    let epic = Epic::new();
    epic.on_path(&common::epic::Fake::default());
    epic.log_in();
    epic
}

#[track_caller]
fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

#[track_caller]
fn assert_code(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn list_json(epic: &Epic) -> Value {
    let output = epic.run(&["import", "epic", "--list", "--json"]);
    assert_ok(&output);
    serde_json::from_str(&stdout(&output)).unwrap()
}

fn game<'a>(list: &'a Value, app: &str) -> &'a Value {
    list["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["id"] == app)
        .unwrap()
}

fn record(epic: &Epic, app: &str) -> Option<wineo_core::epic::game::Record> {
    wineo_core::epic::game::peek(&epic.f.paths(), app).unwrap()
}

/// `wineo import epic --game APP --yes`, started in the background.
fn spawn_install(epic: &Epic, app: &str) -> KillOnDrop {
    KillOnDrop::new(
        epic.install_command()
            .args(["import", "epic", "--game", app, "--yes"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

fn interrupt(child: &KillOnDrop) {
    let pid = Pid::from_raw(child.id() as i32).unwrap();
    let pidfd = pidfd_open(pid, PidfdFlags::empty()).unwrap();
    pidfd_send_signal(&pidfd, Signal::INT).unwrap();
}

#[test]
fn a_game_installs_with_a_review_progress_and_its_environment() {
    let epic = epic();
    let output = epic.install(&["import", "epic", "--game", APP, "--yes"]);
    assert_ok(&output);
    let folder = epic.games().join("Offline Game");
    assert_eq!(
        stdout(&output),
        "Install Offline Game (AppOffline)\n\
         \x20 environment  offline-game (new Windows environment)\n\
         \x20 folder       ~/Games/Offline Game\n\
         \x20 runner       wine\n\
         \x20 arch         win64\n\
         \x20 version      0.24\n\
         \x20 download     1.4 GiB\n\
         \x20 installed    2.3 GiB\n\
         Installed Offline Game as offline-game.\n"
    );
    let err = stderr(&output);
    assert!(
        err.contains("Downloading: Offline Game\nDownloading: 50%\nDownloading: 100%\n"),
        "{err}"
    );
    assert_eq!(
        epic.install_calls(),
        [format!(
            "-y install AppOffline --base-path {} --game-folder Offline Game --skip-dlcs --disable-sdl",
            epic.games().display()
        )]
    );
    let config =
        fs::read_to_string(epic.f.config_dir().join("environments/offline-game.toml")).unwrap();
    assert!(config.contains("epic_id = \"AppOffline\""), "{config}");
    assert!(
        config.contains(&format!(
            "exe = \"{}\"",
            folder.join("Binaries/Win64/Game.exe").display()
        )),
        "{config}"
    );
    assert_eq!(record(&epic, APP).unwrap().in_flight, None);

    let list = list_json(&epic);
    let row = game(&list, APP);
    assert_eq!(row["state"], "installed");
    assert_eq!(row["environment"], "offline-game");
    assert_eq!(row["version"], "1.0");
    assert_eq!(row["dlc"], serde_json::json!([]));
    assert_eq!(game(&list, "AppToken")["state"], "not_installed");
    let table = stdout(&epic.run(&["import", "epic", "--list"]));
    assert!(
        table.contains(
            "ID             TITLE             STATE                                              ENVIRONMENT   VERSION\n\
             Dlc Game       Dlc Game          not installed\n\
             AppOffline     Offline Game      installed                                          offline-game  1.0\n"
        ),
        "{table}"
    );

    let again = epic.install(&["import", "epic", "--game", APP]);
    assert_ok(&again);
    assert_eq!(
        stdout(&again),
        "Already installed. offline-game owns ~/Games/Offline Game.\n"
    );
    assert_eq!(epic.install_calls().len(), 1);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_game_with_dlc_counts_its_runs_and_a_failed_dlc_is_a_warning() {
    let epic = epic();
    epic.set_mode("0000000000000000000000000000000e", "unrecorded");
    let output = epic.install(&["import", "epic", "--game", "Dlc Game", "--yes"]);
    assert_ok(&output);
    let out = stdout(&output);
    assert!(
        out.contains(
            "  dlc          Dlc Game Extra One Content\n\
             \x20              Extra Two Dlc Game Content\n\
             \x20 download     145.8 GiB plus 2 DLC\n\
             \x20 installed    152.1 GiB plus 2 DLC\n"
        ),
        "{out}"
    );
    assert!(out.ends_with("Installed Dlc Game as dlc-game.\n"), "{out}");
    let err = stderr(&output);
    for line in [
        "warning: Dlc Game isn't marked to run offline, so it starts only while Epic answers.",
        "Downloading 1 of 3: Dlc Game\nDownloading 1 of 3: 50%\nDownloading 1 of 3: 100%",
        "Downloading 2 of 3: Dlc Game Extra One Content\nDownloading 2 of 3: 50%\nDownloading 2 of 3: 100%",
        "Downloading 3 of 3: Extra Two Dlc Game Content",
        "warning: Couldn't install Extra Two Dlc Game Content: Legendary didn't install Extra Two Dlc Game Content: it isn't in Legendary's records afterwards. The game installed without it.",
    ] {
        assert!(err.contains(line), "{line}\n---\n{err}");
    }
    assert_eq!(epic.install_calls().len(), 3);
    let list = list_json(&epic);
    assert_eq!(
        game(&list, "Dlc Game")["dlc"],
        serde_json::json!([{
            "id": "0000000000000000000000000000000c",
            "title": "Dlc Game Extra One Content",
            "state": "installed",
        }])
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_dry_run_reviews_and_changes_nothing() {
    let epic = epic();
    let output = epic.install(&["import", "epic", "--game", APP, "--dry-run"]);
    assert_ok(&output);
    let out = stdout(&output);
    assert!(
        out.starts_with("Install Offline Game (AppOffline)\n"),
        "{out}"
    );
    assert!(out.ends_with("Dry run, nothing was installed.\n"), "{out}");
    assert!(epic.install_calls().is_empty());
    assert!(
        epic.calls()
            .iter()
            .any(|call| call == "info AppOffline --json")
    );
    assert!(record(&epic, APP).is_none());
    assert!(!epic.games().exists());
    assert!(!epic.f.config_dir().join("environments").exists());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_dry_run_without_legendary_says_what_it_would_ask_and_stops() {
    let epic = Epic::new();
    epic.python("3.14.0");
    let output = epic.install(&["import", "epic", "--game", APP, "--dry-run"]);
    assert_ok(&output);
    let err = stderr(&output);
    assert!(
        err.contains("Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it?\nDry run, so wineo didn't ask or download anything."),
        "{err}"
    );
    assert!(!epic.own_copy().exists());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn without_a_terminal_it_needs_yes() {
    let epic = epic();
    let output = epic.install(&["import", "epic", "--game", APP]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: Installing without a terminal needs --yes.\n"
    );
    assert!(epic.calls().is_empty());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn flags_that_dont_fit_are_usage_errors() {
    let epic = epic();
    for args in [
        &["import", "epic", "--forget", APP, "--yes"][..],
        &["import", "epic", "--forget", APP, "--dry-run"],
        &["import", "epic", "--runner", "wine"],
        &["import", "epic", "--list", "--dir", "/tmp/x"],
        &["import", "epic", "--game", APP, "--forget", APP],
        &["import", "epic", "--game", APP, "--list"],
        &["import", "epic", "--game", APP, "--refresh"],
        &["import", "epic", "--game", APP, "--json"],
        &["import", "epic", "--forget", APP, "--dir", "/tmp/x"],
        &["import", "epic", "--login", "--runner", "wine"],
    ] {
        assert_code(&epic.run(args), 2);
    }
    let output = epic.install(&["import", "epic", "--game", " x", "--yes"]);
    assert_code(&output, 2);
    assert_eq!(stderr(&output), "error: \" x\" isn't an Epic game id.\n");
    assert!(epic.calls().is_empty());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_portable_root_refuses_installing_and_forgetting() {
    let epic = epic();
    let portable = epic.file("portable");
    fs::create_dir_all(&portable).unwrap();
    for flag in ["--game", "--forget"] {
        let output = epic
            .install_command()
            .args(["--portable", portable.to_str().unwrap()])
            .args(
                ["import", "epic", flag, APP, "--yes"]
                    .iter()
                    .filter(|arg| flag == "--game" || **arg != "--yes"),
            )
            .output()
            .unwrap();
        assert_code(&output, 1);
        assert_eq!(
            stderr(&output),
            "error: Epic games live in the host library. Run this without --portable.\n",
            "{flag}"
        );
    }
    assert!(epic.calls().is_empty());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn not_owned_and_unavailable_games_are_refused() {
    let epic = epic();
    let output = epic.install(&["import", "epic", "--game", "AppNobody", "--yes"]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: AppNobody isn't one of your Epic games. See wineo import epic --list.\n"
    );
    let list = epic.edited_list(|list| {
        for game in list.as_array_mut().unwrap() {
            if game["app_name"] == "AppOffline" {
                game["metadata"]["customAttributes"]["ThirdPartyManagedApp"] =
                    serde_json::json!({"type": "STRING", "value": "Origin"});
            }
        }
    });
    epic.on_path(&common::epic::Fake {
        list,
        ..common::epic::Fake::default()
    });
    let output = epic.run(&["import", "epic", "--list", "--refresh"]);
    assert_ok(&output);
    let output = epic.install(&["import", "epic", "--game", APP, "--yes"]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: Offline Game can't be installed: another launcher manages it.\n"
    );
    // The probe's list marks AppThirdParty as needing a Ubisoft account.
    let list = epic.edited_list(|_| {});
    epic.on_path(&common::epic::Fake {
        list,
        ..common::epic::Fake::default()
    });
    assert_ok(&epic.run(&["import", "epic", "--list", "--refresh"]));
    let output = epic.install(&["import", "epic", "--game", "AppThirdParty", "--yes"]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: Third Party Game can't be installed: it needs an account at another launcher.\n"
    );
    assert!(epic.install_calls().is_empty());
    assert!(!epic.calls().iter().any(|call| call.starts_with("info")));
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn ctrl_c_interrupts_the_download_keeps_the_mark_and_the_same_command_resumes() {
    let epic = epic();
    epic.set_mode(APP, "hang");
    let child = spawn_install(&epic, APP);
    epic.wait_started(APP);
    interrupt(&child);
    let output = child.wait_with_output().unwrap();
    assert_code(&output, 130);
    let err = stderr(&output);
    assert!(
        err.ends_with(
            "Install interrupted.\nFinish the interrupted install with: wineo import epic --game AppOffline\n"
        ),
        "{err}"
    );
    // Legendary's whole group got the interrupt, and the fake left its
    // resume file as Legendary does.
    assert_eq!(
        fs::read_to_string(epic.file("got-int")).unwrap(),
        "AppOffline\n"
    );
    assert!(epic.state().join("tmp/AppOffline.resume").exists());
    assert_eq!(
        record(&epic, APP).unwrap().in_flight.map(|mark| mark.work),
        Some(wineo_core::epic::game::Work::Install)
    );
    assert!(
        !epic
            .f
            .config_dir()
            .join("environments/offline-game.toml")
            .exists()
    );
    assert_eq!(game(&list_json(&epic), APP)["state"], "unfinished");
    assert_eq!(epic.sweep(), Vec::<i32>::new());

    epic.set_mode(APP, "");
    let output = epic.install(&["import", "epic", "--game", APP, "--yes"]);
    assert_ok(&output);
    let out = stdout(&output);
    assert!(
        out.starts_with("Finish the interrupted install of Offline Game (AppOffline)\n"),
        "{out}"
    );
    assert!(
        out.ends_with("Finished the interrupted install of offline-game.\n"),
        "{out}"
    );
    let calls = epic.install_calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1], "the same folder");
    assert!(!epic.state().join("tmp/AppOffline.resume").exists());
    assert_eq!(game(&list_json(&epic), APP)["state"], "installed");
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn ctrl_c_kills_a_download_that_ignores_it_and_removes_only_its_shared_memory() {
    let epic = epic();
    let tag = format!("wineoclitest{}x{}", std::process::id(), line!());
    fs::write(epic.file("shm-tag"), &tag).unwrap();
    let held = PathBuf::from(format!("/dev/shm/psm_{tag}_held"));
    let decoy = PathBuf::from(format!("/dev/shm/psm_{tag}_decoy"));
    struct Cleanup(Vec<PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for path in &self.0 {
                let _ = fs::remove_file(path);
            }
        }
    }
    let _cleanup = Cleanup(vec![held.clone(), decoy.clone()]);
    fs::write(&decoy, "").unwrap();
    epic.set_mode(APP, "deaf");
    let child = spawn_install(&epic, APP);
    epic.wait_started(APP);
    common::wait_for("the fake's shared memory", || held.exists());
    interrupt(&child);
    let output = child.wait_with_output().unwrap();
    assert_code(&output, 130);
    assert!(
        stderr(&output).contains("Install interrupted.\n"),
        "{}",
        stderr(&output)
    );
    assert!(!held.exists(), "the group's segment is removed");
    assert!(decoy.exists(), "a segment it didn't hold is left");
    assert!(record(&epic, APP).unwrap().in_flight.is_some());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_run_that_exits_0_without_installing_fails_and_can_be_finished() {
    let epic = epic();
    epic.set_mode(APP, "failed-write");
    let output = epic.install(&["import", "epic", "--game", APP, "--yes"]);
    assert_code(&output, 1);
    let err = stderr(&output);
    assert!(
        err.ends_with(
            "error: Legendary didn't install Offline Game: it logged an error (Writing file failed: [Errno 28] No space left on device).\n\
             Finish the interrupted install with: wineo import epic --game AppOffline\n"
        ),
        "{err}"
    );
    assert!(record(&epic, APP).unwrap().in_flight.is_some());
    // The game is in Legendary's records now, so finishing doesn't download
    // it again.
    epic.set_mode(APP, "");
    let output = epic.install(&["import", "epic", "--game", APP, "--yes"]);
    assert_ok(&output);
    assert_eq!(epic.install_calls().len(), 1);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_download_with_no_progress_is_said() {
    let epic = epic();
    epic.set_mode(APP, "quiet");
    let output = epic
        .install_command()
        .env("WINEO_TEST_EPIC_STALL_SECONDS", "1")
        .args(["import", "epic", "--game", APP, "--yes"])
        .output()
        .unwrap();
    assert_ok(&output);
    let err = stderr(&output);
    assert_eq!(
        err.matches("warning: No download progress for 0 minutes. It keeps trying; press Ctrl+C to stop, and run the same command again to resume.\n").count(),
        1,
        "{err}"
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_game_another_environment_owns_isnt_installed_again() {
    let epic = epic();
    let folder = epic.games().join("Offline Game");
    fs::create_dir_all(&folder).unwrap();
    epic.f.environment("other", "");
    let config = epic.f.config_dir().join("environments/other.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("epic_id = \"AppOffline\"\n{text}")).unwrap();

    // Owned, but without a record.
    let output = epic.install(&["import", "epic", "--game", APP, "--yes"]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: other's Epic record is missing. Repair it with wineo update other --repair.\n"
    );

    let paths = epic.f.paths();
    let lock = wineo_core::epic::game::RecordLock::exclusive(&paths, APP, &|| false).unwrap();
    lock.write(
        &paths,
        &wineo_core::epic::game::Record::new(APP, folder.clone()),
    )
    .unwrap();
    drop(lock);
    let output = epic.install(&["import", "epic", "--game", APP, "--yes"]);
    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        "Already installed. other owns ~/Games/Offline Game.\n"
    );
    assert!(epic.calls().is_empty(), "{:?}", epic.calls());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn two_installs_of_one_game_at_once_install_it_once() {
    let epic = epic();
    // The second starts once the first is downloading, which takes it three
    // seconds, so the two always overlap there.
    epic.set_mode(APP, "quiet");
    let first = spawn_install(&epic, APP);
    epic.wait_started(APP);
    let second = spawn_install(&epic, APP);
    let lost = second.wait_with_output().unwrap();
    let won = first.wait_with_output().unwrap();
    assert_ok(&won);
    assert_code(&lost, 1);
    // It found the first's mark and would finish that install, but the
    // first holds the folder.
    assert!(
        stdout(&lost).starts_with("Finish the interrupted install of Offline Game (AppOffline)\n"),
        "{}",
        stdout(&lost)
    );
    assert_eq!(
        stderr(&lost),
        "error: Something else is using offline-game's game folder. Try again once it's done.\n\
         Finish the interrupted install with: wineo import epic --game AppOffline\n"
    );
    assert_eq!(epic.install_calls().len(), 1);
    let environments: Vec<_> = fs::read_dir(epic.f.config_dir().join("environments"))
        .unwrap()
        .collect();
    assert_eq!(environments.len(), 1);
    assert_eq!(record(&epic, APP).unwrap().in_flight, None);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn rm_refuses_an_unfinished_install_unless_forced_and_forget_leaves_the_files() {
    let epic = epic();
    let output = epic.install(&["import", "epic", "--game", APP, "--yes"]);
    assert_ok(&output);
    let paths = epic.f.paths();
    let lock = wineo_core::epic::game::RecordLock::exclusive(&paths, APP, &|| false).unwrap();
    let mut record = lock.read(&paths).unwrap().unwrap();
    lock.mark(
        &paths,
        &mut record,
        wineo_core::epic::game::Work::Install,
        APP,
    )
    .unwrap();
    drop(lock);
    let output = epic.run(&["rm", "offline-game"]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: offline-game's Epic install didn't finish. Finish it with wineo import epic --game AppOffline, or remove it anyway with --force.\n"
    );
    assert_ok(&epic.run(&["rm", "offline-game", "--force"]));

    // In the trash, it can't be forgotten.
    let output = epic.run(&["import", "epic", "--forget", APP]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: Epic game AppOffline belongs to offline-game in the trash, so it can't be forgotten. Restore it with wineo restore, or remove it for good with wineo prune.\n"
    );
    assert_ok(&epic.run(&["prune", "--all", "--yes"]));
    let output = epic.run(&["import", "epic", "--forget", APP]);
    assert_ok(&output);
    let folder = epic.games().join("Offline Game");
    assert_eq!(
        stdout(&output),
        "Forgot Epic game AppOffline. Its files are still in ~/Games/Offline Game.\n"
    );
    assert!(folder.join("Binaries/Win64/Game.exe").exists());
    assert!(
        epic.calls()
            .contains(&"-y uninstall AppOffline --keep-files".to_owned())
    );
    assert_eq!(game(&list_json(&epic), APP)["state"], "not_installed");
    let output = epic.run(&["import", "epic", "--forget", APP]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: wineo has no record of Epic game AppOffline.\n"
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_proton_install_writes_the_store_beside_epic_id() {
    let epic = epic();
    epic.f.install_proton("GE-Proton-Test");
    let output = epic.install(&[
        "import",
        "epic",
        "--game",
        APP,
        "--yes",
        "--runner",
        "GE-Proton-Test",
    ]);
    assert_ok(&output);
    let config =
        fs::read_to_string(epic.f.config_dir().join("environments/offline-game.toml")).unwrap();
    assert!(config.contains("runner = \"GE-Proton-Test\""), "{config}");
    assert!(
        config.contains("epic_id = \"AppOffline\"\nstore = \"egs\"\n"),
        "{config}"
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_game_only_epic_says_needs_another_launcher_is_refused_before_downloading() {
    let epic = Epic::new();
    let list = epic.edited_list(|list| {
        for game in list.as_array_mut().unwrap() {
            if game["app_name"] == "AppThirdParty" {
                game["metadata"]["customAttributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("partnerLinkType");
            }
        }
    });
    epic.on_path(&common::epic::Fake {
        list,
        ..common::epic::Fake::default()
    });
    epic.log_in();
    let output = epic.install(&["import", "epic", "--game", "AppThirdParty", "--yes"]);
    assert_code(&output, 1);
    assert_eq!(
        stderr(&output),
        "error: Third Party Game can't be installed: it needs an account at another launcher.\n"
    );
    assert!(
        epic.calls()
            .contains(&"info AppThirdParty --json".to_owned())
    );
    assert!(epic.install_calls().is_empty());
    assert!(record(&epic, "AppThirdParty").is_none());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn at_a_terminal_it_asks_and_no_installs_nothing() {
    let epic = epic();
    let mut command = epic.install_command();
    command.args(["import", "epic", "--game", APP]);
    let mut pty = Pty::spawn(command);
    pty.expect("Install Offline Game (AppOffline)");
    pty.expect("installed    2.3 GiB");
    pty.expect("Esc or Ctrl+C to cancel.");
    pty.expect("Install it? [Y/n]: ");
    pty.send("n");
    let (status, seen) = pty.finish();
    assert_eq!(status.code(), Some(130), "{seen}");
    assert!(seen.contains("Install canceled."), "{seen}");
    assert!(epic.install_calls().is_empty());
    assert!(record(&epic, APP).is_none(), "no record, so no mark");
    assert!(!epic.f.config_dir().join("environments").exists());
    assert!(!epic.games().exists());
    assert_eq!(epic.sweep(), Vec::<i32>::new());

    // Yes installs it.
    let mut command = epic.install_command();
    command.args(["import", "epic", "--game", APP]);
    let mut pty = Pty::spawn(command);
    pty.expect("Install it? [Y/n]: ");
    pty.send("y");
    let (status, seen) = pty.finish();
    assert!(status.success(), "{seen}");
    assert!(
        seen.contains("Installed Offline Game as offline-game."),
        "{seen}"
    );
    assert_eq!(epic.install_calls().len(), 1);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}
