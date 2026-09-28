use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::launch::{self, Launch};
use wineo_core::paths::Paths;
use wineo_core::runtime::Runtime;

use crate::commands::run::LaunchArgs;
use crate::commands::transfer::Cancellation;
use crate::commands::{build, start};

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
    let cancellation = Cancellation::new()?;
    let prepared = launch::prepare_target(
        paths,
        &global,
        &environment,
        &overrides,
        &cancellation.token,
        |event| build::print_event(paths, name, event),
    );
    // Dropped before stopping, so Ctrl+C can still abort a slow stop.
    drop(cancellation);
    match prepared {
        Err(err)
            if err
                .downcast_ref::<Error>()
                .is_some_and(|err| matches!(err, Error::Canceled(_))) =>
        {
            return Ok(ExitCode::from(130));
        }
        Err(err) => return Err(err),
        Ok(()) => {}
    }
    Launch::target(paths, &global, &environment, &overrides)?;

    let stopped = launch::stop_run(paths, name, false)?;
    for message in stopped.messages() {
        eprintln!("{message}");
    }
    let was_running = matches!(stopped, launch::StopOutcome::Stopped { .. });
    match start::start(paths, name, overrides) {
        Ok(true) => {
            let verb = if was_running { "Restarted" } else { "Started" };
            println!("{verb} {name}");
        }
        Ok(false) => {}
        Err(err)
            if err
                .downcast_ref::<Error>()
                .is_some_and(|err| matches!(err, Error::Canceled(_))) =>
        {
            return Ok(ExitCode::from(130));
        }
        Err(err) => return Err(err),
    }
    Ok(ExitCode::SUCCESS)
}
