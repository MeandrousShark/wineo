use std::process::ExitCode;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wineo_core::paths::Paths;

use super::json;
use super::table;

#[derive(clap::Args)]
pub struct Args {
    /// Print machine-readable JSON
    #[arg(long)]
    json: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let gathered = table::gather(paths, Timestamp::now(), &TimeZone::system(), |_, _| true)?;
    if args.json {
        json::print(&gathered.json())?;
    } else {
        gathered.print_errors();
        for line in table::render(&gathered.rows) {
            println!("{line}");
        }
    }
    Ok(if gathered.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
