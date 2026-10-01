use std::process::ExitCode;

use wineo_core::paths::Paths;
use wineo_core::trash;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to remove
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    /// Also move the Wine prefix and snapshots to trash
    #[arg(long)]
    delete_prefix: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let entry = trash::remove_with(
        paths,
        &args.name,
        args.delete_prefix,
        &wineo_core::cancel::Token::new(),
        || {
            eprintln!("Waiting for wine to exit in {}", args.name);
        },
    )?;
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
    let prefix_was_left = !args.delete_prefix && entry.prefix_present == Some(true);
    if args.delete_prefix && entry.prefix_present == Some(true) {
        println!(
            "Removed environment {} and its prefix to trash {}",
            entry.name, entry.id
        );
    } else {
        println!("Removed environment {} to trash {}", entry.name, entry.id);
    }
    if let Some(prefix) = entry.prefix_left {
        if entry.prefix_present == Some(false) {
            println!("Prefix was already absent at {}", prefix.display());
        } else {
            println!("Prefix left at {}", prefix.display());
        }
    }
    if let Some(snapshots) = entry.snapshots_left
        && snapshots.is_dir()
        && std::fs::read_dir(&snapshots)?.next().is_some()
    {
        println!("Snapshots left at {}", snapshots.display());
    }
    println!("Restore with: wineo restore {}", entry.id);
    println!(
        "To permanently delete, run: wineo prune {}{}",
        entry.id,
        if prefix_was_left {
            " (the prefix stays)"
        } else {
            ""
        }
    );
    Ok(ExitCode::SUCCESS)
}
