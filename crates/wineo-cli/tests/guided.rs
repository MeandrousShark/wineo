//! The guided flows driven through a pseudo-terminal, the way someone at a
//! terminal would, against the fake wine scripts.

mod common;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
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
                let seen = strip_escapes(&self.seen.lock().unwrap());
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

    /// Types `line` and presses Enter, which a terminal sends as `\r`.
    fn send(&mut self, line: &str) {
        self.type_keys(&format!("{line}\r"));
    }

    fn type_keys(&mut self, keys: &str) {
        self.master.write_all(keys.as_bytes()).unwrap();
    }

    /// Ctrl+D at the start of a line, which the terminal turns into EOF.
    fn end_input(&mut self) {
        self.master.write_all(b"\x04").unwrap();
    }

    /// Ctrl+C typed at a question, which the line editor reads as a key.
    fn ctrl_c(&mut self) {
        self.type_keys("\x03");
    }

    /// SIGINT from outside, the way Ctrl+C reaches wineo while no question
    /// is being asked, like during an installer.
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
        let seen = strip_escapes(&self.seen.lock().unwrap());
        (status, seen)
    }
}

/// The output without the escape sequences a line editor redraws with.
fn strip_escapes(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        if char != '\x1b' {
            plain.push(char);
            continue;
        }
        if chars.next_if_eq(&'[').is_some() {
            // Parameters, then one final byte from `@` to `~`.
            for char in chars.by_ref() {
                if ('@'..='~').contains(&char) {
                    break;
                }
            }
        } else {
            chars.next();
        }
    }
    plain
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

    fn new_guided(&self, args: &[&str]) -> Pty {
        let mut command = self.wineo();
        command.arg("new").args(args);
        Pty::spawn(command)
    }

    fn registered(&self, name: &str) -> bool {
        self.config_dir()
            .join(format!("environments/{name}.toml"))
            .exists()
    }
}

/// Answers the questions up to the review for an environment called `name`
/// with no template, the suggested location and settings, and `how`.
fn plain_answers(pty: &mut Pty, name: &str, how: &str) {
    pty.expect("Start from a template?");
    pty.expect("Template [1]: ");
    pty.send("1");
    pty.expect("Install name, or < to go back: ");
    pty.send(name);
    pty.expect("Prefix location, or < to go back [");
    pty.send("");
    pty.expect(&format!("Settings for {name}:"));
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect(&format!("How does {name} get its program?"));
    pty.expect("Choice [1]: ");
    pty.send(how);
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
    pty.ctrl_c();
    pty.expect("Canceled. Unsaved changes were dropped.");
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
    pty.expect("The config changed meanwhile. Review again:");
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
    pty.ctrl_c();
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
    pty.expect("app's last installer didn't finish cleanly.");
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

#[test]
fn guided_new_needs_a_terminal() {
    let f = Fixture::new();
    let output = f.wineo().arg("new").stdin(Stdio::piped()).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("wineo new -n <name>"),
        "{}",
        stderr(&output)
    );
    assert!(!f.config_dir().join("environments").exists());
}

#[test]
fn guided_new_makes_an_empty_environment_with_nothing_switched_on() {
    let f = Fixture::new();
    let mut pty = f.new_guided(&[]);
    plain_answers(&mut pty, "game", "3");
    pty.expect("Ready to create game:");
    pty.expect("  template  none");
    pty.expect("  program   none for now");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Created game.");
    pty.expect("Add a target later with: wineo setup game");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    let config = f.config("game");
    assert!(config.contains("runner = \"wine\""), "{config}");
    for key in ["dxvk", "vkd3d", "gamemode", "gameid", "[targets"] {
        assert!(!config.contains(key), "{key} in {config}");
    }
    assert!(
        f.root
            .path()
            .join("data/wineo/prefixes/game/system.reg")
            .exists()
    );
}

#[test]
fn a_template_from_either_flag_is_prefilled_and_shown() {
    let f = Fixture::new();
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    f.install_build("vkd3d", "vkd3d-2.14", &["x64", "x86"]);
    for (flag, name) in [("-t", "short"), ("--template", "long")] {
        let mut pty = f.new_guided(&[flag, "gaming"]);
        // The template came from the command line, so it isn't asked about.
        pty.expect("Install name, or < to go back: ");
        pty.send(name);
        pty.expect("Prefix location, or < to go back [");
        pty.send("");
        pty.expect("  template  gaming (DXVK latest, vkd3d-proton latest, gamemode on)");
        pty.expect("Choice [1]: ");
        pty.send("1");
        pty.expect("Choice [1]: ");
        pty.send("3");
        pty.expect("Ready to create");
        pty.expect("Choice [1]: ");
        pty.send("1");
        pty.expect("Add a target later with");
        let (status, output) = pty.finish();
        assert!(status.success(), "{output}");
        let config = f.config(name);
        assert!(config.contains("dxvk = \"latest\""), "{config}");
        assert!(config.contains("vkd3d = \"latest\""), "{config}");
        assert!(config.contains("gamemode = true"), "{config}");
    }
}

#[test]
fn a_missing_build_is_explained_with_a_way_back() {
    let f = Fixture::new();
    let mut pty = f.new_guided(&["-t", "gaming"]);
    pty.expect("Install name, or < to go back: ");
    pty.send("game");
    pty.expect("Prefix location, or < to go back [");
    pty.send("");
    pty.expect(
        "No matching DXVK build is installed. Download the latest release? [Y/n], or < to go back: ",
    );
    pty.send("n");
    pty.expect("Can't use the gaming template: no dxvk builds are installed");
    // Nothing that can't work is offered, but going back is.
    pty.expect("  1) Change the runner");
    pty.expect("Choice [1]: ");
    pty.send("b");
    pty.expect("Start from a template?");
    pty.expect("Template [2]: ");
    pty.send("q");
    pty.expect("Canceled. Nothing was created.");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(!f.registered("game"));
    assert!(!f.root.path().join("data/wineo/prefixes/game").exists());
}

#[test]
fn explicit_flags_win_over_the_template() {
    let f = Fixture::new();
    f.install_runner("custom");
    f.write_template("mine", "runner = \"custom\"\n");
    let mut pty = f.new_guided(&[
        "--interactive",
        "-n",
        "game",
        "-t",
        "mine",
        "--runner",
        "wine",
    ]);
    pty.expect("Install name, or < to go back [game]: ");
    pty.send("");
    pty.expect("Prefix location, or < to go back [");
    pty.send("");
    pty.expect("  runner    wine (wine-fake)");
    pty.expect("Choice [1]: ");
    pty.send("q");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
}

#[test]
fn enter_takes_the_global_suggestions() {
    let f = Fixture::new();
    f.install_runner("custom");
    f.global_config("[defaults]\nrunner = \"custom\"\n");
    let mut pty = f.new_guided(&[]);
    pty.expect("Template [1]: ");
    pty.send("");
    pty.expect("Install name, or < to go back: ");
    pty.send("game");
    pty.expect("Prefix location, or < to go back [");
    pty.send("");
    pty.expect("  runner    custom (wine-fake)");
    pty.expect("  arch      win64");
    pty.expect("Choice [1]: ");
    pty.send("");
    pty.expect("Choice [1]: ");
    pty.send("3");
    pty.expect("Choice [1]: ");
    pty.send("");
    pty.expect("Add a target later with");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(f.config("game").contains("runner = \"custom\""));
}

#[test]
fn guided_new_runs_the_installer_then_picks_what_it_installed() {
    let f = Fixture::new();
    let installer = f.root.path().join("Downloads/My Games/install-game.exe");
    fs::create_dir_all(installer.parent().unwrap()).unwrap();
    fs::write(&installer, b"").unwrap();
    let mut pty = f.new_guided(&[]);
    plain_answers(&mut pty, "game", "1");
    pty.expect("Installer, or < to go back: ");
    pty.send(installer.to_str().unwrap());
    pty.expect("  program   run the installer ");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Created game.");
    pty.expect("The installer finished and the prefix is idle.");
    pty.expect("  1) Game");
    pty.expect("Add which, like 1 3: ");
    pty.send("1");
    pty.expect("Adding \"Game\":");
    pty.expect("Choice [1]: ");
    pty.send("");
    pty.expect("Add which, like 1 3: ");
    pty.send("s");
    pty.expect("  + game  C:\\Games\\Game\\Game.exe, desktop entry \"Game\"");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Saved game.");
    pty.expect("Start it:        wineo start game");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(f.config("game").contains(r"exe = 'C:\Games\Game\Game.exe'"));
    let log = fs::read_to_string(f.runtime("game").install_log_path()).unwrap();
    assert!(log.contains("install-game.exe"), "{log}");
}

#[test]
fn canceling_after_creation_keeps_the_environment() {
    let f = Fixture::new();
    let installer = f.root.path().join("install-game.exe");
    fs::write(&installer, b"").unwrap();
    let mut pty = f.new_guided(&[]);
    plain_answers(&mut pty, "game", "1");
    pty.expect("Installer, or < to go back: ");
    pty.send(installer.to_str().unwrap());
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Add which, like 1 3: ");
    pty.send("q");
    pty.expect("Canceled. Kept game, continue with `wineo setup game`.");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(f.registered("game"));
}

#[test]
fn a_registered_name_offers_setup_instead() {
    let f = Fixture::new();
    f.games("app", "");
    let before = f.config("app");
    let mut pty = f.new_guided(&[]);
    pty.expect("Template [1]: ");
    pty.send("1");
    pty.expect("Install name, or < to go back: ");
    pty.send("app");
    pty.expect("app already exists.");
    pty.expect("  1) Set up app instead");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("What next?");
    pty.expect("Choice [1]: ");
    pty.send("5");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert_eq!(f.config("app"), before);
}

#[test]
fn a_failed_creation_says_nothing_was_registered() {
    let f = Fixture::new();
    let bin = f.root.path().join("data/wineo/runners/broken/bin");
    fs::create_dir_all(&bin).unwrap();
    for (file, script) in [
        (
            "wine",
            "#!/bin/sh\necho 'wine: could not load kernel32.dll' >&2\nexit 1\n",
        ),
        ("wineserver", "#!/bin/sh\nexit 0\n"),
    ] {
        fs::write(bin.join(file), script).unwrap();
        fs::set_permissions(bin.join(file), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut pty = f.new_guided(&["--runner", "broken"]);
    plain_answers(&mut pty, "game", "3");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("error: wine failed to create the prefix: could not load kernel32.dll");
    pty.expect("game wasn't created.");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1), "{output}");
    assert!(!f.registered("game"));
    assert!(!f.root.path().join("data/wineo/prefixes/game").exists());
}

#[test]
fn a_name_that_cant_be_used_says_why_and_suggests_one() {
    let f = Fixture::new();
    let mut pty = f.new_guided(&[]);
    pty.expect("Template [1]: ");
    pty.send("1");
    pty.expect("Used in commands like `wineo start <name>`");
    pty.expect("Install name, or < to go back: ");
    pty.send("Stellar Blade");
    pty.expect("Invalid input: spaces not allowed.");
    pty.expect("Install name, or < to go back: ");
    pty.ctrl_c();
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
}

#[test]
fn a_proton_runner_leaves_the_templates_dxvk_and_vkd3d_out() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    // No DXVK or vkd3d builds are installed, and none are needed.
    let mut pty = f.new_guided(&["-t", "gaming", "--runner", "GE-Proton9-20"]);
    pty.expect("Install name, or < to go back: ");
    pty.send("game");
    pty.expect("Prefix location, or < to go back [");
    pty.send("");
    pty.expect("  template  gaming (gamemode on; no DXVK or vkd3d-proton, Proton has its own)");
    pty.expect("  1) Use these");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Choice [1]: ");
    pty.send("3");
    pty.expect("no DXVK or vkd3d-proton, Proton has its own");
    pty.expect("Choice [1]: ");
    pty.send("1");
    pty.expect("Add a target later with");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    let config = f.config("game");
    assert!(config.contains("gamemode = true"), "{config}");
    assert!(!config.contains("dxvk"), "{config}");
    assert!(!config.contains("vkd3d"), "{config}");
}

#[test]
fn the_arrow_keys_and_word_deletion_edit_an_answer() {
    let f = Fixture::new();
    let mut pty = f.new_guided(&[]);
    pty.expect("Template [1]: ");
    pty.send("1");
    pty.expect("Install name, or < to go back: ");
    // `game Xx`, then Ctrl+W deletes `Xx`, and Left twice lands before the
    // `e` for an insert.
    pty.type_keys("game Xx");
    pty.type_keys("\x17");
    pty.type_keys("\x1b[D\x1b[D-2\r");
    pty.expect("Prefix location, or < to go back [");
    pty.expect("/prefixes/gam-2e]: ");
    pty.ctrl_c();
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
}
