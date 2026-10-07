use std::path::PathBuf;
use std::process::ExitCode;

use clap_complete::ArgValueCompleter;
use wineo_core::clone::{self, CloneInspection};
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::paths::Paths;

use super::transfer::{self, Cancellation};
use crate::picker;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to copy
    #[arg(value_name = "SOURCE", add = ArgValueCompleter::new(crate::completion::environments))]
    source: String,

    /// Name for the copied environment
    #[arg(value_name = "NEW_NAME")]
    name: String,

    /// Copy destination, defaults to the configured prefix root, or DIR/prefixes with --portable
    #[arg(long, value_name = "PATH", value_hint = clap::ValueHint::DirPath)]
    dir: Option<String>,
}

pub fn run(paths: &Paths, args: Args, verbose: bool) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    if let Ok(source) = Environment::load(paths, &args.source)
        && source.config.wine().is_none()
    {
        if args.dir.is_some() {
            return Err(Error::NoPrefix(args.source).into());
        }
        return clone_native(paths, &args.source, &args.name);
    }
    let destination: Option<PathBuf> = args
        .dir
        .as_deref()
        .map(|dir| paths.absolute(&picker::clean_path(dir)))
        .transpose()?;
    transfer::wait_idle(paths, &global, &args.source)?;
    let reviewed = clone::inspect(
        paths,
        &global,
        &args.source,
        &args.name,
        destination.as_deref(),
    )?;
    print_review(&reviewed, verbose);

    let cancellation = Cancellation::new()?;
    let result = clone::execute_reviewed(
        paths,
        &global,
        &reviewed,
        &cancellation.token,
        transfer::copy_progress(),
    );
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

/// A native environment is only its config and art, so there is nothing to
/// wait for or copy beyond them.
fn clone_native(paths: &Paths, source: &str, name: &str) -> anyhow::Result<ExitCode> {
    let reviewed = clone::inspect_native(paths, source, name)?;
    println!(
        "Clone {} as {}\n  kind: native\n  targets: {}",
        reviewed.source_name,
        reviewed.new_name,
        if reviewed.targets.is_empty() {
            "(none)".to_owned()
        } else {
            reviewed.targets.join(", ")
        }
    );
    if reviewed.art {
        println!("  art: copied");
    }
    for setting in &reviewed.changed_settings {
        println!("  change: {setting}");
    }
    let environment = clone::execute_native(paths, &reviewed)?;
    println!("Cloned {} as {}", reviewed.source_name, environment.name);
    Ok(ExitCode::SUCCESS)
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
    if reviewed.art {
        println!("  art: copied");
    }
    for setting in &reviewed.changed_settings {
        println!("  change: {setting}");
    }
    for path in &reviewed.literal_source_paths {
        println!("  source path in settings: {path} (check after cloning)");
    }
    for path in &reviewed.external_target_paths {
        println!("  external target path: {path} (kept in place)");
    }
    transfer::print_external_links(
        "external",
        &reviewed.external_dependencies,
        verbose,
        "kept in place",
    );
    println!(
        "The source and any external launcher must be idle. External game files stay where they are."
    );
}
