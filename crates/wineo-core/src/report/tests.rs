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

#[test]
fn a_report_does_not_reread_the_config_it_was_given() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("game"),
        "prefix = \"~/prefix\"\nrunner = \"proton-ge\"\narch = \"win64\"\n",
    )
    .unwrap();
    let env = Environment::load(&paths, "game").unwrap();

    crate::read_log::clear();
    let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &env).unwrap();
    assert_eq!(report.name, "game");
    assert_eq!(crate::read_log::count("game.toml"), 0);
}

fn write_global(paths: &Paths, text: &str) {
    let path = paths.global_config();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn installed_manifest(paths: &Paths, app_id: &str) {
    fs::write(
        paths
            .home()
            .join(".local/share/Steam/steamapps")
            .join(format!("appmanifest_{app_id}.acf")),
        format!("\"AppState\" {{ \"appid\" \"{app_id}\" \"name\" \"Live\" \"StateFlags\" \"4\" }}"),
    )
    .unwrap();
}

fn install_of(environment: &OverviewEnvironment) -> &InstallReport {
    match &environment.kind {
        Some(KindReport::Proxied { install, .. }) => install,
        other => panic!("not proxied: {other:?}"),
    }
}

fn write_steam_run(paths: &Paths, name: &str) {
    let mut run = RunState::running(
        RunMode::Detached,
        "steam".to_owned(),
        1,
        None,
        &Default::default(),
    );
    run.launcher = Some(Launcher::Steam);
    Runtime::new(paths, name).write_state(&run).unwrap();
}

#[test]
fn a_bad_global_config_means_no_install_to_the_overview() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_library(&paths, &[("game", "111")]);
    installed_manifest(&paths, "111");
    let calls = std::cell::Cell::new(0);
    let scan = |_: &crate::steam::Install, _: &[String]| {
        calls.set(calls.get() + 1);
        Ok(HashMap::from([("111".to_owned(), vec![reaper(7)])]))
    };

    let good = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();
    assert!(good.steam.unwrap().install.is_some());
    assert_eq!(install_of(&good.environments[0]).state, "installed");
    assert_eq!(good.environments[0].status.state, State::SteamRunning);
    assert_eq!(calls.replace(0), 1);

    write_global(&paths, "not toml [");
    let bad = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();
    assert!(bad.steam.is_none());
    assert!(bad.errors.is_empty());
    let install = install_of(&bad.environments[0]);
    assert_eq!(install.state, "unknown");
    assert_eq!(install.problem.as_deref(), Some("Steam isn't found"));
    assert_eq!(bad.environments[0].status.state, State::Inactive);
    assert_eq!(calls.get(), 0);
}

#[test]
fn no_steam_with_a_good_global_config_still_has_a_steam_overview() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_library(&paths, &[("game", "111")]);
    fs::remove_dir_all(paths.home().join(".local/share/Steam")).unwrap();

    let overview = overview(&paths, OverviewFilter::default()).unwrap();
    let steam = overview.steam.unwrap();
    assert!(steam.install.is_none());
    assert!(steam.manifests.is_empty());
}

#[test]
fn a_lost_steam_run_is_cleaned_up_before_its_status_and_the_sweep() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_library(&paths, &[("idle", "111"), ("playing", "222")]);
    for name in ["idle", "playing"] {
        write_steam_run(&paths, name);
        assert!(Runtime::new(&paths, name).status().unwrap().lost);
    }
    let scan = |_: &crate::steam::Install, _: &[String]| {
        Ok(HashMap::from([("222".to_owned(), vec![reaper(7)])]))
    };

    let overview = overview_with(&paths, OverviewFilter::default(), &scan).unwrap();

    let states: Vec<_> = overview
        .environments
        .iter()
        .map(|environment| (environment.status.state, environment.status.lost))
        .collect();
    assert_eq!(
        states,
        [(State::Stopped, false), (State::SteamRunning, false)]
    );
    for name in ["idle", "playing"] {
        let run = Runtime::new(&paths, name).read_state().unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Stopped);
        assert!(run.lost);
        assert!(!paths.stats_log(name).exists());
    }
}

#[test]
fn only_a_loaded_steam_environment_cleans_up_a_lost_run() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_library(&paths, &[("blocked", "111"), ("broken", "222")]);
    fs::write(paths.environment_config("broken"), "not toml [").unwrap();
    fs::write(paths.environment_config("native"), "kind = \"native\"\n").unwrap();
    let marker = paths.restore_marker("blocked");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(marker, "").unwrap();
    for name in ["blocked", "broken"] {
        write_steam_run(&paths, name);
    }
    let mut run = RunState::running(
        RunMode::Detached,
        "app".to_owned(),
        1,
        None,
        &Default::default(),
    );
    run.native = true;
    Runtime::new(&paths, "native").write_state(&run).unwrap();

    let overview = overview(&paths, OverviewFilter::default()).unwrap();

    assert_eq!(overview.environments.len(), 3);
    for environment in &overview.environments {
        assert!(environment.status.lost, "{}", environment.name);
        assert_eq!(environment.status.state, State::Failed);
        let run = Runtime::new(&paths, &environment.name)
            .read_state()
            .unwrap()
            .unwrap();
        assert_eq!(run.status, RunStatus::Running);
    }
}

#[test]
fn a_bad_environment_is_a_blank_row_or_with_a_tag_filter_an_error() {
    let root = tempfile::tempdir().unwrap();
    let paths = test_paths(root.path());
    steam_library(&paths, &[("blocked", "111")]);
    fs::write(paths.environment_config("broken"), "not toml [").unwrap();
    fs::write(paths.environment_config("binary"), [0xff, 0xfe]).unwrap();
    let marker = paths.restore_marker("blocked");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(marker, "").unwrap();

    let plain = overview(&paths, OverviewFilter::default()).unwrap();
    assert!(plain.errors.is_empty());
    let rows: Vec<_> = plain
        .environments
        .iter()
        .map(|row| (row.name.as_str(), row.kind.is_none(), row.tags.is_empty()))
        .collect();
    assert_eq!(
        rows,
        [
            ("binary", true, true),
            ("blocked", true, true),
            ("broken", true, true),
        ]
    );
    assert!(plain.environments.iter().all(|row| row.hooks.is_none()));

    let tags = ["game".to_owned()];
    let filtered = overview(
        &paths,
        OverviewFilter {
            tags: Some(&tags),
            running_only: false,
        },
    )
    .unwrap();
    assert!(filtered.environments.is_empty());
    let errors: Vec<_> = filtered
        .errors
        .iter()
        .map(|error| (error.name.clone(), error.message.clone()))
        .collect();
    let expected: Vec<_> = ["binary", "blocked", "broken"]
        .map(|name| {
            let error = Environment::load(&paths, name).unwrap_err();
            (name.to_owned(), format!("{:#}", anyhow::Error::from(error)))
        })
        .to_vec();
    assert_eq!(errors, expected);
    // The source's text is part of the message, not only the context.
    assert!(errors[0].1.contains("failed to read"));
    assert!(errors[0].1.contains("UTF-8"), "{}", errors[0].1);
}
