//! Creates a Wine environment and brings its prefix and desktop entries in
//! line with the new config.

use std::path::PathBuf;

use anyhow::bail;

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
    pub desktop: bool,
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
}

pub fn check_arch(runner: &Runner, arch: Option<Arch>) -> crate::Result<()> {
    if runner.is_proton() && arch == Some(Arch::Win32) {
        return Err(Error::ProtonWin32(runner.name().to_owned()));
    }
    Ok(())
}

pub fn run(
    paths: &Paths,
    request: CreateRequest,
    token: &Token,
    mut events: impl FnMut(CreateEvent),
) -> anyhow::Result<Created> {
    let global = GlobalConfig::load(paths)?;
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
            let runner_name = match (&request.runner, &template) {
                (Some(runner), _) => runner.clone(),
                (None, Some(template)) => template
                    .runner_name(&runners, &global.defaults.runner)?
                    .unwrap_or_else(|| global.defaults.runner.clone()),
                (None, None) => global.defaults.runner.clone(),
            };
            runners.resolve(&runner_name)?
        }
    };
    check_arch(&runner, arch)?;
    let template = template.map(|template| {
        let (fitted, left_out) = template.for_runner(&runner);
        if left_out {
            events(CreateEvent::TemplateComponentsOmitted {
                runner: runner.name().to_owned(),
            });
        }
        fitted
    });
    if let Some(template) = &template {
        install::check_available(
            paths,
            &global,
            &request.name,
            &runner,
            &Wanted {
                dxvk: template.dxvk.clone(),
                vkd3d: template.vkd3d.clone(),
            },
        )?;
    }
    let new = NewEnvironment {
        name: request.name,
        dir: request.dir,
        arch,
        template,
    };
    let prefix = new.prefix_dir(paths, &global)?;
    let guard = Locks::new(paths).acquire(Operation::Create, &new.name, &[&prefix])?;
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
    let target = match request.exe {
        Some(exe) if add_default_target(paths, &environment, &exe, request.desktop)? => {
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
    })
}

fn add_default_target(
    paths: &Paths,
    environment: &Environment,
    exe: &str,
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
            name: None,
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
