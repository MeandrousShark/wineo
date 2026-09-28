use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Context;
use signal_hook::consts::SIGINT;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::launch::{Host, Launch, parse_env_assignment};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::runtime::{Runtime, exit_code};

use crate::commands::build;
use crate::commands::components;
use crate::commands::desktop;
use crate::commands::transfer::Cancellation;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to run the program in
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    /// Set an environment variable, overriding the config
    #[arg(short, long = "env", value_name = "KEY=VALUE", value_parser = parse_env_assignment)]
    env: Vec<(String, String)>,

    /// Windows path, Unix path, or a name wine knows like winecfg
    #[arg(value_hint = clap::ValueHint::FilePath)]
    program: String,

    /// Arguments passed to the program
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, &args.name)?;
    let env = args.env.into_iter().collect();
    let runner = build::runner(paths, &global, &environment)?;
    Launch::check_exec(paths, &global, &environment, &runner, &args.program, &env)?;
    let cancellation = Cancellation::new()?;
    match build::ensure_prefix(paths, &global, &environment, &runner, &cancellation.token) {
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
    drop(cancellation);
    let launch = Launch::exec(
        paths,
        &global,
        &environment,
        &args.program,
        &args.args,
        &env,
    )?;
    let Host::Wine { runner, prefix } = &launch.host else {
        unreachable!("exec launches always run in a prefix");
    };
    let locks = Locks::new(paths);
    let _guard = locks.acquire(Operation::Launch, &environment.name, &[prefix])?;
    Environment::ensure_exists(paths, &environment.name)?;
    Runtime::new(paths, &environment.name).check_resolved(&environment.name)?;
    components::reconcile_config(paths, &global, &environment, runner, prefix)?;
    desktop::reconcile(paths, &global, &environment, Some(runner), prefix)?;

    let code = run_attached(&launch)?;
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

/// Runs `launch` attached to the terminal.
fn run_attached(launch: &Launch) -> anyhow::Result<i32> {
    // The program shares our process group and gets Ctrl+C itself. wineo keeps
    // waiting instead of exiting out from under it.
    signal_hook::flag::register(SIGINT, Arc::new(AtomicBool::new(false)))
        .context("failed to set up signal handling")?;
    let status = launch
        .command()
        .status()
        .with_context(|| format!("failed to start {}", launch.host.launcher()))?;
    Ok(exit_code(status))
}
