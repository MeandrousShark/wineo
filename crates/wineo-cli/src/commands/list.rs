use std::process::ExitCode;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wineo_core::paths::Paths;

use super::table;

#[derive(clap::Args)]
pub struct Args;

pub fn run(paths: &Paths, _args: Args) -> anyhow::Result<ExitCode> {
    let (rows, failed) = table::gather(paths, Timestamp::now(), &TimeZone::system(), |_| true)?;
    for line in table::render(&rows) {
        println!("{line}");
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
