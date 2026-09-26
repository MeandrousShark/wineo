use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Subcommand, ValueHint};
use clap_complete::ArgValueCompleter;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::import::ImportProgress;
use wineo_core::paths::Paths;
use wineo_core::portable::{self, CopyInspection, Storage};

use super::transfer::{self, Cancellation};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Copy an environment and its snapshots into a portable folder
    Copy {
        /// Environment to copy from the standard locations
        #[arg(add = ArgValueCompleter::new(crate::completion::environments))]
        name: String,
        /// Portable folder to copy into
        #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
        to: PathBuf,
    },
}

pub fn run(paths: &Paths, args: Args, verbose: bool) -> anyhow::Result<ExitCode> {
    match args.command {
        Command::Copy { name, to } => copy(paths, &name, &to, verbose),
    }
}

fn copy(
    paths: &Paths,
    name: &str,
    to: &std::path::Path,
    verbose: bool,
) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let folder = Paths::from_env(Some(to))?;
    if let Ok(source) = Environment::load(paths, name)
        && source.config.wine().is_none()
    {
        return copy_native(paths, &folder, name);
    }
    transfer::wait_idle(paths, &global, name)?;
    let reviewed = portable::inspect(paths, &folder, &global, name)?;
    print_review(&reviewed, verbose);

    let cancellation = Cancellation::new()?;
    let mut copied = 0_u64;
    let result = portable::execute_reviewed(
        paths,
        &folder,
        &global,
        &reviewed,
        &cancellation.flag,
        |event| {
            if let ImportProgress::Copied { .. } = event {
                copied += 1;
                if copied.is_multiple_of(500) {
                    eprintln!("Copied {copied} entries");
                }
            }
        },
    );
    match result {
        Ok(environment) => {
            println!(
                "Copied {} to {}",
                environment.name,
                reviewed.folder.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(error) if error.is::<portable::Canceled>() => {
            eprintln!("Copy canceled");
            Ok(ExitCode::from(130))
        }
        Err(error) => Err(error),
    }
}

/// A native environment is only its config and art, with no prefix or
/// snapshots to copy.
fn copy_native(paths: &Paths, folder: &Paths, name: &str) -> anyhow::Result<ExitCode> {
    let reviewed = portable::inspect_native(paths, folder, name)?;
    println!(
        "Copy {} to {}\n  kind: native\n  targets: {}",
        reviewed.name,
        reviewed.folder.display(),
        if reviewed.targets.is_empty() {
            "(none)".to_owned()
        } else {
            reviewed.targets.join(", ")
        }
    );
    for setting in &reviewed.changed_settings {
        println!("  change: {setting}");
    }
    if reviewed.first_copy {
        if reviewed.copies_global_config {
            println!("  global config: copied, prefix_root follows the folder");
        }
        if !reviewed.copied_templates.is_empty() {
            println!("  templates: {}", reviewed.copied_templates.join(", "));
        }
    } else {
        println!("  global config and templates: the folder's own are kept");
    }
    if reviewed.art {
        println!("  art: copied");
    }
    for path in &reviewed.external_target_paths {
        println!("  external target path: {path} (kept in place)");
    }
    println!("The source stays as it is. External game files stay where they are.");

    let cancellation = Cancellation::new()?;
    match portable::execute_native(paths, folder, &reviewed, &cancellation.flag) {
        Ok(environment) => {
            println!(
                "Copied {} to {}",
                environment.name,
                reviewed.folder.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(error) if error.is::<portable::Canceled>() => {
            eprintln!("Copy canceled");
            Ok(ExitCode::from(130))
        }
        Err(error) => Err(error),
    }
}

fn print_review(reviewed: &CopyInspection, verbose: bool) {
    let list = |items: &[String]| {
        if items.is_empty() {
            "(none)".to_owned()
        } else {
            items.join(", ")
        }
    };
    println!(
        "Copy {} to {}\n  source: {}\n  copy to: {}\n  runner: {}\n  layout: {:?}\n  targets: {}\n  snapshots: {}",
        reviewed.name,
        reviewed.folder.display(),
        reviewed.source_root.display(),
        reviewed.destination.display(),
        reviewed.runner,
        reviewed.layout,
        list(&reviewed.targets),
        list(&reviewed.snapshots),
    );
    for setting in &reviewed.changed_settings {
        println!("  change: {setting}");
    }
    if reviewed.first_copy {
        if reviewed.copies_global_config {
            println!("  global config: copied, prefix_root follows the folder");
        }
        if !reviewed.copied_templates.is_empty() {
            println!("  templates: {}", reviewed.copied_templates.join(", "));
        }
    } else {
        println!("  global config and templates: the folder's own are kept");
    }
    if reviewed.art {
        println!("  art: copied");
    }
    for path in &reviewed.literal_source_paths {
        println!("  source path in settings: {path} (check after copying)");
    }
    for path in &reviewed.external_target_paths {
        println!("  external target path: {path} (kept in place)");
    }
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
            "  external: {} -> {} (kept in place)",
            link.source.display(),
            link.target.display()
        );
    }
    for link in &reviewed.snapshot_links {
        println!(
            "  snapshot external: {} -> {} (kept in place)",
            link.source.display(),
            link.target.display()
        );
    }
    match reviewed.storage {
        Storage::Reflink => println!("  storage: reflinked, shares space with the source"),
        Storage::FullCopy => {
            println!("  storage: no reflink here, so the copy takes full space");
            if !reviewed.snapshots.is_empty() {
                println!("  snapshots: creating or restoring them here needs a reflink filesystem");
            }
        }
        Storage::Unknown => println!("  storage: reflink support could not be checked"),
    }
    println!(
        "The source stays as it is and must be idle. Runners and external game files stay where they are."
    );
}
