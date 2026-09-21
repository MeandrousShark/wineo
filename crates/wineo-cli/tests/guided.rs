//! The guided flows driven through a pseudo-terminal, the way someone at a
//! terminal would, against the fake wine scripts.

mod common;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, symlink};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::fs::OFlags;
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

use common::*;

/// wineo on the far side of a pseudo-terminal.
struct Pty {
    master: File,
    child: Child,
    seen: Arc<Mutex<String>>,
    /// How much of the output earlier expectations have used up.
    cursor: usize,
    reader: Option<JoinHandle<()>>,
}

impl Pty {
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
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave);
        let child = command.spawn().unwrap();
        // The command holds the only other copies of the terminal's end, and
        // dropping them is what lets reading see the end once wineo exits.
        drop(command);

        let master = File::from(master);
        let mut output = master.try_clone().unwrap();
        let seen = Arc::new(Mutex::new(String::new()));
        let collected = seen.clone();
        let reader = thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = output.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&buffer[..read]).replace('\r', "");
                collected.lock().unwrap().push_str(&text);
            }
        });
        Self {
            master,
            child,
            seen,
            cursor: 0,
            reader: Some(reader),
        }
    }

    /// Waits for `text` to show up after whatever was expected last.
    #[track_caller]
    fn expect(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            {
                let seen = self.seen.lock().unwrap();
                if let Some(at) = seen[self.cursor..].find(text) {
                    self.cursor += at + text.len();
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {text:?} in:\n{}",
                    &seen[self.cursor..]
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn send(&mut self, line: &str) {
        self.master
            .write_all(format!("{line}\n").as_bytes())
            .unwrap();
    }

    /// Ctrl+D at the start of a line, which the terminal turns into EOF.
    fn end_input(&mut self) {
        self.master.write_all(b"\x04").unwrap();
    }

    fn interrupt(&self) {
        let sent = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success());
    }

    fn finish(mut self) -> (ExitStatus, String) {
        let status = self.child.wait().unwrap();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let seen = self.seen.lock().unwrap().clone();
        (status, seen)
    }
}

impl Fixture {
    /// An environment whose prefix has a couple of games installed, with the
    /// `C:` drive linked the way wine links it.
    fn games(&self, name: &str, targets: &str) {
        self.environment(name, targets);
        let prefix = self.root.path().join("prefixes").join(name);
        fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        symlink("../drive_c", prefix.join("dosdevices/c:")).unwrap();
        for exe in ["Hades.exe", "Config.exe"] {
            let path = prefix.join("drive_c/Games/Hades").join(exe);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"").unwrap();
        }
    }

    fn setup(&self, name: &str) -> Pty {
        let mut command = self.wineo();
        command.args(["setup", name]);
        Pty::spawn(command)
    }

    fn config(&self, name: &str) -> String {
        fs::read_to_string(self.config_dir().join(format!("environments/{name}.toml"))).unwrap()
    }
}

#[test]
fn setup_needs_a_terminal() {
    let f = Fixture::new();
    f.games("app", "");
    let before = f.config("app");
    let output = f
        .wineo()
        .args(["setup", "app"])
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(text.contains("needs a terminal"), "{text}");
    assert!(
        text.contains("wineo set app targets.<target>.exe="),
        "{text}"
    );
    assert_eq!(f.config("app"), before);
}

#[test]
fn setup_adds_targets_from_what_is_installed() {
    let f = Fixture::new();
    f.games("app", "");
    let mut pty = f.setup("app");
    pty.expect("No targets yet.");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Add which, like 1 3: ");
    pty.send("1 2");
    pty.expect("Adding \"Config\":");
    pty.expect("Choice [1]: ");
    pty.send("");
    pty.expect("Adding \"Hades\":");
    pty.expect("Choice [1]: ");
    pty.send("");
    pty.expect("s) done adding");
    pty.send("s");
    pty.expect("Choice [5]: ");
    pty.send("5");
    pty.expect("Which target should `wineo start app` launch?");
    pty.send("2");
    pty.expect("  default: hades (was none)");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Saved app.");
    pty.expect("Start it:        wineo start app");
    pty.expect("Start another:   wineo start app --target config");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");

    let config = f.config("app");
    assert!(config.contains("[targets.hades]"), "{config}");
    assert!(
        config.contains(r"exe = 'C:\Games\Hades\Hades.exe'"),
        "{config}"
    );
    assert!(config.contains("default = true"), "{config}");
    let entries = f.root.path().join("data/applications");
    assert!(entries.join("wineo-app-hades.desktop").exists());
    assert!(entries.join("wineo-app-config.desktop").exists());
}

#[test]
fn a_wrong_answer_is_explained_and_the_question_asked_again() {
    let f = Fixture::new();
    f.games("app", "");
    let mut pty = f.setup("app");
    pty.expect("Choice [1]: ");
    pty.send("9");
    pty.expect("`9` is not one of the choices");
    pty.expect("Choice [1]: ");
    pty.send("5");
    pty.expect("Add a target later with: wineo setup app");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
}

#[test]
fn ctrl_c_at_a_question_cancels_without_saving() {
    let f = Fixture::new();
    f.games("app", "");
    let before = f.config("app");
    let mut pty = f.setup("app");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Add which, like 1 3: ");
    pty.send("2");
    pty.expect("Choice [1]: ");
    pty.send("");
    pty.expect("Add which, like 1 3: ");
    pty.send("s");
    pty.expect("Choice [5]: ");
    pty.send("5");
    pty.expect("Save these changes?");
    pty.expect("Choice [1]: ");
    pty.interrupt();
    pty.expect("Canceled, and anything not yet saved was left out");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert_eq!(f.config("app"), before);
}

#[test]
fn ctrl_d_cancels_too() {
    let f = Fixture::new();
    f.games("app", "");
    let mut pty = f.setup("app");
    pty.expect("Choice [1]: ");
    pty.end_input();
    pty.expect("Canceled");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
}

#[test]
fn a_change_made_while_a_question_waits_refreshes_the_review() {
    let f = Fixture::new();
    f.games(
        "app",
        "[targets.app]\nexe = 'C:\\Games\\Hades\\Hades.exe'\n",
    );
    let mut pty = f.setup("app");
    pty.expect("Choice [5]: ");
    pty.send("2");
    pty.expect("Target: ");
    pty.send("1");
    pty.expect("Choice [1]: ");
    // The working directory, which is all this setup changes.
    pty.send("4");
    pty.expect("Working directory, or empty for the executable's folder, or < to go back: ");
    pty.send(r"C:\Games");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Choice [5]: ");
    pty.send("5");
    pty.expect("Save these changes?");
    pty.expect("Choice [1]: ");

    // Meanwhile, someone changes the same target's arguments.
    assert_success(&f.run(&["set", "app", r#"targets.app.args=["-safe"]"#]));
    pty.send("1");
    pty.expect("app's config changed while you were choosing, so here are the changes again.");
    pty.expect(r"  ~ app  working dir: the executable's folder -> C:\Games");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Saved app.");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");

    let config = f.config("app");
    assert!(config.contains(r#"args = ["-safe"]"#), "{config}");
    assert!(config.contains(r"cwd = 'C:\Games'"), "{config}");
}

#[test]
fn setup_runs_an_installer_then_picks_from_what_it_left() {
    let f = Fixture::new();
    f.games("app", "");
    let mut pty = f.setup("app");
    pty.expect("Choice [1]: ");
    pty.send("4");
    pty.expect("Installer or updater, as a Unix path or a Windows one, or < to go back: ");
    pty.send("handoff");
    pty.expect("Run handoff in app? Its output goes to ");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("The installer exited. Waiting for anything it started to finish");
    pty.expect("The installer finished and the prefix is idle.");
    pty.expect("Programs in app:");
    pty.expect("Add which, like 1 3: ");
    pty.send("s");
    pty.expect("Choice [1]: ");
    pty.send("5");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(f.root.path().join("prefixes/app/handoff-done").exists());
    let log = fs::read_to_string(f.runtime("app").install_log_path()).unwrap();
    assert!(log.contains("fake wine: handoff"), "{log}");
}

#[test]
fn a_failed_installer_ends_setup_with_its_code() {
    let f = Fixture::new();
    f.games("app", "");
    let mut pty = f.setup("app");
    pty.expect("Choice [1]: ");
    pty.send("4");
    pty.expect("or < to go back: ");
    pty.send("exit-3");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("The installer exited with code 3.");
    pty.expect("Continue with `wineo setup app`");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(3), "{output}");
}

#[test]
fn staged_changes_are_saved_before_an_installer_runs() {
    let f = Fixture::new();
    f.games("app", "");
    let mut pty = f.setup("app");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Add which, like 1 3: ");
    pty.send("2");
    pty.expect("Choice [1]: ");
    pty.send("");
    pty.expect("Add which, like 1 3: ");
    pty.send("s");
    pty.expect("Choice [5]: ");
    pty.send("4");
    pty.expect("You have unsaved changes:");
    pty.expect("They are saved before the installer runs.");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Save these changes?");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Saved app.");
    pty.expect("or < to go back: ");
    // Going back from the installer keeps what was saved.
    pty.send("<");
    pty.expect("Choice [5]: ");
    pty.interrupt();
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(f.config("app").contains("[targets.hades]"));
}

#[test]
fn ctrl_c_during_an_installer_cancels_it_and_ends_setup() {
    let f = Fixture::new();
    f.games("app", "");
    let mut pty = f.setup("app");
    pty.expect("Choice [1]: ");
    pty.send("4");
    pty.expect("or < to go back: ");
    pty.send("setup");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Running it now. Ctrl+C asks it to close.");
    wait_for("the installer to start", || f.fake_program_running("app"));
    pty.interrupt();
    pty.expect("Canceling the installer, press Ctrl+C again to force it");
    pty.expect("The installer was canceled.");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(!f.fake_program_running("app"));
    assert!(!f.runtime("app").dir().join("operation.toml").exists());
}

#[test]
fn setup_offers_to_clean_up_after_an_unresolved_installer() {
    let f = Fixture::new();
    f.games("app", "");
    let record = f.runtime("app").dir().join("operation.toml");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(&record, "").unwrap();
    let mut pty = f.setup("app");
    pty.expect("app's last installer did not finish cleanly");
    pty.expect("  1) Stop what it left running");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Stopped what it left running.");
    pty.expect("Choice [1]: ");
    pty.send("5");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(!record.exists());
}
