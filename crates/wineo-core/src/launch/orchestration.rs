use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow};

use crate::cancel::Token;
use crate::config::{GlobalConfig, Kind};
use crate::desktop;
use crate::environment::Environment;
use crate::error::Error;
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
        if let (Host::Proxied { baseline, .. }, Some(first)) = (&mut launch.host, &steam_baseline) {
            *baseline = first.clone();
        }
        let held: Vec<&Path> = launch.host.prefix().into_iter().collect();
        guard = Some(match &reservation {
            Some(reservation) => reservation.acquire(Operation::Launch, &held)?,
            None => locks.acquire(Operation::Launch, name, &held)?,
        });
        Environment::ensure_exists(paths, name)?;
        runtime.check_resolved(name)?;
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
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn launcher_cleanup_after_writes_to_the_callers_output() {
        for stage in ["prepare", "start", "prepared"] {
            let root = tempfile::tempdir().unwrap();
            let paths = Paths::from_lookup(|key| {
                (key == "HOME").then(|| root.path().as_os_str().to_owned())
            })
            .unwrap();
            let config = paths.environment_config("app");
            fs::create_dir_all(config.parent().unwrap()).unwrap();
            let exe = if stage == "prepare" {
                root.path().join("missing")
            } else {
                PathBuf::from("/bin/sh")
            };
            fs::write(
                config,
                format!(
                    "kind = 'native'\n[hooks]\nbefore = 'echo before'\nafter = 'echo after; echo cleanup-error >&2'\n[targets.app]\nexe = '{}'\n",
                    exe.display()
                ),
            )
            .unwrap();
            let mut capture = tempfile::tempfile().unwrap();
            let output = hooks::Output::File(capture.try_clone().unwrap());
            match stage {
                "prepare" => assert!(
                    prepare(
                        &paths,
                        "app",
                        &Overrides::default(),
                        &Token::new(),
                        output,
                        |_| {},
                    )
                    .is_err()
                ),
                "start" => assert!(
                    start(
                        &paths,
                        "app",
                        Overrides::default(),
                        &root.path().join("missing-supervisor"),
                        &Token::new(),
                        output,
                        Output::Inherit,
                        |_| {},
                    )
                    .is_err()
                ),
                "prepared" => {
                    let mut prepared = prepare(
                        &paths,
                        "app",
                        &Overrides::default(),
                        &Token::new(),
                        output,
                        |_| {},
                    )
                    .unwrap();
                    prepared.failed(&paths, None, &Token::new());
                }
                _ => unreachable!(),
            }
            use std::io::{Read, Seek};
            capture.rewind().unwrap();
            let mut captured = String::new();
            capture.read_to_string(&mut captured).unwrap();
            assert_eq!(captured, "before\nafter\ncleanup-error\n", "{stage}");
        }
    }

    #[test]
    fn reconcile_uses_the_component_build_locked_by_the_launch() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let runner_bin = paths.default_runner_root().join("fake/bin");
        fs::create_dir_all(&runner_bin).unwrap();
        for name in ["wine", "wineserver"] {
            let path = runner_bin.join(name);
            fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let prefix = paths.default_prefix_root().join("app");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        let install_dxvk = |name: &str| {
            for arch in ["x64", "x32"] {
                let dir = paths.default_dxvk_root().join(name).join(arch);
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("dxgi.dll"), name).unwrap();
            }
        };
        install_dxvk("1.0");
        let path = paths.environment_config("app");
        let environment = Environment {
            name: "app".to_owned(),
            path: path.clone(),
            config: crate::config::EnvironmentConfig::parse(
                &path,
                &format!(
                    "prefix = '{}'\nrunner = 'fake'\narch = 'win64'\ndxvk = 'latest'\n[targets.app]\nexe = 'app'\n",
                    prefix.display()
                ),
            )
            .unwrap(),
        };
        let global = GlobalConfig::default();
        let launch = Launch::target(&paths, &global, &environment, &Overrides::default()).unwrap();
        install_dxvk("2.0");
        reconcile(&paths, &global, &environment, &launch, |_| {}).unwrap();
        assert_eq!(
            install::installed(
                launch.host.runner().unwrap(),
                &prefix,
                crate::components::Component::Dxvk
            )
            .unwrap()
            .as_deref(),
            Some("1.0")
        );
        assert!(
            crate::build_lock::try_exclusive(&paths, crate::build_lock::Kind::Dxvk, "1.0")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn failed_preparation_waits_for_same_name_shared_users_before_after() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let locks = Locks::new(&paths);
        let held = locks.acquire(Operation::Launch, "app", &[]).unwrap();
        let captured = root.path().join("after");
        let after = AfterHook {
            steam: None,
            paths: paths.clone(),
            environment: "app".to_owned(),
            target: "app".to_owned(),
            exe: "missing".to_owned(),
            wine: None,
            command: format!("printf '%s\\n' \"$WINEO_END\" >> '{}'", captured.display()),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            failed_after(
                &paths,
                "app",
                Some(&after),
                None,
                1,
                None,
                &Token::new(),
                hooks::Output::Inherit,
            );
            sender.send(()).unwrap();
        });
        let result = receiver.recv_timeout(Duration::from_millis(250));
        let admission = locks.acquire(Operation::Launch, "app", &[]);
        let blocked = matches!(
            admission,
            Err(Error::Busy {
                holder: Some(Operation::Hook(Phase::After)),
                ..
            })
        );
        drop(admission);
        drop(held);
        worker.join().unwrap();
        assert!(
            matches!(result, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
            "after must wait for the existing shared user"
        );
        assert!(
            blocked,
            "new shared users must not enter while cleanup waits"
        );
        assert_eq!(fs::read_to_string(captured).unwrap(), "failed\n");
    }

    #[test]
    fn launcher_reservation_blocks_exclusive_operations_between_before_and_failed_after() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        let locks = Locks::new(&paths);
        let reservation = locks.reserve_launch("app").unwrap();
        let before = reservation
            .acquire(Operation::Hook(Phase::Before), &[])
            .unwrap();
        let context = hooks::Context {
            steam: None,
            environment: "app",
            target: "app",
            exe: "missing",
            wine: None,
            hook: Hook::Before,
        };
        assert!(matches!(
            hooks::run(
                "true",
                &paths,
                &context,
                &before,
                &Token::new(),
                Duration::from_secs(1),
                hooks::Output::Inherit,
            ).unwrap(),
            hooks::Outcome::Finished(status) if status.success()
        ));
        drop(before);
        let config = paths.environment_config("app");
        let environment = Environment {
            name: "app".to_owned(),
            path: config.clone(),
            config: crate::config::EnvironmentConfig::parse(
                &config,
                &format!(
                    "kind = 'native'\n[targets.app]\nexe = '{}'\n",
                    root.path().join("missing").display()
                ),
            )
            .unwrap(),
        };
        assert!(
            prepare_target_reserved(
                &paths,
                &GlobalConfig::default(),
                &environment,
                &Overrides::default(),
                &Token::new(),
                Some(&reservation),
                |_| {},
            )
            .is_err()
        );
        // Early preparation failures have no operation guard to protect cleanup.
        for operation in [Operation::Snapshot, Operation::Winetricks] {
            assert!(matches!(
                locks.acquire(operation, "app", &[]),
                Err(Error::Busy { .. })
            ));
        }
        let captured = root.path().join("after");
        let after = AfterHook {
            steam: None,
            paths: paths.clone(),
            environment: "app".to_owned(),
            target: "app".to_owned(),
            exe: "missing".to_owned(),
            wine: None,
            command: format!("printf '%s\\n' \"$WINEO_END\" >> '{}'", captured.display()),
        };
        failed_after(
            &paths,
            "app",
            Some(&after),
            None,
            1,
            None,
            &Token::new(),
            hooks::Output::Inherit,
        );
        assert_eq!(fs::read_to_string(captured).unwrap(), "failed\n");
        assert!(matches!(
            locks.acquire(Operation::Snapshot, "app", &[]),
            Err(Error::Busy { .. })
        ));
        drop(reservation);
        locks.acquire(Operation::Snapshot, "app", &[]).unwrap();
    }

    #[test]
    fn detached_supervisor_failures_run_after_once_in_the_launching_process() {
        for reply in [None, Some("failed spawn failed"), Some("no report")] {
            let root = tempfile::tempdir().unwrap();
            let paths = Paths::from_lookup(|key| {
                (key == "HOME").then(|| root.path().as_os_str().to_owned())
            })
            .unwrap();
            let captured = root.path().join("after");
            let command = format!(
                "printf '%s|%s|%s|%s\\n' \"$WINEO_END\" \"$WINEO_EXIT_CODE\" \"$WINEO_SECONDS\" \"$PPID\" >> '{}'",
                captured.display()
            );
            let config = paths.environment_config("app");
            fs::create_dir_all(config.parent().unwrap()).unwrap();
            fs::write(
                &config,
                format!(
                    "kind = 'native'\n[hooks]\nbefore = 'true'\nafter = {}\n[targets.app]\nexe = '/bin/sh'\n",
                    toml::Value::String(command)
                ),
            ).unwrap();
            let exe = root.path().join("supervisor");
            if let Some(reply) = reply {
                fs::write(
                    &exe,
                    format!("#!/bin/sh\ncat >/dev/null\nprintf '{reply}\\n'\n"),
                )
                .unwrap();
                fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
            }
            assert!(
                start(
                    &paths,
                    "app",
                    Overrides::default(),
                    &exe,
                    &Token::new(),
                    hooks::Output::Inherit,
                    Output::Inherit,
                    |_| {},
                )
                .is_err()
            );
            assert_eq!(
                fs::read_to_string(captured).unwrap(),
                format!("failed||0|{}\n", std::process::id())
            );
            assert!(Locks::new(&paths).busy_with("app").unwrap().is_none());
            assert!(!Runtime::new(&paths, "app").is_running().unwrap());
        }
    }

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
                hooks::Output::Inherit,
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
