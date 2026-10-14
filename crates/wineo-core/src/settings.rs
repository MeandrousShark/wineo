use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::bail;
use toml_edit::DocumentMut;

use crate::art::TargetCleanup;
use crate::build_lock;
use crate::config::{
    self, EnvironmentConfig, GlobalConfig, Kind, Launcher, WINE_ONLY_KEYS, native_key_message,
    proxied_key_message,
};
use crate::desktop;
use crate::edit::{self, Scope};
use crate::environment::Environment;
use crate::error::{Error, IoContext, Result};
use crate::files::{self, write_atomic};
use crate::gog;
use crate::install::{self, Change, Wanted};
use crate::launch;
use crate::name;
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::prefix;
use crate::runner::Runner;
use crate::runners::Runners;
use crate::runtime::Runtime;

/// Which config file `update` edits.
#[derive(Debug, Clone, Copy)]
pub enum ConfigFile<'a> {
    Global,
    Environment(&'a str),
}

/// Validation failure that the caller may render with its own recovery hint.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("runner layouts are not compatible")]
    RunnerKindMismatch {
        current: String,
        changed: String,
        kind: String,
    },
}

/// What `apply` did, in the order it happened.
#[derive(Debug, Default)]
pub struct Applied {
    /// Components installed or removed before the config was written.
    pub components: Vec<Change>,
    /// Keys asked to be removed that weren't set.
    pub not_set: Vec<String>,
    pub desktop: Vec<desktop::Change>,
    /// The runner this set switched to. The prefix is not migrated, and one
    /// set up by a newer wine may not work with it.
    pub changed_runner: Option<String>,
    /// The environment is running, so the changes apply the next time it starts.
    pub running: bool,
    /// What stopped `apply` once its checks had passed, so what happened up
    /// to then can still be reported. The other fields hold whatever
    /// completed before the failure: components may be empty, and the config
    /// may already be written when a later step, such as desktop entries,
    /// failed.
    pub failed: Option<anyhow::Error>,
}

/// Locks, checks, writes, and reconciles desktop entries for one settings
/// change, the whole of what `wineo set` does once its arguments are parsed.
/// An error means nothing was changed.
pub fn apply(
    paths: &Paths,
    file: ConfigFile,
    pairs: &[(&str, &str)],
    unset: &[&str],
) -> anyhow::Result<Applied> {
    if let ConfigFile::Environment(name) = file
        && (pairs.iter().any(|(key, _)| *key == "kind") || unset.contains(&"kind"))
    {
        bail!("{name}'s kind can't be changed, make a new environment instead");
    }
    if let ConfigFile::Environment(_) = file
        && pairs.iter().any(|(key, _)| *key == "gog_id")
    {
        return Err(edit::read_only_gog_id().into());
    }

    // Pointing an environment somewhere else has to exclude whatever is using
    // either prefix, so a removal can't delete one this is about to adopt. A
    // running environment holds the same lock, so this refuses that too.
    let guard = match file {
        // Unsetting gog_id lets go of a game folder, which takes the folder
        // locks, and those come after the operation lock.
        ConfigFile::Environment(name)
            if changes_the_launch(pairs, unset) || unset.contains(&"gog_id") =>
        {
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
            let guard = Locks::new(paths).acquire(Operation::Configure, name, &borrowed)?;
            // An installer that never confirmed it was done may still have
            // programs running in the prefix these settings reach into.
            Runtime::new(paths, name).check_resolved(name)?;
            for (_, raw) in pairs.iter().filter(|(key, _)| *key == "prefix") {
                if let Some(prefix) = new_prefix(paths, raw) {
                    crate::environment::check_nesting(paths, name, &prefix)?;
                }
            }
            Some(guard)
        }
        _ => None,
    };

    // Unsetting gog_id lets go of the game folder. A folder mid-update is
    // refused before anything changes. GOG games live in the host library,
    // so a portable environment never owns one.
    let gog = match file {
        ConfigFile::Environment(name)
            if unset.contains(&"gog_id") && paths.portable_root().is_none() =>
        {
            let strict = gog::folder::may_track_game(paths, name);
            let held = gog::folder::lock_owned(paths, name, strict, &|| false)?;
            if held.iter().any(|(_, state)| state.journal.is_some()) {
                return Err(gog::folder::FolderError::UnsetMidUpdate {
                    name: name.to_owned(),
                }
                .into());
            }
            held
        }
        _ => Vec::new(),
    };

    let mut applied = Applied::default();
    // Checked under the lock, so a typo never gets written and the runner
    // cannot be removed between the check and the write.
    let _build_guards = if let ConfigFile::Environment(name) = file {
        let global = GlobalConfig::load_or_default(paths)?;
        let build_guards = build_lock::config(
            paths,
            &global,
            new_setting(pairs, "runner"),
            new_setting(pairs, "dxvk"),
            new_setting(pairs, "vkd3d"),
        )?;
        applied.components = check(paths, &global, name, pairs, unset)?;
        build_guards
    } else {
        Vec::new()
    };
    // Released before the config edit, like `rm`, so a crash in between
    // leaves an environment whose folder has no owner, which claiming heals.
    for (lock, _) in &gog {
        lock.release()?;
    }
    if let Err(err) = write_and_follow(paths, file, pairs, unset, guard.is_some(), &mut applied) {
        applied.failed = Some(err);
    }
    // Held through the desktop step, so its icon extraction runs under it.
    drop(guard);
    Ok(applied)
}

/// The part of `apply` that runs once components may have changed.
fn write_and_follow(
    paths: &Paths,
    file: ConfigFile,
    pairs: &[(&str, &str)],
    unset: &[&str],
    held: bool,
    applied: &mut Applied,
) -> anyhow::Result<()> {
    applied.not_set = update(paths, file, pairs, unset)?;
    if let ConfigFile::Environment(name) = file {
        applied.desktop = desktop::apply(paths, name, held)?;
    }
    // Wine writes its version into a prefix, so an older build can choke on a
    // prefix a newer one set up. Wineo does not try to detect that.
    applied.changed_runner = changed_runner(pairs).map(str::to_owned);
    if let ConfigFile::Environment(name) = file {
        applied.running = Runtime::new(paths, name).is_running()?;
    }
    Ok(())
}

/// Validates environment settings and reconciles installed components before
/// the config is written. Callers hold the environment lock while checking
/// and updating, so the prefix and config cannot diverge during a launch.
pub fn check(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    pairs: &[(&str, &str)],
    unset: &[&str],
) -> anyhow::Result<Vec<Change>> {
    check_native(paths, global, name, pairs, unset)?;
    let runner = check_runner(paths, global, name, pairs)?;
    check_desktop(paths, name, pairs, unset)?;
    apply_components(
        paths,
        global,
        name,
        runner.as_ref().map(|(runner, _guard)| runner),
        pairs,
        unset,
    )
}

/// Settings that decide what the next launch runs, and against what. They all
/// have to exclude whatever is using the environment, so nothing launches
/// halfway between the old setting and the new one, and `dxvk` and `vkd3d`
/// additionally write into the prefix while they are applied.
pub fn changes_the_launch(pairs: &[(&str, &str)], unset: &[&str]) -> bool {
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
) -> anyhow::Result<Vec<Change>> {
    let environment = Environment::load(paths, name).ok();
    let config = environment.as_ref().map(|env| &env.config);
    let wanted = Wanted {
        dxvk: value_after(
            config.and_then(|config| config.wine()?.dxvk.as_deref()),
            pairs,
            unset,
            "dxvk",
        ),
        vkd3d: value_after(
            config.and_then(|config| config.wine()?.vkd3d.as_deref()),
            pairs,
            unset,
            "vkd3d",
        ),
    };
    // Nothing resolvable to check against, so leave it to the next launch.
    let Some(runner) = runner else {
        return Ok(Vec::new());
    };
    // Refused whether or not there is a prefix to install into yet.
    install::check(name, runner, &wanted)?;
    let Some(prefix) = environment.as_ref().and_then(|env| env.prefix(paths).ok()) else {
        return Ok(Vec::new());
    };
    // A prefix that does not exist yet is left to the next launch, which
    // reconciles anyway.
    if !prefix::is_prefix(&runner.wine_prefix(&prefix)) {
        return Ok(Vec::new());
    }
    Ok(install::reconcile(
        paths, global, name, runner, &prefix, &wanted,
    )?)
}

/// The targets a set turns a desktop entry on for.
fn enabled_desktops(pairs: &[(&str, &str)]) -> Vec<String> {
    pairs
        .iter()
        .filter_map(|(key, raw)| {
            let (target, field) = key.strip_prefix("targets.")?.split_once('.')?;
            (field == "desktop" && unquote(raw) == "true").then(|| target.to_owned())
        })
        .collect()
}

/// Refuses a `desktop = true` whose target points at something that is not
/// there, before anything is written, so a typo never leaves a config asking
/// for an entry that could not be made. A target with no `exe` at all is left
/// to the schema, which requires one.
fn check_desktop(
    paths: &Paths,
    name: &str,
    pairs: &[(&str, &str)],
    unset: &[&str],
) -> anyhow::Result<()> {
    let enabled = enabled_desktops(pairs);
    let proxied_enabled = pairs.iter().any(|(key, raw)| {
        *key == "desktop"
            && unquote(raw) == "true"
            && Environment::load(paths, name)
                .is_ok_and(|environment| matches!(environment.config.kind, Kind::Proxied(_)))
    });
    if enabled.is_empty() && !proxied_enabled {
        return Ok(());
    }
    desktop::supported(paths)?;
    if proxied_enabled {
        return Ok(());
    }
    let environment = Environment::load(paths, name).ok();
    let Some(prefix) = environment.as_ref().and_then(|env| env.prefix(paths).ok()) else {
        return Ok(());
    };
    // No prefix yet means nothing to resolve against, so the next launch,
    // which reconciles anyway, is where this gets checked.
    if !prefix::holds_prefix(&prefix) {
        return Ok(());
    }
    for target in enabled {
        let current = environment
            .as_ref()
            .and_then(|env| env.config.targets.get(&target))
            .map(|target| target.exe.as_str());
        if let Some(exe) = value_after(current, pairs, unset, &format!("targets.{target}.exe")) {
            desktop::check(paths, &prefix, &exe)?;
        }
    }
    Ok(())
}

/// Refuses the keys a native environment can't have, with the same line its
/// config would be refused with, and checks the exes it is given the way a
/// launch resolves them. Nothing to do for a Wine environment.
fn check_native(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    pairs: &[(&str, &str)],
    unset: &[&str],
) -> anyhow::Result<()> {
    let Ok(environment) = Environment::load(paths, name) else {
        return Ok(());
    };
    match &environment.config.kind {
        Kind::Wine(_) => return Ok(()),
        Kind::Proxied(proxied) => return check_proxied(name, proxied.launcher, pairs),
        Kind::Native => {}
    }
    if let Some((key, _)) = pairs.iter().find(|(key, _)| WINE_ONLY_KEYS.contains(key)) {
        bail!("{}", native_key_message(name, key));
    }
    let mut targets: Vec<String> = pairs
        .iter()
        .filter_map(|(key, _)| {
            let (target, field) = key.strip_prefix("targets.")?.split_once('.')?;
            (field == "exe").then(|| target.to_owned())
        })
        .collect();
    targets.extend(enabled_desktops(pairs));
    for target in targets {
        let current = environment
            .config
            .targets
            .get(&target)
            .map(|target| target.exe.as_str());
        if let Some(exe) = value_after(current, pairs, unset, &format!("targets.{target}.exe")) {
            launch::check_native_exe(paths, global, name, &environment.config.env, &exe)?;
        }
    }
    Ok(())
}

/// Refuses every key a proxied environment can't have, with the same line
/// its config would be refused with.
fn check_proxied(name: &str, launcher: Launcher, pairs: &[(&str, &str)]) -> anyhow::Result<()> {
    for (key, _) in pairs {
        let top = key.split('.').next().unwrap_or(key);
        if !PROXIED_SETTABLE.contains(&top) {
            bail!("{}", proxied_key_message(name, launcher, top));
        }
    }
    Ok(())
}

/// What `set` can change on a proxied environment.
const PROXIED_SETTABLE: [&str; 4] = ["tags", "title", "hooks", "desktop"];

pub fn changed_runner<'a>(pairs: &[(&'a str, &'a str)]) -> Option<&'a str> {
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

fn new_setting<'a>(set: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
    set.iter()
        .rev()
        .find(|(candidate, _)| *candidate == key)
        .map(|(_, raw)| unquote(raw))
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
) -> anyhow::Result<Option<(Runner, Option<build_lock::Guard>)>> {
    let changed = changed_runner(pairs);
    let runners = Runners::new(paths, global)?;
    let current = Environment::load(paths, name).ok().and_then(|environment| {
        runners
            .resolve_locked(paths, &environment.wine().ok()?.runner)
            .ok()
    });

    let runner = match changed {
        Some(changed) => {
            let new = runners.resolve_locked(paths, changed)?;
            // A wine prefix has no `pfx/`, and a Proton one keeps nothing where
            // a plain wine looks for it, so crossing kinds is worse than not
            // migrating.
            if let Some(current) = &current
                && current.0.is_proton() != new.0.is_proton()
            {
                bail!(CheckError::RunnerKindMismatch {
                    current: current.0.name().to_owned(),
                    changed: changed.to_owned(),
                    kind: new.0.kind_name().to_owned(),
                });
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

    if proton_only && !runner.0.is_proton() {
        let key = pairs
            .iter()
            .find_map(|(key, _)| PROTON_ONLY.contains(key).then_some(*key))
            .expect("checked above");
        bail!(
            "`{key}` only means something to Proton, and `{name}` runs on {}",
            runner.0.name()
        );
    }
    Ok(Some(runner))
}

/// Where a `prefix=` setting would point.
pub fn new_prefix(paths: &Paths, raw: &str) -> Option<PathBuf> {
    paths.absolute(unquote(raw)).ok()
}

/// Applies `key=value` settings and removals to a config file, keeping its
/// formatting. Nothing is written unless the result passes validation.
/// Returns the keys asked to be removed that weren't set.
pub fn update(
    paths: &Paths,
    file: ConfigFile,
    set: &[(&str, &str)],
    unset: &[&str],
) -> Result<Vec<String>> {
    update_with_writer(paths, file, set, unset, |path, contents| {
        write_atomic(path, contents)
    })
}

/// Records the answer to the offer to use runners from Steam, Heroic, and
/// Lutris, keeping the global config's formatting like any other edit.
pub fn set_external_runners(paths: &Paths, on: bool) -> Result<()> {
    let value = if on { "true" } else { "false" };
    update(
        paths,
        ConfigFile::Global,
        &[("external_runners", value)],
        &[],
    )
    .map(|_| ())
}

fn update_with_writer(
    paths: &Paths,
    file: ConfigFile,
    set: &[(&str, &str)],
    unset: &[&str],
    write: impl FnOnce(&std::path::Path, &str) -> Result<()>,
) -> Result<Vec<String>> {
    let (scope, path) = config_path(paths, file)?;
    // Read under the lock, so a change or a removal that lands in the meantime
    // is built on or refused rather than overwritten.
    let _guard = Locks::new(paths).config(&path)?;
    let text = files::read_to_string_optional(&path)
        .context(|| format!("failed to read {}", path.display()))?;
    if text.is_none()
        && let ConfigFile::Environment(env) = file
    {
        return Err(Error::UnknownEnvironment(env.to_owned()));
    }
    let mut doc = match &text {
        Some(text) => {
            config::check_schema(&path, text)?;
            text.parse().map_err(|source| Error::ConfigParse {
                path: path.clone(),
                source: Box::new(source),
            })?
        }
        None => config::new_document(),
    };
    let before = doc.to_string();
    let targets_before = target_names(&doc);

    for (key, value) in set {
        let value = match (scope, *key) {
            // Stored the same way `wineo new` writes them.
            (Scope::Environment, "prefix") => paths.store_prefix(&paths.absolute(value)?)?,
            (Scope::Global, "defaults.prefix_root") => {
                let dir = paths.absolute(value)?;
                paths.check_prefix_root(&dir)?;
                paths.store(&dir)?
            }
            // GOG games live in the host library, so this is never
            // relative to a portable folder.
            (Scope::Global, "defaults.games_root") => paths.contract(&paths.absolute(value)?)?,
            (Scope::Environment, key)
                if key.starts_with("targets.")
                    && (key.ends_with(".exe") || key.ends_with(".cwd")) =>
            {
                paths.store_value(value)?
            }
            _ => (*value).to_owned(),
        };
        edit::set(&mut doc, scope, key, &value)?;
    }
    if paths.portable_root().is_some() && scope == Scope::Environment {
        for (key, _) in set {
            let Some((target, "desktop")) = key
                .strip_prefix("targets.")
                .and_then(|rest| rest.rsplit_once('.'))
            else {
                continue;
            };
            let enabled = doc
                .get("targets")
                .and_then(|targets| targets.get(target))
                .and_then(|table| table.get("desktop"))
                .and_then(|value| value.as_bool());
            if enabled == Some(true) {
                return Err(Error::PortableDesktop);
            }
        }
    }
    let mut missing = Vec::new();
    for key in unset {
        if !edit::unset(&mut doc, scope, key)? {
            missing.push((*key).to_owned());
        }
    }

    let mut updated = doc.to_string();
    let changed = updated != before;
    if changed {
        config::stamp_schema(&mut doc);
        updated = doc.to_string();
    }
    let (global, environment) = match scope {
        Scope::Global => (Some(GlobalConfig::parse(&path, &updated)?), None),
        Scope::Environment => {
            let environment = EnvironmentConfig::parse(&path, &updated)?;
            (None, Some(environment))
        }
    };
    let cleanup = match file {
        ConfigFile::Environment(env) => {
            let targets_after = target_names(&doc);
            let removed: Vec<String> = targets_before.difference(&targets_after).cloned().collect();
            if removed.is_empty() {
                None
            } else {
                Some(TargetCleanup::prepare(paths, env, removed)?)
            }
        }
        ConfigFile::Global => None,
    };
    if changed {
        let wine = environment.as_ref().and_then(EnvironmentConfig::wine);
        let runner = match scope {
            Scope::Global if set.iter().any(|(key, _)| *key == "defaults.runner") => global
                .as_ref()
                .map(|global| global.defaults.runner.as_str()),
            Scope::Environment if set.iter().any(|(key, _)| *key == "runner") => {
                wine.map(|wine| wine.runner.as_str())
            }
            _ => None,
        };
        let dxvk = set
            .iter()
            .any(|(key, _)| *key == "dxvk")
            .then(|| wine.and_then(|wine| wine.dxvk.as_deref()))
            .flatten();
        let vkd3d = set
            .iter()
            .any(|(key, _)| *key == "vkd3d")
            .then(|| wine.and_then(|wine| wine.vkd3d.as_deref()))
            .flatten();
        if runner.is_some() || dxvk.is_some() || vkd3d.is_some() {
            let loaded_global;
            let global = match &global {
                Some(global) => global,
                None => {
                    loaded_global = GlobalConfig::load_or_default(paths)?;
                    &loaded_global
                }
            };
            write_with_build_locks(paths, global, runner, dxvk, vkd3d, || {
                write(&path, &updated)
            })?;
        } else {
            write(&path, &updated)?;
        }
    }
    if let Some(cleanup) = cleanup {
        cleanup.finish()?;
    }
    Ok(missing)
}

fn write_with_build_locks(
    paths: &Paths,
    global: &GlobalConfig,
    runner: Option<&str>,
    dxvk: Option<&str>,
    vkd3d: Option<&str>,
    write: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let _guards = build_lock::config(paths, global, runner, dxvk, vkd3d)?;
    write()
}

fn target_names(doc: &DocumentMut) -> BTreeSet<String> {
    doc.get("targets")
        .and_then(|targets| targets.as_table_like())
        .map(|targets| targets.iter().map(|(name, _)| name.to_owned()).collect())
        .unwrap_or_default()
}

fn config_path(paths: &Paths, file: ConfigFile) -> Result<(Scope, PathBuf)> {
    match file {
        ConfigFile::Global => Ok((Scope::Global, paths.global_config())),
        ConfigFile::Environment(env) => {
            if !name::is_valid(env) {
                return Err(Error::InvalidName(env.to_owned()));
            }
            Ok((Scope::Environment, paths.environment_config(env)))
        }
    }
}

#[cfg(test)]
mod tests;
