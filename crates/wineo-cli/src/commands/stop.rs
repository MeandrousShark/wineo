use std::process::ExitCode;

use crate::commands::transfer::Cancellation;
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
    let token = wineo_core::cancel::Token::new();
    let mut cancellation = None;
    let mut signal_error = None;
    let result = launch::stop_with(paths, name, args.force, &token, |event| match event {
        launch::StopEvent::WaitingHook => match Cancellation::with_token(&token) {
            Ok(listener) => cancellation = Some(listener),
            Err(err) => {
                signal_error = Some(err);
                token.cancel();
            }
        },
        launch::StopEvent::WaitingAfter => eprintln!("Waiting for the after hook."),
    });
    drop(cancellation);
    if let Some(err) = signal_error {
        return Err(err);
    }
    let outcome = match result {
        Err(err)
            if matches!(
                err.downcast_ref::<wineo_core::error::Error>(),
                Some(wineo_core::error::Error::Canceled(_))
            ) =>
        {
            return Ok(ExitCode::from(130));
        }
        result => result?,
    };
    match outcome {
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
