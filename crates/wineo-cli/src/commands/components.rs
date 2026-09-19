use std::path::Path;

use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::install::{self, Change, Wanted};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;

/// Brings a prefix in line with what its config asks for, and says what it
/// changed. Every caller holds the environment's lock, since this writes into
/// the prefix. A prefix that already matches is untouched and silent.
pub fn reconcile(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    runner: &Runner,
    prefix: &Path,
    wanted: &Wanted,
) -> anyhow::Result<()> {
    for change in install::reconcile(paths, global, name, runner, prefix, wanted)? {
        eprintln!("{}", describe(&change, name));
    }
    Ok(())
}

/// The same, for the callers that want exactly what the config already says.
pub fn reconcile_config(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: &Runner,
    prefix: &Path,
) -> anyhow::Result<()> {
    let wanted = Wanted::from_config(&environment.config);
    reconcile(paths, global, &environment.name, runner, prefix, &wanted)
}

fn describe(change: &Change, name: &str) -> String {
    let label = |component: &Component| component.label();
    match change {
        Change::Installed { component, build } => {
            format!("Installed {} {build} into {name}", label(component))
        }
        Change::Replaced {
            component,
            from,
            to,
        } => format!("Replaced {} {from} with {to} in {name}", label(component)),
        Change::Removed { component, build } => {
            format!("Removed {} {build} from {name}", label(component))
        }
    }
}
