use std::process::ExitCode;

use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::launch;
use wineo_core::paths::Paths;
use wineo_core::runtime::Runtime;

use crate::commands::run::LaunchArgs;
use crate::commands::start;
use crate::commands::transfer::Cancellation;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to restart
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    #[command(flatten)]
    launch: LaunchArgs,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let global = wineo_core::config::GlobalConfig::load(paths)?;
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
    // Path checks must follow before, which may mount the target's files.
    let before = if environment.config.hooks.is_some()
        || environment
            .config
            .targets
            .values()
            .any(|target| target.hooks.is_some())
    {
        launch::resolve_target(&environment, overrides.target.as_deref())
            .ok()
            .and_then(|(_, target)| launch::hooks(&environment, Some(target)))
            .and_then(|hooks| hooks.before.as_ref())
    } else {
        None
    };
    if before.is_none() {
        let cancellation = Cancellation::new()?;
        let result = launch::prepare_target(
            paths,
            &global,
            &environment,
            &overrides,
            &cancellation.token,
            |event| crate::commands::build::print_event(paths, name, event),
        );
        // Dropped before stopping, so Ctrl+C can still abort a slow stop.
        drop(cancellation);
        match result {
            Err(err) if matches!(err.downcast_ref::<Error>(), Some(Error::Canceled(_))) => {
                return Ok(ExitCode::from(130));
            }
            result => result?,
        }
        launch::Launch::target(paths, &global, &environment, &overrides)?;
    }

    let token = wineo_core::cancel::Token::new();
    let mut cancellation = None;
    let mut signal_error = None;
    let result = launch::stop_run_with(paths, name, false, &token, |event| match event {
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
    let stopped = match result {
        Err(err) if matches!(err.downcast_ref::<Error>(), Some(Error::Canceled(_))) => {
            return Ok(ExitCode::from(130));
        }
        result => result?,
    };
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
