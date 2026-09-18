use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::bail;
use wineo_core::edit;
use wineo_core::environment::Environment;
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::runtime::Runtime;
use wineo_core::settings::{self, ConfigFile};

fn changes_prefix(pairs: &[(&str, &str)], unset: &[&str]) -> bool {
    pairs.iter().any(|(key, _)| *key == "prefix") || unset.contains(&"prefix")
}

/// Where a `prefix=` setting would point. Quoting is optional on the command
/// line, so both spellings resolve to the same directory.
fn new_prefix(paths: &Paths, raw: &str) -> Option<PathBuf> {
    let unquoted = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            raw.strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(raw);
    paths.absolute(unquoted).ok()
}

#[derive(clap::Args)]
pub struct Args {
    /// Environment to change, left out with --global
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

    let pairs = settings
        .iter()
        .map(|setting| edit::parse_assignment(setting))
        .collect::<Result<Vec<_>, _>>()?;
    let unset: Vec<&str> = args.unset.iter().map(String::as_str).collect();

    // Pointing an environment somewhere else has to exclude whatever is using
    // either prefix, so a removal can't delete one this is about to adopt.
    let _guard = match file {
        ConfigFile::Environment(name) if changes_prefix(&pairs, &unset) => {
            let mut prefixes = Vec::new();
            if let Ok(environment) = Environment::load(paths, name)
                && let Ok(prefix) = environment.prefix(paths)
            {
                prefixes.push(prefix);
            }
            prefixes.extend(pairs.iter().filter_map(|(key, raw)| {
                (*key == "prefix").then(|| new_prefix(paths, raw)).flatten()
            }));
            let borrowed: Vec<&std::path::Path> = prefixes.iter().map(PathBuf::as_path).collect();
            Some(Locks::new(paths).acquire(Operation::Configure, name, &borrowed)?)
        }
        _ => None,
    };

    for key in settings::update(paths, file, &pairs, &unset)? {
        eprintln!("`{key}` was not set");
    }

    if let ConfigFile::Environment(name) = file
        && Runtime::new(paths, name).is_running()?
    {
        eprintln!("{name} is running, changes apply the next time it starts");
    }
    Ok(ExitCode::SUCCESS)
}
