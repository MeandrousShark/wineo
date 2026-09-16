mod commands;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use wineo_core::paths::Paths;

#[derive(Parser)]
#[command(name = "wineo", version, about = "Manage Wine environments")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create an environment
    New(commands::new::Args),
    /// Change settings of an environment, or the global config
    Set(commands::set::Args),
    /// Print an environment's config, or the global config
    Config(commands::config::Args),
    /// Run an environment attached to the terminal
    Run(commands::run::Args),
    /// Run another program inside an environment
    Exec(commands::exec::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = Paths::from_env()
        .map_err(anyhow::Error::from)
        .and_then(|paths| match cli.command {
            Command::New(args) => commands::new::run(&paths, args),
            Command::Set(args) => commands::set::run(&paths, args),
            Command::Config(args) => commands::config::run(&paths, args),
            Command::Run(args) => commands::run::run(&paths, args),
            Command::Exec(args) => commands::exec::run(&paths, args),
        });
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
