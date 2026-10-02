//! References and removal shared by runners and downloaded components.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use rustix::fs::{CWD, RenameFlags, renameat_with};

use crate::build_lock::{Kind, Lock};
use crate::components::{Builds, Component};
use crate::config::GlobalConfig;
use crate::download::InstallLock;
use crate::environment::{self, Environment};
use crate::error::{Error, IoContext, Result};
use crate::paths::Paths;
use crate::report::OverviewError;
use crate::runners::Runners;
use crate::runtime::{Runtime, State};
use crate::templates::Templates;

/// Unreadable configs are skipped and reported, like an environment overview.
#[derive(Debug, Default)]
pub struct Uses {
    pub labels: Vec<String>,
    pub errors: Vec<OverviewError>,
}

#[derive(Debug)]
pub struct Removed {
    pub bytes: u64,
    pub errors: Vec<OverviewError>,
}

pub fn uses(paths: &Paths, build: &Lock) -> Result<Uses> {
    let global = GlobalConfig::load(paths)?;
    let runners = Runners::new(paths, &global)?;
    let component = match build.kind() {
        Kind::Runner => None,
        Kind::Dxvk => Some(Builds::new(paths, &global, Component::Dxvk)?),
        Kind::Vkd3d => Some(Builds::new(paths, &global, Component::Vkd3d)?),
    };
    let matches = |asked: &str| {
        // An explicit reference counts even when its build is currently unusable.
        asked == build.name()
            || match &component {
                None => runners
                    .resolve(asked)
                    .is_ok_and(|r| r.name() == build.name()),
                Some(builds) => builds
                    .resolve(asked)
                    .is_ok_and(|b| b.name() == build.name()),
            }
    };
    let mut result = Uses::default();
    for name in environment::names(paths)? {
        match Environment::load(paths, &name) {
            Ok(environment) => {
                if let Some(wine) = environment.config.wine() {
                    let asked = match build.kind() {
                        Kind::Runner => Some(wine.runner.as_str()),
                        Kind::Dxvk => wine.dxvk.as_deref(),
                        Kind::Vkd3d => wine.vkd3d.as_deref(),
                    };
                    if let Some(asked) = asked.filter(|asked| matches(asked)) {
                        result
                            .labels
                            .push(reference_label(&name, asked, build.name()));
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
        let selected = match build.kind() {
            Kind::Runner => template.runner_name(&runners, &global.defaults.runner)?,
            Kind::Dxvk => template.dxvk.clone(),
            Kind::Vkd3d => template.vkd3d.clone(),
        };
        if selected.as_deref().is_some_and(matches) {
            let asked = match build.kind() {
                Kind::Runner => template.runner.as_deref(),
                Kind::Dxvk => template.dxvk.as_deref(),
                Kind::Vkd3d => template.vkd3d.as_deref(),
            }
            .expect("a selected template build has a selection");
            result.labels.push(reference_label(
                &format!("template {}", row.name),
                asked,
                build.name(),
            ));
        }
    }
    if build.kind() == Kind::Runner {
        if matches(&global.defaults.runner) {
            result.labels.push(reference_label(
                "global default runner",
                &global.defaults.runner,
                build.name(),
            ));
        }
        // Runtime records can outlive the environment's config.
        for name in runtime_names(paths)? {
            let status = Runtime::new(paths, &name).status()?;
            if (status.state == State::Running || status.lost)
                && status
                    .run
                    .as_ref()
                    .and_then(|r| r.runner.as_ref())
                    .is_some_and(|r| r.name() == build.name())
            {
                result.labels.push(format!("active run {name}"));
            }
        }
        for (name, kind, recorded) in crate::installer::unresolved_builds(paths)? {
            if kind == build.kind() && recorded == build.name() {
                result.labels.push(format!("installer {name}"));
            }
        }
    }
    result.labels.sort();
    result.labels.dedup();
    Ok(result)
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
    managed(directory, build.name())?;
    let bytes = directory_size(directory)?;
    let staged = move_out(directory)?;
    fs::remove_dir_all(&staged).context(|| format!("failed to delete {}", staged.display()))?;
    sync(root)?;
    let mut errors = first.errors;
    for error in current.errors {
        if !errors.iter().any(|existing| existing.name == error.name) {
            errors.push(error);
        }
    }
    Ok(Removed { bytes, errors })
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
    File::open(dir)
        .and_then(|file| file.sync_all())
        .context(|| format!("failed to sync {}", dir.display()))
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
