use crate::test_script::write_script;
use std::ffi::OsString;
use std::fs;

use std::path::Path;

use super::*;
use crate::test_snapshot::{assert_unchanged, snapshot_tree};

fn paths(root: &Path) -> Paths {
    let home = root.join("home");
    Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(&home))).unwrap()
}

fn fake_runner(paths: &Paths, name: &str) {
    let bin = GlobalConfig::default()
        .runner_root(paths)
        .unwrap()
        .join(name)
        .join("bin");
    fs::create_dir_all(&bin).unwrap();
    for program in ["wine", "wineserver"] {
        let path = bin.join(program);
        write_script(&path, "#!/bin/sh\nexit 0\n");
    }
}

fn request(name: &str) -> CreateRequest {
    CreateRequest {
        name: name.to_owned(),
        runner: Some("fake".to_owned()),
        ..Default::default()
    }
}

fn build_registry(event: CreateEvent, events: &mut Vec<&'static str>) {
    match event {
        CreateEvent::Warning(_) => events.push("warning"),
        CreateEvent::BuildingPrefix { dir, .. } => {
            events.push("building");
            fs::write(dir.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        }
        CreateEvent::Saved => events.push("saved"),
        CreateEvent::Registered { .. } => events.push("registered"),
        CreateEvent::Components(_) => events.push("components"),
        CreateEvent::Desktop(_) => events.push("desktop"),
        CreateEvent::TemplateComponentsOmitted { .. } => events.push("omitted"),
    }
}

fn prefix(dir: &Path, arch: Arch) {
    fs::create_dir_all(dir.join("drive_c")).unwrap();
    fs::write(
        dir.join("system.reg"),
        format!("WINE REGISTRY Version 2\n#arch={}\n", arch.as_str()),
    )
    .unwrap();
}

fn fake_component(paths: &Paths, component: crate::components::Component, name: &str) {
    let builds =
        crate::components::Builds::new(paths, &GlobalConfig::default(), component).unwrap();
    let marker = match component {
        crate::components::Component::Dxvk => "dxgi.dll",
        crate::components::Component::Vkd3d => "d3d12.dll",
    };
    let (wide, narrow) = component.arch_dirs();
    for arch in [wide, narrow] {
        let dir = builds.root().join(name).join(arch);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(marker), "dll").unwrap();
    }
}

#[test]
fn planning_new_prefix_writes_nothing_and_reports_target() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let exe = root.path().join("game.exe");
    fs::write(&exe, "").unwrap();
    let mut request = request("game");
    request.exe = Some(exe.display().to_string());
    request.target_name = Some("Game Title".into());
    request.desktop = true;
    let before = snapshot_tree(root.path());
    let plan = plan(&paths, request).unwrap();
    assert_unchanged(&before, &snapshot_tree(root.path()));
    assert_eq!(plan.name(), "game");
    assert!(!plan.adopted());
    assert!(!plan.prefix().exists());
    assert_eq!(plan.runner().selection(), "fake");
    assert_eq!(plan.runner().name(), "fake");
    assert_eq!(plan.arch(), Arch::Win64);
    assert_eq!(plan.exe_check(), Some(ExeCheck::Found));
    assert_eq!(plan.exe(), Some(exe.to_str().unwrap()));
    assert_eq!(plan.target_name(), "Game Title");
    assert!(plan.desktop());
}

#[test]
fn broken_library_does_not_block_wine_creation() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let library = paths.library_file();
    fs::create_dir_all(library.parent().unwrap()).unwrap();
    let broken = b"[broken\n";
    fs::write(&library, broken).unwrap();
    let mut warnings = Vec::new();
    let created = apply(
        &paths,
        plan(&paths, request("game")).unwrap(),
        &Token::new(),
        |event| match event {
            CreateEvent::Warning(warning) => warnings.push(warning),
            CreateEvent::BuildingPrefix { dir, .. } => {
                fs::write(dir.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
            }
            _ => {}
        },
    )
    .unwrap();
    assert!(paths.environment_config(&created.environment.name).exists());
    assert!(warnings.iter().any(|warning| {
        warning.starts_with("Can't update library.toml: ") && warning.contains("it can't be read")
    }));
    assert_eq!(fs::read(&library).unwrap(), broken);
}

#[test]
fn planning_adoption_writes_nothing_and_checks_windows_exe() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let dir = root.path().join("prefix");
    prefix(&dir, Arch::Win32);
    fs::write(dir.join("drive_c/game.exe"), "").unwrap();
    fs::create_dir_all(dir.join("dosdevices")).unwrap();
    std::os::unix::fs::symlink("../drive_c", dir.join("dosdevices/c:")).unwrap();
    let mut request = request("game");
    request.dir = Some(dir.display().to_string());
    request.exe = Some(r"C:\game.exe".into());
    let before = snapshot_tree(root.path());
    let plan = plan(&paths, request).unwrap();
    assert_unchanged(&before, &snapshot_tree(root.path()));
    assert!(plan.adopted());
    assert_eq!(plan.prefix(), dir);
    assert_eq!(plan.arch(), Arch::Win32);
    assert_eq!(plan.exe_check(), Some(ExeCheck::Found));
    assert_eq!(plan.target_name(), "game");
}

#[test]
fn planning_template_components_writes_nothing_and_resolves_builds() {
    use crate::components::Component;

    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    fake_component(&paths, Component::Dxvk, "2.6");
    fake_component(&paths, Component::Vkd3d, "2.14");
    fs::create_dir_all(paths.templates_dir()).unwrap();
    fs::write(
        paths.templates_dir().join("custom.toml"),
        "runner = 'fake'\ndxvk = 'latest'\nvkd3d = '2.14'\n",
    )
    .unwrap();
    let request = CreateRequest {
        name: "game".into(),
        template_name: Some("custom".into()),
        ..Default::default()
    };
    let before = snapshot_tree(root.path());
    let plan = plan(&paths, request).unwrap();
    assert_unchanged(&before, &snapshot_tree(root.path()));
    assert_eq!(plan.template_name(), Some("custom"));
    assert_eq!(plan.wanted().dxvk.as_deref(), Some("latest"));
    assert_eq!(plan.resolved_components().dxvk.as_deref(), Some("2.6"));
    assert_eq!(plan.resolved_components().vkd3d.as_deref(), Some("2.14"));
}

#[test]
fn planning_missing_component_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let mut request = request("game");
    request.template = Some(Template {
        dxvk: Some("missing".into()),
        ..Default::default()
    });
    let before = snapshot_tree(root.path());
    let error = plan(&paths, request).unwrap_err();
    assert_unchanged(&before, &snapshot_tree(root.path()));
    assert!(matches!(
        error.downcast_ref::<Error>(),
        Some(Error::UnknownBuild { .. })
    ));
}

#[test]
fn planning_missing_exe_reports_it_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let mut request = request("game");
    request.exe = Some(root.path().join("missing.exe").display().to_string());
    let before = snapshot_tree(root.path());
    let plan = plan(&paths, request).unwrap();
    assert_unchanged(&before, &snapshot_tree(root.path()));
    assert_eq!(plan.exe_check(), Some(ExeCheck::Missing));
}

#[test]
fn planning_unadopted_windows_exe_is_unchecked_but_adopted_is_found() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let exe = root.path().join("home/me/setup.exe");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    fs::write(&exe, "").unwrap();
    let windows_exe = format!("Z:{}", exe.display().to_string().replace('/', "\\"));

    let mut fresh_request = request("fresh");
    fresh_request.exe = Some(windows_exe.clone());
    let fresh = plan(&paths, fresh_request).unwrap();
    assert!(!fresh.adopted());
    assert_eq!(fresh.exe_check(), Some(ExeCheck::Unchecked));

    let dir = root.path().join("adopted");
    prefix(&dir, Arch::Win64);
    fs::create_dir_all(dir.join("dosdevices")).unwrap();
    std::os::unix::fs::symlink("/", dir.join("dosdevices/z:")).unwrap();
    let mut adopted_request = request("adopted-game");
    adopted_request.dir = Some(dir.display().to_string());
    adopted_request.exe = Some(windows_exe);
    let adopted = plan(&paths, adopted_request).unwrap();
    assert!(adopted.adopted());
    assert_eq!(adopted.exe_check(), Some(ExeCheck::Found));
}

#[test]
fn planning_does_not_take_runner_or_create_locks() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let build_lock = crate::build_lock::Lock::new(&paths, crate::build_lock::Kind::Runner, "fake");
    let _build_guard = build_lock.try_exclusive().unwrap().unwrap();
    let create_guard = Locks::new(&paths)
        .acquire(Operation::Create, "game", &[])
        .unwrap();

    let plan = plan(&paths, request("game")).unwrap();
    assert_eq!(plan.runner().name(), "fake");
    assert_eq!(plan.exe_check(), None);
    assert!(build_lock.try_exclusive().unwrap().is_none());
    assert!(matches!(
        Locks::new(&paths).acquire(Operation::Create, "game", &[]),
        Err(Error::Busy { .. })
    ));
    drop(create_guard);
}

#[test]
fn applying_plan_reports_removed_non_alias_runner_as_unknown() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let runner_dir = GlobalConfig::default()
        .runner_root(&paths)
        .unwrap()
        .join("fake");
    fake_runner(&paths, "fake");
    let plan = plan(&paths, request("game")).unwrap();
    fs::remove_dir_all(runner_dir).unwrap();

    let error = apply(&paths, plan, &Token::new(), |_| {
        panic!("no writes expected")
    })
    .err()
    .unwrap();
    assert!(matches!(
        error.downcast_ref::<Error>(),
        Some(Error::UnknownRunner { name, .. }) if name == "fake"
    ));
    assert!(!paths.environment_config("game").exists());
}

#[test]
fn applying_plan_refuses_changed_adoption() {
    for adopted in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_runner(&paths, "fake");
        let dir = root.path().join("prefix");
        if adopted {
            prefix(&dir, Arch::Win64);
        }
        let mut request = request("game");
        request.dir = Some(dir.display().to_string());
        let plan = plan(&paths, request).unwrap();
        if adopted {
            fs::remove_dir_all(&dir).unwrap();
        } else {
            prefix(&dir, Arch::Win64);
        }
        let error = apply(&paths, plan, &Token::new(), |_| {
            panic!("no writes expected")
        })
        .err()
        .unwrap();
        assert!(error.to_string().contains("prefix changed since review"));
        assert!(!paths.environment_config("game").exists());
    }
}

fn fake_proton(paths: &Paths, name: &str) -> PathBuf {
    let dir = GlobalConfig::default()
        .runner_root(paths)
        .unwrap()
        .join(name);
    fs::create_dir_all(&dir).unwrap();
    let script = dir.join("proton");
    write_script(&script, "#!/bin/sh\nexit 0\n");
    let bin = dir.join("files/bin");
    fs::create_dir_all(&bin).unwrap();
    for program in ["wine", "wineserver"] {
        let file = bin.join(program);
        write_script(&file, "#!/bin/sh\nexit 0\n");
    }
    dir
}

#[test]
fn planning_proton_omits_components_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_proton(&paths, "GE-Proton11-7");
    let before = snapshot_tree(root.path());
    let plan = plan(
        &paths,
        CreateRequest {
            name: "game".into(),
            template_name: Some("gaming".into()),
            expected_adopted: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert_unchanged(&before, &snapshot_tree(root.path()));
    assert_eq!(plan.runner().name(), "GE-Proton11-7");
    assert!(plan.template_components_omitted());
    assert_eq!(plan.wanted(), &Wanted::default());
    assert_eq!(plan.resolved_components(), &Wanted::default());
    let mut events = Vec::new();
    let error = apply(&paths, plan, &Token::new(), |event| {
        build_registry(event, &mut events)
    })
    .err()
    .unwrap();
    assert!(error.to_string().contains("prefix changed since review"));
    assert_eq!(events, ["omitted"]);
}

#[test]
fn applying_plan_refuses_alias_fallback_to_another_runner() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let selected = fake_proton(&paths, "GE-Proton11-7");
    let mut request = request("game");
    request.runner = Some("GE-Proton-Latest".into());
    let before = snapshot_tree(root.path());
    let plan = plan(&paths, request).unwrap();
    assert_unchanged(&before, &snapshot_tree(root.path()));
    assert_eq!(plan.runner().selection(), "GE-Proton-Latest");
    assert_eq!(plan.runner().name(), "GE-Proton11-7");
    fs::remove_dir_all(selected).unwrap();
    fake_proton(&paths, "GE-Proton12-1");
    let error = apply(&paths, plan, &Token::new(), |_| {
        panic!("no writes expected")
    })
    .err()
    .unwrap();
    assert!(error.to_string().contains("runner changed since review"));
    assert!(!paths.environment_config("game").exists());
}

#[test]
fn planning_refuses_invalid_and_taken_names_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(paths.environment_config("taken"), "invalid config").unwrap();
    let before = snapshot_tree(root.path());
    assert!(matches!(
        plan(&paths, request("bad name"))
            .unwrap_err()
            .downcast_ref::<Error>(),
        Some(Error::InvalidName(_))
    ));
    assert!(matches!(
        plan(&paths, request("taken"))
            .unwrap_err()
            .downcast_ref::<Error>(),
        Some(Error::EnvironmentExists(_))
    ));
    assert_unchanged(&before, &snapshot_tree(root.path()));
}

#[test]
fn native_planning_writes_nothing_and_resolves_exe() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let bin = root.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("game");
    write_script(&exe, "#!/bin/sh\nexit 0\n");
    let mut global = GlobalConfig::default();
    global.env.insert("PATH".into(), bin.display().to_string());
    let before = snapshot_tree(root.path());
    let plan = Environment::plan_native(&paths, &global, "game", Some("game")).unwrap();
    assert_eq!(plan.name, "game");
    assert_eq!(plan.exe.as_deref(), Some("game"));
    assert_eq!(plan.resolved_exe, Some(exe));
    assert!(Environment::plan_native(&paths, &global, "game", Some("missing")).is_err());
    assert!(Environment::plan_native(&paths, &global, "bad name", None).is_err());
    assert_unchanged(&before, &snapshot_tree(root.path()));
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(paths.environment_config("taken"), "kind = 'native'\n").unwrap();
    let before = snapshot_tree(root.path());
    assert!(matches!(
        Environment::plan_native(&paths, &global, "taken", None),
        Err(Error::EnvironmentExists(_))
    ));
    assert_unchanged(&before, &snapshot_tree(root.path()));
}

#[test]
fn reviewed_creation_refuses_changed_adoption_before_writing() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    for present in [false, true] {
        let prefix = root.path().join(format!("prefix-{present}"));
        if present {
            fs::create_dir_all(prefix.join("drive_c")).unwrap();
            fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        }
        let mut request = request("reviewed");
        request.dir = Some(prefix.display().to_string());
        request.expected_adopted = Some(!present);
        let result = run(&paths, request, &Token::new(), |_| {
            panic!("no mutation expected")
        });
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("prefix changed since review")
        );
        assert!(!paths.environment_config("reviewed").exists());
    }
}

#[test]
fn creates_prefix_target_and_retains_create_lock() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let exe = root.path().join("game.exe");
    fs::write(&exe, "").unwrap();
    let mut request = request("game");
    request.exe = Some(exe.display().to_string());
    let mut events = Vec::new();
    let created = run(&paths, request, &Token::new(), |event| {
        build_registry(event, &mut events)
    })
    .unwrap();
    assert_eq!(events, ["building", "saved", "registered"]);
    assert!(created.created_prefix);
    assert_eq!(created.target, TargetOutcome::Added);
    assert_eq!(created.environment.config.targets.len(), 1);
    assert!(paths.runtime_dir("game").join("create.log").exists());
    assert!(matches!(
        Locks::new(&paths).acquire(Operation::Create, "game", &[&created.prefix]),
        Err(Error::Busy { .. })
    ));
    drop(created);
    assert!(
        Locks::new(&paths)
            .acquire(Operation::Create, "game", &[])
            .is_ok()
    );
}

#[test]
fn target_name_titles_the_default_target() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let exe = root.path().join("game.exe");
    fs::write(&exe, "").unwrap();
    let mut request = request("the-sims-4");
    request.exe = Some(exe.display().to_string());
    request.target_name = Some("The Sims 4".to_owned());
    let created = run(&paths, request, &Token::new(), |event| {
        build_registry(event, &mut Vec::new())
    })
    .unwrap();
    let target = &created.environment.config.targets["the-sims-4"];
    assert_eq!(target.name.as_deref(), Some("The Sims 4"));
}

#[test]
fn missing_exe_keeps_environment_without_a_target() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let mut request = request("game");
    request.exe = Some(root.path().join("missing.exe").display().to_string());
    let created = run(&paths, request, &Token::new(), |event| {
        build_registry(event, &mut Vec::new())
    })
    .unwrap();
    assert_eq!(created.target, TargetOutcome::ExeNotFound);
    assert!(created.environment.config.targets.is_empty());
    assert!(paths.environment_config("game").exists());
}

#[test]
fn template_selects_runner_and_checks_components_before_creation() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let templates = paths.templates_dir();
    fs::create_dir_all(&templates).unwrap();
    fs::write(
        templates.join("custom.toml"),
        "runner = \"fake\"\ntags = [\"from-template\"]\n",
    )
    .unwrap();
    let request = CreateRequest {
        name: "game".to_owned(),
        template_name: Some("custom".to_owned()),
        ..Default::default()
    };
    let created = run(&paths, request, &Token::new(), |event| {
        build_registry(event, &mut Vec::new())
    })
    .unwrap();
    assert_eq!(created.runner.name(), "fake");
    assert_eq!(created.environment.wine().unwrap().runner, "fake");
    assert_eq!(created.environment.config.tags, ["from-template"]);

    fs::write(
        templates.join("missing-build.toml"),
        "runner = \"fake\"\ndxvk = \"missing\"\n",
    )
    .unwrap();
    let request = CreateRequest {
        name: "other".to_owned(),
        template_name: Some("missing-build".to_owned()),
        ..Default::default()
    };
    let error = run(&paths, request, &Token::new(), |_| {}).err().unwrap();
    assert!(matches!(
        error.downcast_ref::<Error>(),
        Some(Error::UnknownBuild { .. })
    ));
    assert!(!paths.environment_config("other").exists());
    assert!(
        !GlobalConfig::default()
            .prefix_root(&paths)
            .unwrap()
            .join("other")
            .exists()
    );
}

#[test]
fn proton_win32_is_typed_and_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let proton = GlobalConfig::default()
        .runner_root(&paths)
        .unwrap()
        .join("proton-test");
    fs::create_dir_all(&proton).unwrap();
    let script = proton.join("proton");
    write_script(&script, "#!/bin/sh\nexit 0\n");
    let mut request = request("game");
    request.runner = Some("proton-test".to_owned());
    request.arch = Some(Arch::Win32);
    let error = run(&paths, request, &Token::new(), |_| {}).err().unwrap();
    assert!(matches!(
        error.downcast_ref::<Error>(),
        Some(Error::ProtonWin32(_))
    ));
    assert!(!paths.environment_config("game").exists());
}

#[test]
fn canceled_prefix_removes_partial_state() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    let token = Token::new();
    let result = run(&paths, request("game"), &token, |event| {
        if matches!(event, CreateEvent::BuildingPrefix { .. }) {
            token.cancel();
        }
    });
    assert!(matches!(
        result.err().unwrap().downcast_ref::<Error>(),
        Some(Error::Canceled(_))
    ));
    assert!(!paths.environment_config("game").exists());
    assert!(
        !GlobalConfig::default()
            .prefix_root(&paths)
            .unwrap()
            .join("game")
            .exists()
    );
}

#[test]
fn creating_from_a_template_carries_hooks() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    fake_runner(&paths, "fake");
    fs::create_dir_all(paths.templates_dir()).unwrap();
    fs::write(
        paths.templates_dir().join("hooks.toml"),
        "runner = 'fake'\n[hooks]\nbefore = 'mount /mnt/games'\nafter = 'sync'\n",
    )
    .unwrap();
    let created = run(
        &paths,
        CreateRequest {
            name: "game".to_owned(),
            template_name: Some("hooks".to_owned()),
            ..Default::default()
        },
        &Token::new(),
        |event| build_registry(event, &mut Vec::new()),
    )
    .unwrap();
    let hooks = created
        .environment
        .config
        .hooks
        .as_ref()
        .unwrap()
        .on()
        .unwrap();
    assert_eq!(hooks.before.as_deref(), Some("mount /mnt/games"));
    assert_eq!(hooks.after.as_deref(), Some("sync"));
}

/// Records what `create_and_install` reports, and cancels `token` once
/// `cancel_at` is reported.
struct Recorder {
    events: Vec<&'static str>,
    token: Token,
    cancel_at: &'static str,
    /// A config file and a line to add to it once the environment exists.
    edit: Option<(PathBuf, &'static str)>,
}

impl Recorder {
    fn new(token: &Token, cancel_at: &'static str) -> Self {
        Self {
            events: Vec::new(),
            token: token.clone(),
            cancel_at,
            edit: None,
        }
    }

    fn record(&mut self, event: &'static str) {
        self.events.push(event);
        if event == self.cancel_at {
            self.token.cancel();
        }
    }
}

impl Hooks for Recorder {
    type Stop = ();

    fn event(&mut self, event: CreateEvent) {
        self.record(match event {
            CreateEvent::Warning(_) => "warning",
            CreateEvent::BuildingPrefix { .. } => "building",
            CreateEvent::Saved => "saved",
            CreateEvent::Registered { .. } => "registered",
            CreateEvent::Components(_) => "components",
            CreateEvent::Desktop(_) => "desktop",
            CreateEvent::TemplateComponentsOmitted { .. } => "omitted",
        });
    }

    fn created(&mut self, _created: &Created) -> Result<(), ()> {
        if let Some((config, line)) = &self.edit {
            let text = fs::read_to_string(config).unwrap();
            fs::write(config, format!("{text}{line}\n")).unwrap();
        }
        self.record("created");
        Ok(())
    }

    fn ready(&mut self, _launch: &Launch) -> Result<(), ()> {
        self.record("ready");
        Ok(())
    }

    fn watched(&mut self, watched: Watched) {
        if matches!(watched, Watched::Exited(_)) {
            self.record("exited");
        }
    }
}

/// A runner whose `wineboot --init` builds a prefix and whose run of
/// `setup.exe` does `installer`.
fn installing_runner(paths: &Paths, installer: &str) {
    fake_runner(paths, "fake");
    let wine = GlobalConfig::default()
        .runner_root(paths)
        .unwrap()
        .join("fake/bin/wine");
    write_script(
        &wine,
        format!(
            "#!/bin/sh\n\
                 case \"$*\" in\n\
                 'wineboot --init') echo 'WINE REGISTRY Version 2' > \"$WINEPREFIX/system.reg\" ;;\n\
                 'wineboot --end-session') touch \"$WINEPREFIX/stopped\" ;;\n\
                 *setup.exe*) {installer} ;;\n\
                 esac\n"
        ),
    );
}

fn create_and_install_with(
    root: &Path,
    installer: &str,
    token: &Token,
    cancel_at: &'static str,
) -> (Paths, anyhow::Result<Installed<()>>, Vec<&'static str>) {
    let paths = paths(root);
    installing_runner(&paths, installer);
    let setup = root.join("setup.exe");
    fs::write(&setup, "").unwrap();
    let mut recorder = Recorder::new(token, cancel_at);
    let log = root.join("install.log");
    let step = Step::Installer {
        program: setup.to_str().unwrap(),
        output: Output::Log(&log),
        token,
    };
    let result = create_and_install(&paths, request("game"), Some(step), token, &mut recorder);
    (paths, result, recorder.events)
}

fn prefix_dir(paths: &Paths) -> PathBuf {
    GlobalConfig::default()
        .prefix_root(paths)
        .unwrap()
        .join("game")
}

#[test]
fn create_and_install_runs_the_installer_under_the_create_lock() {
    let root = tempfile::tempdir().unwrap();
    let (paths, result, events) = create_and_install_with(
        root.path(),
        "touch \"$WINEPREFIX/installed\"",
        &Token::new(),
        "",
    );
    let installed = result.unwrap();
    assert_eq!(
        events,
        [
            "building",
            "saved",
            "registered",
            "created",
            "ready",
            "exited"
        ]
    );
    assert!(matches!(
        installed.step,
        Stepped::Ran {
            outcome: Ok(installer::Outcome::Finished { exit_code: Some(0) }),
            resolved: true,
        }
    ));
    assert!(prefix_dir(&paths).join("installed").exists());
    assert!(paths.environment_config("game").exists());
    assert!(
        Runtime::new(&paths, "game")
            .read_operation()
            .unwrap()
            .is_none()
    );
    // The create lock went to the installer, which has let go of it.
    assert!(
        Locks::new(&paths)
            .acquire(Operation::Create, "game", &[&installed.prefix])
            .is_ok()
    );
}

#[test]
fn create_and_install_keeps_the_environment_after_a_failed_installer() {
    let root = tempfile::tempdir().unwrap();
    let (paths, result, _) = create_and_install_with(root.path(), "exit 3", &Token::new(), "");
    let installed = result.unwrap();
    assert!(matches!(
        installed.step,
        Stepped::Ran {
            outcome: Ok(installer::Outcome::Finished { exit_code: Some(3) }),
            resolved: true,
        }
    ));
    assert!(paths.environment_config("game").exists());
    assert!(prefix_dir(&paths).join("system.reg").exists());
}

#[test]
fn create_and_install_canceled_while_building_leaves_nothing() {
    let root = tempfile::tempdir().unwrap();
    let (paths, result, events) = create_and_install_with(
        root.path(),
        "touch \"$WINEPREFIX/installed\"",
        &Token::new(),
        "building",
    );
    assert!(matches!(
        result.err().unwrap().downcast_ref::<Error>(),
        Some(Error::Canceled(_))
    ));
    assert_eq!(events, ["building"]);
    assert!(!paths.environment_config("game").exists());
    assert!(!prefix_dir(&paths).exists());
}

#[test]
fn create_and_install_canceled_during_the_installer_keeps_the_environment() {
    let root = tempfile::tempdir().unwrap();
    let installer = "touch \"$WINEPREFIX/started\"\n\
                         while [ ! -e \"$WINEPREFIX/stopped\" ]; do sleep 0.02; done";
    let started = prefix_dir(&paths(root.path())).join("started");
    let token = Token::new();
    let request = token.clone();
    let canceler = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !started.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        request.cancel();
    });
    let (paths, result, events) = create_and_install_with(root.path(), installer, &token, "");
    canceler.join().unwrap();
    let installed = result.unwrap();
    assert!(events.contains(&"ready"), "{events:?}");
    assert!(prefix_dir(&paths).join("started").exists());
    assert!(
        matches!(
            installed.step,
            Stepped::Ran {
                outcome: Ok(installer::Outcome::Canceled { .. }),
                resolved: true,
            }
        ),
        "{:?}",
        installed.step
    );
    assert!(paths.environment_config("game").exists());
    assert!(
        Runtime::new(&paths, "game")
            .read_operation()
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_gog_installer_step_gets_its_arguments_and_variables() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    installing_runner(
        &paths,
        "shift; printf '%s|' \"$@\" \"$LC_ALL\" > \"$WINEPREFIX/args\"; exit 1",
    );
    let setup = root.path().join("setup.exe");
    fs::write(&setup, "").unwrap();
    let token = Token::new();
    let mut recorder = Recorder::new(&token, "");
    let log = root.path().join("install.log");
    let args = installer::Args {
        args: vec!["/SILENT".to_owned(), "/DIR=Z:\\a b".to_owned()],
        env: [("LC_ALL".to_owned(), "fr_FR.UTF-8".to_owned())].into(),
    };
    let step = Step::GogInstaller {
        program: setup.to_str().unwrap(),
        args: &args,
        output: Output::Log(&log),
        token: &token,
    };
    let request = CreateRequest {
        gog_id: Some("1207658930".to_owned()),
        ..request("game")
    };
    let installed = create_and_install(&paths, request, Some(step), &token, &mut recorder).unwrap();
    assert!(matches!(
        installed.step,
        Stepped::Ran {
            outcome: Ok(installer::Outcome::Finished { exit_code: Some(1) }),
            resolved: true,
        }
    ));
    assert_eq!(
        fs::read_to_string(prefix_dir(&paths).join("args")).unwrap(),
        "/SILENT|/DIR=Z:\\a b|fr_FR.UTF-8|"
    );
    assert_eq!(
        installed.environment.config.gog_id.as_deref(),
        Some("1207658930")
    );
}

#[test]
fn create_and_install_sets_up_components_before_the_installer() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    installing_runner(&paths, "true");
    fake_component(&paths, crate::components::Component::Dxvk, "2.6");
    let setup = root.path().join("setup.exe");
    fs::write(&setup, "").unwrap();
    let token = Token::new();
    let mut recorder = Recorder::new(&token, "");
    // A change made after creation, like one made while art is picked,
    // is what the installer gets.
    recorder.edit = Some((paths.environment_config("game"), "dxvk = '2.6'"));
    let log = root.path().join("install.log");
    let step = Step::Installer {
        program: setup.to_str().unwrap(),
        output: Output::Log(&log),
        token: &token,
    };
    let installed =
        create_and_install(&paths, request("game"), Some(step), &token, &mut recorder).unwrap();
    assert!(
        matches!(installed.step, Stepped::Ran { outcome: Ok(_), .. }),
        "{:?}",
        installed.step
    );
    let created = recorder.events.iter().position(|event| *event == "created");
    let components = recorder
        .events
        .iter()
        .position(|event| *event == "components");
    let ready = recorder.events.iter().position(|event| *event == "ready");
    assert!(
        created < components && components < ready,
        "{:?}",
        recorder.events
    );
}

#[test]
fn after_install_offers_programs_once_wine_has_settled_whatever_the_exit_code() {
    use installer::Outcome;
    let unresolved = || Outcome::Unresolved {
        exit_code: Some(0),
        canceled: false,
        error: Error::Unsettled("still running".to_owned()),
    };
    for (outcome, resolved, next) in [
        (
            Ok(Outcome::Finished { exit_code: Some(0) }),
            true,
            AfterInstall::Programs,
        ),
        (
            Ok(Outcome::Finished { exit_code: Some(3) }),
            true,
            AfterInstall::Programs,
        ),
        (
            Ok(Outcome::Finished { exit_code: None }),
            true,
            AfterInstall::Programs,
        ),
        (
            Ok(Outcome::Finished { exit_code: Some(0) }),
            false,
            AfterInstall::Unresolved,
        ),
        (
            Ok(Outcome::Canceled { exit_code: None }),
            true,
            AfterInstall::Canceled,
        ),
        (
            Ok(Outcome::Canceled { exit_code: None }),
            false,
            AfterInstall::Unresolved,
        ),
        (Ok(unresolved()), false, AfterInstall::Unresolved),
        (
            Err(Error::Unsettled("no wine".to_owned())),
            true,
            AfterInstall::NotRun,
        ),
    ] {
        assert_eq!(after_install(&outcome, resolved), next, "{outcome:?}");
    }
}
