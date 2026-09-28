//! Building the prefix of an environment whose config was written by hand.

use std::path::Path;

use anyhow::anyhow;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::installer::Output;
use wineo_core::launch::{Host, Launch, Overrides};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;
use wineo_core::runtime::Runtime;
use wineo_core::winetricks;

/// The runner an environment's config names, resolved the way a launch does.
pub fn runner(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> anyhow::Result<Runner> {
    Ok(Runners::new(paths, global)?.resolve(&environment.wine()?.runner)?)
}

/// What `start`, `run`, and `restart` check before anything else, building a
/// missing prefix. A native environment has no prefix to build.
pub fn prepare_target(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    overrides: &Overrides,
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
    ensure_prefix(paths, global, environment, &runner)
}

/// Brings the prefix's components and the desktop entries in line with the
/// config, under the launch's lock.
pub fn reconcile(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    launch: &Launch,
) -> anyhow::Result<()> {
    let Host::Wine { runner, prefix } = &launch.host else {
        return crate::commands::desktop::reconcile_native(paths, global, environment);
    };
    crate::commands::components::reconcile_config(paths, global, environment, runner, prefix)?;
    crate::commands::desktop::reconcile(paths, global, environment, Some(runner), prefix)
}

/// Called under the launch lock after the running check and reconciliation.
pub fn install_winetricks(
    global: &GlobalConfig,
    environment: &Environment,
    launch: &Launch,
    overrides: &Overrides,
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
    let cancellation = super::transfer::Cancellation::new()?;
    winetricks::install_missing(
        &environment.name,
        runner,
        prefix,
        &env,
        &environment.config.winetricks,
        global.stop_timeout,
        &cancellation.token,
        Output::Inherit,
        super::winetricks::print_event,
    )
}

/// What is printed right before a prefix is built, since that takes a while.
pub fn creating_note(paths: &Paths, name: &str, runner: &Runner, dir: &Path) -> String {
    let dir = paths
        .contract(dir)
        .unwrap_or_else(|_| dir.display().to_string());
    if runner.is_proton() {
        format!("Creating a Proton prefix for {name} in {dir}, this takes a while")
    } else {
        format!("Creating a wine prefix for {name} in {dir}, this takes a few seconds")
    }
}

/// Builds the prefix when it is missing and safe to build, for a command about
/// to hand it to wine. The note goes to stderr so `--json` output stays clean.
pub fn ensure_prefix(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: &Runner,
) -> anyhow::Result<()> {
    let name = &environment.name;
    match environment.build_prefix(paths, global, runner, false, |dir| {
        eprintln!("{}", creating_note(paths, name, runner, dir));
    }) {
        Ok(_) => Ok(()),
        // Only setup builds there, after asking.
        Err(err @ (Error::EmptyPrefix(_) | Error::EmptyPrefixParent { .. })) => Err(anyhow!(
            "{err}. Run `wineo setup {name}` to create it there."
        )),
        Err(err) => Err(err.into()),
    }
}
