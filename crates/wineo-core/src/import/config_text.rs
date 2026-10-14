//! Building an imported environment's config text and writing it exactly once.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use crate::config::{Arch, EnvironmentConfig, validate_env_var};
use crate::environment::Environment;
use crate::error::Error;
use crate::files;
use crate::name;
use crate::operation::{Locks, Operation};
use crate::paths::Paths;

use super::ImportedSettings;

/// Target paths in the config form `paths` stores them in.
pub(super) fn stored_settings(
    paths: &Paths,
    settings: &ImportedSettings,
) -> Result<ImportedSettings> {
    let mut stored = settings.clone();
    for target in stored.targets.values_mut() {
        target.exe = paths.store_value(&target.exe)?;
        if let Some(cwd) = &mut target.cwd {
            *cwd = paths.store_value(cwd)?;
        }
    }
    Ok(stored)
}

pub(super) fn config_text(
    prefix: &str,
    runner: &str,
    arch: Arch,
    settings: Option<&ImportedSettings>,
) -> Result<String> {
    let mut doc = crate::config::new_document();
    doc["prefix"] = value(prefix);
    doc["runner"] = value(runner);
    doc["arch"] = value(arch.as_str());
    finish_config(doc, settings)
}

/// A native environment's config, with nothing about a prefix or runner.
fn native_config_text(settings: &ImportedSettings) -> Result<String> {
    let mut doc = crate::config::new_document();
    doc["kind"] = value("native");
    finish_config(doc, Some(settings))
}

fn finish_config(mut doc: DocumentMut, settings: Option<&ImportedSettings>) -> Result<String> {
    if let Some(settings) = settings {
        for (key, env_value) in &settings.env {
            validate_env_var(key, env_value).map_err(|message| anyhow!(message))?;
        }
        if let Some(gamemode) = settings.gamemode {
            doc["gamemode"] = Value::from(gamemode).into();
        }
        if let Some(wayland) = settings.wayland {
            doc["wayland"] = value(wayland);
        }
        if let Some(mangohud) = settings.mangohud {
            doc["mangohud"] = value(mangohud);
        }
        if let Some(gamescope) = &settings.gamescope {
            doc["gamescope"] = gamescope.to_item();
        }
        if let Some(hooks) = &settings.hooks {
            doc["hooks"] = hooks.to_item();
        }
        if let Some(gameid) = &settings.gameid {
            doc["gameid"] = value(gameid);
        }
        if let Some(store) = &settings.store {
            doc["store"] = value(store);
        }
        if let Some(dxvk) = &settings.dxvk {
            doc["dxvk"] = value(dxvk);
        }
        if let Some(vkd3d) = &settings.vkd3d {
            doc["vkd3d"] = value(vkd3d);
        }
        if let Some(heroic_id) = &settings.heroic_id {
            doc["heroic_id"] = value(heroic_id);
        }
        if let Some(gog_id) = &settings.gog_id {
            doc["gog_id"] = value(gog_id);
        }
        if !settings.env.is_empty() {
            let mut table = Table::new();
            for (key, env_value) in &settings.env {
                table[key] = value(env_value);
            }
            doc["env"] = Item::Table(table);
        }
        if !settings.targets.is_empty() {
            let mut targets = Table::new();
            for (name, target) in &settings.targets {
                if !crate::name::is_valid(name) {
                    bail!("invalid target name `{name}`");
                }
                let mut table = Table::new();
                table["exe"] = value(&target.exe);
                if !target.args.is_empty() {
                    let args: Array = target.args.iter().map(String::as_str).collect();
                    table["args"] = Value::Array(args).into();
                }
                if let Some(cwd) = &target.cwd {
                    table["cwd"] = value(cwd);
                }
                if target.default {
                    table["default"] = Value::from(true).into();
                }
                if let Some(gamemode) = target.gamemode {
                    table["gamemode"] = Value::from(gamemode).into();
                }
                if target.desktop {
                    table["desktop"] = Value::from(true).into();
                }
                if let Some(display_name) = &target.name {
                    table["name"] = value(display_name);
                }
                targets[name] = Item::Table(table);
            }
            doc["targets"] = Item::Table(targets);
        }
    }
    let text = doc.to_string();
    let path = Path::new("<import>");
    EnvironmentConfig::parse(path, &text)?;
    Ok(text)
}

/// Registers a native environment from imported settings, which has no
/// prefix to copy or adopt. The config is written once, so an environment
/// that appeared meanwhile is never replaced.
pub fn register_native(
    paths: &Paths,
    name: &str,
    settings: &ImportedSettings,
) -> Result<Environment> {
    if !name::is_valid(name) {
        bail!("invalid environment name `{name}`");
    }
    let config_path = paths.environment_config(name);
    if config_path.exists() {
        return Err(Error::EnvironmentExists(name.to_owned()).into());
    }
    let text = native_config_text(&stored_settings(paths, settings)?)?;
    let _operation = Locks::new(paths).acquire(Operation::Create, name, &[])?;
    write_native_config(paths, name, &text)
}

/// The config `register_native` writes for `settings`, checked but not
/// written.
pub(crate) fn native_config(paths: &Paths, settings: &ImportedSettings) -> Result<String> {
    native_config_text(&stored_settings(paths, settings)?)
}

/// Writes a native environment's config once, for a caller that already
/// holds the environment's operation lock.
pub(crate) fn write_native_config(paths: &Paths, name: &str, text: &str) -> Result<Environment> {
    let config_path = paths.environment_config(name);
    let _config = Locks::new(paths).config(&config_path)?;
    write_config_once(&config_path, text)?;
    Ok(Environment::load(paths, name)?)
}

pub(crate) fn write_config_once(path: &Path, contents: &str) -> Result<()> {
    #[cfg(test)]
    BEFORE_CONFIG_WRITE.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(path);
        }
    });
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("config has no parent: {}", path.display()))?;
    // On first use the directories themselves are new, and each one's entry
    // in its parent has to be on disk too, or the config could vanish with
    // them after a power loss.
    let mut missing = Vec::new();
    let mut current = parent;
    while !current.exists() {
        missing.push(current.to_owned());
        match current.parent() {
            Some(up) => current = up,
            None => break,
        }
    }
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    for dir in missing.iter().rev() {
        if let Some(up) = dir.parent() {
            sync_config_dir(up)?;
        }
    }
    let staging = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create a config staging file in {}",
            parent.display()
        )
    })?;
    let staging_path = staging.path().to_owned();
    staging.as_file().write_all(contents.as_bytes())?;
    staging.as_file().sync_all()?;
    match fs::hard_link(&staging_path, path) {
        Ok(()) => {}
        Err(err) => {
            if err.kind() == io::ErrorKind::AlreadyExists {
                bail!("config already exists: {}", path.display());
            }
            return Err(err).with_context(|| format!("failed to register {}", path.display()));
        }
    }
    // The new name has to be on disk before anything that relies on the
    // environment existing, like a GOG folder state naming it, is written.
    // The staging name goes first, so a power loss can't leave it behind.
    drop(staging);
    sync_config_dir(parent)
}

fn sync_config_dir(dir: &Path) -> Result<()> {
    #[cfg(test)]
    SYNCED_DIRS.with(|synced| synced.borrow_mut().push(dir.to_owned()));
    files::sync_dir(dir).with_context(|| format!("failed to sync {}", dir.display()))
}

#[cfg(test)]
type ConfigWriteHook = Box<dyn FnMut(&Path)>;

#[cfg(test)]
thread_local! {
    /// Runs as `write_config_once` starts on this thread, so a test can look
    /// at what registration still holds.
    pub(super) static BEFORE_CONFIG_WRITE: std::cell::RefCell<Option<ConfigWriteHook>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    /// Every directory `write_config_once` synced on this thread, in order.
    pub(super) static SYNCED_DIRS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}
