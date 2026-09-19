use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::bail;
use wineo_core::config::GlobalConfig;
use wineo_core::edit;
use wineo_core::environment::Environment;
use wineo_core::install::{self, Wanted};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::prefix;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;
use wineo_core::runtime::Runtime;
use wineo_core::settings::{self, ConfigFile};

use crate::commands::components;

/// Settings that decide what the next launch runs, and against what. They all
/// have to exclude whatever is using the environment, so nothing launches
/// halfway between the old setting and the new one, and `dxvk` and `vkd3d`
/// additionally write into the prefix while they are applied.
fn changes_the_launch(pairs: &[(&str, &str)], unset: &[&str]) -> bool {
    ["prefix", "runner", "dxvk", "vkd3d"]
        .iter()
        .any(|key| pairs.iter().any(|(set, _)| set == key) || unset.contains(key))
}

/// The value a key will hold once this set is written, so a check can look at
/// the result rather than at the change.
fn value_after(
    current: Option<&str>,
    pairs: &[(&str, &str)],
    unset: &[&str],
    key: &str,
) -> Option<String> {
    if unset.contains(&key) {
        return None;
    }
    pairs
        .iter()
        .rev()
        .find(|(set, _)| *set == key)
        .map(|(_, raw)| unquote(raw).to_owned())
        .or_else(|| current.map(str::to_owned))
}

/// Applies `dxvk` and `vkd3d` before the config is written, so a failed
/// install leaves the config saying what the prefix actually has.
fn apply_components(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    runner: Option<&Runner>,
    pairs: &[(&str, &str)],
    unset: &[&str],
) -> anyhow::Result<()> {
    let environment = Environment::load(paths, name).ok();
    let config = environment.as_ref().map(|env| &env.config);
    let wanted = Wanted {
        dxvk: value_after(
            config.and_then(|config| config.dxvk.as_deref()),
            pairs,
            unset,
            "dxvk",
        ),
        vkd3d: value_after(
            config.and_then(|config| config.vkd3d.as_deref()),
            pairs,
            unset,
            "vkd3d",
        ),
    };
    // Nothing resolvable to check against, so leave it to the next launch.
    let Some(runner) = runner else {
        return Ok(());
    };
    // Refused whether or not there is a prefix to install into yet.
    install::check(name, runner, &wanted)?;
    let Some(prefix) = environment.as_ref().and_then(|env| env.prefix(paths).ok()) else {
        return Ok(());
    };
    // A prefix that does not exist yet is left to the next launch, which
    // reconciles anyway.
    if !prefix::is_prefix(&runner.wine_prefix(&prefix)) {
        return Ok(());
    }
    components::reconcile(paths, global, name, runner, &prefix, &wanted)
}

fn changed_runner<'a>(pairs: &[(&'a str, &'a str)]) -> Option<&'a str> {
    pairs
        .iter()
        .rev()
        .find(|(key, _)| *key == "runner")
        .map(|(_, raw)| unquote(raw))
}

/// Quoting is optional on the command line, so both spellings mean the same.
fn unquote(raw: &str) -> &str {
    raw.strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            raw.strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(raw)
}

/// Settings only a Proton environment has anything to do with.
const PROTON_ONLY: [&str; 2] = ["gameid", "store"];

/// Resolves a new runner and refuses the settings that would not survive it:
/// a change across kinds, or protonfixes on something that is not Proton.
fn check_runner(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    pairs: &[(&str, &str)],
) -> anyhow::Result<Option<Runner>> {
    let changed = changed_runner(pairs);
    let runners = Runners::new(paths, global)?;
    let current = Environment::load(paths, name)
        .ok()
        .and_then(|environment| runners.resolve(&environment.config.runner).ok());

    let runner = match changed {
        Some(changed) => {
            let new = runners.resolve(changed)?;
            // A wine prefix has no `pfx/`, and a Proton one keeps nothing where
            // a plain wine looks for it, so crossing kinds is worse than not
            // migrating.
            if let Some(current) = &current
                && current.is_proton() != new.is_proton()
            {
                bail!(
                    "`{name}` runs on {} and `{changed}` is a {} runner. Their prefix layouts are not compatible, so make a new environment with `wineo new --runner {changed}` instead",
                    current.name(),
                    new.kind_name()
                );
            }
            new
        }
        // Nothing resolvable to check against, and the write is still valid
        // TOML, so this is not the place to complain.
        None => match current {
            Some(current) => current,
            None => return Ok(None),
        },
    };
    let proton_only = pairs.iter().any(|(key, _)| PROTON_ONLY.contains(key));

    if proton_only && !runner.is_proton() {
        let key = pairs
            .iter()
            .find_map(|(key, _)| PROTON_ONLY.contains(key).then_some(*key))
            .expect("checked above");
        bail!(
            "`{key}` only means something to Proton, and `{name}` runs on {}",
            runner.name()
        );
    }
    Ok(Some(runner))
}

/// Where a `prefix=` setting would point.
fn new_prefix(paths: &Paths, raw: &str) -> Option<PathBuf> {
    paths.absolute(unquote(raw)).ok()
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
    // either prefix, so a removal can't delete one this is about to adopt. A
    // running environment holds the same lock, so this refuses that too.
    let _guard = match file {
        ConfigFile::Environment(name) if changes_the_launch(&pairs, &unset) => {
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

    // Checked under the lock, so a typo never gets written and the runner
    // cannot be removed between the check and the write.
    if let ConfigFile::Environment(name) = file {
        let global = GlobalConfig::load(paths).unwrap_or_default();
        let runner = check_runner(paths, &global, name, &pairs)?;
        apply_components(paths, &global, name, runner.as_ref(), &pairs, &unset)?;
    }

    for key in settings::update(paths, file, &pairs, &unset)? {
        eprintln!("`{key}` was not set");
    }

    // Wine writes its version into a prefix, so an older build can choke on a
    // prefix a newer one set up. Wineo does not try to detect that.
    if let Some(runner) = changed_runner(&pairs) {
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
