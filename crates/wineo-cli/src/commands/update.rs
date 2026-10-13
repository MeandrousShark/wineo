//! `wineo update`: what GOG offers for the games wineo manages, and
//! applying it to native games.

use std::process::ExitCode;

use clap_complete::ArgValueCompleter;
use serde_json::json;
use wineo_core::gog::update::{self, Batch, Status};
use wineo_core::paths::Paths;

use super::gog_support::{
    Account, dlc_json, dlc_notes, error_json, error_subject, reason_json, reason_note, track_label,
    update_label, warn,
};
use super::table::render_columns;

mod apply;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to check or update
    #[arg(value_name = "NAME", conflicts_with = "all", add = ArgValueCompleter::new(crate::completion::gog_environments))]
    name: Option<String>,
    /// Every GOG game, which is the default for --check
    #[arg(long)]
    all: bool,
    /// List the updates GOG offers and change nothing
    #[arg(long)]
    check: bool,
    /// Rebuild a game from GOG's current files even when nothing changed
    #[arg(long, conflicts_with_all = ["check", "all"])]
    reinstall: bool,
    /// Fetch GOG's game details again instead of using the saved copy
    #[arg(long)]
    refresh: bool,
    /// Skip the confirmation and allow running without a terminal
    #[arg(short, long)]
    yes: bool,
    /// Review the update without changing anything
    #[arg(long)]
    dry_run: bool,
    /// With --check, print JSON
    #[arg(long, requires = "check")]
    json: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    if args.check {
        check(paths, &args)
    } else {
        apply::run(paths, &args)
    }
}

fn check(paths: &Paths, args: &Args) -> anyhow::Result<ExitCode> {
    let account = Account::new();
    let mut session = account.session(paths)?;
    let batch = match &args.name {
        // A named game has no partial result, so its failure is the error.
        Some(name) => Batch {
            statuses: vec![update::check(&mut session, name, args.refresh)?],
            errors: Vec::new(),
        },
        None => update::check_all(&mut session, args.refresh)?,
    };
    let warnings: Vec<&String> = batch
        .statuses
        .iter()
        .flat_map(|status| &status.warnings)
        .collect();
    // On stderr in both modes, and in the JSON for scripts.
    for warning in &warnings {
        warn(warning);
    }
    if args.json {
        super::json::print(&json!({
            "environments": batch.statuses.iter().map(status_json).collect::<Vec<_>>(),
            "errors": batch.errors.iter().map(error_json).collect::<Vec<_>>(),
            "warnings": warnings,
        }))?;
    } else {
        print_table(&batch.statuses);
        for error in &batch.errors {
            eprintln!("Failed {}: {}", error_subject(error), error.message);
        }
        if batch.statuses.is_empty() && batch.errors.is_empty() {
            println!("No GOG games to check.");
        }
    }
    Ok(if batch.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn status_json(status: &Status) -> serde_json::Value {
    json!({
        "name": status.environment,
        "id": status.gog_id.to_string(),
        "track": track_label(status.track),
        "installed": status.installed,
        "available": status.available,
        "update": status.state.as_str(),
        "applicable": status.applicable,
        "reason": reason_json(&status.reason),
        "dlc": dlc_json(&status.dlc),
    })
}

fn print_table(statuses: &[Status]) {
    if statuses.is_empty() {
        return;
    }
    let dash = || "-".to_owned();
    let mut table = vec![["NAME", "INSTALLED", "AVAILABLE", "STATE"].map(str::to_owned)];
    table.extend(statuses.iter().map(|status| {
        [
            status.environment.clone(),
            status.installed.clone().unwrap_or_else(dash),
            status.available.clone().unwrap_or_else(dash),
            update_label(status.state).to_owned(),
        ]
    }));
    let mut lines = render_columns(&table).into_iter();
    println!("{}", lines.next().expect("the header"));
    for (status, line) in statuses.iter().zip(lines) {
        println!("{line}");
        if let Some(reason) = &status.reason {
            println!("  {}", reason_note(reason));
        }
        if status.unfinished {
            println!(
                "  Finish the interrupted update with: wineo update {}",
                status.environment
            );
        }
        for note in dlc_notes(status) {
            println!("{note}");
        }
    }
}
