use super::*;

use std::io::{Read, Write};
use std::os::unix::fs::symlink;
use std::process::{Command, Stdio};

use crate::launch::Overrides;
use crate::operation::Operation;
use crate::runtime::{OperationState, RunMode, RunState};

struct Fixture {
    root: tempfile::TempDir,
    paths: Paths,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        Self { root, paths }
    }

    fn runner(&self, name: &str) -> PathBuf {
        let directory = self.paths.default_runner_root().join(name);
        fs::create_dir_all(directory.join("bin")).unwrap();
        for program in ["wine", "wineserver"] {
            symlink("/bin/true", directory.join("bin").join(program)).unwrap();
        }
        directory
    }

    fn proton(&self, name: &str) -> PathBuf {
        let directory = self.paths.default_runner_root().join(name);
        fs::create_dir_all(directory.join("files/bin")).unwrap();
        for program in ["wine", "wineserver"] {
            symlink("/bin/true", directory.join("files/bin").join(program)).unwrap();
        }
        symlink("/bin/true", directory.join("proton")).unwrap();
        directory
    }

    fn component(&self, component: Component, name: &str) -> PathBuf {
        let builds = Builds::new(&self.paths, &GlobalConfig::default(), component).unwrap();
        let directory = builds.root().join(name);
        let (wide, narrow) = component.arch_dirs();
        for arch in [wide, narrow] {
            fs::create_dir_all(directory.join(arch)).unwrap();
            for dll in component.overrides() {
                fs::write(directory.join(arch).join(format!("{dll}.dll")), b"dll").unwrap();
            }
        }
        directory
    }

    fn environment(&self, name: &str, selections: &str) {
        fs::create_dir_all(self.paths.environments_dir()).unwrap();
        fs::write(
            self.paths.environment_config(name),
            format!(
                "prefix = '{}'\narch = 'win64'\n{selections}\n",
                self.root.path().join("prefix").display(),
            ),
        )
        .unwrap();
    }

    fn template(&self, name: &str, text: &str) {
        fs::create_dir_all(self.paths.templates_dir()).unwrap();
        fs::write(
            self.paths.templates_dir().join(format!("{name}.toml")),
            text,
        )
        .unwrap();
    }

    fn global(&self, text: &str) {
        let path = self.paths.global_config();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn uses(&self, kind: Kind, name: &str) -> Uses {
        uses(&self.paths, &Lock::new(&self.paths, kind, name)).unwrap()
    }
}

#[test]
fn runner_references_cover_concrete_environment_template_and_global() {
    let f = Fixture::new();
    f.template("gaming", "");
    f.runner("build");
    f.environment("hades", "runner = 'build'");
    f.template("custom", "runner = 'build'");
    f.global("[defaults]\nrunner = 'build'\n");
    assert_eq!(
        f.uses(Kind::Runner, "build").labels,
        ["global default runner", "hades", "template custom"]
    );
    let error = crate::runners::remove(&f.paths, "build").unwrap_err();
    assert_eq!(
        error.to_string(),
        "build is used by global default runner, hades, and template custom."
    );
    assert!(f.paths.default_runner_root().join("build").is_dir());
}

#[test]
fn runner_aliases_and_template_proton_follow_existing_resolution() {
    for (alias, old, current) in [
        ("GE-Proton-Latest", "GE-Proton9-19", "GE-Proton9-20"),
        (
            "Proton-CachyOS-Latest",
            "proton-cachyos-10.0-20260101-slr",
            "proton-cachyos-11.0-20260201-slr",
        ),
    ] {
        let f = Fixture::new();
        f.proton(old);
        f.proton(current);
        f.environment("alias", &format!("runner = '{alias}'"));
        f.environment("not-an-alias", "runner = 'proton'");
        f.global(&format!("[defaults]\nrunner = '{alias}'\n"));
        let references = f.uses(Kind::Runner, current).labels;
        assert_eq!(
            references,
            [
                format!("alias ({alias})"),
                format!("global default runner ({alias})"),
                "template gaming (proton)".to_owned()
            ]
        );
        assert!(f.uses(Kind::Runner, old).labels.is_empty());
        assert!(crate::runners::remove(&f.paths, current).is_err());
        assert!(crate::runners::remove(&f.paths, old).is_ok());
    }
}

#[test]
fn template_proton_falls_back_to_the_global_runner() {
    let f = Fixture::new();
    f.runner("plain");
    f.global("[defaults]\nrunner = 'plain'\n");
    assert_eq!(
        f.uses(Kind::Runner, "plain").labels,
        ["global default runner", "template gaming (proton)"]
    );
}

#[test]
fn component_references_cover_concrete_and_latest_in_environments_and_templates() {
    for component in Component::ALL {
        let f = Fixture::new();
        f.component(component, "1.0");
        let current = f.component(component, "2.0");
        f.environment("direct", &format!("runner = 'wine'\n{component} = '2.0'"));
        f.environment("alias", &format!("runner = 'wine'\n{component} = 'latest'"));
        f.template("custom", &format!("{component} = '2.0'\n"));
        assert_eq!(
            f.uses(component.into(), "2.0").labels,
            [
                "alias (latest)",
                "direct",
                "template custom",
                "template gaming (latest)"
            ]
        );
        assert!(f.uses(component.into(), "1.0").labels.is_empty());
        assert!(crate::components::remove(&f.paths, component, "2.0").is_err());
        assert!(current.is_dir());
        assert!(crate::components::remove(&f.paths, component, "1.0").is_ok());
    }
}

#[test]
fn unreadable_configs_are_listed_but_block_removal() {
    let f = Fixture::new();
    f.runner("unused");
    f.environment("broken", "runner = [invalid");
    f.template("broken", "not valid TOML");
    let uses = f.uses(Kind::Runner, "unused");
    assert!(uses.labels.is_empty());
    assert_eq!(
        uses.errors
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["broken", "template broken"]
    );
    assert!(matches!(
        crate::runners::remove(&f.paths, "unused"),
        Err(Error::UnreadableConfigs { .. })
    ));
    assert!(f.paths.default_runner_root().join("unused").is_dir());
}

#[test]
fn an_unreadable_environment_blocks_removal_until_fixed() {
    let f = Fixture::new();
    let dir = f.runner("unused");
    f.environment("hades", "runner = [invalid");
    let error = crate::runners::remove(&f.paths, "unused").unwrap_err();
    assert!(matches!(error, Error::UnreadableConfigs { .. }));
    let text = error.to_string();
    let (summary, rest) = text.split_once('\n').unwrap();
    assert_eq!(summary, "unused is kept, since a config can't be read");
    assert!(
        rest.starts_with(&format!(
            "invalid config {}",
            f.paths.environment_config("hades").display()
        )),
        "{text}"
    );
    assert!(dir.is_dir());
    f.environment("hades", "runner = 'wine'");
    crate::runners::remove(&f.paths, "unused").unwrap();
    assert!(!dir.exists());
}

#[test]
fn an_unreadable_template_blocks_removal_until_fixed() {
    let f = Fixture::new();
    let dir = f.runner("unused");
    f.template("custom", "not valid TOML");
    let error = crate::runners::remove(&f.paths, "unused").unwrap_err();
    let text = error.to_string();
    assert!(text.starts_with("unused is kept, since a config can't be read\n"));
    assert!(
        text.contains(
            &f.paths
                .templates_dir()
                .join("custom.toml")
                .display()
                .to_string()
        ),
        "{text}"
    );
    assert!(dir.is_dir());
    f.template("custom", "");
    crate::runners::remove(&f.paths, "unused").unwrap();
    assert!(!dir.exists());
}

#[test]
fn an_unreadable_config_blocks_component_removal_too() {
    for component in Component::ALL {
        let f = Fixture::new();
        let dir = f.component(component, "1.0");
        // The default template selects the newest build, so keep one newer.
        f.component(component, "2.0");
        f.environment("hades", "runner = [invalid");
        assert!(matches!(
            crate::components::remove(&f.paths, component, "1.0"),
            Err(Error::UnreadableConfigs { .. })
        ));
        assert!(dir.is_dir());
    }
}

#[test]
fn a_build_in_use_still_reports_its_users_before_unreadable_configs() {
    let f = Fixture::new();
    let dir = f.runner("used");
    f.environment("hades", "runner = 'used'");
    f.environment("broken", "runner = [invalid");
    assert!(matches!(
        crate::runners::remove(&f.paths, "used"),
        Err(Error::BuildInUse { .. })
    ));
    assert!(dir.is_dir());
}

#[test]
fn active_runs_and_installer_records_survive_changed_or_removed_configs() {
    let f = Fixture::new();
    f.runner("old");
    let runner = Runners::new(&f.paths, &GlobalConfig::default())
        .unwrap()
        .resolve("old")
        .unwrap();
    f.environment("changed", "runner = 'wine'");
    let runtime = Runtime::new(&f.paths, "changed");
    let held = runtime.try_lock().unwrap().unwrap();
    let mut state = RunState::running(
        RunMode::Detached,
        "app".to_owned(),
        std::process::id(),
        None,
        &Overrides::default(),
    );
    state.runner = Some(runner.clone());
    runtime.write_state(&state).unwrap();
    let record = OperationState {
        operation: Operation::Install,
        program: "setup.exe".to_owned(),
        runner,
        prefix: f.root.path().join("prefix"),
        pid: None,
        group: None,
        boot_id: None,
        leader_start: None,
        started_at: jiff::Timestamp::now(),
    };
    Runtime::new(&f.paths, "removed")
        .write_operation(&record)
        .unwrap();
    assert_eq!(
        f.uses(Kind::Runner, "old").labels,
        ["active run changed", "installer removed"]
    );
    assert!(crate::runners::remove(&f.paths, "old").is_err());
    drop(held);
    assert!(
        f.uses(Kind::Runner, "old")
            .labels
            .contains(&"active run changed".to_owned())
    );
    state.finish(crate::runtime::RunStatus::Exited);
    runtime.write_state(&state).unwrap();
    Runtime::new(&f.paths, "removed").clear_operation().unwrap();
    assert!(f.uses(Kind::Runner, "old").labels.is_empty());
    crate::runners::remove(&f.paths, "old").unwrap();
}

#[test]
fn shared_build_locks_and_install_locks_refuse_without_waiting() {
    let f = Fixture::new();
    let dir = f.runner("unused");
    let lock = Lock::new(&f.paths, Kind::Runner, "unused");
    let shared = lock.shared().unwrap();
    let started = std::time::Instant::now();
    let error = crate::runners::remove(&f.paths, "unused").unwrap_err();
    assert_eq!(error.to_string(), "unused is used by another operation.");
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(dir.is_dir());
    drop(shared);
    let installing = InstallLock::acquire(&dir).unwrap();
    let error = crate::runners::remove(&f.paths, "unused").unwrap_err();
    assert!(error.to_string().contains("another install"));
    assert!(dir.is_dir());
    drop(installing);
    crate::runners::remove(&f.paths, "unused").unwrap();
}

#[test]
fn config_written_between_first_check_and_lock_is_caught() {
    let f = Fixture::new();
    let dir = f.runner("unused");
    let lock = Lock::new(&f.paths, Kind::Runner, "unused");
    let error = remove_after_check(&f.paths, &lock, &dir, || {
        let _guard = lock.shared().unwrap();
        f.environment("new-user", "runner = 'unused'");
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "unused is used by new-user.");
    assert!(dir.is_dir());
}

#[test]
fn success_returns_size_and_does_not_follow_external_symlinks() {
    let f = Fixture::new();
    let dir = f.runner("unused");
    let outside = f.root.path().join("external");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep"), b"untouched").unwrap();
    symlink(&outside, dir.join("external")).unwrap();
    fs::write(dir.join("payload"), b"1234567").unwrap();
    let expected = crate::runners::size(&f.paths, "unused").unwrap();
    assert_eq!(
        crate::runners::remove(&f.paths, "unused").unwrap().bytes,
        expected
    );
    assert!(!dir.exists());
    assert_eq!(fs::read(outside.join("keep")).unwrap(), b"untouched");
    assert!(!fs::read_dir(dir.parent().unwrap()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".wineo-remove-")
    }));
    for component in Component::ALL {
        f.template("gaming", "");
        let dir = f.component(component, "1.0");
        let expected = crate::components::size(&f.paths, component, "1.0").unwrap();
        assert_eq!(
            crate::components::remove(&f.paths, component, "1.0")
                .unwrap()
                .bytes,
            expected
        );
        assert!(!dir.exists());
    }
}

#[test]
fn system_alias_and_nonmanaged_symlinked_builds_are_refused() {
    let f = Fixture::new();
    assert!(matches!(
        crate::runners::remove(&f.paths, "wine"),
        Err(Error::UnmanagedBuild(_))
    ));
    for alias in crate::runner_alias::ALIASES {
        assert!(matches!(
            crate::runners::remove(&f.paths, alias),
            Err(Error::UnmanagedBuild(_))
        ));
    }
    let original = f.runner("outside");
    symlink(&original, f.paths.default_runner_root().join("linked")).unwrap();
    assert!(matches!(
        crate::runners::remove(&f.paths, "linked"),
        Err(Error::UnmanagedBuild(_))
    ));
    assert!(original.is_dir());
    for name in ["..", "../outside", "/outside", ".hidden"] {
        assert!(matches!(
            crate::runners::remove(&f.paths, name),
            Err(Error::InvalidRunnerName(_))
        ));
    }
    for component in Component::ALL {
        let original = f.component(component, "outside");
        symlink(&original, original.parent().unwrap().join("linked")).unwrap();
        assert!(matches!(
            crate::components::remove(&f.paths, component, "linked"),
            Err(Error::UnmanagedBuild(_))
        ));
        assert!(matches!(
            crate::components::remove(&f.paths, component, "latest"),
            Err(Error::UnmanagedBuild(_))
        ));
        assert!(original.is_dir());
    }
}

#[test]
fn interrupted_removal_leaves_no_usable_build() {
    let f = Fixture::new();
    let dir = f.runner("unused");
    let staged = move_out(&dir).unwrap();
    fs::remove_file(staged.join("build/bin/wine")).unwrap();
    assert!(!dir.exists());
    assert!(
        Runners::new(&f.paths, &GlobalConfig::default())
            .unwrap()
            .list()
            .unwrap()
            .runners
            .is_empty()
    );
    assert!(staged.is_dir());
    for component in Component::ALL {
        let dir = f.component(component, "1.0");
        let staged = move_out(&dir).unwrap();
        fs::remove_dir_all(staged.join("build/x64")).unwrap();
        assert!(
            Builds::new(&f.paths, &GlobalConfig::default(), component)
                .unwrap()
                .list()
                .unwrap()
                .builds
                .is_empty()
        );
    }
}

#[test]
fn shared_lock_child() {
    let Some(root) = std::env::var_os("WINEO_REMOVE_LOCK_TEST_ROOT") else {
        return;
    };
    let paths = Paths::from_lookup(|key| (key == "HOME").then(|| root.clone())).unwrap();
    let _held = Lock::new(&paths, Kind::Runner, "unused").shared().unwrap();
    fs::write(Path::new(&root).join("ready"), "ready").unwrap();
    let mut bytes = [0];
    std::io::stdin().read_exact(&mut bytes).unwrap();
}

#[test]
fn another_process_holding_the_launch_build_lock_refuses_removal() {
    let f = Fixture::new();
    let dir = f.runner("unused");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "build_removal::tests::shared_lock_child"])
        .env("WINEO_REMOVE_LOCK_TEST_ROOT", f.root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !f.root.path().join("ready").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child did not take its lock"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let start = std::time::Instant::now();
    let refused = crate::runners::remove(&f.paths, "unused");
    let elapsed = start.elapsed();
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(matches!(refused, Err(Error::BuildInUse { .. })));
    assert!(elapsed < std::time::Duration::from_secs(1));
    assert!(dir.is_dir());
    crate::runners::remove(&f.paths, "unused").unwrap();
}

#[test]
fn component_build_and_install_locks_refuse_removal() {
    for component in Component::ALL {
        let f = Fixture::new();
        f.template("gaming", "");
        let dir = f.component(component, "1.0");
        let shared = Lock::new(&f.paths, component.into(), "1.0")
            .shared()
            .unwrap();
        assert!(matches!(
            crate::components::remove(&f.paths, component, "1.0"),
            Err(Error::BuildInUse { .. })
        ));
        assert!(dir.is_dir());
        drop(shared);
        let install = InstallLock::acquire(&dir).unwrap();
        assert!(
            crate::components::remove(&f.paths, component, "1.0")
                .unwrap_err()
                .to_string()
                .contains("another install")
        );
        assert!(dir.is_dir());
        drop(install);
        crate::components::remove(&f.paths, component, "1.0").unwrap();
    }
}

#[test]
fn configured_roots_are_used_and_reading_references_does_not_rewrite_configs() {
    let f = Fixture::new();
    f.template("gaming", "");
    f.environment("hades", "runner = 'build'");
    let config = f.paths.environment_config("hades");
    let before = fs::read(&config).unwrap();
    let modified = fs::metadata(&config).unwrap().modified().unwrap();
    let custom = f.root.path().join("custom-runners");
    fs::create_dir_all(custom.join("build/bin")).unwrap();
    for program in ["wine", "wineserver"] {
        symlink("/bin/true", custom.join("build/bin").join(program)).unwrap();
    }
    f.global(&format!(
        "[defaults]\nrunner_root = '{}'\n",
        custom.display()
    ));
    assert_eq!(f.uses(Kind::Runner, "build").labels, ["hades"]);
    assert!(crate::runners::remove(&f.paths, "build").is_err());
    assert_eq!(fs::read(&config).unwrap(), before);
    assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), modified);
    fs::remove_file(config).unwrap();
    crate::runners::remove(&f.paths, "build").unwrap();
    assert!(!custom.join("build").exists());
}

#[test]
fn removal_names_each_unreadable_config_and_keeps_the_build() {
    let f = Fixture::new();
    let dir = f.runner("unused");
    f.environment("broken", "runner = [invalid");
    f.template("broken", "not valid TOML");
    let error = crate::runners::remove(&f.paths, "unused").unwrap_err();
    let text = error.to_string();
    let (summary, rest) = text.split_once('\n').unwrap();
    assert_eq!(summary, "unused is kept, since 2 configs can't be read");
    let environment = rest
        .find(&f.paths.environment_config("broken").display().to_string())
        .unwrap();
    let template = rest
        .find(
            &f.paths
                .templates_dir()
                .join("broken.toml")
                .display()
                .to_string(),
        )
        .unwrap();
    assert!(environment < template, "{text}");
    assert!(dir.is_dir());
}

#[test]
fn removal_refuses_a_config_that_breaks_between_the_checks() {
    let f = Fixture::new();
    let dir = f.runner("unused");
    let lock = Lock::new(&f.paths, Kind::Runner, "unused");
    let error = remove_after_check(&f.paths, &lock, &dir, || {
        f.environment("later", "runner = [invalid");
    })
    .unwrap_err();
    let text = error.to_string();
    assert!(text.starts_with("unused is kept, since a config can't be read\n"));
    assert!(
        text.contains(&f.paths.environment_config("later").display().to_string()),
        "{text}"
    );
    assert!(dir.is_dir());
}

#[test]
fn staged_removal_child() {
    let Some(root) = std::env::var_os("WINEO_STAGED_REMOVAL_TEST_ROOT") else {
        return;
    };
    let paths = Paths::from_lookup(|key| (key == "HOME").then(|| root.clone())).unwrap();
    let _held = Lock::new(&paths, Kind::Runner, "abandoned-build-1.0")
        .try_exclusive()
        .unwrap()
        .unwrap();
    let staged = move_out(&paths.default_runner_root().join("abandoned-build-1.0")).unwrap();
    fs::write(Path::new(&root).join("staged"), staged.to_str().unwrap()).unwrap();
    let mut bytes = [0];
    std::io::stdin().read_exact(&mut bytes).unwrap();
    fs::remove_dir_all(staged).unwrap();
}

#[test]
fn later_removal_sweeps_staging_after_a_process_crashes() {
    let f = Fixture::new();
    let abandoned = f.runner("abandoned-build-1.0");
    let later = f.runner("later");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "build_removal::tests::staged_removal_child"])
        .env("WINEO_STAGED_REMOVAL_TEST_ROOT", f.root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !f.root.path().join("staged").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child did not stage removal"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let staged = PathBuf::from(fs::read_to_string(f.root.path().join("staged")).unwrap());
    assert!(!abandoned.exists());
    assert!(staged.is_dir());
    child.kill().unwrap();
    child.wait().unwrap();
    crate::runners::remove(&f.paths, "later").unwrap();
    assert!(!staged.exists());
    assert!(!later.exists());
}

#[test]
fn later_component_removal_sweeps_abandoned_staging() {
    for component in Component::ALL {
        let f = Fixture::new();
        f.template("gaming", "");
        let abandoned = f.component(component, "old-build-1.0");
        let staged = move_out(&abandoned).unwrap();
        let later = f.component(component, "2.0");
        crate::components::remove(&f.paths, component, "2.0").unwrap();
        assert!(!staged.exists());
        assert!(!later.exists());
    }
}

#[test]
fn sweep_skips_staging_with_a_held_build_lock_without_waiting() {
    let f = Fixture::new();
    let dir = f.runner("held-build-1.0");
    let lock = Lock::new(&f.paths, Kind::Runner, "held-build-1.0");
    let held = lock.try_exclusive().unwrap().unwrap();
    let staged = move_out(&dir).unwrap();
    let started = std::time::Instant::now();
    sweep(&f.paths, Kind::Runner, dir.parent().unwrap()).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(staged.join("build/bin/wine").exists());
    drop(held);
    sweep(&f.paths, Kind::Runner, dir.parent().unwrap()).unwrap();
    assert!(!staged.exists());
}

#[test]
fn sweep_failure_does_not_fail_removal_or_skip_other_staging() {
    let f = Fixture::new();
    let abandoned = f.runner("abandoned");
    let staged = move_out(&abandoned).unwrap();
    let lock = Lock::new(&f.paths, Kind::Runner, "abandoned");
    drop(lock.try_exclusive().unwrap().unwrap());
    let lock_path = fs::read_dir(f.paths.build_locks_dir())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_file(&lock_path).unwrap();
    fs::create_dir(lock_path).unwrap();
    let cleanable = move_out(&f.runner("cleanable")).unwrap();
    assert!(sweep(&f.paths, Kind::Runner, abandoned.parent().unwrap()).is_err());
    assert!(!cleanable.exists());
    assert!(staged.is_dir());
    let later = f.runner("later");
    crate::runners::remove(&f.paths, "later").unwrap();
    assert!(!later.exists());
    assert!(staged.is_dir());
}

#[test]
fn sweep_ignores_symlinks_and_unrecognized_staging_names() {
    let f = Fixture::new();
    let dir = f.runner("outside");
    let root = dir.parent().unwrap();
    let link = root.join(".wineo-remove-outside-ABC123");
    symlink(&dir, &link).unwrap();
    let unknown = root.join(".wineo-remove-legacy");
    fs::create_dir(&unknown).unwrap();
    sweep(&f.paths, Kind::Runner, root).unwrap();
    assert!(link.is_symlink());
    assert!(dir.join("bin/wine").exists());
    assert!(unknown.is_dir());
}

#[test]
fn batch_uses_matches_single_build_checks_in_a_mixed_fixture() {
    let f = Fixture::new();
    f.runner("plain");
    f.runner("unused");
    f.proton("GE-Proton9-19");
    f.proton("GE-Proton9-20");
    f.proton("proton-cachyos-10.0-20260101-slr");
    f.proton("proton-cachyos-11.0-20260201-slr");
    for component in Component::ALL {
        f.component(component, "1.0");
        f.component(component, "2.0");
    }
    f.global("[defaults]\nrunner = 'GE-Proton-Latest'\n");
    f.environment(
        "alias",
        "runner = 'GE-Proton-Latest'\ndxvk = 'latest'\nvkd3d = 'latest'",
    );
    f.environment(
        "other-alias",
        "runner = 'Proton-CachyOS-Latest'\ndxvk = '1.0'",
    );
    f.environment("direct", "runner = 'plain'\ndxvk = '2.0'\nvkd3d = '1.0'");
    f.environment("broken", "runner = [invalid");
    f.template("broken", "not valid TOML");
    f.template("direct", "runner = 'GE-Proton9-19'\ndxvk = '1.0'\n");
    f.template("alias", "runner = 'GE-Proton-Latest'\nvkd3d = 'latest'\n");
    f.template("proton", "runner = 'proton'\n");

    let runner = Runners::new(&f.paths, &GlobalConfig::default())
        .unwrap()
        .resolve("plain")
        .unwrap();
    let runtime = Runtime::new(&f.paths, "active");
    let _held = runtime.try_lock().unwrap().unwrap();
    let mut state = RunState::running(
        RunMode::Detached,
        "app".to_owned(),
        std::process::id(),
        None,
        &Overrides::default(),
    );
    state.runner = Some(runner.clone());
    runtime.write_state(&state).unwrap();
    Runtime::new(&f.paths, "lost").write_state(&state).unwrap();
    Runtime::new(&f.paths, "removed")
        .write_operation(&OperationState {
            operation: Operation::Install,
            program: "setup.exe".to_owned(),
            runner,
            prefix: f.root.path().join("prefix"),
            pid: None,
            group: None,
            boot_id: None,
            leader_start: None,
            started_at: jiff::Timestamp::now(),
        })
        .unwrap();

    let batch = uses_all(&f.paths).unwrap();
    assert_eq!(batch.len(), 10);
    for ((kind, name), actual) in &batch {
        let expected = f.uses(*kind, name);
        assert_eq!(actual.labels, expected.labels, "{kind:?} / {name}");
        assert_eq!(
            actual
                .errors
                .iter()
                .map(|e| (&e.name, &e.message))
                .collect::<Vec<_>>(),
            expected
                .errors
                .iter()
                .map(|e| (&e.name, &e.message))
                .collect::<Vec<_>>(),
            "{kind:?} / {name}",
        );
        assert_eq!(actual.errors.len(), 2);
    }
    assert_eq!(
        batch[&(Kind::Runner, "plain".to_owned())].labels,
        [
            "active run active",
            "active run lost",
            "direct",
            "installer removed"
        ]
    );
    assert!(
        batch[&(Kind::Runner, "unused".to_owned())]
            .labels
            .is_empty()
    );
    assert_eq!(
        batch[&(Kind::Dxvk, "2.0".to_owned())].labels,
        ["alias (latest)", "direct", "template gaming (latest)"]
    );
}
