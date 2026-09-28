use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};

use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::desktop;
use crate::environment::Environment;
use crate::error::Error;
use crate::install::{self, Wanted};
use crate::installer::Output;
use crate::operation::{Guard, Locks, Operation};
use crate::paths::Paths;
use crate::prefix::PrefixEvent;
use crate::runner::Runner;
use crate::runners::Runners;
use crate::runtime::{RunStatus, Runtime};
use crate::supervisor::{self, Claim, Spec, Started};
use crate::winetricks::{self, WinetricksEvent};

use super::{Host, Launch, Overrides, Wayland};

#[derive(Debug)]
pub enum LaunchEvent {
    BuildingPrefix {
        dir: PathBuf,
        runner: Runner,
    },
    Components(Vec<install::Change>),
    Desktop(Vec<desktop::Change>),
    Winetricks(WinetricksEvent),
    NoWaylandSession,
    /// The caller can release temporary signal handlers before supervisor IPC.
    Starting,
    Started,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    Started,
    AlreadyRunning,
}

/// A checked launch that keeps its environment and prefix locked.
pub struct PreparedLaunch {
    pub global: GlobalConfig,
    pub environment: Environment,
    pub launch: Launch,
    _guard: Guard,
}

pub fn prepare(
    paths: &Paths,
    name: &str,
    overrides: &Overrides,
    token: &Token,
    mut events: impl FnMut(LaunchEvent),
) -> anyhow::Result<PreparedLaunch> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, name)?;
    prepare_target(paths, &global, &environment, overrides, token, &mut events)?;
    let launch = Launch::target(paths, &global, &environment, overrides)?;
    let held: Vec<&Path> = launch.host.prefix().into_iter().collect();
    let guard = Locks::new(paths).acquire(Operation::Launch, name, &held)?;
    Environment::ensure_exists(paths, name)?;
    Runtime::new(paths, name).check_resolved(name)?;
    reconcile(paths, &global, &environment, &launch, events)?;
    Ok(PreparedLaunch {
        global,
        environment,
        launch,
        _guard: guard,
    })
}

pub fn runner(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> anyhow::Result<Runner> {
    Ok(Runners::new(paths, global)?.resolve(&environment.wine()?.runner)?)
}

pub fn prepare_target(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    overrides: &Overrides,
    token: &Token,
    mut events: impl FnMut(LaunchEvent),
) -> anyhow::Result<()> {
    if environment.config.wine().is_none() {
        return Ok(Launch::check_native_target(
            paths,
            global,
            environment,
            overrides,
        )?);
    }
    let runner = runner(paths, global, environment)?;
    Launch::check_target(paths, global, environment, &runner, overrides)?;
    if !environment.config.winetricks.is_empty()
        && !Runtime::new(paths, &environment.name).is_running()?
        && !winetricks::missing_for(
            &runner,
            &environment.prefix(paths)?,
            &environment.config.winetricks,
        )?
        .is_empty()
    {
        winetricks::check_available(&runner)?;
    }
    ensure_prefix(paths, global, environment, &runner, token, &mut events)
}

pub fn ensure_prefix(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: &Runner,
    token: &Token,
    mut events: impl FnMut(LaunchEvent),
) -> anyhow::Result<()> {
    let log = paths.runtime_dir(&environment.name).join("create.log");
    match environment.build_prefix(
        paths,
        global,
        runner,
        false,
        token,
        Output::Log(&log),
        |event| {
            if let PrefixEvent::Started { dir } = event {
                events(LaunchEvent::BuildingPrefix {
                    dir,
                    runner: runner.clone(),
                });
            }
        },
    ) {
        Ok(_) => Ok(()),
        Err(err @ (Error::EmptyPrefix(_) | Error::EmptyPrefixParent { .. })) => Err(anyhow!(
            "{err}. Run `wineo setup {}` to create it there.",
            environment.name
        )),
        Err(err) => Err(err.into()),
    }
}

pub fn reconcile(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    launch: &Launch,
    mut events: impl FnMut(LaunchEvent),
) -> anyhow::Result<()> {
    let Host::Wine { runner, prefix } = &launch.host else {
        let changes = desktop::reconcile_native(paths, global, environment)?;
        if !changes.is_empty() {
            events(LaunchEvent::Desktop(changes));
        }
        return Ok(());
    };
    let wanted = Wanted::from_config(&environment.config);
    let changes = install::reconcile(paths, global, &environment.name, runner, prefix, &wanted)?;
    if !changes.is_empty() {
        events(LaunchEvent::Components(changes));
    }
    let changes = desktop::reconcile(paths, global, environment, Some(runner), prefix)?;
    if !changes.is_empty() {
        events(LaunchEvent::Desktop(changes));
    }
    Ok(())
}

pub fn install_winetricks(
    global: &GlobalConfig,
    environment: &Environment,
    launch: &Launch,
    overrides: &Overrides,
    token: &Token,
    output: Output<'_>,
    mut events: impl FnMut(LaunchEvent),
) -> anyhow::Result<()> {
    let Host::Wine { runner, prefix } = &launch.host else {
        return Ok(());
    };
    if environment.config.winetricks.is_empty() {
        return Ok(());
    }
    let missing = winetricks::missing_for(runner, prefix, &environment.config.winetricks)?;
    if missing.is_empty() {
        return Ok(());
    }
    let env = winetricks::merged_env(runner, global, environment, &overrides.env)?;
    winetricks::install_missing(
        &environment.name,
        runner,
        prefix,
        &env,
        &environment.config.winetricks,
        global.stop_timeout,
        token,
        output,
        |event| events(LaunchEvent::Winetricks(event)),
    )
}

pub fn start(
    paths: &Paths,
    name: &str,
    overrides: Overrides,
    exe: &Path,
    token: &Token,
    winetricks_output: Output<'_>,
    mut events: impl FnMut(LaunchEvent),
) -> anyhow::Result<StartOutcome> {
    let PreparedLaunch {
        global,
        environment,
        launch,
        _guard,
    } = prepare(paths, name, &overrides, token, &mut events)?;
    let runtime = Runtime::new(paths, name);

    if runtime.is_running()? {
        let running = runtime
            .read_state()
            .ok()
            .flatten()
            .filter(|run| run.status == RunStatus::Running);
        if let Some(run) = &running
            && Some(run.target.as_str()) != launch.target.as_deref()
        {
            return Err(Error::DifferentTargetRunning {
                name: name.to_owned(),
                running: run.target.clone(),
                requested: launch.target.clone().unwrap_or_default(),
            }
            .into());
        }
        return Ok(StartOutcome::AlreadyRunning);
    }
    install_winetricks(
        &global,
        &environment,
        &launch,
        &overrides,
        token,
        winetricks_output,
        &mut events,
    )?;
    if launch.wayland == Wayland::NoSession {
        events(LaunchEvent::NoWaylandSession);
    }
    let spec = Spec {
        claim: Claim {
            name: name.to_owned(),
            locks: Locks::new(paths),
            config: environment.path.clone(),
        },
        runtime_dir: runtime.dir().to_owned(),
        stats_log: paths.stats_log(name),
        stop_timeout: global.stop_timeout,
        log_limit_mb: global.log_limit_mb,
        launch,
        overrides,
    };
    events(LaunchEvent::Starting);
    match supervisor::start(exe, paths.portable_root(), &spec)
        .with_context(|| format!("failed to start {name}"))?
    {
        Started::Running(_) => {
            events(LaunchEvent::Started);
            Ok(StartOutcome::Started)
        }
        Started::AlreadyRunning => Ok(StartOutcome::AlreadyRunning),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn detached_start_reports_supervisor_outcomes_and_event_order() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        let config = paths.environment_config("app");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, "kind = 'native'\n[targets.app]\nexe = '/bin/sh'\n").unwrap();

        for (reply, expected) in [
            ("started 42", StartOutcome::Started),
            ("already-running", StartOutcome::AlreadyRunning),
        ] {
            let exe = root.path().join("supervisor");
            fs::write(
                &exe,
                format!("#!/bin/sh\ncat >/dev/null\nprintf '{reply}\\n'\n"),
            )
            .unwrap();
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
            let mut events = Vec::new();
            let outcome = start(
                &paths,
                "app",
                Overrides::default(),
                &exe,
                &Token::new(),
                Output::Inherit,
                |event| events.push(event),
            )
            .unwrap();
            assert_eq!(outcome, expected);
            assert!(matches!(events.first(), Some(LaunchEvent::Starting)));
            assert_eq!(
                events
                    .iter()
                    .any(|event| matches!(event, LaunchEvent::Started)),
                expected == StartOutcome::Started
            );
        }
    }
}
