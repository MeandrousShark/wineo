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
    /// Print an environment's config, or the global config
    Config(commands::config::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = Paths::from_env()
        .map_err(anyhow::Error::from)
        .and_then(|paths| match cli.command {
            Command::New(args) => commands::new::run(&paths, args),
            Command::Config(args) => commands::config::run(&paths, args),
        });
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
