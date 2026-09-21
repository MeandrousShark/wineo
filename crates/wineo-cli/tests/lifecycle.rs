//! Starting, stopping, and restarting against fake wine and wineserver scripts,
//! so the supervisor is covered without wine installed.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use wineo_core::runtime::Runtime;

/// A program named exit-N exits with N right away, a program named stubborn
/// ignores requests to close, a program named slow takes a moment before
/// wineserver can see it, noisy writes megabytes in bursts, noisy-exit leaves
/// a background writer behind, handoff exits and leaves something running for
/// a couple of seconds the way an installer hands off to its child,
/// break-wineserver deletes the wineserver beside it, and any other program
/// runs until `wineboot --end-session` or `wineserver -k` ends it.
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
        mkdir -p "$WINEPREFIX/drive_c"
        printf 'WINE REGISTRY Version 2\n\n#arch=win64\n' >"$WINEPREFIX/system.reg"
    elif [ "$2" = --end-session ] && read -r program _ 2>/dev/null <"$pids"; then
        kill "$program" 2>/dev/null
    fi
    exit 0
    ;;
reg)
    echo "$*" >>"$WINEPREFIX/reg.log"
    exit 0
    ;;
winemenubuilder.exe)
    echo "$*" >>"$WINEPREFIX/menubuilder.log"
    exit 1
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
handoff)
    (sleep 2; touch "$WINEPREFIX/handoff-done"; rm -f "$pids") &
    echo "$! $!" >"$pids"
    exit 0
    ;;
break-wineserver)
    rm -f "${0%/*}/wineserver"
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

/// Announces itself into the run's output, then becomes what it was asked to
/// wrap, so a test can see both that it ran and that the program still did.
const FAKE_GAMEMODERUN: &str = r#"#!/bin/sh
echo "gamemoderun wrapped: $1"
exec "$@"
"#;

/// Stands in for umu-launcher: it records what it was asked to do, makes `pfx/`
/// under the compatdata directory when handed an empty program, which is how umu
/// creates a prefix, and otherwise runs like the fake wine so the same programs
/// work under both kinds. Its handoff leaves something in umu's process group.
const FAKE_UMU: &str = r#"#!/bin/sh
echo "umu-run: $*"
echo "umu-run: WINEPREFIX=$WINEPREFIX PROTONPATH=$PROTONPATH"
echo "umu-run: GAMEID=${GAMEID-unset} STORE=${STORE-unset}"
if [ -z "$1" ]; then
    mkdir -p "$WINEPREFIX/pfx/drive_c"
    printf 'WINE REGISTRY Version 2\n\n#arch=win64\n' >"$WINEPREFIX/pfx/system.reg"
    exit 0
fi
echo "umu-run: FAKE_VAR=${FAKE_VAR-}"
pids="$WINEPREFIX/fake-pids"
case "$1" in
reg | winetricks | winecfg)
    exit 0
    ;;
exit-*)
    exit "${1#exit-}"
    ;;
stubborn)
    # Nothing outside the container can ask it to close, so only SIGKILL to
    # the process group ends this.
    trap '' TERM
    echo "$$ $$" >"$pids"
    while :; do sleep 0.1; done
    ;;
handoff)
    # Leaves something in umu's process group after umu itself has exited.
    (sleep 2; touch "$WINEPREFIX/handoff-done") &
    exit 0
    ;;
esac
sleep 60 &
trap 'kill "$!" 2>/dev/null; rm -f "$pids"; exit 0' TERM
echo "$$ $!" >"$pids"
wait
rm -f "$pids"
"#;

struct Fixture {
    root: tempfile::TempDir,
    path_var: OsString,
    /// A PATH holding only the fakes and the handful of real tools they call.
    /// Nothing else is on it, so neither gamemoderun nor winetricks can be
    /// found however the machine running the tests is set up.
    sandbox: OsString,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let write_bin = |dir: &str, scripts: &[(&str, &str)]| {
            let bin = root.path().join(dir);
            fs::create_dir_all(&bin).unwrap();
            for (name, script) in scripts {
                let path = bin.join(name);
                fs::write(&path, script).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            bin
        };
        let wine = [("wine", FAKE_WINE), ("wineserver", FAKE_WINESERVER)];
        let bin = write_bin(
            "bin",
            &[
                wine[0],
                wine[1],
                ("gamemoderun", FAKE_GAMEMODERUN),
                ("umu-run", FAKE_UMU),
            ],
        );
        let sandbox = write_bin("bin-sandbox", &[wine[0], wine[1], ("umu-run", FAKE_UMU)]);
        // The machine running these tests may well have gamemode installed, so
        // the sandbox cannot inherit the real PATH. It gets links to the few
        // tools the fake scripts reach for instead.
        for tool in [
            "sh", "sleep", "rm", "kill", "printf", "seq", "touch", "head", "tr", "mkdir",
        ] {
            if let Some(real) = find_on_path(tool) {
                std::os::unix::fs::symlink(real, sandbox.join(tool)).unwrap();
            }
        }

        let inherited = std::env::var_os("PATH").unwrap_or_default();
        Self {
            path_var: std::env::join_paths(
                std::iter::once(bin).chain(std::env::split_paths(&inherited)),
            )
            .unwrap(),
            sandbox: sandbox.into_os_string(),
            root,
        }
    }

    /// Uses the wine installed on the system instead of the fakes.
    fn real() -> Self {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        Self {
            root: tempfile::tempdir().unwrap(),
            path_var: inherited.clone(),
            sandbox: inherited,
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

    /// A DXVK or vkd3d build of stub DLLs, extracted into its drop-in root.
    /// The DLL bodies name where they came from, so a test can tell an
    /// installed file from what it replaced.
    fn install_build(&self, component: &str, name: &str, dirs: &[&str]) {
        let marker = if component == "dxvk" {
            "dxgi.dll"
        } else {
            "d3d12.dll"
        };
        let dir = self
            .root
            .path()
            .join("data/wineo")
            .join(component)
            .join(name);
        for arch in dirs {
            let arch = dir.join(arch);
            fs::create_dir_all(&arch).unwrap();
            for dll in [marker, "d3d11.dll"] {
                fs::write(arch.join(dll), format!("{name}/{arch:?}")).unwrap();
            }
        }
    }

    fn write_template(&self, name: &str, text: &str) {
        let dir = self.config_dir().join("templates");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.toml")), text).unwrap();
    }

    fn created_config(&self, name: &str) -> String {
        fs::read_to_string(self.config_dir().join(format!("environments/{name}.toml")))
            .unwrap_or_default()
    }

    fn system32(&self, name: &str) -> PathBuf {
        self.root
            .path()
            .join("prefixes")
            .join(name)
            .join("drive_c/windows/system32")
    }

    fn manifest(&self, name: &str) -> String {
        fs::read_to_string(
            self.root
                .path()
                .join("prefixes")
                .join(name)
                .join(".wineo-components.toml"),
        )
        .unwrap_or_default()
    }

    fn reg_log(&self, name: &str) -> String {
        fs::read_to_string(self.root.path().join("prefixes").join(name).join("reg.log"))
            .unwrap_or_default()
    }

    /// A Proton runner: umu only needs the `proton` script to be there, and
    /// wineo reads the name out of the `version` file beside it.
    fn install_proton(&self, name: &str) {
        let dir = self.root.path().join("data/wineo/runners").join(name);
        fs::create_dir_all(&dir).unwrap();
        let proton = dir.join("proton");
        fs::write(&proton, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&proton, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("version"), format!("1717451631 {name}\n")).unwrap();
    }

    /// wineo with the fakes off PATH entirely, so anything that still reaches
    /// for the system wine fails loudly instead of quietly working.
    fn wineo_without_fakes(&self) -> Command {
        let mut command = self.wineo();
        command.env("PATH", std::env::var_os("PATH").unwrap_or_default());
        command
    }

    /// wineo with the fakes on PATH but nothing else, so a test can prove a
    /// command does not need gamemoderun or winetricks installed.
    fn run_sandboxed(&self, args: &[&str]) -> Output {
        self.wineo()
            .env("PATH", &self.sandbox)
            .args(args)
            .output()
            .expect("wineo should start")
    }

    fn run_without_gamemode(&self, args: &[&str]) -> Output {
        self.run_sandboxed(args)
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
        // A win64 prefix, so both architectures have somewhere to go.
        fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/windows/syswow64")).unwrap();
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

    /// A Proton environment: its configured prefix is the compatdata directory
    /// and the wine prefix sits in `pfx/` inside it.
    fn proton_environment(&self, name: &str, runner: &str, targets: &str) {
        let prefix = self.root.path().join("prefixes").join(name);
        let pfx = prefix.join("pfx");
        fs::create_dir_all(pfx.join("drive_c")).unwrap();
        fs::create_dir_all(pfx.join("dosdevices")).unwrap();
        symlink("../drive_c", pfx.join("dosdevices/c:")).unwrap();
        fs::write(
            pfx.join("system.reg"),
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

    /// Whether the process group the last run recorded still exists. The only
    /// way to tell for a program killed outright, which leaves no pid file.
    fn run_group_alive(&self, name: &str) -> bool {
        self.state(name)
            .lines()
            .find_map(|line| line.strip_prefix("group = "))
            .and_then(|value| value.trim().parse::<u32>().ok())
            .is_some_and(|group| PathBuf::from(format!("/proc/{group}")).exists())
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

fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
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
fn start_reconciles_a_desktop_setting_added_by_hand() {
    let f = Fixture::new();
    f.environment("app", "[targets.app]\nexe = \"exit-0\"\n");

    assert_success(&f.run(&["start", "app"]));
    wait_for("the first run to finish", || {
        f.state("app").contains("status = \"exited\"")
    });

    let config = f.config_dir().join("environments/app.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("desktop = true\n");
    fs::write(&config, text).unwrap();

    let started = f.run(&["start", "app"]);
    assert_success(&started);
    assert!(
        f.root
            .path()
            .join("data/applications/wineo-app-app.desktop")
            .is_file()
    );
    assert!(
        stderr(&started).contains("Added a desktop entry for app in app"),
        "{}",
        stderr(&started)
    );
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
    assert!(status.contains("  runner    wine (wine-"), "{status}");
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

#[test]
fn gamemode_wraps_a_started_game() {
    let f = Fixture::new();
    f.global_config("gamemode = true\n");
    f.environment("app", APP);

    assert_success(&f.run(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));

    let shown = f.run(&["status", "app"]);
    assert_success(&shown);
    assert!(stdout(&shown).contains("gamemode"), "{}", stdout(&shown));

    assert_success(&f.run(&["stop", "app"]));
    let log = f.log("app");
    // Wrapped, and the program underneath still ran.
    assert!(log.contains("gamemoderun wrapped:"), "{log}");
    assert!(log.contains("fake wine: app"), "{log}");
}

#[test]
fn a_target_can_opt_out_of_gamemode() {
    let f = Fixture::new();
    f.environment(
        "app",
        "[targets.app]\nexe = \"app\"\n\n[targets.launcher]\nexe = \"launcher\"\ngamemode = false\n",
    );
    assert_success(&f.run(&["set", "app", "gamemode=true"]));

    assert_success(&f.run(&["start", "app", "--target", "launcher"]));
    wait_for("the launcher to start", || f.fake_program_running("app"));
    assert_success(&f.run(&["stop", "app"]));
    let log = f.log("app");
    assert!(!log.contains("gamemoderun"), "{log}");
    assert!(log.contains("fake wine: launcher"), "{log}");

    // The other target still takes the environment's setting.
    assert_success(&f.run(&["start", "app", "--target", "app"]));
    wait_for("the game to start", || f.fake_program_running("app"));
    assert_success(&f.run(&["stop", "app"]));
    assert!(
        f.log("app").contains("gamemoderun wrapped:"),
        "{}",
        f.log("app")
    );
}

#[test]
fn gamemode_never_wraps_tools_in_the_prefix() {
    let f = Fixture::new();
    f.global_config("gamemode = true\n");
    f.environment("app", APP);

    // Running a program in the prefix is not playing a game.
    let ran = f.run(&["exec", "app", "exit-0"]);
    assert_success(&ran);
    assert!(!stdout(&ran).contains("gamemoderun"), "{}", stdout(&ran));
}

#[test]
fn a_launch_is_refused_when_gamemode_is_not_installed() {
    let f = Fixture::new();
    f.global_config("gamemode = true\n");
    f.environment("app", APP);

    for verb in ["start", "run"] {
        let refused = f.run_without_gamemode(&[verb, "app"]);
        assert!(!refused.status.success(), "{verb} was allowed");
        let message = stderr(&refused);
        assert!(
            message.contains("gamemode is on for `app`"),
            "{verb}: {message}"
        );
        assert!(
            message.contains("wineo set app gamemode=false"),
            "{verb}: {message}"
        );
        // Nothing was started, and nothing was recorded as a run.
        assert!(!f.fake_program_running("app"), "{verb} left wine running");
        assert!(f.state("app").is_empty(), "{verb} recorded a run");
    }

    // Turning the setting off is enough to launch again.
    assert_success(&f.run_without_gamemode(&["set", "app", "gamemode=false"]));
    assert_success(&f.run_without_gamemode(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));
    assert_success(&f.run_without_gamemode(&["stop", "app"]));
}

#[test]
fn gamemode_can_be_set_and_unset_at_every_level() {
    let f = Fixture::new();
    f.environment("app", APP);

    assert_success(&f.run(&["set", "--global", "gamemode=true"]));
    assert_success(&f.run(&["set", "app", "gamemode=false"]));
    assert_success(&f.run(&["set", "app", "targets.app.gamemode=true"]));

    let config = fs::read_to_string(f.config_dir().join("environments/app.toml")).unwrap();
    assert!(config.contains("gamemode = false"), "{config}");
    assert!(config.contains("gamemode = true"), "{config}");

    for args in [
        vec!["set", "--global", "--unset", "gamemode"],
        vec!["set", "app", "--unset", "gamemode"],
        vec!["set", "app", "--unset", "targets.app.gamemode"],
    ] {
        assert_success(&f.run(&args));
    }
    assert!(
        !fs::read_to_string(f.config_dir().join("config.toml"))
            .unwrap()
            .contains("gamemode")
    );
    assert!(
        !fs::read_to_string(f.config_dir().join("environments/app.toml"))
            .unwrap()
            .contains("gamemode")
    );
}

#[test]
fn runners_tells_wine_and_proton_apart() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    f.install_proton("GE-Proton9-20");

    let listed = f.run(&["runners"]);
    assert_success(&listed);
    let text = stdout(&listed);
    let row = |name: &str| {
        text.lines()
            .find(|line| line.starts_with(name))
            .unwrap_or_else(|| panic!("no row for {name} in:\n{text}"))
            .to_owned()
    };
    assert!(row("wine-tkg").contains("wine"), "{text}");
    let proton = row("GE-Proton9-20");
    assert!(proton.contains("proton"), "{text}");
    // The version comes from Proton's own file, not from running anything.
    assert!(proton.contains("GE-Proton9-20"), "{text}");
}

#[test]
fn new_creates_a_proton_prefix_through_umu() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");

    let created = f.run(&["new", "-n", "hades", "--runner", "GE-Proton9-20"]);
    assert_success(&created);
    assert!(
        stderr(&created).contains("Steam Linux Runtime"),
        "{}",
        stderr(&created)
    );

    // The configured prefix is the compatdata dir, with the wine prefix inside.
    let prefix = f.root.path().join("data/wineo/prefixes/hades");
    assert!(prefix.join("pfx/system.reg").is_file());
    assert!(!prefix.join("system.reg").exists());

    let config = fs::read_to_string(f.config_dir().join("environments/hades.toml")).unwrap();
    assert!(config.contains("runner = \"GE-Proton9-20\""), "{config}");

    // umu was handed the compatdata dir and the runner root, not pfx/.
    let log = fs::read_to_string(f.runtime("hades").dir().join("create.log")).unwrap();
    assert!(
        log.contains(&format!("WINEPREFIX={}", prefix.display())),
        "{log}"
    );
    assert!(
        log.contains(&format!(
            "PROTONPATH={}",
            f.root
                .path()
                .join("data/wineo/runners/GE-Proton9-20")
                .display()
        )),
        "{log}"
    );
}

#[test]
fn a_proton_prefix_is_removed_whole() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    assert_success(&f.run(&["new", "-n", "hades", "--runner", "GE-Proton9-20"]));

    let prefix = f.root.path().join("data/wineo/prefixes/hades");
    let removed = f.run(&["rm", "hades", "--delete-prefix"]);
    assert_success(&removed);
    assert!(!prefix.exists());
    assert!(!f.config_dir().join("environments/hades.toml").exists());
}

#[test]
fn proton_prefixes_are_always_win64() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    let refused = f.run(&[
        "new",
        "-n",
        "hades",
        "--runner",
        "GE-Proton9-20",
        "--arch",
        "win32",
    ]);
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("win64"), "{}", stderr(&refused));
    assert!(!f.config_dir().join("environments/hades.toml").exists());
}

#[test]
fn protonfixes_settings_reach_umu_on_a_launch_only() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);
    assert_success(&f.run(&["set", "hades", "gameid=umu-hades", "store=gog"]));

    assert_success(&f.run(&["start", "hades"]));
    wait_for("the program to start", || f.fake_program_running("hades"));
    assert_success(&f.run(&["stop", "hades"]));
    assert!(
        f.log("hades").contains("GAMEID=umu-hades STORE=gog"),
        "{}",
        f.log("hades")
    );

    // A tool has no business triggering a game's fix script, which can install
    // things, so umu falls back to umu-default for those.
    for args in [
        vec!["exec", "hades", "winecfg"],
        vec!["winetricks", "hades", "dxvk"],
    ] {
        let printed = stdout(&f.run(&args));
        assert!(
            printed.contains("GAMEID=unset STORE=unset"),
            "{args:?}: {printed}"
        );
    }
}

#[test]
fn a_proton_launch_refuses_to_have_the_protonfixes_variables_set() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);

    for key in ["GAMEID", "STORE"] {
        let refused = f.run(&["start", "hades", "-e", &format!("{key}=x")]);
        assert!(!refused.status.success(), "{key} was allowed");
        assert!(
            stderr(&refused).contains(&format!("{key} is managed by wineo")),
            "{key}: {}",
            stderr(&refused)
        );
    }
}

#[test]
fn gameid_and_store_round_trip_and_show_in_status() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);
    let config = f.config_dir().join("environments/hades.toml");

    assert_success(&f.run(&["set", "hades", "gameid=umu-hades", "store=gog"]));
    let text = fs::read_to_string(&config).unwrap();
    assert!(text.contains("gameid = \"umu-hades\""), "{text}");
    assert!(text.contains("store = \"gog\""), "{text}");

    let shown = stdout(&f.run(&["status", "hades"]));
    assert!(shown.contains("gameid    umu-hades"), "{shown}");
    assert!(shown.contains("store     gog"), "{shown}");

    assert_success(&f.run(&["set", "hades", "--unset", "gameid", "--unset", "store"]));
    let text = fs::read_to_string(&config).unwrap();
    assert!(!text.contains("gameid"), "{text}");
    assert!(!text.contains("store"), "{text}");
    assert!(!stdout(&f.run(&["status", "hades"])).contains("gameid"));
}

#[test]
fn protonfixes_settings_are_refused_on_a_wine_environment() {
    let f = Fixture::new();
    f.environment("app", APP);

    for key in ["gameid", "store"] {
        let refused = f.run(&["set", "app", &format!("{key}=x")]);
        assert!(!refused.status.success(), "{key} was allowed");
        assert!(
            stderr(&refused).contains("only means something to Proton"),
            "{key}: {}",
            stderr(&refused)
        );
    }
    assert!(
        !fs::read_to_string(f.config_dir().join("environments/app.toml"))
            .unwrap()
            .contains("gameid")
    );
}

#[test]
fn set_runner_refuses_a_change_across_kinds() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    f.install_proton("GE-Proton9-20");
    f.environment("app", APP);
    f.proton_environment("hades", "GE-Proton9-20", HADES);

    let to_proton = f.run(&["set", "app", "runner=GE-Proton9-20"]);
    assert!(!to_proton.status.success());
    assert!(
        stderr(&to_proton).contains("prefix layouts are not compatible"),
        "{}",
        stderr(&to_proton)
    );
    let to_wine = f.run(&["set", "hades", "runner=wine-tkg"]);
    assert!(!to_wine.status.success());
    assert!(
        stderr(&to_wine).contains("prefix layouts are not compatible"),
        "{}",
        stderr(&to_wine)
    );

    // Neither config was touched, and a change within a kind still works.
    assert!(
        fs::read_to_string(f.config_dir().join("environments/hades.toml"))
            .unwrap()
            .contains("runner = \"GE-Proton9-20\"")
    );
    assert_success(&f.run(&["set", "app", "runner=wine-tkg"]));
}

#[test]
fn rm_refuses_a_compatdata_dir_that_holds_no_prefix() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);
    // Whatever is in there, it is not a prefix wineo made.
    let prefix = f.root.path().join("prefixes/hades");
    fs::remove_file(prefix.join("pfx/system.reg")).unwrap();

    let refused = f.run(&["rm", "hades", "--delete-prefix"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("is not a wine prefix"),
        "{}",
        stderr(&refused)
    );
    assert!(prefix.exists());
    assert!(f.config_dir().join("environments/hades.toml").exists());
}

/// Installs the Proton build `WINEO_TEST_PROTON` names into the fixture's
/// runners root and returns its name. For the ignored tests only, which say
/// what to set rather than passing quietly when it is missing.
fn real_proton(f: &Fixture) -> String {
    let source = PathBuf::from(std::env::var("WINEO_TEST_PROTON").expect(
        "set WINEO_TEST_PROTON to a Proton build directory, the one holding its `proton` script",
    ));
    assert!(
        source.join("proton").is_file(),
        "{} has no `proton` script",
        source.display()
    );
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .expect("a Proton build directory has a name")
        .to_owned();
    let runners = f.root.path().join("data/wineo/runners");
    fs::create_dir_all(&runners).unwrap();
    symlink(&source, runners.join(&name)).unwrap();
    name
}

#[test]
#[ignore = "runs umu, which downloads the Steam Linux Runtime on first use"]
fn a_real_proton_environment_is_created_used_and_removed() {
    let f = Fixture::real();
    let runner = real_proton(&f);
    assert!(stdout(&f.run(&["runners"])).contains("proton"));

    assert_success(&f.run(&["new", "-n", "hades", "--runner", &runner]));
    let prefix = f.root.path().join("data/wineo/prefixes/hades");
    assert!(prefix.join("pfx/system.reg").is_file());

    // umu ships its own winetricks, so this needs none installed.
    assert_success(&f.run(&["winetricks", "hades", "list-installed"]));

    assert_success(&f.run(&["set", "hades", "gameid=umu-default"]));
    assert_success(&f.run(&["set", "hades", "targets.notepad.exe=notepad"]));
    assert_success(&f.run(&["start", "hades"]));
    // Let the runtime come up and notepad open its window.
    thread::sleep(Duration::from_secs(10));
    let status = stdout(&f.run(&["status", "hades"]));
    assert!(status.starts_with("hades: running"), "{status}");
    assert!(status.contains("gameid    umu-default"), "{status}");

    assert_success(&f.run(&["stop", "hades"]));
    assert!(f.state("hades").contains("status = \"stopped\""));

    assert_success(&f.run(&["rm", "hades", "--delete-prefix"]));
    assert!(!prefix.exists());
}

#[test]
fn tools_on_proton_run_inside_umu() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);
    let prefix = f.root.path().join("prefixes/hades");
    let proton = f.root.path().join("data/wineo/runners/GE-Proton9-20");

    // Sandboxed, so neither tool can be reaching a winetricks or a wine that
    // happens to be installed on the machine running this.
    for (args, expected) in [
        (vec!["exec", "hades", "winecfg"], "umu-run: winecfg"),
        (
            vec!["winetricks", "hades", "dxvk"],
            "umu-run: winetricks dxvk",
        ),
    ] {
        let out = f.run_sandboxed(&args);
        assert_success(&out);
        let printed = stdout(&out);
        assert!(printed.contains(expected), "{args:?}: {printed}");
        assert!(
            printed.contains(&format!("WINEPREFIX={}", prefix.display())),
            "{args:?}: {printed}"
        );
        assert!(
            printed.contains(&format!("PROTONPATH={}", proton.display())),
            "{args:?}: {printed}"
        );
    }
}

const HADES: &str = "[targets.hades]\nexe = \"hades\"\n";

#[test]
fn a_proton_target_launches_through_umu() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);

    assert_success(&f.run(&["start", "hades"]));
    wait_for("the program to start", || f.fake_program_running("hades"));

    let log = f.log("hades");
    assert!(log.contains("umu-run: hades"), "{log}");
    // The compatdata directory, not the wine prefix inside it.
    let prefix = f.root.path().join("prefixes/hades");
    assert!(
        log.contains(&format!("WINEPREFIX={}", prefix.display())),
        "{log}"
    );
    assert!(
        log.contains(&format!(
            "PROTONPATH={}",
            f.root
                .path()
                .join("data/wineo/runners/GE-Proton9-20")
                .display()
        )),
        "{log}"
    );
    // Nothing sets these until an environment can ask for protonfixes.
    assert!(log.contains("GAMEID=unset STORE=unset"), "{log}");

    let shown = stdout(&f.run(&["status", "hades"]));
    assert!(shown.contains("running"), "{shown}");
    assert!(shown.contains("GE-Proton9-20"), "{shown}");

    assert_success(&f.run(&["stop", "hades"]));
    assert!(!f.fake_program_running("hades"));
    assert!(f.state("hades").contains("status = \"stopped\""));
}

#[test]
fn a_proton_launch_resolves_windows_paths_inside_pfx() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment(
        "hades",
        "GE-Proton9-20",
        "[targets.hades]\nexe = 'C:\\Games\\hades.exe'\n",
    );
    let games = f.root.path().join("prefixes/hades/pfx/drive_c/Games");
    fs::create_dir_all(&games).unwrap();
    fs::write(games.join("hades.exe"), "").unwrap();

    assert_success(&f.run(&["start", "hades"]));
    wait_for("the program's output in the log", || {
        f.log("hades").contains("umu-run:")
    });
    assert!(
        f.log("hades").contains(r"umu-run: C:\Games\hades.exe"),
        "{}",
        f.log("hades")
    );
    assert_success(&f.run(&["stop", "--force", "hades"]));
}

#[test]
fn gamemode_wraps_a_proton_launch() {
    let f = Fixture::new();
    f.global_config("gamemode = true\n");
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);

    assert_success(&f.run(&["start", "hades"]));
    wait_for("the program to start", || f.fake_program_running("hades"));
    assert_success(&f.run(&["stop", "hades"]));

    let log = f.log("hades");
    assert!(log.contains("gamemoderun wrapped:"), "{log}");
    assert!(log.contains("umu-run: hades"), "{log}");
}

#[test]
fn a_proton_launch_refuses_to_have_protonpath_set() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);

    let refused = f.run(&["start", "hades", "-e", "PROTONPATH=/elsewhere"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("PROTONPATH is managed by wineo"),
        "{}",
        stderr(&refused)
    );

    assert_success(&f.run(&["set", "hades", "env.PROTONPATH=/elsewhere"]));
    let refused = f.run(&["start", "hades"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("PROTONPATH is managed by wineo"),
        "{}",
        stderr(&refused)
    );

    // A wine environment has no PROTONPATH of wineo's to clash with.
    f.environment("app", APP);
    assert_success(&f.run(&["start", "app", "-e", "PROTONPATH=/elsewhere"]));
}

#[test]
fn a_proton_run_ends_when_umu_exits() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment(
        "hades",
        "GE-Proton9-20",
        "[targets.hades]\nexe = \"exit-3\"\n",
    );

    assert_success(&f.run(&["start", "hades"]));
    wait_for("the run to end", || {
        f.state("hades").contains("status = \"failed\"")
    });
    let state = f.state("hades");
    assert!(state.contains("exit_code = 3"), "{state}");
    // umu waits for the game, so its exit is the end of the run. Nothing can
    // still be running in a container that is gone.
    assert!(!state.contains("program_exited_at"), "{state}");

    let shown = stdout(&f.run(&["status", "hades"]));
    assert!(shown.contains("failed with code 3"), "{shown}");
}

#[test]
fn stop_forces_a_proton_run_that_ignores_sigterm() {
    let f = Fixture::new();
    f.global_config("stop_timeout = 1\n");
    f.install_proton("GE-Proton9-20");
    f.proton_environment(
        "hades",
        "GE-Proton9-20",
        "[targets.hades]\nexe = \"stubborn\"\n",
    );

    assert_success(&f.run(&["start", "hades"]));
    wait_for("the program to start", || f.fake_program_running("hades"));

    let began = Instant::now();
    assert_success(&f.run(&["stop", "hades"]));
    assert!(began.elapsed() >= Duration::from_secs(1));
    assert!(!f.run_group_alive("hades"));
    let log = f.log("hades");
    assert!(log.contains("wineo: stopping"), "{log}");
    assert!(log.contains("wineo: killing"), "{log}");
    assert!(f.state("hades").contains("status = \"stopped\""));
}

#[test]
fn force_stop_kills_a_proton_run_right_away() {
    let f = Fixture::new();
    f.global_config("stop_timeout = 30\n");
    f.install_proton("GE-Proton9-20");
    f.proton_environment(
        "hades",
        "GE-Proton9-20",
        "[targets.hades]\nexe = \"stubborn\"\n",
    );

    assert_success(&f.run(&["start", "hades"]));
    wait_for("the program to start", || f.fake_program_running("hades"));

    let began = Instant::now();
    assert_success(&f.run(&["stop", "--force", "hades"]));
    assert!(began.elapsed() < Duration::from_secs(10));
    assert!(!f.run_group_alive("hades"));
    let log = f.log("hades");
    assert!(!log.contains("wineo: stopping"), "{log}");
    assert!(log.contains("wineo: killing"), "{log}");
}

#[test]
fn stop_cleans_up_a_lost_proton_run_through_its_group() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);
    assert_success(&f.run(&["start", "hades"]));
    wait_for("the program to start", || f.fake_program_running("hades"));

    let supervisor = f.runtime("hades").holder_pid().unwrap().unwrap();
    assert!(
        Command::new("kill")
            .args(["-KILL", &supervisor.to_string()])
            .status()
            .unwrap()
            .success()
    );
    wait_for("the lock to be released", || {
        !f.runtime("hades").is_running().unwrap()
    });
    assert!(f.runtime("hades").status().unwrap().lost);
    assert!(f.run_group_alive("hades"));

    let stopped = f.run(&["stop", "hades"]);
    assert_success(&stopped);
    assert!(
        stderr(&stopped).contains("killed anything left behind"),
        "{}",
        stderr(&stopped)
    );
    wait_for("the process group to go", || !f.run_group_alive("hades"));
    assert!(f.state("hades").contains("status = \"stopped\""));

    // Cleaning up happens once, so a later stop has nothing to say.
    let again = f.run(&["stop", "hades"]);
    assert_success(&again);
    assert_eq!(stderr(&again), "hades is not running\n");
}

#[test]
fn a_lost_run_that_recorded_no_group_says_it_cant_be_cleaned_up() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);
    // A state file from before wineo recorded the process group.
    let runtime = f.runtime("hades");
    fs::create_dir_all(runtime.dir()).unwrap();
    fs::write(
        runtime.dir().join("state.toml"),
        "status = \"running\"\nmode = \"detached\"\ntarget = \"hades\"\npid = 1\nstarted_at = \"2026-09-16T21:02:11Z\"\n",
    )
    .unwrap();
    assert!(runtime.status().unwrap().lost);

    let stopped = f.run(&["stop", "hades"]);
    assert_success(&stopped);
    assert!(
        stderr(&stopped).contains("too old to record what to kill"),
        "{}",
        stderr(&stopped)
    );
    assert!(f.state("hades").contains("status = \"stopped\""));
}

#[test]
fn dxvk_is_installed_backed_up_and_restored() {
    let f = Fixture::new();
    f.environment("app", APP);
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    // A wine build that ships its own d3d11, and one that ships no dxgi at all.
    let original = f.system32("app").join("d3d11.dll");
    fs::write(&original, "wine").unwrap();

    let installed = f.run(&["set", "app", "dxvk=dxvk-2.7"]);
    assert_success(&installed);
    assert!(
        stderr(&installed).contains("Installed DXVK dxvk-2.7 into app"),
        "{}",
        stderr(&installed)
    );

    // Both architectures, and the original moved aside rather than lost.
    for dir in ["system32", "syswow64"] {
        let dir = f.system32("app").with_file_name(dir);
        assert!(dir.join("dxgi.dll").is_file(), "{dir:?}");
        assert!(dir.join("d3d11.dll").is_file(), "{dir:?}");
    }
    assert_ne!(fs::read_to_string(&original).unwrap(), "wine");
    let backup = f.system32("app").join("d3d11.dll.wineo-orig");
    assert_eq!(fs::read_to_string(&backup).unwrap(), "wine");
    // dxgi had no original, so nothing was backed up for it.
    assert!(!f.system32("app").join("dxgi.dll.wineo-orig").exists());

    let manifest = f.manifest("app");
    assert!(manifest.contains("build = \"dxvk-2.7\""), "{manifest}");
    assert!(manifest.contains("system32/dxgi.dll"), "{manifest}");
    assert!(manifest.contains("syswow64/dxgi.dll"), "{manifest}");

    // The overrides point wine at what was just copied in.
    let reg = f.reg_log("app");
    for dll in ["d3d8", "d3d9", "d3d10core", "d3d11", "dxgi"] {
        assert!(
            reg.contains(&format!(
                "add HKCU\\Software\\Wine\\DllOverrides /v {dll} /t REG_SZ /d native /f"
            )),
            "{dll} override missing from:\n{reg}"
        );
    }

    let removed = f.run(&["set", "app", "--unset", "dxvk"]);
    assert_success(&removed);
    assert!(
        stderr(&removed).contains("Removed DXVK dxvk-2.7 from app"),
        "{}",
        stderr(&removed)
    );
    // The original is back, what wineo added is gone, and so is the backup.
    assert_eq!(fs::read_to_string(&original).unwrap(), "wine");
    assert!(!backup.exists());
    assert!(!f.system32("app").join("dxgi.dll").exists());
    assert!(f.manifest("app").is_empty());
    assert!(
        f.reg_log("app")
            .contains("delete HKCU\\Software\\Wine\\DllOverrides /v dxgi /f")
    );
}

#[test]
fn swapping_a_build_removes_the_old_files_first() {
    let f = Fixture::new();
    f.environment("app", APP);
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    f.install_build("dxvk", "dxvk-2.10", &["x64", "x32"]);
    // A DLL only the old build ships, so a leftover would be visible.
    fs::write(
        f.root
            .path()
            .join("data/wineo/dxvk/dxvk-2.7/x64/d3d10core.dll"),
        "old",
    )
    .unwrap();

    assert_success(&f.run(&["set", "app", "dxvk=dxvk-2.7"]));
    assert!(f.system32("app").join("d3d10core.dll").is_file());

    let swapped = f.run(&["set", "app", "dxvk=dxvk-2.10"]);
    assert_success(&swapped);
    assert!(
        stderr(&swapped).contains("Replaced DXVK dxvk-2.7 with dxvk-2.10 in app"),
        "{}",
        stderr(&swapped)
    );
    assert!(!f.system32("app").join("d3d10core.dll").exists());
    let dxgi = fs::read_to_string(f.system32("app").join("dxgi.dll")).unwrap();
    assert!(dxgi.contains("dxvk-2.10"), "{dxgi}");
    assert!(f.manifest("app").contains("dxvk-2.10"));
}

#[test]
fn latest_picks_up_a_build_dropped_in_later() {
    let f = Fixture::new();
    f.environment("app", APP);
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    assert_success(&f.run(&["set", "app", "dxvk=latest"]));
    assert!(f.manifest("app").contains("dxvk-2.7"));

    // The config still says `latest`, not the version it resolved to.
    let config = fs::read_to_string(f.config_dir().join("environments/app.toml")).unwrap();
    assert!(config.contains("dxvk = \"latest\""), "{config}");
    let shown = stdout(&f.run(&["status", "app"]));
    assert!(shown.contains("dxvk      latest (dxvk-2.7)"), "{shown}");

    // Extracting a newer one moves the prefix to it on the next launch, with
    // no `wineo set` in between.
    f.install_build("dxvk", "dxvk-2.10", &["x64", "x32"]);
    assert_success(&f.run(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));
    assert_success(&f.run(&["stop", "app"]));
    assert!(
        f.manifest("app").contains("dxvk-2.10"),
        "{}",
        f.manifest("app")
    );
}

#[test]
fn a_config_edited_by_hand_is_applied_at_the_next_launch() {
    let f = Fixture::new();
    f.environment("app", APP);
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);

    let config = f.config_dir().join("environments/app.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("dxvk = \"dxvk-2.7\"\n{text}")).unwrap();
    assert!(f.manifest("app").is_empty());
    let shown = stdout(&f.run(&["status", "app"]));
    assert!(
        shown.contains("will be installed on the next launch"),
        "{shown}"
    );

    let started = f.run(&["start", "app"]);
    assert_success(&started);
    assert!(
        stderr(&started).contains("Installed DXVK dxvk-2.7 into app"),
        "{}",
        stderr(&started)
    );
    wait_for("the program to start", || f.fake_program_running("app"));
    assert_success(&f.run(&["stop", "app"]));
    assert!(f.manifest("app").contains("dxvk-2.7"));
    assert!(!stdout(&f.run(&["status", "app"])).contains("will be installed"));
}

#[test]
fn a_prefix_that_already_matches_is_left_alone() {
    let f = Fixture::new();
    f.environment("app", APP);
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    assert_success(&f.run(&["set", "app", "dxvk=dxvk-2.7"]));

    // Removing the build it came from changes nothing: the DLLs were copied,
    // not linked, so the prefix still has what the config asks for.
    fs::remove_dir_all(f.root.path().join("data/wineo/dxvk/dxvk-2.7")).unwrap();
    let started = f.run(&["start", "app"]);
    assert_success(&started);
    assert!(
        !stderr(&started).contains("Installed"),
        "{}",
        stderr(&started)
    );
    wait_for("the program to start", || f.fake_program_running("app"));
    assert_success(&f.run(&["stop", "app"]));
    assert!(f.system32("app").join("dxgi.dll").is_file());
}

#[test]
fn a_launch_is_refused_when_the_build_is_not_installed() {
    let f = Fixture::new();
    f.environment("app", APP);
    let config = f.config_dir().join("environments/app.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("dxvk = \"nope\"\n{text}")).unwrap();

    let refused = f.run(&["start", "app"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("is not installed"),
        "{}",
        stderr(&refused)
    );
    assert!(!f.system32("app").join("dxgi.dll").exists());
}

#[test]
fn dxvk_and_vkd3d_are_refused_on_a_proton_environment() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", HADES);
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    f.install_build("vkd3d", "2.14", &["x64", "x86"]);

    for key in ["dxvk=dxvk-2.7", "vkd3d=2.14"] {
        let refused = f.run(&["set", "hades", key]);
        assert!(!refused.status.success(), "{key} was allowed");
        assert!(
            stderr(&refused).contains("bundles its own"),
            "{key}: {}",
            stderr(&refused)
        );
    }
    assert!(
        !fs::read_to_string(f.config_dir().join("environments/hades.toml"))
            .unwrap()
            .contains("dxvk")
    );
}

#[test]
fn vkd3d_needs_dxvk_beside_it() {
    let f = Fixture::new();
    f.environment("app", APP);
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    f.install_build("vkd3d", "2.14", &["x64", "x86"]);

    let refused = f.run(&["set", "app", "vkd3d=2.14"]);
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("DXGI"), "{}", stderr(&refused));
    assert!(f.manifest("app").is_empty());

    // Together they are fine, and both show in status.
    assert_success(&f.run(&["set", "app", "dxvk=dxvk-2.7", "vkd3d=2.14"]));
    let shown = stdout(&f.run(&["status", "app"]));
    assert!(shown.contains("dxvk      dxvk-2.7"), "{shown}");
    assert!(shown.contains("vkd3d     2.14"), "{shown}");
    assert!(f.system32("app").join("d3d12.dll").is_file());

    // Dropping DXVK alone would leave vkd3d without a DXGI, so it is refused.
    let orphaned = f.run(&["set", "app", "--unset", "dxvk"]);
    assert!(!orphaned.status.success());
    assert!(f.system32("app").join("dxgi.dll").is_file());
}

#[test]
fn installing_is_refused_while_the_environment_runs() {
    let f = Fixture::new();
    f.environment("app", APP);
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    assert_success(&f.run(&["start", "app"]));
    wait_for("the program to start", || f.fake_program_running("app"));

    let refused = f.run(&["set", "app", "dxvk=dxvk-2.7"]);
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("is busy"), "{}", stderr(&refused));
    assert!(!f.system32("app").join("dxgi.dll").exists());

    assert_success(&f.run(&["stop", "app"]));
    assert_success(&f.run(&["set", "app", "dxvk=dxvk-2.7"]));
}

#[test]
fn a_win32_prefix_takes_only_its_32_bit_dlls() {
    let f = Fixture::new();
    f.environment("app", APP);
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    // A win32 prefix has no syswow64 at all.
    let prefix = f.root.path().join("prefixes/app");
    fs::remove_dir_all(prefix.join("drive_c/windows/syswow64")).unwrap();

    assert_success(&f.run(&["set", "app", "dxvk=dxvk-2.7"]));
    let dxgi = fs::read_to_string(f.system32("app").join("dxgi.dll")).unwrap();
    assert!(dxgi.contains("x32"), "{dxgi}");
    assert!(!prefix.join("drive_c/windows/syswow64").exists());
    let manifest = f.manifest("app");
    assert!(!manifest.contains("syswow64"), "{manifest}");
}

#[test]
#[ignore = "runs wine against a real DXVK build"]
fn a_real_dxvk_build_installs_and_comes_back_out() {
    let f = Fixture::real();
    let source = PathBuf::from(std::env::var("WINEO_TEST_DXVK").expect(
        "set WINEO_TEST_DXVK to an extracted DXVK release, the directory holding x64 and x32",
    ));
    assert!(
        source.join("x64/dxgi.dll").is_file(),
        "{} has no x64/dxgi.dll",
        source.display()
    );
    let root = f.root.path().join("data/wineo/dxvk");
    fs::create_dir_all(&root).unwrap();
    symlink(&source, root.join("under-test")).unwrap();

    assert_success(&f.run(&["new", "-n", "d3d"]));
    let prefix = f.root.path().join("data/wineo/prefixes/d3d");
    let system32 = prefix.join("drive_c/windows/system32");
    let before = fs::read(system32.join("d3d11.dll")).unwrap();

    assert_success(&f.run(&["set", "d3d", "dxvk=under-test"]));
    assert_eq!(
        fs::read(system32.join("d3d11.dll")).unwrap(),
        fs::read(source.join("x64/d3d11.dll")).unwrap()
    );
    // Wine itself agrees the override is in the prefix, not just wineo.
    let query = f.run(&[
        "exec",
        "d3d",
        "reg",
        "query",
        r"HKCU\Software\Wine\DllOverrides",
        "/v",
        "d3d11",
    ]);
    assert_success(&query);
    assert!(stdout(&query).contains("native"), "{}", stdout(&query));

    assert_success(&f.run(&["set", "d3d", "--unset", "dxvk"]));
    assert_eq!(fs::read(system32.join("d3d11.dll")).unwrap(), before);
    assert!(!system32.join("d3d11.dll.wineo-orig").exists());
    assert!(!prefix.join(".wineo-components.toml").exists());
}

#[test]
fn the_built_in_gaming_template_sets_up_a_whole_environment() {
    let f = Fixture::new();
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    f.install_build("vkd3d", "2.14", &["x64", "x86"]);

    let listed = stdout(&f.run(&["templates"]));
    assert!(listed.contains("gaming"), "{listed}");
    assert!(listed.contains("built-in"), "{listed}");

    let created = f.run(&["new", "-n", "hades", "--template", "gaming"]);
    assert_success(&created);
    assert!(
        stderr(&created).contains("Installed DXVK dxvk-2.7 into hades"),
        "{}",
        stderr(&created)
    );

    let config = f.created_config("hades");
    assert!(config.contains("gamemode = true"), "{config}");
    // The config keeps `latest` rather than the version it resolved to.
    assert!(config.contains("dxvk = \"latest\""), "{config}");
    assert!(config.contains("vkd3d = \"latest\""), "{config}");

    let prefix = f.root.path().join("data/wineo/prefixes/hades");
    assert!(prefix.join("drive_c/windows/system32/dxgi.dll").is_file());
    assert!(prefix.join("drive_c/windows/system32/d3d12.dll").is_file());
    let shown = stdout(&f.run(&["status", "hades"]));
    assert!(shown.contains("dxvk      latest (dxvk-2.7)"), "{shown}");
}

#[test]
fn the_built_in_fails_clearly_with_no_builds_installed() {
    let f = Fixture::new();
    let refused = f.run(&["new", "-n", "hades", "--template", "gaming"]);
    assert!(!refused.status.success());
    let message = stderr(&refused);
    assert!(
        message.contains("`latest` has nothing to pick"),
        "{message}"
    );
    assert!(message.contains("dxvk"), "{message}");
    // Nothing was created, so a retry after extracting a build is clean.
    assert!(!f.config_dir().join("environments/hades.toml").exists());
    assert!(!f.root.path().join("data/wineo/prefixes/hades").exists());
}

#[test]
fn a_template_sets_every_value_it_names() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    f.write_template(
        "mine",
        "runner = \"wine-tkg\"\narch = \"win32\"\ndxvk = \"dxvk-2.7\"\ngamemode = true\n\n[env]\nDXVK_HUD = \"fps\"\n",
    );

    assert_success(&f.run(&["new", "-n", "app", "--template", "mine"]));
    let config = f.created_config("app");
    assert!(config.contains("runner = \"wine-tkg\""), "{config}");
    assert!(config.contains("arch = \"win32\""), "{config}");
    assert!(config.contains("dxvk = \"dxvk-2.7\""), "{config}");
    assert!(config.contains("gamemode = true"), "{config}");
    assert!(config.contains("DXVK_HUD = \"fps\""), "{config}");
}

#[test]
fn explicit_flags_win_over_the_template() {
    let f = Fixture::new();
    f.install_runner("wine-tkg");
    f.write_template("mine", "runner = \"wine-tkg\"\narch = \"win32\"\n");

    assert_success(&f.run(&[
        "new",
        "-n",
        "app",
        "--template",
        "mine",
        "--runner",
        "wine",
        "--arch",
        "win64",
    ]));
    let config = f.created_config("app");
    assert!(config.contains("runner = \"wine\""), "{config}");
    assert!(config.contains("arch = \"win64\""), "{config}");
}

#[test]
fn a_user_template_shadows_the_built_in() {
    let f = Fixture::new();
    f.write_template("gaming", "gamemode = false\n");

    let listed = stdout(&f.run(&["templates"]));
    assert!(listed.contains("gaming"), "{listed}");
    assert!(!listed.contains("built-in"), "{listed}");

    // No builds are installed, and it still works: the shadow asks for none.
    assert_success(&f.run(&["new", "-n", "app", "--template", "gaming"]));
    let config = f.created_config("app");
    assert!(config.contains("gamemode = false"), "{config}");
    assert!(!config.contains("dxvk"), "{config}");
}

#[test]
fn a_template_that_cannot_be_used_fails_before_anything_is_created() {
    let f = Fixture::new();
    f.write_template("no-runner", "runner = \"nope\"\n");
    f.write_template("no-build", "dxvk = \"nope\"\n");
    f.write_template("has-prefix", "prefix = \"/p\"\n");
    f.write_template("has-targets", "[targets.app]\nexe = \"a\"\n");

    for (template, expected) in [
        ("nope", "there is no template"),
        ("no-runner", "is not installed"),
        ("no-build", "is not installed"),
        ("has-prefix", "belongs to an environment"),
        ("has-targets", "belongs to an environment"),
    ] {
        let refused = f.run(&["new", "-n", "app", "--template", template]);
        assert!(!refused.status.success(), "{template} was allowed");
        assert!(
            stderr(&refused).contains(expected),
            "{template}: {}",
            stderr(&refused)
        );
        assert!(
            !f.config_dir().join("environments/app.toml").exists(),
            "{template} left a config behind"
        );
        assert!(
            !f.root.path().join("data/wineo/prefixes/app").exists(),
            "{template} left a prefix behind"
        );
    }
}

#[test]
fn a_template_naming_proton_and_dxvk_is_refused() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    f.write_template("mixed", "runner = \"GE-Proton9-20\"\ndxvk = \"dxvk-2.7\"\n");

    let refused = f.run(&["new", "-n", "hades", "--template", "mixed"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("bundles its own"),
        "{}",
        stderr(&refused)
    );
    assert!(!f.config_dir().join("environments/hades.toml").exists());
}

impl Fixture {
    fn prefix(&self, name: &str) -> PathBuf {
        self.root.path().join("prefixes").join(name)
    }

    /// `new` running `installer`, with the prefix where the other helpers look.
    fn new_with_installer(&self, name: &str, extra: &[&str], installer: &str) -> Command {
        let mut command = self.wineo();
        command
            .args(["new", "-n", name, "--dir"])
            .arg(self.prefix(name))
            .args(extra)
            .args(["--install-exe", installer]);
        command
    }

    /// Whether an installer's recovery record is there.
    fn installer_recorded(&self, name: &str) -> bool {
        self.runtime(name).dir().join("operation.toml").exists()
    }
}

fn signal(child: &std::process::Child, signal: &str) {
    let sent = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
}

#[test]
fn new_waits_for_what_an_installer_hands_off_to() {
    let f = Fixture::new();
    // Nothing reads the output, so the handoff can't hold `new` up through a
    // pipe and only the wait on the prefix does.
    let mut child = f
        .new_with_installer("game", &[], "handoff")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the installer to hand off", || {
        f.fake_program_running("game")
    });

    // The installer itself has exited, but the environment is still held.
    let exec = f.run(&["exec", "game", "notepad"]);
    assert!(!exec.status.success());
    assert!(
        stderr(&exec).contains("an installer is running in it"),
        "{}",
        stderr(&exec)
    );
    let listed = f.run(&["list"]);
    assert!(stdout(&listed).contains("install"), "{}", stdout(&listed));

    assert!(child.wait().unwrap().success());
    assert!(f.prefix("game").join("handoff-done").exists());
    assert!(!f.installer_recorded("game"));
}

#[test]
fn an_installers_exit_code_is_passed_on_and_the_environment_kept() {
    let f = Fixture::new();
    let output = f
        .new_with_installer("game", &[], "exit-3")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("The installer exited with code 3"),
        "{}",
        stderr(&output)
    );
    assert!(f.created_config("game").contains("runner = \"wine\""));
    assert!(f.prefix("game").join("system.reg").is_file());
    assert!(!f.installer_recorded("game"));
}

#[test]
fn ctrl_c_cancels_an_installer_and_keeps_the_environment() {
    let f = Fixture::new();
    let child = f
        .new_with_installer("game", &[], "setup")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("the installer to start", || f.fake_program_running("game"));

    // An installer is not a run, so it is not stop's to end.
    let stop = f.run(&["stop", "game"]);
    assert!(!stop.status.success());
    assert!(
        stderr(&stop).contains("an installer is running in it"),
        "{}",
        stderr(&stop)
    );

    signal(&child, "-INT");
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("Canceling the installer, press Ctrl+C again to force it"),
        "{text}"
    );
    assert!(
        text.contains("The installer was canceled. game and its prefix were kept"),
        "{text}"
    );
    assert!(!f.fake_program_running("game"));
    assert!(!f.installer_recorded("game"));
    assert!(!f.created_config("game").is_empty());
    assert_success(&f.run(&["exec", "game", "exit-0"]));
}

#[test]
fn a_stubborn_installer_is_killed_once_the_stop_timeout_passes() {
    let f = Fixture::new();
    f.global_config("stop_timeout = 1\n");
    let child = f
        .new_with_installer("game", &[], "stubborn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("the installer to start", || f.fake_program_running("game"));

    signal(&child, "-INT");
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Killing everything the installer left running"),
        "{}",
        stderr(&output)
    );
    assert!(!f.fake_program_running("game"));
    assert!(!f.installer_recorded("game"));
}

#[test]
fn a_second_ctrl_c_kills_an_installer_right_away() {
    let f = Fixture::new();
    let child = f
        .new_with_installer("game", &[], "stubborn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("the installer to start", || f.fake_program_running("game"));

    let started = Instant::now();
    signal(&child, "-INT");
    thread::sleep(Duration::from_millis(300));
    signal(&child, "-INT");
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    // Well inside the default ten second stop timeout.
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(!f.fake_program_running("game"));
}

#[test]
fn a_crashed_installer_is_refused_until_stop_cleans_up() {
    let f = Fixture::new();
    f.environment("other", APP);
    // The fake program would hold a piped output open for a minute.
    let mut child = f
        .new_with_installer("game", &[], "setup")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the installer to start", || f.fake_program_running("game"));
    signal(&child, "-KILL");
    child.wait().unwrap();
    assert!(f.installer_recorded("game"));
    assert!(f.fake_program_running("game"));

    // A target is only a config edit, so it can still be changed.
    assert_success(&f.run(&["set", "game", "targets.app.exe=app"]));
    for args in [
        &["start", "game"][..],
        &["run", "game"],
        &["exec", "game", "notepad"],
        &["set", "game", "runner=wine"],
        &["rm", "game"],
    ] {
        let refused = f.run(args);
        assert!(!refused.status.success(), "{args:?} was allowed");
        let text = stderr(&refused);
        assert!(
            text.contains("last installer did not finish cleanly")
                && text.contains("`wineo stop game`"),
            "{args:?}: {text}"
        );
    }
    let status = f.run(&["status", "game"]);
    assert!(
        stdout(&status).contains("an installer did not finish cleanly"),
        "{}",
        stdout(&status)
    );
    let listed = f.run(&["list"]);
    assert!(
        stdout(&listed).contains("unresolved"),
        "{}",
        stdout(&listed)
    );
    // Nothing about it reaches another environment.
    assert!(
        stdout(&listed).contains("other  inactive"),
        "{}",
        stdout(&listed)
    );
    assert_success(&f.run(&["exec", "other", "exit-0"]));

    let stopped = f.run(&["stop", "game"]);
    assert_success(&stopped);
    assert!(
        stderr(&stopped).contains(
            "could not confirm that game's last installer had finished, so it stopped anything left behind"
        ),
        "{}",
        stderr(&stopped)
    );
    assert!(!f.fake_program_running("game"));
    assert!(!f.installer_recorded("game"));
    assert_success(&f.run(&["rm", "game"]));
}

#[test]
fn an_installer_whose_prefix_cannot_be_confirmed_idle_is_left_unresolved() {
    let f = Fixture::new();
    f.install_runner("fake");
    let output = f
        .new_with_installer("game", &["--runner", "fake"], "break-wineserver")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("could not confirm that everything the installer started has stopped"),
        "{text}"
    );
    assert!(text.contains("The installer itself finished"), "{text}");
    assert!(text.contains("`wineo stop game`"), "{text}");
    assert!(f.installer_recorded("game"));

    // The cleanup needs the same wineserver, so it can't succeed until that
    // is back, and the environment stays refused until then.
    let failed = f.run(&["stop", "game"]);
    assert!(!failed.status.success());
    assert!(f.installer_recorded("game"));
    f.install_runner("fake");
    assert_success(&f.run(&["stop", "game"]));
    assert!(!f.installer_recorded("game"));
}

#[test]
fn a_proton_installer_lasts_until_its_process_group_is_gone() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    let status = f
        .new_with_installer("hades", &["--runner", "GE-Proton9-20"], "handoff")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    // umu exited right away. Only the wait on its group covers the handoff.
    assert!(f.prefix("hades").join("handoff-done").exists());
    assert!(!f.installer_recorded("hades"));
}

#[test]
fn ctrl_c_cancels_a_proton_installer_through_its_group() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    let child = f
        .new_with_installer("hades", &["--runner", "GE-Proton9-20"], "setup")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("the installer to start", || f.fake_program_running("hades"));

    signal(&child, "-INT");
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert!(!f.fake_program_running("hades"));
    assert!(!f.installer_recorded("hades"));
}

/// An unresolved installer record for `name`, as if wineo had died mid-install.
fn record_unresolved(f: &Fixture, name: &str, text: &str) {
    let dir = f.runtime(name).dir().to_owned();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("operation.toml"), text).unwrap();
}

/// The group a Proton installer record names, as its leader's pid.
fn recorded_group(f: &Fixture, name: &str) -> u32 {
    fs::read_to_string(f.runtime(name).dir().join("operation.toml"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("group = "))
        .and_then(|group| group.trim().parse().ok())
        .expect("the record names a group")
}

#[test]
fn a_crashed_proton_installer_is_stopped_through_its_group() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    let mut child = f
        .new_with_installer("hades", &["--runner", "GE-Proton9-20"], "setup")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the installer to start", || f.fake_program_running("hades"));
    wait_for("the record to name its group", || {
        fs::read_to_string(f.runtime("hades").dir().join("operation.toml"))
            .is_ok_and(|text| text.contains("group = "))
    });
    signal(&child, "-KILL");
    child.wait().unwrap();
    let group = recorded_group(&f, "hades");
    assert!(PathBuf::from(format!("/proc/{group}")).exists());

    let stopped = f.run(&["stop", "hades"]);
    assert_success(&stopped);
    assert!(
        stderr(&stopped).contains("so it stopped anything left behind"),
        "{}",
        stderr(&stopped)
    );
    wait_for("umu's group to be gone", || {
        !PathBuf::from(format!("/proc/{group}")).exists()
    });
    assert!(!f.installer_recorded("hades"));
}

#[test]
fn a_process_group_that_reused_the_number_is_never_signalled() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.proton_environment("hades", "GE-Proton9-20", "");
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    for (boot_id, reused) in [(boot.trim(), true), ("some-other-boot", false)] {
        // Something unrelated now leads a group with the recorded number.
        let mut bystander = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let group = bystander.id();
        let started = fs::read_to_string(format!("/proc/{group}/stat")).unwrap();
        let started: u64 = started[started.rfind(')').unwrap() + 1..]
            .split_whitespace()
            .nth(19)
            .unwrap()
            .parse()
            .unwrap();
        // Either the leader started at another time, or the record is from
        // another boot.
        let leader_start = if reused { started + 1 } else { started };
        record_unresolved(
            &f,
            "hades",
            &format!(
                "operation = \"install\"\nprogram = \"setup\"\nprefix = \"{}\"\ngroup = {group}\nboot_id = \"{boot_id}\"\nleader_start = {leader_start}\nstarted_at = \"2026-09-20T00:00:00Z\"\n\n[runner]\nname = \"GE-Proton9-20\"\n\n[runner.kind.proton]\nroot = \"/nonexistent\"\n",
                f.prefix("hades").display()
            ),
        );
        let stopped = f.run(&["stop", "hades"]);
        assert_success(&stopped);
        assert!(
            stderr(&stopped).contains("what it started had already exited"),
            "{}",
            stderr(&stopped)
        );
        assert!(bystander.try_wait().unwrap().is_none(), "it was signalled");
        assert!(!f.installer_recorded("hades"));
        bystander.kill().unwrap();
        bystander.wait().unwrap();
    }
}

#[test]
fn winetricks_refuses_before_touching_the_prefix() {
    let f = Fixture::new();
    f.install_build("dxvk", "dxvk-2.7", &["x64", "x32"]);
    f.environment("app", &format!("dxvk = \"dxvk-2.7\"\n\n{APP}"));
    record_unresolved(&f, "app", "not a record [");

    let refused = f.run_sandboxed(&["winetricks", "app", "-q", "corefonts"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("last installer did not finish cleanly"),
        "{}",
        stderr(&refused)
    );
    assert!(f.manifest("app").is_empty(), "{}", f.manifest("app"));
    assert!(f.reg_log("app").is_empty(), "{}", f.reg_log("app"));
}

#[test]
fn an_unreadable_installer_record_is_still_cleaned_up() {
    let f = Fixture::new();
    f.environment("app", APP);
    record_unresolved(&f, "app", "");

    let refused = f.run(&["start", "app"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("`wineo stop app`"),
        "{}",
        stderr(&refused)
    );

    // With nothing to read, the environment's own runner and prefix are used.
    let stopped = f.run(&["stop", "app"]);
    assert_success(&stopped);
    assert!(!f.installer_recorded("app"));
    assert_success(&f.run(&["exec", "app", "exit-0"]));
}

#[test]
fn restart_is_refused_rather_than_cleaning_up() {
    let f = Fixture::new();
    f.environment("app", APP);
    record_unresolved(&f, "app", "");
    let refused = f.run(&["restart", "app"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("last installer did not finish cleanly"),
        "{}",
        stderr(&refused)
    );
    assert!(f.installer_recorded("app"));
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

#[test]
fn set_leaves_icons_for_later_while_an_installer_is_unresolved() {
    let f = Fixture::new();
    f.environment("app", APP);
    let prefix = f.prefix("app");
    fs::create_dir_all(prefix.join("dosdevices")).unwrap();
    symlink("../drive_c", prefix.join("dosdevices/c:")).unwrap();
    fs::create_dir_all(prefix.join("drive_c/Game")).unwrap();
    fs::write(prefix.join("drive_c/Game/game.exe"), b"").unwrap();
    let menu = prefix.join("drive_c/ProgramData/Microsoft/Windows/Start Menu/Programs");
    fs::create_dir_all(&menu).unwrap();
    fs::write(menu.join("Game.lnk"), lnk(r"C:\Game\game.exe")).unwrap();
    record_unresolved(&f, "app", "");

    // Pulling out the icon would run wine in the prefix, so it waits.
    assert_success(&f.run(&[
        "set",
        "app",
        r"targets.game.exe=C:\Game\game.exe",
        "targets.game.desktop=true",
    ]));
    assert!(
        f.root
            .path()
            .join("data/applications/wineo-app-game.desktop")
            .exists()
    );
    assert!(!prefix.join("menubuilder.log").exists());

    // Once nothing is unresolved, the next change tries for the icon again.
    assert_success(&f.run(&["stop", "app"]));
    assert_success(&f.run(&["set", "app", "targets.game.name=Game"]));
    assert!(prefix.join("menubuilder.log").exists());
}
