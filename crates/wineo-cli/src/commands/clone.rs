use std::path::PathBuf;
use std::process::ExitCode;

use clap_complete::ArgValueCompleter;
use wineo_core::clone::{self, CloneInspection};
use wineo_core::config::GlobalConfig;
use wineo_core::import::ImportProgress;
use wineo_core::paths::Paths;

use super::transfer::Cancellation;
use crate::picker;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to copy
    #[arg(value_name = "SOURCE", add = ArgValueCompleter::new(crate::completion::environments))]
    source: String,

    /// Name for the copied environment
    #[arg(value_name = "NEW_NAME")]
    name: String,

    /// Copy destination, defaults to the configured prefix root
    #[arg(long, value_name = "PATH")]
    dir: Option<String>,
}

pub fn run(paths: &Paths, args: Args, verbose: bool) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let destination: Option<PathBuf> = args
        .dir
        .as_deref()
        .map(|dir| paths.absolute(&picker::clean_path(dir)))
        .transpose()?;
    let reviewed = clone::inspect(
        paths,
        &global,
        &args.source,
        &args.name,
        destination.as_deref(),
    )?;
    print_review(&reviewed, verbose);

    let cancellation = Cancellation::new()?;
    let mut copied = 0_u64;
    let result = clone::execute_reviewed(paths, &global, &reviewed, &cancellation.flag, |event| {
        if let ImportProgress::Copied { .. } = event {
            copied += 1;
            if copied.is_multiple_of(500) {
                eprintln!("Copied {copied} entries");
            }
        }
    });
    match result {
        Ok(environment) => {
            println!("Cloned {} as {}", reviewed.source_name, environment.name);
            Ok(ExitCode::SUCCESS)
        }
        Err(error) if error.is::<clone::Canceled>() => {
            eprintln!("Clone canceled");
            Ok(ExitCode::from(130))
        }
        Err(error) => Err(error),
    }
}

fn print_review(reviewed: &CloneInspection, verbose: bool) {
    println!(
        "Clone {} as {}\n  source: {}\n  copy to: {}\n  runner: {}\n  layout: {:?}\n  targets: {}",
        reviewed.source_name,
        reviewed.new_name,
        reviewed.source_root.display(),
        reviewed.destination.display(),
        reviewed.runner,
        reviewed.layout,
        if reviewed.targets.is_empty() {
            "(none)".to_owned()
        } else {
            reviewed.targets.join(", ")
        }
    );
    for setting in &reviewed.changed_settings {
        println!("  change: {setting}");
    }
    for path in &reviewed.literal_source_paths {
        println!("  source path in settings: {path} (check after cloning)");
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
    println!(
        "The source and any external launcher must be idle. External game files stay where they are."
    );
}
