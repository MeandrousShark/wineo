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
/// ignores requests to close, a program named slow takes a moment before
/// wineserver can see it, noisy writes megabytes in bursts, noisy-exit leaves
/// a background writer behind, and any other program runs until
/// `wineboot --end-session` or `wineserver -k` ends it.
const FAKE_WINE: &str = r#"#!/bin/sh
pids="$WINEPREFIX/fake-pids"
burst() {
    line=$(printf 'x%.0s' $(seq 1023))
    i=0
    while [ "$i" -lt 2048 ]; do
        echo "$line"
        i=$((i + 1))
    done
}
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
slow)
    sleep 0.5
    ;;
noisy)
    # Two megabytes, a pause long enough for the size check to run, then two
    # more, so the log is cleared while the program is still going.
    burst
    sleep 2
    burst
    ;;
noisy-exit)
    # Exits right away, leaving something in the prefix still writing.
    (sleep 0.5; burst; burst; rm -f "$pids") &
    echo "$! $!" >"$pids"
    exit 0
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

    /// Installs the same fakes as a drop-in runner instead of putting them on
    /// PATH, so a test can prove resolution reaches the supervisor.
    fn install_runner(&self, name: &str) {
        let bin = self
            .root
            .path()
            .join("data/wineo/runners")
            .join(name)
            .join("bin");
        fs::create_dir_all(&bin).unwrap();
        for (file, script) in [("wine", FAKE_WINE), ("wineserver", FAKE_WINESERVER)] {
            let path = bin.join(file);
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// wineo with the fakes off PATH entirely, so anything that still reaches
    /// for the system wine fails loudly instead of quietly working.
    fn wineo_without_fakes(&self) -> Command {
        let mut command = self.wineo();
        command.env("PATH", std::env::var_os("PATH").unwrap_or_default());
        command
    }

    fn run_without_fakes(&self, args: &[&str]) -> Output {
        self.wineo_without_fakes()
            .args(args)
            .output()
            .expect("wineo should start")
    }

    /// Writes an environment config with a fake prefix.
    fn environment(&self, name: &str, targets: &str) {
        self.environment_with_runner(name, "wine", targets);
    }

    fn environment_with_runner(&self, name: &str, runner: &str, targets: &str) {
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
                "prefix = \"{}\"\nrunner = \"{runner}\"\narch = \"win64\"\n\n{targets}",
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
const NOISY: &str = "[targets.noisy]\nexe = \"noisy\"\n";

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
fn force_stop_catches_a_program_that_was_still_starting() {
    let f = Fixture::new();
    f.environment("app", "[targets.slow]\nexe = \"slow\"\n");
    assert_success(&f.run(&["start", "app"]));

    let began = Instant::now();
    let stopped = f.run(&["stop", "--force", "app"]);
    assert_success(&stopped);
    assert!(began.elapsed() < Duration::from_secs(5));
    assert!(!f.fake_program_running("app"));
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
    assert!(f.state("app").contains("status = \"stopped\""));

    // The cleanup happens once, so a later stop leaves the prefix alone.
    let again = f.run(&["stop", "app"]);
    assert_success(&again);
    assert_eq!(stderr(&again), "app is not running\n");
}

#[test]
fn a_corrupt_state_file_does_not_block_start_stop_or_logs() {
    let f = Fixture::new();
    f.environment("app", APP);
    let state = f.runtime("app").dir().join("state.toml");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&state, "").unwrap();
    fs::write(f.runtime("app").log_path(), "old output\n").unwrap();

    let logs = f.run(&["logs", "app"]);
    assert_success(&logs);
    assert_eq!(stdout(&logs), "old output\n");
    let stopped = f.run(&["stop", "app"]);
    assert_success(&stopped);
    assert!(
        stderr(&stopped).contains("invalid state file"),
        "{}",
        stderr(&stopped)
    );

    assert_success(&f.run(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));
    fs::write(&state, "").unwrap();
    let stopped = f.run(&["stop", "app"]);
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "Stopped app\n");
}

#[test]
fn a_broken_global_config_does_not_block_stop() {
    let f = Fixture::new();
    f.environment("app", APP);
    assert_success(&f.run(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));
    fs::write(f.config_dir().join("config.toml"), "typo_key = 1\n").unwrap();

    let stopped = f.run(&["stop", "app"]);
    assert_success(&stopped);
    assert_eq!(stdout(&stopped), "Stopped app\n");
    assert!(
        stderr(&stopped).contains("warning: invalid config"),
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

impl Fixture {
    fn global_config(&self, text: &str) {
        let path = self.config_dir().join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn resets(&self, name: &str) -> u64 {
        let mut path = self.runtime(name).log_path().into_os_string();
        path.push(".reset");
        fs::read_to_string(PathBuf::from(path))
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(0)
    }
}

#[test]
fn an_oversized_log_is_cleared_while_the_run_goes_on() {
    let f = Fixture::new();
    f.global_config("log_limit_mb = 1\n");
    f.environment("noisy", NOISY);
    assert_success(&f.run(&["start", "noisy"]));

    wait_for("the log to be cleared", || f.resets("noisy") > 0);
    assert_success(&f.run(&["stop", "noisy"]));

    let text = f.log("noisy");
    assert!(
        text.contains("wineo: the log passed 1 MB; all output before this reset was dropped"),
        "{}",
        &text[..text.len().min(400)]
    );
    // Far less than the four megabytes the program wrote. Not a hard bound:
    // output between two checks can push it past the limit for a while.
    assert!(text.len() < 4 * 1024 * 1024, "{} bytes kept", text.len());
    // The second burst was written after the first reset, so the cap kept going.
    assert!(f.resets("noisy") >= 2, "only {} resets", f.resets("noisy"));
}

#[test]
fn a_disabled_log_limit_keeps_everything() {
    let f = Fixture::new();
    f.global_config("log_limit_mb = 0\n");
    f.environment("noisy", NOISY);
    assert_success(&f.run(&["start", "noisy"]));

    // Waits for the whole second burst, so a cap would have had every chance.
    wait_for("both bursts to be written", || {
        f.log("noisy").len() > 4 * 1024 * 1024
    });
    assert_success(&f.run(&["stop", "noisy"]));

    assert_eq!(f.resets("noisy"), 0);
    assert!(!f.log("noisy").contains("all output before this reset"));
}

#[test]
fn a_previous_log_is_left_at_whatever_size_it_was() {
    let f = Fixture::new();
    f.global_config("log_limit_mb = 0\n");
    f.environment("noisy", NOISY);
    assert_success(&f.run(&["start", "noisy"]));
    wait_for("both bursts to be written", || {
        f.log("noisy").len() > 4 * 1024 * 1024
    });
    assert_success(&f.run(&["stop", "noisy"]));

    // The next run's cap has nothing to say about the log before it.
    f.global_config("log_limit_mb = 1\n");
    f.environment("app", APP);
    assert_success(&f.run(&["start", "noisy"]));
    wait_for("the new log to be cleared", || f.resets("noisy") > 0);
    assert_success(&f.run(&["stop", "noisy"]));

    let previous = fs::read_to_string(f.runtime("noisy").previous_log_path()).unwrap();
    assert!(previous.len() > 4 * 1024 * 1024, "{} bytes", previous.len());
    assert!(!previous.contains("all output before this reset"));
    // And each run counts its own resets.
    assert!(f.resets("noisy") >= 1);
}

#[test]
#[ignore = "runs wine and opens notepad"]
fn an_oversized_real_wine_log_is_cleared() {
    let f = Fixture::real();
    f.global_config("log_limit_mb = 1\n");
    assert_success(&f.run(&["new", "-n", "notepad"]));
    assert_success(&f.run(&["set", "notepad", "targets.notepad.exe=notepad"]));
    assert_success(&f.run(&["start", "notepad", "-e", "WINEDEBUG=+all"]));

    wait_for("wine's debug output to pass the limit", || {
        f.resets("notepad") > 0
    });
    assert_success(&f.run(&["stop", "notepad"]));
    assert!(
        f.log("notepad")
            .contains("wineo: the log passed 1 MB; all output before this reset was dropped")
    );
}

#[test]
fn the_cap_keeps_going_while_background_processes_write() {
    let f = Fixture::new();
    f.global_config("log_limit_mb = 1\n");
    f.environment("bg", "[targets.bg]\nexe = \"noisy-exit\"\n");
    assert_success(&f.run(&["start", "bg"]));

    // The program is gone before any of this is written, so everything that
    // passes the limit comes from what it left running in the prefix.
    wait_for("the program to exit", || {
        f.state("bg").contains("program_exited_at")
    });
    wait_for("the log to be cleared", || f.resets("bg") > 0);

    f.run(&["stop", "bg"]);
    assert!(
        f.log("bg")
            .contains("wineo: the log passed 1 MB; all output before this reset was dropped")
    );
}

#[test]
fn the_log_limit_is_a_global_setting() {
    let f = Fixture::new();
    assert_success(&f.run(&["set", "--global", "log_limit_mb=100"]));
    assert!(
        fs::read_to_string(f.config_dir().join("config.toml"))
            .unwrap()
            .contains("log_limit_mb = 100")
    );
    // It parses back, so the next start picks it up rather than erroring.
    let shown = f.run(&["config"]);
    assert_success(&shown);
    assert!(
        stdout(&shown).contains("log_limit_mb = 100"),
        "{}",
        stdout(&shown)
    );
    assert_success(&f.run(&["set", "--global", "--unset", "log_limit_mb"]));
    assert!(
        !fs::read_to_string(f.config_dir().join("config.toml"))
            .unwrap()
            .contains("log_limit_mb")
    );
}

#[test]
fn a_custom_runner_drives_a_whole_run() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    f.environment_with_runner("app", "wine-tkg", APP);

    // No fakes on PATH, so only the runners root can supply wine.
    assert_success(&f.run_without_fakes(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));

    let listed = f.run_without_fakes(&["runners"]);
    assert_success(&listed);
    assert!(stdout(&listed).contains("wine-tkg"), "{}", stdout(&listed));

    let shown = f.run_without_fakes(&["status", "app"]);
    assert_success(&shown);
    assert!(stdout(&shown).contains("wine-tkg"), "{}", stdout(&shown));

    assert_success(&f.run_without_fakes(&["stop", "app"]));
    assert!(!f.fake_program_running("app"));
    // The fake wine says so in the log, so this could not have been system wine.
    assert!(f.log("app").contains("fake wine: app"), "{}", f.log("app"));
}

#[test]
fn runners_lists_the_system_wine_and_what_is_installed() {
    let f = Fixture::new();
    let listed = f.run(&["runners"]);
    assert_success(&listed);
    // Nothing installed yet, so it says where to put one.
    assert!(stderr(&listed).contains("runners"), "{}", stderr(&listed));
    assert!(stdout(&listed).starts_with("wine"), "{}", stdout(&listed));

    f.install_runner("wine-tkg");
    fs::create_dir_all(f.root.path().join("data/wineo/runners/broken/bin")).unwrap();

    let listed = f.run(&["runners"]);
    assert_success(&listed);
    assert!(stdout(&listed).contains("wine-tkg"), "{}", stdout(&listed));
    assert!(!stdout(&listed).contains("broken"), "{}", stdout(&listed));

    let all = f.run(&["runners", "--all"]);
    assert_success(&all);
    assert!(stdout(&all).contains("broken"), "{}", stdout(&all));
    assert!(stdout(&all).contains("no bin/wine"), "{}", stdout(&all));
}

#[test]
fn new_checks_the_runner_before_creating_anything() {
    let f = Fixture::new();
    let created = f.run(&["new", "-n", "app", "--runner", "nope"]);
    assert!(!created.status.success());
    assert!(
        stderr(&created).contains("is not installed"),
        "{}",
        stderr(&created)
    );
    assert!(!f.config_dir().join("environments/app.toml").exists());
    assert!(!f.root.path().join("data/wineo/prefixes/app").exists());
}

#[test]
fn set_runner_checks_the_name_and_warns_about_the_prefix() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    f.environment("app", APP);

    let bad = f.run(&["set", "app", "runner=nope"]);
    assert!(!bad.status.success());
    assert!(
        stderr(&bad).contains("is not installed"),
        "{}",
        stderr(&bad)
    );
    assert!(
        fs::read_to_string(f.config_dir().join("environments/app.toml"))
            .unwrap()
            .contains("runner = \"wine\"")
    );

    let changed = f.run(&["set", "app", "runner=wine-tkg"]);
    assert_success(&changed);
    assert!(
        stderr(&changed).contains("was not migrated"),
        "{}",
        stderr(&changed)
    );
    assert!(
        fs::read_to_string(f.config_dir().join("environments/app.toml"))
            .unwrap()
            .contains("runner = \"wine-tkg\"")
    );
}

#[test]
fn set_runner_is_refused_while_the_environment_runs() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    f.environment("app", APP);
    assert_success(&f.run(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));

    let refused = f.run(&["set", "app", "runner=wine-tkg"]);
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("is busy"), "{}", stderr(&refused));
    assert!(
        fs::read_to_string(f.config_dir().join("environments/app.toml"))
            .unwrap()
            .contains("runner = \"wine\"")
    );

    assert_success(&f.run(&["stop", "app"]));
    assert_success(&f.run(&["set", "app", "runner=wine-tkg"]));
}

#[test]
fn the_runner_root_can_be_moved() {
    let f = Fixture::new();
    let elsewhere = f.root.path().join("my-runners");
    let bin = elsewhere.join("wine-tkg/bin");
    fs::create_dir_all(&bin).unwrap();
    for (file, script) in [("wine", FAKE_WINE), ("wineserver", FAKE_WINESERVER)] {
        let path = bin.join(file);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Not in the default root, so it is invisible until the setting moves.
    assert!(!stdout(&f.run(&["runners"])).contains("wine-tkg"));
    assert_success(&f.run(&[
        "set",
        "--global",
        &format!("defaults.runner_root={}", elsewhere.display()),
    ]));
    assert!(stdout(&f.run(&["runners"])).contains("wine-tkg"));
}
