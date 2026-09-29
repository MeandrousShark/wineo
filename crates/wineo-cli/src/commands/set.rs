use std::process::ExitCode;

use anyhow::bail;
use wineo_core::edit;
use wineo_core::environment::Environment;
use wineo_core::paths::Paths;
use wineo_core::settings::{self, ConfigFile};

use crate::commands::{components, desktop};

#[derive(clap::Args)]
pub struct Args {
    /// Environment to change, left out with --global
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: Option<String>,

    /// Settings to change, like runner=wine or targets.app.exe='C:\app.exe'
    #[arg(value_name = "KEY=VALUE")]
    settings: Vec<String>,

    /// Remove a setting
    #[arg(long, value_name = "KEY")]
    unset: Vec<String>,

    /// Change the global config instead of an environment
    #[arg(long)]
    global: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let mut settings = args.settings;
    let file = match (args.global, args.name.as_deref()) {
        (true, name) => {
            // clap can't tell the first setting from a name, but names never contain `=`.
            if let Some(first) = name {
                settings.insert(0, first.to_owned());
            }
            ConfigFile::Global
        }
        (false, Some(name)) if !name.contains('=') => ConfigFile::Environment(name),
        (false, _) => bail!("name an environment before the settings, or pass --global"),
    };
    if settings.is_empty() && args.unset.is_empty() {
        bail!("nothing to change, pass KEY=VALUE settings or --unset KEY");
    }
    if let ConfigFile::Environment(name) = file {
        Environment::ensure_exists(paths, name)?;
    }
    let pairs = settings
        .iter()
        .map(|setting| edit::parse_assignment(setting))
        .collect::<Result<Vec<_>, _>>()?;
    let unset: Vec<&str> = args.unset.iter().map(String::as_str).collect();

    let applied = settings::apply(paths, file, &pairs, &unset).map_err(|err| {
        if let (
            ConfigFile::Environment(name),
            Some(settings::CheckError::RunnerKindMismatch { current, changed, kind }),
        ) = (file, err.downcast_ref::<settings::CheckError>())
        {
            anyhow::anyhow!(
                "`{name}` runs on {current} and `{changed}` is a {kind} runner. Their prefix layouts are not compatible, so make a new environment with `wineo new --runner {changed}` instead"
            )
        } else {
            err
        }
    })?;
    // Only an environment gets component, desktop, or running reports.
    let name = match file {
        ConfigFile::Environment(name) => name,
        ConfigFile::Global => "",
    };
    for change in &applied.components {
        eprintln!("{}", components::describe(change, name));
    }
    for key in &applied.not_set {
        eprintln!("`{key}` was not set");
    }
    for change in &applied.desktop {
        eprintln!("{}", desktop::describe(change, name));
    }
    if let Some(runner) = &applied.changed_runner {
        eprintln!(
            "The prefix was not migrated. If it was set up by a newer wine than `{runner}`, it may not work"
        );
    }
    if applied.running {
        eprintln!("{name} is running, changes apply the next time it starts");
    }
    if let Some(err) = applied.failed {
        return Err(err);
    }
    Ok(ExitCode::SUCCESS)
}
