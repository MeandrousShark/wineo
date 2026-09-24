use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::bail;
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use wineo_core::config::GlobalConfig;
use wineo_core::import::ImportProgress;
use wineo_core::paths::Paths;
use wineo_core::snapshot::{self, CreateInspection, RestoreInspection};

use super::transfer::Cancellation;
use crate::prompt;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Save the prefix and config under a new label
    Create {
        #[arg(add = ArgValueCompleter::new(crate::completion::environments))]
        env: String,
        label: String,
    },
    /// List an environment's snapshots
    List {
        #[arg(add = ArgValueCompleter::new(crate::completion::environments))]
        env: String,
    },
    /// Restore the saved prefix and config
    Restore {
        #[arg(add = ArgValueCompleter::new(crate::completion::environments))]
        env: String,
        #[arg(add = ArgValueCompleter::new(crate::completion::snapshots))]
        label: String,
        /// Accept the reviewed restore without a terminal prompt
        #[arg(long)]
        yes: bool,
    },
    /// Remove a named snapshot
    Delete {
        #[arg(add = ArgValueCompleter::new(crate::completion::environments))]
        env: String,
        #[arg(add = ArgValueCompleter::new(crate::completion::snapshots))]
        label: String,
    },
    /// Roll back an interrupted restore
    Recover {
        #[arg(add = ArgValueCompleter::new(crate::completion::environments))]
        env: String,
    },
}

pub fn run(paths: &Paths, args: Args, verbose: bool) -> anyhow::Result<ExitCode> {
    match args.command {
        Command::Create { env, label } => create(paths, &env, &label, verbose),
        Command::List { env } => {
            let snapshots = snapshot::list(paths, &env)?;
            if snapshots.is_empty() {
                println!("No snapshots for {env}");
            } else {
                for item in snapshots {
                    println!("{}\t{}", item.label, item.created_at);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Restore { env, label, yes } => restore(paths, &env, &label, yes),
        Command::Delete { env, label } => {
            snapshot::delete(paths, &env, &label)?;
            println!("Deleted snapshot {label} of {env}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Recover { env } => {
            snapshot::recover(paths, &env)?;
            println!("Recovered {env} from the interrupted restore");
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn create(paths: &Paths, env: &str, label: &str, verbose: bool) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let reviewed = snapshot::inspect_create(paths, &global, env, label)?;
    print_create_review(&reviewed, verbose);
    let cancellation = Cancellation::new()?;
    let mut copied = 0_u64;
    let result =
        snapshot::create_reviewed(paths, &global, &reviewed, &cancellation.flag, |event| {
            if let ImportProgress::Copied { .. } = event {
                copied += 1;
                if copied.is_multiple_of(500) {
                    eprintln!("Copied {copied} entries");
                }
            }
        });
    match result {
        Ok(item) => {
            println!("Created snapshot {} of {}", item.label, env);
            Ok(ExitCode::SUCCESS)
        }
        Err(error) if error.is::<snapshot::Canceled>() => {
            eprintln!("Snapshot canceled");
            Ok(ExitCode::from(130))
        }
        Err(error) => Err(error),
    }
}

fn restore(paths: &Paths, env: &str, label: &str, yes: bool) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let reviewed = snapshot::inspect_restore(paths, &global, env, label)?;
    print_restore_review(&reviewed);
    if !yes {
        if !prompt::interactive() {
            bail!("restore needs an interactive terminal or --yes");
        }
        print!("Restore {env} from {label}? [y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Restore canceled");
            return Ok(ExitCode::SUCCESS);
        }
    }
    let cancellation = Cancellation::new()?;
    let mut copied = 0_u64;
    let result =
        snapshot::restore_reviewed(paths, &global, &reviewed, &cancellation.flag, |event| {
            if let ImportProgress::Copied { .. } = event {
                copied += 1;
                if copied.is_multiple_of(500) {
                    eprintln!("Copied {copied} entries");
                }
            }
        });
    match result {
        Ok(outcome) => {
            println!("Restored {env} from snapshot {label}");
            if let Some(path) = outcome.retained_old_prefix {
                println!(
                    "The old Btrfs subvolume remains at {}. Remove it manually when you no longer need it.",
                    path.display()
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(error) if error.is::<snapshot::Canceled>() => {
            eprintln!("Restore canceled");
            Ok(ExitCode::from(130))
        }
        Err(error) => Err(error),
    }
}

fn print_create_review(reviewed: &CreateInspection, verbose: bool) {
    println!(
        "Snapshot {} as {}\n  prefix: {}\n  save to: {}\n  runner: {}\n  layout: {:?}",
        reviewed.name,
        reviewed.label,
        reviewed.prefix.display(),
        reviewed.snapshot.display(),
        reviewed.runner,
        reviewed.layout
    );
    let count = reviewed.external_dependencies.len();
    if count > 0 {
        if !verbose && count > 12 {
            println!("  external links: {count} (showing first 12; use -v for all)");
        } else {
            println!("  external links: {count}");
        }
    }
    for link in reviewed
        .external_dependencies
        .iter()
        .take(if verbose { count } else { 12 })
    {
        println!(
            "  external: {} -> {} (not included)",
            link.source.display(),
            link.target.display()
        );
    }
    println!("The prefix and any external launcher must be idle.");
}

fn print_restore_review(reviewed: &RestoreInspection) {
    println!(
        "Restore {} from {}\n  snapshot: {}\n  replace prefix: {}\n  layout: {:?}\n  current runner: {}\n  saved runner: {}\n  config: {}",
        reviewed.name,
        reviewed.label,
        reviewed.snapshot.display(),
        reviewed.prefix.display(),
        reviewed.layout,
        reviewed.current_runner,
        reviewed.saved_runner,
        if reviewed.current_config == reviewed.saved_config {
            "unchanged"
        } else {
            "restore saved settings and targets"
        }
    );
    println!("Current prefix contents and config will be replaced. External files stay in place.");
}
