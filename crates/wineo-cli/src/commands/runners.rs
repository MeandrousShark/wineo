use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;

#[derive(clap::Args)]
pub struct Args {
    /// Also list directories that aren't usable as runners, and why
    #[arg(long)]
    all: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let runners = Runners::new(paths, &global)?;
    let listing = runners.list()?;

    // The system wine leads, since that is what every config starts out using.
    let mut rows = vec![match Runner::system() {
        Ok(runner) => (
            Runner::SYSTEM.to_owned(),
            version(&runner),
            runner.wine_path().display().to_string(),
        ),
        Err(err) => (Runner::SYSTEM.to_owned(), err.to_string(), String::new()),
    }];
    rows.extend(
        listing
            .runners
            .iter()
            .map(|runner| (runner.name().to_owned(), version(runner), String::new())),
    );
    if args.all {
        rows.extend(listing.skipped.iter().map(|(name, reason)| {
            (
                name.clone(),
                format!("skipped, {}", reason.as_str()),
                String::new(),
            )
        }));
    }

    let name_width = rows.iter().map(|(name, ..)| name.len()).max().unwrap_or(0);
    let detail_width = rows
        .iter()
        .map(|(_, detail, _)| detail.len())
        .max()
        .unwrap_or(0);
    for (name, detail, path) in &rows {
        println!(
            "{}",
            format!("{name:name_width$}  {detail:detail_width$}  {path}").trim_end()
        );
    }

    if listing.runners.is_empty() {
        eprintln!(
            "No runners installed. Extract one into {}",
            runners.root().display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn version(runner: &Runner) -> String {
    runner
        .version()
        .unwrap_or_else(|| "unknown version".to_owned())
}
