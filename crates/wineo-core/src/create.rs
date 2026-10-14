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
use crate::installer::{self, Installer, Output};
use crate::launch::Launch;
use crate::operation::{Guard, Locks, Operation};
use crate::paths::Paths;
use crate::prefix::PrefixEvent;
use crate::runner::Runner;
use crate::runners::Runners;
use crate::runtime::Runtime;
use crate::supervisor::Watched;
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
    /// The GOG game the environment runs, written with the rest of its first
    /// config.
    pub gog_id: Option<String>,
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

/// The create lock stays held until this value is dropped. `create_and_install`
/// hands `guard` to the installer, so the prefix stays locked through install.
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
        gog_id: request.gog_id,
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
    events: impl FnMut(CreateEvent),
) -> anyhow::Result<Created> {
    apply_with(paths, plan, None, token, events)
}

/// `apply`, for a caller already holding the environment and the planned
/// prefix exclusively, which the creation takes over instead of locking.
fn apply_with(
    paths: &Paths,
    plan: CreatePlan,
    held: Option<Guard>,
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
    let guard = match held {
        Some(mut guard) => {
            guard.relabel(Operation::Create)?;
            guard
        }
        None => Locks::new(paths).acquire(Operation::Create, &new.name, &[&prefix])?,
    };
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

/// What runs in a new environment once it exists.
#[derive(Debug, Clone, Copy)]
pub enum Step<'a> {
    /// A Windows program, without arguments, supervised until everything it
    /// started has exited. It has a token of its own, since the CLI listens
    /// for Ctrl+C separately while creating and while installing.
    Installer {
        program: &'a str,
        output: Output<'a>,
        token: &'a crate::cancel::Token,
    },
    /// A GOG game's Windows installer, run with the arguments and locale
    /// variables that make it install silently into the game's folder. How
    /// its exit code reads is up to the caller.
    GogInstaller {
        program: &'a str,
        args: &'a installer::Args,
        output: Output<'a>,
        token: &'a crate::cancel::Token,
    },
}

/// What a frontend does around `create_and_install`. Hooks run under the
/// create lock, which the install step keeps, so nothing can start in the
/// environment in between. `Stop` is the caller's own reason to end early.
pub trait Hooks {
    type Stop;

    /// Creation progress, as `run` reports it.
    fn event(&mut self, event: CreateEvent);

    /// Runs once the environment exists, before the install step. A stop
    /// keeps the environment and skips the step.
    fn created(&mut self, _created: &Created) -> Result<(), Self::Stop> {
        Ok(())
    }

    /// Runs once the installer holds the prefix, right before it starts.
    fn ready(&mut self, _launch: &Launch) -> Result<(), Self::Stop> {
        Ok(())
    }

    /// The installer's supervision notices.
    fn watched(&mut self, _watched: Watched) {}
}

/// How the install step ended. The environment is kept either way.
#[derive(Debug)]
pub enum Stepped<S> {
    /// No step was asked for.
    None,
    /// A hook stopped before the step started.
    Stopped(S),
    /// The installer could not be prepared, so nothing ran.
    NotPrepared(crate::error::Error),
    /// The installer ran, or failed to start. `resolved` is whether nothing
    /// it may have left running is recorded against the environment.
    Ran {
        outcome: crate::Result<installer::Outcome>,
        resolved: bool,
    },
}

/// A created environment and how its install step went. The create lock is
/// already released.
pub struct Installed<S> {
    pub global: GlobalConfig,
    pub environment: Environment,
    pub runner: Runner,
    pub prefix: PathBuf,
    pub created_prefix: bool,
    pub target: TargetOutcome,
    pub step: Stepped<S>,
    /// The environment's and prefix's locks, still held, when the caller
    /// handed them in with `create_and_install_held`. Lost when the step
    /// couldn't be prepared.
    pub guard: Option<Guard>,
    _build_guard: Option<build_lock::Guard>,
}

/// Creates an environment, building or adopting its prefix, then runs `step`
/// in it under the same lock. A failure or cancel before the config is saved
/// leaves nothing behind. From `CreateEvent::Saved` on the environment is
/// kept: an error from the work after it (the default target, components,
/// desktop entries) still returns `Err`, and however the step ends, the
/// caller reports how to recover.
pub fn create_and_install<H: Hooks>(
    paths: &Paths,
    request: CreateRequest,
    step: Option<Step>,
    token: &crate::cancel::Token,
    hooks: &mut H,
) -> anyhow::Result<Installed<H::Stop>> {
    create_and_install_with(paths, request, step, None, token, hooks)
}

/// `create_and_install` for a caller already holding the environment and the
/// prefix `request` plans exclusively. The locks pass through creation and
/// the step and come back in `Installed::guard`, so nothing else can touch
/// the prefix until the caller lets go.
pub fn create_and_install_held<H: Hooks>(
    paths: &Paths,
    request: CreateRequest,
    step: Option<Step>,
    guard: Guard,
    token: &crate::cancel::Token,
    hooks: &mut H,
) -> anyhow::Result<Installed<H::Stop>> {
    create_and_install_with(paths, request, step, Some(guard), token, hooks)
}

fn create_and_install_with<H: Hooks>(
    paths: &Paths,
    request: CreateRequest,
    step: Option<Step>,
    held: Option<Guard>,
    token: &crate::cancel::Token,
    hooks: &mut H,
) -> anyhow::Result<Installed<H::Stop>> {
    let keep = held.is_some();
    let created = apply_with(paths, plan(paths, request)?, held, token, |event| {
        hooks.event(event)
    })?;
    let stopped = hooks.created(&created);
    let Created {
        global,
        environment,
        runner,
        prefix,
        created_prefix,
        target,
        guard,
        _build_guard,
    } = created;
    let (step, guard) = match (stopped, step) {
        (Err(stop), _) => (Stepped::Stopped(stop), Some(guard)),
        (Ok(()), None) => (Stepped::None, Some(guard)),
        (
            Ok(()),
            Some(Step::Installer {
                program,
                output,
                token,
            }),
        ) => install(
            paths,
            &global,
            &environment.name,
            &runner,
            guard,
            (program, &installer::Args::default()),
            output,
            token,
            hooks,
        ),
        (
            Ok(()),
            Some(Step::GogInstaller {
                program,
                args,
                output,
                token,
            }),
        ) => install(
            paths,
            &global,
            &environment.name,
            &runner,
            guard,
            (program, args),
            output,
            token,
            hooks,
        ),
    };
    Ok(Installed {
        global,
        environment,
        runner,
        prefix,
        created_prefix,
        target,
        step,
        guard: guard.filter(|_| keep),
        _build_guard,
    })
}

#[allow(clippy::too_many_arguments)]
fn install<H: Hooks>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    runner: &Runner,
    guard: Guard,
    (program, args): (&str, &installer::Args),
    output: Output,
    token: &crate::cancel::Token,
    hooks: &mut H,
) -> (Stepped<H::Stop>, Option<Guard>) {
    let installer = match Installer::prepare_with_runner_args(
        guard, paths, global, name, program, args, runner,
    ) {
        Ok(installer) => installer,
        Err(err) => return (Stepped::NotPrepared(err), None),
    };
    // Under the installer's lock, so the installer runs against the
    // components the config asks for, like a launch does.
    if let Err(err) = reconcile_components(paths, global, name, installer.launch(), hooks) {
        return (Stepped::NotPrepared(err), None);
    }
    if let Err(stop) = hooks.ready(installer.launch()) {
        return (Stepped::Stopped(stop), None);
    }
    let (outcome, guard) = installer.run_keeping(
        token,
        std::time::Duration::from_secs(global.stop_timeout.into()),
        output,
        |watched| hooks.watched(watched),
    );
    let resolved = Runtime::new(paths, name).check_resolved(name).is_ok();
    (Stepped::Ran { outcome, resolved }, Some(guard))
}

fn reconcile_components<H: Hooks>(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    launch: &Launch,
    hooks: &mut H,
) -> crate::Result<()> {
    let crate::launch::Host::Wine { runner, prefix } = &launch.host else {
        return Ok(());
    };
    let environment = Environment::load(paths, name)?;
    let wanted = Wanted::from_config(&environment.config);
    let changes = install::reconcile(paths, global, name, runner, prefix, &wanted)?;
    if !changes.is_empty() {
        hooks.event(CreateEvent::Components(changes));
    }
    Ok(())
}

/// What every frontend does once an installer step has run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterInstall {
    /// Wine has settled, so offer what is installed as targets, whatever
    /// the exit code.
    Programs,
    /// Canceled: keep the environment, skip the programs, and point to setup.
    Canceled,
    /// Something it started may still be running, so offer nothing and warn.
    Unresolved,
    /// It never started.
    NotRun,
}

/// Decides what follows an installer run from how it ended.
pub fn after_install(outcome: &crate::Result<installer::Outcome>, resolved: bool) -> AfterInstall {
    match outcome {
        _ if !resolved => AfterInstall::Unresolved,
        Ok(installer::Outcome::Finished { .. }) => AfterInstall::Programs,
        Ok(installer::Outcome::Canceled { .. }) => AfterInstall::Canceled,
        Ok(installer::Outcome::Unresolved { .. }) => AfterInstall::Unresolved,
        Err(_) => AfterInstall::NotRun,
    }
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
mod tests;
