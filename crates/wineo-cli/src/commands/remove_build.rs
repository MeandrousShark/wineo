use std::collections::BTreeSet;
use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::bail;
use wineo_core::build_lock::{Kind, Lock};
use wineo_core::build_removal::{self, Removed};
use wineo_core::paths::Paths;
use wineo_core::report::OverviewError;

use crate::cancel;
use crate::{prompt, style};

pub fn run(
    paths: &Paths,
    name: &str,
    kind: &str,
    build_kind: Kind,
    yes: bool,
    size: impl FnOnce(&Paths, &str) -> anyhow::Result<u64>,
    remove: impl FnOnce(&Paths, &str) -> anyhow::Result<Removed>,
) -> anyhow::Result<ExitCode> {
    let mut warned = BTreeSet::new();
    if !yes {
        if !prompt::interactive() {
            bail!("remove requires a terminal or --yes; nothing was removed");
        }

        let size = size(paths, name)?;
        warnings(
            paths,
            &build_removal::uses(paths, &Lock::new(paths, build_kind, name))?.errors,
            &mut warned,
        );
        print!("Remove {kind} {name} ({})? [y/N] ", style::size(size));
        crate::output::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Canceled. Nothing was removed.");
            return Ok(cancel::exit_code());
        }
    }

    let removed = remove(paths, name)?;
    warnings(paths, &removed.errors, &mut warned);
    println!(
        "Removed {kind} {name} ({} freed)",
        style::size(removed.bytes)
    );
    Ok(ExitCode::SUCCESS)
}

fn warnings(paths: &Paths, errors: &[OverviewError], warned: &mut BTreeSet<String>) {
    for error in errors {
        if !warned.insert(error.name.clone()) {
            continue;
        }
        let path = match error.name.strip_prefix("template ") {
            Some(name) => paths.templates_dir().join(format!("{name}.toml")),
            None => paths.environment_config(&error.name),
        };
        let message = error
            .message
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        eprintln!("warning: skipped {}: {message}", path.display());
    }
}
