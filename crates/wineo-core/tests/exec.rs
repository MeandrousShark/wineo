use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use wineo_core::build_lock::{self, Kind};
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::launch::{self, ExecEvent, ExecResolution, Launch, LaunchEvent, Overrides};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::runtime::{RunMode, RunState, Runtime, exit_code};

#[path = "common/test_script.rs"]
mod test_script;
use test_script::write_script;
#[allow(dead_code)]
#[path = "common/runners.rs"]
mod runners;

const INSPECT: &str = r#"case "$1" in
inspect | winecfg | msiexec | *setup.exe)
    printf 'arg=<%s>\n' "$@"
    printf 'cwd=<%s>\n' "$PWD"
    printf 'env=<%s>\n' "${FAKE_VAR-}"
    printf 'stderr line\npartial' >&2
    exit 0
    ;;
linger)
    printf '%s\n' "$$" >"$WINEPREFIX/linger-pid"
    sleep 1
    printf 'still writing\n'
    touch "$WINEPREFIX/linger-done"
    exit 0
    ;;
corelimit)
    # The limit is set right after spawn, so wait for it to land.
    i=0
    while [ "$i" -lt 100 ] && ! grep -Eq '^Max core file size +1 ' /proc/self/limits; do
        sleep 0.1
        i=$((i + 1))
    done
    grep 'Max core file size' /proc/self/limits
    exit 0
    ;;
forest)
    sleep 60 &
    printf '%s %s\n' "$$" "$!" >"$WINEPREFIX/forest-pids"
    wait
    ;;
esac
"#;

struct Fixture {
    root: tempfile::TempDir,
    paths: Paths,
    prefix: PathBuf,
}

fn paths(root: &Path) -> Paths {
    Paths::from_lookup(|key| (key == "HOME").then(|| root.as_os_str().to_owned())).unwrap()
}

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_script(path, body);
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let prefix = paths.default_prefix_root().join("app");
        let runner = paths.default_runner_root().join("fake/bin");
        script(
            &runner.join("wine"),
            &runners::FAKE_WINE.replacen("#!/bin/sh\n", &format!("#!/bin/sh\n{INSPECT}"), 1),
        );
        script(&runner.join("wineserver"), runners::FAKE_WINESERVER);
        let config = paths.environment_config("app");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            &config,
            format!(
                "prefix = '{}'\nrunner = 'fake'\narch = 'win64'\n",
                prefix.display()
            ),
        )
        .unwrap();
        Self {
            root,
            paths,
            prefix,
        }
    }

    fn built() -> Self {
        let fixture = Self::new();
        fs::create_dir_all(fixture.prefix.join("drive_c")).unwrap();
        fs::write(
            fixture.prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        fs::create_dir_all(fixture.prefix.join("dosdevices")).unwrap();
        symlink("../drive_c", fixture.prefix.join("dosdevices/c:")).unwrap();
        fixture
    }

    fn collect(
        &self,
        program: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Vec<ExecEvent> {
        let mut events = Vec::new();
        let status = launch::exec(
            &self.paths,
            "app",
            program,
            args,
            env,
            &Token::new(),
            |event| events.push(event),
        )
        .unwrap();
        assert!(status.success(), "{status}");
        events
    }

    fn subprocess(&self, scenario: &str, cwd: &Path) {
        let bin = self.root.path().join("bin");
        let proton = self.paths.default_runner_root().join("proton");
        script(
            &bin.join("umu-run"),
            &runners::FAKE_UMU.replacen("pids=", &format!("{INSPECT}pids="), 1),
        );
        script(&proton.join("proton"), "#!/bin/sh\nexit 0\n");
        fs::write(proton.join("version"), "1717451631 proton\n").unwrap();
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "exec_subprocess", "--nocapture"])
            .env("WINEO_EXEC_TEST_ROOT", self.root.path())
            .env("WINEO_EXEC_TEST_SCENARIO", scenario)
            .env("PATH", path)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

fn output_lines(events: &[ExecEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            ExecEvent::OutputLine(line) => Some(line.as_str()),
            _ => None,
        })
        .collect()
}

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.0.wait();
    }
}

#[test]
fn builtins_msi_exact_arguments_and_temporary_overrides() {
    let f = Fixture::built();
    let config = f.paths.environment_config("app");
    let original = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("{original}gamemode = true\nmangohud = true\nwayland = true\ngamescope = {{}}\nwinetricks = ['corefonts']\ngameid = '123'\nstore = 'steam'\n[env]\nFAKE_VAR = 'configured'\n")).unwrap();
    let before = fs::read(&config).unwrap();
    let args = [
        "two words",
        "\"literal quotes\"",
        "",
        "--flag",
        "$(no shell)",
    ]
    .map(str::to_owned);
    let env = BTreeMap::from([("FAKE_VAR".to_owned(), "temporary value".to_owned())]);
    for (program, expected) in [
        ("winecfg".to_owned(), vec!["winecfg".to_owned()]),
        (
            "setup.MSI".to_owned(),
            vec![
                "msiexec".to_owned(),
                "/i".to_owned(),
                "setup.MSI".to_owned(),
            ],
        ),
        (
            r"C:\Program Files\setup.exe".to_owned(),
            vec![r"C:\Program Files\setup.exe".to_owned()],
        ),
        (
            "~/setup.exe".to_owned(),
            vec![f.root.path().join("setup.exe").display().to_string()],
        ),
    ] {
        fs::create_dir_all(f.prefix.join("drive_c/Program Files")).unwrap();
        fs::write(f.prefix.join("drive_c/Program Files/setup.exe"), "").unwrap();
        fs::write(f.root.path().join("setup.exe"), "").unwrap();
        let events = f.collect(&program, &args, &env);
        let lines = output_lines(&events);
        let argv: Vec<_> = expected
            .iter()
            .chain(args.iter())
            .map(|arg| format!("arg=<{arg}>"))
            .collect();
        assert_eq!(&lines[..argv.len()], argv);
        assert!(lines.contains(&"env=<temporary value>"));
        assert!(lines.contains(&"stderr line"));
        assert_eq!(lines.last(), Some(&"partial"));
        assert!(matches!(events.first(), Some(ExecEvent::Started)));
        assert!(matches!(events.last(), Some(ExecEvent::Exited { code: 0 })));
    }
    let events = f.collect("inspect", &[], &BTreeMap::new());
    assert!(output_lines(&events).contains(&"env=<configured>"));
    assert_eq!(fs::read(&config).unwrap(), before);
    assert!(!f.prefix.join("winetricks.log").exists());
    assert!(!f.paths.stats_log("app").exists());
    assert!(
        Runtime::new(&f.paths, "app")
            .read_state()
            .unwrap()
            .is_none()
    );

    let environment = Environment::load(&f.paths, "app").unwrap();
    let launch = Launch::exec_with_resolution(
        &f.paths,
        &GlobalConfig::default(),
        &environment,
        "winecfg",
        &[],
        &env,
        ExecResolution::ExplicitPaths,
    )
    .unwrap();
    assert!(launch.wrappers.command_line().is_empty());
    assert_eq!(launch.wayland, launch::Wayland::Off);
    assert!(launch.target.is_none());
    assert!(launch.gameid.is_none());
    assert!(launch.store.is_none());
}

#[test]
fn absolute_msi_uses_the_prefix_z_drive() {
    let f = Fixture::built();
    fs::create_dir_all(f.prefix.join("dosdevices")).unwrap();
    symlink("/", f.prefix.join("dosdevices/z:")).unwrap();
    let installer = f.root.path().join("two words.msi");
    fs::write(&installer, "").unwrap();
    let events = f.collect(installer.to_str().unwrap(), &[], &BTreeMap::new());
    let dos = format!(
        "arg=<Z:{}>",
        installer.display().to_string().replace('/', "\\")
    );
    assert_eq!(
        &output_lines(&events)[..3],
        ["arg=<msiexec>", "arg=</i>", &dos]
    );
}

#[test]
fn frontend_resolution_ignores_cwd_and_refuses_relative_unix_paths() {
    let f = Fixture::built();
    fs::write(f.root.path().join("setup.exe"), "").unwrap();
    f.subprocess("resolution", f.root.path());
}

#[test]
fn prefix_is_built_before_the_program_starts() {
    let f = Fixture::new();
    let events = f.collect("inspect", &[], &BTreeMap::new());
    let building = events
        .iter()
        .position(|event| matches!(event, ExecEvent::Launch(LaunchEvent::BuildingPrefix { .. })))
        .unwrap();
    let started = events
        .iter()
        .position(|event| matches!(event, ExecEvent::Started))
        .unwrap();
    assert!(building < started);
    assert!(f.prefix.join("system.reg").is_file());
    assert!(
        output_lines(&events)
            .contains(&format!("cwd=<{}>", f.prefix.join("drive_c").display()).as_str())
    );
}

#[test]
fn invalid_input_and_cancellation_do_not_build_a_prefix() {
    let f = Fixture::new();
    for program in [
        "./setup.exe",
        "dir/setup.exe",
        "../setup.exe",
        "~other/setup.exe",
    ] {
        let error = launch::exec(
            &f.paths,
            "app",
            program,
            &[],
            &BTreeMap::new(),
            &Token::new(),
            |_| panic!("unexpected event"),
        )
        .unwrap_err();
        assert!(
            matches!(error.downcast_ref::<Error>(), Some(Error::RelativeExe(_))),
            "{error}"
        );
    }
    let error = launch::exec(
        &f.paths,
        "app",
        "inspect",
        &[],
        &BTreeMap::from([("WINEPREFIX".to_owned(), "/other".to_owned())]),
        &Token::new(),
        |_| panic!("unexpected event"),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("WINEPREFIX is managed"),
        "{error}"
    );
    let token = Token::new();
    token.cancel();
    let error = launch::exec(
        &f.paths,
        "app",
        "inspect",
        &[],
        &BTreeMap::new(),
        &token,
        |_| panic!("unexpected event"),
    )
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<Error>(),
        Some(Error::Canceled("exec"))
    ));
    assert!(!f.prefix.exists());
}

#[test]
fn prefix_build_cancellation_never_starts_the_program() {
    let f = Fixture::new();
    let token = Token::new();
    let mut building = false;
    let error = launch::exec(
        &f.paths,
        "app",
        "inspect",
        &[],
        &BTreeMap::new(),
        &token,
        |event| {
            if matches!(event, ExecEvent::Launch(LaunchEvent::BuildingPrefix { .. })) {
                building = true;
                token.cancel();
            }
            assert!(!matches!(event, ExecEvent::Started));
        },
    )
    .unwrap_err();
    assert!(building);
    assert!(
        matches!(error.downcast_ref::<Error>(), Some(Error::Canceled(_))),
        "{error}"
    );
    assert!(!f.prefix.exists());
    assert!(f.paths.environment_config("app").exists());
}

#[test]
fn nonzero_exit_is_a_status_and_locks_are_released() {
    let f = Fixture::built();
    let mut code = None;
    let status = launch::exec(
        &f.paths,
        "app",
        "exit-37",
        &[],
        &BTreeMap::new(),
        &Token::new(),
        |event| {
            if let ExecEvent::Exited { code: value } = event {
                code = Some(value);
            }
        },
    )
    .unwrap();
    assert_eq!(status.code(), Some(37));
    assert_eq!(code, Some(37));
    drop(
        Locks::new(&f.paths)
            .acquire(Operation::Remove, "app", &[&f.prefix])
            .unwrap(),
    );
    assert!(
        build_lock::try_exclusive(&f.paths, Kind::Runner, "fake")
            .unwrap()
            .is_some()
    );
}

#[test]
fn exec_children_get_a_soft_core_limit_of_one() {
    let own = fs::read_to_string("/proc/self/limits").unwrap();
    let hard = own
        .lines()
        .find(|line| line.starts_with("Max core file size"))
        .and_then(|line| line.split_whitespace().nth(5))
        .unwrap()
        .to_owned();
    // A hard limit of 0 leaves no room for a soft limit of 1.
    if hard == "0" {
        return;
    }
    let f = Fixture::built();
    let events = f.collect("corelimit", &[], &BTreeMap::new());
    let line = output_lines(&events)
        .into_iter()
        .find(|line| line.starts_with("Max core file size"))
        .unwrap_or_else(|| panic!("no limit reported: {events:?}"));
    let fields: Vec<_> = line.split_whitespace().skip(4).collect();
    assert_eq!(fields[..2], ["1", hard.as_str()], "{line}");
}

#[test]
fn reconciliation_events_precede_start_and_component_locks_last_through_exit() {
    let f = Fixture::built();
    for (root, dirs, dll) in [
        (f.paths.default_dxvk_root(), ["x64", "x32"], "dxgi.dll"),
        (f.paths.default_vkd3d_root(), ["x64", "x86"], "d3d12.dll"),
    ] {
        for arch in dirs {
            let dir = root.join("test").join(arch);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(dll), "test DLL").unwrap();
        }
    }
    let config = f.paths.environment_config("app");
    let before = fs::read_to_string(&config).unwrap();
    fs::write(config, format!("{before}dxvk = 'test'\nvkd3d = 'test'\n")).unwrap();
    let mut reconciled = false;
    let mut exited = false;
    launch::exec(
        &f.paths,
        "app",
        "inspect",
        &[],
        &BTreeMap::new(),
        &Token::new(),
        |event| {
            if let ExecEvent::Launch(LaunchEvent::Components(changes)) = &event {
                assert_eq!(changes.len(), 2);
                reconciled = true;
            }
            if matches!(event, ExecEvent::Started | ExecEvent::Exited { .. }) {
                assert!(reconciled);
                for kind in [Kind::Runner, Kind::Dxvk, Kind::Vkd3d] {
                    let name = if kind == Kind::Runner { "fake" } else { "test" };
                    assert!(
                        build_lock::try_exclusive(&f.paths, kind, name)
                            .unwrap()
                            .is_none()
                    );
                }
                assert!(
                    Locks::new(&f.paths)
                        .acquire(Operation::Remove, "app", &[&f.prefix])
                        .is_err()
                );
                exited |= matches!(event, ExecEvent::Exited { .. });
            }
        },
    )
    .unwrap();
    assert!(exited);
    for kind in [Kind::Dxvk, Kind::Vkd3d] {
        assert!(
            build_lock::try_exclusive(&f.paths, kind, "test")
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn cancellation_holds_locks_and_only_kills_the_started_group() {
    let f = Fixture::built();
    let runtime = Runtime::new(&f.paths, "app");
    let _run_lock = runtime.try_lock().unwrap().unwrap();
    let environment = Environment::load(&f.paths, "app").unwrap();
    let tracked = Launch::exec(
        &f.paths,
        &GlobalConfig::default(),
        &environment,
        "stubborn",
        &[],
        &BTreeMap::new(),
    )
    .unwrap();
    let mut other = Running(
        tracked
            .command()
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    let state = RunState::running(
        RunMode::Detached,
        "app".to_owned(),
        other.0.id(),
        Some(other.0.id()),
        &Overrides::default(),
    );
    runtime.write_state(&state).unwrap();
    let state_before = runtime.read_state().unwrap();
    let _tracked_guard = Locks::new(&f.paths)
        .acquire(Operation::Launch, "app", &[&f.prefix])
        .unwrap();
    wait_for(|| f.prefix.join("fake-pids").exists());
    let token = Token::new();
    let mut seen = Vec::new();
    let status = launch::exec(
        &f.paths,
        "app",
        "forest",
        &[],
        &BTreeMap::new(),
        &token,
        |event| {
            if matches!(event, ExecEvent::Started) {
                wait_for(|| f.prefix.join("forest-pids").exists());
                assert!(
                    build_lock::try_exclusive(&f.paths, Kind::Runner, "fake")
                        .unwrap()
                        .is_none()
                );
                assert!(
                    Locks::new(&f.paths)
                        .acquire(Operation::Remove, "app", &[&f.prefix])
                        .is_err()
                );
                drop(
                    Locks::new(&f.paths)
                        .acquire(Operation::Launch, "app", &[&f.prefix])
                        .unwrap(),
                );
                token.force();
            }
            seen.push(event);
        },
    )
    .unwrap();
    assert!(!status.success());
    assert!(seen.iter().any(|event| matches!(event, ExecEvent::Killing)));
    assert!(matches!(seen.last(), Some(ExecEvent::Exited { code }) if *code == exit_code(status)));
    let pids: Vec<u32> = fs::read_to_string(f.prefix.join("forest-pids"))
        .unwrap()
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    wait_for(|| wineo_core::process::group_has_live_members(pids[0]) == Some(false));
    assert!(other.0.try_wait().unwrap().is_none());
    assert_eq!(runtime.read_state().unwrap(), state_before);
    assert!(!f.paths.stats_log("app").exists());
    drop(_tracked_guard);
    drop(tracked);
    drop(
        Locks::new(&f.paths)
            .acquire(Operation::Remove, "app", &[&f.prefix])
            .unwrap(),
    );
    assert!(
        build_lock::try_exclusive(&f.paths, Kind::Runner, "fake")
            .unwrap()
            .is_some()
    );
}

#[test]
fn cancel_escalates_a_stubborn_program_without_prefix_wide_killing() {
    let f = Fixture::built();
    let config = f.paths.global_config();
    fs::write(config, "stop_timeout = 0\n").unwrap();
    let token = Token::new();
    let mut killed = false;
    let status = launch::exec(
        &f.paths,
        "app",
        "stubborn",
        &[],
        &BTreeMap::new(),
        &token,
        |event| {
            if matches!(event, ExecEvent::OutputLine(_)) {
                token.cancel();
            }
            if matches!(event, ExecEvent::Killing) {
                killed = true;
            }
        },
    )
    .unwrap();
    assert!(killed);
    assert!(!status.success());
}

/// A GOG game folder with a program in it, owned by `owner`.
fn gog_game(f: &Fixture, owner: &str, mid_update: bool) -> PathBuf {
    use wineo_core::gog::folder::{
        Committed, FolderLock, FolderState, Journal, JournalKind, Platform, Target, Track,
    };
    let folder = fs::canonicalize(f.root.path())
        .unwrap()
        .join("Games/RimWorld");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("game.exe"), "").unwrap();
    let target = Target {
        gog_id: 1094900565,
        platform: Platform::Windows,
        language: "en-US".to_owned(),
        track: Track::Installer,
        products: Vec::new(),
    };
    let state = FolderState {
        committed: Some(Committed {
            source: None,
            gog_id: target.gog_id,
            platform: target.platform,
            language: target.language.clone(),
            track: target.track,
            products: Vec::new(),
            files: Vec::new(),
        }),
        journal: mid_update.then(|| Journal {
            operation: "op".to_owned(),
            kind: JournalKind::Update,
            target,
            packages: Vec::new(),
            attempted: Vec::new(),
        }),
        ..FolderState::new(folder.clone(), Some(owner.to_owned()))
    };
    FolderLock::exclusive(&f.paths, &folder, &|| false)
        .unwrap()
        .write(&state)
        .unwrap();
    folder
}

#[test]
fn exec_from_a_gog_folder_holds_it_until_the_program_exits() {
    use wineo_core::gog::folder::{FolderError, lock_for_update};

    let f = Fixture::built();
    let folder = gog_game(&f, "rimworld", false);
    let exe = folder.join("game.exe").display().to_string();
    let token = Token::new();
    let mut refused = None;
    // Canceled, the prefix is asked to close, and the fake program exits
    // cleanly when asked, so its status says nothing here.
    launch::exec(
        &f.paths,
        "app",
        &exe,
        &[],
        &BTreeMap::new(),
        &token,
        |event| {
            if matches!(event, ExecEvent::Started) {
                refused = Some(
                    lock_for_update(&f.paths, &folder, "rimworld")
                        .err()
                        .and_then(|error| error.downcast::<FolderError>().ok()),
                );
                token.cancel();
            }
        },
    )
    .unwrap();
    assert!(
        matches!(refused, Some(Some(FolderError::InUse { .. }))),
        "{refused:?}"
    );
    assert!(lock_for_update(&f.paths, &folder, "rimworld").is_ok());

    let updating = lock_for_update(&f.paths, &folder, "rimworld").unwrap();
    let error = launch::exec(
        &f.paths,
        "app",
        &exe,
        &[],
        &BTreeMap::new(),
        &Token::new(),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "rimworld's game folder is being updated."
    );
    drop(updating);

    gog_game(&f, "rimworld", true);
    let mut started = false;
    let error = launch::exec(
        &f.paths,
        "app",
        &exe,
        &[],
        &BTreeMap::new(),
        &Token::new(),
        |event| started |= matches!(event, ExecEvent::Started),
    )
    .unwrap_err();
    assert!(!started);
    assert!(
        matches!(
            error.downcast_ref::<FolderError>(),
            Some(FolderError::MidUpdate { name }) if name == "rimworld"
        ),
        "{error}"
    );
}

/// Runs `handoff` from the GOG folder on a thread, returning once the
/// program has handed off and exited.
fn gog_handoff(
    f: &Fixture,
    folder: &Path,
    token: &Token,
) -> thread::JoinHandle<(std::process::ExitStatus, Vec<ExecEvent>)> {
    let (paths, arg, token) = (f.paths.clone(), folder.display().to_string(), token.clone());
    let exec = thread::spawn(move || {
        let mut events = Vec::new();
        let status = launch::exec(
            &paths,
            "app",
            "handoff",
            &[arg],
            &BTreeMap::new(),
            &token,
            |event| events.push(event),
        )
        .unwrap();
        (status, events)
    });
    // The fake program exits right after writing this.
    wait_for(|| f.prefix.join("fake-pids").exists());
    thread::sleep(Duration::from_millis(300));
    exec
}

#[test]
fn a_handoff_from_a_gog_folder_holds_it_until_the_prefix_is_idle() {
    use wineo_core::gog::folder::{FolderError, lock_for_update};

    let f = Fixture::built();
    let folder = gog_game(&f, "rimworld", false);
    let started = Instant::now();
    let exec = gog_handoff(&f, &folder, &Token::new());
    assert!(!exec.is_finished());
    assert!(!f.prefix.join("handoff-done").exists());
    assert!(matches!(
        lock_for_update(&f.paths, &folder, "rimworld")
            .unwrap_err()
            .downcast_ref::<FolderError>(),
        Some(FolderError::InUse { .. })
    ));
    let (status, events) = exec.join().unwrap();
    // The fake wineserver -w returns once the handed-off program is done.
    assert!(f.prefix.join("handoff-done").exists());
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert!(status.success());
    assert!(matches!(events.last(), Some(ExecEvent::Exited { code: 0 })));
    assert!(lock_for_update(&f.paths, &folder, "rimworld").is_ok());
}

#[test]
fn canceling_a_gog_wait_asks_the_prefix_to_close() {
    let f = Fixture::built();
    let folder = gog_game(&f, "rimworld", false);
    let token = Token::new();
    let exec = gog_handoff(&f, &folder, &token);
    token.cancel();
    let (status, events) = exec.join().unwrap();
    // The started program's own status, though the wait was stopped.
    assert!(status.success());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ExecEvent::Stopping))
    );
    thread::sleep(Duration::from_millis(2500));
    assert!(!f.prefix.join("handoff-done").exists());
    assert!(wineo_core::gog::folder::lock_for_update(&f.paths, &folder, "rimworld").is_ok());
}

#[test]
fn forcing_a_gog_wait_kills_the_prefix() {
    let f = Fixture::built();
    let folder = gog_game(&f, "rimworld", false);
    let token = Token::new();
    let exec = gog_handoff(&f, &folder, &token);
    token.force();
    let (status, events) = exec.join().unwrap();
    assert!(status.success());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ExecEvent::Killing))
    );
    assert!(!f.prefix.join("handoff-done").exists());
    assert!(wineo_core::gog::folder::lock_for_update(&f.paths, &folder, "rimworld").is_ok());
}

#[test]
fn a_gog_wait_that_cant_settle_says_so_even_when_canceled() {
    let f = Fixture::built();
    let folder = gog_game(&f, "rimworld", false);
    let wineserver = f.paths.default_runner_root().join("fake/bin/wineserver");
    script(
        &wineserver,
        &runners::FAKE_WINESERVER.replacen("    done\n    ;;", "    done\n    exit 42\n    ;;", 1),
    );
    let token = Token::new();
    let (paths, arg, waiter) = (f.paths.clone(), folder.display().to_string(), token.clone());
    let exec = thread::spawn(move || {
        launch::exec(
            &paths,
            "app",
            "handoff",
            &[arg],
            &BTreeMap::new(),
            &waiter,
            |_| {},
        )
    });
    wait_for(|| f.prefix.join("fake-pids").exists());
    thread::sleep(Duration::from_millis(300));
    token.cancel();
    let error = exec.join().unwrap().unwrap_err();
    assert!(
        format!("{error:#}").contains("wineserver -w failed"),
        "{error:#}"
    );
}

#[test]
fn a_gog_wait_shows_what_failed_while_stopping() {
    let f = Fixture::built();
    let folder = gog_game(&f, "rimworld", false);
    let token = Token::new();
    let exec = gog_handoff(&f, &folder, &token);
    // The running wineserver -w keeps going, but the kill can't start.
    fs::remove_file(f.paths.default_runner_root().join("fake/bin/wineserver")).unwrap();
    token.force();
    let (_, events) = exec.join().unwrap();
    let warnings: Vec<_> = output_lines(&events)
        .into_iter()
        .filter(|line| line.starts_with("warning: "))
        .collect();
    assert!(
        warnings.iter().any(|line| line.contains("wineserver -k")),
        "{warnings:?}"
    );
}

#[test]
fn a_proton_handoff_from_a_gog_folder_holds_it_until_the_group_is_gone() {
    let f = Fixture::new();
    fs::write(
        f.paths.environment_config("app"),
        format!(
            "prefix = '{}'\nrunner = 'proton'\narch = 'win64'\n",
            f.prefix.display()
        ),
    )
    .unwrap();
    gog_game(&f, "rimworld", false);
    f.subprocess("proton-gog", f.root.path());
}

#[test]
fn handoff_releases_locks_without_waiting_for_descendants_or_their_output() {
    let f = Fixture::built();
    let started = Instant::now();
    f.collect("handoff", &[], &BTreeMap::new());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!f.prefix.join("handoff-done").exists());
    drop(
        Locks::new(&f.paths)
            .acquire(Operation::Remove, "app", &[&f.prefix])
            .unwrap(),
    );
    assert!(
        build_lock::try_exclusive(&f.paths, Kind::Runner, "fake")
            .unwrap()
            .is_some()
    );
    wait_for(|| f.prefix.join("handoff-done").exists());
}

#[test]
fn frontend_death_releases_locks_and_leaves_the_program_running() {
    let f = Fixture::built();
    f.subprocess("crash", f.root.path());
    wait_for(|| f.prefix.join("linger-pid").exists());
    drop(
        Locks::new(&f.paths)
            .acquire(Operation::Remove, "app", &[&f.prefix])
            .unwrap(),
    );
    assert!(
        build_lock::try_exclusive(&f.paths, Kind::Runner, "fake")
            .unwrap()
            .is_some()
    );
    wait_for(|| f.prefix.join("linger-done").exists());
}

#[test]
fn proton_build_exec_and_handoff() {
    let f = Fixture::new();
    fs::write(f.paths.environment_config("app"), format!("prefix = '{}'\nrunner = 'proton'\narch = 'win64'\ngameid = '123'\nstore = 'steam'\nwinetricks = ['corefonts']\n", f.prefix.display())).unwrap();
    f.subprocess("proton", f.root.path());
}

#[test]
fn exec_subprocess() {
    let Some(root) = std::env::var_os("WINEO_EXEC_TEST_ROOT") else {
        return;
    };
    let paths = paths(Path::new(&root));
    let scenario = std::env::var("WINEO_EXEC_TEST_SCENARIO").unwrap();
    let prefix = paths.default_prefix_root().join("app");
    let environment = Environment::load(&paths, "app").unwrap();
    match scenario.as_str() {
        "resolution" => {
            let global = GlobalConfig::default();
            let frontend = Launch::exec_with_resolution(
                &paths,
                &global,
                &environment,
                "setup.exe",
                &[],
                &BTreeMap::new(),
                ExecResolution::ExplicitPaths,
            )
            .unwrap();
            assert_eq!(frontend.args, ["setup.exe"]);
            assert_eq!(frontend.cwd, prefix.join("drive_c"));
            let cli = Launch::exec(
                &paths,
                &global,
                &environment,
                "setup.exe",
                &[],
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(
                cli.args,
                [Path::new(&root).join("setup.exe").display().to_string()]
            );
            let mut lines = Vec::new();
            launch::exec(
                &paths,
                "app",
                "setup.exe",
                &[],
                &BTreeMap::new(),
                &Token::new(),
                |event| {
                    if let ExecEvent::OutputLine(line) = event {
                        lines.push(line);
                    }
                },
            )
            .unwrap();
            assert_eq!(lines[0], "arg=<setup.exe>");
            assert!(lines.contains(&format!("cwd=<{}>", prefix.join("drive_c").display())));
        }
        "crash" => {
            launch::exec(
                &paths,
                "app",
                "linger",
                &[],
                &BTreeMap::new(),
                &Token::new(),
                |event| {
                    if matches!(event, ExecEvent::Started) {
                        std::process::exit(0);
                    }
                },
            )
            .unwrap();
            panic!("frontend should have exited");
        }
        "proton" => {
            let mut events = Vec::new();
            let status = launch::exec(
                &paths,
                "app",
                "winecfg",
                &["two words".to_owned()],
                &BTreeMap::from([("FAKE_VAR".to_owned(), "temporary value".to_owned())]),
                &Token::new(),
                |event| events.push(event),
            )
            .unwrap();
            assert!(status.success());
            assert!(matches!(
                events.first(),
                Some(ExecEvent::Launch(LaunchEvent::BuildingPrefix { .. }))
            ));
            let lines = output_lines(&events);
            assert!(lines.contains(&"umu-run: winecfg two words"));
            assert!(lines.contains(&"umu-run: GAMEID=unset STORE=unset"));
            assert!(lines.contains(&"umu-run: FAKE_VAR=temporary value"));
            assert!(lines.contains(&"arg=<two words>"));
            assert!(!prefix.join("pfx/winetricks.log").exists());
            let mut msi = Vec::new();
            let status = launch::exec(
                &paths,
                "app",
                "setup.MSI",
                &["two words".to_owned(), "".to_owned()],
                &BTreeMap::new(),
                &Token::new(),
                |event| msi.push(event),
            )
            .unwrap();
            assert!(status.success());
            let argv: Vec<_> = output_lines(&msi)
                .into_iter()
                .filter(|line| line.starts_with("arg="))
                .collect();
            assert_eq!(
                argv,
                [
                    "arg=<msiexec>",
                    "arg=</i>",
                    "arg=<setup.MSI>",
                    "arg=<two words>",
                    "arg=<>"
                ]
            );
            assert_eq!(
                launch::exec(
                    &paths,
                    "app",
                    "exit-37",
                    &[],
                    &BTreeMap::new(),
                    &Token::new(),
                    |_| {}
                )
                .unwrap()
                .code(),
                Some(37)
            );
            let started = Instant::now();
            assert!(
                launch::exec(
                    &paths,
                    "app",
                    "handoff",
                    &[],
                    &BTreeMap::new(),
                    &Token::new(),
                    |_| {}
                )
                .unwrap()
                .success()
            );
            assert!(started.elapsed() < Duration::from_secs(1));
            assert!(!prefix.join("handoff-done").exists());
            wait_for(|| prefix.join("handoff-done").exists());
        }
        "proton-gog" => {
            use wineo_core::gog::folder::lock_for_update;

            let folder = fs::canonicalize(&root).unwrap().join("Games/RimWorld");
            let started = Instant::now();
            let (exec_paths, arg) = (paths.clone(), folder.display().to_string());
            let exec = thread::spawn(move || {
                launch::exec(
                    &exec_paths,
                    "app",
                    "handoff",
                    &[arg],
                    &BTreeMap::new(),
                    &Token::new(),
                    |_| {},
                )
                .unwrap()
            });
            thread::sleep(Duration::from_millis(1000));
            assert!(!exec.is_finished());
            assert!(lock_for_update(&paths, &folder, "rimworld").is_err());
            assert!(exec.join().unwrap().success());
            // umu's group is only gone once the handed-off program is.
            assert!(prefix.join("handoff-done").exists());
            assert!(started.elapsed() >= Duration::from_secs(2));
            assert!(lock_for_update(&paths, &folder, "rimworld").is_ok());
        }
        _ => panic!("unknown scenario"),
    }
}
