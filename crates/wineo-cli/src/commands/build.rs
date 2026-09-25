//! Building the prefix of an environment whose config was written by hand.

use std::path::Path;

use anyhow::anyhow;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;

/// The runner an environment's config names, resolved the way a launch does.
pub fn runner(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> anyhow::Result<Runner> {
    Ok(Runners::new(paths, global)?.resolve(&environment.config.runner)?)
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
