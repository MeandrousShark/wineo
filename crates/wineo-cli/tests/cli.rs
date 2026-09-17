use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use wineo_core::runtime::Runtime;

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

/// A minimal environment config, enough for commands that don't need a target.
const MINIMAL_ENVIRONMENT: &str = "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n";

fn runtime_dir(root: &Path, name: &str) -> PathBuf {
    root.join("state/wineo").join(name)
}

fn write_log(root: &Path, name: &str, file: &str, text: &str) -> PathBuf {
    let dir = runtime_dir(root, name);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(file);
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

/// Recursively finds files wine's menu builder would have created.
fn find_wine_extension_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("wine-extension-"))
            {
                found.push(path);
            }
        }
    }
    found
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
fn new_with_a_missing_installer_fails_before_creating_anything() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).args([
        "new",
        "-n",
        "app",
        "--install-exe",
        "/nonexistent/setup.exe",
    ]));
    assert!(!result.status.success());
    assert!(
        stderr(&result).contains("/nonexistent/setup.exe"),
        "{}",
        stderr(&result)
    );
    assert!(
        !root
            .path()
            .join("config/wineo/environments/app.toml")
            .exists()
    );
    assert!(!root.path().join("data/wineo/prefixes/app").exists());
}

#[test]
fn logs_prints_the_full_log_by_default() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    write_log(
        root.path(),
        "notepad",
        "output.log",
        "line1\nline2\nline3\n",
    );

    let result = output(wineo(root.path()).args(["logs", "notepad"]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "line1\nline2\nline3\n"
    );
}

#[test]
fn logs_dash_n_prints_the_last_lines() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    write_log(
        root.path(),
        "notepad",
        "output.log",
        "line1\nline2\nline3\n",
    );

    let result = output(wineo(root.path()).args(["logs", "notepad", "-n", "2"]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "line2\nline3\n");

    let result = output(wineo(root.path()).args(["logs", "notepad", "-n", "0"]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(result.stdout.is_empty());
}

#[test]
fn logs_previous_prints_the_run_before_the_latest() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    write_log(root.path(), "notepad", "output.log", "latest run\n");
    write_log(root.path(), "notepad", "output.log.1", "previous run\n");

    let result = output(wineo(root.path()).args(["logs", "notepad", "--previous"]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "previous run\n");
}

#[test]
fn logs_notes_when_there_is_no_log_yet() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);

    let result = output(wineo(root.path()).args(["logs", "notepad"]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(result.stdout.is_empty());
    assert!(
        stderr(&result)
            .contains("notepad has no log yet, only runs started with `wineo start` are logged"),
        "{}",
        stderr(&result)
    );

    write_log(root.path(), "notepad", "output.log", "latest run\n");
    let result = output(wineo(root.path()).args(["logs", "notepad", "--previous"]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(result.stdout.is_empty());
    assert!(
        stderr(&result).contains("notepad has no previous log"),
        "{}",
        stderr(&result)
    );
}

#[test]
fn logs_errors_for_an_unknown_environment() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).args(["logs", "nope"]));
    assert!(!result.status.success());
    assert!(
        stderr(&result).contains("does not exist"),
        "{}",
        stderr(&result)
    );
}

#[test]
fn logs_follow_and_previous_conflict() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).args(["logs", "notepad", "-f", "--previous"]));
    assert!(!result.status.success());
    assert!(
        stderr(&result).contains("cannot be used with"),
        "{}",
        stderr(&result)
    );
}

#[test]
fn logs_follow_with_nothing_running_prints_and_exits() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    write_log(root.path(), "notepad", "output.log", "only run\n");

    let result = output(wineo(root.path()).args(["logs", "notepad", "-f"]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "only run\n");
}

#[test]
fn logs_follow_follows_a_running_environment() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    let log = write_log(root.path(), "notepad", "output.log", "before\n");
    let lock = Runtime::at(runtime_dir(root.path(), "notepad"))
        .try_lock()
        .unwrap()
        .unwrap();

    let mut child = wineo(root.path())
        .args(["logs", "notepad", "-f"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(500));

    let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
    writeln!(file, "after").unwrap();
    drop(file);
    thread::sleep(Duration::from_millis(500));
    drop(lock);

    wait_for("wineo logs to notice the run ended", || {
        matches!(child.try_wait(), Ok(Some(_)))
    });
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{}", stderr(&result));
    let text = String::from_utf8_lossy(&result.stdout);
    assert!(text.contains("before"), "{text}");
    assert!(text.contains("after"), "{text}");
}

#[test]
fn logs_notes_when_running_attached() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    write_log(root.path(), "notepad", "output.log", "old run\n");

    let dir = runtime_dir(root.path(), "notepad");
    let lock = Runtime::at(dir.clone()).try_lock().unwrap().unwrap();
    fs::write(
        dir.join("state.toml"),
        "status = \"running\"\nmode = \"attached\"\ntarget = \"notepad\"\npid = 4242\nstarted_at = \"2026-09-16T21:02:11Z\"\n",
    )
    .unwrap();

    let result = output(wineo(root.path()).args(["logs", "notepad"]));
    drop(lock);

    assert!(result.status.success(), "{}", stderr(&result));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "old run\n");
    assert!(
        stderr(&result).contains("notepad is running attached, so its output is in that terminal"),
        "{}",
        stderr(&result)
    );
}

#[test]
#[ignore = "runs wine and opens notepad"]
fn new_run_and_exec_with_real_wine() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).args(["new", "-n", "notepad"]));
    assert!(result.status.success(), "{}", stderr(&result));

    let user_reg = root.path().join("data/wineo/prefixes/notepad/user.reg");
    let user_reg_text = fs::read_to_string(&user_reg).unwrap();
    assert!(
        user_reg_text.contains("\"winemenubuilder.exe\"=\"\""),
        "{user_reg_text}"
    );

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

    let menu_entries = find_wine_extension_files(&root.path().join("data"));
    assert!(menu_entries.is_empty(), "{menu_entries:?}");
}

#[test]
#[ignore = "runs wine"]
fn new_with_install_exe_runs_the_installer_and_prints_the_hint() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).args([
        "new",
        "-n",
        "app",
        "--install-exe",
        "/usr/lib/wine/x86_64-windows/hostname.exe",
    ]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("Add a target with"),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
}
