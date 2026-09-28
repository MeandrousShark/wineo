use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::bail;
use wineo_core::config::GlobalConfig;
use wineo_core::edit;
use wineo_core::environment::Environment;
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::runtime::Runtime;
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
    if let ConfigFile::Environment(name) = file
        && (pairs.iter().any(|(key, _)| *key == "kind") || unset.contains(&"kind"))
    {
        bail!("{name}'s kind can't be changed, make a new environment instead");
    }

    // Pointing an environment somewhere else has to exclude whatever is using
    // either prefix, so a removal can't delete one this is about to adopt. A
    // running environment holds the same lock, so this refuses that too.
    let guard = match file {
        ConfigFile::Environment(name) if settings::changes_the_launch(&pairs, &unset) => {
            let mut prefixes = Vec::new();
            if let Ok(environment) = Environment::load(paths, name)
                && let Ok(prefix) = environment.prefix(paths)
            {
                prefixes.push(prefix);
            }
            prefixes.extend(pairs.iter().filter_map(|(key, raw)| {
                (*key == "prefix")
                    .then(|| settings::new_prefix(paths, raw))
                    .flatten()
            }));
            let borrowed: Vec<&std::path::Path> = prefixes.iter().map(PathBuf::as_path).collect();
            let guard = Locks::new(paths).acquire(Operation::Configure, name, &borrowed)?;
            // An installer that never confirmed it was done may still have
            // programs running in the prefix these settings reach into.
            Runtime::new(paths, name).check_resolved(name)?;
            Some(guard)
        }
        _ => None,
    };

    // Checked under the lock, so a typo never gets written and the runner
    // cannot be removed between the check and the write.
    if let ConfigFile::Environment(name) = file {
        let global = GlobalConfig::load_or_default(paths)?;
        let changes = settings::check(paths, &global, name, &pairs, &unset).map_err(|err| {
            if let Some(settings::CheckError::RunnerKindMismatch { current, changed, kind }) =
                err.downcast_ref::<settings::CheckError>()
            {
                anyhow::anyhow!(
                    "`{name}` runs on {current} and `{changed}` is a {kind} runner. Their prefix layouts are not compatible, so make a new environment with `wineo new --runner {changed}` instead"
                )
            } else {
                err
            }
        })?;
        for change in changes {
            eprintln!("{}", components::describe(&change, name));
        }
    }

    for key in settings::update(paths, file, &pairs, &unset)? {
        eprintln!("`{key}` was not set");
    }

    if let ConfigFile::Environment(name) = file {
        desktop::apply(paths, name, guard.is_some())?;
    }

    // Wine writes its version into a prefix, so an older build can choke on a
    // prefix a newer one set up. Wineo does not try to detect that.
    if let Some(runner) = settings::changed_runner(&pairs) {
        eprintln!(
            "The prefix was not migrated. If it was set up by a newer wine than `{runner}`, it may not work"
        );
    }

    if let ConfigFile::Environment(name) = file
        && Runtime::new(paths, name).is_running()?
    {
        eprintln!("{name} is running, changes apply the next time it starts");
    }
    Ok(ExitCode::SUCCESS)
}
