use std::process::ExitCode;

use wineo_core::launch::{self, StopOutcome};
use wineo_core::paths::Paths;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to stop
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    /// Kill everything in the prefix right away instead of asking programs to close
    #[arg(long)]
    force: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let name = &args.name;
    match launch::stop(paths, name, args.force)? {
        StopOutcome::Stopped { messages } => {
            print_messages(messages);
            println!("Stopped {name}");
        }
        StopOutcome::NotRunning { messages } => {
            print_messages(messages);
            eprintln!("{name} is not running");
        }
        StopOutcome::CleanedUp { messages } => print_messages(messages),
        StopOutcome::Unresolved { messages } => {
            print_messages(messages);
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_messages(messages: Vec<String>) {
    for message in messages {
        eprintln!("{message}");
    }
}
