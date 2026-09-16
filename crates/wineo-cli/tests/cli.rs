use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Points every XDG dir into `root` so tests never touch the real config.
fn wineo(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wineo"));
    command
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"));
    command
}

fn output(command: &mut Command) -> Output {
    command.output().expect("wineo should start")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn write_environment(root: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = root.join(format!("config/wineo/environments/{name}.toml"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn set_global_takes_settings_without_a_name() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).args([
        "set",
        "--global",
        "stop_timeout=5",
        "env.WINEDEBUG=-all",
    ]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert_eq!(
        fs::read_to_string(root.path().join("config/wineo/config.toml")).unwrap(),
        "stop_timeout = 5\n\n[env]\nWINEDEBUG = \"-all\"\n"
    );
}

#[test]
fn set_needs_a_name_or_global_and_something_to_change() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).args(["set", "runner=wine"]));
    assert!(!result.status.success());
    assert!(
        stderr(&result).contains("name an environment"),
        "{}",
        stderr(&result)
    );

    let result = output(wineo(root.path()).args(["set", "notepad"]));
    assert!(!result.status.success());
    assert!(
        stderr(&result).contains("nothing to change"),
        "{}",
        stderr(&result)
    );
}

#[test]
fn set_edits_an_environment_and_reports_unset_keys_that_were_missing() {
    let root = tempfile::tempdir().unwrap();
    let path = write_environment(
        root.path(),
        "notepad",
        "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n",
    );
    let result = output(wineo(root.path()).args([
        "set",
        "notepad",
        "targets.notepad.exe=notepad",
        "--unset",
        "env.NOPE",
    ]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(stderr(&result).contains("`env.NOPE` was not set"));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n\n[targets.notepad]\nexe = \"notepad\"\n"
    );

    let result = output(wineo(root.path()).args(["set", "notepad", "arch=win32"]));
    assert!(!result.status.success());
    assert!(
        stderr(&result).contains("arch is fixed"),
        "{}",
        stderr(&result)
    );
}

#[test]
fn config_prints_stored_files_even_when_invalid() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).arg("config"));
    assert!(result.status.success());
    assert!(stderr(&result).contains("defaults are in use"));

    write_environment(root.path(), "broken", "this is not toml");
    let result = output(wineo(root.path()).args(["config", "broken"]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "this is not toml");

    let result = output(wineo(root.path()).args(["config", "nope"]));
    assert!(!result.status.success());
    assert!(stderr(&result).contains("does not exist"));
}

#[test]
#[ignore = "runs wine and opens notepad"]
fn new_run_and_exec_with_real_wine() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).args(["new", "-n", "notepad"]));
    assert!(result.status.success(), "{}", stderr(&result));
    let result = output(wineo(root.path()).args(["set", "notepad", "targets.notepad.exe=notepad"]));
    assert!(result.status.success(), "{}", stderr(&result));

    let result = output(wineo(root.path()).args([
        "exec",
        "-e",
        "GREETING=hello",
        "notepad",
        "cmd",
        "/c",
        "echo %GREETING% & exit 3",
    ]));
    assert_eq!(result.status.code(), Some(3), "{}", stderr(&result));
    assert!(String::from_utf8_lossy(&result.stdout).contains("hello"));

    let state = root.path().join("state/wineo/notepad/state.toml");
    let run = wineo(root.path())
        .args(["run", "notepad"])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("the run to be recorded", || {
        fs::read_to_string(&state).is_ok_and(|text| text.contains("status = \"running\""))
    });
    // Let notepad open its window, so it can be asked to close.
    thread::sleep(Duration::from_secs(3));

    let second = output(wineo(root.path()).args(["run", "notepad"]));
    assert!(!second.status.success());
    assert!(
        stderr(&second).contains("already running"),
        "{}",
        stderr(&second)
    );

    let interrupted = Command::new("kill")
        .args(["-INT", &run.id().to_string()])
        .status()
        .unwrap();
    assert!(interrupted.success());
    let result = run.wait_with_output().unwrap();
    assert!(result.status.success(), "{}", stderr(&result));
    let text = fs::read_to_string(&state).unwrap();
    assert!(text.contains("status = \"stopped\""), "{text}");
}
