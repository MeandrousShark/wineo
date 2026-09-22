mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use common::{Fixture, assert_success};
use wineo_core::config::{EnvironmentConfig, GlobalConfig};
use wineo_core::environment::Environment;
use wineo_core::operation::Operation;
use wineo_core::paths::Paths;
use wineo_core::process;
use wineo_core::report::EnvironmentReport;
use wineo_core::runners::Runners;
use wineo_core::runtime::{OperationState, Runtime};

fn runner_root(f: &Fixture) -> PathBuf {
    f.root.path().join("data/wineo/runners")
}
fn complete_proton(f: &Fixture, name: &str) {
    f.install_proton(name);
    let bin = runner_root(f).join(name).join("files/bin");
    fs::create_dir_all(&bin).unwrap();
    for name in ["wine", "wineserver"] {
        let path = bin.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
fn paths(f: &Fixture) -> Paths {
    Paths::from_lookup(|key| {
        let dir = match key {
            "HOME" => f.root.path().to_owned(),
            "XDG_CONFIG_HOME" => f.root.path().join("config"),
            "XDG_DATA_HOME" => f.root.path().join("data"),
            "XDG_STATE_HOME" => f.root.path().join("state"),
            _ => return None,
        };
        Some(dir.into_os_string())
    })
    .unwrap()
}

fn disposable_group() -> Child {
    Command::new("sleep")
        .arg("60")
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn stop_child(child: &mut Child) {
    if child.try_wait().unwrap().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

#[test]
fn aliases_accept_supported_hyphenated_architecture_names() {
    let f = Fixture::new();
    complete_proton(&f, "GE-Proton11-7-x86-64");
    complete_proton(&f, "proton-cachyos-11.0-20260703-slr-x86-64");
    let runners = Runners::at(runner_root(&f));
    assert_eq!(
        runners.resolve("GE-Proton-Latest").unwrap().name(),
        "GE-Proton11-7-x86-64"
    );
    assert_eq!(
        runners.resolve("Proton-CachyOS-Latest").unwrap().name(),
        "proton-cachyos-11.0-20260703-slr-x86-64"
    );
}

#[test]
fn aliases_skip_incomplete_manually_extracted_proton_builds() {
    let f = Fixture::new();
    complete_proton(&f, "GE-Proton11-7");
    f.install_proton("GE-Proton99-1");
    fs::remove_dir_all(runner_root(&f).join("GE-Proton99-1/files")).unwrap();
    assert_eq!(
        Runners::at(runner_root(&f))
            .resolve("GE-Proton-Latest")
            .unwrap()
            .name(),
        "GE-Proton11-7"
    );
}

#[test]
fn receipt_architecture_variant_stability_and_family_are_enforced() {
    let f = Fixture::new();
    complete_proton(&f, "GE-Proton11-7");
    complete_proton(&f, "candidate");
    let receipt = runner_root(&f).join("candidate/.wineo-install.json");
    let base = r#"{"provider":"ge-proton","version":"GE-Proton99-1","architecture":"x86_64","variant":"baseline","stable":true}"#;
    for (old, new) in [
        ("ge-proton", "proton-cachyos"),
        ("x86_64", "aarch64"),
        ("baseline", "x86_64_v3"),
        ("baseline", "slr"),
        ("true", "false"),
        ("GE-Proton99-1", "GE-Proton99-1-rc1"),
    ] {
        fs::write(&receipt, base.replace(old, new)).unwrap();
        assert_eq!(
            Runners::at(runner_root(&f))
                .resolve("GE-Proton-Latest")
                .unwrap()
                .name(),
            "GE-Proton11-7"
        );
    }
}

#[test]
fn status_uses_the_unresolved_installers_concrete_runner() {
    let f = Fixture::new();
    let paths = paths(&f);
    complete_proton(&f, "GE-Proton9-20");
    let runner = Runners::at(runner_root(&f))
        .resolve("GE-Proton-Latest")
        .unwrap();
    let runtime = Runtime::new(&paths, "test");
    runtime
        .write_operation(&OperationState {
            operation: Operation::Install,
            program: "setup.exe".into(),
            runner,
            prefix: f.root.path().join("prefix"),
            pid: None,
            group: None,
            boot_id: None,
            leader_start: None,
            started_at: jiff::Timestamp::now(),
        })
        .unwrap();
    complete_proton(&f, "GE-Proton11-7");
    let path = paths.environment_config("test");
    let environment = Environment {
        name: "test".into(),
        path: path.clone(),
        config: EnvironmentConfig::parse(
            &path,
            "prefix = \"~/prefix\"\nrunner = \"GE-Proton-Latest\"\narch = \"win64\"\n",
        )
        .unwrap(),
    };
    let report = EnvironmentReport::load(&paths, &GlobalConfig::default(), &environment).unwrap();
    assert_eq!(report.runner, "GE-Proton-Latest -> GE-Proton9-20");
}

#[test]
fn template_and_set_keep_the_alias_in_config() {
    let f = Fixture::new();
    complete_proton(&f, "GE-Proton11-7");
    f.write_template("following", "runner = \"GE-Proton-Latest\"\n");
    assert_success(&f.run(&["new", "-n", "templated", "--template", "following"]));
    assert!(
        f.created_config("templated")
            .contains("runner = \"GE-Proton-Latest\"")
    );
    assert_success(&f.run(&["set", "templated", "runner=GE-Proton11-7"]));
    assert!(
        f.created_config("templated")
            .contains("runner = \"GE-Proton11-7\"")
    );
    assert_success(&f.run(&["set", "templated", "runner=GE-Proton-Latest"]));
    complete_proton(&f, "GE-Proton12-1");
    assert!(
        f.created_config("templated")
            .contains("runner = \"GE-Proton-Latest\"")
    );
}

#[test]
fn creation_installer_keeps_the_runner_resolved_before_a_new_download() {
    let f = Fixture::new();
    let paths = paths(&f);
    complete_proton(&f, "GE-Proton9-20");
    let old = Runners::at(runner_root(&f))
        .resolve("GE-Proton-Latest")
        .unwrap();
    let prefix = f.root.path().join("prefix");
    fs::create_dir_all(prefix.join("pfx/drive_c")).unwrap();
    fs::write(
        prefix.join("pfx/system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("test"),
        format!(
            "prefix = {:?}\nrunner = \"GE-Proton-Latest\"\narch = \"win64\"\n",
            prefix.to_str().unwrap()
        ),
    )
    .unwrap();
    let installer_program = f.root.path().join("setup.exe");
    fs::write(&installer_program, "fixture").unwrap();
    let guard = wineo_core::operation::Locks::new(&paths)
        .acquire(Operation::Create, "test", &[&prefix])
        .unwrap();
    complete_proton(&f, "GE-Proton11-7");
    let installer = wineo_core::installer::Installer::prepare_with_runner(
        guard,
        &paths,
        &GlobalConfig::default(),
        "test",
        installer_program.to_str().unwrap(),
        &old,
    )
    .unwrap();
    assert_eq!(installer.launch().runner.name(), "GE-Proton9-20");
    assert_eq!(installer.launch().runner.selection(), "GE-Proton-Latest");
    assert!(
        f.created_config("test")
            .contains("runner = \"GE-Proton-Latest\"")
    );
}

#[test]
fn lost_run_cleanup_uses_its_snapshot_when_the_alias_has_no_candidates() {
    let f = Fixture::new();
    let paths = paths(&f);
    complete_proton(&f, "GE-Proton9-20");
    let runner = Runners::at(runner_root(&f))
        .resolve("GE-Proton-Latest")
        .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("test"),
        "prefix = \"~/prefix\"\nrunner = \"GE-Proton-Latest\"\narch = \"win64\"\n",
    )
    .unwrap();
    fs::create_dir_all(f.root.path().join("prefix")).unwrap();
    let prefix = fs::canonicalize(f.root.path().join("prefix")).unwrap();
    let mut child = disposable_group();
    let pid = child.id();
    let mut state = wineo_core::runtime::RunState::running(
        wineo_core::runtime::RunMode::Detached,
        "game".into(),
        pid,
        Some(pid),
        &Default::default(),
    );
    state.runner = Some(runner);
    state.prefix = Some(prefix);
    state.boot_id = process::boot_id();
    state.leader_start = process::start_time(pid);
    let runtime = Runtime::new(&paths, "test");
    runtime.write_state(&state).unwrap();
    fs::remove_dir_all(runner_root(&f)).unwrap();
    let stopped = f.run(&["stop", "test"]);
    stop_child(&mut child);
    assert_success(&stopped);
    assert_eq!(
        runtime.read_state().unwrap().unwrap().status,
        wineo_core::runtime::RunStatus::Stopped
    );
}

#[test]
fn installer_cleanup_uses_its_snapshot_when_the_alias_has_no_candidates() {
    let f = Fixture::new();
    let paths = paths(&f);
    complete_proton(&f, "GE-Proton9-20");
    let runner = Runners::at(runner_root(&f))
        .resolve("GE-Proton-Latest")
        .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("test"),
        "prefix = \"~/prefix\"\nrunner = \"GE-Proton-Latest\"\narch = \"win64\"\n",
    )
    .unwrap();
    let runtime = Runtime::new(&paths, "test");
    fs::create_dir_all(f.root.path().join("prefix")).unwrap();
    let prefix = fs::canonicalize(f.root.path().join("prefix")).unwrap();
    let mut child = disposable_group();
    let pid = child.id();
    runtime
        .write_operation(&OperationState {
            operation: Operation::Install,
            program: "setup.exe".into(),
            runner,
            prefix,
            pid: Some(pid),
            group: Some(pid),
            boot_id: process::boot_id(),
            leader_start: process::start_time(pid),
            started_at: jiff::Timestamp::now(),
        })
        .unwrap();
    fs::remove_dir_all(runner_root(&f)).unwrap();
    let stopped = f.run(&["stop", "test"]);
    stop_child(&mut child);
    assert_success(&stopped);
    assert!(runtime.read_operation().unwrap().is_none());
}
