//! Launch hook wiring through the CLI and its fake Wine runner.

mod common;

use std::ffi::OsString;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use wineo_core::hooks::Phase;
use wineo_core::launch::{self, StopAction, StopOutcome};
use wineo_core::operation::Locks;
use wineo_core::paths::Paths;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

fn native_environment(f: &Fixture, name: &str, exe: &Path, hooks: &str) {
    let config = f.config_dir().join(format!("environments/{name}.toml"));
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        format!(
            "kind = 'native'\n\n[targets.{name}]\nexe = {}\n\n{hooks}",
            toml_string(&exe.display().to_string())
        ),
    )
    .unwrap();
}

fn install_native_program(f: &Fixture, name: &str, script: &str) -> PathBuf {
    let path = f.root.path().join("bin").join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn hooked_targets(target: &str, before: Option<&str>, after: Option<&str>) -> String {
    let mut config = String::new();
    if before.is_some() || after.is_some() {
        config.push_str("[hooks]\n");
        if let Some(before) = before {
            config.push_str(&format!("before = {}\n", toml_string(before)));
        }
        if let Some(after) = after {
            config.push_str(&format!("after = {}\n", toml_string(after)));
        }
        config.push('\n');
    }
    config.push_str(target);
    config
}

fn install_program_marker(f: &Fixture, program: &str, marker: &std::path::Path) {
    let real = f.root.path().join("bin/wine.real");
    fs::copy(f.root.path().join("bin/wine"), &real).unwrap();
    let wrapper = format!(
        "#!/bin/sh\nif [ \"$1\" = {} ]; then printf '%s\\n' program >> {}; exit 0; fi\nexec {} \"$@\"\n",
        shell_quote(program),
        shell_quote(&marker.display().to_string()),
        shell_quote(&real.display().to_string())
    );
    let path = f.root.path().join("bin/wine");
    fs::write(&path, wrapper).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn append_marker(marker: &std::path::Path, value: &str) -> String {
    format!(
        "printf '%s\\n' {} >> {}",
        shell_quote(value),
        shell_quote(&marker.display().to_string())
    )
}

fn wait_for_text(path: &std::path::Path, expected: &str) {
    wait_for("hook output", || {
        fs::read_to_string(path).is_ok_and(|text| text.contains(expected))
    });
}

fn spawn_wineo(f: &Fixture, args: &[&str]) -> Child {
    f.wineo()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn spawn_wineo_with_stderr(f: &Fixture, args: &[&str], stderr: &std::path::Path) -> Child {
    f.wineo()
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(stderr).unwrap()))
        .spawn()
        .unwrap()
}

fn signal(child: &Child, signal: &str) {
    signal_pid(child.id(), signal);
}

fn signal_pid(pid: u32, signal: &str) {
    let sent = Command::new("kill")
        .args([signal, &pid.to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
}

fn detached_supervisor_pid(f: &Fixture) -> Option<u32> {
    let expected_state = format!("XDG_STATE_HOME={}", f.root.path().join("state").display());
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(command_line) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<_> = command_line.split(|byte| *byte == 0).collect();
        if !args.get(1).is_some_and(|arg| *arg == b"__supervise") {
            continue;
        }
        let Ok(environment) = fs::read(entry.path().join("environ")) else {
            continue;
        };
        if environment
            .split(|byte| *byte == 0)
            .any(|value| value == expected_state.as_bytes())
        {
            return Some(pid);
        }
    }
    None
}

fn gate_hook(marker: &std::path::Path, release: &std::path::Path) -> String {
    format!(
        "touch {}; while [ ! -e {} ]; do sleep 0.02; done",
        shell_quote(&marker.display().to_string()),
        shell_quote(&release.display().to_string())
    )
}

fn captured_end(path: &std::path::Path) -> String {
    format!(
        "printf '%s|%s|%s|%s|%s|%s\\n' \"$WINEO_END\" \"$WINEO_EXIT_CODE\" \"$WINEO_SECONDS\" \"$WINEO_HOOK\" \"$WINEO_TARGET\" \"$WINEO_EXE\" > {}",
        shell_quote(&path.display().to_string())
    )
}

fn captured_failure(path: &std::path::Path) -> String {
    format!(
        "printf '%s|%s|%s|%s\\n' \"$WINEO_END\" \"$WINEO_EXIT_CODE\" \"$WINEO_SECONDS\" \"$PPID\" >> {}",
        shell_quote(&path.display().to_string())
    )
}

#[test]
fn preparation_failures_run_after_once_in_the_launching_process() {
    for mode in ["start", "run"] {
        for failure in ["exe", "cwd", "prefix", "component"] {
            let f = Fixture::new();
            let before = f.root.path().join("before");
            let after = f.root.path().join("after");
            let missing = f.root.path().join("missing");
            let target = match failure {
                "exe" => format!(
                    "[targets.app]\nexe = {}\n",
                    toml_string(missing.to_str().unwrap())
                ),
                "cwd" => format!(
                    "[targets.app]\nexe = 'exit-0'\ncwd = {}\n",
                    toml_string(missing.to_str().unwrap())
                ),
                _ => "[targets.app]\nexe = 'exit-0'\n".to_owned(),
            };
            f.environment(
                "app",
                &hooked_targets(
                    &target,
                    Some(&append_marker(&before, "ran")),
                    Some(&captured_failure(&after)),
                ),
            );
            if failure == "prefix" {
                fs::remove_dir_all(f.root.path().join("prefixes/app")).unwrap();
            } else if failure == "component" {
                f.install_build("dxvk", "broken", &["x64", "x32"]);
                let config = f.config_dir().join("environments/app.toml");
                let text = fs::read_to_string(&config).unwrap();
                fs::write(&config, format!("dxvk = 'broken'\n{text}")).unwrap();
                fs::remove_dir(f.system32("app")).unwrap();
                fs::write(f.system32("app"), "blocked").unwrap();
            }
            let child = spawn_wineo(&f, &[mode, "app"]);
            let pid = child.id();
            let output = child.wait_with_output().unwrap();
            assert!(
                !output.status.success(),
                "{mode}/{failure}: {}",
                stderr(&output)
            );
            assert_eq!(fs::read_to_string(before).unwrap(), "ran\n");
            assert_eq!(
                fs::read_to_string(after).unwrap(),
                format!("failed||0|{pid}\n"),
                "{mode}/{failure}"
            );
            assert!(!f.runtime("app").is_running().unwrap());
            assert!(locks(&f).busy_with("app").unwrap().is_none());
        }
    }
}

#[test]
fn failed_or_canceled_winetricks_runs_after_once_and_preserves_the_error() {
    for mode in ["start", "run"] {
        for cancel in [false, true] {
            let f = Fixture::new();
            let before = f.root.path().join("before");
            let after = f.root.path().join("after");
            let active = f.root.path().join("winetricks-active");
            f.environment(
                "app",
                &format!(
                    "winetricks = ['corefonts']\n{}",
                    hooked_targets(
                        "[targets.app]\nexe = 'exit-0'\n",
                        Some(&append_marker(&before, "ran")),
                        Some(&format!("{}; exit 9", captured_failure(&after))),
                    )
                ),
            );
            let script = if cancel {
                format!(
                    "#!/bin/sh\ntouch {}; while :; do sleep 0.02; done\n",
                    shell_quote(active.to_str().unwrap())
                )
            } else {
                "#!/bin/sh\nexit 7\n".to_owned()
            };
            let winetricks = f.root.path().join("bin/winetricks");
            fs::write(&winetricks, script).unwrap();
            fs::set_permissions(&winetricks, fs::Permissions::from_mode(0o755)).unwrap();
            let child = spawn_wineo(&f, &[mode, "app"]);
            let pid = child.id();
            if cancel {
                wait_for("winetricks to start", || active.exists());
                signal(&child, "-INT");
            }
            let output = child.wait_with_output().unwrap();
            assert!(!output.status.success(), "{}", stderr(&output));
            if cancel {
                assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
            } else {
                assert!(
                    stderr(&output).contains("winetricks failed installing"),
                    "{}",
                    stderr(&output)
                );
            }
            assert!(stderr(&output).contains("warning:"), "{}", stderr(&output));
            assert_eq!(fs::read_to_string(before).unwrap(), "ran\n");
            assert_eq!(
                fs::read_to_string(after).unwrap(),
                format!("failed||0|{pid}\n")
            );
            assert!(!f.runtime("app").is_running().unwrap());
            assert!(locks(&f).busy_with("app").unwrap().is_none());
        }
    }
}

#[test]
fn failed_launch_blocks_exclusive_operations_through_launcher_cleanup() {
    for mode in ["start", "run"] {
        let f = Fixture::new();
        let before = f.root.path().join("before");
        let preparation = f.root.path().join("preparation");
        let fail = f.root.path().join("fail");
        let after_active = f.root.path().join("after-active");
        let release = f.root.path().join("release");
        let after = f.root.path().join("after");
        f.environment(
            "app",
            &format!(
                "winetricks = ['corefonts']\n{}",
                hooked_targets(
                    "[targets.app]\nexe = 'exit-0'\n",
                    Some(&append_marker(&before, "ran")),
                    Some(&format!(
                        "{}; {}",
                        captured_failure(&after),
                        gate_hook(&after_active, &release)
                    )),
                )
            ),
        );
        let winetricks = f.root.path().join("bin/winetricks");
        fs::write(
            &winetricks,
            format!("#!/bin/sh\n{}; exit 7\n", gate_hook(&preparation, &fail)),
        )
        .unwrap();
        fs::set_permissions(&winetricks, fs::Permissions::from_mode(0o755)).unwrap();
        let child = spawn_wineo(&f, &[mode, "app"]);
        let pid = child.id();
        wait_for("preparation after before", || preparation.exists());
        assert_eq!(fs::read_to_string(before).unwrap(), "ran\n");
        assert!(matches!(
            locks(&f).acquire(wineo_core::operation::Operation::Snapshot, "app", &[]),
            Err(wineo_core::error::Error::Busy { .. })
        ));
        fs::write(fail, "").unwrap();
        wait_for("launcher cleanup", || after_active.exists());
        assert!(matches!(
            locks(&f).acquire(wineo_core::operation::Operation::Winetricks, "app", &[]),
            Err(wineo_core::error::Error::Busy { .. })
        ));
        fs::write(release, "").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(
            stderr(&output).contains("winetricks failed installing"),
            "{}",
            stderr(&output)
        );
        assert_eq!(
            fs::read_to_string(after).unwrap(),
            format!("failed||0|{pid}\n")
        );
        locks(&f)
            .acquire(wineo_core::operation::Operation::Snapshot, "app", &[])
            .unwrap();
    }
}

#[test]
fn launcher_cleanup_after_uses_the_two_signal_wait_then_kill_behavior() {
    let f = Fixture::new();
    let before = f.root.path().join("before");
    let active = f.root.path().join("after-active");
    let child_pid = f.root.path().join("child-pid");
    let after = format!(
        "touch {}; sleep 60 & echo $! > {}; while :; do sleep 0.02; done",
        shell_quote(&active.display().to_string()),
        shell_quote(&child_pid.display().to_string())
    );
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            Some(&append_marker(&before, "ran")),
            Some(&after),
        ),
    );
    let wine = f.root.path().join("bin/wine");
    fs::write(&wine, "#!/nonexistent/interpreter\n").unwrap();
    fs::set_permissions(&wine, fs::Permissions::from_mode(0o755)).unwrap();

    let stderr_path = f.root.path().join("run.stderr");
    let run = spawn_wineo_with_stderr(&f, &["run", "app"], &stderr_path);
    wait_for("launcher-side after hook child to start", || {
        child_pid.exists()
    });
    let group = hook_group(&f, "app");
    signal(&run, "-INT");
    wait_for_text(
        &stderr_path,
        "Waiting for the after hook, press Ctrl+C again to kill it.",
    );
    assert_eq!(
        wineo_core::process::group_has_live_members(group),
        Some(true),
        "the first Ctrl+C should leave cleanup running"
    );
    signal(&run, "-INT");
    assert!(!run.wait_with_output().unwrap().status.success());
    wait_for("launcher-side after hook group to be killed", || {
        wineo_core::process::group_has_live_members(group) == Some(false)
            && locks(&f).busy_with("app").unwrap().is_none()
    });
    assert_eq!(fs::read_to_string(before).unwrap(), "ran\n");
}

fn paths(f: &Fixture) -> Paths {
    let root = f.root.path().to_owned();
    Paths::from_lookup(move |key| {
        let path = match key {
            "HOME" => root.clone(),
            "XDG_CONFIG_HOME" => root.join("config"),
            "XDG_DATA_HOME" => root.join("data"),
            "XDG_STATE_HOME" => root.join("state"),
            _ => return None,
        };
        Some(OsString::from(path.as_os_str()))
    })
    .unwrap()
}

fn locks(f: &Fixture) -> Locks {
    Locks::new(&paths(f))
}

fn hook_group(f: &Fixture, name: &str) -> u32 {
    locks(f)
        .hook_group(name)
        .unwrap()
        .expect("active hook lock should record its process group")
        .1
        .group
}

#[test]
fn before_program_after_order_for_detached_and_attached_launches() {
    for mode in ["start", "run"] {
        let f = Fixture::new();
        let order = f.root.path().join("order");
        install_program_marker(&f, "ordered-game", &order);
        let before = append_marker(&order, "before");
        let after = append_marker(&order, "after");
        f.environment(
            "app",
            &hooked_targets(
                "[targets.app]\nexe = 'ordered-game'\n",
                Some(&before),
                Some(&after),
            ),
        );

        let launched = f.run(&[mode, "app"]);
        if mode == "run" {
            assert_success(&launched);
        } else {
            assert_success(&launched);
            f.wait_for_run_end("app", "exited");
        }
        wait_for_text(&order, "after");
        assert_eq!(
            fs::read_to_string(order).unwrap(),
            "before\nprogram\nafter\n"
        );
    }
}

#[test]
fn before_can_create_a_missing_executable_before_target_checks() {
    let f = Fixture::new();
    let exe = f.root.path().join("games/created-game.exe");
    let exe_text = exe.display().to_string();
    let order = f.root.path().join("order");
    install_program_marker(&f, &exe_text, &order);
    let before = format!(
        "mkdir -p {}; : > {}",
        shell_quote(exe.parent().unwrap().to_str().unwrap()),
        shell_quote(&exe_text)
    );
    let after = append_marker(&order, "after");
    f.environment(
        "app",
        &hooked_targets(
            &format!("[targets.app]\nexe = {}\n", toml_string(&exe_text)),
            Some(&before),
            Some(&after),
        ),
    );

    let launched = f.run(&["run", "app"]);
    assert_success(&launched);
    assert!(exe.is_file());
    assert_eq!(fs::read_to_string(order).unwrap(), "program\nafter\n");
}

#[test]
fn a_failed_before_hook_cancels_the_launch_and_skips_after() {
    let f = Fixture::new();
    let before = f.root.path().join("before");
    let after = f.root.path().join("after");
    let before_command = format!("{}; exit 7", append_marker(&before, "ran"));
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            Some(&before_command),
            Some(&append_marker(&after, "ran")),
        ),
    );

    let launched = f.run(&["start", "app"]);
    assert!(!launched.status.success());
    assert!(stderr(&launched).contains("Launch canceled: the before hook exited with code 7."));
    assert_eq!(fs::read_to_string(before).unwrap(), "ran\n");
    assert!(!after.exists());
    assert!(!f.runtime("app").is_running().unwrap());
}

#[test]
fn ctrl_c_during_before_cancels_the_hook_and_skips_after() {
    let f = Fixture::new();
    let before = f.root.path().join("before");
    let after = f.root.path().join("after");
    let before_command = format!(
        "touch {}; while :; do sleep 0.02; done",
        shell_quote(&before.display().to_string())
    );
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            Some(&before_command),
            Some(&append_marker(&after, "ran")),
        ),
    );

    let child = spawn_wineo(&f, &["run", "app"]);
    wait_for("before hook to start", || before.exists());
    signal(&child, "-INT");
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert!(!after.exists());
}

#[test]
fn stop_during_before_cancels_the_hook() {
    let f = Fixture::new();
    let before = f.root.path().join("before");
    let after = f.root.path().join("after");
    let before_command = format!(
        "touch {}; trap 'exit 0' TERM; while :; do sleep 0.02; done",
        shell_quote(&before.display().to_string())
    );
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            Some(&before_command),
            Some(&append_marker(&after, "ran")),
        ),
    );

    let child = spawn_wineo(&f, &["start", "app"]);
    wait_for("before hook to start", || before.exists());
    let stopped = f.run(&["stop", "app"]);
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "The before hook was canceled.\n");
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success(), "{}", stderr(&output));
    assert!(!after.exists());
}

#[test]
fn force_stop_during_before_kills_the_hook_group() {
    let f = Fixture::new();
    let active = f.root.path().join("before-active");
    let child_pid = f.root.path().join("child-pid");
    let after = f.root.path().join("after");
    let before = format!(
        "touch {}; sleep 60 & echo $! > {}; while :; do sleep 0.02; done",
        shell_quote(&active.display().to_string()),
        shell_quote(&child_pid.display().to_string())
    );
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            Some(&before),
            Some(&append_marker(&after, "ran")),
        ),
    );

    let child = spawn_wineo(&f, &["start", "app"]);
    wait_for("before hook child to start", || child_pid.exists());
    let group = hook_group(&f, "app");
    let stopped = f.run(&["stop", "--force", "app"]);
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "The before hook was canceled.\n");
    wait_for("before hook process group to be killed", || {
        wineo_core::process::group_has_live_members(group) == Some(false)
    });
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success(), "{}", stderr(&output));
    assert!(!after.exists());
}

#[test]
fn confirmed_after_kill_requires_the_same_hook_run() {
    let f = Fixture::new();
    let active = f.root.path().join("after-active");
    let after = format!(
        "touch {}; while :; do sleep 0.02; done",
        shell_quote(&active.display().to_string())
    );
    f.environment(
        "app",
        &hooked_targets("[targets.app]\nexe = 'exit-0'\n", None, Some(&after)),
    );

    assert_success(&f.run(&["start", "app"]));
    wait_for("first after hook to start", || active.exists());
    let (phase, first) = locks(&f).hook_group("app").unwrap().unwrap();
    assert_eq!(phase, Phase::After);
    let mut wrong = first.clone();
    wrong.leader_start += 1;
    assert!(matches!(
        launch::force_stop_after(&paths(&f), "app", &wrong).unwrap(),
        StopOutcome::NotRunning { .. }
    ));
    assert_eq!(
        wineo_core::process::group_has_live_members(first.group),
        Some(true)
    );
    assert!(matches!(
        launch::force_stop_after(&paths(&f), "app", &first).unwrap(),
        StopOutcome::Stopped {
            action: StopAction::AfterKilled,
            ..
        }
    ));

    fs::remove_file(&active).unwrap();
    assert_success(&f.run(&["start", "app"]));
    wait_for("second after hook to start", || active.exists());
    let (phase, second) = locks(&f).hook_group("app").unwrap().unwrap();
    assert_eq!(phase, Phase::After);
    assert_ne!(first, second);
    assert!(matches!(
        launch::force_stop_after(&paths(&f), "app", &first).unwrap(),
        StopOutcome::NotRunning { .. }
    ));
    assert_eq!(
        wineo_core::process::group_has_live_members(second.group),
        Some(true)
    );
    assert!(matches!(
        launch::force_stop_after(&paths(&f), "app", &second).unwrap(),
        StopOutcome::Stopped {
            action: StopAction::AfterKilled,
            ..
        }
    ));
}

#[test]
fn concurrent_run_lock_skips_after_when_before_finishes() {
    for mode in ["start", "run"] {
        let f = Fixture::new();
        let before_active = f.root.path().join("before-active");
        let release = f.root.path().join("release");
        let before = gate_hook(&before_active, &release);
        let after = f.root.path().join("after");
        f.environment(
            "app",
            &hooked_targets(
                "[targets.app]\nexe = 'exit-0'\n",
                Some(&before),
                Some(&append_marker(&after, "ran")),
            ),
        );

        let child = spawn_wineo(&f, &[mode, "app"]);
        wait_for("before hook to start", || before_active.exists());
        let run_lock = f.runtime("app").try_lock().unwrap().unwrap();
        fs::write(release, "").unwrap();
        let output = child.wait_with_output().unwrap();
        drop(run_lock);

        if mode == "start" {
            assert_success(&output);
            assert!(
                stderr(&output).contains("already running"),
                "{}",
                stderr(&output)
            );
        } else {
            assert!(!output.status.success(), "{}", stderr(&output));
            assert!(
                stderr(&output).contains("already running"),
                "{}",
                stderr(&output)
            );
        }
        assert!(
            !after.exists(),
            "{mode} must leave cleanup to the locked run"
        );
        assert!(
            !f.runtime("app")
                .read_state()
                .unwrap()
                .is_some_and(|state| state.pid == 0)
        );
    }
}

#[test]
fn native_hooks_keep_order_and_receive_native_environment_values() {
    let f = Fixture::new();
    let order = f.root.path().join("order");
    let before_env = f.root.path().join("before-env");
    let after_env = f.root.path().join("after-env");
    let exe = install_native_program(
        &f,
        "native-game",
        &format!(
            "#!/bin/sh\nprintf 'program\\n' >> {}\n",
            shell_quote(&order.display().to_string())
        ),
    );
    let before = format!(
        "{}; printf '%s|%s|%s|%s|%s|%s|%s|%s|%s\\n' \"${{WINEO_HOOK-UNSET}}\" \"${{WINEO_ENVIRONMENT-UNSET}}\" \"${{WINEO_TARGET-UNSET}}\" \"${{WINEO_EXE-UNSET}}\" \"${{WINEO_PREFIX-UNSET}}\" \"${{WINEO_END-UNSET}}\" \"${{WINEO_EXIT_CODE-UNSET}}\" \"${{WINEO_SECONDS-UNSET}}\" \"${{WINEPREFIX-UNSET}}\" > {}",
        append_marker(&order, "before"),
        shell_quote(&before_env.display().to_string())
    );
    let after = format!(
        "{}; printf '%s|%s|%s|%s|%s|%s|%s|%s|%s\\n' \"${{WINEO_HOOK-UNSET}}\" \"${{WINEO_ENVIRONMENT-UNSET}}\" \"${{WINEO_TARGET-UNSET}}\" \"${{WINEO_EXE-UNSET}}\" \"${{WINEO_PREFIX-UNSET}}\" \"${{WINEO_END-UNSET}}\" \"${{WINEO_EXIT_CODE-UNSET}}\" \"${{WINEO_SECONDS-UNSET}}\" \"${{WINEPREFIX-UNSET}}\" > {}",
        append_marker(&order, "after"),
        shell_quote(&after_env.display().to_string())
    );
    native_environment(
        &f,
        "app",
        &exe,
        &format!(
            "[hooks]\nbefore = {}\nafter = {}\n",
            toml_string(&before),
            toml_string(&after)
        ),
    );

    let launched = f
        .wineo()
        .env("WINEPREFIX", "inherited-prefix")
        .env("WINEO_PREFIX", "inherited-wineo-prefix")
        .env("WINEO_END", "inherited-end")
        .env("WINEO_EXIT_CODE", "inherited-code")
        .args(["run", "app"])
        .output()
        .unwrap();
    assert_success(&launched);
    assert_eq!(
        fs::read_to_string(order).unwrap(),
        "before\nprogram\nafter\n"
    );
    assert_eq!(
        fs::read_to_string(before_env).unwrap(),
        format!(
            "before|app|app|{}|UNSET|UNSET|UNSET|UNSET|UNSET\n",
            exe.display()
        )
    );
    let after_env = fs::read_to_string(after_env).unwrap();
    let fields: Vec<_> = after_env.trim_end().split('|').collect();
    assert_eq!(fields.len(), 9, "{after_env}");
    assert_eq!(
        &fields[..7],
        [
            "after",
            "app",
            "app",
            exe.to_str().unwrap(),
            "UNSET",
            "exited",
            "0"
        ]
    );
    assert!(
        !fields[7].is_empty(),
        "WINEO_SECONDS was empty: {after_env}"
    );
    assert_eq!(fields[8], "UNSET", "{after_env}");
}

#[test]
fn stop_during_attached_run_before_cancels_it() {
    let f = Fixture::new();
    let active = f.root.path().join("before-active");
    let release = f.root.path().join("release");
    let after = f.root.path().join("after");
    let before = gate_hook(&active, &release);
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            Some(&before),
            Some(&append_marker(&after, "ran")),
        ),
    );

    let run = spawn_wineo(&f, &["run", "app"]);
    wait_for("attached before hook to start", || active.exists());
    assert_success(&f.run(&["stop", "app"]));
    let output = run.wait_with_output().unwrap();
    assert!(!output.status.success(), "{}", stderr(&output));
    assert!(!after.exists());
}

#[test]
fn stop_during_attached_run_after_waits_for_the_hook() {
    let f = Fixture::new();
    let active = f.root.path().join("after-active");
    let release = f.root.path().join("release");
    let after = gate_hook(&active, &release);
    f.environment(
        "app",
        &hooked_targets("[targets.app]\nexe = 'exit-0'\n", None, Some(&after)),
    );

    let run = spawn_wineo(&f, &["run", "app"]);
    wait_for("attached after hook to start", || active.exists());
    let stop_output = f.root.path().join("stop.output");
    let mut stop = f
        .wineo()
        .args(["stop", "app"])
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&stop_output).unwrap()))
        .spawn()
        .unwrap();
    wait_for_text(&stop_output, "Waiting for the after hook.");
    thread::sleep(Duration::from_millis(150));
    assert!(
        stop.try_wait().unwrap().is_none(),
        "stop returned before after finished"
    );
    fs::write(release, "").unwrap();
    assert!(stop.wait().unwrap().success());
    assert_success(&run.wait_with_output().unwrap());
}

#[test]
fn sigint_interrupts_a_slow_stop_without_hooks() {
    // Ignored signals survive exec, and hook-less stop relies on SIGINT's
    // default disposition. Background shells can make that impossible here.
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let ignored = status
        .lines()
        .find_map(|line| line.strip_prefix("SigIgn:"))
        .map(|mask| u64::from_str_radix(mask.trim(), 16).unwrap())
        .expect("proc status should report ignored signals");
    if ignored & (1 << (signal_hook::consts::SIGINT - 1)) != 0 {
        eprintln!("skipping slow stop SIGINT test: SIGINT is inherited as ignored");
        return;
    }
    let f = Fixture::new();
    f.global_config("stop_timeout = 10\n");
    f.environment("app", "[targets.app]\nexe = 'stubborn'\n");
    assert_success(&f.run(&["start", "app"]));
    wait_for("stubborn game to run", || f.fake_program_running("app"));

    let mut stop = f
        .wineo()
        .args(["stop", "app"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Signal only once the stop is really waiting on the stubborn game, so the
    // test can't pass without exercising the wait.
    let log = f.runtime("app").log_path();
    wait_for("stop to ask the game to close", || {
        fs::read_to_string(&log)
            .is_ok_and(|text| text.contains("stopping, asking programs to close"))
    });
    signal(&stop, "-INT");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if stop.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "SIGINT did not interrupt wineo stop"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert_success(&f.run(&["stop", "--force", "app"]));
}

#[test]
fn hookless_launch_creates_only_the_existing_operation_lock_files() {
    let f = Fixture::new();
    f.environment("app", "[targets.app]\nexe = 'stubborn'\n");
    assert_success(&f.run(&["start", "app"]));
    wait_for("stubborn game to run", || f.fake_program_running("app"));

    let lock_dir = f.root.path().join("state/wineo/.locks");
    let mut files: Vec<_> = fs::read_dir(&lock_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "unexpected operation locks: {files:?}");
    assert!(
        files.iter().any(|file| file == "name-app.lock"),
        "{files:?}"
    );
    assert_eq!(
        files
            .iter()
            .filter(|file| { file.starts_with("prefix-app-") && file.ends_with(".lock") })
            .count(),
        1,
        "{files:?}"
    );
    assert!(
        !files
            .iter()
            .any(|file| file.starts_with("admit-") || file.starts_with("launch-")),
        "hook-less launch made reservation locks: {files:?}"
    );
    assert!(f.root.path().join("state/wineo/app/lock").is_file());
    assert_success(&f.run(&["stop", "app"]));
}

#[test]
fn hookless_launch_reports_runner_errors_before_target_errors() {
    let f = Fixture::new();
    f.environment_with_runner("app", "not-installed", "[targets.app]\nexe = 'exit-0'\n");

    for mode in ["run", "start", "restart"] {
        let output = f.run(&[mode, "app", "--target", "missing"]);
        assert!(!output.status.success(), "{mode} unexpectedly succeeded");
        assert!(
            stderr(&output).contains("not-installed")
                && stderr(&output).contains("is not installed"),
            "{mode}: {}",
            stderr(&output)
        );
        assert!(
            !stderr(&output).contains("no target named"),
            "{mode} reported target resolution first: {}",
            stderr(&output)
        );
    }
}

#[test]
fn hooks_do_not_block_another_environment_sharing_the_prefix() {
    for phase in ["before", "after"] {
        let f = Fixture::new();
        let active = f.root.path().join("active");
        let release = f.root.path().join("release");
        let hook = gate_hook(&active, &release);
        f.environment(
            "app",
            &hooked_targets(
                "[targets.app]\nexe = 'exit-0'\n",
                (phase == "before").then_some(hook.as_str()),
                (phase == "after").then_some(hook.as_str()),
            ),
        );
        f.environment("alias", "[targets.alias]\nexe = 'exit-0'\n");
        let config = f.config_dir().join("environments/alias.toml");
        let text = fs::read_to_string(&config).unwrap().replace(
            f.root.path().join("prefixes/alias").to_str().unwrap(),
            f.root.path().join("prefixes/app").to_str().unwrap(),
        );
        fs::write(config, text).unwrap();
        let child = spawn_wineo(&f, &["start", "app"]);
        wait_for("hook to hold its name lock", || active.exists());
        let second = f.run(&["start", "app"]);
        assert!(!second.status.success());
        assert!(stderr(&second).contains("busy"), "{}", stderr(&second));
        assert_success(&f.run(&["run", "alias"]));
        assert_eq!(
            locks(&f).busy_with("app").unwrap(),
            Some(wineo_core::operation::Operation::Hook(
                if phase == "before" {
                    wineo_core::hooks::Phase::Before
                } else {
                    wineo_core::hooks::Phase::After
                }
            ))
        );
        fs::write(release, "").unwrap();
        assert_success(&child.wait_with_output().unwrap());
        f.wait_for_run_end("app", "exited");
        wait_for("hook name lock to release", || {
            locks(&f).busy_with("app").unwrap().is_none()
        });
    }
}

#[test]
fn a_second_launch_is_busy_during_before_and_status_reports_it() {
    let f = Fixture::new();
    let before = f.root.path().join("before");
    let release = f.root.path().join("release");
    let hook = format!(
        "{}; {}",
        append_marker(&before, "ran"),
        gate_hook(&f.root.path().join("before-active"), &release)
    );
    f.environment(
        "app",
        &hooked_targets("[targets.app]\nexe = 'exit-0'\n", Some(&hook), None),
    );

    let child = spawn_wineo(&f, &["start", "app"]);
    wait_for("before hook to hold its lock", || {
        f.root.path().join("before-active").exists()
    });
    let second = f.run(&["start", "app"]);
    assert!(!second.status.success());
    assert!(stderr(&second).contains("busy"), "{}", stderr(&second));
    let status = f.run(&["status", "app"]);
    assert!(stdout(&status).contains("busy"), "{}", stdout(&status));
    assert!(
        stdout(&status).contains("before hook"),
        "{}",
        stdout(&status)
    );
    let json = f.run(&["status", "app", "--json"]);
    assert_success(&json);
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(json["busy"], "hook-before");
    assert_eq!(
        json["hooks"],
        serde_json::json!({"before": true, "after": false})
    );
    assert!(
        !json.to_string().contains(&hook),
        "hook commands must not be included in status JSON: {json}"
    );
    assert_eq!(fs::read_to_string(before).unwrap(), "ran\n");

    let overview = f.run(&["status"]);
    assert_success(&overview);
    assert!(stdout(&overview).contains("running its before hook"));
    assert!(!stdout(&overview).contains("hook-before"));

    fs::write(release, "").unwrap();
    assert_success(&child.wait_with_output().unwrap());
}

#[test]
fn status_and_launches_during_after_show_ended_but_busy() {
    let f = Fixture::new();
    let before = f.root.path().join("before");
    let after_active = f.root.path().join("after-active");
    let release = f.root.path().join("release");
    let before_command = append_marker(&before, "ran");
    let after = format!(
        "{}; {}",
        append_marker(&f.root.path().join("after-runs"), "ran"),
        gate_hook(&after_active, &release)
    );
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            Some(&before_command),
            Some(&after),
        ),
    );

    assert_success(&f.run(&["start", "app"]));
    wait_for("after hook to start", || after_active.exists());
    let status = f.run(&["status", "app"]);
    assert!(
        stdout(&status).contains("app: exited"),
        "{}",
        stdout(&status)
    );
    assert!(
        stdout(&status).contains("after hook"),
        "{}",
        stdout(&status)
    );
    let json = f.run(&["status", "app", "--json"]);
    assert_success(&json);
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(json["state"], "exited");
    assert_eq!(json["busy"], "hook-after");
    assert_eq!(
        json["hooks"],
        serde_json::json!({"before": true, "after": true})
    );
    assert!(
        !json.to_string().contains(&before_command) && !json.to_string().contains(&after),
        "hook commands must not be included in status JSON: {json}"
    );

    let overview = f.run(&["status"]);
    assert_success(&overview);
    assert!(stdout(&overview).contains("running its after hook"));
    assert!(!stdout(&overview).contains("hook-after"));

    let second = f.run(&["start", "app"]);
    assert!(!second.status.success());
    assert!(stderr(&second).contains("busy"), "{}", stderr(&second));
    assert_eq!(fs::read_to_string(before).unwrap(), "ran\n");
    assert_eq!(
        fs::read_to_string(f.root.path().join("after-runs")).unwrap(),
        "ran\n"
    );

    fs::write(release, "").unwrap();
    wait_for("after hook to finish", || {
        locks(&f).busy_with("app").unwrap().is_none()
    });
}

#[test]
fn starting_an_already_running_game_does_not_run_before_again() {
    let f = Fixture::new();
    let before = f.root.path().join("before");
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'stubborn'\n",
            Some(&append_marker(&before, "ran")),
            None,
        ),
    );

    assert_success(&f.run(&["start", "app"]));
    wait_for("the game to run", || f.fake_program_running("app"));
    let second = f.run(&["start", "app"]);
    assert_success(&second);
    assert!(
        stderr(&second).contains("already running"),
        "{}",
        stderr(&second)
    );
    assert_eq!(fs::read_to_string(before).unwrap(), "ran\n");
    assert_success(&f.run(&["stop", "app"]));
}

#[test]
fn after_receives_exit_values_and_runs_after_a_failed_spawn() {
    let f = Fixture::new();
    let captured = f.root.path().join("end");
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-3'\n",
            None,
            Some(&captured_end(&captured)),
        ),
    );
    let exited = f.run(&["run", "app"]);
    assert_eq!(exited.status.code(), Some(3), "{}", stderr(&exited));
    let end = fs::read_to_string(&captured).unwrap();
    assert!(end.starts_with("failed|3|"), "{end}");
    assert!(end.trim_end().ends_with("|after|app|exit-3"), "{end}");

    let f = Fixture::new();
    let captured = f.root.path().join("end");
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            None,
            Some(&captured_end(&captured)),
        ),
    );
    assert_success(&f.run(&["run", "app"]));
    let end = fs::read_to_string(captured).unwrap();
    assert!(end.starts_with("exited|0|"), "{end}");

    for mode in ["start", "run"] {
        let f = Fixture::new();
        let before = f.root.path().join("before");
        let captured = f.root.path().join("end");
        let owners = f.root.path().join("owners");
        f.environment(
            "app",
            &hooked_targets(
                "[targets.app]\nexe = 'exit-0'\n",
                Some(&append_marker(&before, "ran")),
                Some(&format!(
                    "{}; {}",
                    captured_end(&captured),
                    captured_failure(&owners)
                )),
            ),
        );
        let wine = f.root.path().join("bin/wine");
        fs::write(&wine, "#!/nonexistent/interpreter\n").unwrap();
        fs::set_permissions(&wine, fs::Permissions::from_mode(0o755)).unwrap();
        let child = spawn_wineo(&f, &[mode, "app"]);
        let pid = child.id();
        let failed = child.wait_with_output().unwrap();
        assert!(!failed.status.success());
        wait_for_text(&before, "ran");
        wait_for_text(&captured, "failed||");
        let end = fs::read_to_string(captured).unwrap();
        assert!(end.starts_with("failed||"), "{mode}: {end}");
        assert!(
            end.trim_end().ends_with("|after|app|exit-0"),
            "{mode}: {end}"
        );
        assert_eq!(
            fs::read_to_string(owners).unwrap(),
            format!("failed||0|{pid}\n"),
            "{mode}: {end}"
        );
    }
}

#[test]
fn stop_runs_after_and_force_stop_kills_after_hook_children() {
    let f = Fixture::new();
    f.global_config("stop_timeout = 1\n");
    let active = f.root.path().join("after-active");
    let release = f.root.path().join("release");
    let after = gate_hook(&active, &release);
    f.environment(
        "app",
        &hooked_targets("[targets.app]\nexe = 'exit-0'\n", None, Some(&after)),
    );
    assert_success(&f.run(&["start", "app"]));
    wait_for("after hook to start", || active.exists());

    let stop_output = f.root.path().join("stop.output");
    let mut stop = f
        .wineo()
        .args(["stop", "app"])
        .stdout(Stdio::piped())
        .stderr(Stdio::from(File::create(&stop_output).unwrap()))
        .spawn()
        .unwrap();
    wait_for_text(&stop_output, "Waiting for the after hook.");
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline {
        assert!(
            stop.try_wait().unwrap().is_none(),
            "stop returned before the hook finished"
        );
        thread::sleep(Duration::from_millis(20));
    }
    fs::write(release, "").unwrap();
    let stopped = stop.wait_with_output().unwrap();
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "The after hook finished.\n");

    let f = Fixture::new();
    let active = f.root.path().join("after-active");
    let child_pid = f.root.path().join("child-pid");
    let after = format!(
        "touch {}; sleep 60 & echo $! > {}; while :; do sleep 0.02; done",
        shell_quote(&active.display().to_string()),
        shell_quote(&child_pid.display().to_string())
    );
    f.environment(
        "app",
        &hooked_targets("[targets.app]\nexe = 'exit-0'\n", None, Some(&after)),
    );
    assert_success(&f.run(&["start", "app"]));
    wait_for("after hook child to start", || child_pid.exists());
    let group = hook_group(&f, "app");
    let stopped = f.run(&["stop", "--force", "app"]);
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "The after hook was killed.\n");
    wait_for("after hook process group to be killed", || {
        wineo_core::process::group_has_live_members(group) == Some(false)
    });
    assert!(f.state("app").contains("status = \"exited\""));
}

#[test]
fn ctrl_c_while_stop_waits_leaves_the_after_hook_running() {
    let f = Fixture::new();
    let active = f.root.path().join("after-active");
    let release = f.root.path().join("release");
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            None,
            Some(&gate_hook(&active, &release)),
        ),
    );
    assert_success(&f.run(&["start", "app"]));
    wait_for("after hook to start", || active.exists());
    let group = hook_group(&f, "app");

    let stop_output = f.root.path().join("stop.output");
    let stop = spawn_wineo_with_stderr(&f, &["stop", "app"], &stop_output);
    wait_for_text(&stop_output, "Waiting for the after hook.");
    signal(&stop, "-INT");
    let output = stop.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert_eq!(
        wineo_core::process::group_has_live_members(group),
        Some(true),
        "Ctrl+C on stop must leave the after hook running"
    );

    fs::write(release, "").unwrap();
    wait_for("after hook to finish after stop was interrupted", || {
        wineo_core::process::group_has_live_members(group) == Some(false)
            && locks(&f).busy_with("app").unwrap().is_none()
    });
}

#[test]
fn restart_waits_for_after_before_starting_again() {
    let f = Fixture::new();
    let active = f.root.path().join("after-active");
    let release = f.root.path().join("release");
    let before = f.root.path().join("before");
    let after = format!(
        "if [ ! -e {} ]; then touch {}; while [ ! -e {} ]; do sleep 0.02; done; fi",
        shell_quote(&release.display().to_string()),
        shell_quote(&active.display().to_string()),
        shell_quote(&release.display().to_string())
    );
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            Some(&append_marker(&before, "ran")),
            Some(&after),
        ),
    );
    assert_success(&f.run(&["start", "app"]));
    f.wait_for_run_end("app", "exited");
    wait_for("first after hook to wait", || active.exists());

    let restart_output = f.root.path().join("restart.output");
    let restart = f
        .wineo()
        .args(["restart", "app"])
        .stdout(Stdio::piped())
        .stderr(Stdio::from(File::create(&restart_output).unwrap()))
        .spawn()
        .unwrap();
    wait_for_text(&restart_output, "Waiting for the after hook.");
    assert_eq!(fs::read_to_string(&before).unwrap(), "ran\n");
    fs::write(&release, "").unwrap();
    let restarted = restart.wait_with_output().unwrap();
    assert_success(&restarted);
    assert_eq!(stdout(&restarted), "Started app\n");
    wait_for("restart to run its before hook", || {
        fs::read_to_string(&before).is_ok_and(|text| text.lines().count() == 2)
    });
}

#[test]
fn restart_during_before_reports_started() {
    let f = Fixture::new();
    let active = f.root.path().join("before-active");
    let release = f.root.path().join("release");
    let before = format!(
        "if [ ! -e {} ]; then {}; fi",
        shell_quote(&active.display().to_string()),
        gate_hook(&active, &release)
    );
    f.environment(
        "app",
        &hooked_targets("[targets.app]\nexe = 'exit-0'\n", Some(&before), None),
    );

    let first = spawn_wineo(&f, &["start", "app"]);
    wait_for("before hook to start", || active.exists());
    let restarted = f.run(&["restart", "app"]);
    assert_success(&restarted);
    assert_eq!(stdout(&restarted), "Started app\n");
    let first_output = first.wait_with_output().unwrap();
    assert!(!first_output.status.success(), "{}", stderr(&first_output));
    fs::write(release, "").unwrap();
}

#[test]
fn ctrl_c_twice_during_after_waits_then_kills_the_hook_group() {
    let f = Fixture::new();
    let active = f.root.path().join("after-active");
    let child_pid = f.root.path().join("child-pid");
    let after = format!(
        "touch {}; sleep 60 & echo $! > {}; while :; do sleep 0.02; done",
        shell_quote(&active.display().to_string()),
        shell_quote(&child_pid.display().to_string())
    );
    f.environment(
        "app",
        &hooked_targets("[targets.app]\nexe = 'exit-0'\n", None, Some(&after)),
    );
    let stderr_path = f.root.path().join("run.stderr");
    let run = spawn_wineo_with_stderr(&f, &["run", "app"], &stderr_path);
    wait_for("after hook child to start", || child_pid.exists());
    let group = hook_group(&f, "app");

    signal(&run, "-INT");
    let waiting = b"Waiting for the after hook, press Ctrl+C again to kill it.";
    wait_for_text(&stderr_path, std::str::from_utf8(waiting).unwrap());
    signal(&run, "-INT");
    let output = run.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let text = fs::read_to_string(&stderr_path).unwrap();
    assert!(text.ends_with("The after hook was killed.\n"), "{text}");
    assert_eq!(
        text.matches("The after hook was killed.").count(),
        1,
        "{text}"
    );
    wait_for("run after hook group to be killed", || {
        wineo_core::process::group_has_live_members(group) == Some(false)
    });
}

#[test]
fn ctrl_c_during_the_program_announces_after_so_the_next_one_kills_it() {
    let f = Fixture::new();
    let active = f.root.path().join("after-active");
    let after = format!(
        "touch {}; while :; do sleep 0.02; done",
        shell_quote(&active.display().to_string())
    );
    f.environment(
        "app",
        &hooked_targets("[targets.app]\nexe = 'game'\n", None, Some(&after)),
    );
    let stderr_path = f.root.path().join("run.stderr");
    let run = spawn_wineo_with_stderr(&f, &["run", "app"], &stderr_path);
    wait_for("game to run", || f.fake_program_running("app"));

    signal(&run, "-INT");
    wait_for("after hook to start", || active.exists());
    wait_for_text(
        &stderr_path,
        "Waiting for the after hook, press Ctrl+C again to kill it.",
    );
    let group = hook_group(&f, "app");
    signal(&run, "-INT");
    run.wait_with_output().unwrap();
    let text = fs::read_to_string(&stderr_path).unwrap();
    assert_eq!(
        text.matches("Waiting for the after hook").count(),
        1,
        "{text}"
    );
    assert!(text.ends_with("The after hook was killed.\n"), "{text}");
    assert_eq!(
        text.matches("The after hook was killed.").count(),
        1,
        "{text}"
    );
    wait_for("run after hook group to be killed", || {
        wineo_core::process::group_has_live_members(group) == Some(false)
    });
}

#[test]
fn repeated_hangup_and_terminate_do_not_force_a_wine_run() {
    for signal_name in ["-HUP", "-TERM"] {
        let f = Fixture::new();
        f.global_config("stop_timeout = 1\n");
        f.environment("app", "[targets.app]\nexe = 'stubborn'\n");
        let stderr_path = f.root.path().join("run.stderr");
        let mut run = spawn_wineo_with_stderr(&f, &["run", "app"], &stderr_path);
        wait_for("stubborn game to run", || f.fake_program_running("app"));

        signal(&run, signal_name);
        wait_for_text(&stderr_path, "Stopping app, press Ctrl+C again to force it");
        signal(&run, signal_name);
        thread::sleep(Duration::from_millis(200));
        assert!(
            run.try_wait().unwrap().is_none(),
            "repeated {signal_name} forced the run"
        );

        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if run.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = run.kill();
                panic!("repeated {signal_name} did not finish its graceful stop");
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(run.wait().unwrap().code(), Some(137));
        assert!(f.state("app").contains("status = \"stopped\""));
    }
}

#[test]
fn a_second_ctrl_c_forces_a_wine_run() {
    let f = Fixture::new();
    f.global_config("stop_timeout = 10\n");
    f.environment("app", "[targets.app]\nexe = 'stubborn'\n");
    let stderr_path = f.root.path().join("run.stderr");
    let mut run = spawn_wineo_with_stderr(&f, &["run", "app"], &stderr_path);
    wait_for("stubborn game to run", || f.fake_program_running("app"));

    signal(&run, "-INT");
    wait_for_text(&stderr_path, "Stopping app, press Ctrl+C again to force it");
    signal(&run, "-INT");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if run.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = run.kill();
            panic!("second Ctrl+C did not force the run");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(run.wait().unwrap().code(), Some(137));
    assert!(f.state("app").contains("status = \"stopped\""));
}

#[test]
fn repeated_stop_signals_do_not_force_a_detached_wine_supervisor() {
    for signal_name in ["-HUP", "-TERM"] {
        let f = Fixture::new();
        f.global_config("stop_timeout = 1\n");
        f.environment("app", "[targets.app]\nexe = 'stubborn'\n");
        assert_success(&f.run(&["start", "app"]));
        wait_for("stubborn game to run", || f.fake_program_running("app"));

        let deadline = Instant::now() + Duration::from_secs(2);
        let supervisor = loop {
            if let Some(pid) = detached_supervisor_pid(&f) {
                break pid;
            }
            assert!(
                Instant::now() < deadline,
                "detached supervisor was not found"
            );
            thread::sleep(Duration::from_millis(20));
        };
        signal_pid(supervisor, signal_name);
        wait_for("detached supervisor to begin stopping", || {
            fs::read_to_string(f.runtime("app").log_path())
                .is_ok_and(|text| text.contains("stopping, asking programs to close"))
        });
        signal_pid(supervisor, signal_name);
        thread::sleep(Duration::from_millis(200));
        assert!(
            f.runtime("app").is_running().unwrap(),
            "repeated {signal_name} forced the detached run"
        );

        wait_for("detached graceful stop to finish", || {
            !f.runtime("app").is_running().unwrap()
        });
        assert!(f.state("app").contains("status = \"stopped\""));
    }
}

#[test]
fn force_stop_of_the_program_says_how_to_kill_the_after_hook() {
    let f = Fixture::new();
    let active = f.root.path().join("after-active");
    let release = f.root.path().join("release");
    let after = gate_hook(&active, &release);
    f.environment(
        "app",
        &hooked_targets("[targets.app]\nexe = 'game'\n", None, Some(&after)),
    );
    assert_success(&f.run(&["start", "app"]));
    wait_for("game to run", || f.fake_program_running("app"));

    let stop_output = f.root.path().join("stop.output");
    let stop = f
        .wineo()
        .args(["stop", "--force", "app"])
        .stdout(Stdio::piped())
        .stderr(Stdio::from(File::create(&stop_output).unwrap()))
        .spawn()
        .unwrap();
    wait_for_text(
        &stop_output,
        "Waiting for the after hook, run stop --force again to kill it.",
    );
    fs::write(release, "").unwrap();
    let stopped = stop.wait_with_output().unwrap();
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "Stopped app\n");
}

#[test]
fn after_uses_stopped_values_and_failed_after_is_only_a_warning() {
    let f = Fixture::new();
    let captured = f.root.path().join("end");
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'stubborn'\n",
            None,
            Some(&captured_end(&captured)),
        ),
    );
    assert_success(&f.run(&["start", "app"]));
    wait_for("game to run", || f.fake_program_running("app"));
    assert_success(&f.run(&["stop", "app"]));
    wait_for_text(&captured, "stopped|");
    assert!(
        fs::read_to_string(captured)
            .unwrap()
            .starts_with("stopped|")
    );

    let f = Fixture::new();
    let captured = f.root.path().join("end");
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'stubborn'\n",
            None,
            Some(&captured_end(&captured)),
        ),
    );
    assert_success(&f.run(&["start", "app"]));
    wait_for("game to run", || f.fake_program_running("app"));
    assert_success(&f.run(&["stop", "--force", "app"]));
    wait_for_text(&captured, "stopped||");
    assert!(
        fs::read_to_string(captured)
            .unwrap()
            .starts_with("stopped||"),
        "force stop should leave the program exit code empty"
    );
    let state = f.runtime("app").read_state().unwrap().unwrap();
    assert_eq!(state.status, wineo_core::runtime::RunStatus::Stopped);
    assert_eq!(state.exit_code, Some(137));

    let f = Fixture::new();
    f.environment(
        "app",
        &hooked_targets("[targets.app]\nexe = 'exit-0'\n", None, Some("exit 9")),
    );
    let run = f.run(&["run", "app"]);
    assert_success(&run);
    assert!(stderr(&run).contains("warning:"), "{}", stderr(&run));
}

#[test]
fn lost_run_does_not_run_after() {
    let f = Fixture::new();
    let before = f.root.path().join("before");
    let after = f.root.path().join("after");
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'stubborn'\n",
            Some(&append_marker(&before, "ran")),
            Some(&append_marker(&after, "ran")),
        ),
    );
    assert_success(&f.run(&["start", "app"]));
    wait_for("game to run", || f.fake_program_running("app"));
    let supervisor = f.runtime("app").holder_pid().unwrap().unwrap();
    let killed = Command::new("kill")
        .args(["-KILL", &supervisor.to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    wait_for("lost run lock to release", || {
        !f.runtime("app").is_running().unwrap() && locks(&f).busy_with("app").unwrap().is_none()
    });
    assert!(!after.exists());
    let retry = f.run(&["start", "app"]);
    assert!(
        !retry.status.success(),
        "lost run should refuse a new launch"
    );
    assert_eq!(fs::read_to_string(before).unwrap(), "ran\n");
    assert!(!after.exists());
    assert_success(&f.run(&["stop", "app"]));
}

#[test]
fn failed_spawn_preserves_the_previous_real_run_record() {
    for mode in ["start", "run"] {
        let f = Fixture::new();
        let before = f.root.path().join("before");
        let after = f.root.path().join("after");
        f.environment(
            "app",
            &hooked_targets(
                "[targets.app]\nexe = 'exit-0'\n",
                Some(&append_marker(&before, "ran")),
                Some(&captured_failure(&after)),
            ),
        );

        let first = f.run(&[mode, "app"]);
        assert_success(&first);
        if mode == "start" {
            f.wait_for_run_end("app", "exited");
        }
        // The end record precedes release of the run and after-hook locks.
        wait_for("previous launch to release its locks", || {
            !f.runtime("app").is_running().unwrap() && locks(&f).busy_with("app").unwrap().is_none()
        });
        let previous = f.state("app");
        assert!(previous.contains("status = \"exited\""), "{previous}");

        let wine = f.root.path().join("bin/wine");
        fs::write(&wine, "#!/nonexistent/interpreter\n").unwrap();
        fs::set_permissions(&wine, fs::Permissions::from_mode(0o755)).unwrap();
        let failed = f.run(&[mode, "app"]);
        assert!(!failed.status.success(), "{mode}: {}", stderr(&failed));
        assert_eq!(
            f.state("app"),
            previous,
            "{mode} overwrote the last real run"
        );
        assert_eq!(fs::read_to_string(before).unwrap(), "ran\nran\n");
    }
}

#[test]
fn exec_winetricks_and_installers_do_not_run_hooks() {
    let f = Fixture::new();
    let hooks = f.root.path().join("hooks");
    let before = append_marker(&hooks, "before");
    let after = append_marker(&hooks, "after");
    f.environment(
        "app",
        &hooked_targets(
            "[targets.app]\nexe = 'exit-0'\n",
            Some(&before),
            Some(&after),
        ),
    );
    assert_success(&f.run(&["exec", "app", "exit-0"]));

    let winetricks = f.root.path().join("bin/winetricks");
    fs::write(&winetricks, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&winetricks, fs::Permissions::from_mode(0o755)).unwrap();
    assert_success(&f.run(&["winetricks", "app", "list-installed"]));

    f.write_template(
        "hooked",
        &format!(
            "[hooks]\nbefore = {}\nafter = {}\n",
            toml_string(&before),
            toml_string(&after)
        ),
    );
    let installer = f.run(&[
        "new",
        "-n",
        "installer",
        "-t",
        "hooked",
        "--install-exe",
        "exit-0",
    ]);
    assert_success(&installer);
    assert!(!hooks.exists());

    let no_hooks = f.run(&["status", "installer", "--json"]);
    assert_success(&no_hooks);
    let json: serde_json::Value = serde_json::from_slice(&no_hooks.stdout).unwrap();
    assert_eq!(
        json["hooks"],
        serde_json::json!({"before": true, "after": true})
    );

    f.environment("plain", "[targets.plain]\nexe = 'exit-0'\n");
    let no_hooks = f.run(&["status", "plain", "--json"]);
    assert_success(&no_hooks);
    let json: serde_json::Value = serde_json::from_slice(&no_hooks.stdout).unwrap();
    assert_eq!(json["hooks"], serde_json::Value::Null);
}
