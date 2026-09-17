//! Starting, stopping, and restarting against fake wine and wineserver scripts,
//! so the supervisor is covered without wine installed.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use wineo_core::runtime::Runtime;

/// A program named exit-N exits with N right away, a program named stubborn
/// ignores requests to close, and any other program runs until
/// `wineboot --end-session` or `wineserver -k` ends it.
const FAKE_WINE: &str = r#"#!/bin/sh
pids="$WINEPREFIX/fake-pids"
case "$1" in
--version)
    echo wine-fake
    exit 0
    ;;
wineboot)
    if [ "$2" = --init ]; then
        printf 'WINE REGISTRY Version 2\n\n#arch=win64\n' >"$WINEPREFIX/system.reg"
    elif [ "$2" = --end-session ] && read -r program _ 2>/dev/null <"$pids"; then
        kill "$program" 2>/dev/null
    fi
    exit 0
    ;;
reg)
    exit 0
    ;;
esac
echo "fake wine: $*"
echo "fake wine: FAKE_VAR=${FAKE_VAR-}"
case "$1" in
exit-*)
    exit "${1#exit-}"
    ;;
esac
sleep 60 &
if [ "$1" = stubborn ]; then
    trap '' TERM
else
    trap 'kill "$!" 2>/dev/null; rm -f "$pids"; exit 0' TERM
fi
echo "$$ $!" >"$pids"
wait
rm -f "$pids"
"#;

/// Treats the fake wine program as the only process in the prefix.
const FAKE_WINESERVER: &str = r#"#!/bin/sh
pids="$WINEPREFIX/fake-pids"
case "$1" in
-w)
    while read -r program _ 2>/dev/null <"$pids" && kill -0 "$program" 2>/dev/null; do
        sleep 0.1
    done
    ;;
-k)
    if read -r program sleeper 2>/dev/null <"$pids"; then
        kill -9 "$program" "$sleeper" 2>/dev/null
        rm -f "$pids"
    fi
    ;;
esac
exit 0
"#;

struct Fixture {
    root: tempfile::TempDir,
    path_var: OsString,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        for (name, script) in [("wine", FAKE_WINE), ("wineserver", FAKE_WINESERVER)] {
            let path = bin.join(name);
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let path_var =
            std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&inherited)))
                .unwrap();
        Self { root, path_var }
    }

    /// Uses the wine installed on the system instead of the fakes.
    fn real() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            path_var: std::env::var_os("PATH").unwrap_or_default(),
        }
    }

    fn wineo(&self) -> Command {
        let root = self.root.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_wineo"));
        command
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("PATH", &self.path_var)
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.wineo()
            .args(args)
            .output()
            .expect("wineo should start")
    }

    /// Writes an environment config with a fake prefix.
    fn environment(&self, name: &str, targets: &str) {
        let prefix = self.root.path().join("prefixes").join(name);
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n\n#arch=win64\n",
        )
        .unwrap();
        let config = self.config_dir().join(format!("environments/{name}.toml"));
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            config,
            format!(
                "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n\n{targets}",
                prefix.display()
            ),
        )
        .unwrap();
    }

    fn config_dir(&self) -> PathBuf {
        self.root.path().join("config/wineo")
    }

    fn runtime(&self, name: &str) -> Runtime {
        Runtime::at(self.root.path().join("state/wineo").join(name))
    }

    fn state(&self, name: &str) -> String {
        fs::read_to_string(self.runtime(name).dir().join("state.toml")).unwrap_or_default()
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.runtime(name).log_path()).unwrap_or_default()
    }

    fn fake_program_running(&self, name: &str) -> bool {
        self.root
            .path()
            .join("prefixes")
            .join(name)
            .join("fake-pids")
            .exists()
    }
}

impl Drop for Fixture {
    /// A failed assertion must not leave supervisors or fake programs running.
    fn drop(&mut self) {
        let Ok(entries) = fs::read_dir(self.config_dir().join("environments")) else {
            return;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.path().file_stem().and_then(|stem| stem.to_str()) {
                let _ = self
                    .wineo()
                    .args(["stop", "--force", name])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[track_caller]
fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", stderr(output));
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

const APP: &str = "[targets.app]\nexe = \"app\"\n";

#[test]
fn start_then_stop() {
    let f = Fixture::new();
    f.environment("app", APP);

    let started = f.run(&["start", "app"]);
    assert_success(&started);
    assert_eq!(stdout(&started), "Started app\n");
    let state = f.state("app");
    assert!(state.contains("status = \"running\""), "{state}");
    assert!(state.contains("mode = \"detached\""), "{state}");
    wait_for("the program's output in the log", || {
        f.log("app").contains("fake wine: app")
    });

    let again = f.run(&["start", "app"]);
    assert_success(&again);
    assert_eq!(stderr(&again), "app is already running\n");
    let with_options = f.run(&["start", "app", "-e", "FAKE_VAR=1"]);
    assert_success(&with_options);
    assert!(
        stderr(&with_options).contains("options weren't applied"),
        "{}",
        stderr(&with_options)
    );

    let stopped = f.run(&["stop", "app"]);
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "Stopped app\n");
    let state = f.state("app");
    assert!(state.contains("status = \"stopped\""), "{state}");
    assert!(!f.fake_program_running("app"));
    let log = f.log("app");
    assert!(
        log.contains("wineo: stopping, asking programs to close"),
        "{log}"
    );
    assert!(!log.contains("killing"), "{log}");

    let not_running = f.run(&["stop", "app"]);
    assert_success(&not_running);
    assert_eq!(stderr(&not_running), "app is not running\n");
}

#[test]
fn a_program_exiting_on_its_own_records_its_code() {
    let f = Fixture::new();
    f.environment("app", "[targets.fail]\nexe = \"exit-3\"\n");

    assert_success(&f.run(&["start", "app"]));
    wait_for("the run to end", || {
        f.state("app").contains("status = \"failed\"")
    });
    let state = f.state("app");
    assert!(state.contains("exit_code = 3"), "{state}");
    assert!(state.contains("program_exited_at"), "{state}");
    assert!(state.contains("ended_at"), "{state}");
}

#[test]
fn restart_reuses_previous_options_until_new_ones_are_given() {
    let f = Fixture::new();
    f.environment("app", APP);

    let restarted = f.run(&["restart", "app", "-e", "FAKE_VAR=first", "--", "-windowed"]);
    assert_success(&restarted);
    assert_eq!(stdout(&restarted), "Started app\n");
    wait_for("the first run's output", || {
        f.log("app").contains("FAKE_VAR=first")
    });
    assert!(f.log("app").contains("fake wine: app -windowed"));

    let restarted = f.run(&["restart", "app"]);
    assert_success(&restarted);
    assert_eq!(stdout(&restarted), "Restarted app\n");
    wait_for("the second run's output", || {
        f.log("app").contains("FAKE_VAR=first")
    });
    assert!(f.log("app").contains("fake wine: app -windowed"));
    let previous = fs::read_to_string(f.runtime("app").previous_log_path()).unwrap();
    assert!(previous.contains("wineo: stopping"), "{previous}");

    assert_success(&f.run(&["restart", "app", "-e", "FAKE_VAR=second"]));
    wait_for("the third run's output", || {
        f.log("app").contains("FAKE_VAR=second")
    });
    let log = f.log("app");
    assert!(!log.contains("-windowed"), "{log}");
    assert!(f.state("app").contains("status = \"running\""));
}

#[test]
fn restart_checks_the_launch_before_stopping() {
    let f = Fixture::new();
    f.environment("app", APP);
    assert_success(&f.run(&["start", "app"]));

    let restarted = f.run(&["restart", "app", "--target", "nope"]);
    assert!(!restarted.status.success());
    assert!(
        stderr(&restarted).contains("has no target `nope`"),
        "{}",
        stderr(&restarted)
    );
    assert!(f.runtime("app").is_running().unwrap());
}

#[test]
fn starting_another_target_while_running_is_an_error() {
    let f = Fixture::new();
    f.environment(
        "app",
        "[targets.a]\nexe = \"app\"\ndefault = true\n[targets.b]\nexe = \"app\"\n",
    );
    assert_success(&f.run(&["start", "app"]));

    let other = f.run(&["start", "app", "--target", "b"]);
    assert!(!other.status.success());
    assert!(
        stderr(&other).contains("already running target `a`"),
        "{}",
        stderr(&other)
    );
    assert_success(&f.run(&["start", "app", "--target", "a"]));
}

#[test]
fn stop_kills_what_ignores_a_request_to_close() {
    let f = Fixture::new();
    fs::create_dir_all(f.config_dir()).unwrap();
    fs::write(f.config_dir().join("config.toml"), "stop_timeout = 1\n").unwrap();
    f.environment("app", "[targets.stubborn]\nexe = \"stubborn\"\n");
    assert_success(&f.run(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));

    let began = Instant::now();
    assert_success(&f.run(&["stop", "app"]));
    assert!(began.elapsed() >= Duration::from_secs(1));
    assert!(!f.fake_program_running("app"));
    let log = f.log("app");
    assert!(log.contains("wineo: stopping"), "{log}");
    assert!(
        log.contains("wineo: killing everything left in the prefix"),
        "{log}"
    );
    assert!(f.state("app").contains("status = \"stopped\""));
}

#[test]
fn force_stop_kills_right_away() {
    let f = Fixture::new();
    fs::create_dir_all(f.config_dir()).unwrap();
    fs::write(f.config_dir().join("config.toml"), "stop_timeout = 30\n").unwrap();
    f.environment("app", "[targets.stubborn]\nexe = \"stubborn\"\n");
    assert_success(&f.run(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));

    let began = Instant::now();
    let stopped = f.run(&["stop", "--force", "app"]);
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "Stopped app\n");
    assert!(began.elapsed() < Duration::from_secs(10));
    assert!(!f.fake_program_running("app"));
    let log = f.log("app");
    assert!(!log.contains("wineo: stopping"), "{log}");
    assert!(log.contains("wineo: killing"), "{log}");
    assert!(f.state("app").contains("status = \"stopped\""));
}

#[test]
fn start_reports_wine_failing_to_spawn() {
    let f = Fixture::new();
    f.environment("app", APP);
    let wine = f.root.path().join("bin/wine");
    fs::write(&wine, "#!/nonexistent/interpreter\n").unwrap();

    let started = f.run(&["start", "app"]);
    assert!(!started.status.success());
    assert!(
        stderr(&started).contains("failed to start app: failed to start wine"),
        "{}",
        stderr(&started)
    );
    assert_eq!(f.state("app"), "");
    assert!(!f.runtime("app").is_running().unwrap());
}

#[test]
fn stop_ends_an_attached_run() {
    let f = Fixture::new();
    f.environment("app", APP);
    let run = f
        .wineo()
        .args(["run", "app"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("the attached run to start", || {
        f.fake_program_running("app")
    });
    assert!(f.state("app").contains("mode = \"attached\""));

    let stopped = f.run(&["stop", "app"]);
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "Stopped app\n");
    let output = run.wait_with_output().unwrap();
    assert_success(&output);
    assert!(stdout(&output).contains("fake wine: app"));
    assert!(
        stderr(&output).contains("Stopping app, press Ctrl+C again to force it"),
        "{}",
        stderr(&output)
    );
    assert!(f.state("app").contains("status = \"stopped\""));
}

#[test]
fn stop_cleans_up_after_a_dead_supervisor() {
    let f = Fixture::new();
    f.environment("app", APP);
    assert_success(&f.run(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));

    let supervisor = f.runtime("app").holder_pid().unwrap().unwrap();
    let killed = Command::new("kill")
        .args(["-KILL", &supervisor.to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    wait_for("the lock to be released", || {
        !f.runtime("app").is_running().unwrap()
    });
    assert!(f.runtime("app").status().unwrap().lost);
    assert!(f.fake_program_running("app"));

    let stopped = f.run(&["stop", "app"]);
    assert_success(&stopped);
    assert!(
        stderr(&stopped).contains("lost track"),
        "{}",
        stderr(&stopped)
    );
    assert!(!f.fake_program_running("app"));
}

#[test]
fn following_logs_ends_with_the_run() {
    let f = Fixture::new();
    f.environment("app", APP);
    assert_success(&f.run(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));

    let mut follow = f
        .wineo()
        .args(["logs", "app", "-f"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_success(&f.run(&["stop", "app"]));

    wait_for("logs -f to exit", || {
        matches!(follow.try_wait(), Ok(Some(_)))
    });
    let output = follow.wait_with_output().unwrap();
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("fake wine: app"), "{text}");
    assert!(
        text.contains("wineo: stopping, asking programs to close"),
        "{text}"
    );
}

#[test]
#[ignore = "runs wine and opens notepad"]
fn start_status_logs_restart_and_stop_with_real_wine() {
    let f = Fixture::real();
    assert_success(&f.run(&["new", "-n", "notepad"]));
    assert_success(&f.run(&["set", "notepad", "targets.notepad.exe=notepad"]));

    let started = f.run(&["start", "notepad"]);
    assert_success(&started);
    assert_eq!(stdout(&started), "Started notepad\n");
    // Let notepad open its window, so it can be asked to close.
    thread::sleep(Duration::from_secs(3));

    let status = f.run(&["status"]);
    assert_success(&status);
    assert!(
        stdout(&status).starts_with("notepad "),
        "{}",
        stdout(&status)
    );
    let status = stdout(&f.run(&["status", "notepad"]));
    assert!(status.starts_with("notepad: running since "), "{status}");
    assert!(status.contains("  runner  wine (wine-"), "{status}");
    let list = stdout(&f.run(&["list"]));
    assert!(list.contains("running"), "{list}");
    assert_success(&f.run(&["logs", "notepad", "-n", "5"]));

    let restarted = f.run(&["restart", "notepad"]);
    assert_success(&restarted);
    assert_eq!(stdout(&restarted), "Restarted notepad\n");
    let previous = fs::read_to_string(f.runtime("notepad").previous_log_path()).unwrap();
    assert!(previous.contains("wineo: stopping"), "{previous}");
    thread::sleep(Duration::from_secs(3));

    let stopped = f.run(&["stop", "notepad"]);
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "Stopped notepad\n");
    let status = stdout(&f.run(&["status", "notepad"]));
    assert!(status.starts_with("notepad: stopped at "), "{status}");

    assert_success(&f.run(&["start", "notepad"]));
    thread::sleep(Duration::from_secs(3));
    let closed = f.run(&["exec", "notepad", "taskkill", "/im", "notepad.exe"]);
    assert_success(&closed);
    wait_for("notepad's run to end", || {
        !f.runtime("notepad").is_running().unwrap()
    });
    let status = stdout(&f.run(&["status", "notepad"]));
    assert!(
        status.starts_with("notepad: exited with code 0 at "),
        "{status}"
    );

    let applications = f.root.path().join("data/applications");
    let entries: Vec<_> = fs::read_dir(&applications)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    assert!(entries.is_empty(), "{entries:?}");
}
