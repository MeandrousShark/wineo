use std::process::ExitCode;

use anyhow::Context;
use wineo_core::error::Error;
use wineo_core::installer::Output;
use wineo_core::launch::{self, LaunchEvent, Overrides, StartOutcome};
use wineo_core::paths::Paths;

use crate::commands::build;
use crate::commands::run::LaunchArgs;
use crate::commands::transfer::Cancellation;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to start
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    #[command(flatten)]
    launch: LaunchArgs,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    match start(paths, &args.name, args.launch.into_overrides()) {
        Ok(true) => println!("Started {}", args.name),
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

pub fn start(paths: &Paths, name: &str, overrides: Overrides) -> anyhow::Result<bool> {
    let exe = std::env::current_exe().context("failed to find the wineo binary")?;
    let mut cancellation = Some(Cancellation::new()?);
    let token = cancellation.as_ref().unwrap().token.clone();
    let mut signal_error = None;
    let result = launch::start(
        paths,
        name,
        overrides.clone(),
        &exe,
        &token,
        wineo_core::hooks::Output::Inherit,
        Output::Inherit,
        |event| {
            if matches!(event, LaunchEvent::Starting) {
                // The supervisor reply does not use the token. Restore normal Ctrl+C first.
                cancellation.take();
            } else if let LaunchEvent::AfterHook(hook) = event {
                match Cancellation::after_hook_with(&hook, token.is_canceled()) {
                    Ok(listener) => cancellation = Some(listener),
                    Err(err) => {
                        signal_error = Some(err);
                        hook.force();
                    }
                }
            } else {
                build::print_event(paths, name, event);
            }
        },
    );
    if let Some(err) = signal_error {
        return Err(err);
    }
    let result = result?;
    match result {
        StartOutcome::SteamAlreadyRunning => {
            eprintln!("{name} is already running (started from Steam).")
        }
        StartOutcome::SteamRequested => println!("Asked Steam to start {name}."),
        StartOutcome::SteamAdopted => println!("Started {name} (Steam had already launched it)."),
        _ => {}
    }
    if result == StartOutcome::AlreadyRunning {
        already_running(name, &overrides);
    }
    Ok(result == StartOutcome::Started)
}

fn already_running(name: &str, overrides: &Overrides) {
    if overrides.env.is_empty() && overrides.args.is_empty() {
        eprintln!("{name} is already running");
    } else {
        eprintln!(
            "{name} is already running, so the options weren't applied. Pass them to `wineo restart` instead"
        );
    }
}
