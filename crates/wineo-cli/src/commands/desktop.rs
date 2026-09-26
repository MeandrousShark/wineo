use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::desktop::{self, Change};
use wineo_core::environment::{self, Environment};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;
use wineo_core::runtime::Runtime;

#[derive(clap::Args)]
pub struct Args {}

pub fn run(paths: &Paths, _args: Args) -> anyhow::Result<ExitCode> {
    desktop::supported(paths)?;
    let entries = desktop::list(paths)?;
    let known: BTreeSet<String> = environment::names(paths)?.into_iter().collect();

    let width = |column: fn(&desktop::Entry) -> &str| {
        entries
            .iter()
            .map(|entry| column(entry).len())
            .max()
            .unwrap_or(0)
    };
    let (environments, targets) = (
        width(|entry| entry.environment.as_str()),
        width(|entry| entry.target.as_str()),
    );

    let mut strays = Vec::new();
    for entry in &entries {
        let stray = !known.contains(&entry.environment);
        if stray {
            strays.push(entry);
        }
        let line = format!(
            "{:environments$}  {:targets$}  {}  {}",
            entry.environment,
            entry.target,
            entry.name,
            if stray { "stray" } else { "" }
        );
        println!("{}", line.trim_end());
    }

    if entries.is_empty() {
        eprintln!(
            "No desktop entries. Add one with: wineo set <name> targets.<target>.desktop=true"
        );
    }
    if !strays.is_empty() {
        eprintln!(
            "{} entr{} belong to an environment that no longer exists. Remove with `wineo validate`:",
            strays.len(),
            if strays.len() == 1 { "y" } else { "ies" }
        );
        for entry in strays {
            eprintln!("  {}", entry.path.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Writes the entries a config now asks for. Run after the write, so an entry
/// is built from the config as it ends up rather than as it was. `held` says
/// this command already has the environment to itself.
pub fn apply(paths: &Paths, name: &str, held: bool) -> anyhow::Result<()> {
    let Ok(environment) = Environment::load(paths, name) else {
        return Ok(());
    };
    if environment.config.wine().is_none() {
        let global = GlobalConfig::load_or_default(paths)?;
        return reconcile_native(paths, &global, &environment);
    }
    let Ok(prefix) = environment.prefix(paths) else {
        return Ok(());
    };
    // Pulling out an icon runs wine in the prefix, which has to wait for
    // whatever holds it exclusively, like an installer, and for what an
    // unresolved one may have left running. The entry is still written, and
    // gets its icon on a later launch.
    let launch = (!held)
        .then(|| {
            Locks::new(paths)
                .acquire(Operation::Launch, name, &[&prefix])
                .ok()
        })
        .flatten();
    let may_run_wine =
        held || (launch.is_some() && Runtime::new(paths, name).check_resolved(name).is_ok());
    let global = GlobalConfig::load_or_default(paths)?;
    let runner = may_run_wine
        .then(|| Runners::new(paths, &global).ok())
        .flatten()
        .and_then(|runners| runners.resolve(&environment.wine().ok()?.runner).ok());
    reconcile(paths, &global, &environment, runner.as_ref(), &prefix)
}

/// Brings an environment's desktop entries in line with its config and says
/// what changed. Called from every path that opens an environment, so staying
/// silent when nothing changed is what keeps it out of the way.
pub fn reconcile(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: &Path,
) -> anyhow::Result<()> {
    for change in desktop::reconcile(paths, global, environment, runner, prefix)? {
        eprintln!("{}", describe(&change, &environment.name));
    }
    Ok(())
}

/// `reconcile` for a native environment, which has no runner or prefix.
pub fn reconcile_native(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> anyhow::Result<()> {
    for change in desktop::reconcile_native(paths, global, environment)? {
        eprintln!("{}", describe(&change, &environment.name));
    }
    Ok(())
}

fn describe(change: &Change, name: &str) -> String {
    match change {
        Change::Wrote { target, icon } => {
            let icon = if *icon { "" } else { ", with no icon" };
            format!("Added a desktop entry for {target} in {name}{icon}")
        }
        Change::Updated { target } => format!("Updated the desktop entry for {target} in {name}"),
        Change::Removed { target } => format!("Removed the desktop entry for {target} in {name}"),
        Change::IconArtUnreadable { target } => {
            format!("Icon art for {target} in {name} could not be read")
        }
        Change::Skipped { target, reason } => {
            format!("No desktop entry for {target} in {name}: {reason}")
        }
    }
}
