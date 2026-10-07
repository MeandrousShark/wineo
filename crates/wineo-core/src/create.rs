//! Creates a Wine environment and brings its prefix and desktop entries in
//! line with the new config.

use std::path::{Path, PathBuf};

use anyhow::bail;

use crate::build_lock;
use crate::cancel::Token;
use crate::config::{Arch, GlobalConfig};
use crate::desktop;
use crate::environment::{Environment, NewEnvironment};
use crate::error::Error;
use crate::install::{self, Wanted};
use crate::installer::Output;
use crate::operation::{Guard, Locks, Operation};
use crate::paths::Paths;
use crate::prefix::PrefixEvent;
use crate::runner::Runner;
use crate::runners::Runners;
use crate::targets::{self, Batch, Spec, Written};
use crate::templates::{Template, Templates};

#[derive(Debug, Default)]
pub struct CreateRequest {
    pub name: String,
    pub dir: Option<String>,
    pub runner: Option<String>,
    pub resolved_runner: Option<Runner>,
    pub arch: Option<Arch>,
    pub template: Option<Template>,
    pub template_name: Option<String>,
    pub exe: Option<String>,
    /// What the default target's desktop entry is called. None uses the
    /// environment name.
    pub target_name: Option<String>,
    pub desktop: bool,
    /// Refuse a changed create/adopt decision after a caller's review.
    pub expected_adopted: Option<bool>,
}

/// Resolved creation choices, without locks or filesystem changes.
#[derive(Debug)]
pub struct CreatePlan {
    name: String,
    prefix: PathBuf,
    adopted: bool,
    runner: Runner,
    template_name: Option<String>,
    arch: Arch,
    wanted: Wanted,
    /// Concrete build names for the fitted template's component selections.
    resolved_components: Wanted,
    exe: Option<String>,
    exe_check: Option<ExeCheck>,
    /// The default target's desktop entry title. Its target key is `name`.
    target_name: String,
    desktop: bool,
    template_components_omitted: bool,
    global: GlobalConfig,
    new: NewEnvironment,
    target_title: Option<String>,
    expected_adopted: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExeCheck {
    Found,
    Missing,
    Unchecked,
}

impl CreatePlan {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    pub fn adopted(&self) -> bool {
        self.adopted
    }

    pub fn runner(&self) -> &Runner {
        &self.runner
    }

    pub fn template_name(&self) -> Option<&str> {
        self.template_name.as_deref()
    }

    pub fn arch(&self) -> Arch {
        self.arch
    }

    pub fn wanted(&self) -> &Wanted {
        &self.wanted
    }

    pub fn resolved_components(&self) -> &Wanted {
        &self.resolved_components
    }

    pub fn exe(&self) -> Option<&str> {
        self.exe.as_deref()
    }

    pub fn exe_check(&self) -> Option<ExeCheck> {
        self.exe_check
    }

    pub fn target_name(&self) -> &str {
        &self.target_name
    }

    pub fn desktop(&self) -> bool {
        self.desktop
    }

    pub fn template_components_omitted(&self) -> bool {
        self.template_components_omitted
    }
}

#[derive(Debug)]
pub enum CreateEvent {
    TemplateComponentsOmitted {
        runner: String,
    },
    BuildingPrefix {
        dir: PathBuf,
        runner: Runner,
    },
    /// The config is written, so the environment exists from here on, even
    /// when it adopted a prefix instead of building one.
    Saved,
    Registered {
        name: String,
        stored_prefix: String,
        created_prefix: bool,
        target: TargetOutcome,
    },
    Components(Vec<install::Change>),
    Desktop(Vec<desktop::Change>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOutcome {
    None,
    Added,
    ExeNotFound,
}

/// The create lock stays held until this value is dropped. Pass `guard` to
/// `Installer::prepare_with_runner` to keep the prefix locked through install.
pub struct Created {
    pub global: GlobalConfig,
    pub environment: Environment,
    pub runner: Runner,
    pub prefix: PathBuf,
    pub created_prefix: bool,
    pub target: TargetOutcome,
    pub guard: Guard,
    _build_guard: Option<build_lock::Guard>,
}

pub fn check_arch(runner: &Runner, arch: Option<Arch>) -> crate::Result<()> {
    if runner.is_proton() && arch == Some(Arch::Win32) {
        return Err(Error::ProtonWin32(runner.name().to_owned()));
    }
    Ok(())
}

pub fn plan(paths: &Paths, request: CreateRequest) -> anyhow::Result<CreatePlan> {
    let global = GlobalConfig::load(paths)?;
    let template_name = request.template_name.clone();
    let template = match (request.template, request.template_name) {
        (Some(template), None) => Some(template),
        (None, Some(name)) => Some(Templates::new(paths).resolve(&name)?),
        (None, None) => None,
        (Some(_), Some(_)) => bail!("a template and template name cannot both be supplied"),
    };
    let arch = request
        .arch
        .or_else(|| template.as_ref().and_then(|template| template.arch));
    let runner = match request.resolved_runner {
        Some(runner) => runner,
        None => {
            let runners = Runners::new(paths, &global)?;
            let template_proton = request.runner.is_none()
                && template.as_ref().is_some_and(|template| {
                    template.runner.as_deref() == Some(crate::templates::NEWEST_PROTON)
                });
            let runner_name = match (&request.runner, &template) {
                (Some(runner), _) => runner.clone(),
                (None, Some(template)) => template
                    .runner_name(&runners, &global.defaults.runner)?
                    .unwrap_or_else(|| global.defaults.runner.clone()),
                (None, None) => global.defaults.runner.clone(),
            };
            match runners.resolve(&runner_name) {
                Ok(resolved) => resolved,
                Err(err @ (Error::UnknownRunner { .. } | Error::NotARunner { .. }))
                    if template_proton =>
                {
                    let Some(retry) = runners.newest_proton()? else {
                        return Err(err.into());
                    };
                    runners.resolve(retry.name())?
                }
                Err(err) => return Err(err.into()),
            }
        }
    };
    check_arch(&runner, arch)?;
    let (template, template_components_omitted) = match template {
        Some(template) => {
            let (fitted, omitted) = template.for_runner(&runner);
            (Some(fitted), omitted)
        }
        None => (None, false),
    };
    let wanted = Wanted {
        dxvk: template.as_ref().and_then(|template| template.dxvk.clone()),
        vkd3d: template
            .as_ref()
            .and_then(|template| template.vkd3d.clone()),
    };
    let resolved_components =
        install::resolve_available(paths, &global, &request.name, &runner, &wanted)?;
    let new = NewEnvironment {
        name: request.name,
        dir: request.dir,
        arch,
        template,
    };
    let prefix = new.prefix_dir(paths, &global)?;
    crate::environment::check_new_name(paths, &new.name)?;
    let wine_prefix = runner.wine_prefix(&prefix);
    let adopted = crate::prefix::is_prefix(&wine_prefix);
    let arch = if adopted {
        crate::environment::adopted_arch(
            &wine_prefix,
            new.arch.unwrap_or(global.defaults.arch),
            new.arch,
        )?
    } else {
        new.arch.unwrap_or(global.defaults.arch)
    };
    let exe_check = request.exe.as_deref().map(|exe| {
        if !adopted && crate::winpath::is_windows_path(exe) {
            ExeCheck::Unchecked
        } else if desktop::check(paths, &prefix, exe).is_ok() {
            ExeCheck::Found
        } else {
            ExeCheck::Missing
        }
    });
    Ok(CreatePlan {
        name: new.name.clone(),
        prefix,
        adopted,
        runner,
        template_name,
        arch,
        wanted,
        resolved_components,
        exe: request.exe,
        exe_check,
        target_name: request
            .target_name
            .clone()
            .unwrap_or_else(|| new.name.clone()),
        desktop: request.desktop,
        template_components_omitted,
        global,
        new,
        target_title: request.target_name,
        expected_adopted: request.expected_adopted,
    })
}

pub fn run(
    paths: &Paths,
    request: CreateRequest,
    token: &Token,
    events: impl FnMut(CreateEvent),
) -> anyhow::Result<Created> {
    apply(paths, plan(paths, request)?, token, events)
}

pub fn apply(
    paths: &Paths,
    plan: CreatePlan,
    token: &Token,
    mut events: impl FnMut(CreateEvent),
) -> anyhow::Result<Created> {
    let CreatePlan {
        global,
        new,
        prefix,
        runner: planned_runner,
        adopted,
        exe,
        target_title,
        desktop,
        template_components_omitted,
        expected_adopted,
        ..
    } = plan;
    let (runner, build_guard) = Runners::new(paths, &global)?.lock_resolved(
        paths,
        planned_runner.selection(),
        planned_runner.clone(),
    )?;
    if runner != planned_runner
        || runner.directory() != planned_runner.directory()
        || runner.source() != planned_runner.source()
    {
        bail!("runner changed since review; review the creation choices again");
    }
    if template_components_omitted {
        events(CreateEvent::TemplateComponentsOmitted {
            runner: runner.name().to_owned(),
        });
    }
    if let Some(template) = &new.template {
        install::check_available(
            paths,
            &global,
            &new.name,
            &runner,
            &Wanted {
                dxvk: template.dxvk.clone(),
                vkd3d: template.vkd3d.clone(),
            },
        )?;
    }
    let guard = Locks::new(paths).acquire(Operation::Create, &new.name, &[&prefix])?;
    let current_adopted = crate::prefix::is_prefix(&runner.wine_prefix(&prefix));
    if current_adopted != adopted
        || expected_adopted.is_some_and(|expected| current_adopted != expected)
    {
        bail!("prefix changed since review; review the creation choices again");
    }
    let log = paths.runtime_dir(&new.name).join("create.log");
    let mut created_prefix = false;
    let environment = Environment::create(
        paths,
        &global,
        &runner,
        &new,
        token,
        Output::Log(&log),
        |event| {
            if let PrefixEvent::Started { dir } = event {
                created_prefix = true;
                events(CreateEvent::BuildingPrefix {
                    dir,
                    runner: runner.clone(),
                });
            }
        },
    )?;
    events(CreateEvent::Saved);
    let target = match exe {
        Some(exe) if add_default_target(paths, &environment, &exe, target_title, desktop)? => {
            TargetOutcome::Added
        }
        Some(_) => TargetOutcome::ExeNotFound,
        None => TargetOutcome::None,
    };
    events(CreateEvent::Registered {
        name: environment.name.clone(),
        stored_prefix: environment.wine()?.prefix.clone(),
        created_prefix,
        target,
    });
    if target == TargetOutcome::ExeNotFound {
        return Ok(Created {
            global,
            environment,
            runner,
            prefix,
            created_prefix,
            target,
            guard,
            _build_guard: build_guard,
        });
    }
    let environment = if target == TargetOutcome::Added {
        Environment::load(paths, &environment.name)?
    } else {
        environment
    };
    let wanted = Wanted::from_config(&environment.config);
    let changes = install::reconcile(
        paths,
        &global,
        &environment.name,
        &runner,
        &environment.prefix(paths)?,
        &wanted,
    )?;
    if !changes.is_empty() {
        events(CreateEvent::Components(changes));
    }
    let changes = desktop::reconcile(paths, &global, &environment, Some(&runner), &prefix)?;
    if !changes.is_empty() {
        events(CreateEvent::Desktop(changes));
    }
    Ok(Created {
        global,
        environment,
        runner,
        prefix,
        created_prefix,
        target,
        guard,
        _build_guard: build_guard,
    })
}

fn add_default_target(
    paths: &Paths,
    environment: &Environment,
    exe: &str,
    name: Option<String>,
    desktop: bool,
) -> anyhow::Result<bool> {
    let prefix = environment.prefix(paths)?;
    if desktop::check(paths, &prefix, exe).is_err() {
        return Ok(false);
    }
    let mut batch = Batch::new(environment.config.targets.clone());
    batch.add(
        &environment.name,
        Spec {
            exe: exe.to_owned(),
            args: Vec::new(),
            cwd: None,
            name,
            desktop,
        },
    )?;
    batch.set_default(Some(&environment.name));
    match targets::write(paths, &environment.name, &batch)? {
        Written::Saved => Ok(true),
        Written::Changed(_) => bail!(
            "{}'s config changed while its target was being added",
            environment.name
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

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
            fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
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

    fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
        fn visit(
            root: &Path,
            path: &Path,
            entries: &mut std::collections::BTreeMap<PathBuf, (u64, std::time::SystemTime)>,
        ) {
            let metadata = fs::symlink_metadata(path).unwrap();
            entries.insert(
                path.strip_prefix(root).unwrap().to_owned(),
                (metadata.len(), metadata.modified().unwrap()),
            );
            if metadata.is_dir() {
                for entry in fs::read_dir(path).unwrap() {
                    visit(root, &entry.unwrap().path(), entries);
                }
            }
        }
        let mut entries = std::collections::BTreeMap::new();
        visit(root, root, &mut entries);
        entries
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
        let before = snapshot(root.path());
        let plan = plan(&paths, request).unwrap();
        assert_eq!(snapshot(root.path()), before);
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
        let before = snapshot(root.path());
        let plan = plan(&paths, request).unwrap();
        assert_eq!(snapshot(root.path()), before);
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
        let before = snapshot(root.path());
        let plan = plan(&paths, request).unwrap();
        assert_eq!(snapshot(root.path()), before);
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
        let before = snapshot(root.path());
        let error = plan(&paths, request).unwrap_err();
        assert_eq!(snapshot(root.path()), before);
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
        let before = snapshot(root.path());
        let plan = plan(&paths, request).unwrap();
        assert_eq!(snapshot(root.path()), before);
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
        let build_lock =
            crate::build_lock::Lock::new(&paths, crate::build_lock::Kind::Runner, "fake");
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
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
        let bin = dir.join("files/bin");
        fs::create_dir_all(&bin).unwrap();
        for program in ["wine", "wineserver"] {
            let file = bin.join(program);
            fs::write(&file, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        dir
    }

    #[test]
    fn planning_proton_omits_components_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_proton(&paths, "GE-Proton11-7");
        let before = snapshot(root.path());
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
        assert_eq!(snapshot(root.path()), before);
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
        let before = snapshot(root.path());
        let plan = plan(&paths, request).unwrap();
        assert_eq!(snapshot(root.path()), before);
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
        let before = snapshot(root.path());
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
        assert_eq!(snapshot(root.path()), before);
    }

    #[test]
    fn native_planning_writes_nothing_and_resolves_exe() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let bin = root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("game");
        fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let mut global = GlobalConfig::default();
        global.env.insert("PATH".into(), bin.display().to_string());
        let before = snapshot(root.path());
        let plan = Environment::plan_native(&paths, &global, "game", Some("game")).unwrap();
        assert_eq!(plan.name, "game");
        assert_eq!(plan.exe.as_deref(), Some("game"));
        assert_eq!(plan.resolved_exe, Some(exe));
        assert!(Environment::plan_native(&paths, &global, "game", Some("missing")).is_err());
        assert!(Environment::plan_native(&paths, &global, "bad name", None).is_err());
        assert_eq!(snapshot(root.path()), before);
        fs::create_dir_all(paths.environments_dir()).unwrap();
        fs::write(paths.environment_config("taken"), "kind = 'native'\n").unwrap();
        let before = snapshot(root.path());
        assert!(matches!(
            Environment::plan_native(&paths, &global, "taken", None),
            Err(Error::EnvironmentExists(_))
        ));
        assert_eq!(snapshot(root.path()), before);
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
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
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
}
