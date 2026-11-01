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
        // A build in use, by a reference or a held lock, still gets its usual
        // refusal from removal itself.
        let lock = Lock::new(paths, build_kind, name);
        let uses = build_removal::uses(paths, &lock)?;
        if uses.labels.is_empty() && lock.try_exclusive()?.is_some() {
            build_removal::refuse_unreadable(name, &uses.errors)?;
        }
        let question = format!("Remove {kind} {name} ({})?", style::size(size));
        if !prompt::confirm_once(&question, false)? {
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
