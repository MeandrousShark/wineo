use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Context;
use signal_hook::consts::SIGINT;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::launch::{Launch, parse_env_assignment};
use wineo_core::paths::Paths;
use wineo_core::runtime::exit_code;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to run the program in
    name: String,

    /// Set an environment variable, overriding the config
    #[arg(short, long = "env", value_name = "KEY=VALUE", value_parser = parse_env_assignment)]
    env: Vec<(String, String)>,

    /// Windows path, Unix path, or a name wine knows like winecfg
    program: String,

    /// Arguments passed to the program
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, &args.name)?;
    let env = args.env.into_iter().collect();
    let launch = Launch::exec(
        paths,
        &global,
        &environment,
        &args.program,
        &args.args,
        &env,
    )?;
    let code = run_attached(&launch)?;
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

/// Runs `launch` attached to the terminal. Shared with `new --install-exe`.
pub(crate) fn run_attached(launch: &Launch) -> anyhow::Result<i32> {
    // The program shares our process group and gets Ctrl+C itself. wineo keeps
    // waiting instead of exiting out from under it.
    signal_hook::flag::register(SIGINT, Arc::new(AtomicBool::new(false)))
        .context("failed to set up signal handling")?;
    let status = launch.command().status().context("failed to start wine")?;
    Ok(exit_code(status))
}
