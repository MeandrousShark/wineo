use std::process::ExitCode;

use clap::Subcommand;
use wineo_core::paths::Paths;
use wineo_core::trash::{self, Phase};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// List retained entries and unfinished operations
    List,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    match args.action {
        None | Some(Action::List) => {
            let now = jiff::Timestamp::now();
            let rows: Vec<_> = trash::list(paths)?
                .into_iter()
                .map(|entry| {
                    // A damaged manifest should still list, so show its raw time
                    let removed = match entry.removed_at.parse::<jiff::Timestamp>() {
                        Ok(at) => crate::commands::time::relative(now, at),
                        Err(_) => entry.removed_at.clone(),
                    };
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
                    [
                        entry.id,
                        entry.name,
                        format!("removed {removed}"),
                        state.to_owned(),
                    ]
                })
                .collect();
            for line in super::table::render_columns(&rows) {
                println!("{line}");
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
