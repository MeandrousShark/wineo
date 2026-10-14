use std::ffi::OsString;
use std::fs;
use std::path::Path;

use super::*;
use crate::config::EnvironmentConfig;
use crate::runtime::{RunMode, RunState};

fn test_paths(root: &Path) -> Paths {
    let root = root.to_owned();
    Paths::from_lookup(move |key| match key {
        "HOME" => Some(OsString::from(root.join("home"))),
        _ => None,
    })
    .unwrap()
}

fn environment(text: &str) -> Environment {
    let text = format!("prefix = \"~/prefix\"\nrunner = \"proton-ge\"\narch = \"win64\"\n{text}");
    Environment {
        name: "notepad".to_owned(),
        path: Path::new("/notepad.toml").to_owned(),
        config: EnvironmentConfig::parse(Path::new("/notepad.toml"), &text).unwrap(),
    }
}

// An unsupported runner name resolves deterministically to no version,
// without needing wine installed to run this test.
#[test]
fn runner_version_is_none_when_the_runner_cant_be_resolved() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let env = environment("");
    let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
    assert_eq!(report.runner.as_deref(), Some("proton-ge"));
    assert_eq!(report.runner_version, None);
    assert_eq!(report.prefix.as_deref(), Some("~/prefix"));
}

#[test]
fn live_manifest_title_wins_over_imported_title() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let steamapps = paths.home().join(".steam/root/steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::write(
        steamapps.join("appmanifest_289070.acf"),
        "\"AppState\" { \"appid\" \"289070\" \"name\" \"Civilization VI\" \"StateFlags\" \"4\" }",
    )
    .unwrap();
    let config = EnvironmentConfig::parse(
        Path::new("/civ.toml"),
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"289070\"\ntitle = \"Old title\"\n",
    )
    .unwrap();
    let kind = kind_report(&paths, &GlobalConfig::default(), &config.kind);
    assert!(
        matches!(kind, KindReport::Proxied { title: Some(title), .. } if title == "Civilization VI")
    );
}

#[test]
fn overview_does_not_scan_steam_without_proxied_environments() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let steamapps = paths.home().join(".local/share/Steam/steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::write(
        steamapps.join("appmanifest_289070.acf"),
        "\"AppState\" { \"appid\" \"289070\" \"name\" \"Civilization VI\" \"StateFlags\" \"4\" }",
    )
    .unwrap();
    assert!(crate::steam::active_for(&paths, &GlobalConfig::default()).is_some());

    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("notepad"),
        "prefix = \"~/prefix\"\nrunner = \"proton-ge\"\narch = \"win64\"\n",
    )
    .unwrap();

    let overview = overview(&paths, OverviewFilter::default()).unwrap();
    assert_eq!(overview.environments.len(), 1);
    assert!(overview.steam.is_none());
}

/// One proxied environment per `(name, app id)`, with a Steam install.
fn steam_library(paths: &Paths, proxied: &[(&str, &str)]) {
    fs::create_dir_all(paths.home().join(".local/share/Steam/steamapps")).unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    for (name, app_id) in proxied {
        fs::write(
            paths.environment_config(name),
            format!("kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"{app_id}\"\n"),
        )
        .unwrap();
    }
}

fn reaper(pid: u32) -> crate::steam::Reaper {
    crate::steam::Reaper {
        pid,
        start_time: 1,
        boot_id: "boot".to_owned(),
    }
}

#[test]
fn overview_sweeps_processes_once_for_every_proxied_environment() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_library(
        &paths,
        &[
            ("game-a", "111"),
            ("game-b", "222"),
            ("game-c", "333"),
            ("game-d", "222"),
        ],
    );

    let calls = std::cell::RefCell::new(Vec::<Vec<String>>::new());
    let scan = |_: &crate::steam::Install, app_ids: &[String]| {
        calls.borrow_mut().push(app_ids.to_vec());
        Ok(app_ids
            .iter()
            .map(|app_id| {
                let reapers = if app_id == "222" {
                    vec![reaper(7), reaper(8)]
                } else {
                    Vec::new()
                };
                (app_id.clone(), reapers)
            })
            .collect())
    };
    let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();

    // Two environments share app 222, which is asked about once.
    assert_eq!(*calls.borrow(), [["111", "222", "333"]]);
    let states: Vec<_> = overview
        .environments
        .iter()
        .map(|environment| (environment.name.as_str(), environment.status.state))
        .collect();
    assert_eq!(
        states,
        [
            ("game-a", State::Inactive),
            ("game-b", State::SteamRunning),
            ("game-c", State::Inactive),
            ("game-d", State::SteamRunning),
        ]
    );
}

#[test]
fn environments_that_are_busy_or_running_never_cause_a_sweep() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_library(
        &paths,
        &[
            ("busy", "111"),
            ("running", "222"),
            ("starting", "333"),
            ("hook", "444"),
        ],
    );
    let locks = Locks::new(&paths);
    let _busy = locks.acquire(Operation::Winetricks, "busy", &[]).unwrap();

    let state = |name: &str, phase: Option<crate::runtime::RunPhase>| {
        let mut state = RunState::running(
            RunMode::Detached,
            "steam".to_owned(),
            1,
            None,
            &Default::default(),
        );
        state.phase = phase;
        Runtime::new(&paths, name).write_state(&state).unwrap();
    };
    state("running", None);
    let _running = Runtime::new(&paths, "running").try_lock().unwrap().unwrap();
    state("starting", Some(crate::runtime::RunPhase::Starting));
    let _starting = Runtime::new(&paths, "starting")
        .try_lock()
        .unwrap()
        .unwrap();
    // The after hook runs once the game has already exited.
    let mut exited = RunState::running(
        RunMode::Detached,
        "steam".to_owned(),
        1,
        None,
        &Default::default(),
    );
    exited.status = RunStatus::Exited;
    Runtime::new(&paths, "hook").write_state(&exited).unwrap();
    let _hook = locks
        .acquire(Operation::Hook(crate::hooks::Phase::After), "hook", &[])
        .unwrap();

    let calls = std::cell::Cell::new(0);
    let scan = |_: &crate::steam::Install, _: &[String]| {
        calls.set(calls.get() + 1);
        Ok(HashMap::new())
    };
    let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();

    assert_eq!(calls.get(), 0);
    let states: Vec<_> = overview
        .environments
        .iter()
        .map(|environment| (environment.name.as_str(), environment.status.state))
        .collect();
    assert_eq!(
        states,
        [
            ("busy", State::Inactive),
            ("hook", State::Exited),
            ("running", State::Running),
            ("starting", State::Starting),
        ]
    );

    // One idle game among them is enough to sweep, once.
    steam_library(&paths, &[("idle", "555")]);
    overview_with(&paths, OverviewFilter::default(), &scan).unwrap();
    assert_eq!(calls.get(), 1);
}

#[test]
fn overview_reads_each_trash_manifest_once() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    fs::create_dir_all(paths.environments_dir()).unwrap();
    for index in 0..5 {
        let name = format!("gone-{index}");
        fs::write(paths.environment_config(&name), "kind = 'native'\n").unwrap();
        crate::trash::remove(&paths, &name, false).unwrap();
    }
    for index in 0..4 {
        fs::write(
            paths.environment_config(&format!("kept-{index}")),
            "kind = 'native'\n",
        )
        .unwrap();
    }
    let scan = |_: &crate::steam::Install, _: &[String]| Ok(HashMap::new());

    crate::trash::take_manifest_reads();
    let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();
    assert_eq!(overview.environments.len(), 4);
    assert_eq!(crate::trash::take_manifest_reads(), 5);
}

#[test]
fn a_failed_sweep_is_reported_for_each_environment_that_needed_it() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_library(&paths, &[("game-111", "111"), ("game-222", "222")]);

    let calls = std::cell::Cell::new(0);
    let scan = |_: &crate::steam::Install, _: &[String]| {
        calls.set(calls.get() + 1);
        Err(std::io::Error::other("no /proc"))
    };
    let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();

    assert_eq!(calls.get(), 1);
    assert!(overview.environments.is_empty());
    let messages: Vec<_> = overview
        .errors
        .iter()
        .map(|error| (error.name.as_str(), error.message.as_str()))
        .collect();
    assert_eq!(
        messages,
        [
            (
                "game-111",
                "failed to scan Steam processes for game-111: no /proc"
            ),
            (
                "game-222",
                "failed to scan Steam processes for game-222: no /proc"
            ),
        ]
    );
}

#[test]
fn overview_skips_the_sweep_without_a_steam_install() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_library(&paths, &[("game-111", "111")]);
    fs::remove_dir_all(paths.home().join(".local/share/Steam")).unwrap();

    let calls = std::cell::Cell::new(0);
    let scan = |_: &crate::steam::Install, _: &[String]| {
        calls.set(calls.get() + 1);
        Ok(HashMap::new())
    };
    let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();

    assert_eq!(calls.get(), 0);
    assert_eq!(overview.environments.len(), 1);
}

#[test]
fn target_exe_is_looked_up_from_the_current_config() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    let env = environment("[targets.notepad]\nexe = \"notepad\"\n");
    let runtime = Runtime::new(&paths, &env.name);

    // No run yet: no target to report.
    let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
    assert_eq!(report.target_exe, None);

    let state = RunState::running(
        RunMode::Detached,
        "notepad".to_owned(),
        1,
        Some(1),
        &Default::default(),
    );
    runtime.write_state(&state).unwrap();
    let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
    assert_eq!(report.target_exe.as_deref(), Some("notepad"));

    let mut state = state;
    state.target = "gone".to_owned();
    runtime.write_state(&state).unwrap();
    let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
    assert_eq!(report.target_exe, None);
}
