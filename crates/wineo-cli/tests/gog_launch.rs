//! Launch protection for GOG game folders through the CLI: which launches
//! are refused, claiming an ownerless folder, and the in-use lock a run
//! holds until it ends.

mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use common::{Fixture, assert_success, stderr, wait_for, write_script};
use wineo_core::gog::folder::{FolderError, lock_for_update};

const ID: u64 = 1094900565;

fn write_config(f: &Fixture, name: &str, text: &str) {
    let dir = f.config_dir().join("environments");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.toml")), text).unwrap();
}

/// The game folder, with a Linux binary that runs until stopped and a
/// Windows one the fake wine runs the same way.
fn folder(f: &Fixture) -> PathBuf {
    let folder = fs::canonicalize(f.root.path())
        .unwrap()
        .join("Games/RimWorld");
    fs::create_dir_all(&folder).unwrap();
    let linux = folder.join("RimWorldLinux");
    write_script(&linux, "#!/bin/sh\nexec sleep 60\n");
    fs::write(folder.join("RimWorld.exe"), "").unwrap();
    folder
}

/// A native RimWorld owning its folder.
fn rimworld(f: &Fixture, mid_update: bool) -> PathBuf {
    let folder = folder(f);
    write_config(
        f,
        "rimworld",
        &format!(
            "kind = \"native\"\ngog_id = \"{ID}\"\n\n[targets.game]\nexe = \"{}\"\n",
            folder.join("RimWorldLinux").display()
        ),
    );
    f.gog_folder(&folder, Some("rimworld"), ID, mid_update);
    folder
}

/// A Wine environment with no `gog_id`, launching `target`.
fn copy(f: &Fixture, target: &str) {
    f.environment("copy", &format!("[targets.game]\n{target}\n"));
}

#[track_caller]
fn assert_refused(output: &Output, message: &str) {
    assert!(!output.status.success(), "{}", stderr(output));
    assert!(stderr(output).contains(message), "{}", stderr(output));
}

const MID_UPDATE: &str =
    "rimworld's game folder is mid-update. Finish it with wineo update rimworld.";

fn update_refused(f: &Fixture, folder: &Path) -> bool {
    matches!(
        lock_for_update(&f.paths(), folder, "rimworld")
            .err()
            .and_then(|error| error.downcast::<FolderError>().ok()),
        Some(FolderError::InUse { .. })
    )
}

#[test]
fn an_owner_mid_update_is_refused() {
    let f = Fixture::new();
    rimworld(&f, true);
    assert_refused(&f.run(&["start", "rimworld"]), MID_UPDATE);
    assert!(!f.runtime("rimworld").is_running().unwrap());
}

#[test]
fn a_clone_running_the_game_is_refused_naming_the_owner() {
    let f = Fixture::new();
    let folder = rimworld(&f, true);
    copy(
        &f,
        &format!("exe = \"{}\"", folder.join("RimWorld.exe").display()),
    );
    assert_refused(&f.run(&["start", "copy"]), MID_UPDATE);
}

#[test]
fn a_windows_path_is_translated_through_the_prefix_drives() {
    let f = Fixture::new();
    let folder = rimworld(&f, true);
    copy(&f, "exe = 'D:\\RimWorld\\RimWorld.exe'");
    let dosdevices = f.root.path().join("prefixes/copy/dosdevices");
    fs::create_dir_all(&dosdevices).unwrap();
    symlink(folder.parent().unwrap(), dosdevices.join("d:")).unwrap();
    assert_refused(&f.run(&["start", "copy"]), MID_UPDATE);
}

#[test]
fn a_z_drive_path_reaches_the_folder_through_the_host_root() {
    let f = Fixture::new();
    let folder = rimworld(&f, true);
    let exe = folder.join("RimWorld.exe").display().to_string();
    copy(&f, &format!("exe = 'Z:{}'", exe.replace('/', "\\")));
    let dosdevices = f.root.path().join("prefixes/copy/dosdevices");
    fs::create_dir_all(&dosdevices).unwrap();
    symlink("/", dosdevices.join("z:")).unwrap();
    assert_refused(&f.run(&["start", "copy"]), MID_UPDATE);

    f.gog_folder(&folder, Some("rimworld"), ID, false);
    assert_success(&f.run(&["start", "copy"]));
    assert!(update_refused(&f, &folder));
    assert_success(&f.run(&["stop", "copy"]));
    f.wait_for_run_end("copy", "stopped");
}

#[test]
fn a_working_directory_in_the_folder_is_refused() {
    let f = Fixture::new();
    let folder = rimworld(&f, true);
    copy(
        &f,
        &format!("exe = \"winecfg\"\ncwd = \"{}\"", folder.display()),
    );
    assert_refused(&f.run(&["start", "copy"]), MID_UPDATE);
}

#[test]
fn an_argument_in_the_folder_is_refused_even_before_it_exists() {
    let f = Fixture::new();
    let folder = rimworld(&f, true);
    copy(
        &f,
        &format!(
            "exe = \"winecfg\"\nargs = [\"-savedatafolder\", \"{}\"]",
            folder.join("Saves/new").display()
        ),
    );
    assert_refused(&f.run(&["start", "copy"]), MID_UPDATE);
}

#[test]
fn a_native_environment_naming_the_folder_is_refused() {
    let f = Fixture::new();
    let folder = rimworld(&f, true);
    write_config(
        &f,
        "tool",
        &format!(
            "kind = \"native\"\n\n[targets.game]\nexe = \"/bin/sh\"\nargs = [\"{}\"]\n",
            folder.join("RimWorldLinux").display()
        ),
    );
    assert_refused(&f.run(&["start", "tool"]), MID_UPDATE);
}

#[test]
fn a_lost_state_is_refused() {
    let f = Fixture::new();
    let folder = folder(&f);
    write_config(
        &f,
        "rimworld",
        &format!(
            "kind = \"native\"\ngog_id = \"{ID}\"\n\n[targets.game]\nexe = \"{}\"\n",
            folder.join("RimWorldLinux").display()
        ),
    );
    assert_refused(
        &f.run(&["start", "rimworld"]),
        "rimworld's GOG state is missing. Migrate it again with wineo import gog --game 1094900565, or stop tracking it with wineo set rimworld --unset gog_id.",
    );
}

#[test]
fn a_clone_of_a_removed_owner_runs_unless_mid_update() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    copy(
        &f,
        &format!("exe = \"{}\"", folder.join("RimWorld.exe").display()),
    );
    assert_success(&f.run(&["rm", "rimworld"]));
    assert_success(&f.run(&["start", "copy"]));
    assert_success(&f.run(&["stop", "copy"]));
    f.wait_for_run_end("copy", "stopped");

    f.gog_folder(&folder, None, ID, true);
    assert_refused(
        &f.run(&["start", "copy"]),
        "GOG game 1094900565's folder is mid-update. Migrate it again with wineo import gog --game 1094900565 and finish it.",
    );
    assert_eq!(f.gog_state(&folder).unwrap().owner, None);
}

#[test]
fn an_ownerless_folder_of_its_game_is_claimed_at_launch() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    f.gog_folder(&folder, None, ID, false);
    assert_success(&f.run(&["start", "rimworld"]));
    assert_eq!(
        f.gog_state(&folder).unwrap().owner.as_deref(),
        Some("rimworld")
    );
    assert_success(&f.run(&["stop", "rimworld"]));
    f.wait_for_run_end("rimworld", "stopped");
}

#[test]
fn racing_launches_claim_an_ownerless_folder_once() {
    let f = Fixture::new();
    let folder = folder(&f);
    f.environment("rimworld", "[targets.game]\nexe = \"winecfg\"\n");
    let config = f.config_dir().join("environments/rimworld.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("gog_id = \"{ID}\"\n{text}")).unwrap();
    f.gog_folder(&folder, None, ID, false);
    let arg = folder.join("Saves").display().to_string();
    let racers: Vec<_> = (0..2)
        .map(|_| {
            f.wineo()
                .args(["exec", "rimworld", "exit-0", &arg])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for racer in racers {
        assert_success(&racer.wait_with_output().unwrap());
    }
    let state = f.gog_state(&folder).unwrap();
    assert_eq!(state.owner.as_deref(), Some("rimworld"));
    assert!(state.journal.is_none());
}

#[test]
fn only_a_broken_state_covering_the_launch_refuses_it() {
    let f = Fixture::new();
    let folder = folder(&f);
    let elsewhere = fs::canonicalize(f.root.path()).unwrap().join("Elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("Tool.exe"), "").unwrap();
    let state = wineo_core::gog::folder::state_path(&f.paths(), &folder);
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&state, "not = [valid").unwrap();

    copy(
        &f,
        &format!("exe = \"{}\"", elsewhere.join("Tool.exe").display()),
    );
    assert_success(&f.run(&["start", "copy"]));
    assert_success(&f.run(&["stop", "copy"]));
    f.wait_for_run_end("copy", "stopped");

    copy(
        &f,
        &format!("exe = \"{}\"", folder.join("RimWorld.exe").display()),
    );
    assert_refused(
        &f.run(&["start", "copy"]),
        &format!("The GOG state for {} can't be checked: ", folder.display()),
    );
}

#[test]
fn a_background_run_holds_the_folder_until_it_stops() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    assert_success(&f.run(&["start", "rimworld"]));
    assert!(update_refused(&f, &folder));
    assert_success(&f.run(&["stop", "rimworld"]));
    wait_for("the supervisor to release the folder", || {
        !update_refused(&f, &folder)
    });

    let updating = lock_for_update(&f.paths(), &folder, "rimworld").unwrap();
    assert_refused(
        &f.run(&["start", "rimworld"]),
        "rimworld's game folder is being updated.",
    );
    drop(updating);
    assert!(!f.runtime("rimworld").is_running().unwrap());
}

/// What an update of rimworld would say, or None when it can take the folder.
fn update_refusal(f: &Fixture, folder: &Path) -> Option<String> {
    lock_for_update(&f.paths(), folder, "rimworld")
        .err()
        .map(|error| error.to_string())
}

#[test]
fn the_refusal_names_a_background_run_from_the_moment_start_returns() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    // The supervisor re-takes the lock and leaves its own record before
    // start lets go of its, so there is never a running game with no name.
    assert_success(&f.run(&["start", "rimworld"]));
    assert_eq!(
        update_refusal(&f, &folder).as_deref(),
        Some("rimworld is running from this game folder. Stop it before updating rimworld.")
    );
    assert_success(&f.run(&["stop", "rimworld"]));
    wait_for("the supervisor to release the folder", || {
        update_refusal(&f, &folder).is_none()
    });
}

#[test]
fn the_refusal_names_the_environment_running_an_exec() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    f.environment("copy", "");
    let exe = folder.join("RimWorld.exe").display().to_string();
    let mut running = f
        .wineo()
        .args(["exec", "copy", &exe])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pids = f.root.path().join("prefixes/copy/fake-pids");
    wait_for("the program to start", || pids.exists());
    assert_eq!(
        update_refusal(&f, &folder).as_deref(),
        Some("copy is running from this game folder. Stop it before updating rimworld.")
    );
    running.kill().unwrap();
    running.wait().unwrap();
    wait_for("the exec to release the folder", || {
        update_refusal(&f, &folder).is_none()
    });
}

#[test]
fn exec_holds_the_folder_until_the_program_exits() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    f.environment("copy", "");
    let exe = folder.join("RimWorld.exe").display().to_string();
    let mut running = f
        .wineo()
        .args(["exec", "copy", &exe])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pids = f.root.path().join("prefixes/copy/fake-pids");
    wait_for("the program to start", || pids.exists());
    assert!(update_refused(&f, &folder));
    let program = fs::read_to_string(&pids).unwrap();
    let program = program.split_whitespace().next().unwrap();
    assert!(
        std::process::Command::new("kill")
            .arg(program)
            .status()
            .unwrap()
            .success()
    );
    running.wait().unwrap();
    assert!(!update_refused(&f, &folder));

    let updating = lock_for_update(&f.paths(), &folder, "rimworld").unwrap();
    assert_refused(
        &f.run(&["exec", "copy", &exe]),
        "rimworld's game folder is being updated.",
    );
    drop(updating);

    f.gog_folder(&folder, Some("rimworld"), ID, true);
    assert_refused(&f.run(&["exec", "copy", &exe]), MID_UPDATE);
}

/// Hooks that leave a marker file each, so a test can tell whether they ran.
fn hooks(f: &Fixture) -> (String, PathBuf, PathBuf) {
    let before = f.root.path().join("before-ran");
    let after = f.root.path().join("after-ran");
    let table = format!(
        "[hooks]\nbefore = \"touch '{}'\"\nafter = \"touch '{}'\"\n",
        before.display(),
        after.display()
    );
    (table, before, after)
}

#[test]
fn no_hook_runs_for_an_owner_refused_mid_update() {
    let f = Fixture::new();
    let folder = folder(&f);
    let (table, before, after) = hooks(&f);
    write_config(
        &f,
        "rimworld",
        &format!(
            "kind = \"native\"\ngog_id = \"{ID}\"\n\n{table}\n[targets.game]\nexe = \"{}\"\n",
            folder.join("RimWorldLinux").display()
        ),
    );
    f.gog_folder(&folder, Some("rimworld"), ID, true);
    assert_refused(&f.run(&["start", "rimworld"]), MID_UPDATE);
    assert!(!before.exists());
    assert!(!after.exists());

    f.gog_folder(&folder, Some("rimworld"), ID, false);
    assert_success(&f.run(&["start", "rimworld"]));
    assert!(before.exists());
    assert_success(&f.run(&["stop", "rimworld"]));
    f.wait_for_run_end("rimworld", "stopped");
}

#[test]
fn no_hook_runs_for_a_clone_refused_mid_update() {
    let f = Fixture::new();
    let folder = rimworld(&f, true);
    let (table, before, after) = hooks(&f);
    f.environment(
        "copy",
        &format!(
            "{table}\n[targets.game]\nexe = \"{}\"\n",
            folder.join("RimWorld.exe").display()
        ),
    );
    assert_refused(&f.run(&["start", "copy"]), MID_UPDATE);
    assert!(!before.exists());
    assert!(!after.exists());
}

#[test]
fn restart_keeps_the_run_when_the_folder_went_mid_update() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    assert_success(&f.run(&["start", "rimworld"]));
    f.gog_folder(&folder, Some("rimworld"), ID, true);
    assert_refused(&f.run(&["restart", "rimworld"]), MID_UPDATE);
    assert!(f.runtime("rimworld").is_running().unwrap());
    assert_success(&f.run(&["stop", "rimworld"]));
    f.wait_for_run_end("rimworld", "stopped");
}

#[test]
fn ctrl_c_while_waiting_to_claim_the_folder_exits_130() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    f.gog_folder(&folder, None, ID, false);
    let held =
        wineo_core::gog::folder::FolderLock::exclusive(&f.paths(), &folder, &|| false).unwrap();
    let child = f
        .wineo()
        .args(["start", "rimworld"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The launch holds the in-use lock while it waits for the state lock.
    wait_for("the launch to wait for the state lock", || {
        update_refused(&f, &folder)
    });
    let sent = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    drop(held);
    assert_eq!(f.gog_state(&folder).unwrap().owner, None);
    assert!(!f.runtime("rimworld").is_running().unwrap());
}

#[test]
fn no_hook_runs_for_an_owner_mid_update_whose_exe_is_gone() {
    let f = Fixture::new();
    let folder = folder(&f);
    fs::remove_file(folder.join("RimWorldLinux")).unwrap();
    let (table, before, after) = hooks(&f);
    write_config(
        &f,
        "rimworld",
        &format!(
            "kind = \"native\"\ngog_id = \"{ID}\"\n\n{table}\n[targets.game]\nexe = \"{}\"\n",
            folder.join("RimWorldLinux").display()
        ),
    );
    f.gog_folder(&folder, Some("rimworld"), ID, true);
    assert_refused(&f.run(&["start", "rimworld"]), MID_UPDATE);
    assert!(!before.exists());
    assert!(!after.exists());
}

const WAITING: &str = "Waiting for programs handoff started. Press Ctrl+C to stop them.\n";

#[test]
fn exec_waits_for_what_a_handoff_leaves_running_in_the_folder() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    f.environment("copy", "");
    let prefix = f.root.path().join("prefixes/copy");
    let started = std::time::Instant::now();
    let mut running = f
        .wineo()
        .args(["exec", "copy", "handoff", &folder.display().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The fake program exits right after writing this, leaving a descendant
    // the fake wineserver -w waits for.
    wait_for("the program to hand off", || {
        prefix.join("fake-pids").exists()
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!prefix.join("handoff-done").exists());
    assert!(running.try_wait().unwrap().is_none());
    assert!(update_refused(&f, &folder));
    let output = running.wait_with_output().unwrap();
    assert_success(&output);
    assert!(stderr(&output).contains(WAITING), "{}", stderr(&output));
    assert!(prefix.join("handoff-done").exists());
    assert!(started.elapsed() >= std::time::Duration::from_secs(2));
    assert!(!update_refused(&f, &folder));
}

#[test]
fn ctrl_c_during_the_wait_asks_the_prefix_to_close() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    f.environment("copy", "");
    let prefix = f.root.path().join("prefixes/copy");
    let log = f.root.path().join("exec-stderr");
    let mut running = f
        .wineo()
        .args(["exec", "copy", "handoff", &folder.display().to_string()])
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    wait_for("the waiting line", || {
        fs::read_to_string(&log).is_ok_and(|text| text.contains(WAITING))
    });
    let sent = std::process::Command::new("kill")
        .args(["-INT", &running.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    // The started program's own status, though the wait was stopped.
    assert!(running.wait().unwrap().success());
    let text = fs::read_to_string(&log).unwrap();
    assert!(
        text.contains("Press Ctrl+C again to force them to stop.\n"),
        "{text}"
    );
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert!(!prefix.join("handoff-done").exists());
    assert!(!update_refused(&f, &folder));
}

#[test]
fn exec_from_a_folder_says_nothing_when_nothing_is_left_running() {
    let f = Fixture::new();
    let folder = rimworld(&f, false);
    f.environment("copy", "");
    let output = f.run(&["exec", "copy", "exit-0", &folder.display().to_string()]);
    assert_success(&output);
    assert!(
        !stderr(&output).contains("Waiting for programs"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn exec_away_from_game_folders_returns_when_its_program_exits() {
    let f = Fixture::new();
    rimworld(&f, false);
    f.environment("copy", "");
    let prefix = f.root.path().join("prefixes/copy");
    // A file rather than a pipe, which the descendant would hold open.
    let log = f.root.path().join("exec-stderr");
    let status = f
        .wineo()
        .args(["exec", "copy", "handoff"])
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log).unwrap())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!prefix.join("handoff-done").exists());
    assert!(
        !fs::read_to_string(&log)
            .unwrap()
            .contains("Waiting for programs")
    );
    wait_for("the handoff to finish", || {
        prefix.join("handoff-done").exists()
    });
}

#[test]
fn a_terminal_ctrl_c_stops_the_wait_in_order() {
    use std::os::unix::process::CommandExt;

    let f = Fixture::new();
    let folder = rimworld(&f, false);
    f.environment("copy", "");
    let prefix = f.root.path().join("prefixes/copy");
    let log = f.root.path().join("exec-stderr");
    // A group of its own stands in for the terminal's foreground job, so the
    // test's own processes aren't signaled with it.
    let mut running = f
        .wineo()
        .args(["exec", "copy", "handoff", &folder.display().to_string()])
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log).unwrap())
        .process_group(0)
        .spawn()
        .unwrap();
    wait_for("the waiting line", || {
        fs::read_to_string(&log).is_ok_and(|text| text.contains(WAITING))
    });
    let sent = std::process::Command::new("kill")
        .args(["-INT", &format!("-{}", running.id())])
        .status()
        .unwrap();
    assert!(sent.success());
    // What the handoff left behind still runs for a moment after it's asked
    // to close, and the folder stays held until it's gone.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(running.try_wait().unwrap().is_none());
    assert!(update_refused(&f, &folder));
    let status = running.wait().unwrap();
    let text = fs::read_to_string(&log).unwrap();
    assert!(status.success(), "{text}");
    assert!(
        text.contains("Press Ctrl+C again to force them to stop.\n"),
        "{text}"
    );
    assert!(!prefix.join("handoff-done").exists());
    assert!(!update_refused(&f, &folder));
}
