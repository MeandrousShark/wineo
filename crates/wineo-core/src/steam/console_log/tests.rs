use super::*;
use crate::steam::{InstallKind, installs};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

const APP: &str = "427520";

// A real launch, with the trailing `with ""` as Steam writes it.
const LAUNCH: [&str; 12] = [
    r#"[2026-10-30 11:11:45] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to CheckShaderDepotManifest with """#,
    r#"[2026-10-30 11:11:46] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to RunningInstallScript with """#,
    r#"[2026-10-30 11:11:46] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to SynchronizingCloud with """#,
    r#"[2026-10-30 11:11:46] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to SynchronizingStats with """#,
    r#"[2026-10-30 11:11:46] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to ShowInterstitials with """#,
    r#"[2026-10-30 11:11:47] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to SynchronizingControllerConfig with """#,
    r#"[2026-10-30 11:11:47] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to ProcessingShaderCache with """#,
    r#"[2026-10-30 11:11:50] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to SiteLicenseSeatCheckout with """#,
    r#"[2026-10-30 11:11:50] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to DelayLaunch with """#,
    r#"[2026-10-30 11:11:50] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to CreatingProcess with """#,
    r#"[2026-10-30 11:11:50] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to WaitingGameWindow with """#,
    r#"[2026-10-30 11:11:50] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to Completed with """#,
];
const SHADER_LINE: usize = 6;

fn line(app: &str, action: u32, task: &str) -> String {
    format!(
        "[2026-10-30 12:00:00] GameAction [AppID {app}, ActionID {action}] : LaunchApp changed task to {task} with \"\"\n"
    )
}

fn native_home() -> (tempfile::TempDir, Install) {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".local/share/Steam/logs")).unwrap();
    let install = installs(home.path()).remove(0);
    assert_eq!(install.kind(), InstallKind::Native);
    (home, install)
}

fn log_path(install: &Install) -> PathBuf {
    install.root().join("logs/console_log.txt")
}

fn write_log(install: &Install, text: &str) {
    fs::write(log_path(install), text).unwrap();
}

fn append_log(install: &Install, text: &str) {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(install))
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

/// Steam writes the new log aside and renames it over the old one.
fn rotate_log(install: &Install, text: &str) {
    let fresh = install.root().join("logs/console_log.new");
    fs::write(&fresh, text).unwrap();
    fs::rename(fresh, log_path(install)).unwrap();
}

fn joined(lines: &[&str]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn watch(install: &Install) -> ShaderWatch {
    ShaderWatch::start(install, APP)
}

fn noise(bytes: usize) -> String {
    let unit = line("1245620", 1, "Completed");
    unit.repeat(bytes / unit.len() + 1)
}

#[test]
fn a_real_launch_is_a_pass_only_while_it_processes_shaders() {
    let (_home, install) = native_home();
    let mut watch = watch(&install);
    for (index, line) in LAUNCH.iter().enumerate() {
        append_log(&install, &format!("{line}\n"));
        assert_eq!(watch.processing(), index == SHADER_LINE, "line {index}");
    }
}

#[test]
fn another_apps_pass_does_not_count_and_does_not_end_ours() {
    let (_home, install) = native_home();
    let mut watch = watch(&install);
    for app in ["1245620", "4275200", "42752"] {
        append_log(&install, &line(app, 1, "ProcessingShaderCache"));
        assert!(!watch.processing(), "{app}");
    }
    append_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(watch.processing());
    append_log(&install, &line("1245620", 1, "CreatingProcess"));
    assert!(watch.processing());
}

#[test]
fn the_newest_line_wins_whatever_its_action_id() {
    let (_home, install) = native_home();
    let mut watch = watch(&install);
    append_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(watch.processing());
    append_log(&install, &line(APP, 2, "CheckShaderDepotManifest"));
    assert!(!watch.processing());
    append_log(&install, &line(APP, 2, "ProcessingShaderCache"));
    assert!(watch.processing());
    // An older action's late line is still the newest in the file.
    append_log(&install, &line(APP, 1, "Completed"));
    assert!(!watch.processing());
}

#[test]
fn crlf_line_endings_read_the_same() {
    let (_home, install) = native_home();
    let mut watch = watch(&install);
    let text: String = LAUNCH[..=SHADER_LINE]
        .iter()
        .map(|line| format!("{line}\r\n"))
        .collect();
    append_log(&install, &text);
    assert!(watch.processing());
    append_log(&install, &format!("{}\r\n", LAUNCH[SHADER_LINE + 1]));
    assert!(!watch.processing());
}

#[test]
fn lines_before_the_start_do_not_count() {
    let (_home, install) = native_home();
    // A pass an earlier launch never finished.
    write_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    let mut watch = watch(&install);
    assert!(!watch.processing());
    append_log(&install, &line(APP, 2, "CheckShaderDepotManifest"));
    assert!(!watch.processing());
    append_log(&install, &line(APP, 2, "ProcessingShaderCache"));
    assert!(watch.processing());
}

#[test]
fn a_start_on_a_line_boundary_keeps_the_next_line() {
    let (_home, install) = native_home();
    write_log(&install, &line("1245620", 1, "Completed"));
    let mut watch = watch(&install);
    append_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(watch.processing());
}

#[test]
fn a_start_that_cuts_a_line_ignores_the_rest_of_it() {
    let (_home, install) = native_home();
    let text = line(APP, 1, "ProcessingShaderCache");
    // Every cut but the two ends, including right after the time stamp.
    for cut in 1..text.len() {
        write_log(&install, &text[..cut]);
        let mut watch = watch(&install);
        append_log(&install, &text[cut..]);
        assert!(!watch.processing(), "cut at {cut}");
    }
    // The line after the cut one is whole.
    write_log(&install, &text[..40]);
    let mut watch = watch(&install);
    append_log(&install, &text[40..]);
    append_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(watch.processing());
}

#[test]
fn a_rotated_log_is_read_from_its_start() {
    let (_home, install) = native_home();
    write_log(&install, &joined(&LAUNCH));
    let mut watch = watch(&install);
    let old_len = fs::metadata(log_path(&install)).unwrap().len();
    // The new file is longer than the old one, so only its identity differs.
    let mut fresh = line(APP, 1, "ProcessingShaderCache");
    while (fresh.len() as u64) <= old_len {
        fresh.push_str(&line("1245620", 1, "Completed"));
    }
    rotate_log(&install, &fresh);
    assert!(watch.processing());
}

#[test]
fn a_rotation_is_not_forgotten_once_the_new_log_outgrows_the_old_one() {
    let (_home, install) = native_home();
    write_log(&install, &joined(&LAUNCH));
    let mut watch = watch(&install);
    let old_len = fs::metadata(log_path(&install)).unwrap().len();
    rotate_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(watch.processing());
    append_log(&install, &noise(old_len as usize * 2));
    assert!(watch.processing());
}

#[test]
fn a_truncated_log_is_read_from_its_start_and_stays_followed() {
    let (_home, install) = native_home();
    write_log(&install, &joined(&LAUNCH));
    let mut watch = watch(&install);
    let old_len = fs::metadata(log_path(&install)).unwrap().len();
    append_log(&install, &line("1245620", 1, "Completed"));
    assert!(!watch.processing());
    // Cut in place, so the identity stays.
    write_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(watch.processing());
    append_log(&install, &noise(old_len as usize * 2));
    assert!(watch.processing());
}

#[test]
fn a_log_that_appears_after_the_start_is_read_from_its_start() {
    let (_home, install) = native_home();
    let mut watch = watch(&install);
    assert!(!watch.processing());
    write_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(watch.processing());
}

#[test]
fn a_log_that_disappears_is_no_pass_and_a_new_one_is_read_whole() {
    let (_home, install) = native_home();
    let mut watch = watch(&install);
    write_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(watch.processing());
    fs::remove_file(log_path(&install)).unwrap();
    assert!(!watch.processing());
    write_log(&install, &line(APP, 1, "CheckShaderDepotManifest"));
    assert!(!watch.processing());
    append_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(watch.processing());
}

#[test]
fn a_missing_or_garbled_log_is_no_pass() {
    let (_home, install) = native_home();
    let mut watch = watch(&install);
    assert!(!watch.processing());
    for text in [
        &b""[..],
        &[0xff, 0xfe, b'\n', 0x80],
        b"\n\n\n",
        b"GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to\n",
        b"GameAction [AppID 427520, ActionID x] : LaunchApp changed task to ProcessingShaderCache\n",
        b"GameAction [AppID 427520] : LaunchApp changed task to ProcessingShaderCache\n",
        b"GameAction [AppID 427520, ActionID 1] : AppUpdate changed task to ProcessingShaderCache\n",
        b"ProcessingShaderCache\n",
    ] {
        fs::write(log_path(&install), text).unwrap();
        let mut watch = watch_of(&install);
        assert!(!watch.processing(), "{text:?}");
    }
}

fn watch_of(install: &Install) -> ShaderWatch {
    // Started before the bytes, so they all count if they parse.
    let path = log_path(install);
    let text = fs::read(&path).unwrap();
    fs::write(&path, "").unwrap();
    let watch = watch(install);
    fs::write(&path, text).unwrap();
    watch
}

#[test]
fn a_log_that_cannot_be_read_is_no_pass() {
    let (_home, install) = native_home();
    fs::create_dir(log_path(&install)).unwrap();
    let mut watch = watch(&install);
    assert!(!watch.processing());
}

#[test]
fn a_line_still_being_written_is_not_read_until_it_is_whole() {
    let (_home, install) = native_home();
    let mut watch = watch(&install);
    let text = line(APP, 1, "ProcessingShaderCache");
    append_log(&install, &text[..text.len() - 5]);
    assert!(!watch.processing());
    append_log(&install, &text[text.len() - 5..]);
    assert!(watch.processing());
    // A half-written next line doesn't end the pass yet.
    let next = line(APP, 1, "SiteLicenseSeatCheckout");
    append_log(&install, &next[..next.len() - 5]);
    assert!(watch.processing());
    append_log(&install, &next[next.len() - 5..]);
    assert!(!watch.processing());
}

#[test]
fn a_pass_line_followed_by_more_than_the_tail_of_other_output_still_counts() {
    let (_home, install) = native_home();
    let mut watch = watch(&install);
    let text = line(APP, 1, "ProcessingShaderCache") + &noise(3 * 512 * 1024);
    append_log(&install, &text);
    assert!(watch.processing());
    // And across several looks of a noisy pass.
    for _ in 0..3 {
        append_log(&install, &noise(600 * 1024));
        assert!(watch.processing());
    }
}

#[test]
fn a_flatpak_install_reads_its_own_log() {
    let home = tempfile::tempdir().unwrap();
    let root = home
        .path()
        .join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
    fs::create_dir_all(root.join("logs")).unwrap();
    let install = installs(home.path()).remove(0);
    assert_eq!(install.kind(), InstallKind::Flatpak);
    let mut watch = watch(&install);
    write_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(watch.processing());
}
