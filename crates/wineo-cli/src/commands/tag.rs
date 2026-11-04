use std::process::ExitCode;

use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use wineo_core::paths::Paths;
use wineo_core::settings::{self, TagChange, Tagged};
use wineo_core::view::{CollectionError, TagOutput};

use super::json;
use crate::style;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Add a tag to environments
    Add(Change),
    /// Remove a tag from environments
    #[command(visible_alias = "rm")]
    Remove(Change),
}

#[derive(clap::Args)]
struct Change {
    #[arg(add = ArgValueCompleter::new(crate::completion::tags))]
    tag: String,
    /// Environments to change
    #[arg(
        value_name = "ENV",
        required = true,
        add = ArgValueCompleter::new(crate::completion::tag_environments)
    )]
    names: Vec<String>,
    /// Print machine-readable JSON
    #[arg(long)]
    json: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let (args, change) = match &args.command {
        Command::Add(args) => (args, TagChange::Add(&args.tag)),
        Command::Remove(args) => (args, TagChange::Remove(&args.tag)),
    };
    // Checked once up front, so a bad tag writes nothing anywhere.
    settings::check_tag(&args.tag)?;
    let mut output = TagOutput {
        tag: &args.tag,
        changed: Vec::new(),
        unchanged: Vec::new(),
        errors: Vec::new(),
    };
    // Each environment stands alone, so one that fails leaves the rest changed.
    for name in &args.names {
        let result = settings::change_tags(paths, name, change).map_err(message);
        if !args.json {
            report(&result, change, name);
        }
        match result {
            Ok(Tagged::Changed) => output.changed.push(name),
            Ok(Tagged::Unchanged) => output.unchanged.push(name),
            Err(message) => output.errors.push(CollectionError {
                name: name.clone(),
                message,
            }),
        }
    }
    if args.json {
        json::print(&output)?;
    }
    Ok(if output.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Worded the way `list` and `stats` word theirs.
fn message(error: wineo_core::Error) -> String {
    format!("{:#}", anyhow::Error::from(error))
}

/// One line for `name` as soon as it's done, so a long list shows progress.
fn report(result: &Result<Tagged, String>, change: TagChange, name: &str) {
    let tag = change.tag();
    match (result, change) {
        (Ok(Tagged::Changed), TagChange::Add(_)) => println!("Added {tag} to {name}"),
        (Ok(Tagged::Changed), TagChange::Remove(_)) => println!("Removed {tag} from {name}"),
        (Ok(Tagged::Unchanged), TagChange::Add(_)) => eprintln!("{name} already has {tag}"),
        (Ok(Tagged::Unchanged), TagChange::Remove(_)) => eprintln!("{name} doesn't have {tag}"),
        (Err(message), _) => eprintln!("{}", style::error_stderr(format!("error: {message}"))),
    }
}
