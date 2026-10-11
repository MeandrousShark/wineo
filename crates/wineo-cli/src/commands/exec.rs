use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Context;
use signal_hook::consts::SIGINT;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::launch::{self, Host, Launch, parse_env_assignment};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::process;
use wineo_core::runtime::{Runtime, exit_code};

use crate::commands::build;
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
    let (runner, _build_guard) = launch::runner_locked(paths, &global, &environment)?;
    Launch::check_exec(paths, &global, &environment, &runner, &args.program, &env)?;
    let cancellation = Cancellation::new()?;
    match launch::ensure_prefix(
        paths,
        &global,
        &environment,
        &runner,
        &cancellation.token,
        |event| build::print_event(paths, &environment.name, event),
    ) {
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
    let Host::Wine { prefix, .. } = &launch.host else {
        unreachable!("exec launches always run in a prefix");
    };
    let locks = Locks::new(paths);
    let _guard = locks.acquire(Operation::Launch, &environment.name, &[prefix])?;
    Environment::ensure_exists(paths, &environment.name)?;
    Runtime::new(paths, &environment.name).check_resolved(&environment.name)?;
    // Claiming a folder can wait on its state lock, so Ctrl+C still works
    // until the program starts and takes Ctrl+C itself.
    let cancellation = Cancellation::new()?;
    let folders = launch::protect(paths, &environment, &launch, &cancellation.token);
    drop(cancellation);
    let _folders = match folders {
        Err(err) if matches!(err.downcast_ref(), Some(Error::Canceled(_))) => {
            return Ok(ExitCode::from(130));
        }
        result => result?,
    };
    launch::reconcile(paths, &global, &environment, &launch, |event| {
        build::print_event(paths, &environment.name, event)
    })?;

    let code = run_attached(&launch)?;
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

/// Runs `launch` attached to the terminal.
fn run_attached(launch: &Launch) -> anyhow::Result<i32> {
    // The program shares our process group and gets Ctrl+C itself. wineo keeps
    // waiting instead of exiting out from under it.
    signal_hook::flag::register(SIGINT, Arc::new(AtomicBool::new(false)))
        .context("failed to set up signal handling")?;
    let mut child = launch
        .command()
        .spawn()
        .with_context(|| format!("failed to start {}", launch.host.launcher()))?;
    process::quiet_core_dumps(&child);
    let status = child
        .wait()
        .with_context(|| format!("failed to start {}", launch.host.launcher()))?;
    Ok(exit_code(status))
}
