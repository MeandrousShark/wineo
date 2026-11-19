use std::process::ExitCode;

use jiff::Timestamp;
use wineo_core::paths::Paths;
use wineo_core::report::{self, OverviewFilter};
use wineo_core::search::{Field, Search};
use wineo_core::time::LocalZone;

use super::json;
use super::table;

#[derive(clap::Args)]
pub struct Args {
    /// Show environments with every given tag
    #[arg(
        long = "tag",
        value_name = "TAG",
        add = clap_complete::ArgValueCompleter::new(crate::completion::tags)
    )]
    tags: Vec<String>,
    /// Print machine-readable JSON
    #[arg(long)]
    json: bool,
    /// Show environments whose title, name, or a tag matches these words
    #[arg(value_name = "SEARCH")]
    search: Vec<String>,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let tags = (!args.tags.is_empty()).then_some(args.tags.as_slice());
    let now = Timestamp::now();
    let tz = LocalZone::new();
    let mut overview = report::overview(
        paths,
        OverviewFilter {
            tags,
            running_only: false,
        },
    )?;
    let typed = args.search.join(" ");
    let typed = typed.trim();
    let search = Search::new(typed);
    // Errors stay whatever the search, so a broken config still fails the run.
    if !search.is_empty() {
        overview.environments.retain(|environment| {
            [&environment.title, &environment.name]
                .into_iter()
                .chain(&environment.tags)
                .any(|text| search.matches(&Field::new(text)))
        });
    }
    let gathered = table::format_overview(overview, now, &tz);
    if args.json {
        json::print(&gathered.json())?;
    } else {
        gathered.print_errors();
        for line in table::render(&gathered.rows) {
            println!("{line}");
        }
        if gathered.rows.is_empty() && !search.is_empty() {
            if args.tags.is_empty() {
                println!("No environments match \"{typed}\".");
            } else {
                println!(
                    "No environments tagged {} match \"{typed}\".",
                    args.tags.join(" and ")
                );
            }
        } else if gathered.rows.is_empty() && !args.tags.is_empty() {
            println!("No environments are tagged {}", args.tags.join(" and "));
        } else if gathered.rows.is_empty() && !gathered.failed() {
            // On stderr like `status`, so piped output stays empty.
            eprintln!("No environments. Create one with: wineo new");
        }
    }
    Ok(if gathered.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
