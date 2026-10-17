use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::bail;
use wineo_core::build_lock::{Kind, Lock};
use wineo_core::build_removal::{self, Removed};
use wineo_core::paths::Paths;

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
    if !yes {
        if !prompt::interactive() {
            bail!("remove requires a terminal or --yes; nothing was removed");
        }

        let size = size(paths, name)?;
        // Fail before the prompt, since confirming could not change the answer.
        // A build in use still gets its usual refusal from removal itself.
        let uses = build_removal::uses(paths, &Lock::new(paths, build_kind, name))?;
        if uses.labels.is_empty() {
            build_removal::refuse_unreadable(name, &uses.errors)?;
        }
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
    println!(
        "Removed {kind} {name} ({} freed)",
        style::size(removed.bytes)
    );
    Ok(ExitCode::SUCCESS)
}
