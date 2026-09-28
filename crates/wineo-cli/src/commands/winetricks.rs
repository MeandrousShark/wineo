use std::collections::BTreeMap;
use std::process::ExitCode;

use wineo_core::error::Error;
use wineo_core::installer::Output;
use wineo_core::launch::parse_env_assignment;
use wineo_core::paths::Paths;
use wineo_core::runtime::exit_code;
use wineo_core::winetricks::{self, WinetricksEvent};

use super::transfer::Cancellation;

pub fn print_event(event: WinetricksEvent) {
    match event {
        WinetricksEvent::Installing(verbs) => {
            eprintln!("Installing {} with winetricks", verbs.join(", "));
        }
        WinetricksEvent::Stopping => {
            eprintln!("Canceling winetricks, press Ctrl+C again to force it");
        }
        WinetricksEvent::Killing => eprintln!("Killing everything left in the prefix"),
        WinetricksEvent::Warning(warning) => eprintln!("warning: {warning}"),
        WinetricksEvent::OutputLine(_) | WinetricksEvent::Done(_) => {}
    }
}

fn print_ensure_event(paths: &Paths, name: &str, event: winetricks::EnsureEvent) {
    match event {
        winetricks::EnsureEvent::Launch(event) => {
            crate::commands::build::print_event(paths, name, event);
        }
        winetricks::EnsureEvent::Winetricks(event) => print_event(event),
    }
}

#[derive(clap::Args)]
// clap's own -h would otherwise be caught before the name's trailing arguments,
// and `-h` is winetricks' own help.
#[command(disable_help_flag = true)]
pub struct Args {
    /// Print this help
    #[arg(long, action = clap::ArgAction::Help)]
    help: Option<bool>,

    /// Set an environment variable, overriding the config
    #[arg(short, long = "env", value_name = "KEY=VALUE", value_parser = parse_env_assignment)]
    env: Vec<(String, String)>,

    /// The environment, then what to pass winetricks, like `-q dotnet48`.
    /// Nothing after the name opens the winetricks GUI.
    #[arg(
        required = true,
        value_name = "NAME [WINETRICKS ARGS]...",
        trailing_var_arg = true,
        allow_hyphen_values = true,
        add = clap_complete::ArgValueCompleter::new(crate::completion::environments)
    )]
    rest: Vec<String>,
}

impl Args {
    /// Splits the environment name off the arguments winetricks gets. Keeping
    /// them one clap argument is what stops clap claiming winetricks' own
    /// flags, like `-q` or its `-e`.
    fn split(&self) -> (&str, &[String]) {
        let (name, forwarded) = self.rest.split_first().expect("at least one value");
        // An optional separator right after the name, for arguments that would
        // otherwise be hard to tell apart from wineo's own.
        match forwarded.split_first() {
            Some((first, rest)) if first == "--" => (name, rest),
            _ => (name, forwarded),
        }
    }
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let (name, forwarded) = args.split();
    let overrides: BTreeMap<String, String> = args.env.iter().cloned().collect();
    let cancellation = Cancellation::new()?;
    match winetricks::run_in(
        paths,
        name,
        &overrides,
        forwarded,
        &cancellation.token,
        Output::Inherit,
        |event| print_ensure_event(paths, name, event),
    ) {
        Ok(status) => Ok(ExitCode::from(u8::try_from(exit_code(status)).unwrap_or(1))),
        Err(err)
            if err
                .downcast_ref::<Error>()
                .is_some_and(|err| matches!(err, Error::Canceled(_))) =>
        {
            Ok(ExitCode::from(130))
        }
        Err(err) => Err(err),
    }
}
