mod common;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use common::{Fixture, assert_success, stderr, stdout};
use rustix::fs::OFlags;
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

struct Pty {
    master: File,
    child: Child,
    seen: Arc<Mutex<String>>,
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
        drop(command);

        let master = File::from(master);
        let mut output = master.try_clone().unwrap();
        let seen = Arc::new(Mutex::new(String::new()));
        let collected = seen.clone();
        let reader = thread::spawn(move || {
            let mut buffer = [0; 4096];
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
            reader: Some(reader),
        }
    }

    fn expect(&self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.seen.lock().unwrap().contains(text) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {text:?} in:\n{}",
                self.seen.lock().unwrap()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn send(&mut self, text: &str) {
        self.master
            .write_all(format!("{text}\r").as_bytes())
            .unwrap();
    }

    fn send_eof(&mut self) {
        self.master.write_all(b"\x04").unwrap();
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
fn removing_builds_without_a_terminal_needs_yes() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    f.install_build("dxvk", "2.0", &["x64", "x32"]);
    f.install_build("vkd3d", "2.0", &["x64", "x86"]);

    for (args, path) in [
        (
            &["runners", "remove", "wine-tkg"][..],
            "data/wineo/runners/wine-tkg",
        ),
        (&["dxvk", "remove", "2.0"][..], "data/wineo/dxvk/2.0"),
        (&["vkd3d", "remove", "2.0"][..], "data/wineo/vkd3d/2.0"),
    ] {
        let refused = f.run(args);
        assert!(!refused.status.success());
        assert!(stdout(&refused).is_empty());
        assert!(
            stderr(&refused).contains("terminal or --yes"),
            "{}",
            stderr(&refused)
        );
        assert!(f.root.path().join(path).exists());
    }
}

#[test]
fn yes_removes_runners_and_components_with_one_line_success() {
    let f = Fixture::new();
    f.write_template("gaming", "schema_version = 1\ngamemode = true\n");
    f.install_runner("wine-tkg");
    f.install_build("dxvk", "2.0", &["x64", "x32"]);
    f.install_build("vkd3d", "2.0", &["x64", "x86"]);

    for (args, path, expected) in [
        (
            &["runners", "remove", "wine-tkg", "--yes"][..],
            "data/wineo/runners/wine-tkg",
            "Removed runner wine-tkg (",
        ),
        (
            &["dxvk", "remove", "2.0", "--yes"][..],
            "data/wineo/dxvk/2.0",
            "Removed DXVK build 2.0 (",
        ),
        (
            &["vkd3d", "remove", "2.0", "--yes"][..],
            "data/wineo/vkd3d/2.0",
            "Removed vkd3d-proton build 2.0 (",
        ),
    ] {
        let removed = f.run(args);
        assert_success(&removed);
        assert_eq!(stdout(&removed).lines().count(), 1, "{}", stdout(&removed));
        assert!(
            stdout(&removed).starts_with(expected),
            "{}",
            stdout(&removed)
        );
        assert!(stdout(&removed).contains("freed)"), "{}", stdout(&removed));
        assert!(!f.root.path().join(path).exists());
    }
}

#[test]
fn retrying_interrupted_removal_sweeps_the_last_build_for_each_kind() {
    for (kind, name, root, args) in [
        (
            "runner",
            "wine-tkg",
            "runners",
            &["runners", "remove", "wine-tkg", "--yes"][..],
        ),
        (
            "dxvk",
            "2.0",
            "dxvk",
            &["dxvk", "remove", "2.0", "--yes"][..],
        ),
        (
            "vkd3d",
            "2.0",
            "vkd3d",
            &["vkd3d", "remove", "2.0", "--yes"][..],
        ),
    ] {
        for interactive in [false, true] {
            let f = Fixture::new();
            if kind == "runner" {
                f.install_runner(name);
            } else if kind == "dxvk" {
                f.install_build(kind, name, &["x64", "x32"]);
            } else {
                f.install_build(kind, name, &["x64", "x86"]);
            }

            let root = f.root.path().join("data/wineo").join(root);
            let build = root.join(name);
            let staged = root.join(format!(".wineo-remove-{name}-ABC123"));
            fs::create_dir(&staged).unwrap();
            fs::rename(build, staged.join("build")).unwrap();

            let (status, output) = if interactive {
                let mut command = f.wineo();
                command.args(&args[..args.len() - 1]);
                Pty::spawn(command).finish()
            } else {
                let retried = f.run(args);
                (retried.status, stderr(&retried).to_owned())
            };
            assert!(!status.success(), "{kind}: retry should report missing");
            assert!(output.contains("is not installed"), "{kind}: {output}");
            assert!(
                !staged.exists(),
                "{kind}: abandoned staging should be swept"
            );
        }
    }
}

#[test]
fn yes_refuses_builds_used_by_an_environment() {
    let f = Fixture::new();
    f.write_template("gaming", "schema_version = 1\ngamemode = true\n");
    f.install_runner("wine-tkg");
    f.install_build("dxvk", "2.0", &["x64", "x32"]);
    f.install_build("vkd3d", "2.0", &["x64", "x86"]);
    f.environment_with_runner("app", "wine-tkg", "");
    assert_success(&f.run(&["set", "app", "dxvk=2.0", "vkd3d=2.0"]));

    for (args, path, name) in [
        (
            &["runners", "remove", "wine-tkg", "--yes"][..],
            "data/wineo/runners/wine-tkg",
            "wine-tkg",
        ),
        (
            &["dxvk", "remove", "2.0", "--yes"][..],
            "data/wineo/dxvk/2.0",
            "2.0",
        ),
        (
            &["vkd3d", "remove", "2.0", "--yes"][..],
            "data/wineo/vkd3d/2.0",
            "2.0",
        ),
    ] {
        let refused = f.run(args);
        assert!(!refused.status.success());
        assert!(
            stderr(&refused).contains(&format!("{name} is used by app")),
            "{}",
            stderr(&refused)
        );
        assert!(f.root.path().join(path).exists());
    }
}

#[test]
fn system_wine_cannot_be_removed() {
    let f = Fixture::new();
    let refused = f.run(&["runners", "remove", "wine", "--yes"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("not a managed build"),
        "{}",
        stderr(&refused)
    );
}

#[test]
fn terminal_removal_shows_size_and_accepts_confirmation() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    let mut command = f.wineo();
    command.args(["runners", "remove", "wine-tkg"]);
    let mut pty = Pty::spawn(command);

    pty.expect("Remove runner wine-tkg (");
    pty.expect(")? [y/N] ");
    pty.send("y");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    let shown_size = output
        .split("Remove runner wine-tkg (")
        .nth(1)
        .and_then(|text| text.split(")? [y/N]").next())
        .expect("the confirmation includes the build size");
    assert!(!shown_size.trim().is_empty(), "{output}");
    assert!(output.contains("Removed runner wine-tkg ("), "{output}");
    assert!(!f.root.path().join("data/wineo/runners/wine-tkg").exists());
}

#[test]
fn empty_terminal_confirmation_keeps_the_build() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    let mut command = f.wineo();
    command.args(["runners", "remove", "wine-tkg"]);
    let mut pty = Pty::spawn(command);

    pty.expect("Remove runner wine-tkg (");
    pty.expect(")? [y/N] ");
    pty.send("");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(
        output.contains("Canceled. Nothing was removed."),
        "{output}"
    );
    assert!(f.root.path().join("data/wineo/runners/wine-tkg").exists());
}

#[test]
fn declining_terminal_confirmation_keeps_all_builds() {
    for (kind, name, args, path) in [
        (
            "runner",
            "wine-tkg",
            &["runners", "remove", "wine-tkg"][..],
            "data/wineo/runners/wine-tkg",
        ),
        (
            "dxvk",
            "2.0",
            &["dxvk", "remove", "2.0"][..],
            "data/wineo/dxvk/2.0",
        ),
        (
            "vkd3d",
            "2.0",
            &["vkd3d", "remove", "2.0"][..],
            "data/wineo/vkd3d/2.0",
        ),
    ] {
        for (response, eof) in [("n", false), ("Ctrl+D", true)] {
            let f = Fixture::new();
            match kind {
                "runner" => f.install_runner(name),
                "dxvk" => f.install_build(kind, name, &["x64", "x32"]),
                "vkd3d" => f.install_build(kind, name, &["x64", "x86"]),
                _ => unreachable!(),
            }
            let mut command = f.wineo();
            command.args(args);
            let mut pty = Pty::spawn(command);

            pty.expect(")? [y/N] ");
            if eof {
                pty.send_eof();
            } else {
                pty.send(response);
            }
            let (status, output) = pty.finish();
            assert_eq!(status.code(), Some(130), "{kind}, {response}: {output}");
            assert!(
                output.contains("Canceled. Nothing was removed."),
                "{kind}, {response}: {output}"
            );
            assert!(
                f.root.path().join(path).exists(),
                "{kind}, {response}: {path} was removed"
            );
        }
    }
}

#[test]
fn malformed_configs_refuse_removal_before_the_terminal_prompt() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    f.environment_with_runner("broken", "wine", "");
    let config = f.root.path().join("config/wineo/environments/broken.toml");
    std::fs::write(&config, "runner = [invalid").unwrap();
    f.write_template("broken", "not valid TOML");
    let template = f.root.path().join("config/wineo/templates/broken.toml");
    let mut command = f.wineo();
    command.args(["runners", "remove", "wine-tkg"]);
    let (status, output) = Pty::spawn(command).finish();
    assert_eq!(status.code(), Some(1), "{output}");
    assert!(!output.contains("[y/N]"), "{output}");
    assert!(
        output.contains("error: wine-tkg is kept, since 2 configs can't be read"),
        "{output}"
    );
    for path in [&config, &template] {
        assert!(output.contains(&path.display().to_string()), "{output}");
    }
    assert!(f.root.path().join("data/wineo/runners/wine-tkg").exists());

    std::fs::remove_file(&config).unwrap();
    std::fs::remove_file(&template).unwrap();
    let mut command = f.wineo();
    command.args(["runners", "remove", "wine-tkg"]);
    let mut pty = Pty::spawn(command);
    pty.expect(")? [y/N] ");
    pty.send("y");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(!f.root.path().join("data/wineo/runners/wine-tkg").exists());
}

#[test]
fn yes_refuses_removal_when_a_config_is_malformed() {
    let f = Fixture::new();
    f.write_template("gaming", "schema_version = 1\ngamemode = true\n");
    f.environment_with_runner("broken", "wine", "");
    let config = f.root.path().join("config/wineo/environments/broken.toml");
    std::fs::write(&config, "runner = [invalid").unwrap();
    f.write_template("broken", "not valid TOML");
    let template = f.root.path().join("config/wineo/templates/broken.toml");
    f.install_runner("wine-tkg");
    f.install_build("dxvk", "2.0", &["x64", "x32"]);
    f.install_build("vkd3d", "2.0", &["x64", "x86"]);
    for (args, name, path) in [
        (
            ["runners", "remove", "wine-tkg", "--yes"],
            "wine-tkg",
            "data/wineo/runners/wine-tkg",
        ),
        (
            ["dxvk", "remove", "2.0", "--yes"],
            "2.0",
            "data/wineo/dxvk/2.0",
        ),
        (
            ["vkd3d", "remove", "2.0", "--yes"],
            "2.0",
            "data/wineo/vkd3d/2.0",
        ),
    ] {
        let refused = f.run(&args);
        assert_eq!(refused.status.code(), Some(1), "{}", stderr(&refused));
        let errors = stderr(&refused);
        assert!(
            errors.starts_with(&format!(
                "error: {name} is kept, since 2 configs can't be read\n"
            )),
            "{errors}"
        );
        for config in [&config, &template] {
            assert!(errors.contains(&config.display().to_string()), "{errors}");
        }
        assert!(stdout(&refused).is_empty());
        assert!(f.root.path().join(path).exists(), "{path} was removed");
    }

    std::fs::remove_file(&config).unwrap();
    std::fs::remove_file(&template).unwrap();
    for args in [
        ["runners", "remove", "wine-tkg", "--yes"],
        ["dxvk", "remove", "2.0", "--yes"],
        ["vkd3d", "remove", "2.0", "--yes"],
    ] {
        let removed = f.run(&args);
        assert_success(&removed);
        assert!(stderr(&removed).is_empty(), "{}", stderr(&removed));
        assert!(stdout(&removed).starts_with("Removed "));
    }
}
