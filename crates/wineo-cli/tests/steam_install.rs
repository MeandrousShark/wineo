mod common;

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use common::steam::SteamFixture;
use common::{assert_success, stderr, stdout, wait_for};
use rustix::fs::OFlags;
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

const URL_INSTALL: &str = "steam://install/289070";
const URL_UNINSTALL: &str = "steam://uninstall/289070";

fn manifest_path(s: &SteamFixture) -> std::path::PathBuf {
    s.root.join("steamapps/appmanifest_289070.acf")
}

/// Writes the game's manifest the way Steam does, replacing it in one step.
fn manifest(s: &SteamFixture, flags: u32, to_download: u64, downloaded: u64) {
    let steamapps = s.root.join("steamapps");
    let temporary = steamapps.join("appmanifest_289070.tmp");
    fs::write(
        &temporary,
        format!(
            "\"AppState\"\n{{\n\"appid\" \"289070\"\n\"name\" \"Civilization VI\"\n\"StateFlags\" \"{flags}\"\n\"BytesToDownload\" \"{to_download}\"\n\"BytesDownloaded\" \"{downloaded}\"\n}}\n"
        ),
    )
    .unwrap();
    fs::rename(temporary, manifest_path(s)).unwrap();
}

fn not_installed(s: &SteamFixture) {
    fs::remove_file(manifest_path(s)).unwrap();
}

/// Gives the handoff, which runs in a process of its own, time to show up.
fn settle() {
    thread::sleep(Duration::from_millis(300));
}

fn signal(pid: u32, kind: &str) {
    assert!(
        Command::new("kill")
            .args([kind, &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn install_opens_steams_dialog_for_a_game_that_is_not_there_or_queued() {
    for queued in [false, true] {
        let s = SteamFixture::new();
        s.config("");
        if queued {
            manifest(&s, 2, 0, 0);
        } else {
            not_installed(&s);
        }

        let output = s.run(&["install", "civ"]);

        assert_success(&output);
        assert_eq!(stdout(&output), "Asked Steam to install civ.\n");
        wait_for("the install URL", || s.urls() == [URL_INSTALL]);
    }
}

#[test]
fn install_and_uninstall_say_when_there_is_nothing_to_ask() {
    let s = SteamFixture::new();
    s.config("");
    let installed = s.run(&["install", "civ"]);
    assert_success(&installed);
    assert_eq!(stdout(&installed), "Already installed.\n");

    manifest(&s, 1026, 100, 10);
    let installing = s.run(&["install", "civ"]);
    assert_success(&installing);
    assert_eq!(stdout(&installing), "Steam is already installing civ.\n");

    not_installed(&s);
    let gone = s.run(&["uninstall", "civ"]);
    assert_success(&gone);
    assert_eq!(stdout(&gone), "Not installed.\n");

    // With -f a no-op that is nothing to wait for ends right away.
    manifest(&s, 4, 0, 0);
    let followed = s.run(&["install", "civ", "-f"]);
    assert_success(&followed);
    assert_eq!(stdout(&followed), "Already installed.\n");
    settle();
    assert_eq!(s.urls(), Vec::<String>::new());
}

#[test]
fn refusals_exit_one_with_the_reason() {
    let s = SteamFixture::new();
    s.config("");
    s.f.environment("notepad", "");
    let wine = s.run(&["install", "notepad"]);
    assert_eq!(wine.status.code(), Some(1));
    assert_eq!(stderr(&wine), "error: Only Steam games can be installed.\n");
    let wine = s.run(&["uninstall", "notepad"]);
    assert_eq!(wine.status.code(), Some(1));
    assert_eq!(
        stderr(&wine),
        "error: Only Steam games can be uninstalled.\n"
    );

    manifest(&s, 4 | 2048, 0, 0);
    let output = s.run(&["install", "civ"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "error: Steam is uninstalling civ.\n");

    let missing = s.run(&["install", "nope"]);
    assert_eq!(missing.status.code(), Some(1));
    settle();
    assert_eq!(s.urls(), Vec::<String>::new());
}

#[test]
fn uninstall_opens_steams_dialog_for_installed_and_partial_games() {
    for flags in [4, 1026, 2] {
        let s = SteamFixture::new();
        s.config("");
        manifest(&s, flags, 0, 0);

        let output = s.run(&["uninstall", "civ"]);

        assert_success(&output);
        assert_eq!(stdout(&output), "Asked Steam to uninstall civ.\n");
        wait_for("the uninstall URL", || s.urls() == [URL_UNINSTALL]);
    }
}

#[test]
fn uninstall_refuses_a_running_game_started_by_wineo_or_by_steam() {
    let s = SteamFixture::new();
    s.config("");
    assert_success(&s.run(&["start", "civ"]));
    wait_for("the game to run", || {
        stdout(&s.run(&["status", "civ"])).contains("civ: running")
    });
    let output = s.run(&["uninstall", "civ"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "error: Stop civ before uninstalling it.\n");
    assert_success(&s.run(&["stop", "civ"]));
    s.f.wait_for_run_end("civ", "stopped");

    s.launch_from_steam("289070");
    wait_for("the Steam started game", || {
        stdout(&s.run(&["status", "civ"])).contains("started from Steam")
    });
    let output = s.run(&["uninstall", "civ"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "error: Stop civ before uninstalling it.\n");
    assert_eq!(s.urls(), Vec::<String>::new());
}

/// A followed command with its output read a line at a time.
struct Followed {
    child: Child,
    lines: Receiver<String>,
}

impl Followed {
    fn spawn(s: &SteamFixture, args: &[&str]) -> Self {
        let mut child = s
            .wineo()
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (send, lines) = channel();
        let stdout = child.stdout.take().unwrap();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self { child, lines }
    }

    fn line(&self) -> String {
        match self.lines.recv_timeout(Duration::from_secs(20)) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for a line"),
            Err(RecvTimeoutError::Disconnected) => panic!("the output ended"),
        }
    }

    fn expect(&self, line: &str) {
        assert_eq!(self.line(), line);
    }

    /// Waits for the exit, and returns the rest of stdout and all of stderr.
    fn finish(mut self) -> (ExitStatus, Vec<String>, String) {
        let status = self.child.wait().unwrap();
        let rest: Vec<String> = self.lines.try_iter().collect();
        let mut err = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        (status, rest, err)
    }
}

#[test]
fn following_an_install_prints_one_line_per_change_until_it_finishes() {
    let s = SteamFixture::new();
    s.config("");
    not_installed(&s);
    let run = Followed::spawn(&s, &["install", "civ", "--follow"]);

    run.expect("Waiting for Steam's install dialog...");
    wait_for("the install URL", || s.urls() == [URL_INSTALL]);
    manifest(&s, 2, 0, 0);
    run.expect("Install queued for Civilization VI...");
    // Steam hasn't written totals yet, so there is no percent.
    manifest(&s, 1026, 0, 0);
    run.expect("Installing Civilization VI...");
    manifest(&s, 1026, 1000, 0);
    run.expect("Installing Civilization VI... 0% (0 B of 1000 B)");
    manifest(&s, 1026, 1000, 500);
    run.expect("Installing Civilization VI... 50% (500 B of 1000 B)");
    // A pause drops the counts to zero, and following goes on without a percent.
    manifest(&s, 1026, 0, 0);
    run.expect("Installing Civilization VI...");
    manifest(&s, 1026, 1000, 750);
    run.expect("Installing Civilization VI... 75% (750 B of 1000 B)");
    manifest(&s, 4, 0, 0);
    run.expect("Installed.");

    let (status, rest, err) = run.finish();
    assert_eq!(status.code(), Some(0), "{err}");
    assert_eq!(rest, Vec::<String>::new());
    assert_eq!(err, "");
}

#[test]
fn following_an_install_that_steam_drops_exits_one() {
    let s = SteamFixture::new();
    s.config("");
    not_installed(&s);
    let run = Followed::spawn(&s, &["install", "civ", "-f"]);
    run.expect("Waiting for Steam's install dialog...");
    manifest(&s, 1026, 1000, 100);
    run.expect("Installing Civilization VI... 10% (100 B of 1000 B)");

    not_installed(&s);

    let (status, rest, err) = run.finish();
    assert_eq!(status.code(), Some(1));
    assert_eq!(rest, Vec::<String>::new());
    assert_eq!(err, "Steam canceled the install.\n");
}

#[test]
fn following_an_install_already_under_way_picks_it_up() {
    let s = SteamFixture::new();
    s.config("");
    manifest(&s, 1026, 1000, 250);
    let run = Followed::spawn(&s, &["install", "civ", "-f"]);

    run.expect("Steam is already installing civ.");
    run.expect("Installing Civilization VI... 25% (250 B of 1000 B)");
    manifest(&s, 4, 0, 0);
    run.expect("Installed.");

    let (status, _, err) = run.finish();
    assert_eq!(status.code(), Some(0), "{err}");
    settle();
    assert_eq!(s.urls(), Vec::<String>::new(), "no second dialog");
}

#[test]
fn following_an_uninstall_ends_when_the_manifest_is_gone() {
    let s = SteamFixture::new();
    s.config("");
    let run = Followed::spawn(&s, &["uninstall", "civ", "-f"]);

    run.expect("Uninstalling Civilization VI...");
    wait_for("the uninstall URL", || s.urls() == [URL_UNINSTALL]);
    not_installed(&s);
    run.expect("Uninstalled.");

    let (status, rest, err) = run.finish();
    assert_eq!(status.code(), Some(0), "{err}");
    assert_eq!(rest, Vec::<String>::new());
}

#[test]
fn following_an_uninstall_that_steam_backs_out_of_exits_one() {
    let s = SteamFixture::new();
    s.config("");
    let run = Followed::spawn(&s, &["uninstall", "civ", "-f"]);
    run.expect("Uninstalling Civilization VI...");

    // Steam starts removing it, then the player cancels.
    manifest(&s, 4 | 2048, 0, 0);
    thread::sleep(Duration::from_millis(600));
    manifest(&s, 4, 0, 0);

    let (status, rest, err) = run.finish();
    assert_eq!(status.code(), Some(1));
    assert_eq!(rest, Vec::<String>::new());
    assert_eq!(err, "Steam canceled the uninstall.\n");
}

#[test]
fn ctrl_c_while_following_exits_130_and_leaves_steam_alone() {
    let s = SteamFixture::new();
    s.config("");
    not_installed(&s);
    let run = Followed::spawn(&s, &["install", "civ", "-f"]);
    run.expect("Waiting for Steam's install dialog...");
    manifest(&s, 1026, 1000, 100);
    run.expect("Installing Civilization VI... 10% (100 B of 1000 B)");

    signal(run.child.id(), "-INT");

    let (status, rest, err) = run.finish();
    assert_eq!(status.code(), Some(130), "{err}");
    assert_eq!(rest, Vec::<String>::new());
    assert_eq!(err, "");
    // The install carries on: the manifest is untouched and Steam wasn't
    // asked to stop.
    assert!(manifest_path(&s).is_file());
    wait_for("the install URL", || s.urls() == [URL_INSTALL]);
}

/// Output on a terminal, as the bytes sent.
struct Terminal {
    child: Child,
    seen: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
}

impl Terminal {
    fn spawn(mut command: Command) -> Self {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(OFlags::NOCTTY.bits() as i32)
            .open(name.to_str().unwrap())
            .unwrap();
        command
            .stdin(Stdio::null())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave);
        let child = command.spawn().unwrap();
        drop(command);
        let mut output = File::from(master);
        let seen = Arc::new(Mutex::new(String::new()));
        let collected = seen.clone();
        let reader = thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(read) = output.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                collected
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buffer[..read]));
            }
        });
        Self {
            child,
            seen,
            reader: Some(reader),
        }
    }

    fn expect(&self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.seen.lock().unwrap().contains(text) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {text:?} in {:?}",
                self.seen.lock().unwrap()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn finish(mut self) -> (ExitStatus, String) {
        let status = self.child.wait().unwrap();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let output = self.seen.lock().unwrap().clone();
        (status, output)
    }
}

#[test]
fn on_a_terminal_progress_is_redrawn_in_place() {
    let s = SteamFixture::new();
    s.config("");
    manifest(&s, 1026, 1000, 100);
    let mut command = s.wineo();
    command.args(["install", "civ", "-f"]);
    let run = Terminal::spawn(command);

    run.expect("10% (100 B of 1000 B)");
    manifest(&s, 1026, 1000, 600);
    run.expect("60% (600 B of 1000 B)");
    manifest(&s, 4, 0, 0);
    let (status, output) = run.finish();

    assert_eq!(status.code(), Some(0), "{output:?}");
    // Each draw returns to the line start and clears it, and the last line is
    // ended before the result.
    assert!(
        output.contains("\r\x1b[KInstalling Civilization VI... 10% (100 B of 1000 B)"),
        "{output:?}"
    );
    assert!(
        output.contains("\r\x1b[KInstalling Civilization VI... 60% (600 B of 1000 B)"),
        "{output:?}"
    );
    assert!(
        output.ends_with("(600 B of 1000 B)\r\nInstalled.\r\n"),
        "{output:?}"
    );
}

#[test]
fn generated_pages_and_completions_include_the_new_commands_and_flags() {
    let f = common::Fixture::new();
    let pages = f.root.path().join("man");
    assert_success(
        &f.wineo()
            .arg("manpages")
            .arg(&pages)
            .output()
            .expect("wineo should start"),
    );
    for (page, words) in [
        ("wineo-install.1", &["follow"][..]),
        ("wineo-uninstall.1", &["follow"]),
        (
            "wineo-import-steam.1",
            &["owned", "refresh", "key", "remove\\-key"],
        ),
    ] {
        let text = fs::read_to_string(pages.join(page)).unwrap_or_else(|_| panic!("no {page}"));
        for word in words {
            assert!(text.contains(word), "{page} is missing {word}");
        }
    }
    for shell in ["elvish", "powershell"] {
        let script = stdout(&assert_ok(f.wineo().args(["completions", shell])));
        for word in ["install", "uninstall", "owned", "remove-key"] {
            assert!(script.contains(word), "{shell} is missing {word}");
        }
    }
}

fn assert_ok(command: &mut Command) -> std::process::Output {
    let output = command.output().expect("wineo should start");
    assert_success(&output);
    output
}
