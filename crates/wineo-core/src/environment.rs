use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Value, value};

use crate::build_lock;
use crate::cancel::Token;
use crate::config::{self, Arch, EnvironmentConfig, GlobalConfig, WineConfig};
use crate::edit;
use crate::error::{Error, IoContext, Result};
use crate::files::{self, write_atomic, write_atomic_synced};
use crate::installer::Output;
use crate::name;
use crate::operation::{LaunchReservation, Locks, Operation};
use crate::paths::Paths;
use crate::prefix::{self, BuildPlan, PrefixEvent};
use crate::runner::Runner;
use crate::runners::Runners;
use crate::runtime::{Runtime, State};
use crate::templates::Template;

#[derive(Debug)]
pub struct Environment {
    pub name: String,
    pub path: PathBuf,
    pub config: EnvironmentConfig,
}

/// The config file identity used to distinguish a replaced environment with
/// the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnvironmentIdentity {
    device: u64,
    inode: u64,
}

impl EnvironmentIdentity {
    pub fn read(paths: &Paths, name: &str) -> Option<Self> {
        let metadata = fs::metadata(paths.environment_config(name)).ok()?;
        Some(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct HeroicIdSnapshot {
    text: String,
    device: u64,
    inode: u64,
}

/// What `Environment::record_gog_id` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GogIdRecord {
    Written,
    /// The config already had it, so nothing was written.
    AlreadySet,
    /// The config isn't the one the game was matched to any more, so nothing
    /// was written.
    Changed,
}

/// Options for creating an environment. Anything unset falls back to the global defaults.
#[derive(Debug, Default)]
pub struct NewEnvironment {
    pub name: String,
    pub dir: Option<String>,
    pub arch: Option<Arch>,
    /// What `--template` named, already merged with the explicit flags.
    pub template: Option<Template>,
    /// The GOG game it runs, recorded in the same write as the rest.
    pub gog_id: Option<String>,
}

impl NewEnvironment {
    /// Where the prefix will be. Needed before creation, so the prefix can be
    /// locked against another command claiming it at the same time.
    pub fn prefix_dir(&self, paths: &Paths, global: &GlobalConfig) -> Result<PathBuf> {
        match &self.dir {
            Some(dir) => paths.absolute(dir),
            None => Ok(global.prefix_root(paths)?.join(&self.name)),
        }
    }
}

#[derive(Debug)]
pub struct NativeCreatePlan {
    pub name: String,
    pub exe: Option<String>,
    pub resolved_exe: Option<PathBuf>,
}

impl Environment {
    pub fn load_for_heroic_match(paths: &Paths, name: &str) -> Result<(Self, HeroicIdSnapshot)> {
        check_name(name)?;
        crate::trash::check_pending(paths, name, &[])
            .map_err(|err| Error::Unsettled(err.to_string()))?;
        crate::snapshot::check_recovery_marker(paths, name)?;
        let path = paths.environment_config(name);
        let mut file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(Error::UnknownEnvironment(name.to_owned()));
            }
            Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
        };
        let mut text = String::new();
        file.read_to_string(&mut text)
            .context(|| format!("failed to read {}", path.display()))?;
        let metadata = file
            .metadata()
            .context(|| format!("failed to inspect {}", path.display()))?;
        let config = EnvironmentConfig::parse(&path, &text)?;
        let snapshot = HeroicIdSnapshot {
            text,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        Ok((
            Self {
                name: name.to_owned(),
                path,
                config,
            },
            snapshot,
        ))
    }

    pub fn load(paths: &Paths, name: &str) -> Result<Self> {
        check_name(name)?;
        crate::trash::check_pending(paths, name, &[])
            .map_err(|err| Error::Unsettled(err.to_string()))?;
        Self::read(paths, name)
    }

    /// `load` against a trash scan shared by a batch of loads.
    pub fn load_with_pending(
        paths: &Paths,
        name: &str,
        pending: &crate::trash::PendingScan,
    ) -> Result<Self> {
        check_name(name)?;
        pending
            .check(paths, name, &[])
            .map_err(|err| Error::Unsettled(err.to_string()))?;
        Self::read(paths, name)
    }

    fn read(paths: &Paths, name: &str) -> Result<Self> {
        crate::snapshot::check_recovery_marker(paths, name)?;
        let path = paths.environment_config(name);
        #[cfg(test)]
        crate::read_log::note(&path);
        let Some(text) = files::read_to_string_optional(&path)
            .context(|| format!("failed to read {}", path.display()))?
        else {
            return Err(Error::UnknownEnvironment(name.to_owned()));
        };
        let config = EnvironmentConfig::parse(&path, &text)?;
        Ok(Self {
            name: name.to_owned(),
            path,
            config,
        })
    }

    /// Checks that an environment exists without parsing its config, for
    /// commands that shouldn't be blocked by a broken one.
    pub fn ensure_exists(paths: &Paths, name: &str) -> Result<()> {
        check_name(name)?;
        crate::trash::check_pending(paths, name, &[])
            .map_err(|err| Error::Unsettled(err.to_string()))?;
        if paths.environment_config(name).is_file() {
            crate::snapshot::check_recovery_marker(paths, name)?;
            Ok(())
        } else {
            Err(Error::UnknownEnvironment(name.to_owned()))
        }
    }

    /// Creates an environment while reporting prefix creation progress.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        paths: &Paths,
        global: &GlobalConfig,
        runner: &Runner,
        new: &NewEnvironment,
        token: &Token,
        output: Output<'_>,
        mut events: impl FnMut(PrefixEvent),
    ) -> Result<Self> {
        check_new_name(paths, &new.name)?;
        let path = paths.environment_config(&new.name);
        let _runner_guard = build_lock::runner(paths, runner)?;
        let dir = new.prefix_dir(paths, global)?;
        let prefix_value = paths.store_prefix(&dir)?;
        check_nesting(paths, &new.name, &dir)?;

        // Proton keeps its prefix in `pfx/` under the directory it is given.
        let wine_prefix = runner.wine_prefix(&dir);
        let arch = if prefix::is_prefix(&wine_prefix) {
            let arch = adopted_arch(
                &wine_prefix,
                new.arch.unwrap_or(global.defaults.arch),
                new.arch,
            )?;
            // Wine outside wineo could still build menu entries in an adopted
            // prefix otherwise. It might be in use, so this doesn't wait_idle.
            let runtime = paths.runtime_dir(&new.name);
            fs::create_dir_all(&runtime)
                .context(|| format!("failed to create {}", runtime.display()))?;
            prefix::disable_menu_builder(runner, &dir, &runtime.join("create.log"), token, output)?;
            arch
        } else {
            let arch = new.arch.unwrap_or(global.defaults.arch);
            build_new_prefix(
                paths,
                &new.name,
                runner,
                &dir,
                arch,
                token,
                output,
                &mut events,
            )?;
            arch
        };

        // A finished prefix is kept even if this write fails, so a retry adopts it.
        let mut doc = config::new_document();
        doc["prefix"] = value(prefix_value);
        doc["runner"] = value(runner.selection());
        doc["arch"] = value(arch.as_str());
        if let Some(template) = &new.template {
            template.apply(&mut doc);
        }
        if let Some(gog_id) = &new.gog_id {
            doc["gog_id"] = value(gog_id.as_str());
        }
        let _guard = Locks::new(paths).config(&path)?;
        let _build_guards = build_lock::config(
            paths,
            global,
            Some(runner.selection()),
            new.template
                .as_ref()
                .and_then(|template| template.dxvk.as_deref()),
            new.template
                .as_ref()
                .and_then(|template| template.vkd3d.as_deref()),
        )?;
        write_new(&path, &doc.to_string())?;
        Self::load(paths, &new.name)
    }

    /// The Wine settings, or the no-prefix error for a native or proxied
    /// environment.
    pub fn wine(&self) -> Result<&WineConfig> {
        self.config.require_wine(&self.name)
    }

    /// Checks native creation choices without taking locks or writing config.
    pub fn plan_native(
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        exe: Option<&str>,
    ) -> Result<NativeCreatePlan> {
        check_new_name(paths, name)?;
        let resolved_exe = exe
            .map(|exe| {
                crate::launch::check_native_exe(paths, global, name, &Default::default(), exe)
            })
            .transpose()?;
        Ok(NativeCreatePlan {
            name: name.to_owned(),
            exe: exe.map(str::to_owned),
            resolved_exe,
        })
    }

    /// Writes a native environment's config. With an `exe`, which is checked
    /// the way a launch resolves it, it gets one default target named after
    /// the environment, with a desktop entry when `desktop` is set.
    pub fn create_native(
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        exe: Option<&str>,
        desktop: bool,
    ) -> Result<Self> {
        Self::create_native_titled(paths, global, name, exe, None, desktop)
    }

    /// Like `create_native`, with `title` naming the default target's desktop
    /// entry instead of the environment.
    pub fn create_native_titled(
        paths: &Paths,
        global: &GlobalConfig,
        name: &str,
        exe: Option<&str>,
        title: Option<&str>,
        desktop: bool,
    ) -> Result<Self> {
        Self::plan_native(paths, global, name, exe)?;
        let path = paths.environment_config(name);
        let mut doc = config::new_document();
        doc["kind"] = value("native");
        if let Some(exe) = exe {
            let mut table = toml_edit::Table::new();
            table["exe"] = value(paths.store_value(exe)?);
            table["default"] = value(true);
            if let Some(title) = title {
                table["name"] = value(title);
            }
            if desktop {
                table["desktop"] = value(true);
            }
            let mut targets = toml_edit::Table::new();
            targets.set_implicit(true);
            targets.insert(name, toml_edit::Item::Table(table));
            doc["targets"] = toml_edit::Item::Table(targets);
        }
        let _guard = Locks::new(paths).config(&path)?;
        write_new(&path, &doc.to_string())?;
        Self::load(paths, name)
    }

    pub fn prefix(&self, paths: &Paths) -> Result<PathBuf> {
        let prefix = &self.wine()?.prefix;
        // Outside a portable folder a relative prefix would land in whatever
        // directory wineo happened to run from.
        if paths.portable_root().is_none() && !paths.expand(prefix).is_absolute() {
            return Err(Error::RelativePrefix(prefix.clone()));
        }
        paths.config_path(prefix)
    }

    /// Waits for wine to exit in the prefix before something copies, replaces,
    /// or deletes it. wineserver lingers a few seconds after the last program
    /// exits and keeps writing the registry until it does. A supervised run,
    /// or a runner or prefix that can't be resolved, is left to the caller's
    /// own checks to report. `on_wait` runs if wine is still up after a moment.
    pub fn wait_idle(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
        on_wait: impl FnOnce(),
    ) -> Result<()> {
        self.wait_idle_with(paths, global, |runner, prefix| {
            runner.wait_idle_noting(prefix, on_wait)
        })
    }

    pub(crate) fn wait_idle_for_remove(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
        token: &Token,
        on_wait: impl FnOnce(),
    ) -> Result<()> {
        self.wait_idle_with(paths, global, |runner, prefix| {
            match runner.wait_idle_for_copy_noting(prefix, token, on_wait) {
                Err(Error::Canceled(_)) => Err(Error::Canceled("removing environment")),
                // wait_idle_noting never failed removal on wineserver's exit status.
                Err(Error::Unsettled(_)) => Ok(()),
                other => other,
            }
        })
    }

    fn wait_idle_with(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
        wait: impl FnOnce(&Runner, &Path) -> Result<()>,
    ) -> Result<()> {
        let Some(wine) = self.config.wine() else {
            return Ok(());
        };
        let quiet = Runtime::new(paths, &self.name)
            .status()
            .is_ok_and(|status| status.state != State::Running && !status.lost);
        if !quiet {
            return Ok(());
        }
        let Ok((runner, _runner_guard)) = Runners::new(paths, global)
            .and_then(|runners| runners.resolve_locked(paths, &wine.runner))
        else {
            return Ok(());
        };
        let Ok(prefix) = self.prefix(paths) else {
            return Ok(());
        };
        wait(&runner, &prefix)
    }

    /// Builds a missing prefix with cancellation and progress events.
    #[allow(clippy::too_many_arguments)]
    pub fn build_prefix(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
        runner: &Runner,
        empty_confirmed: bool,
        token: &Token,
        output: Output<'_>,
        events: impl FnMut(PrefixEvent),
    ) -> Result<bool> {
        self.build_prefix_reserved(
            paths,
            global,
            runner,
            empty_confirmed,
            token,
            output,
            None,
            events,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_prefix_reserved(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
        runner: &Runner,
        empty_confirmed: bool,
        token: &Token,
        output: Output<'_>,
        reservation: Option<&LaunchReservation>,
        mut events: impl FnMut(PrefixEvent),
    ) -> Result<bool> {
        let dir = self.prefix(paths)?;
        let root = global.prefix_root(paths)?;
        let _runner_guard = build_lock::runner(paths, runner)?;
        let plan = || prefix::build_plan(paths, &root, runner, &dir, empty_confirmed);
        if plan()? == BuildPlan::Ready {
            return Ok(false);
        }
        let _guard = match reservation {
            Some(reservation) => reservation.acquire(Operation::Create, &[&dir])?,
            None => Locks::new(paths).acquire(Operation::Create, &self.name, &[&dir])?,
        };
        // Another command could have removed the environment or built the
        // prefix before the lock was taken.
        Self::ensure_exists(paths, &self.name)?;
        if plan()? == BuildPlan::Ready {
            return Ok(false);
        }
        build_new_prefix(
            paths,
            &self.name,
            runner,
            &dir,
            self.wine()?.arch,
            token,
            output,
            &mut events,
        )?;
        Ok(true)
    }

    /// Writes `heroic_id` onto an environment's config, keeping the rest of
    /// the file unchanged. For backfilling environments matched to a Heroic
    /// game that predate the id being recorded.
    pub fn record_heroic_id(
        paths: &Paths,
        name: &str,
        heroic_id: &str,
        expected: &HeroicIdSnapshot,
    ) -> Result<()> {
        check_name(name)?;
        crate::snapshot::check_recovery_marker(paths, name)?;
        let path = paths.environment_config(name);
        let _guard = Locks::new(paths).config(&path)?;
        let mut file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(Error::UnknownEnvironment(name.to_owned()));
            }
            Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
        };
        let mut text = String::new();
        file.read_to_string(&mut text)
            .context(|| format!("failed to read {}", path.display()))?;
        let metadata = file
            .metadata()
            .context(|| format!("failed to inspect {}", path.display()))?;
        let config = EnvironmentConfig::parse(&path, &text)?;
        if config.heroic_id.as_deref() == Some(heroic_id) {
            return Ok(());
        }
        if config.heroic_id.is_some() {
            return Err(Error::ConfigInvalid {
                path,
                message: "Heroic ID changed; retry the import".to_owned(),
            });
        }
        if text != expected.text
            || metadata.dev() != expected.device
            || metadata.ino() != expected.inode
        {
            return Err(Error::ConfigInvalid {
                path,
                message: "environment changed during Heroic matching; retry the import".to_owned(),
            });
        }
        let mut doc: DocumentMut = text.parse().map_err(|source| Error::ConfigParse {
            path: path.clone(),
            source: Box::new(source),
        })?;
        edit::store_value(
            doc.as_table_mut(),
            "heroic_id",
            Value::from(heroic_id.to_owned()),
        );
        config::stamp_schema(&mut doc);
        let updated = doc.to_string();
        EnvironmentConfig::parse(&path, &updated)?;
        if updated != text {
            write_atomic(&path, &updated)?;
        }
        Ok(())
    }

    /// Writes `gog_id` onto an environment's config, keeping the rest of the
    /// file unchanged, and syncs it so it's on disk before the game folder's
    /// state names the environment. Only migration calls this, holding the
    /// environment's operation lock. Not every edit takes that lock, so
    /// `still_matches` is asked again under the config lock whether the
    /// config is still the one the game was matched to, and nothing is
    /// written when it isn't.
    pub fn record_gog_id(
        paths: &Paths,
        name: &str,
        gog_id: &str,
        still_matches: &dyn Fn(&Environment) -> bool,
    ) -> Result<GogIdRecord> {
        check_name(name)?;
        crate::snapshot::check_recovery_marker(paths, name)?;
        let path = paths.environment_config(name);
        let _guard = Locks::new(paths).config(&path)?;
        let Some(text) = files::read_to_string_optional(&path)
            .context(|| format!("failed to read {}", path.display()))?
        else {
            return Err(Error::UnknownEnvironment(name.to_owned()));
        };
        let environment = Self {
            name: name.to_owned(),
            path: path.clone(),
            config: EnvironmentConfig::parse(&path, &text)?,
        };
        if !still_matches(&environment) {
            return Ok(GogIdRecord::Changed);
        }
        match environment.config.gog_id.as_deref() {
            Some(current) if current == gog_id => return Ok(GogIdRecord::AlreadySet),
            Some(_) => {
                return Err(Error::ConfigInvalid {
                    path,
                    message: "gog_id changed during the migration; retry it".to_owned(),
                });
            }
            None => {}
        }
        let mut doc: DocumentMut = text.parse().map_err(|source| Error::ConfigParse {
            path: path.clone(),
            source: Box::new(source),
        })?;
        edit::store_value(doc.as_table_mut(), "gog_id", Value::from(gog_id.to_owned()));
        config::stamp_schema(&mut doc);
        let updated = doc.to_string();
        EnvironmentConfig::parse(&path, &updated)?;
        write_atomic_synced(&path, &updated)?;
        Ok(GogIdRecord::Written)
    }
}

/// Names of all environments, sorted. Configs aren't parsed, so a broken one is still listed.
pub fn names(paths: &Paths) -> Result<Vec<String>> {
    let dir = paths.environments_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).context(|| format!("failed to read {}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let path = entry
            .context(|| format!("failed to read {}", dir.display()))?
            .path();
        if path.extension() != Some(OsStr::new("toml")) || !path.is_file() {
            continue;
        }
        if let Some(name) = path.file_stem().and_then(OsStr::to_str)
            && name::is_valid(name)
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Distinct tags from readable environments, sorted for completion.
pub fn tags(paths: &Paths) -> Result<Vec<String>> {
    let mut tags = BTreeSet::new();
    for name in names(paths)? {
        if let Ok(environment) = Environment::load(paths, &name) {
            tags.extend(environment.config.tags);
        }
    }
    Ok(tags.into_iter().collect())
}

/// What else refers to a prefix. An environment whose config can't be read
/// could refer to it too, so those are reported instead of assumed harmless.
#[derive(Debug, Default, PartialEq)]
pub struct Sharing {
    pub names: Vec<String>,
    pub unreadable: Vec<String>,
}

impl Sharing {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.unreadable.is_empty()
    }
}

/// Environments other than `name` that use `prefix`. Paths are compared
/// resolved, so two spellings of one directory are recognized as the same.
pub fn sharing_prefix(paths: &Paths, name: &str, prefix: &Path) -> Result<Sharing> {
    let wanted = crate::paths::resolve(prefix);
    let mut sharing = Sharing::default();
    for other in names(paths)? {
        if other == name {
            continue;
        }
        let loaded = Environment::load(paths, &other);
        if loaded.as_ref().is_ok_and(|env| env.config.wine().is_none()) {
            continue;
        }
        match loaded.and_then(|env| env.prefix(paths)) {
            Ok(other_prefix) if crate::paths::resolve(&other_prefix) == wanted => {
                sharing.names.push(other);
            }
            Ok(_) => {}
            Err(_) => sharing.unreadable.push(other),
        }
    }
    Ok(sharing)
}

/// Refuses a copy when another environment's config cannot be inspected.
pub fn checked_sharing(paths: &Paths, name: &str, prefix: &Path) -> Result<Sharing> {
    let sharing = sharing_prefix(paths, name, prefix)?;
    if !sharing.unreadable.is_empty() {
        return Err(Error::UnreadableEnvironments(sharing.unreadable));
    }
    Ok(sharing)
}

/// Refuses `prefix` for `name` when another environment's prefix contains it
/// or sits inside it, since moving or replacing the outer one would take the
/// inner one along. Equal prefixes are sharing, which `sharing_prefix`
/// reports. A config that can't be read could be either, so it refuses too.
pub fn check_nesting(paths: &Paths, name: &str, prefix: &Path) -> Result<()> {
    check_overlap(paths, name, prefix, true)
}

/// `check_nesting` for a directory a copy is about to fill, which no other
/// environment may name as its prefix either, even one missing on disk.
pub fn check_destination(paths: &Paths, name: &str, prefix: &Path) -> Result<()> {
    check_overlap(paths, name, prefix, false)
}

fn check_overlap(paths: &Paths, name: &str, prefix: &Path, allow_equal: bool) -> Result<()> {
    let wanted = crate::paths::resolve(prefix);
    let mut unreadable = Vec::new();
    for other in names(paths)? {
        if other == name {
            continue;
        }
        let loaded = Environment::load(paths, &other);
        if loaded.as_ref().is_ok_and(|env| env.config.wine().is_none()) {
            continue;
        }
        match loaded.and_then(|env| env.prefix(paths)) {
            Ok(other_prefix) => {
                let other_prefix = crate::paths::resolve(&other_prefix);
                if other_prefix == wanted {
                    if allow_equal {
                        continue;
                    }
                    return Err(Error::PrefixTaken {
                        path: wanted,
                        owner: other,
                    });
                }
                if wanted.starts_with(&other_prefix) {
                    return Err(Error::NestedPrefix {
                        inner: name.to_owned(),
                        outer: other,
                    });
                }
                if other_prefix.starts_with(&wanted) {
                    return Err(Error::NestedPrefix {
                        inner: other,
                        outer: name.to_owned(),
                    });
                }
            }
            Err(_) => unreadable.push(other),
        }
    }
    if !unreadable.is_empty() {
        return Err(Error::UnreadableEnvironments(unreadable));
    }
    Ok(())
}

pub(crate) fn check_new_name(paths: &Paths, name: &str) -> Result<()> {
    check_name(name)?;
    if paths.environment_config(name).exists() {
        return Err(Error::EnvironmentExists(name.to_owned()));
    }
    Ok(())
}

fn check_name(name: &str) -> Result<()> {
    if name::is_valid(name) {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_owned()))
    }
}

/// An adopted prefix keeps the arch it was created with, so an explicit
/// `--arch` that disagrees is an error rather than silently ignored.
pub(crate) fn adopted_arch(dir: &Path, fallback: Arch, requested: Option<Arch>) -> Result<Arch> {
    match (prefix::detect_arch(dir)?, requested) {
        (Some(actual), Some(requested)) if actual != requested => Err(Error::ArchMismatch {
            path: dir.to_owned(),
            actual,
            requested,
        }),
        (Some(actual), _) => Ok(actual),
        (None, _) => Ok(fallback),
    }
}

/// Initializes a prefix in `dir`, logging to the environment's create.log. A
/// failed build removes `dir` only if this made it. A canceled build also
/// empties a directory that already existed empty, but leaves the directory.
#[allow(clippy::too_many_arguments)]
fn build_new_prefix(
    paths: &Paths,
    name: &str,
    runner: &Runner,
    dir: &Path,
    arch: Arch,
    token: &Token,
    output: Output<'_>,
    events: &mut impl FnMut(PrefixEvent),
) -> Result<()> {
    let created = prefix::prepare_dir(dir)?;
    let runtime = paths.runtime_dir(name);
    let result = fs::create_dir_all(&runtime)
        .context(|| format!("failed to create {}", runtime.display()))
        .and_then(|()| {
            prefix::create(
                runner,
                dir,
                arch,
                &runtime.join("create.log"),
                token,
                output,
                events,
            )
        });
    if let Err(err) = result {
        // Best effort, the error that got us here matters more.
        if created {
            let _ = fs::remove_dir_all(dir);
        } else if matches!(err, Error::Canceled(_)) {
            // The user supplied the directory, so keep it but drop what wineboot wrote.
            let _ = empty_dir(dir);
        }
        return Err(err);
    }
    Ok(())
}

/// Removes everything inside `dir`. Symlinks are unlinked, never followed.
/// Keeps going after a failed entry and returns the first error.
fn empty_dir(dir: &Path) -> io::Result<()> {
    let mut first = None;
    for entry in fs::read_dir(dir)? {
        let removed = entry.and_then(|entry| {
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())
            } else {
                fs::remove_file(entry.path())
            }
        });
        if let Err(err) = removed {
            first.get_or_insert(err);
        }
    }
    first.map_or(Ok(()), Err)
}

fn write_new(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests;
