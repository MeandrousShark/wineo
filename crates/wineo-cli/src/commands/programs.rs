use std::process::ExitCode;

use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::paths::Paths;
use wineo_core::prefix;
use wineo_core::programs::{self, Listing};

#[derive(clap::Args)]
pub struct Args {
    /// Environment to look in
    name: String,

    /// Also list the executables held back as noise, like uninstallers
    #[arg(long)]
    all: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    // The config is read but the runner is not resolved, since listing what is
    // installed shouldn't depend on the runner still being there.
    let environment = Environment::load(paths, &args.name)?;
    let dir = environment.prefix(paths)?;
    let wine_prefix = prefix::wine_prefix_in(&dir).ok_or(Error::MissingPrefix(dir))?;

    let listing = programs::scan(&wine_prefix)?;
    report(&args.name, &listing, args.all);
    Ok(ExitCode::SUCCESS)
}

/// Prints what a scan found, and how to turn one of them into a target.
/// Shared with `new --install-exe`, which scans once the installer is done.
pub fn report(name: &str, listing: &Listing, all: bool) {
    let rows = if all {
        listing.all()
    } else {
        listing.programs.clone()
    };
    let width = |column: fn(&programs::Program) -> &str| {
        rows.iter().map(|row| column(row).len()).max().unwrap_or(0)
    };
    let (sources, names) = (
        width(|row| row.source.as_str()),
        width(|row| row.name.as_str()),
    );
    for row in &rows {
        println!(
            "{:sources$}  {:names$}  {}",
            row.source.as_str(),
            row.name,
            row.exe
        );
    }

    if rows.is_empty() {
        eprintln!("Nothing is installed in {name}");
    }
    // Each of the notes below explains a program someone expected to see and
    // didn't, so they come before the hint rather than after the rows.
    for (shortcut, reason) in &listing.unreadable {
        eprintln!("Skipped the shortcut {shortcut}: {reason}");
    }
    if !all && !listing.filtered.is_empty() {
        let held = listing.filtered.len();
        let plural = if held == 1 { "" } else { "s" };
        eprintln!("{held} executable{plural} looked like noise. Pass --all to see them");
    }
    if let Some(row) = rows.first() {
        eprintln!(
            "Add one with: wineo set {name} targets.<target>.exe='{}'",
            row.exe
        );
    }
}
