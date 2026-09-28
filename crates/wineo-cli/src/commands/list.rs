use std::process::ExitCode;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wineo_core::paths::Paths;

use super::json;
use super::table;

#[derive(clap::Args)]
pub struct Args {
    /// Show environments with every given tag
    #[arg(long = "tag", add = clap_complete::ArgValueCompleter::new(crate::completion::tags))]
    tags: Vec<String>,
    /// Print machine-readable JSON
    #[arg(long)]
    json: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let tags = (!args.tags.is_empty()).then_some(args.tags.as_slice());
    let gathered = table::gather(
        paths,
        Timestamp::now(),
        &TimeZone::system(),
        tags,
        |_, _| true,
    )?;
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
