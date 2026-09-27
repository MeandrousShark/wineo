use std::process::ExitCode;

use wineo_core::paths::Paths;
use wineo_core::trash;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to remove
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    /// Retain the environment's Wine prefix and snapshots in trash too
    #[arg(long)]
    delete_prefix: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let entry = trash::remove_with(paths, &args.name, args.delete_prefix, || {
        eprintln!("Waiting for wine to exit in {}", args.name);
    })?;
    if args.delete_prefix && entry.prefix_present == Some(false) {
        eprintln!("The prefix is already gone; its config was retained in trash");
    }
    if entry.desktop_removed > 0 {
        eprintln!(
            "Removed {} desktop entr{}",
            entry.desktop_removed,
            if entry.desktop_removed == 1 {
                "y"
            } else {
                "ies"
            }
        );
    }
    println!("Removed environment {} to trash {}", entry.name, entry.id);
    if args.delete_prefix {
        println!("Environment {} and its prefix were retained", entry.name);
    }
    println!("Restore with: wineo trash restore {}", entry.id);
    if let Some(prefix) = entry.prefix_left {
        if entry.prefix_present == Some(false) {
            println!("Prefix was already absent at {}", prefix.display());
        } else {
            println!(
                "Removed {}, its prefix is still at {}",
                entry.name,
                prefix.display()
            );
        }
    } else if !args.delete_prefix {
        println!("Removed {}, its prefix was left untouched", entry.name);
    }
    if let Some(snapshots) = entry.snapshots_left
        && snapshots.is_dir()
        && std::fs::read_dir(&snapshots)?.next().is_some()
    {
        println!("Snapshots remain at {}", snapshots.display());
    }
    Ok(ExitCode::SUCCESS)
}
