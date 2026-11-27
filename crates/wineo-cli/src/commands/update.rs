//! `wineo update`: what GOG and Epic offer for the games wineo manages, and
//! applying it.

use std::process::ExitCode;

use clap_complete::ArgValueCompleter;
use serde_json::json;
use wineo_core::gog::update::galaxy::Choice;
use wineo_core::gog::update::{self, Batch, GameError, Status};
use wineo_core::paths::Paths;

use super::galaxy::GogTrack;
use super::gog_support::{
    Account, LOGGED_OUT, dlc_json, dlc_notes, error_json, reason_json, state_label, usage_error,
    warn,
};
use super::table::render_columns;

mod apply;
mod builds;
mod epic;

#[derive(clap::Args)]
#[command(group = clap::ArgGroup::new("listing").args(["check", "builds"]))]
pub struct Args {
    /// Environment to check or update
    #[arg(value_name = "NAME", conflicts_with = "all", add = ArgValueCompleter::new(crate::completion::store_environments))]
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
    /// With --reinstall, the track to put the game on
    #[arg(long, value_enum, requires = "reinstall", conflicts_with = "all")]
    track: Option<GogTrack>,
    /// Check every file of a Galaxy game and fix what is missing or damaged
    #[arg(long, conflicts_with_all = ["reinstall", "track", "check", "all"])]
    repair: bool,
    /// Follow this Galaxy branch from now on, and update from it
    #[arg(
        long,
        value_name = "BRANCH",
        conflicts_with_all = ["default_branch", "build", "latest", "repair", "all", "check"]
    )]
    branch: Option<String>,
    /// With --branch, read the branch's password from standard input
    #[arg(long, requires = "branch", conflicts_with_all = ["all", "check", "builds"])]
    password_stdin: bool,
    /// Return to the default Galaxy branch, and update from it
    #[arg(
        long,
        conflicts_with_all = ["branch", "build", "latest", "repair", "all", "check"]
    )]
    default_branch: bool,
    /// Install one of the builds --builds lists, by id or version name, and stay on it
    #[arg(
        long,
        value_name = "BUILD",
        conflicts_with_all = ["branch", "default_branch", "latest", "repair", "all", "check"]
    )]
    build: Option<String>,
    /// Stop staying on a build, and update to the latest of the branch followed
    #[arg(
        long,
        conflicts_with_all = ["branch", "default_branch", "build", "repair", "all", "check"]
    )]
    latest: bool,
    /// List the builds GOG offers for a Galaxy game and change nothing
    #[arg(
        long,
        requires = "name",
        conflicts_with_all = [
            "all", "reinstall", "track", "repair", "yes", "dry_run", "branch",
            "default_branch", "build", "latest",
        ]
    )]
    builds: bool,
    /// Fetch GOG's game details again instead of using the saved copy
    #[arg(long)]
    refresh: bool,
    /// Skip the confirmation and allow running without a terminal
    #[arg(short, long)]
    yes: bool,
    /// Review the update without changing anything
    #[arg(long)]
    dry_run: bool,
    /// With --check or --builds, print JSON
    #[arg(long, requires = "listing")]
    json: bool,
}

impl Args {
    /// The branch or build the flags ask for, and the flag that did.
    fn choice(&self) -> (Choice, Option<&'static str>) {
        if let Some(branch) = &self.branch {
            (Choice::Branch(branch.clone()), Some("--branch"))
        } else if self.default_branch {
            (Choice::DefaultBranch, Some("--default-branch"))
        } else if let Some(build) = &self.build {
            (Choice::Build(build.clone()), Some("--build"))
        } else if self.latest {
            (Choice::Latest, Some("--latest"))
        } else {
            (Choice::Keep, None)
        }
    }
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    // clap lets this through beside the flags that conflict with --branch.
    if args.password_stdin && args.branch.is_none() {
        return Ok(usage_error("--password-stdin needs --branch."));
    }
    // A branch is Galaxy's; clap can't tell this value from --track galaxy.
    if let (Some(GogTrack::Installer), (_, Some(flag))) = (args.track, args.choice()) {
        return Ok(usage_error(format!(
            "{flag} is for Galaxy games, so leave out --track installer."
        )));
    }
    if let Some(name) = &args.name
        && epic::is_epic(paths, name)
    {
        if let Some(flag) = epic::gog_flag(&args) {
            anyhow::bail!("{flag} is for GOG games.");
        }
        return if args.check {
            epic::check_one(paths, &args, name)
        } else {
            apply::run(paths, &args)
        };
    }
    if args.builds {
        builds::run(paths, &args)
    } else if args.check {
        check(paths, &args)
    } else {
        apply::run(paths, &args)
    }
}

fn check(paths: &Paths, args: &Args) -> anyhow::Result<ExitCode> {
    if args.name.is_none() && epic::has_games(paths)? {
        return check_stores(paths, args);
    }
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
        print_table(&batch.statuses.iter().map(Row::Gog).collect::<Vec<_>>());
        for error in &batch.errors {
            eprintln!("Failed {}: {}", error.subject(), error.message);
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

/// `update --check` with Epic games beside GOG's. Logged out of one store,
/// its games are errors and the other's are checked.
fn check_stores(paths: &Paths, args: &Args) -> anyhow::Result<ExitCode> {
    let account = Account::new();
    let gog = match account.open(paths)? {
        Some(mut session) => update::check_all(&mut session, args.refresh)?,
        None => logged_out_of_gog(paths)?,
    };
    let (offered, epic_warnings) = epic::offered(paths, args.refresh)?;
    let epic = epic::check_all(paths, &offered)?;
    let mut warnings: Vec<&String> = gog
        .statuses
        .iter()
        .flat_map(|status| &status.warnings)
        .collect();
    warnings.extend(&epic_warnings);
    for warning in &warnings {
        warn(warning);
    }
    let mut rows: Vec<Row> = gog.statuses.iter().map(Row::Gog).collect();
    rows.extend(epic.statuses.iter().map(Row::Epic));
    rows.sort_by(|a, b| a.name().cmp(b.name()));
    let mut errors: Vec<(Option<&str>, serde_json::Value, String)> = gog
        .errors
        .iter()
        .map(|error| (error.environment.as_deref(), error_json(error), error.subject()))
        .collect();
    errors.extend(epic.errors.iter().map(|error| {
        (
            Some(error.environment.as_str()),
            epic::error_json(error),
            error.environment.clone(),
        )
    }));
    errors.sort_by(|a, b| a.0.cmp(&b.0));
    if args.json {
        super::json::print(&json!({
            "environments": rows.iter().map(Row::json).collect::<Vec<_>>(),
            "errors": errors.iter().map(|(_, json, _)| json).collect::<Vec<_>>(),
            "warnings": warnings,
        }))?;
    } else {
        print_table(&rows);
        for (_, json, subject) in &errors {
            eprintln!(
                "Failed {subject}: {}",
                json["message"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Each GOG game as an error that says to log in, for a check that goes on
/// with Epic's games.
fn logged_out_of_gog(paths: &Paths) -> anyhow::Result<Batch> {
    let mut batch = Batch::default();
    for name in wineo_core::environment::names(paths)? {
        if let Ok(Some(id)) = update::recorded_gog_id(paths, &name) {
            batch.errors.push(GameError {
                environment: Some(name),
                id: id.parse().ok(),
                message: LOGGED_OUT.to_owned(),
            });
        }
    }
    Ok(batch)
}

/// A row of the check, from either store.
enum Row<'a> {
    Gog(&'a Status),
    Epic(&'a wineo_core::epic::update::Status),
}

impl Row<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Gog(status) => &status.environment,
            Self::Epic(status) => &status.environment,
        }
    }

    fn json(&self) -> serde_json::Value {
        match self {
            Self::Gog(status) => status_json(status),
            Self::Epic(status) => epic::status_json(status),
        }
    }
}

fn status_json(status: &Status) -> serde_json::Value {
    json!({
        "name": status.environment,
        "id": status.gog_id.to_string(),
        "track": status.track.as_str(),
        "installed": status.installed,
        "available": status.available,
        "branch": status.branch,
        "pinned": status.pinned,
        "update": status.state.as_str(),
        "applicable": status.applicable,
        "reason": reason_json(&status.reason),
        "unfinished": status.unfinished,
        "dlc": dlc_json(&status.dlc),
    })
}

fn print_table(rows: &[Row]) {
    if rows.is_empty() {
        return;
    }
    // A Galaxy build reads by GOG's name for it, else by its id.
    let shown = |name: &Option<String>, id: &Option<String>| {
        name.clone()
            .or_else(|| id.clone())
            .unwrap_or_else(|| "-".to_owned())
    };
    let mut table = vec![["NAME", "INSTALLED", "AVAILABLE", "STATE"].map(str::to_owned)];
    table.extend(rows.iter().map(|row| match row {
        Row::Gog(status) => [
            status.environment.clone(),
            shown(&status.installed_name, &status.installed),
            shown(&status.available_name, &status.available),
            state_label(status).to_owned(),
        ],
        Row::Epic(status) => epic::table_row(status),
    }));
    let mut lines = render_columns(&table).into_iter();
    println!("{}", lines.next().expect("the header"));
    for (row, line) in rows.iter().zip(lines) {
        println!("{line}");
        let status = match row {
            Row::Gog(status) => status,
            Row::Epic(status) => {
                for note in epic::notes(status) {
                    println!("{note}");
                }
                continue;
            }
        };
        if let Some(reason) = &status.reason {
            println!("  {}", reason.note());
        } else if let Some(note) = status.pin_note() {
            // Nothing newer, but the pin stays visible.
            println!("  {note}");
        }
        if status.galaxy_install {
            println!(
                "  Finish the interrupted install with: wineo import gog --game {}",
                status.gog_id
            );
        } else if status.unfinished {
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
