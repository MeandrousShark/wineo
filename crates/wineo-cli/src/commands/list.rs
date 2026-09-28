use std::process::ExitCode;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wineo_core::paths::Paths;
use wineo_core::report::{self, OverviewFilter};

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
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let tags = (!args.tags.is_empty()).then_some(args.tags.as_slice());
    let now = Timestamp::now();
    let tz = TimeZone::system();
    let overview = report::overview(
        paths,
        OverviewFilter {
            tags,
            running_only: false,
        },
    )?;
    let gathered = table::format_overview(overview, now, &tz);
    if args.json {
        json::print(&gathered.json())?;
    } else {
        gathered.print_errors();
        for line in table::render(&gathered.rows) {
            println!("{line}");
        }
        if gathered.rows.is_empty() && !args.tags.is_empty() {
            println!("No environments are tagged {}", args.tags.join(" and "));
        }
    }
    Ok(if gathered.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
