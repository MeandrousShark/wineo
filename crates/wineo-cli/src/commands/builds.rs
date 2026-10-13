use std::process::ExitCode;

use clap::Subcommand;
use wineo_core::components::{Builds, Component};
use wineo_core::config::GlobalConfig;
use wineo_core::paths::Paths;

#[derive(clap::Args)]
pub struct Args {
    /// Also list directories that aren't usable builds, and why
    #[arg(long)]
    all: bool,

    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Remove an installed build
    Remove(RemoveArgs),
}

#[derive(clap::Args)]
struct RemoveArgs {
    #[arg(value_name = "NAME", add = clap_complete::ArgValueCompleter::new(crate::completion::removable_build_names))]
    name: String,

    /// Confirm removal without a terminal
    #[arg(long)]
    yes: bool,
}

pub fn run(paths: &Paths, component: Component, args: Args) -> anyhow::Result<ExitCode> {
    if let Some(Action::Remove(args)) = args.action {
        let kind = format!("{} build", component.label());
        return super::remove_build::run(
            paths,
            &args.name,
            &kind,
            component.into(),
            args.yes,
            |paths, name| Ok(wineo_core::components::size(paths, component, name)?),
            |paths, name| Ok(wineo_core::components::remove(paths, component, name)?),
        );
    }

    let global = GlobalConfig::load(paths)?;
    let builds = Builds::new(paths, &global, component)?;
    let listing = builds.list()?;

    let mut rows: Vec<(String, String)> = listing
        .builds
        .iter()
        .map(|build| (build.name().to_owned(), String::new()))
        .collect();
    if args.all {
        rows.extend(listing.skipped.iter().map(|(name, reason)| {
            (
                name.clone(),
                format!("skipped, {}", reason.describe(component)),
            )
        }));
    }

    let names = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    for (name, detail) in &rows {
        println!("{}", format!("{name:names$}  {detail}").trim_end());
    }

    if listing.builds.is_empty() {
        eprintln!(
            "No {component} builds installed. Extract one into {}",
            builds.root().display()
        );
    }
    Ok(ExitCode::SUCCESS)
}
