use std::process::ExitCode;

use anyhow::bail;
use clap::Subcommand;
use wineo_core::environment::Environment;
use wineo_core::paths::Paths;
use wineo_core::trash::{self, Phase};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// List retained entries and unfinished operations
    List,
    /// Return the saved data to its original name and paths
    Restore {
        #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::trash_ids))]
        id: String,
    },
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    match args.action {
        Action::List => {
            for entry in trash::list(paths)? {
                let state = match entry.phase {
                    Phase::Trashed => {
                        if entry.include_prefix {
                            "prefix included"
                        } else {
                            "prefix left in place"
                        }
                    }
                    Phase::Removing => "removal interrupted",
                    Phase::Restoring => "restoration interrupted",
                    Phase::Pruning => "prune interrupted - not restorable",
                };
                println!(
                    "{}  {}  {}  {}",
                    entry.id, entry.name, entry.removed_at, state
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Action::Restore { id } => {
            let entry = trash::inspect(paths, &id)?;
            if entry.phase != Phase::Trashed {
                bail!("entry {id} has interrupted work; run `wineo validate --fix`");
            }
            trash::restore(paths, &id)?;
            println!("Restored environment {} from trash {id}", entry.name);
            if Environment::load(paths, &entry.name).is_err() {
                eprintln!(
                    "Warning: {}'s raw config is still invalid; fix it before desktop entries can be recreated",
                    entry.name
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
