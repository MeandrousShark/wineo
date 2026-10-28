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
            let survey = trash::survey(paths)?;
            let mut rows: Vec<_> = survey
                .entries
                .into_iter()
                .map(|entry| {
                    // A damaged manifest should still list, so show its raw time
                    let removed = match entry.removed_at.parse::<jiff::Timestamp>() {
                        Ok(at) => wineo_core::time::relative(now, at),
                        Err(_) => entry.removed_at.clone(),
                    };
                    let state = match entry.phase {
                        Phase::Trashed if entry.prefix_present != Some(true) => "",
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
            rows.extend(survey.unreadable.into_iter().map(|entry| {
                [
                    entry.id,
                    entry.blocks.clone().unwrap_or_else(|| "-".to_owned()),
                    "unreadable".to_owned(),
                    format!("{}; see `wineo validate`", entry.reason),
                ]
            }));
            if rows.is_empty() {
                eprintln!("Trash is empty");
            }
            for line in super::table::render_columns(&rows) {
                println!("{line}");
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
