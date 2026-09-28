use std::collections::BTreeMap;
use std::process::ExitCode;

use anyhow::bail;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::launch::parse_env_assignment;
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::prefix;
use wineo_core::runners::Runners;
use wineo_core::runtime::{Runtime, exit_code};
use wineo_core::winetricks;

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
    let global = GlobalConfig::load(paths)?;
    let environment = Environment::load(paths, name)?;
    let runner = Runners::new(paths, &global)?.resolve(&environment.wine()?.runner)?;

    let overrides: BTreeMap<String, String> = args.env.iter().cloned().collect();
    let env = winetricks::merged_env(&runner, &global, &environment, &overrides)?;
    let prefix = environment.prefix(paths)?;
    // Only ahead of a build, so a typo never waits on one. Otherwise the
    // checks below come first, as they report what to clean up.
    if !runner.is_proton() && !prefix::is_prefix(&runner.wine_prefix(&prefix)) {
        winetricks::check_available(&runner)?;
    }
    crate::commands::build::ensure_prefix(paths, &global, &environment, &runner)?;
    let locks = Locks::new(paths);
    // Held until winetricks is done and the prefix is idle. Several winetricks
    // verbs wait for every wine process to exit, so nothing else may start one.
    let _guard = locks.acquire(Operation::Winetricks, name, &[&prefix])?;
    Environment::ensure_exists(paths, name)?;
    if !prefix::is_prefix(&runner.wine_prefix(&prefix)) {
        return Err(Error::MissingPrefix(prefix).into());
    }
    // Refused before anything writes into the prefix.
    let runtime = Runtime::new(paths, name);
    if runtime.is_running()? {
        bail!("{name} is running, stop it before running winetricks");
    }
    if runtime.status().is_ok_and(|status| status.lost) {
        bail!(
            "{name}'s last run was lost, so wine may still be running in its prefix. Clean it up with `wineo stop {name}` first"
        );
    }
    runtime.check_resolved(name)?;
    crate::commands::components::reconcile_config(paths, &global, &environment, &runner, &prefix)?;

    crate::commands::desktop::reconcile(paths, &global, &environment, Some(&runner), &prefix)?;

    let status = winetricks::run(&runner, &prefix, &env, forwarded, global.stop_timeout)?;
    Ok(ExitCode::from(u8::try_from(exit_code(status)).unwrap_or(1)))
}
