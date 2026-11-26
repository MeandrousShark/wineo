//! Starting an Epic game through the CLI against the fake Legendary: Epic's
//! arguments reach the game and nothing else, each start asks once, other
//! targets and `exec` start plainly, and the offline notices and refusals.
//! The game runs under a fake wine that never prints its arguments, so
//! only the game's command line can hold them.

mod common;

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use common::epic::{Epic, Fake};
use common::{FAKE_WINE, FAKE_WINESERVER, KillOnDrop, stderr, stdout, wait_for, write_script};

const APP: &str = "AppOffline";
const NAME: &str = "offline-game";
/// In every Epic argument the fake hands out, and nowhere else.
const MARK: &str = "EPICSECRET";

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
fn assert_quiet(output: &Output) {
    assert!(!stdout(output).contains(MARK), "{}", stdout(output));
    assert!(!stderr(output).contains(MARK), "{}", stderr(output));
}

/// Logged in, with the game installed through the fake and a quiet wine
/// for starting it.
fn installed() -> Epic {
    let epic = Epic::new();
    epic.on_path(&Fake::default());
    epic.log_in();
    assert_ok(&epic.install(&["import", "epic", "--game", APP, "--yes"]));
    let quiet = epic.file("quiet-bin");
    fs::create_dir_all(&quiet).unwrap();
    let wine = FAKE_WINE.replace("echo \"fake wine: $*\"\n", "");
    assert_ne!(wine, FAKE_WINE);
    write_script(quiet.join("wine"), wine);
    write_script(quiet.join("wineserver"), FAKE_WINESERVER);
    epic
}

/// wineo with the quiet wine ahead of the fixture's.
fn wineo(epic: &Epic) -> Command {
    let mut command = epic.command();
    let path = std::env::join_paths([
        epic.bin.as_os_str(),
        epic.file("quiet-bin").as_os_str(),
        epic.f.sandbox.as_os_str(),
    ])
    .unwrap();
    command.env("PATH", path);
    command
}

fn run(epic: &Epic, args: &[&str]) -> Output {
    wineo(epic).args(args).output().unwrap()
}

fn folder(epic: &Epic) -> PathBuf {
    epic.games().join("Offline Game")
}

fn exe(epic: &Epic) -> String {
    folder(epic)
        .join("Binaries/Win64/Game.exe")
        .display()
        .to_string()
}

fn config(epic: &Epic) -> PathBuf {
    epic.f
        .config_dir()
        .join(format!("environments/{NAME}.toml"))
}

/// Adds `lines` to the game's own target.
fn edit_target(epic: &Epic, lines: &str) {
    let text = fs::read_to_string(config(epic)).unwrap();
    assert!(text.contains("[targets.game]\n"), "{text}");
    let text = text.replace("[targets.game]\n", &format!("[targets.game]\n{lines}\n"));
    fs::write(config(epic), text).unwrap();
}

fn launch_calls(epic: &Epic) -> usize {
    epic.calls()
        .iter()
        .filter(|call| call.starts_with("launch "))
        .count()
}

fn pids_file(epic: &Epic) -> PathBuf {
    epic.f
        .root
        .path()
        .join(format!("data/wineo/prefixes/{NAME}/fake-pids"))
}

/// The running game's arguments, read from its process, waiting for a game
/// whose pid isn't `not`.
fn game_args(epic: &Epic, not: Option<u32>) -> (u32, Vec<String>) {
    let mut found = None;
    wait_for("the game to start", || {
        let Ok(text) = fs::read_to_string(pids_file(epic)) else {
            return false;
        };
        let Some(pid) = text
            .split_whitespace()
            .next()
            .and_then(|pid| pid.parse().ok())
        else {
            return false;
        };
        if Some(pid) == not {
            return false;
        }
        found = Some(pid);
        true
    });
    let pid = found.unwrap();
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    let words: Vec<String> = cmdline
        .split(|&byte| byte == 0)
        .filter(|word| !word.is_empty())
        .map(|word| String::from_utf8_lossy(word).into_owned())
        .collect();
    // `sh`, then the fake wine itself.
    let at = words
        .iter()
        .position(|word| word.ends_with("/quiet-bin/wine"))
        .unwrap_or_else(|| panic!("{words:?}"));
    (pid, words[at + 1..].to_vec())
}

fn epic_arguments(code: &str) -> Vec<String> {
    [
        "-AUTH_LOGIN=unused".to_owned(),
        format!("-AUTH_PASSWORD={code}"),
        "-AUTH_TYPE=exchangecode".to_owned(),
        format!("-epicapp={APP}"),
        "-epicenv=Prod".to_owned(),
        "-EpicPortal".to_owned(),
        "-epicusername=EPICSECRETuser".to_owned(),
        "-epicuserid=EPICSECRETid".to_owned(),
        "-epiclocale=en".to_owned(),
        "-epicsandboxid=0".to_owned(),
    ]
    .to_vec()
}

fn expected(epic: &Epic, middle: &[&str], code: Option<&str>) -> Vec<String> {
    let mut args = vec![exe(epic)];
    args.extend(middle.iter().map(|&arg| arg.to_owned()));
    if let Some(code) = code {
        args.extend(epic_arguments(code));
    }
    args
}

/// Every file under the test's root, wineo's config, data, state, and
/// cache, and its `HOME`, that holds the mark. The fakes hold it themselves.
#[track_caller]
fn assert_written_nowhere(epic: &Epic) {
    assert_eq!(epic.files_holding(MARK), Vec::<PathBuf>::new());
}

fn stop(epic: &Epic) {
    assert_ok(&run(epic, &["stop", NAME]));
    epic.f.wait_for_run_end(NAME, "stopped");
}

#[test]
fn a_start_gives_the_game_epics_arguments_last_and_writes_them_nowhere() {
    let epic = installed();
    edit_target(&epic, "args = [\"-fromtarget\"]");
    let output = run(&epic, &["start", NAME, "--", "-fromcli"]);
    assert_ok(&output);
    assert_eq!(stdout(&output), format!("Started {NAME}\n"));
    assert_eq!(stderr(&output), "");
    let (first, args) = game_args(&epic, None);
    assert_eq!(
        args,
        expected(
            &epic,
            &["-skipintro", "-lang=en", "-fromtarget", "-fromcli"],
            Some("EPICSECRETcode")
        )
    );
    assert_eq!(launch_calls(&epic), 1);
    for args in [
        &["status"][..],
        &["status", NAME],
        &["status", NAME, "--json"],
        &["status", "--json"],
        &["logs", NAME],
        &["list"],
    ] {
        let output = run(&epic, args);
        assert_ok(&output);
        assert_quiet(&output);
    }
    assert_written_nowhere(&epic);

    // A restart replays the command line, never Epic's arguments, and asks
    // again; its checks before stopping the run ask nothing.
    let output = run(&epic, &["restart", NAME]);
    assert_ok(&output);
    assert_quiet(&output);
    assert_eq!(stdout(&output), format!("Restarted {NAME}\n"));
    let (_, args) = game_args(&epic, Some(first));
    assert_eq!(
        args,
        expected(
            &epic,
            &["-skipintro", "-lang=en", "-fromtarget", "-fromcli"],
            Some("EPICSECRETcode")
        )
    );
    assert_eq!(launch_calls(&epic), 2);
    assert_written_nowhere(&epic);
    stop(&epic);
    assert_written_nowhere(&epic);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_foreground_run_gives_the_game_epics_arguments_and_prints_none() {
    let epic = installed();
    let child = KillOnDrop::group(
        wineo(&epic)
            .args(["run", NAME])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    let (_, args) = game_args(&epic, None);
    assert_eq!(
        args,
        expected(&epic, &["-skipintro", "-lang=en"], Some("EPICSECRETcode"))
    );
    assert_written_nowhere(&epic);
    assert_ok(&run(&epic, &["stop", NAME]));
    let output = child.wait_with_output().unwrap();
    assert_quiet(&output);
    assert_eq!(launch_calls(&epic), 1);
    assert_written_nowhere(&epic);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn other_targets_and_exec_start_without_asking_epic() {
    let epic = installed();
    let text = fs::read_to_string(config(&epic)).unwrap();
    fs::write(
        config(&epic),
        format!("{text}\n[targets.tool]\nexe = \"{}\"\n", exe(&epic)),
    )
    .unwrap();
    let output = run(&epic, &["start", NAME, "--target", "tool"]);
    assert_ok(&output);
    assert_eq!(stderr(&output), "");
    let (_, args) = game_args(&epic, None);
    assert_eq!(args, [exe(&epic)]);
    stop(&epic);
    let output = run(&epic, &["exec", NAME, "exit-0"]);
    assert_ok(&output);
    assert_eq!(launch_calls(&epic), 0);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn logged_out_a_game_marked_offline_starts_from_wineos_record_with_a_notice() {
    let epic = installed();
    assert_ok(&epic.run(&["import", "epic", "--logout"]));
    let output = run(&epic, &["start", NAME]);
    assert_ok(&output);
    assert_eq!(
        stderr(&output),
        format!("Not logged in to Epic, so {NAME} starts offline. Online features may not work.\n")
    );
    let (_, args) = game_args(&epic, None);
    assert_eq!(args, expected(&epic, &["-skipintro", "-lang=en"], None));
    stop(&epic);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn without_legendary_a_game_marked_offline_starts_from_wineos_record_with_a_notice() {
    let epic = installed();
    fs::remove_file(epic.bin.join("legendary")).unwrap();
    let output = run(&epic, &["start", NAME]);
    assert_ok(&output);
    assert_eq!(
        stderr(&output),
        format!(
            "Legendary isn't installed, so {NAME} starts offline. Online features may not work.\n"
        )
    );
    let (_, args) = game_args(&epic, None);
    assert_eq!(args, expected(&epic, &["-skipintro", "-lang=en"], None));
    stop(&epic);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn when_epic_doesnt_answer_a_game_marked_offline_starts_offline_with_a_notice() {
    let epic = installed();
    fs::write(epic.file("launch-mode"), "unreachable").unwrap();
    let output = run(&epic, &["start", NAME]);
    assert_ok(&output);
    assert_eq!(
        stderr(&output),
        format!("Epic didn't answer, so {NAME} starts offline. Online features may not work.\n")
    );
    let (_, args) = game_args(&epic, None);
    assert_eq!(args, expected(&epic, &["-skipintro", "-lang=en"], Some("")));
    assert_written_nowhere(&epic);
    stop(&epic);
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

/// Marks the game as one that can't run offline, as Legendary would have
/// recorded it.
fn not_offline(epic: &Epic) {
    let paths = epic.f.paths();
    let lock = wineo_core::epic::game::RecordLock::exclusive(&paths, APP, &|| false).unwrap();
    let mut record = lock.read(&paths).unwrap().unwrap();
    record.can_run_offline = false;
    lock.write(&paths, &record).unwrap();
}

#[track_caller]
fn assert_refused(epic: &Epic, message: &str) {
    let output = run(epic, &["start", NAME]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), format!("error: {message}\n"));
    assert!(!pids_file(epic).exists());
    assert!(!epic.f.runtime(NAME).is_running().unwrap());
}

#[test]
fn a_game_not_marked_offline_refuses_when_epic_cant_be_asked() {
    let epic = installed();
    not_offline(&epic);
    fs::write(epic.file("launch-mode"), "unreachable").unwrap();
    assert_refused(
        &epic,
        &format!(
            "{NAME} isn't marked to run offline, and Epic didn't answer: `legendary launch {APP} --json --no-wine --skip-version-check` failed: exit code 1: [cli] ERROR: Login failed, cannot continue!"
        ),
    );
    fs::remove_file(epic.file("launch-mode")).unwrap();
    assert_ok(&epic.run(&["import", "epic", "--logout"]));
    assert_refused(
        &epic,
        &format!(
            "{NAME} isn't marked to run offline, so it needs an Epic login. Log in with wineo import epic --login."
        ),
    );
    fs::remove_file(epic.bin.join("legendary")).unwrap();
    assert_refused(
        &epic,
        &format!(
            "{NAME} isn't marked to run offline, so it needs Legendary to start. Get it with wineo import epic --login."
        ),
    );
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}

#[test]
fn a_refusal_runs_the_after_hook_its_before_hook_needs() {
    let epic = installed();
    not_offline(&epic);
    assert_ok(&epic.run(&["import", "epic", "--logout"]));
    let hooks = epic.file("hooks.log");
    edit_target(
        &epic,
        &format!(
            "hooks.before = \"echo before >> '{0}'\"\nhooks.after = \"echo after >> '{0}'\"",
            hooks.display()
        ),
    );
    let output = run(&epic, &["start", NAME]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(fs::read_to_string(&hooks).unwrap(), "before\nafter\n");
    assert!(!epic.f.runtime(NAME).is_running().unwrap());
    assert_eq!(epic.sweep(), Vec::<i32>::new());
}
