use super::*;
use crate::steam::{InstallKind, installs};
use std::fs;
use std::io::Write;

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

#[test]
fn a_real_launch_is_a_pass_only_while_it_processes_shaders() {
    let (_home, install) = native_home();
    let nothing = console_mark(&install);
    for end in 0..=LAUNCH.len() {
        write_log(&install, &joined(&LAUNCH[..end]));
        assert_eq!(
            shader_pass(&install, &nothing, APP),
            end == SHADER_LINE + 1,
            "after {end} lines"
        );
    }
}

#[test]
fn another_apps_pass_does_not_count_and_does_not_end_ours() {
    let (_home, install) = native_home();
    let mark = console_mark(&install);
    write_log(&install, &line("1245620", 1, "ProcessingShaderCache"));
    assert!(!shader_pass(&install, &mark, APP));
    // The id must match whole, not by prefix.
    write_log(&install, &line("4275200", 1, "ProcessingShaderCache"));
    assert!(!shader_pass(&install, &mark, APP));
    write_log(&install, &line("42752", 1, "ProcessingShaderCache"));
    assert!(!shader_pass(&install, &mark, APP));
    let text = line(APP, 1, "ProcessingShaderCache") + &line("1245620", 1, "CreatingProcess");
    write_log(&install, &text);
    assert!(shader_pass(&install, &mark, APP));
}

#[test]
fn the_newest_line_wins_whatever_its_action_id() {
    let (_home, install) = native_home();
    let mark = console_mark(&install);
    let text = line(APP, 1, "ProcessingShaderCache") + &line(APP, 2, "CheckShaderDepotManifest");
    write_log(&install, &text);
    assert!(!shader_pass(&install, &mark, APP));
    // Another launch's pass is newer than the first one's end.
    let text = line(APP, 1, "Completed") + &line(APP, 2, "ProcessingShaderCache");
    write_log(&install, &text);
    assert!(shader_pass(&install, &mark, APP));
    // And an older action's late line doesn't hide the newest by number.
    let text = line(APP, 2, "ProcessingShaderCache") + &line(APP, 1, "Completed");
    write_log(&install, &text);
    assert!(!shader_pass(&install, &mark, APP));
}

#[test]
fn crlf_line_endings_read_the_same() {
    let (_home, install) = native_home();
    let mark = console_mark(&install);
    let text: String = LAUNCH[..=SHADER_LINE]
        .iter()
        .map(|line| format!("{line}\r\n"))
        .collect();
    write_log(&install, &text);
    assert!(shader_pass(&install, &mark, APP));
    append_log(&install, &format!("{}\r\n", LAUNCH[SHADER_LINE + 1]));
    assert!(!shader_pass(&install, &mark, APP));
}

#[test]
fn lines_before_the_mark_do_not_count() {
    let (_home, install) = native_home();
    // A pass an earlier launch never finished.
    write_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    let mark = console_mark(&install);
    assert!(!shader_pass(&install, &mark, APP));
    append_log(&install, &line(APP, 2, "CheckShaderDepotManifest"));
    assert!(!shader_pass(&install, &mark, APP));
    append_log(&install, &line(APP, 2, "ProcessingShaderCache"));
    assert!(shader_pass(&install, &mark, APP));
}

#[test]
fn a_mark_that_cuts_a_line_ignores_its_rest() {
    let (_home, install) = native_home();
    let text = line(APP, 1, "ProcessingShaderCache");
    write_log(&install, &text[..text.len() - 10]);
    let mark = console_mark(&install);
    append_log(&install, &text[text.len() - 10..]);
    assert!(!shader_pass(&install, &mark, APP));
}

#[test]
fn a_rotated_log_is_read_from_its_start() {
    let (_home, install) = native_home();
    write_log(&install, &joined(&LAUNCH));
    let mark = console_mark(&install);
    // The new file is longer than the old one, so only its identity differs.
    let mut fresh = line(APP, 1, "ProcessingShaderCache");
    while fresh.len() as u64 <= mark.len {
        fresh.push_str(&line("1245620", 1, "Completed"));
    }
    rotate_log(&install, &fresh);
    assert!(shader_pass(&install, &mark, APP));
}

#[test]
fn a_shrunk_log_is_read_from_its_start() {
    let (_home, install) = native_home();
    write_log(&install, &joined(&LAUNCH));
    let mark = console_mark(&install);
    write_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(shader_pass(&install, &mark, APP));
}

#[test]
fn a_log_that_appears_after_the_mark_is_read_from_its_start() {
    let (_home, install) = native_home();
    let mark = console_mark(&install);
    write_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(shader_pass(&install, &mark, APP));
}

#[test]
fn a_log_that_disappears_is_no_pass() {
    let (_home, install) = native_home();
    write_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    let mark = console_mark(&install);
    assert!(!shader_pass(&install, &mark, APP));
    append_log(&install, &line(APP, 2, "ProcessingShaderCache"));
    assert!(shader_pass(&install, &mark, APP));
    fs::remove_file(log_path(&install)).unwrap();
    assert!(!shader_pass(&install, &mark, APP));
}

#[test]
fn a_missing_or_garbled_log_is_no_pass() {
    let (_home, install) = native_home();
    let mark = console_mark(&install);
    assert!(!shader_pass(&install, &mark, APP));
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
        assert!(!shader_pass(&install, &mark, APP), "{text:?}");
    }
}

#[test]
fn a_line_still_being_written_is_not_read() {
    let (_home, install) = native_home();
    let mark = console_mark(&install);
    let text = line(APP, 1, "ProcessingShaderCache")
        + "[2026-10-30 12:00:03] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to Si";
    write_log(&install, &text);
    assert!(shader_pass(&install, &mark, APP));
    // With no whole line yet, nothing has been decided.
    let first = line(APP, 1, "ProcessingShaderCache");
    write_log(&install, &first[..first.len() - 5]);
    assert!(!shader_pass(&install, &mark, APP));
}

#[test]
fn a_long_stretch_after_the_mark_keeps_its_newest_lines() {
    let (_home, install) = native_home();
    write_log(&install, "");
    let mark = console_mark(&install);
    let noise = line("1245620", 1, "Completed");
    let mut text = String::new();
    while text.len() < 2 * TAIL_BYTES {
        text.push_str(&noise);
    }
    text.push_str(&line(APP, 1, "ProcessingShaderCache"));
    write_log(&install, &text);
    assert!(shader_pass(&install, &mark, APP));
}

const TAIL_BYTES: usize = client_log::TAIL as usize;

#[test]
fn a_flatpak_install_reads_its_own_log() {
    let home = tempfile::tempdir().unwrap();
    let root = home
        .path()
        .join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
    fs::create_dir_all(root.join("logs")).unwrap();
    let install = installs(home.path()).remove(0);
    assert_eq!(install.kind(), InstallKind::Flatpak);
    let mark = console_mark(&install);
    write_log(&install, &line(APP, 1, "ProcessingShaderCache"));
    assert!(shader_pass(&install, &mark, APP));
}
