use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow};

use crate::cancel::Token;
use crate::config::{GlobalConfig, Kind};
use crate::desktop;
use crate::environment::Environment;
use crate::error::Error;
use crate::gog::folder::{FolderError, InUseGuard};
use crate::hooks::{self, Hook, Phase};
use crate::install::{self, Wanted};
use crate::installer::Output;
use crate::operation::{Guard, LaunchReservation, Locks, Operation};
use crate::paths::Paths;
use crate::prefix::PrefixEvent;
use crate::runner::Runner;
use crate::runners::Runners;
use crate::runtime::{RunStatus, Runtime};
use crate::supervisor::{self, Claim, Spec, Started};
use crate::winetricks::{self, WinetricksEvent};

use super::{Host, Launch, Overrides, Wayland, resolve_target};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AfterHook {
    paths: Paths,
    environment: String,
    target: String,
    exe: String,
    wine: Option<(Runner, PathBuf)>,
    command: String,
    #[serde(default)]
    steam: Option<hooks::Steam>,
}

impl AfterHook {
    pub fn run(
        &self,
        guard: &Guard,
        state: &crate::runtime::RunState,
        token: &Token,
        stop_timeout: Duration,
        output: hooks::Output,
    ) -> crate::error::Result<()> {
        let end = match state.status {
            RunStatus::Exited => hooks::End::Exited,
            RunStatus::Failed => hooks::End::Failed,
            RunStatus::Stopped => hooks::End::Stopped,
            RunStatus::Running => return Ok(()),
        };
        let running_at = state.running_at.unwrap_or(state.started_at);
        let seconds = if self.steam.is_some() && state.running_at.is_none() {
            0
        } else {
            state
                .ended_at
                .unwrap_or(running_at)
                .duration_since(running_at)
                .as_secs()
                .max(0) as u64
        };
        self.run_ending(
            guard,
            hooks::Ending {
                end,
                exit_code: state.exit_code,
                seconds,
            },
            token,
            stop_timeout,
            output,
        )
    }

    fn run_ending(
        &self,
        guard: &Guard,
        ending: hooks::Ending,
        token: &Token,
        stop_timeout: Duration,
        output: hooks::Output,
    ) -> crate::error::Result<()> {
        let context = hooks::Context {
            environment: &self.environment,
            target: &self.target,
            exe: &self.exe,
            wine: self
                .wine
                .as_ref()
                .map(|(runner, prefix)| (runner, prefix.as_path())),
            steam: self.steam.as_ref(),
            hook: Hook::After(ending),
        };
        match hooks::run(
            &self.command,
            &self.paths,
            &context,
            guard,
            token,
            stop_timeout,
            output,
        )? {
            hooks::Outcome::Finished(status) if !status.success() => {
                Err(Error::StartFailed(format!(
                    "the after hook exited with code {}.",
                    crate::runtime::exit_code(status)
                )))
            }
            _ => Ok(()),
        }
    }
}

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
    /// Launcher-side cleanup uses a fresh token, even after a canceled before.
    AfterHook(Token),
    Started,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    Started,
    AlreadyRunning,
    SteamAlreadyRunning,
    SteamRequested,
    SteamAdopted,
}

/// A checked launch that keeps its environment and prefix locked.
pub struct PreparedLaunch {
    pub global: GlobalConfig,
    pub environment: Environment,
    pub launch: Launch,
    pub after: Option<AfterHook>,
    guard: Option<Guard>,
    hook_output: hooks::Output,
    _reservation: Option<LaunchReservation>,
    /// The in-use locks of the GOG folders it runs from, held until the run
    /// ends or a supervisor takes them over.
    folders: Vec<InUseGuard>,
}

impl PreparedLaunch {
    pub fn after_guard(&mut self, paths: &Paths) -> crate::error::Result<Option<Guard>> {
        let Some(guard) = self.guard.take() else {
            return Ok(None);
        };
        if self.after.is_none() {
            return Ok(None);
        }
        Locks::new(paths)
            .after_launch(guard, &self.environment.name)
            .map(Some)
            .map_err(|err| {
                Error::StartFailed(format!(
                    "the after hook was skipped: {}",
                    crate::error::chain(&err)
                ))
            })
    }

    /// Cleans up a launch that got past before but never started its program.
    pub fn failed(&mut self, paths: &Paths, lock: Option<crate::runtime::RunLock>, token: &Token) {
        failed_after(
            paths,
            &self.environment.name,
            self.after.as_ref(),
            self.guard.take(),
            self.global.stop_timeout,
            lock,
            token,
            std::mem::replace(&mut self.hook_output, hooks::Output::Inherit),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn failed_after(
    paths: &Paths,
    name: &str,
    after: Option<&AfterHook>,
    guard: Option<Guard>,
    stop_timeout: u32,
    lock: Option<crate::runtime::RunLock>,
    token: &Token,
    output: hooks::Output,
) {
    let Some(after) = after else {
        return;
    };
    let locks = Locks::new(paths);
    let result = (|| {
        let guard = locks.after_hook(guard, name).map_err(|err| {
            Error::StartFailed(format!(
                "the after hook was skipped: {}",
                crate::error::chain(&err)
            ))
        })?;
        drop(lock);
        // A canceled preparation must still undo a successful before hook.
        after.run_ending(
            &guard,
            hooks::Ending {
                end: hooks::End::Failed,
                exit_code: None,
                seconds: 0,
            },
            token,
            Duration::from_secs(stop_timeout.into()),
            output,
        )
    })();
    if let Err(err) = result {
        eprintln!("warning: {}", crate::error::chain(&err));
    }
}

pub fn prepare(
    paths: &Paths,
    name: &str,
    overrides: &Overrides,
    token: &Token,
    hook_output: hooks::Output,
    mut events: impl FnMut(LaunchEvent),
) -> anyhow::Result<PreparedLaunch> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, name)?;
    // Before anything else, so no hook runs for a launch that can't happen.
    let steam_baseline = if let Kind::Proxied(proxied) = &environment.config.kind {
        super::check_steam_overrides(&environment, overrides)?;
        Runtime::new(paths, name).clean_up_lost_run(
            Duration::from_secs(global.stop_timeout.into()),
            &paths.stats_log(name),
        )?;
        if Runtime::new(paths, name).is_running()? {
            return Err(Error::AlreadyRunning(name.to_owned()).into());
        }
        if let Some(install) = crate::steam::active_for(paths, &global) {
            let (baseline, matching) = crate::steam::baseline(&install, &proxied.app_id)?;
            if !matching.is_empty() {
                return Err(Error::SteamAlreadyRunning(name.to_owned()).into());
            }
            Launch::target(paths, &global, &environment, overrides)?;
            Some(baseline)
        } else {
            Launch::target(paths, &global, &environment, overrides)?;
            None
        }
    } else {
        None
    };
    let steam_hook = if let Kind::Proxied(proxied) = &environment.config.kind {
        crate::steam::active_for(paths, &global)
            .map(|install| hooks::Steam::new(&install, &proxied.app_id))
    } else {
        None
    };
    precheck(paths, &global, &environment, overrides)?;
    // Leave ordinary target errors to preparation, which checks the runner first.
    let selected = if environment.config.hooks.is_some()
        || environment
            .config
            .targets
            .values()
            .any(|target| target.hooks.is_some())
    {
        resolve_target(&environment, overrides.target.as_deref()).ok()
    } else {
        None
    };
    let locks = Locks::new(paths);
    let runtime = Runtime::new(paths, name);
    let configured = selected
        .and_then(|(_, target)| super::hooks(&environment, Some(target)))
        .filter(|hooks| hooks.before.is_some() || hooks.after.is_some());
    let reservation = if configured.is_some() {
        let (target_name, _) = selected.expect("hooks have a selected target");
        check_not_running(&locks, &runtime, name, target_name)?;
        runtime.check_resolved(name)?;
        let reservation = locks.reserve_launch(name)?;
        check_not_running(&locks, &runtime, name, target_name)?;
        runtime.check_resolved(name)?;
        Some(reservation)
    } else {
        None
    };
    let wine = if configured.is_some() && environment.config.wine().is_some() {
        Some((
            runner(paths, &global, &environment)?,
            environment.prefix(paths)?,
        ))
    } else {
        None
    };
    let after = configured
        .and_then(|hooks| hooks.after.as_ref())
        .map(|command| {
            let (target_name, target) = selected.expect("hooks have a selected target");
            AfterHook {
                paths: paths.clone(),
                environment: name.to_owned(),
                target: target_name.to_owned(),
                exe: target.exe.clone(),
                wine: wine.clone(),
                command: command.clone(),
                steam: steam_hook.clone(),
            }
        });
    if let Some(before) = configured.and_then(|hooks| hooks.before.as_deref()) {
        let (target_name, target) = selected.expect("hooks have a selected target");
        let prefixes: Vec<&Path> = wine
            .as_ref()
            .map(|(_, prefix)| prefix.as_path())
            .into_iter()
            .collect();
        crate::trash::check_pending(paths, name, &prefixes)?;
        let guard = reservation
            .as_ref()
            .expect("hooks reserved the launch")
            .acquire(Operation::Hook(Phase::Before), &[])?;
        runtime.check_resolved(name)?;
        if runtime.is_running()? {
            return Err(Error::AlreadyRunning(name.to_owned()).into());
        }
        let context = hooks::Context {
            environment: name,
            target: target_name,
            exe: &target.exe,
            wine: wine
                .as_ref()
                .map(|(runner, prefix)| (runner, prefix.as_path())),
            steam: steam_hook.as_ref(),
            hook: Hook::Before,
        };
        match hooks::run(
            before,
            paths,
            &context,
            &guard,
            token,
            Duration::from_secs(global.stop_timeout.into()),
            match &hook_output {
                hooks::Output::Inherit => hooks::Output::Inherit,
                hooks::Output::File(file) => {
                    hooks::Output::File(file.try_clone().context("failed to clone hook output")?)
                }
            },
        )? {
            hooks::Outcome::Canceled(_) => return Err(Error::Canceled("launch").into()),
            hooks::Outcome::Finished(status) if !status.success() => {
                return Err(Error::BeforeHookFailed(crate::runtime::exit_code(status)).into());
            }
            _ => {}
        }
    }
    let mut guard = None;
    let mut folders = Vec::new();
    let result = (|| {
        prepare_target_reserved(
            paths,
            &global,
            &environment,
            overrides,
            token,
            reservation.as_ref(),
            &mut events,
        )?;
        let mut launch = Launch::target(paths, &global, &environment, overrides)?;
        if let Host::Proxied { baseline, .. } = &mut launch.host {
            *baseline = steam_baseline.clone();
        }
        let held: Vec<&Path> = launch.host.prefix().into_iter().collect();
        guard = Some(match &reservation {
            Some(reservation) => reservation.acquire(Operation::Launch, &held)?,
            None => locks.acquire(Operation::Launch, name, &held)?,
        });
        Environment::ensure_exists(paths, name)?;
        runtime.check_resolved(name)?;
        folders = protect(paths, &environment, &launch, token)?;
        reconcile(paths, &global, &environment, &launch, &mut events)?;
        Ok::<_, anyhow::Error>(launch)
    })();
    let launch = match result {
        Ok(launch) => launch,
        Err(err) => {
            let cleanup = Token::new();
            if matches!(
                err.downcast_ref::<Error>(),
                Some(Error::AlreadyRunning(_) | Error::DifferentTargetRunning { .. })
            ) {
                return Err(err);
            }
            if after.is_some() {
                events(LaunchEvent::AfterHook(cleanup.clone()));
            }
            failed_after(
                paths,
                name,
                after.as_ref(),
                guard,
                global.stop_timeout,
                None,
                &cleanup,
                hook_output,
            );
            return Err(err);
        }
    };
    Ok(PreparedLaunch {
        global,
        environment,
        launch,
        after,
        guard,
        hook_output,
        _reservation: reservation,
        folders,
    })
}

/// Refuses a launch `protect` would refuse, before any hook runs or a
/// restart stops the old run. Holds nothing afterwards. A target that
/// doesn't resolve yet is left to preparation, which reports it, but the
/// owner's own folder is still checked, since an interrupted update can be
/// what broke the target.
pub fn precheck(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    overrides: &Overrides,
) -> anyhow::Result<()> {
    if matches!(environment.config.kind, Kind::Proxied(_))
        || (environment.config.gog_id.is_none() && !paths.gog_folders_dir().exists())
    {
        return Ok(());
    }
    let gog_id = environment.config.gog_id.as_deref();
    let Ok(launch) = Launch::target(paths, global, environment, overrides) else {
        // Without paths an ownerless folder to claim can't be found, so a
        // missing state is left to preparation too.
        return match crate::gog::folder::precheck(paths, &environment.name, gog_id, &[]) {
            Err(err) if matches!(err.downcast_ref(), Some(FolderError::StateMissing { .. })) => {
                Ok(())
            }
            result => result,
        };
    };
    crate::gog::folder::precheck(paths, &environment.name, gog_id, &launch.named_paths())
}

/// Checks the GOG game folders a launch runs from and takes their in-use
/// locks, which the caller holds until the program exits. Called holding
/// the environment's operation lock, which comes first in lock order.
pub fn protect(
    paths: &Paths,
    environment: &Environment,
    launch: &Launch,
    token: &Token,
) -> anyhow::Result<Vec<InUseGuard>> {
    protect_as(paths, environment, launch, token, "launch")
}

/// `protect`, with a cancel reported as `Error::Canceled(what)`, the error
/// frontends already handle.
pub(super) fn protect_as(
    paths: &Paths,
    environment: &Environment,
    launch: &Launch,
    token: &Token,
    what: &'static str,
) -> anyhow::Result<Vec<InUseGuard>> {
    if matches!(launch.host, Host::Proxied { .. }) {
        return Ok(Vec::new());
    }
    crate::gog::folder::protect(
        paths,
        &environment.name,
        environment.config.gog_id.as_deref(),
        &launch.named_paths(),
        &|| token.is_canceled(),
    )
    .map_err(|err| match err.downcast_ref() {
        Some(FolderError::Canceled) => Error::Canceled(what).into(),
        _ => err,
    })
}

fn check_not_running(
    locks: &Locks,
    runtime: &Runtime,
    name: &str,
    requested: &str,
) -> crate::error::Result<()> {
    if let Some(holder) = locks.busy_with(name)? {
        return Err(Error::Busy {
            subject: format!("environment `{name}`"),
            holder: Some(holder),
            holder_label: holder.describe(),
        });
    }
    if runtime.is_running()? {
        if let Some(run) = runtime
            .read_state()?
            .filter(|run| run.status == RunStatus::Running)
            && run.target != requested
        {
            return Err(Error::DifferentTargetRunning {
                name: name.to_owned(),
                running: run.target,
                requested: requested.to_owned(),
            });
        }
        return Err(Error::AlreadyRunning(name.to_owned()));
    }
    Ok(())
}

pub fn runner(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> anyhow::Result<Runner> {
    Ok(Runners::new(paths, global)?.resolve(&environment.wine()?.runner)?)
}

pub fn runner_locked(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> anyhow::Result<(Runner, Option<crate::build_lock::Guard>)> {
    Ok(Runners::new(paths, global)?.resolve_locked(paths, &environment.wine()?.runner)?)
}

pub fn prepare_target(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    overrides: &Overrides,
    token: &Token,
    events: impl FnMut(LaunchEvent),
) -> anyhow::Result<()> {
    prepare_target_reserved(paths, global, environment, overrides, token, None, events)
}

fn prepare_target_reserved(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    overrides: &Overrides,
    token: &Token,
    reservation: Option<&LaunchReservation>,
    mut events: impl FnMut(LaunchEvent),
) -> anyhow::Result<()> {
    match &environment.config.kind {
        Kind::Wine(_) => {}
        Kind::Native => {
            return Ok(Launch::check_native_target(
                paths,
                global,
                environment,
                overrides,
            )?);
        }
        Kind::Proxied(_) => {
            return Ok(Launch::target(paths, global, environment, overrides).map(|_| ())?);
        }
    }
    let (runner, _build_guard) = runner_locked(paths, global, environment)?;
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
    ensure_prefix_reserved(
        paths,
        global,
        environment,
        &runner,
        token,
        reservation,
        &mut events,
    )
}

pub fn ensure_prefix(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: &Runner,
    token: &Token,
    events: impl FnMut(LaunchEvent),
) -> anyhow::Result<()> {
    ensure_prefix_reserved(paths, global, environment, runner, token, None, events)
}

fn ensure_prefix_reserved(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: &Runner,
    token: &Token,
    reservation: Option<&LaunchReservation>,
    mut events: impl FnMut(LaunchEvent),
) -> anyhow::Result<()> {
    let log = paths.runtime_dir(&environment.name).join("create.log");
    match environment.build_prefix_reserved(
        paths,
        global,
        runner,
        false,
        token,
        Output::Log(&log),
        reservation,
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
    let (runner, prefix) = match &launch.host {
        Host::Wine { runner, prefix } => (runner, prefix),
        Host::Proxied { .. } => return Ok(()),
        Host::Native { .. } => {
            let changes = desktop::reconcile_native(paths, global, environment)?;
            if !changes.is_empty() {
                events(LaunchEvent::Desktop(changes));
            }
            return Ok(());
        }
    };
    let mut wanted = Wanted::from_config(&environment.config);
    // Keep component aliases on the builds this launch already locked.
    for guard in &launch.build_guards {
        let lock = guard.lock();
        match lock.kind() {
            crate::build_lock::Kind::Runner => {}
            crate::build_lock::Kind::Dxvk => wanted.dxvk = Some(lock.name().to_owned()),
            crate::build_lock::Kind::Vkd3d => wanted.vkd3d = Some(lock.name().to_owned()),
        }
    }
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

#[allow(clippy::too_many_arguments)]
pub fn start(
    paths: &Paths,
    name: &str,
    overrides: Overrides,
    exe: &Path,
    token: &Token,
    hook_output: hooks::Output,
    winetricks_output: Output<'_>,
    mut events: impl FnMut(LaunchEvent),
) -> anyhow::Result<StartOutcome> {
    let PreparedLaunch {
        global,
        environment,
        launch,
        after,
        mut guard,
        hook_output,
        _reservation,
        folders,
    } = match prepare(paths, name, &overrides, token, hook_output, &mut events) {
        Err(err)
            if matches!(
                err.downcast_ref::<Error>(),
                Some(Error::SteamAlreadyRunning(_))
            ) =>
        {
            return Ok(StartOutcome::SteamAlreadyRunning);
        }
        Err(err) if matches!(err.downcast_ref::<Error>(), Some(Error::AlreadyRunning(_))) => {
            return Ok(StartOutcome::AlreadyRunning);
        }
        result => result?,
    };
    let result: anyhow::Result<StartOutcome> = (|| {
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
                builds: launch
                    .build_guards
                    .iter()
                    .map(|guard| guard.lock().clone())
                    .collect(),
                folders: folders.iter().map(|guard| guard.lock().clone()).collect(),
            },
            runtime_dir: runtime.dir().to_owned(),
            stats_log: paths.stats_log(name),
            stop_timeout: global.stop_timeout,
            log_limit_mb: global.log_limit_mb,
            launch,
            overrides,
            after: after.clone(),
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
            Started::SteamRequested => Ok(StartOutcome::SteamRequested),
            Started::SteamAdopted => Ok(StartOutcome::SteamAdopted),
        }
    })();
    if result.as_ref().is_err_and(|err| {
        !matches!(
            err.downcast_ref::<Error>(),
            Some(Error::AlreadyRunning(_) | Error::DifferentTargetRunning { .. })
        )
    }) {
        let cleanup = Token::new();
        if after.is_some() {
            events(LaunchEvent::AfterHook(cleanup.clone()));
        }
        failed_after(
            paths,
            name,
            after.as_ref(),
            guard.take(),
            global.stop_timeout,
            None,
            &cleanup,
            hook_output,
        );
    }
    result
}

#[cfg(test)]
mod tests;
