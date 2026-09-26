use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::launch::Launch;
use wineo_core::paths::Paths;
use wineo_core::runtime::Runtime;

use crate::commands::run::LaunchArgs;
use crate::commands::{build, start, stop};

#[derive(clap::Args)]
pub struct Args {
    /// Environment to restart
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    #[command(flatten)]
    launch: LaunchArgs,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, &args.name)?;
    let name = &environment.name;
    let overrides = if args.launch.is_empty() {
        Runtime::new(paths, name)
            .read_state()?
            .map(|run| run.overrides())
            .unwrap_or_default()
    } else {
        args.launch.into_overrides()
    };
    // Checked before stopping, so a bad config or a missing target doesn't
    // leave the environment stopped. A missing prefix has nothing running in
    // it, so it is built here like `start` would.
    build::prepare_target(paths, &global, &environment, &overrides)?;
    Launch::target(paths, &global, &environment, &overrides)?;

    let was_running = matches!(stop::stop(paths, name, false)?, stop::Outcome::Stopped);
    if start::start(paths, name, overrides)? {
        let verb = if was_running { "Restarted" } else { "Started" };
        println!("{verb} {name}");
    }
    Ok(ExitCode::SUCCESS)
}
