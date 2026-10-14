use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::desktop::{self, Change};
use wineo_core::environment::{self, Environment};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;

use crate::text;

#[derive(clap::Args)]
pub struct Args {}

fn line(entry: &desktop::Entry, stray: bool, environments: usize, targets: usize) -> String {
    format!(
        "{}  {}  {}  {}",
        text::pad(&entry.environment, environments),
        text::pad(&entry.target, targets),
        entry.name,
        if stray { "stray" } else { "" }
    )
    .trim_end()
    .to_owned()
}

pub fn run(paths: &Paths, _args: Args) -> anyhow::Result<ExitCode> {
    desktop::supported(paths)?;
    let entries = desktop::list(paths)?;
    let known: BTreeSet<String> = environment::names(paths)?.into_iter().collect();

    let width = |column: fn(&desktop::Entry) -> &str| text::widest(entries.iter().map(column));
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
        println!("{}", line(entry, stray, environments, targets));
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
            text::plural(strays.len(), "y", "ies")
        );
        for entry in strays {
            eprintln!("  {}", entry.path.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Writes the entries a config now asks for and says what changed.
pub fn apply(paths: &Paths, name: &str, held: bool) -> anyhow::Result<()> {
    for change in desktop::apply(paths, name, held)? {
        eprintln!("{}", describe(&change, name));
    }
    Ok(())
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

pub(crate) fn describe(change: &Change, name: &str) -> String {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn lines_pad_environment_and_target_by_display_width() {
        let entry = |environment: &str, target: &str| desktop::Entry {
            environment: environment.to_owned(),
            target: target.to_owned(),
            name: "Name".to_owned(),
            path: PathBuf::new(),
        };
        let wide = entry("\u{539f}\u{795e}", "\u{6e38}\u{620f}");
        let plain = entry("abcd", "xy");
        let (environments, targets) = (
            text::widest([wide.environment.as_str(), plain.environment.as_str()]),
            text::widest([wide.target.as_str(), plain.target.as_str()]),
        );
        assert_eq!(
            line(&wide, false, environments, targets),
            "\u{539f}\u{795e}  \u{6e38}\u{620f}  Name"
        );
        assert_eq!(
            line(&plain, true, environments, targets),
            "abcd  xy    Name  stray"
        );
    }
}
