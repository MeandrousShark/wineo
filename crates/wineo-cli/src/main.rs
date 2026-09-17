mod commands;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use wineo_core::paths::Paths;
use wineo_core::supervisor;

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
    /// Start an environment in the background
    Start(commands::start::Args),
    /// Stop a running environment
    Stop(commands::stop::Args),
    /// Stop an environment if it's running, then start it again
    Restart(commands::restart::Args),
    /// Show whether environments are running
    Status(commands::status::Args),
    /// List environments and their state
    List(commands::list::Args),
    /// Print the output of an environment's detached runs
    Logs(commands::logs::Args),
    /// Run an environment attached to the terminal
    Run(commands::run::Args),
    /// Run another program inside an environment
    Exec(commands::exec::Args),
    /// Owns a detached run for `start`
    #[command(name = supervisor::SUPERVISE_COMMAND, hide = true)]
    Supervise,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = Paths::from_env()
        .map_err(anyhow::Error::from)
        .and_then(|paths| match cli.command {
            Command::New(args) => commands::new::run(&paths, args),
            Command::Set(args) => commands::set::run(&paths, args),
            Command::Config(args) => commands::config::run(&paths, args),
            Command::Start(args) => commands::start::run(&paths, args),
            Command::Stop(args) => commands::stop::run(&paths, args),
            Command::Restart(args) => commands::restart::run(&paths, args),
            Command::Status(args) => commands::status::run(&paths, args),
            Command::List(args) => commands::list::run(&paths, args),
            Command::Logs(args) => commands::logs::run(&paths, args),
            Command::Run(args) => commands::run::run(&paths, args),
            Command::Exec(args) => commands::exec::run(&paths, args),
            Command::Supervise => supervisor::serve()
                .map(|()| ExitCode::SUCCESS)
                .map_err(Into::into),
        });
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
