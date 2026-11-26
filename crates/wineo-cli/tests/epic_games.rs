//! Epic games' guards through the CLI: `epic_id`'s config rules, `set`,
//! clone, portable copy, trash restore, launch refusals and the folder lock,
//! and what validate reports. Tests write the records; no Legendary runs.

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Fixture, assert_success, stderr, stdout};
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
use wineo_core::epic::game::{self, Record, RecordLock, Work};

const APP: &str = "AppOffline";
const NEVER: &dyn Fn() -> bool = &|| false;

/// The game's folder, with an exe the fake wine runs until stopped.
fn folder(f: &Fixture) -> PathBuf {
    let folder = fs::canonicalize(f.root.path())
        .unwrap()
        .join("Games/Offline Game");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("OfflineGame.exe"), "").unwrap();
    folder
}

fn config_path(f: &Fixture, name: &str) -> PathBuf {
    f.config_dir().join(format!("environments/{name}.toml"))
}

/// A Wine environment running `exe`, with `epic_id` when given, as an older
/// wineo would have left it: no `schema_version`.
fn environment(f: &Fixture, name: &str, epic_id: Option<&str>, exe: &Path) {
    f.environment(
        name,
        &format!("[targets.game]\nexe = \"{}\"\n", exe.display()),
    );
    if let Some(epic_id) = epic_id {
        let path = config_path(f, name);
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("epic_id = \"{epic_id}\"\n{text}")).unwrap();
    }
}

/// The game's record, installed, with `mark` left on it.
fn record(f: &Fixture, folder: &Path, mark: Option<Work>) {
    let paths = f.paths();
    let mut record = Record {
        installed_version: Some("1.0".to_owned()),
        can_run_offline: true,
        ..Record::new(APP, folder.to_owned())
    };
    let lock = RecordLock::exclusive(&paths, APP, NEVER).unwrap();
    lock.write(&paths, &record).unwrap();
    if let Some(work) = mark {
        lock.mark(&paths, &mut record, work, APP).unwrap();
    }
}

/// fowl, owning the game, with its record.
fn fowl(f: &Fixture, mark: Option<Work>) -> PathBuf {
    let folder = folder(f);
    environment(f, "fowl", Some(APP), &folder.join("OfflineGame.exe"));
    record(f, &folder, mark);
    folder
}

/// A file's bytes and modification time, to show nothing rewrote it.
fn stamp(path: &Path) -> (Vec<u8>, i64, i64) {
    let meta = fs::metadata(path).unwrap();
    (fs::read(path).unwrap(), meta.mtime(), meta.mtime_nsec())
}

#[track_caller]
fn assert_refused(output: &Output, message: &str) {
    assert!(!output.status.success(), "{}", stdout(output));
    assert!(stderr(output).contains(message), "{}", stderr(output));
}

fn trash_id(output: &Output) -> String {
    stdout(output)
        .lines()
        .find_map(|line| line.strip_prefix("Restore with: wineo restore "))
        .unwrap()
        .to_owned()
}

fn update_refusal(f: &Fixture, folder: &Path) -> Option<String> {
    game::lock_for(&f.paths(), folder, "fowl", Work::Update)
        .err()
        .map(|error| error.to_string())
}

fn validate_codes(f: &Fixture, args: &[&str]) -> Vec<(String, String)> {
    let mut command = vec!["validate"];
    command.extend_from_slice(args);
    command.push("--json");
    let output = f.run(&command);
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    report["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|problem| problem["area"] == "epic")
        .map(|problem| {
            (
                problem["code"].as_str().unwrap().to_owned(),
                problem["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// Kills every live process started for this fixture, found by the state
/// folder wineo gave it, and returns their ids.
fn sweep(f: &Fixture) -> Vec<i32> {
    let needle = format!("XDG_STATE_HOME={}\0", f.root.path().join("state").display());
    let mut found = Vec::new();
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        let Some(handle) = Pid::from_raw(pid) else {
            continue;
        };
        // Pins the process, so the checks and the kill are about it.
        let Ok(pidfd) = pidfd_open(handle, PidfdFlags::empty()) else {
            continue;
        };
        let Ok(environ) = fs::read(format!("/proc/{pid}/environ")) else {
            continue;
        };
        if !environ
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
        {
            continue;
        }
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let state = stat
            .rsplit(')')
            .next()
            .unwrap_or("")
            .split_whitespace()
            .next();
        if matches!(state, Some("Z" | "X") | None) {
            continue;
        }
        let _ = pidfd_send_signal(&pidfd, Signal::KILL);
        found.push(pid);
    }
    found
}

#[test]
fn the_config_rules_reach_validate() {
    let f = Fixture::new();
    let dir = f.config_dir().join("environments");
    fs::create_dir_all(&dir).unwrap();
    for (name, text, message) in [
        (
            "tool",
            "kind = \"native\"\nepic_id = \"AppOffline\"\n",
            "native environment tool can't set epic_id",
        ),
        (
            "bg3",
            "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1\"\nepic_id = \"AppOffline\"\n",
            "bg3 is a Steam game, so it can't set epic_id.",
        ),
        (
            "both",
            "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\nepic_id = \"AppOffline\"\ngog_id = \"1\"\n",
            "both can't set both epic_id and gog_id.",
        ),
        (
            "empty",
            "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\nepic_id = \"\"\n",
            "Invalid epic_id: empty.",
        ),
    ] {
        fs::write(dir.join(format!("{name}.toml")), text).unwrap();
        let output = f.run(&["validate", name]);
        assert!(!output.status.success());
        let shown = format!("{}{}", stdout(&output), stderr(&output));
        assert!(shown.contains(message), "{shown}");
    }
}

#[test]
fn set_refuses_epic_id_and_leaves_the_file_alone() {
    let f = Fixture::new();
    fowl(&f, None);
    let config = config_path(&f, "fowl");
    let before = stamp(&config);
    for args in [
        &["set", "fowl", "epic_id=Other"][..],
        &["set", "fowl", "--unset", "epic_id"],
        &["set", "fowl", "gamemode=true", "epic_id=Other"],
        &["set", "fowl", "gamemode=true", "--unset", "epic_id"],
    ] {
        assert_refused(&f.run(args), "epic_id is set by wineo import epic.");
        assert_eq!(stamp(&config), before, "{args:?}");
    }
    // The refusal is the key's, not the game's.
    environment(&f, "plain", None, Path::new("/bin/true"));
    assert_refused(
        &f.run(&["set", "plain", "--unset", "epic_id"]),
        "epic_id is set by wineo import epic.",
    );
}

#[test]
fn clones_and_portable_copies_drop_epic_id() {
    let f = Fixture::new();
    let folder = fowl(&f, None);
    let record = stamp(&game::record_path(&f.paths(), APP));
    let line = "  change: epic_id removed (the copy starts the game without Epic's login, so only a game that runs offline works from it)\n";

    let cloned = f.run(&["clone", "fowl", "copy"]);
    assert_success(&cloned);
    assert!(stdout(&cloned).contains(line), "{}", stdout(&cloned));
    let copy = f.created_config("copy");
    assert!(!copy.contains("epic_id"), "{copy}");
    // Still the same game folder, just not its owner.
    assert!(
        copy.contains(&folder.join("OfflineGame.exe").display().to_string()),
        "{copy}"
    );
    assert!(
        f.created_config("fowl")
            .contains("epic_id = \"AppOffline\"")
    );

    let dir = f.root.path().join("usb");
    let copied = f.run(&["portable", "copy", "fowl", "--to", dir.to_str().unwrap()]);
    assert_success(&copied);
    assert!(stdout(&copied).contains(line), "{}", stdout(&copied));
    let portable = fs::read_to_string(dir.join("config/environments/fowl.toml")).unwrap();
    assert!(!portable.contains("epic_id"), "{portable}");

    assert_eq!(stamp(&game::record_path(&f.paths(), APP)), record);
    assert!(
        validate_codes(&f, &[])
            .iter()
            .all(|(code, _)| code != "epic_id_taken")
    );
}

#[test]
fn trash_restore_is_refused_while_another_environment_has_the_game() {
    let f = Fixture::new();
    let folder = fowl(&f, None);
    let removed = f.run(&["rm", "fowl"]);
    assert_success(&removed);
    // rm leaves the game's record alone.
    assert!(game::peek(&f.paths(), APP).unwrap().is_some());
    assert_success(&f.run(&["restore", &trash_id(&removed)]));
    assert!(
        f.created_config("fowl")
            .contains("epic_id = \"AppOffline\"")
    );

    let removed = f.run(&["rm", "fowl"]);
    assert_success(&removed);
    environment(&f, "thief", Some(APP), &folder.join("OfflineGame.exe"));
    let refused = f.run(&["restore", &trash_id(&removed)]);
    assert_refused(
        &refused,
        "fowl's Epic game AppOffline belongs to thief now.",
    );
    assert!(f.created_config("fowl").is_empty());
    assert!(
        stdout(&f.run(&["trash", "list"])).contains("fowl"),
        "the entry stays in the trash"
    );

    fs::remove_file(config_path(&f, "thief")).unwrap();
    assert_success(&f.run(&["restore", &trash_id(&removed)]));
    assert!(f.created_config("fowl").contains("epic_id"));
}

#[test]
fn a_game_with_a_mark_refuses_to_start_and_says_what_finishes_it() {
    let f = Fixture::new();
    let folder = fowl(&f, Some(Work::Update));
    let message = "fowl's game folder is mid-update. Finish it with wineo update fowl.";
    assert_refused(&f.run(&["start", "fowl"]), message);
    assert!(!f.runtime("fowl").is_running().unwrap());
    // A copy running from the same folder is refused the same way.
    environment(&f, "copy", None, &folder.join("OfflineGame.exe"));
    assert_refused(&f.run(&["start", "copy"]), message);
    assert_refused(&f.run(&["exec", "fowl", "winecfg"]), message);

    record(&f, &folder, Some(Work::Install));
    assert_refused(
        &f.run(&["start", "fowl"]),
        "fowl's Epic install didn't finish. Finish it with wineo import epic --game AppOffline.",
    );
    assert_eq!(sweep(&f), Vec::<i32>::new());
}

#[test]
fn an_epic_id_without_a_record_refuses_to_start() {
    let f = Fixture::new();
    let folder = folder(&f);
    environment(&f, "fowl", Some(APP), &folder.join("OfflineGame.exe"));
    let message = "fowl's Epic record is missing. Repair it with wineo update fowl --repair.";
    assert_refused(&f.run(&["start", "fowl"]), message);
    assert_eq!(
        validate_codes(&f, &["fowl"])
            .into_iter()
            .filter(|(code, _)| code == "epic_record_missing")
            .collect::<Vec<_>>(),
        [("epic_record_missing".to_owned(), message.to_owned())]
    );
}

#[test]
fn a_running_game_holds_its_folder_and_leaves_the_record_alone() {
    let f = Fixture::new();
    let folder = fowl(&f, None);
    let record = game::record_path(&f.paths(), APP);
    let before = stamp(&record);
    assert_success(&f.run(&["start", "fowl"]));
    assert_eq!(
        update_refusal(&f, &folder).as_deref(),
        Some("fowl is running from this game folder. Stop it before updating fowl.")
    );
    assert_success(&f.run(&["stop", "fowl"]));
    f.wait_for_run_end("fowl", "stopped");
    assert_eq!(update_refusal(&f, &folder), None);
    assert_eq!(stamp(&record), before);
    assert_eq!(sweep(&f), Vec::<i32>::new());
}

#[test]
fn a_copy_running_from_the_folder_holds_it_too() {
    let f = Fixture::new();
    let folder = fowl(&f, None);
    environment(&f, "copy", None, &folder.join("OfflineGame.exe"));
    assert_success(&f.run(&["start", "copy"]));
    assert_eq!(
        update_refusal(&f, &folder).as_deref(),
        Some("copy is running from this game folder. Stop it before updating fowl.")
    );
    assert_success(&f.run(&["stop", "copy"]));
    f.wait_for_run_end("copy", "stopped");
    assert_eq!(update_refusal(&f, &folder), None);
    assert_eq!(sweep(&f), Vec::<i32>::new());
}

#[test]
fn a_start_during_work_is_refused_naming_the_work() {
    let f = Fixture::new();
    let folder = fowl(&f, Some(Work::Repair));
    let working = game::lock_for(&f.paths(), &folder, "fowl", Work::Repair).unwrap();
    assert_refused(
        &f.run(&["start", "fowl"]),
        "fowl's Epic game is being repaired.",
    );
    drop(working);
    assert!(game::lock_for(&f.paths(), &folder, "fowl", Work::Repair).is_ok());
    assert_eq!(sweep(&f), Vec::<i32>::new());
}

#[test]
fn validate_reports_epic_problems_without_writing_anything() {
    let f = Fixture::new();
    let folder = fowl(&f, Some(Work::Update));
    let record_file = game::record_path(&f.paths(), APP);
    let config = config_path(&f, "fowl");
    let (record_before, config_before) = (stamp(&record_file), stamp(&config));
    assert_eq!(
        validate_codes(&f, &[]),
        [
            (
                "epic_unfinished".to_owned(),
                "fowl's game folder is mid-update. Finish it with wineo update fowl.".to_owned()
            ),
            (
                "epic_legendary_missing".to_owned(),
                "Epic games need Legendary, and none was found. wineo offers to download it the next time Epic needs it, or set [epic] legendary.".to_owned()
            ),
        ]
    );
    let fixed = f.run(&["validate", "--fix"]);
    assert!(!fixed.status.success());
    assert_eq!(stamp(&record_file), record_before);
    assert_eq!(stamp(&config), config_before);

    // A game that isn't in Legendary's records any more.
    record(&f, &folder, None);
    let codes: Vec<String> = validate_codes(&f, &["fowl"])
        .into_iter()
        .map(|(code, _)| code)
        .collect();
    assert_eq!(codes, ["epic_record_mismatch", "epic_legendary_missing"]);
}
