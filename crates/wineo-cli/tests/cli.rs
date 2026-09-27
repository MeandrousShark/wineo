use std::fs;
use std::io::{BufRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use wineo_core::launch::Overrides;
use wineo_core::runtime::{RunMode, RunState, RunStatus, Runtime};

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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn write_environment(root: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = root.join(format!("config/wineo/environments/{name}.toml"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

/// A minimal valid environment config with one target, named the same as the environment.
fn env_toml(target: &str) -> String {
    format!(
        "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n\n[targets.{target}]\nexe = \"{target}\"\n"
    )
}

/// The runtime dir `wineo(root)` uses for `name`.
fn runtime(root: &Path, name: &str) -> Runtime {
    Runtime::at(root.join(format!("state/wineo/{name}")))
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

/// Recursively finds files wine's menu builder would have created. Symlinks
/// aren't followed, since a prefix's `z:` drive links to the root directory.
fn find_wine_extension_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
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
        "schema_version = 1\nstop_timeout = 5\n\n[env]\nWINEDEBUG = \"-all\"\n"
    );
}

fn write_global(root: &Path, text: &str) -> PathBuf {
    let path = root.join("config/wineo/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn set_refuses_to_use_defaults_for_an_unsupported_global_config() {
    let root = tempfile::tempdir().unwrap();
    let global = write_global(root.path(), "schema_version = 2\n");
    let path = write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    let result = output(wineo(root.path()).args(["set", "notepad", "gamemode=true"]));
    assert!(!result.status.success());
    assert!(
        stderr(&result).contains(&format!(
            "unsupported schema_version 2 in {}",
            global.display()
        )),
        "{}",
        stderr(&result)
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), MINIMAL_ENVIRONMENT);
    assert_eq!(fs::read_to_string(&global).unwrap(), "schema_version = 2\n");
}

#[test]
fn set_still_falls_back_to_defaults_for_a_broken_global_config() {
    let root = tempfile::tempdir().unwrap();
    write_global(root.path(), "stop_timeout = [\n");
    let path = write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    let result = output(wineo(root.path()).args(["set", "notepad", "gamemode=true"]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("gamemode = true")
    );
}

#[test]
fn run_refuses_an_unsupported_environment_version() {
    let root = tempfile::tempdir().unwrap();
    let text = format!("schema_version = 0\n{}", env_toml("notepad"));
    let path = write_environment(root.path(), "notepad", &text);
    let result = output(wineo(root.path()).args(["run", "notepad"]));
    assert!(!result.status.success());
    assert!(
        stderr(&result).contains("unsupported schema_version 0"),
        "{}",
        stderr(&result)
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert!(!runtime_dir(root.path(), "notepad").exists());
}

#[test]
fn status_warns_about_an_unsupported_global_config() {
    let root = tempfile::tempdir().unwrap();
    let global = write_global(root.path(), "schema_version = 2\n");
    write_environment(root.path(), "notepad", &env_toml("notepad"));
    let result = output(wineo(root.path()).args(["status", "notepad"]));
    assert!(result.status.success(), "{}", stderr(&result));
    assert_eq!(
        stderr(&result),
        format!(
            "warning: unsupported schema_version 2 in {}\n",
            global.display()
        )
    );
    assert!(stdout(&result).starts_with("notepad: inactive\n"));
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
        "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\nschema_version = 1\n\n[targets.notepad]\nexe = \"notepad\"\n"
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
fn new_with_install_exe_runs_the_installer_and_reports_what_it_left() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).args([
        "new",
        "-n",
        "app",
        "--install-exe",
        "/usr/lib/wine/x86_64-windows/hostname.exe",
    ]));
    assert!(result.status.success(), "{}", stderr(&result));
    // This installer installs nothing, so the scan reporting an empty prefix is
    // what proves it ran at all, against a prefix wine really created.
    assert!(
        stderr(&result).contains("Nothing is installed in app"),
        "{}",
        stderr(&result)
    );
    assert!(
        !stdout(&result).contains("Add a target with"),
        "the generic hint is only for a `new` with no installer: {}",
        stdout(&result)
    );
}

#[test]
fn list_shows_every_environment_regardless_of_state() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", &env_toml("notepad"));
    write_environment(root.path(), "photoshop", &env_toml("photoshop"));
    write_environment(root.path(), "winecfg", &env_toml("winecfg"));
    // A broken config is still just a name to list: it isn't parsed.
    write_environment(root.path(), "broken", "this is not toml");

    let notepad = runtime(root.path(), "notepad");
    let _lock = notepad.try_lock().unwrap().unwrap();
    notepad
        .write_state(&RunState::running(
            RunMode::Detached,
            "notepad".to_owned(),
            111,
            Some(111),
            &Overrides::default(),
        ))
        .unwrap();

    let mut finished = RunState::running(
        RunMode::Detached,
        "photoshop".to_owned(),
        222,
        Some(222),
        &Overrides::default(),
    );
    finished.finish(RunStatus::Exited);
    runtime(root.path(), "photoshop")
        .write_state(&finished)
        .unwrap();
    // winecfg never ran, so it has no state.toml at all.

    let result = output(wineo(root.path()).arg("list"));
    assert!(result.status.success(), "{}", stderr(&result));
    let text = stdout(&result);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(lines[0].starts_with("broken") && lines[0].contains("inactive"));
    assert!(
        lines[1].starts_with("notepad")
            && lines[1].contains("running")
            && lines[1].contains("since")
    );
    assert!(
        lines[2].starts_with("photoshop")
            && lines[2].contains("exited")
            && lines[2].contains("ended")
    );
    assert!(lines[3].starts_with("winecfg") && lines[3].contains("inactive"));
}

#[test]
fn list_reports_a_broken_state_file_but_keeps_going() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", &env_toml("notepad"));
    write_environment(root.path(), "zzz", &env_toml("zzz"));

    let dir = root.path().join("state/wineo/notepad");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("state.toml"), "not valid toml").unwrap();

    let result = output(wineo(root.path()).arg("list"));
    assert!(!result.status.success());
    assert!(stderr(&result).contains("error:"), "{}", stderr(&result));
    let text = stdout(&result);
    assert!(text.contains("zzz") && text.contains("inactive"), "{text}");
    assert!(!text.contains("notepad"), "{text}");
}

fn parsed_json(out: &Output) -> serde_json::Value {
    let text = stdout(out);
    assert!(text.ends_with('\n'), "{text}");
    assert!(!text.contains("\u{1b}["), "{text}");
    serde_json::from_str(&text).unwrap()
}

#[test]
fn json_collections_are_sorted_and_keep_broken_configs() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "zulu", &env_toml("zulu"));
    write_environment(root.path(), "broken", "not valid toml");
    write_environment(root.path(), "alpha", &env_toml("alpha"));
    let running = runtime(root.path(), "zulu");
    let _lock = running.try_lock().unwrap().unwrap();
    running
        .write_state(&RunState::running(
            RunMode::Attached,
            "zulu".to_owned(),
            1234,
            Some(1234),
            &Overrides::default(),
        ))
        .unwrap();

    let listed = output(wineo(root.path()).args(["list", "--json"]));
    assert!(listed.status.success(), "{}", stderr(&listed));
    let value = parsed_json(&listed);
    assert_eq!(value["errors"], serde_json::json!([]));
    assert_eq!(value["environments"].as_array().unwrap().len(), 3);
    assert_eq!(value["environments"][0]["name"], "alpha");
    assert_eq!(value["environments"][1]["name"], "broken");
    assert_eq!(value["environments"][1]["state"], "inactive");
    assert_eq!(value["environments"][1]["run"], serde_json::Value::Null);
    assert_eq!(value["environments"][2]["state"], "running");
    assert_eq!(value["environments"][2]["run"]["mode"], "attached");
    assert_eq!(value["environments"][2]["run"]["pid"], 1234);
    assert!(
        value["environments"][2]["run"]["started_at"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );

    let status = output(wineo(root.path()).args(["status", "--json"]));
    assert!(status.status.success(), "{}", stderr(&status));
    let value = parsed_json(&status);
    assert_eq!(value["environments"].as_array().unwrap().len(), 1);
    assert_eq!(value["environments"][0]["name"], "zulu");
    assert_eq!(value["errors"], serde_json::json!([]));
}

#[test]
fn json_collection_reports_partial_errors_without_stderr() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "bad", &env_toml("bad"));
    write_environment(root.path(), "good", &env_toml("good"));
    write_log(root.path(), "bad", "state.toml", "not valid toml");
    let good = runtime(root.path(), "good");
    let _lock = good.try_lock().unwrap().unwrap();
    good.write_state(&RunState::running(
        RunMode::Detached,
        "good".to_owned(),
        1234,
        Some(1234),
        &Overrides::default(),
    ))
    .unwrap();

    for command in ["list", "status"] {
        let out = output(wineo(root.path()).args([command, "--json"]));
        assert!(!out.status.success());
        assert!(stderr(&out).is_empty(), "{}", stderr(&out));
        let value = parsed_json(&out);
        assert_eq!(value["errors"][0]["name"], "bad");
        assert!(
            value["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("state.toml")
        );
        assert_eq!(value["errors"].as_array().unwrap().len(), 1);
        assert_eq!(value["environments"].as_array().unwrap().len(), 1);
        assert_eq!(value["environments"][0]["name"], "good");
        assert_eq!(value["environments"][0]["state"], "running");
    }
}

#[test]
fn json_named_status_omits_saved_launch_options_and_internal_identity() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "game", &env_toml("game"));
    let runtime = runtime(root.path(), "game");
    let mut run = RunState::running(
        RunMode::Detached,
        "game".to_owned(),
        42,
        Some(99),
        &Overrides {
            target: Some("game".to_owned()),
            args: vec!["secret-argument".to_owned()],
            env: [("SECRET".to_owned(), "secret-value".to_owned())]
                .into_iter()
                .collect(),
        },
    );
    run.boot_id = Some("secret-boot-id".to_owned());
    run.leader_start = Some(987);
    run.finish(RunStatus::Exited);
    runtime.write_state(&run).unwrap();

    let out = output(wineo(root.path()).args(["status", "game", "--json"]));
    assert!(out.status.success(), "{}", stderr(&out));
    let value = parsed_json(&out);
    assert_eq!(value["name"], "game");
    assert_eq!(value["state"], "exited");
    assert_eq!(value["runner"]["configured"], "wine");
    assert!(value["runner"]["resolved"].is_null() || value["runner"]["resolved"] == "wine");
    assert_eq!(value["prefix"], "/p");
    assert_eq!(value["run"]["pid"], 42);
    assert!(value["run"]["ended_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(value["run"]["program_exited_at"], serde_json::Value::Null);
    assert_eq!(value["desktop"], serde_json::json!([]));
    let text = stdout(&out);
    for secret in [
        "secret-argument",
        "SECRET",
        "secret-value",
        "secret-boot-id",
        "group",
        "leader_start",
        "requested_target",
    ] {
        assert!(!text.contains(secret), "{secret}: {text}");
    }

    let missing = output(wineo(root.path()).args(["status", "missing", "--json"]));
    assert!(!missing.status.success());
    assert!(stdout(&missing).is_empty());
    assert!(!stderr(&missing).is_empty());
}

#[test]
fn json_empty_and_busy_unresolved_collections() {
    let root = tempfile::tempdir().unwrap();
    let empty = output(wineo(root.path()).args(["status", "--json"]));
    assert!(empty.status.success());
    assert_eq!(
        parsed_json(&empty),
        serde_json::json!({"environments": [], "errors": []})
    );
    assert!(stderr(&empty).is_empty());

    write_environment(root.path(), "busy", &env_toml("busy"));
    write_environment(root.path(), "unresolved", &env_toml("unresolved"));
    let _held = locks(root.path())
        .acquire(wineo_core::operation::Operation::Winetricks, "busy", &[])
        .unwrap();
    write_log(
        root.path(),
        "unresolved",
        "operation.toml",
        "not valid toml",
    );
    let out = output(wineo(root.path()).args(["status", "--json"]));
    assert!(out.status.success(), "{}", stderr(&out));
    let value = parsed_json(&out);
    assert_eq!(value["environments"][0]["name"], "busy");
    assert_eq!(value["environments"][0]["state"], "inactive");
    assert_eq!(value["environments"][0]["busy"], "winetricks");
    assert_eq!(value["environments"][0]["unresolved"], false);
    assert_eq!(value["environments"][1]["name"], "unresolved");
    assert_eq!(value["environments"][1]["state"], "inactive");
    assert_eq!(value["environments"][1]["busy"], serde_json::Value::Null);
    assert_eq!(value["environments"][1]["unresolved"], true);
}

#[test]
fn status_without_a_name_shows_only_whats_running() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", &env_toml("notepad"));

    let result = output(wineo(root.path()).arg("status"));
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(stdout(&result).is_empty());
    assert!(
        stderr(&result).contains("Nothing is running"),
        "{}",
        stderr(&result)
    );

    let notepad = runtime(root.path(), "notepad");
    let _lock = notepad.try_lock().unwrap().unwrap();
    notepad
        .write_state(&RunState::running(
            RunMode::Detached,
            "notepad".to_owned(),
            111,
            Some(111),
            &Overrides::default(),
        ))
        .unwrap();

    let result = output(wineo(root.path()).arg("status"));
    assert!(result.status.success(), "{}", stderr(&result));
    let text = stdout(&result);
    assert!(
        text.contains("notepad") && text.contains("running") && text.contains("since"),
        "{text}"
    );
}

#[test]
fn status_name_running_shows_target_runner_prefix_and_pid() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", &env_toml("notepad"));
    let notepad = runtime(root.path(), "notepad");
    let _lock = notepad.try_lock().unwrap().unwrap();
    notepad
        .write_state(&RunState::running(
            RunMode::Attached,
            "notepad".to_owned(),
            4242,
            Some(4242),
            &Overrides::default(),
        ))
        .unwrap();

    let result = output(wineo(root.path()).args(["status", "notepad"]));
    assert!(result.status.success(), "{}", stderr(&result));
    let text = stdout(&result);
    assert!(
        text.starts_with("notepad: running attached since "),
        "{text}"
    );
    assert!(
        text.contains("\n  target     notepad (notepad)\n"),
        "{text}"
    );
    // Not asserting on the runner version: wine may not be installed here.
    assert!(text.contains("\n  runner     wine"), "{text}");
    assert!(text.contains("\n  prefix     /p\n"), "{text}");
    assert!(text.contains("\n  pid        4242\n"), "{text}");
}

#[test]
fn status_name_stopped_drops_the_pid_line() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", &env_toml("notepad"));
    let notepad = runtime(root.path(), "notepad");
    let mut state = RunState::running(
        RunMode::Detached,
        "notepad".to_owned(),
        4242,
        Some(4242),
        &Overrides::default(),
    );
    state.finish(RunStatus::Stopped);
    notepad.write_state(&state).unwrap();

    let result = output(wineo(root.path()).args(["status", "notepad"]));
    assert!(result.status.success(), "{}", stderr(&result));
    let text = stdout(&result);
    assert!(text.starts_with("notepad: stopped at "), "{text}");
    assert!(
        text.contains("\n  target     notepad (notepad)\n"),
        "{text}"
    );
    assert!(!text.contains("pid"), "{text}");
}

#[test]
fn status_name_lost_blames_the_run_that_never_finished() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", &env_toml("notepad"));
    // Recorded as running, but nothing holds the lock: the supervisor died.
    runtime(root.path(), "notepad")
        .write_state(&RunState::running(
            RunMode::Detached,
            "notepad".to_owned(),
            4242,
            Some(4242),
            &Overrides::default(),
        ))
        .unwrap();

    let result = output(wineo(root.path()).args(["status", "notepad"]));
    assert!(result.status.success(), "{}", stderr(&result));
    let text = stdout(&result);
    assert!(
        text.starts_with("notepad: failed, wineo lost track of the run started at "),
        "{text}"
    );
    assert!(!text.contains("pid"), "{text}");
}

#[test]
fn status_name_inactive_shows_only_runner_and_prefix() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", &env_toml("notepad"));

    let result = output(wineo(root.path()).args(["status", "notepad"]));
    assert!(result.status.success(), "{}", stderr(&result));
    let text = stdout(&result);
    assert!(text.starts_with("notepad: inactive\n"), "{text}");
    assert!(text.contains("\n  runner     wine"), "{text}");
    assert!(text.contains("\n  prefix     /p\n"), "{text}");
    assert!(!text.contains("target"), "{text}");
}

#[test]
fn status_name_unknown_environment_errors() {
    let root = tempfile::tempdir().unwrap();
    let result = output(wineo(root.path()).args(["status", "nope"]));
    assert!(!result.status.success());
    assert!(
        stderr(&result).contains("does not exist"),
        "{}",
        stderr(&result)
    );
}

const SHELLS: [&str; 5] = ["bash", "elvish", "fish", "powershell", "zsh"];
const STATIC_SHELLS: [&str; 2] = ["elvish", "powershell"];
const DYNAMIC_SHELLS: [&str; 3] = ["bash", "fish", "zsh"];

#[test]
fn static_completions_cover_every_visible_command_and_flag() {
    let root = tempfile::tempdir().unwrap();
    for shell in STATIC_SHELLS {
        let out = output(wineo(root.path()).args(["completions", shell]));
        assert!(out.status.success(), "{shell}: {}", stderr(&out));
        let script = stdout(&out);
        for command in [
            "new",
            "set",
            "setup",
            "config",
            "start",
            "stop",
            "restart",
            "status",
            "list",
            "logs",
            "run",
            "exec",
            "rm",
            "restore",
            "runners",
            "winetricks",
            "completions",
        ] {
            assert!(script.contains(command), "{shell} is missing {command}");
        }
        for flag in [
            "target",
            "global",
            "follow",
            "previous",
            "delete-prefix",
            "all",
        ] {
            let needle = format!("--{flag}");
            assert!(script.contains(&needle), "{shell} is missing {needle}");
        }
    }
}

#[test]
fn dynamic_completions_source_current_binary() {
    let root = tempfile::tempdir().unwrap();
    for shell in DYNAMIC_SHELLS {
        let out = output(wineo(root.path()).args(["completions", shell]));
        assert!(out.status.success(), "{shell}: {}", stderr(&out));
        let script = stdout(&out);
        assert!(
            script.contains(&format!("COMPLETE={shell}")),
            "{shell}: {script}"
        );
        assert!(script.contains("wineo"), "{shell}: {script}");
    }
}

#[test]
fn setup_is_in_the_help() {
    let root = tempfile::tempdir().unwrap();
    let help = output(wineo(root.path()).arg("--help"));
    assert!(
        stdout(&help)
            .lines()
            .any(|line| line.trim_start().starts_with("setup ")
                && line.contains("Pick, edit, and remove an environment's targets")),
        "{}",
        stdout(&help)
    );
    let help = output(wineo(root.path()).args(["setup", "--help"]));
    assert!(
        stdout(&help).contains("Environment to set up"),
        "{}",
        stdout(&help)
    );
}

#[test]
fn completions_leave_out_the_supervisor() {
    let root = tempfile::tempdir().unwrap();
    for shell in SHELLS {
        let script = stdout(&output(wineo(root.path()).args(["completions", shell])));
        assert!(
            !script.contains("__supervise"),
            "{shell} offers __supervise"
        );
    }
    let help = stdout(&output(wineo(root.path()).arg("--help")));
    assert!(!help.contains("__supervise"));
}

#[test]
fn completions_rejects_an_unknown_shell() {
    let root = tempfile::tempdir().unwrap();
    let out = output(wineo(root.path()).args(["completions", "csh"]));
    assert!(!out.status.success());
    assert!(stderr(&out).contains("csh"), "{}", stderr(&out));
}

/// A directory wine would recognize as a prefix, with a symlink out of it like
/// the `z:` drive has.
fn fake_prefix(dir: &Path, links_to: &Path) {
    fs::create_dir_all(dir.join("dosdevices")).unwrap();
    fs::create_dir_all(dir.join("drive_c")).unwrap();
    fs::write(
        dir.join("system.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n",
    )
    .unwrap();
    let link = dir.join("dosdevices/z:");
    if !link.exists() {
        std::os::unix::fs::symlink(links_to, &link).unwrap();
    }
}

fn environment_with_prefix(root: &Path, name: &str, prefix: &Path) {
    write_environment(
        root,
        name,
        &format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n",
            prefix.display()
        ),
    );
}

fn desktop_entry(root: &Path, environment: &str, target: &str, name: &str) -> PathBuf {
    let path = root
        .join("data/applications")
        .join(format!("wineo-{environment}-{target}.desktop"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!(
            "[Desktop Entry]\nName={name}\nX-Wineo-Environment={environment}\nX-Wineo-Target={target}\n"
        ),
    )
    .unwrap();
    path
}

fn locks(root: &Path) -> wineo_core::operation::Locks {
    wineo_core::operation::Locks::at(root.join("state/wineo/.locks"))
}

#[test]
fn rm_removes_the_config_and_runtime_state_but_keeps_the_prefix() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    fake_prefix(&prefix, root.path());
    environment_with_prefix(root.path(), "notepad", &prefix);
    write_log(root.path(), "notepad", "output.log", "old output\n");

    let out = output(wineo(root.path()).args(["rm", "notepad"]));
    assert!(out.status.success(), "{}", stderr(&out));
    let output = stdout(&out);
    let lines: Vec<_> = output.lines().map(str::to_owned).collect();
    let id = lines[0].split_whitespace().last().unwrap();
    assert_eq!(
        lines,
        [
            format!("Removed environment notepad to trash {id}"),
            format!("Prefix left at {}", prefix.display()),
            format!("Restore with: wineo restore {id}"),
            format!("To permanently delete, run: wineo prune {id} (the prefix stays)"),
        ]
    );
    assert!(
        !root
            .path()
            .join("config/wineo/environments/notepad.toml")
            .exists()
    );
    assert!(!runtime_dir(root.path(), "notepad").exists());
    assert!(prefix.join("system.reg").exists());
}

#[test]
fn rm_removes_an_environment_with_a_broken_config() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "broken", "this is not toml");
    let out = output(wineo(root.path()).args(["rm", "broken"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("Removed environment broken to trash "),
        "{}",
        stdout(&out)
    );
    assert!(
        !root
            .path()
            .join("config/wineo/environments/broken.toml")
            .exists()
    );
}

#[test]
fn rm_needs_exactly_one_name_and_removes_nothing_otherwise() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "one", MINIMAL_ENVIRONMENT);
    write_environment(root.path(), "two", MINIMAL_ENVIRONMENT);

    let out = output(wineo(root.path()).args(["rm", "one", "two"]));
    assert!(!out.status.success());
    assert!(
        root.path()
            .join("config/wineo/environments/one.toml")
            .exists()
    );
    assert!(
        root.path()
            .join("config/wineo/environments/two.toml")
            .exists()
    );

    let out = output(wineo(root.path()).arg("rm"));
    assert!(!out.status.success());
    assert!(
        root.path()
            .join("config/wineo/environments/one.toml")
            .exists()
    );
}

#[test]
fn rm_delete_prefix_deletes_the_prefix_without_following_its_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "keep me").unwrap();

    let prefix = root.path().join("prefixes/notepad");
    fake_prefix(&prefix, &outside);
    environment_with_prefix(root.path(), "notepad", &prefix);

    let out = output(wineo(root.path()).args(["rm", "notepad", "--delete-prefix"]));
    assert!(out.status.success(), "{}", stderr(&out));
    let output = stdout(&out);
    let id = output
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap();
    assert!(
        output.starts_with(&format!(
            "Removed environment notepad and its prefix to trash {id}\n"
        )),
        "{output}"
    );
    assert!(output.contains(&format!("Restore with: wineo restore {id}")));
    assert!(output.contains(&format!("To permanently delete, run: wineo prune {id}\n")));
    assert!(!prefix.exists());
    assert!(
        outside.join("keep.txt").exists(),
        "the link target was followed"
    );
}

#[test]
fn rm_delete_prefix_notes_a_prefix_that_is_already_gone() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    environment_with_prefix(root.path(), "notepad", &prefix);

    let out = output(wineo(root.path()).args(["rm", "notepad", "--delete-prefix"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("already gone"), "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("Removed environment notepad to trash "),
        "{}",
        stdout(&out)
    );
    assert!(
        !stdout(&out).contains("and its prefix to trash"),
        "{}",
        stdout(&out)
    );
    assert!(
        !root
            .path()
            .join("config/wineo/environments/notepad.toml")
            .exists()
    );
}

#[test]
fn rm_plain_notes_a_prefix_that_was_already_absent() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    environment_with_prefix(root.path(), "notepad", &prefix);

    let out = output(wineo(root.path()).args(["rm", "notepad"]));
    assert!(out.status.success(), "{}", stderr(&out));
    let output = stdout(&out);
    let id = output
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap();
    assert!(
        output.contains(&format!(
            "Prefix was already absent at {}",
            prefix.display()
        )),
        "{output}"
    );
    assert!(
        output.contains(&format!("To permanently delete, run: wineo prune {id}\n")),
        "{output}"
    );
    assert!(!output.contains("the prefix stays"), "{output}");
}

#[test]
fn rm_delete_prefix_refuses_a_directory_that_is_not_a_prefix() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    fs::create_dir_all(&prefix).unwrap();
    fs::write(prefix.join("notes.txt"), "not a prefix").unwrap();
    environment_with_prefix(root.path(), "notepad", &prefix);

    let out = output(wineo(root.path()).args(["rm", "notepad", "--delete-prefix"]));
    assert!(!out.status.success());
    let message = stderr(&out);
    assert!(message.contains("is not a wine prefix"), "{message}");
    assert!(message.contains("wineo rm notepad"), "{message}");
    assert!(prefix.join("notes.txt").exists());
    assert!(
        root.path()
            .join("config/wineo/environments/notepad.toml")
            .exists()
    );
}

#[test]
fn rm_delete_prefix_refuses_a_prefix_another_environment_uses() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/shared");
    fake_prefix(&prefix, root.path());
    environment_with_prefix(root.path(), "notepad", &prefix);
    // A different spelling of the same directory must still be recognized.
    environment_with_prefix(
        root.path(),
        "other",
        &root.path().join("prefixes/../prefixes/shared"),
    );

    let out = output(wineo(root.path()).args(["rm", "notepad", "--delete-prefix"]));
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("also used by other"),
        "{}",
        stderr(&out)
    );
    assert!(prefix.join("system.reg").exists());
    assert!(
        root.path()
            .join("config/wineo/environments/notepad.toml")
            .exists()
    );
}

#[test]
fn rm_delete_prefix_refuses_while_another_config_cannot_be_read() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    fake_prefix(&prefix, root.path());
    environment_with_prefix(root.path(), "notepad", &prefix);
    write_environment(root.path(), "broken", "this is not toml");

    let out = output(wineo(root.path()).args(["rm", "notepad", "--delete-prefix"]));
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("broken can't be read"),
        "{}",
        stderr(&out)
    );
    assert!(prefix.join("system.reg").exists());

    // Without the flag the unreadable neighbour is irrelevant.
    let out = output(wineo(root.path()).args(["rm", "notepad"]));
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn rm_refuses_while_an_operation_holds_the_environment() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    fake_prefix(&prefix, root.path());
    environment_with_prefix(root.path(), "notepad", &prefix);

    let held = locks(root.path())
        .acquire(
            wineo_core::operation::Operation::Winetricks,
            "notepad",
            &[&prefix],
        )
        .unwrap();
    let out = output(wineo(root.path()).args(["rm", "notepad"]));
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("winetricks is running"),
        "{}",
        stderr(&out)
    );
    assert!(
        root.path()
            .join("config/wineo/environments/notepad.toml")
            .exists()
    );

    drop(held);
    assert!(
        output(wineo(root.path()).args(["rm", "notepad"]))
            .status
            .success()
    );
}

#[test]
fn rm_refuses_a_running_environment() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    let notepad = runtime(root.path(), "notepad");
    let _lock = notepad.try_lock().unwrap().unwrap();

    let out = output(wineo(root.path()).args(["rm", "notepad"]));
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("is running, stop it first"),
        "{}",
        stderr(&out)
    );
    assert!(
        root.path()
            .join("config/wineo/environments/notepad.toml")
            .exists()
    );
}

#[test]
fn rm_refuses_a_lost_run() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    // Recorded as running with nothing holding the lock, so wine may still be up.
    runtime(root.path(), "notepad")
        .write_state(&RunState::running(
            RunMode::Detached,
            "notepad".to_owned(),
            111,
            Some(111),
            &Overrides::default(),
        ))
        .unwrap();

    let out = output(wineo(root.path()).args(["rm", "notepad"]));
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("wineo stop notepad"),
        "{}",
        stderr(&out)
    );
    assert!(
        root.path()
            .join("config/wineo/environments/notepad.toml")
            .exists()
    );
}

#[test]
fn rm_delete_prefix_refuses_unreadable_run_state() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    fake_prefix(&prefix, root.path());
    environment_with_prefix(root.path(), "notepad", &prefix);
    write_log(root.path(), "notepad", "state.toml", "not = valid = toml\n");

    let out = output(wineo(root.path()).args(["rm", "notepad", "--delete-prefix"]));
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("wineo stop notepad"),
        "{}",
        stderr(&out)
    );
    assert!(prefix.join("system.reg").exists());

    // The unreadable run state blocks removing the registration too.
    let out = output(wineo(root.path()).args(["rm", "notepad"]));
    assert!(!out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("wineo stop notepad"),
        "{}",
        stderr(&out)
    );
    assert!(prefix.join("system.reg").exists());

    fs::remove_file(root.path().join("state/wineo/notepad/state.toml")).unwrap();
    let out = output(wineo(root.path()).args(["rm", "notepad"]));
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_partly_deleted_prefix_keeps_the_config_and_explains_the_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    fake_prefix(&prefix, root.path());
    environment_with_prefix(root.path(), "notepad", &prefix);

    // What is left when a delete failed after removing system.reg.
    fs::remove_file(prefix.join("system.reg")).unwrap();
    let out = output(wineo(root.path()).args(["rm", "notepad", "--delete-prefix"]));
    assert!(!out.status.success());
    let message = stderr(&out);
    assert!(message.contains("is not a wine prefix"), "{message}");
    assert!(message.contains("wineo rm notepad"), "{message}");
    assert!(
        root.path()
            .join("config/wineo/environments/notepad.toml")
            .exists()
    );

    // The registration still comes off once the leftovers are dealt with.
    fs::remove_dir_all(&prefix).unwrap();
    let out = output(wineo(root.path()).args(["rm", "notepad"]));
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn rm_delete_prefix_refuses_a_prefix_path_that_is_a_symlink() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("prefixes/real");
    fake_prefix(&real, root.path());
    let link = root.path().join("prefixes/link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    environment_with_prefix(root.path(), "notepad", &link);

    let out = output(wineo(root.path()).args(["rm", "notepad", "--delete-prefix"]));
    assert!(!out.status.success());
    assert!(stderr(&out).contains("is a symlink"), "{}", stderr(&out));
    assert!(real.join("system.reg").exists());
    assert!(link.exists());
}

#[test]
fn rm_errors_for_an_unknown_environment() {
    let root = tempfile::tempdir().unwrap();
    let out = output(wineo(root.path()).args(["rm", "nope"]));
    assert!(!out.status.success());
    assert!(stderr(&out).contains("does not exist"), "{}", stderr(&out));
}

/// Prints everything wineo is supposed to hand winetricks, then exits.
const REPORTING_WINETRICKS: &str = r#"#!/bin/sh
echo "args: $*"
echo "WINEPREFIX=$WINEPREFIX"
echo "WINE=$WINE"
echo "WINESERVER=$WINESERVER"
echo "WINEDLLOVERRIDES=$WINEDLLOVERRIDES"
"#;

fn write_script(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A PATH with fake wine, wineserver, and winetricks, so these tests don't
/// need any of them installed.
fn fake_tools(root: &Path, winetricks: Option<&str>) -> PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for name in ["wine", "wineserver"] {
        write_script(&bin.join(name), "#!/bin/sh\nexit 0\n");
    }
    if let Some(script) = winetricks {
        write_script(&bin.join("winetricks"), script);
    }
    bin
}

/// An environment with a usable prefix and a PATH holding the fake tools.
fn winetricks_fixture(root: &Path, config: &str, script: Option<&str>) -> Command {
    let prefix = root.join("prefixes/notepad");
    fake_prefix(&prefix, root);
    write_environment(
        root,
        "notepad",
        &format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n{config}",
            prefix.display()
        ),
    );
    let bin = fake_tools(root, script);
    let mut command = wineo(root);
    // Ahead of the real PATH, so the fakes win but the scripts can still reach
    // ordinary tools like `touch` and `sleep`.
    command.env("PATH", path_ahead_of_system(&bin));
    command
}

fn path_ahead_of_system(bin: &Path) -> std::ffi::OsString {
    let system = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(std::iter::once(bin.to_owned()).chain(std::env::split_paths(&system)))
        .unwrap()
}

#[test]
fn winetricks_runs_with_the_environments_prefix_and_runner() {
    let root = tempfile::tempdir().unwrap();
    let out = output(
        winetricks_fixture(root.path(), "", Some(REPORTING_WINETRICKS)).args([
            "winetricks",
            "notepad",
            "-q",
            "dotnet48",
        ]),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    assert!(printed.contains("args: -q dotnet48"), "{printed}");
    assert!(
        printed.contains(&format!(
            "WINEPREFIX={}",
            root.path().join("prefixes/notepad").display()
        )),
        "{printed}"
    );
    // Absolute, so a different PATH can't swap the runner out from under wine.
    assert!(
        printed.contains(&format!("WINE={}", root.path().join("bin/wine").display())),
        "{printed}"
    );
    assert!(
        printed.contains(&format!(
            "WINESERVER={}",
            root.path().join("bin/wineserver").display()
        )),
        "{printed}"
    );
    assert!(printed.contains("winemenubuilder.exe=d"), "{printed}");
}

#[test]
fn winetricks_reconciles_desktop_entries_after_it_finishes() {
    let root = tempfile::tempdir().unwrap();
    let out = output(
        winetricks_fixture(
            root.path(),
            "[targets.app]\nexe = \"app\"\ndesktop = true\n",
            Some("#!/bin/sh\nexit 0\n"),
        )
        .args(["winetricks", "notepad", "list-installed"]),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        root.path()
            .join("data/applications/wineo-notepad-app.desktop")
            .is_file()
    );
}

#[test]
fn winetricks_replaces_inherited_managed_variables() {
    let root = tempfile::tempdir().unwrap();
    let out = output(
        winetricks_fixture(root.path(), "", Some(REPORTING_WINETRICKS))
            .env("WINEPREFIX", "/inherited/prefix")
            .env("WINE", "/inherited/wine")
            .env("WINESERVER", "/inherited/wineserver")
            .args(["winetricks", "notepad"]),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    assert!(!printed.contains("/inherited/"), "{printed}");
}

#[test]
fn winetricks_rejects_explicitly_set_managed_variables() {
    let root = tempfile::tempdir().unwrap();
    for key in ["WINE", "WINESERVER"] {
        let out = output(
            winetricks_fixture(root.path(), "", Some(REPORTING_WINETRICKS)).args([
                "winetricks",
                "-e",
                &format!("{key}=/somewhere/else"),
                "notepad",
            ]),
        );
        assert!(!out.status.success(), "{key} was accepted");
        assert!(
            stderr(&out).contains("managed by wineo"),
            "{}",
            stderr(&out)
        );
    }
    // WINEPREFIX is refused everywhere, so it fails while the flag is parsed.
    let out = output(
        winetricks_fixture(root.path(), "", Some(REPORTING_WINETRICKS)).args([
            "winetricks",
            "-e",
            "WINEPREFIX=/somewhere/else",
            "notepad",
        ]),
    );
    assert!(!out.status.success());

    let root = tempfile::tempdir().unwrap();
    let out = output(
        winetricks_fixture(
            root.path(),
            "\n[env]\nWINE = \"/somewhere/else\"\n",
            Some(REPORTING_WINETRICKS),
        )
        .args(["winetricks", "notepad"]),
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("managed by wineo"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn winetricks_keeps_configured_and_command_line_dll_overrides() {
    let root = tempfile::tempdir().unwrap();
    let out = output(
        winetricks_fixture(
            root.path(),
            "\n[env]\nWINEDLLOVERRIDES = \"foo.dll=n\"\n",
            Some(REPORTING_WINETRICKS),
        )
        .args(["winetricks", "notepad"]),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("WINEDLLOVERRIDES=foo.dll=n;winemenubuilder.exe=d"),
        "{}",
        stdout(&out)
    );

    let root = tempfile::tempdir().unwrap();
    let out = output(
        winetricks_fixture(root.path(), "", Some(REPORTING_WINETRICKS)).args([
            "winetricks",
            "-e",
            "WINEDLLOVERRIDES=bar.dll=n",
            "notepad",
        ]),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("WINEDLLOVERRIDES=bar.dll=n;winemenubuilder.exe=d"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn winetricks_forwards_everything_after_the_name() {
    let root = tempfile::tempdir().unwrap();
    for (passed, expected) in [
        (vec!["winetricks", "notepad"], "args: "),
        (vec!["winetricks", "notepad", "-h"], "args: -h"),
        (vec!["winetricks", "notepad", "--", "-h"], "args: -h"),
        (
            vec!["winetricks", "notepad", "-q", "--force", "dotnet48"],
            "args: -q --force dotnet48",
        ),
        // wineo's own -e goes before the name, winetricks' own goes after.
        (
            vec!["winetricks", "-e", "A=1", "notepad", "-e", "shell32"],
            "args: -e shell32",
        ),
    ] {
        let out =
            output(winetricks_fixture(root.path(), "", Some(REPORTING_WINETRICKS)).args(&passed));
        assert!(out.status.success(), "{passed:?}: {}", stderr(&out));
        let first = stdout(&out).lines().next().unwrap_or_default().to_owned();
        assert_eq!(first.trim_end(), expected.trim_end(), "{passed:?}");
    }
}

#[test]
fn winetricks_help_describes_the_wrapper() {
    let root = tempfile::tempdir().unwrap();
    let out = output(
        winetricks_fixture(root.path(), "", Some(REPORTING_WINETRICKS))
            .args(["winetricks", "--help"]),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    assert!(!printed.contains("args:"), "{printed}");
    assert!(printed.contains("--env"), "{printed}");
}

#[test]
fn winetricks_passes_its_exit_code_through() {
    let root = tempfile::tempdir().unwrap();
    let out = output(
        winetricks_fixture(root.path(), "", Some("#!/bin/sh\nexit 3\n"))
            .args(["winetricks", "notepad"]),
    );
    assert_eq!(out.status.code(), Some(3));

    let root = tempfile::tempdir().unwrap();
    let out = output(
        winetricks_fixture(root.path(), "", Some("#!/bin/sh\nkill -TERM $$\n"))
            .args(["winetricks", "notepad"]),
    );
    assert_eq!(out.status.code(), Some(143), "{}", stderr(&out));
}

#[test]
fn winetricks_needs_winetricks_on_path() {
    let root = tempfile::tempdir().unwrap();
    let out = output(
        winetricks_fixture(root.path(), "", None)
            .env("PATH", root.path().join("bin"))
            .args(["winetricks", "notepad"]),
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("`winetricks` was not found on PATH"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn winetricks_refuses_a_missing_prefix() {
    let root = tempfile::tempdir().unwrap();
    let bin = fake_tools(root.path(), Some(REPORTING_WINETRICKS));
    write_environment(
        root.path(),
        "notepad",
        &format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n",
            root.path().join("gone/prefix").display()
        ),
    );
    let out = output(
        wineo(root.path())
            .env("PATH", path_ahead_of_system(&bin))
            .args(["winetricks", "notepad"]),
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(&format!(
            "is missing, and so is {}",
            root.path().join("gone").display()
        )),
        "{}",
        stderr(&out)
    );
    assert!(!root.path().join("gone").exists());
}

#[test]
fn winetricks_refuses_a_running_environment() {
    let root = tempfile::tempdir().unwrap();
    let mut command = winetricks_fixture(root.path(), "", Some(REPORTING_WINETRICKS));
    let _lock = runtime(root.path(), "notepad").try_lock().unwrap().unwrap();
    let out = output(command.args(["winetricks", "notepad"]));
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("stop it before running winetricks"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_held_environment_is_reported_as_busy_everywhere() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    fake_prefix(&prefix, root.path());
    environment_with_prefix(root.path(), "notepad", &prefix);

    let held = locks(root.path())
        .acquire(
            wineo_core::operation::Operation::Winetricks,
            "notepad",
            &[&prefix],
        )
        .unwrap();

    let listed = stdout(&output(wineo(root.path()).arg("list")));
    assert!(listed.contains("winetricks"), "{listed}");

    let running = stdout(&output(wineo(root.path()).arg("status")));
    assert!(running.contains("winetricks"), "{running}");

    let detail = stdout(&output(wineo(root.path()).args(["status", "notepad"])));
    assert!(detail.contains("busy"), "{detail}");
    assert!(detail.contains("winetricks is running"), "{detail}");

    let out = output(wineo(root.path()).args(["stop", "notepad"]));
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Finish or cancel it in its terminal"),
        "{}",
        stderr(&out)
    );
    drop(held);
}

#[test]
fn winetricks_holds_the_environment_until_it_finishes() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("started");
    let release = root.path().join("release");
    let script = "#!/bin/sh\ntouch \"$MARKER\"\nwhile [ ! -e \"$RELEASE\" ]; do sleep 0.05; done\n";

    let mut child = winetricks_fixture(root.path(), "", Some(script))
        .env("MARKER", &marker)
        .env("RELEASE", &release)
        .args(["winetricks", "notepad"])
        .spawn()
        .unwrap();
    wait_for("winetricks to start", || marker.exists());

    let out = output(wineo(root.path()).args(["rm", "notepad"]));
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("winetricks is running"),
        "{}",
        stderr(&out)
    );

    fs::write(&release, "").unwrap();
    assert!(child.wait().unwrap().success());
    // The lock is gone once it has exited and the prefix is idle.
    assert!(
        output(wineo(root.path()).args(["rm", "notepad"]))
            .status
            .success()
    );
}

#[test]
fn canceling_winetricks_cleans_up_before_letting_go() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("started");
    let script = "#!/bin/sh\ntouch \"$MARKER\"\nwhile true; do sleep 0.05; done\n";

    // Its own process group, so the interrupt reaches wineo and winetricks
    // together, the way Ctrl+C in a terminal would.
    let child = winetricks_fixture(root.path(), "", Some(script))
        .env("MARKER", &marker)
        .args(["winetricks", "notepad"])
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    wait_for("winetricks to start", || marker.exists());

    let group = format!("-{}", child.id());
    assert!(
        Command::new("kill")
            .args(["-INT", &group])
            .status()
            .unwrap()
            .success()
    );

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(130), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("Canceling winetricks"),
        "{}",
        stderr(&out)
    );
    // Exclusivity is only released once the prefix has settled.
    assert!(
        output(wineo(root.path()).args(["rm", "notepad"]))
            .status
            .success()
    );
}

#[test]
#[ignore = "runs wine and winetricks"]
fn winetricks_runs_against_a_real_prefix() {
    let root = tempfile::tempdir().unwrap();
    let created = output(wineo(root.path()).args(["new", "-n", "notepad"]));
    assert!(created.status.success(), "{}", stderr(&created));

    let out = output(wineo(root.path()).args(["winetricks", "notepad", "list-installed"]));
    assert!(out.status.success(), "{}", stderr(&out));
    // Nothing is left holding it once winetricks has finished.
    assert!(
        output(wineo(root.path()).args(["rm", "notepad", "--delete-prefix"]))
            .status
            .success()
    );
}

#[test]
fn an_operation_blocks_starting_running_and_exec() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    fake_prefix(&prefix, root.path());
    write_environment(
        root.path(),
        "notepad",
        &format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n\n[targets.notepad]\nexe = \"notepad\"\n",
            prefix.display()
        ),
    );
    let bin = fake_tools(root.path(), Some(REPORTING_WINETRICKS));

    let held = locks(root.path())
        .acquire(
            wineo_core::operation::Operation::Winetricks,
            "notepad",
            &[&prefix],
        )
        .unwrap();
    for passed in [
        vec!["start", "notepad"],
        vec!["run", "notepad"],
        vec!["exec", "notepad", "winecfg"],
    ] {
        let out = output(
            wineo(root.path())
                .env("PATH", path_ahead_of_system(&bin))
                .args(&passed),
        );
        assert!(!out.status.success(), "{passed:?} was allowed");
        assert!(
            stderr(&out).contains("winetricks is running"),
            "{passed:?}: {}",
            stderr(&out)
        );
    }
    drop(held);
}

#[test]
fn an_exec_can_share_an_environment_with_a_run() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    fake_prefix(&prefix, root.path());
    environment_with_prefix(root.path(), "notepad", &prefix);
    let bin = fake_tools(root.path(), Some(REPORTING_WINETRICKS));

    // What a run holds while its app is up.
    let running = locks(root.path())
        .acquire(
            wineo_core::operation::Operation::Launch,
            "notepad",
            &[&prefix],
        )
        .unwrap();
    let out = output(
        wineo(root.path())
            .env("PATH", path_ahead_of_system(&bin))
            .args(["exec", "notepad", "winecfg"]),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    drop(running);
}

#[test]
fn a_removal_blocks_adopting_or_pointing_at_the_same_prefix() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/shared");
    fake_prefix(&prefix, root.path());
    environment_with_prefix(root.path(), "notepad", &prefix);
    environment_with_prefix(root.path(), "other", &root.path().join("prefixes/other"));
    let bin = fake_tools(root.path(), Some(REPORTING_WINETRICKS));

    let held = locks(root.path())
        .acquire(
            wineo_core::operation::Operation::Remove,
            "notepad",
            &[&prefix],
        )
        .unwrap();

    // Adopting it under a new name, and pointing an existing one at it, both
    // have to wait: the removal may be about to delete it.
    let adopt = output(
        wineo(root.path())
            .env("PATH", path_ahead_of_system(&bin))
            .args(["new", "-n", "adopted", "--dir", prefix.to_str().unwrap()]),
    );
    assert!(!adopt.status.success());
    assert!(
        stderr(&adopt).contains("being removed"),
        "{}",
        stderr(&adopt)
    );
    assert!(
        !root
            .path()
            .join("config/wineo/environments/adopted.toml")
            .exists()
    );

    let repoint =
        output(wineo(root.path()).args(["set", "other", &format!("prefix={}", prefix.display())]));
    assert!(!repoint.status.success());
    assert!(
        stderr(&repoint).contains("being removed"),
        "{}",
        stderr(&repoint)
    );
    drop(held);
}

#[test]
fn lock_files_outlive_the_environment_they_coordinate() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/notepad");
    fake_prefix(&prefix, root.path());
    environment_with_prefix(root.path(), "notepad", &prefix);
    write_log(root.path(), "notepad", "output.log", "old output\n");

    assert!(
        output(wineo(root.path()).args(["rm", "notepad"]))
            .status
            .success()
    );
    // The runtime dir is gone, but the locks it never owned are still there.
    assert!(!runtime_dir(root.path(), "notepad").exists());
    let lock = root.path().join("state/wineo/.locks/name-notepad.lock");
    assert!(lock.is_file(), "{} is missing", lock.display());
}

const RESET_NOTE: &str = "wineo: the log passed 1 MB; all output before this reset was dropped";

/// Reads a child's stdout line by line on its own thread, so a test waits for
/// what the follower actually printed instead of guessing a delay.
struct Lines {
    lines: std::sync::mpsc::Receiver<String>,
}

impl Lines {
    fn from(stdout: std::process::ChildStdout) -> Self {
        let (sender, lines) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self { lines }
    }

    fn next(&self, what: &str) -> String {
        self.lines
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|err| panic!("waiting for {what}: {err}"))
    }
}

fn open_log(path: &Path) -> fs::File {
    fs::OpenOptions::new().append(true).open(path).unwrap()
}

#[test]
fn logs_follow_starts_over_when_the_log_is_cleared() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    let path = write_log(root.path(), "notepad", "output.log", "first\n");
    let log = wineo_core::log::Log::new(path.clone());
    let lock = Runtime::at(runtime_dir(root.path(), "notepad"))
        .try_lock()
        .unwrap()
        .unwrap();

    let mut child = wineo(root.path())
        .args(["logs", "notepad", "-f"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let lines = Lines::from(child.stdout.take().unwrap());
    assert_eq!(lines.next("the first line"), "first");

    // The follower is six bytes in. Clearing and regrowing past that offset
    // must not leave it reading from the middle of the new output.
    let mut file = open_log(&path);
    for round in ["second", "third"] {
        log.clear(&file, RESET_NOTE).unwrap();
        writeln!(file, "{round}").unwrap();
        assert_eq!(lines.next("the reset note"), RESET_NOTE);
        assert_eq!(lines.next("output after the reset"), round);
    }

    // A reset as the run ends is still delivered.
    log.clear(&file, RESET_NOTE).unwrap();
    writeln!(file, "last").unwrap();
    drop(file);
    drop(lock);

    assert_eq!(lines.next("the final reset note"), RESET_NOTE);
    assert_eq!(lines.next("the final line"), "last");
    wait_for("wineo logs to notice the run ended", || {
        matches!(child.try_wait(), Ok(Some(_)))
    });
    assert!(child.wait_with_output().unwrap().status.success());
}

#[test]
fn logs_n_tolerates_a_reset_while_it_reads() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    let path = write_log(root.path(), "notepad", "output.log", "");
    let log = wineo_core::log::Log::new(path.clone());

    let stop = Arc::new(AtomicBool::new(false));
    let churn = {
        let (stop, path) = (stop.clone(), path.clone());
        thread::spawn(move || {
            let mut file = open_log(&path);
            while !stop.load(Ordering::Relaxed) {
                for line in 0..200 {
                    writeln!(file, "line {line}").unwrap();
                }
                log.clear(&file, RESET_NOTE).unwrap();
            }
        })
    };

    for _ in 0..20 {
        let out = output(wineo(root.path()).args(["logs", "notepad", "-n", "5"]));
        assert!(out.status.success(), "{}", stderr(&out));
    }
    stop.store(true, Ordering::Relaxed);
    churn.join().unwrap();
}

#[test]
fn logs_previous_is_never_treated_as_resettable() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "notepad", MINIMAL_ENVIRONMENT);
    write_log(root.path(), "notepad", "output.log.1", "older run\n");
    // A counter left over from the run that wrote it must not be consulted.
    write_log(root.path(), "notepad", "output.log.reset", "7\n");

    let out = output(wineo(root.path()).args(["logs", "notepad", "--previous"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "older run\n");
}

/// A DXVK or vkd3d build of stub DLLs, extracted into its drop-in root.
fn install_build(root: &Path, component: &str, name: &str, dirs: &[&str]) {
    let marker = if component == "dxvk" {
        "dxgi.dll"
    } else {
        "d3d12.dll"
    };
    let dir = root.join("data/wineo").join(component).join(name);
    fs::create_dir_all(&dir).unwrap();
    for arch in dirs {
        let arch = dir.join(arch);
        fs::create_dir_all(&arch).unwrap();
        fs::write(arch.join(marker), "").unwrap();
    }
}

#[test]
fn dxvk_and_vkd3d_list_what_is_installed() {
    let root = tempfile::tempdir().unwrap();

    // Nothing installed says where to extract to, and lists nothing.
    for component in ["dxvk", "vkd3d"] {
        let out = output(wineo(root.path()).arg(component));
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(stdout(&out), "");
        let message = stderr(&out);
        assert!(
            message.contains(&format!("No {component} builds")),
            "{message}"
        );
        assert!(
            message.contains(&format!("data/wineo/{component}")),
            "{message}"
        );
    }

    install_build(root.path(), "dxvk", "dxvk-2.7", &["x64", "x32"]);
    install_build(root.path(), "dxvk", "dxvk-2.10", &["x64", "x32"]);
    install_build(root.path(), "vkd3d", "2.14", &["x64", "x86"]);

    let listed = stdout(&output(wineo(root.path()).arg("dxvk")));
    assert_eq!(listed, "dxvk-2.10\ndxvk-2.7\n");
    assert_eq!(stdout(&output(wineo(root.path()).arg("vkd3d"))), "2.14\n");
    // Each root is its own: a DXVK build is not a vkd3d one.
    assert!(!stdout(&output(wineo(root.path()).arg("vkd3d"))).contains("dxvk"));
}

#[test]
fn dxvk_all_says_why_a_directory_was_skipped() {
    let root = tempfile::tempdir().unwrap();
    install_build(root.path(), "dxvk", "2.7", &["x64", "x32"]);
    install_build(root.path(), "dxvk", "half", &["x64"]);
    install_build(root.path(), "dxvk", "latest", &["x64", "x32"]);
    install_build(root.path(), "dxvk", "bad name", &["x64", "x32"]);

    let plain = stdout(&output(wineo(root.path()).arg("dxvk")));
    assert_eq!(plain, "2.7\n");

    let all = stdout(&output(wineo(root.path()).args(["dxvk", "--all"])));
    assert!(all.contains("half"), "{all}");
    assert!(all.contains("x32/dxgi.dll"), "{all}");
    assert!(all.contains("latest"), "{all}");
    assert!(all.contains("reserved"), "{all}");
    assert!(all.contains("bad name"), "{all}");
    assert!(all.contains("not a usable name"), "{all}");
}

#[test]
fn the_component_roots_can_be_moved() {
    let root = tempfile::tempdir().unwrap();
    let elsewhere = root.path().join("my-dxvk");
    fs::create_dir_all(elsewhere.join("2.7/x64")).unwrap();
    fs::create_dir_all(elsewhere.join("2.7/x32")).unwrap();
    for arch in ["x64", "x32"] {
        fs::write(elsewhere.join("2.7").join(arch).join("dxgi.dll"), "").unwrap();
    }

    // Not in the default root, so it is invisible until the setting moves.
    assert_eq!(stdout(&output(wineo(root.path()).arg("dxvk"))), "");
    let set = output(wineo(root.path()).args([
        "set",
        "--global",
        &format!("defaults.dxvk_root={}", elsewhere.display()),
    ]));
    assert!(set.status.success(), "{}", stderr(&set));
    assert_eq!(stdout(&output(wineo(root.path()).arg("dxvk"))), "2.7\n");

    // vkd3d has its own root and is unaffected.
    let vkd3d = output(wineo(root.path()).arg("vkd3d"));
    assert_eq!(stdout(&vkd3d), "");
    assert!(
        stderr(&vkd3d).contains("data/wineo/vkd3d"),
        "{}",
        stderr(&vkd3d)
    );

    assert!(
        output(wineo(root.path()).args(["set", "--global", "--unset", "defaults.dxvk_root"]))
            .status
            .success()
    );
    assert_eq!(stdout(&output(wineo(root.path()).arg("dxvk"))), "");
}

/// A prefix with a `c:` drive, so `wineo programs` can resolve Windows paths.
fn scannable_prefix(dir: &Path) {
    fake_prefix(dir, dir);
    std::os::unix::fs::symlink("../drive_c", dir.join("dosdevices/c:")).unwrap();
}

fn put(dir: &Path, path: &str, contents: &[u8]) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A `.lnk` pointing at `target`, with the one field a scan reads.
fn lnk(target: &str) -> Vec<u8> {
    fn set(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut info = vec![0u8; 0x1C];
    let base_at = u32::try_from(info.len()).unwrap();
    info.extend(target.bytes());
    info.push(0);
    let suffix_at = u32::try_from(info.len()).unwrap();
    info.push(0);
    let size = u32::try_from(info.len()).unwrap();
    set(&mut info, 0, size);
    set(&mut info, 4, 0x1C);
    set(&mut info, 8, 1);
    set(&mut info, 16, base_at);
    set(&mut info, 24, suffix_at);

    let mut out = vec![0u8; 0x4C];
    set(&mut out, 0, 0x4C);
    out[4..20].copy_from_slice(&[
        0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
    ]);
    set(&mut out, 20, 2);
    out.extend(info);
    out
}

const START_MENU: &str = "drive_c/ProgramData/Microsoft/Windows/Start Menu/Programs";

#[test]
fn programs_lists_shortcuts_and_executables() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/hades");
    scannable_prefix(&prefix);
    environment_with_prefix(root.path(), "hades", &prefix);
    put(
        &prefix,
        &format!("{START_MENU}/Hades.lnk"),
        &lnk(r"C:\Program Files\Hades\Hades.exe"),
    );
    put(&prefix, "drive_c/Program Files/Hades/Hades.exe", b"");
    put(&prefix, "drive_c/Program Files/Hades/Helper.exe", b"");
    put(&prefix, "drive_c/Program Files/Hades/unins000.exe", b"");

    let out = output(wineo(root.path()).args(["programs", "hades"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        concat!(
            "shortcut  Hades   C:\\Program Files\\Hades\\Hades.exe\n",
            "exe       Helper  C:\\Program Files\\Hades\\Helper.exe\n",
        )
    );
    // The uninstaller is held back, and the hint names a real program.
    assert!(
        stderr(&out).contains("1 executable looked like noise"),
        "{}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains(r"targets.<target>.exe='C:\Program Files\Hades\Hades.exe'"),
        "{}",
        stderr(&out)
    );

    let all = output(wineo(root.path()).args(["programs", "hades", "--all"]));
    assert!(stdout(&all).contains("unins000"), "{}", stdout(&all));
    assert!(
        !stderr(&all).contains("looked like noise"),
        "{}",
        stderr(&all)
    );
}

#[test]
fn programs_says_when_nothing_is_installed() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/empty");
    scannable_prefix(&prefix);
    environment_with_prefix(root.path(), "empty", &prefix);

    let out = output(wineo(root.path()).args(["programs", "empty"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(
        stderr(&out).contains("Nothing is installed in empty"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn programs_explains_a_shortcut_it_cannot_read() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/hades");
    scannable_prefix(&prefix);
    environment_with_prefix(root.path(), "hades", &prefix);
    put(&prefix, &format!("{START_MENU}/Broken.lnk"), b"junk");

    let out = output(wineo(root.path()).args(["programs", "hades"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("Skipped the shortcut Broken: not a shortcut"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn programs_needs_an_environment_with_a_prefix() {
    let root = tempfile::tempdir().unwrap();
    let missing = output(wineo(root.path()).args(["programs", "nope"]));
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("environment `nope` does not exist"),
        "{}",
        stderr(&missing)
    );

    // A config pointing at a directory that is not a prefix.
    environment_with_prefix(root.path(), "gone", &root.path().join("nowhere"));
    let gone = output(wineo(root.path()).args(["programs", "gone"]));
    assert!(!gone.status.success());
    assert!(
        stderr(&gone).contains("is missing or is not a wine prefix"),
        "{}",
        stderr(&gone)
    );
}

/// A Proton environment keeps its prefix in `pfx/`, and the scan has to find it
/// without resolving the runner, which may not be installed any more.
#[test]
fn programs_finds_a_proton_prefix() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/game");
    scannable_prefix(&prefix.join("pfx"));
    write_environment(
        root.path(),
        "game",
        &format!(
            "prefix = \"{}\"\nrunner = \"GE-Proton\"\narch = \"win64\"\n",
            prefix.display()
        ),
    );
    put(
        &prefix.join("pfx"),
        &format!("{START_MENU}/Game.lnk"),
        &lnk(r"C:\Game\game.exe"),
    );

    let out = output(wineo(root.path()).args(["programs", "game"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "shortcut  Game  C:\\Game\\game.exe\n");
}

#[test]
fn set_desktop_writes_and_unset_removes_an_entry() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/app");
    scannable_prefix(&prefix);
    write_environment(
        root.path(),
        "app",
        &format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n\n[targets.app]\nexe = \"app\"\n",
            prefix.display()
        ),
    );
    let bin = fake_tools(root.path(), None);

    let mut set = wineo(root.path());
    set.env("PATH", path_ahead_of_system(&bin))
        .args(["set", "app", "targets.app.desktop=true"]);
    let out = output(&mut set);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("Added a desktop entry for app in app"));

    let path = root.path().join("data/applications/wineo-app-app.desktop");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        concat!(
            "[Desktop Entry]\n",
            "Type=Application\n",
            "Name=app\n",
            "Comment=Runs in the wineo environment app\n",
            "Exec=wineo start app --target app\n",
            "Terminal=false\n",
            "Categories=Utility;Software;\n",
            "StartupWMClass=app.exe\n",
            "X-Wineo-Environment=app\n",
            "X-Wineo-Target=app\n",
        )
    );

    let mut unset = wineo(root.path());
    unset.env("PATH", path_ahead_of_system(&bin)).args([
        "set",
        "app",
        "--unset",
        "targets.app.desktop",
    ]);
    let out = output(&mut unset);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!path.exists());
}

#[test]
fn set_desktop_rejects_a_missing_windows_executable() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/app");
    scannable_prefix(&prefix);
    write_environment(
        root.path(),
        "app",
        &format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n\n[targets.app]\nexe = 'C:\\Program Files\\App\\App.exe'\n",
            prefix.display()
        ),
    );
    let bin = fake_tools(root.path(), None);

    let mut set = wineo(root.path());
    set.env("PATH", path_ahead_of_system(&bin))
        .args(["set", "app", "targets.app.desktop=true"]);
    let out = output(&mut set);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("`C:\\Program Files\\App\\App.exe` does not exist"));
    let config =
        fs::read_to_string(root.path().join("config/wineo/environments/app.toml")).unwrap();
    assert!(!config.contains("desktop"), "{config}");
    assert!(
        !root
            .path()
            .join("data/applications/wineo-app-app.desktop")
            .exists()
    );
}

#[test]
fn unsetting_a_target_removes_its_desktop_entry() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefixes/app");
    scannable_prefix(&prefix);
    write_environment(
        root.path(),
        "app",
        &format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n\n[targets.app]\nexe = \"app\"\n",
            prefix.display()
        ),
    );
    let bin = fake_tools(root.path(), None);

    let mut set = wineo(root.path());
    set.env("PATH", path_ahead_of_system(&bin))
        .args(["set", "app", "targets.app.desktop=true"]);
    let out = output(&mut set);
    assert!(out.status.success(), "{}", stderr(&out));
    let path = root.path().join("data/applications/wineo-app-app.desktop");
    assert!(path.exists());

    let mut remove = wineo(root.path());
    remove
        .env("PATH", path_ahead_of_system(&bin))
        .args(["set", "app", "--unset", "targets.app"]);
    let out = output(&mut remove);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!path.exists());
    let config =
        fs::read_to_string(root.path().join("config/wineo/environments/app.toml")).unwrap();
    assert!(!config.contains("targets.app"), "{config}");
}

#[test]
fn unsetting_desktop_removes_an_entry_when_the_prefix_is_missing() {
    let root = tempfile::tempdir().unwrap();
    write_environment(
        root.path(),
        "app",
        "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n\n[targets.app]\nexe = \"app\"\ndesktop = true\n",
    );
    let path = desktop_entry(root.path(), "app", "app", "app");
    let bin = fake_tools(root.path(), None);

    let mut unset = wineo(root.path());
    unset.env("PATH", path_ahead_of_system(&bin)).args([
        "set",
        "app",
        "--unset",
        "targets.app.desktop",
    ]);
    let out = output(&mut unset);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!path.exists());
}

#[test]
fn desktop_lists_entries_and_marks_strays() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "known", MINIMAL_ENVIRONMENT);
    desktop_entry(root.path(), "known", "game", "Game");
    let stray = desktop_entry(root.path(), "gone", "old", "Old");
    fs::write(
        root.path().join("data/applications/wineo-ignored.desktop"),
        "Name=ignored\n",
    )
    .unwrap();

    let out = output(wineo(root.path()).arg("desktop"));
    assert!(out.status.success(), "{}", stderr(&out));
    let listed = stdout(&out);
    assert!(
        listed
            .lines()
            .any(|line| line.contains("known") && line.contains("Game"))
    );
    assert!(
        listed
            .lines()
            .any(|line| line.contains("gone") && line.contains("Old") && line.ends_with("stray"))
    );
    assert!(stderr(&out).contains("1 entry belong to an environment that no longer exists"));
    assert!(stderr(&out).contains(&stray.display().to_string()));
}

#[test]
fn status_lists_targets_with_desktop_entries() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "known", &env_toml("game"));
    desktop_entry(root.path(), "known", "game", "Game");

    let out = output(wineo(root.path()).args(["status", "known"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("\n  desktop    game\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn rm_removes_desktop_entries_with_the_environment() {
    let root = tempfile::tempdir().unwrap();
    write_environment(root.path(), "app", MINIMAL_ENVIRONMENT);
    let path = desktop_entry(root.path(), "app", "app", "App");

    let out = output(wineo(root.path()).args(["rm", "app"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!path.exists());
    assert!(
        stderr(&out).contains("Removed 1 desktop entry"),
        "{}",
        stderr(&out)
    );
}
