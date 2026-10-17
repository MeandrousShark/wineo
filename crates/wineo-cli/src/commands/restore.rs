use std::process::ExitCode;

use anyhow::bail;
use wineo_core::environment::Environment;
use wineo_core::paths::Paths;
use wineo_core::trash::{self, Phase};

#[derive(clap::Args)]
pub struct Args {
    /// Trash entry ID
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::trash_ids))]
    id: String,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let entry = trash::inspect(paths, &args.id)?;
    if entry.phase != Phase::Trashed {
        bail!(
            "entry {} has interrupted work; run `wineo validate --fix`",
            args.id
        );
    }
    for warning in trash::restore(paths, &args.id)? {
        eprintln!("warning: {warning}");
    }
    println!("Restored environment {} from trash {}", entry.name, args.id);
    if Environment::load(paths, &entry.name).is_err() {
        eprintln!(
            "Warning: {}'s raw config is still invalid; fix it before desktop entries can be recreated",
            entry.name
        );
    }
    Ok(ExitCode::SUCCESS)
}
