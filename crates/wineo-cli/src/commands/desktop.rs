use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::desktop::{self, Change};
use wineo_core::environment::{self, Environment};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;

#[derive(clap::Args)]
pub struct Args {}

pub fn run(paths: &Paths, _args: Args) -> anyhow::Result<ExitCode> {
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
    // There is no verb for these: the environment they named is gone, so
    // nothing wineo can be pointed at owns them any more.
    if !strays.is_empty() {
        eprintln!(
            "{} entr{} belong to an environment that no longer exists. Delete:",
            strays.len(),
            if strays.len() == 1 { "y" } else { "ies" }
        );
        for entry in strays {
            eprintln!("  {}", entry.path.display());
        }
    }
    Ok(ExitCode::SUCCESS)
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

fn describe(change: &Change, name: &str) -> String {
    match change {
        Change::Wrote { target, icon } => {
            let icon = if *icon { "" } else { ", with no icon" };
            format!("Added a desktop entry for {target} in {name}{icon}")
        }
        Change::Updated { target } => format!("Updated the desktop entry for {target} in {name}"),
        Change::Removed { target } => format!("Removed the desktop entry for {target} in {name}"),
        Change::Skipped { target, reason } => {
            format!("No desktop entry for {target} in {name}: {reason}")
        }
    }
}
