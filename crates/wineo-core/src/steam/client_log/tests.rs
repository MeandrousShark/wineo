use super::*;
use crate::steam::{InstallKind, installs};
use std::fs;

const SILENCE: Duration = Duration::from_secs(120);

// Trimmed from a real bootstrap_log.txt.
const NORMAL_START: &str = "\
[2026-11-16 08:30:44] Startup - updater built Nov 13 2026 02:01:47
[2026-11-16 08:30:44] Startup - Steam Client launched with: '/home/user/.local/share/Steam/steamrt64/steam' '-srt-logger-opened' '-silent'
[2026-11-16 08:30:44] Loading cached metrics from disk (/home/user/.local/share/Steam/package/steam_client_metrics.bin)
[2026-11-16 08:30:44] Verifying installation...
[2026-11-16 08:30:44] Verifying file sizes only
[2026-11-16 08:30:44] Verification complete
[2026-11-16 08:30:44] Shutdown


[2026-11-16 08:30:46] Startup - updater built Nov 13 2026 02:01:47
[2026-11-16 08:30:46] Startup - Steam Client launched with: '/home/user/.local/share/Steam/steamrt64/steam' '-srt-logger-opened' '-silent'
[2026-11-16 08:30:46] Verifying installation...
[2026-11-16 08:30:46] Verifying file sizes only
[2026-11-16 08:30:46] Verification complete
[2026-11-16 08:32:47] Background update loop checking for update. . .
[2026-11-16 08:32:47] Checking for available updates...
[2026-11-16 08:32:47] Download skipped: /steam_client_steamrt64 version 1794536281, installed version 1794536281, existing pending version 0
[2026-11-16 08:32:47] Nothing to do
";

const STARTUP_CHECK: &str = "\
[2026-11-13 10:32:51] Startup - updater built Oct 21 2026 21:09:45
[2026-11-13 10:32:51] Startup - Steam Client launched with: '/home/user/.local/share/Steam/ubuntu12_32/steam' '-srt-logger-opened' '-silent'
[2026-11-13 10:32:51] Loading cached metrics from disk (/home/user/.local/share/Steam/package/steam_client_metrics.bin)
[2026-11-13 10:32:51] Checking for update on startup
[2026-11-13 10:32:51] Checking for available updates...
[2026-11-13 10:32:51] Downloaded new manifest: /steam_client_steamrt64 version 1794424036, installed version 1792618285, existing pending version 0
[2026-11-13 10:32:51] Package file bins_steamrt64.zip.vz.6f500642b86512dd16004d11400f5d2608e5dc47_42245419 missing or incorrect size
[2026-11-13 10:32:51] Add pending download: https://client-update.akamai.steamstatic.com/bins_steamrt64.zip.vz.6f500642b86512dd16004d11400f5d2608e5dc47_42245419
";

const STARTUP_DOWNLOAD: &str = "\
[2026-11-13 10:32:52] Downloading update (60,732 of 410,553 KB)...
[2026-11-13 10:32:52] Downloading update (112,547 of 410,553 KB)...
";

const STARTUP_INSTALL: &str = "\
[2026-11-13 10:32:55] Downloading update (410,553 of 410,553 KB)...
[2026-11-13 10:32:55] Download complete.
[2026-11-13 10:32:55] Saving metrics to disk (/home/user/.local/share/Steam/package/steam_client_metrics.bin)
[2026-11-13 10:32:55] uninstalled manifest found in /home/user/.local/share/Steam/package/steam_client_steamrt64 (1).
[2026-11-13 10:32:55] Found pending update
[2026-11-13 10:32:55] Installing update...
[2026-11-13 10:32:55] Extracting package...
";

const UPDATE_DONE: &str = "\
[2026-11-13 10:33:05] Installing update...
[2026-11-13 10:33:09] Cleaning up...
[2026-11-13 10:33:10] Update complete, launching Steam...
[2026-11-13 10:33:10] Shutdown
";

const AFTER_UPDATE: &str = "

[2026-11-13 10:33:10] Startup - updater built Nov  9 2026 22:38:13
[2026-11-13 10:33:10] Startup - Steam Client launched with: '/home/user/.local/share/Steam/steamrt64/steam' '-srt-logger-opened' '-silent'
[2026-11-13 10:33:10] Verifying installation...
";

const BACKGROUND_DOWNLOAD: &str = "\
[2026-11-13 10:33:11] Startup - updater built Nov  9 2026 22:38:13
[2026-11-13 10:33:11] Startup - Steam Client launched with: '/home/user/.local/share/Steam/steamrt64/steam' '-srt-logger-opened' '-silent'
[2026-11-13 10:33:11] Verifying installation...
[2026-11-13 10:33:11] Verifying file sizes only
[2026-11-13 10:33:11] Verification complete
[2026-11-13 18:11:49] Background update loop checking for update. . .
[2026-11-13 18:11:50] Downloaded new manifest: /steam_client_steamrt64 version 1794536281, installed version 1794424036, existing pending version 0
[2026-11-13 18:11:51] Downloading update (0 of 153,156 KB)...
[2026-11-13 18:11:51] Downloading update (98 of 153,156 KB)...
";

const PENDING_AT_START: &str = "\
[2026-11-14 21:28:00] Startup - updater built Nov  9 2026 22:38:13
[2026-11-14 21:28:00] Startup - Steam Client launched with: '/home/user/.local/share/Steam/steamrt64/steam' '-srt-logger-opened'
[2026-11-14 21:28:00] uninstalled manifest found in /home/user/.local/share/Steam/package/steam_client_steamrt64 (1).
[2026-11-14 21:28:00] Found pending update
[2026-11-14 21:28:00] Installing update...
[2026-11-14 21:28:00] Extracting package...
";

// Steam was killed while running, so this block has no Shutdown.
const KILLED: &str = "\
[2026-11-14 21:28:10] Startup - updater built Nov 13 2026 02:01:47
[2026-11-14 21:28:10] Startup - Steam Client launched with: '/home/user/.local/share/Steam/steamrt64/steam' '-srt-logger-opened'
[2026-11-14 21:28:10] Verifying installation...
[2026-11-14 21:28:10] Verifying file sizes only
[2026-11-14 21:28:10] Verification complete
[2026-11-14 21:30:11] Background update loop checking for update. . .
[2026-11-14 21:30:12] Nothing to do
";

fn phase(text: &str) -> Option<ClientUpdatePhase> {
    newest_block_update(text)
}

#[test]
fn a_startup_update_is_seen_through_each_phase() {
    let check = STARTUP_CHECK.to_owned();
    assert_eq!(phase(&check), Some(ClientUpdatePhase::Checking));
    let download = check + STARTUP_DOWNLOAD;
    assert_eq!(
        phase(&download),
        Some(ClientUpdatePhase::Downloading {
            done_kb: 112_547,
            total_kb: 410_553
        })
    );
    let install = download + STARTUP_INSTALL;
    assert_eq!(phase(&install), Some(ClientUpdatePhase::Installing));
}

#[test]
fn update_complete_and_the_next_block_are_not_updating() {
    let done = format!("{STARTUP_CHECK}{STARTUP_DOWNLOAD}{STARTUP_INSTALL}{UPDATE_DONE}");
    assert_eq!(phase(&done), None);
    // The verify-only block the updated client writes next.
    assert_eq!(phase(&format!("{done}{AFTER_UPDATE}")), None);
}

#[test]
fn each_line_of_a_normal_start_ends_an_update() {
    for line in [
        "Update complete, launching Steam...",
        "Verification complete",
        "Background update loop checking for update. . .",
        "Nothing to do",
        "Download skipped by HTTP 304 Not Modified",
        "Shutdown",
    ] {
        let ended = format!("{STARTUP_CHECK}[2026-11-13 10:33:10] {line}\n");
        assert_eq!(phase(&ended), None, "{line}");
        // A download after it is the background loop's, for the next start.
        assert_eq!(phase(&format!("{ended}{STARTUP_DOWNLOAD}")), None, "{line}");
    }
}

#[test]
fn a_pending_update_applied_at_start_is_updating() {
    let text = format!("{NORMAL_START}\n\n{PENDING_AT_START}");
    assert_eq!(phase(&text), Some(ClientUpdatePhase::Installing));
}

#[test]
fn a_background_download_after_a_normal_start_is_not_updating() {
    assert_eq!(phase(BACKGROUND_DOWNLOAD), None);
}

#[test]
fn normal_and_killed_blocks_are_not_updating() {
    assert_eq!(phase(NORMAL_START), None);
    assert_eq!(phase(KILLED), None);
    // Only the newest block counts, whatever an older one shows.
    assert_eq!(phase(&format!("{PENDING_AT_START}\n\n{KILLED}")), None);
}

#[test]
fn a_block_cut_mid_line_keeps_what_its_whole_lines_show() {
    let text =
        format!("{STARTUP_CHECK}{STARTUP_DOWNLOAD}[2026-11-13 10:32:53] Downloading update (165,6");
    assert_eq!(
        phase(&text),
        Some(ClientUpdatePhase::Downloading {
            done_kb: 112_547,
            total_kb: 410_553
        })
    );
    // A cut before any progress shows the phase so far.
    assert_eq!(
        phase(&format!(
            "{STARTUP_CHECK}[2026-11-13 10:32:52] Downloading upd"
        )),
        Some(ClientUpdatePhase::Checking)
    );
}

#[test]
fn garbage_and_empty_text_are_not_updating() {
    assert_eq!(phase(""), None);
    assert_eq!(
        phase("\u{0}\u{1}not a log\nDownloading update (1 of 2 KB)...\n"),
        None
    );
    assert_eq!(
        phase("Startup - updater built\nDownloading update (x of y KB)...\n"),
        None
    );
}

fn native_home() -> (tempfile::TempDir, Install) {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".local/share/Steam/logs")).unwrap();
    let install = installs(home.path()).remove(0);
    assert_eq!(install.kind(), InstallKind::Native);
    (home, install)
}

fn write_log(install: &Install, text: &[u8]) {
    fs::write(install.root().join("logs/bootstrap_log.txt"), text).unwrap();
}

#[test]
fn a_missing_or_empty_log_is_not_updating() {
    let (_home, install) = native_home();
    assert_eq!(client_update(&install, SILENCE), None);
    write_log(&install, b"");
    assert_eq!(client_update(&install, SILENCE), None);
    write_log(&install, &[0xff, 0xfe, b'\n', 0x80]);
    assert_eq!(client_update(&install, SILENCE), None);
}

#[test]
fn a_log_left_unwritten_for_the_silence_limit_shows_no_update() {
    let (_home, install) = native_home();
    write_log(&install, PENDING_AT_START.as_bytes());
    let log = install.root().join("logs/bootstrap_log.txt");
    let set_age = |age: Duration| {
        fs::File::options()
            .write(true)
            .open(&log)
            .unwrap()
            .set_modified(SystemTime::now() - age)
            .unwrap();
    };
    set_age(SILENCE - Duration::from_secs(5));
    assert!(client_update(&install, SILENCE).is_some());
    set_age(SILENCE + Duration::from_secs(1));
    assert_eq!(client_update(&install, SILENCE), None);
}

#[test]
fn a_log_written_in_the_future_counts_as_just_written() {
    let now = SystemTime::now();
    let ahead = now + Duration::from_secs(3600);
    assert!(written_within(ahead, now, SILENCE));
    assert!(written_within(now - Duration::from_secs(119), now, SILENCE));
    assert!(!written_within(now - SILENCE, now, SILENCE));
}

#[test]
fn an_oversized_log_is_read_from_its_end() {
    let (_home, install) = native_home();
    let mut text = String::new();
    while text.len() < 3 * TAIL as usize {
        text.push_str(NORMAL_START);
        text.push_str("\n\n");
    }
    text.push_str(STARTUP_CHECK);
    text.push_str(STARTUP_DOWNLOAD);
    write_log(&install, text.as_bytes());
    assert!(matches!(
        client_update(&install, SILENCE).map(|update| update.phase),
        Some(ClientUpdatePhase::Downloading { .. })
    ));
    // A block whose start fell out of the tail can't be judged.
    let mut text = STARTUP_CHECK.to_owned();
    let line = "[2026-11-13 10:32:52] Downloading update (60,732 of 410,553 KB)...\n";
    while text.len() < 2 * TAIL as usize {
        text.push_str(line);
    }
    write_log(&install, text.as_bytes());
    assert_eq!(client_update(&install, SILENCE), None);
}

#[test]
fn a_cut_on_a_line_boundary_keeps_the_block_start() {
    let (_home, install) = native_home();
    // The block runs exactly `TAIL` bytes, so the tail starts on its first line.
    let mut block = PENDING_AT_START.to_owned();
    let pad = TAIL as usize - block.len() - 1;
    block.push_str(&"x".repeat(pad));
    block.push('\n');
    assert_eq!(block.len() as u64, TAIL);
    let text = format!("{NORMAL_START}\n\n{block}");
    write_log(&install, text.as_bytes());
    assert_eq!(
        client_update(&install, SILENCE).map(|update| update.phase),
        Some(ClientUpdatePhase::Installing)
    );
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
    write_log(&install, PENDING_AT_START.as_bytes());
    assert_eq!(
        client_update(&install, SILENCE).map(|update| update.phase),
        Some(ClientUpdatePhase::Installing)
    );
}
