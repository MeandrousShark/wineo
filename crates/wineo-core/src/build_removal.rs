//! References and removal shared by runners and downloaded components.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rustix::fs::{CWD, RenameFlags, renameat_with};

use crate::build_lock::{Kind, Lock};
use crate::components::{Builds, Component};
use crate::config::GlobalConfig;
use crate::download::InstallLock;
use crate::environment::{self, Environment};
use crate::error::{Error, IoContext, Result};
use crate::files;
use crate::paths::Paths;
use crate::report::OverviewError;
use crate::runners::Runners;
use crate::runtime::{Runtime, State};
use crate::templates::Templates;

/// Listing skips and reports unreadable configs, like an environment overview.
/// Removal refuses on them instead, see `refuse_unreadable`.
#[derive(Debug, Default)]
pub struct Uses {
    pub labels: Vec<String>,
    pub errors: Vec<OverviewError>,
}

#[derive(Debug)]
pub struct Removed {
    pub bytes: u64,
}

pub fn uses(paths: &Paths, build: &Lock) -> Result<Uses> {
    let global = GlobalConfig::load(paths)?;
    let mut index = UsesIndex::read(paths, &global, &[build.kind()])?;
    Ok(index.take(build.kind(), build.name()))
}

/// References for every installed build, with each config read and selection resolved once.
pub fn uses_all(paths: &Paths) -> Result<HashMap<(Kind, String), Uses>> {
    let global = GlobalConfig::load(paths)?;
    let runners = Runners::new(paths, &global)?;
    let mut installed: Vec<_> = runners
        .list()?
        .runners
        .into_iter()
        .map(|runner| (Kind::Runner, runner.name().to_owned()))
        .collect();
    for component in Component::ALL {
        installed.extend(
            Builds::new(paths, &global, component)?
                .list()?
                .builds
                .into_iter()
                .map(|build| (component.into(), build.name().to_owned())),
        );
    }
    let kinds: Vec<_> = [Kind::Runner, Kind::Dxvk, Kind::Vkd3d]
        .into_iter()
        .filter(|kind| installed.iter().any(|(installed, _)| installed == kind))
        .collect();
    if kinds.is_empty() {
        return Ok(HashMap::new());
    }
    let mut index = UsesIndex::read(paths, &global, &kinds)?;
    Ok(installed
        .into_iter()
        .map(|(kind, name)| {
            let uses = index.take(kind, &name);
            ((kind, name), uses)
        })
        .collect())
}

#[derive(Default)]
struct UsesIndex {
    labels: HashMap<(Kind, String), Vec<String>>,
    errors: Vec<OverviewError>,
}

impl UsesIndex {
    fn read(paths: &Paths, global: &GlobalConfig, kinds: &[Kind]) -> Result<Self> {
        let runners = Runners::new(paths, global)?;
        let mut components: HashMap<Kind, Builds> = HashMap::new();
        for component in Component::ALL {
            if kinds.contains(&component.into()) {
                components.insert(component.into(), Builds::new(paths, global, component)?);
            }
        }
        let mut resolved = HashMap::new();
        let mut result = Self::default();
        let mut reference = |kind: Kind, subject: &str, asked: &str, selected: &str| {
            let concrete =
                resolved
                    .entry((kind, selected.to_owned()))
                    .or_insert_with(|| match kind {
                        Kind::Runner => runners.resolve(selected).ok().map(|r| r.name().to_owned()),
                        _ => components[&kind]
                            .resolve(selected)
                            .ok()
                            .map(|b| b.name().to_owned()),
                    });
            // An explicit reference counts even when its build is currently unusable.
            result
                .labels
                .entry((kind, selected.to_owned()))
                .or_default()
                .push(reference_label(subject, asked, selected));
            if let Some(concrete) = concrete.as_deref().filter(|name| *name != selected) {
                result
                    .labels
                    .entry((kind, concrete.to_owned()))
                    .or_default()
                    .push(reference_label(subject, asked, concrete));
            }
        };
        for name in environment::names(paths)? {
            match Environment::load(paths, &name) {
                Ok(environment) => {
                    if let Some(wine) = environment.config.wine() {
                        for kind in kinds {
                            let asked = match kind {
                                Kind::Runner => Some(wine.runner.as_str()),
                                Kind::Dxvk => wine.dxvk.as_deref(),
                                Kind::Vkd3d => wine.vkd3d.as_deref(),
                            };
                            if let Some(asked) = asked {
                                reference(*kind, &name, asked, asked);
                            }
                        }
                    }
                }
                Err(err) => result.errors.push(OverviewError {
                    name,
                    message: format!("{:#}", anyhow::Error::from(err)),
                }),
            }
        }
        let templates = Templates::new(paths);
        let mut proton_selection = None;
        for row in templates.list()? {
            let template = match templates.resolve(&row.name) {
                Ok(template) => template,
                Err(err) => {
                    result.errors.push(OverviewError {
                        name: format!("template {}", row.name),
                        message: format!("{:#}", anyhow::Error::from(err)),
                    });
                    continue;
                }
            };
            for kind in kinds {
                let asked = match kind {
                    Kind::Runner => template.runner.as_deref(),
                    Kind::Dxvk => template.dxvk.as_deref(),
                    Kind::Vkd3d => template.vkd3d.as_deref(),
                };
                if let Some(asked) = asked {
                    let selected =
                        if *kind == Kind::Runner && asked == crate::templates::NEWEST_PROTON {
                            if proton_selection.is_none() {
                                proton_selection =
                                    template.runner_name(&runners, &global.defaults.runner)?;
                            }
                            proton_selection
                                .as_deref()
                                .expect("proton selects a runner")
                        } else {
                            asked
                        };
                    reference(*kind, &format!("template {}", row.name), asked, selected);
                }
            }
        }
        if kinds.contains(&Kind::Runner) {
            reference(
                Kind::Runner,
                "global default runner",
                &global.defaults.runner,
                &global.defaults.runner,
            );
            // Runtime records can outlive the environment's config.
            for name in runtime_names(paths)? {
                let status = Runtime::new(paths, &name).status()?;
                if (status.state == State::Running || status.lost)
                    && let Some(runner) = status.run.as_ref().and_then(|r| r.runner.as_ref())
                {
                    result
                        .labels
                        .entry((Kind::Runner, runner.name().to_owned()))
                        .or_default()
                        .push(format!("active run {name}"));
                }
            }
            for (name, kind, recorded) in crate::installer::unresolved_builds(paths)? {
                if kind == Kind::Runner {
                    result
                        .labels
                        .entry((kind, recorded))
                        .or_default()
                        .push(format!("installer {name}"));
                }
            }
        }
        for labels in result.labels.values_mut() {
            labels.sort();
            labels.dedup();
        }
        Ok(result)
    }

    fn take(&mut self, kind: Kind, name: &str) -> Uses {
        Uses {
            labels: self
                .labels
                .remove(&(kind, name.to_owned()))
                .unwrap_or_default(),
            errors: self.errors.clone(),
        }
    }
}

fn reference_label(subject: &str, asked: &str, concrete: &str) -> String {
    if asked == concrete {
        subject.to_owned()
    } else {
        format!("{subject} ({asked})")
    }
}

fn runtime_names(paths: &Paths) -> Result<Vec<String>> {
    let root = paths.runtime_dir("");
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).context(|| format!("failed to read {}", root.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("failed to read {}", root.display()))?;
        if let Some(name) = entry.file_name().to_str()
            && crate::name::is_valid(name)
            && entry.path().is_dir()
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The config file behind an error name from the uses index.
pub fn config_path(paths: &Paths, error: &OverviewError) -> PathBuf {
    match error.name.strip_prefix("template ") {
        Some(name) => paths.templates_dir().join(format!("{name}.toml")),
        None => paths.environment_config(&error.name),
    }
}

/// A config this wineo can't read may select the build, so removal can't tell
/// the build is unused.
pub fn refuse_unreadable(name: &str, errors: &[OverviewError]) -> Result<()> {
    if errors.is_empty() {
        return Ok(());
    }
    Err(Error::UnreadableConfigs {
        name: name.to_owned(),
        errors: errors.iter().map(|error| error.message.clone()).collect(),
    })
}

fn in_use(name: &str, labels: &[String]) -> Error {
    let users = match labels {
        [] => "another operation".to_owned(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        many => format!(
            "{}, and {}",
            many[..many.len() - 1].join(", "),
            many.last().unwrap()
        ),
    };
    Error::BuildInUse {
        name: name.to_owned(),
        users,
    }
}

pub(crate) fn managed(dir: &Path, name: &str) -> Result<()> {
    let metadata =
        fs::symlink_metadata(dir).context(|| format!("failed to inspect {}", dir.display()))?;
    if !metadata.file_type().is_dir() {
        return Err(Error::UnmanagedBuild(name.to_owned()));
    }
    Ok(())
}

/// Logical file bytes, without following symlinks outside the build.
pub(crate) fn directory_size(dir: &Path) -> Result<u64> {
    let metadata =
        fs::symlink_metadata(dir).context(|| format!("failed to inspect {}", dir.display()))?;
    if !metadata.is_dir() {
        return Ok(metadata.len());
    }
    let mut bytes = 0u64;
    for entry in fs::read_dir(dir).context(|| format!("failed to read {}", dir.display()))? {
        let path = entry
            .context(|| format!("failed to read {}", dir.display()))?
            .path();
        bytes = bytes.saturating_add(directory_size(&path)?);
    }
    Ok(bytes)
}

pub(crate) fn remove(paths: &Paths, build: &Lock, directory: &Path) -> Result<Removed> {
    remove_after_check(paths, build, directory, || {})
}

fn remove_after_check(
    paths: &Paths,
    build: &Lock,
    directory: &Path,
    after_check: impl FnOnce(),
) -> Result<Removed> {
    let root = directory.parent().expect("a build has a parent");
    let first = uses(paths, build)?;
    if !first.labels.is_empty() {
        return Err(in_use(build.name(), &first.labels));
    }
    refuse_unreadable(build.name(), &first.errors)?;
    after_check();
    let Some(_guard) = build.try_exclusive()? else {
        return Err(in_use(build.name(), &uses(paths, build)?.labels));
    };
    let _install = InstallLock::acquire(directory)
        .map_err(|err| Error::Unsettled(format!("{}: {err:#}", build.name())))?;
    let current = uses(paths, build)?;
    if !current.labels.is_empty() {
        return Err(in_use(build.name(), &current.labels));
    }
    refuse_unreadable(build.name(), &current.errors)?;
    managed(directory, build.name())?;
    let bytes = directory_size(directory)?;
    let staged = move_out(directory)?;
    fs::remove_dir_all(&staged).context(|| format!("failed to delete {}", staged.display()))?;
    sync(root)?;
    Ok(Removed { bytes })
}

pub(crate) fn sweep_best_effort(paths: &Paths, kind: Kind, root: &Path) {
    let _ = sweep(paths, kind, root);
}

/// Deletes abandoned removal staging directories, skipping builds with held locks.
pub fn sweep(paths: &Paths, kind: Kind, root: &Path) -> Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).context(|| format!("failed to read {}", root.display())),
    };
    let mut failure = None;
    for entry in entries {
        let result = (|| {
            let entry = entry.context(|| format!("failed to read {}", root.display()))?;
            let filename = entry.file_name();
            let Some((name, suffix)) = filename
                .to_str()
                .and_then(|name| name.strip_prefix(".wineo-remove-"))
                .and_then(|name| name.rsplit_once('-'))
            else {
                return Ok(());
            };
            if !crate::name::is_valid_runner(name)
                || suffix.len() != 6
                || !suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
            {
                return Ok(());
            }
            let staged = entry.path();
            let Some(_guard) = Lock::new(paths, kind, name).try_exclusive()? else {
                return Ok(());
            };
            let metadata = match fs::symlink_metadata(&staged) {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(err) => {
                    return Err(err).context(|| format!("failed to inspect {}", staged.display()));
                }
            };
            if metadata.file_type().is_dir() {
                fs::remove_dir_all(&staged)
                    .context(|| format!("failed to delete {}", staged.display()))?;
                sync(root)?;
            }
            Ok(())
        })();
        if let Err(err) = result {
            failure.get_or_insert(err);
        }
    }
    failure.map_or(Ok(()), Err)
}

fn sync(dir: &Path) -> Result<()> {
    files::sync_dir(dir).context(|| format!("failed to sync {}", dir.display()))
}

fn move_out(directory: &Path) -> Result<PathBuf> {
    let parent = directory.parent().expect("a build has a parent");
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .expect("a managed build has a valid name");
    let staging = tempfile::Builder::new()
        .prefix(&format!(".wineo-remove-{name}-"))
        .tempdir_in(parent)
        .context(|| format!("failed to stage removal in {}", parent.display()))?;
    let destination = staging.path().join("build");
    renameat_with(CWD, directory, CWD, &destination, RenameFlags::NOREPLACE)
        .map_err(io::Error::from)
        .context(|| format!("failed to move {}", directory.display()))?;
    // Keep the hidden directory after rename, including if syncing or deletion fails.
    let staged = staging.keep();
    sync(parent)?;
    sync(&staged)?;
    Ok(staged)
}

#[cfg(test)]
mod tests;
