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
use crate::files::{write_atomic, write_atomic_synced};
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
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(Error::UnknownEnvironment(name.to_owned()));
            }
            Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
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
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(Error::UnknownEnvironment(name.to_owned()));
            }
            Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
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
mod tests {
    use crate::test_script::write_script;
    use std::ffi::OsString;

    use super::*;

    fn test_paths(root: &Path) -> Paths {
        let root = root.to_owned();
        Paths::from_lookup(move |key| match key {
            "HOME" => Some(OsString::from(root.join("home"))),
            _ => None,
        })
        .unwrap()
    }

    fn fake_prefix(dir: &Path, arch: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("system.reg"),
            format!("WINE REGISTRY Version 2\n\n#arch={arch}\n"),
        )
        .unwrap();
    }

    fn new(name: &str, dir: &Path) -> NewEnvironment {
        NewEnvironment {
            name: name.to_owned(),
            dir: Some(dir.to_str().unwrap().to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn adopting_a_prefix_writes_config_with_its_arch() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("home/Apps/Thing");
        fake_prefix(&prefix, "win64");

        let env = Environment::create(
            &paths,
            &GlobalConfig::default(),
            &Runner::fake("wine"),
            &new("thing", &prefix),
            &Token::new(),
            Output::Log(&paths.runtime_dir("thing").join("create.log")),
            |_| {},
        )
        .unwrap();
        let wine = env.config.wine().unwrap();
        assert_eq!(wine.prefix, "~/Apps/Thing");
        assert_eq!(wine.runner, "wine");
        assert_eq!(wine.arch, Arch::Win64);
        assert_eq!(
            fs::read_to_string(paths.environment_config("thing")).unwrap(),
            "schema_version = 1\nprefix = \"~/Apps/Thing\"\nrunner = \"wine\"\narch = \"win64\"\n"
        );
        assert_eq!(env.prefix(&paths).unwrap(), prefix);
        assert!(paths.runtime_dir("thing").join("create.log").exists());

        assert!(matches!(
            Environment::create(
                &paths,
                &GlobalConfig::default(),
                &Runner::fake("wine"),
                &new("thing", &prefix),
                &Token::new(),
                Output::Log(&paths.runtime_dir("thing").join("create.log")),
                |_| {},
            ),
            Err(Error::EnvironmentExists(_))
        ));
    }

    #[test]
    fn adopting_with_a_conflicting_arch_fails() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("old");
        fake_prefix(&prefix, "win32");

        let mut options = new("old", &prefix);
        options.arch = Some(Arch::Win64);
        assert!(matches!(
            Environment::create(
                &paths,
                &GlobalConfig::default(),
                &Runner::fake("wine"),
                &options,
                &Token::new(),
                Output::Log(&paths.runtime_dir("old").join("create.log")),
                |_| {},
            ),
            Err(Error::ArchMismatch { .. })
        ));
        assert!(!paths.environment_config("old").exists());
    }

    #[test]
    fn record_heroic_id_preserves_the_rest_of_the_file() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let path = paths.environment_config("game");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = "# hand written\nprefix = \"~/Games/Game\"\nrunner = \"wine\"\narch = \"win64\"\n# keep this\ngamemode = true\n";
        fs::write(&path, original).unwrap();

        let (_, snapshot) = Environment::load_for_heroic_match(&paths, "game").unwrap();
        Environment::record_heroic_id(&paths, "game", "abc123", &snapshot).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            format!("{original}heroic_id = \"abc123\"\nschema_version = 1\n")
        );
        let config = EnvironmentConfig::parse(&path, &text).unwrap();
        assert_eq!(config.heroic_id.as_deref(), Some("abc123"));

        Environment::record_heroic_id(&paths, "game", "abc123", &snapshot).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("heroic_id").count(), 1, "{text}");
        assert!(text.contains("heroic_id = \"abc123\"\n"), "{text}");
    }

    #[test]
    fn record_heroic_id_refuses_a_concurrent_id_or_config_edit() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let path = paths.environment_config("game");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = "prefix = \"/games/one\"\nrunner = \"wine\"\narch = \"win64\"\n";
        fs::write(&path, original).unwrap();
        let (_, snapshot) = Environment::load_for_heroic_match(&paths, "game").unwrap();

        let changed = original.replace("/games/one", "/games/two");
        fs::write(&path, &changed).unwrap();
        assert!(matches!(
            Environment::record_heroic_id(&paths, "game", "abc123", &snapshot),
            Err(Error::ConfigInvalid { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);

        let changed_id = format!("{original}heroic_id = \"other\"\n");
        fs::write(&path, &changed_id).unwrap();
        assert!(matches!(
            Environment::record_heroic_id(&paths, "game", "abc123", &snapshot),
            Err(Error::ConfigInvalid { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), changed_id);
    }

    #[test]
    fn record_heroic_id_accepts_a_concurrent_backfill_of_the_same_id() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let path = paths.environment_config("game");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "prefix = \"/games/one\"\nrunner = \"wine\"\narch = \"win64\"\n",
        )
        .unwrap();
        let (_, snapshot) = Environment::load_for_heroic_match(&paths, "game").unwrap();
        let concurrent = "# concurrent edit\nprefix = \"/games/two\"\nrunner = \"wine\"\narch = \"win64\"\nheroic_id = \"abc123\"\n";
        fs::write(&path, concurrent).unwrap();
        Environment::record_heroic_id(&paths, "game", "abc123", &snapshot).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), concurrent);
    }

    #[test]
    fn record_heroic_id_refuses_removed_or_replaced_config() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let path = paths.environment_config("game");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = "prefix = \"/games/one\"\nrunner = \"wine\"\narch = \"win64\"\n";
        fs::write(&path, original).unwrap();
        let (_, snapshot) = Environment::load_for_heroic_match(&paths, "game").unwrap();
        let replacement = path.with_extension("replacement");
        fs::write(&replacement, original).unwrap();

        fs::remove_file(&path).unwrap();
        assert!(matches!(
            Environment::record_heroic_id(&paths, "game", "abc123", &snapshot),
            Err(Error::UnknownEnvironment(_))
        ));
        assert!(!path.exists());

        fs::rename(&replacement, &path).unwrap();
        assert!(matches!(
            Environment::record_heroic_id(&paths, "game", "abc123", &snapshot),
            Err(Error::ConfigInvalid { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn loading_a_legacy_config_leaves_it_alone() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let path = paths.environment_config("game");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = "# mine\nprefix = \"~/Games/Game\"\nrunner = \"wine\"\narch = \"win64\"\n";
        fs::write(&path, original).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            Environment::load(&paths, "game")
                .unwrap()
                .config
                .schema_version,
            0
        );
        Environment::load_for_heroic_match(&paths, "game").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn heroic_matching_refuses_an_unsupported_version() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let path = paths.environment_config("game");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original =
            "schema_version = 2\nprefix = \"~/Games/Game\"\nrunner = \"wine\"\narch = \"win64\"\n";
        fs::write(&path, original).unwrap();
        assert!(matches!(
            Environment::load_for_heroic_match(&paths, "game"),
            Err(Error::SchemaVersion { .. })
        ));
        assert!(matches!(
            Environment::load(&paths, "game"),
            Err(Error::SchemaVersion { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn record_heroic_id_on_unknown_environment_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let path = paths.environment_config("nope");
        let snapshot = HeroicIdSnapshot {
            text: String::new(),
            device: 0,
            inode: 0,
        };
        assert!(matches!(
            Environment::record_heroic_id(&paths, "nope", "abc123", &snapshot),
            Err(Error::UnknownEnvironment(_))
        ));
        assert!(!path.exists());
    }

    #[test]
    fn non_empty_non_prefix_dirs_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let dir = root.path().join("stuff");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.txt"), "keep me").unwrap();

        assert!(matches!(
            Environment::create(
                &paths,
                &GlobalConfig::default(),
                &Runner::fake("wine"),
                &new("stuff", &dir),
                &Token::new(),
                Output::Log(&paths.runtime_dir("stuff").join("create.log")),
                |_| {},
            ),
            Err(Error::NotAPrefix(_))
        ));
        assert!(dir.join("notes.txt").exists());
    }

    fn hand_written(paths: &Paths, name: &str, prefix: &Path) -> (Environment, String) {
        let path = paths.environment_config(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = format!(
            "# mine\nprefix = \"{}\"\nrunner = \"wine\"\narch = \"win32\"\n",
            prefix.display()
        );
        fs::write(&path, &text).unwrap();
        (Environment::load(paths, name).unwrap(), text)
    }

    #[test]
    fn a_relative_prefix_is_refused_outside_portable_mode() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let (env, _) = hand_written(&paths, "game", Path::new("[PREFIX-DIR]"));
        assert!(matches!(env.prefix(&paths), Err(Error::RelativePrefix(_))));
        let (env, _) = hand_written(&paths, "home", Path::new("~/Games/Game"));
        assert_eq!(
            env.prefix(&paths).unwrap(),
            root.path().join("home/Games/Game")
        );
    }

    fn native(paths: &Paths, name: &str) -> Environment {
        let path = paths.environment_config(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "kind = \"native\"\n[targets.game]\nexe = \"/usr/bin/dusklight\"\n",
        )
        .unwrap();
        Environment::load(paths, name).unwrap()
    }

    #[test]
    fn a_native_environment_has_no_prefix() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let env = native(&paths, "dusklight");
        let err = env.prefix(&paths).unwrap_err();
        assert!(matches!(err, Error::NoPrefix(_)));
        assert_eq!(
            err.to_string(),
            "dusklight is a native environment, so it has no prefix"
        );
        assert!(matches!(env.wine(), Err(Error::NoPrefix(_))));
        assert!(matches!(
            env.build_prefix(
                &paths,
                &GlobalConfig::default(),
                &Runner::fake("wine"),
                false,
                &Token::new(),
                Output::Log(&paths.runtime_dir("dusklight").join("create.log")),
                |_| panic!("nothing to build"),
            ),
            Err(Error::NoPrefix(_))
        ));
    }

    #[test]
    fn waiting_for_a_native_environment_to_go_idle_returns_at_once() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let env = native(&paths, "dusklight");
        env.wait_idle(&paths, &GlobalConfig::default(), || {
            panic!("no wine to wait for")
        })
        .unwrap();
    }

    #[test]
    fn a_native_environment_shares_no_prefix() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("home/Games/game");
        hand_written(&paths, "game", &prefix);
        native(&paths, "dusklight");
        assert!(sharing_prefix(&paths, "other", &prefix).unwrap().names == ["game"]);
        assert!(sharing_prefix(&paths, "game", &prefix).unwrap().is_empty());
    }

    /// Stands in for wine, which the fake runner doesn't run, by writing the
    /// registry file `prefix::create` checks for.
    fn wine_builds_it(dir: &Path) {
        fs::write(dir.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    }

    #[test]
    fn build_prefix_builds_once_and_never_writes_the_config() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let global = GlobalConfig::default();
        let prefix = global.prefix_root(&paths).unwrap().join("palworld");
        let (env, text) = hand_written(&paths, "palworld", &prefix);
        let config = paths.environment_config("palworld");
        let before = fs::metadata(&config).unwrap().modified().unwrap();

        let mut announced = None;
        let built = env
            .build_prefix(
                &paths,
                &global,
                &Runner::fake("wine"),
                false,
                &Token::new(),
                Output::Log(&paths.runtime_dir("palworld").join("create.log")),
                |event| {
                    if let PrefixEvent::Started { dir } = event {
                        announced = Some(dir.clone());
                        wine_builds_it(&dir);
                    }
                },
            )
            .unwrap();
        assert!(built);
        assert_eq!(announced.as_deref(), Some(prefix.as_path()));
        assert!(prefix::is_prefix(&prefix));
        assert!(paths.runtime_dir("palworld").join("create.log").exists());
        assert_eq!(fs::read_to_string(&config).unwrap(), text);
        assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), before);

        let built = env
            .build_prefix(
                &paths,
                &global,
                &Runner::fake("wine"),
                false,
                &Token::new(),
                Output::Log(&paths.runtime_dir("palworld").join("create.log")),
                |_| panic!("already built"),
            )
            .unwrap();
        assert!(!built);
        assert_eq!(fs::read_to_string(&config).unwrap(), text);
    }

    #[test]
    fn failed_build_removes_a_dir_it_made() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let global = GlobalConfig::default();
        let prefix = global.prefix_root(&paths).unwrap().join("palworld");
        let (env, text) = hand_written(&paths, "palworld", &prefix);

        let err = env
            .build_prefix(
                &paths,
                &global,
                &Runner::fake("wine"),
                false,
                &Token::new(),
                Output::Log(&paths.runtime_dir("palworld").join("create.log")),
                |_| {},
            )
            .unwrap_err();
        assert!(matches!(err, Error::PrefixCreation { .. }), "{err}");
        assert!(!prefix.exists());
        assert_eq!(
            fs::read_to_string(paths.environment_config("palworld")).unwrap(),
            text
        );
    }

    #[test]
    fn failed_build_keeps_an_empty_dir_it_found() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let global = GlobalConfig::default();
        let prefix = global.prefix_root(&paths).unwrap().join("palworld");
        fs::create_dir_all(&prefix).unwrap();
        let (env, _) = hand_written(&paths, "palworld", &prefix);

        let err = env
            .build_prefix(
                &paths,
                &global,
                &Runner::fake("wine"),
                false,
                &Token::new(),
                Output::Log(&paths.runtime_dir("palworld").join("create.log")),
                |_| {},
            )
            .unwrap_err();
        assert!(matches!(err, Error::PrefixCreation { .. }), "{err}");
        assert!(prefix.is_dir());
    }

    #[test]
    fn build_prefix_refuses_before_touching_anything() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let global = GlobalConfig::default();
        let prefix = root.path().join("mnt/game-ssd");
        fs::create_dir_all(&prefix).unwrap();
        let (env, _) = hand_written(&paths, "palworld", &prefix);

        assert!(matches!(
            env.build_prefix(
                &paths,
                &global,
                &Runner::fake("wine"),
                false,
                &Token::new(),
                Output::Log(&paths.runtime_dir("palworld").join("create.log")),
                |_| { panic!("refused") }
            ),
            Err(Error::EmptyPrefix(_))
        ));
        assert!(!paths.runtime_dir("palworld").exists());

        let built = env
            .build_prefix(
                &paths,
                &global,
                &Runner::fake("wine"),
                true,
                &Token::new(),
                Output::Log(&paths.runtime_dir("palworld").join("create.log")),
                |event| {
                    if let PrefixEvent::Started { dir } = event {
                        wine_builds_it(&dir);
                    }
                },
            )
            .unwrap();
        assert!(built);
        assert!(prefix::is_prefix(&prefix));
    }

    #[test]
    fn build_prefix_reports_a_busy_environment() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let global = GlobalConfig::default();
        let prefix = global.prefix_root(&paths).unwrap().join("palworld");
        let (env, _) = hand_written(&paths, "palworld", &prefix);

        let _held = Locks::new(&paths)
            .acquire(Operation::Create, "palworld", &[&prefix])
            .unwrap();
        assert!(matches!(
            env.build_prefix(
                &paths,
                &global,
                &Runner::fake("wine"),
                false,
                &Token::new(),
                Output::Log(&paths.runtime_dir("palworld").join("create.log")),
                |_| { panic!("busy") }
            ),
            Err(Error::Busy { .. })
        ));
        assert!(!prefix.exists());
    }

    #[test]
    fn build_prefix_refuses_a_removed_environment() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let global = GlobalConfig::default();
        let prefix = global.prefix_root(&paths).unwrap().join("palworld");
        let (env, _) = hand_written(&paths, "palworld", &prefix);
        fs::remove_file(paths.environment_config("palworld")).unwrap();

        assert!(matches!(
            env.build_prefix(
                &paths,
                &global,
                &Runner::fake("wine"),
                false,
                &Token::new(),
                Output::Log(&paths.runtime_dir("palworld").join("create.log")),
                |_| { panic!("removed") }
            ),
            Err(Error::UnknownEnvironment(_))
        ));
        assert!(!prefix.exists());
    }

    #[test]
    fn failed_create_removes_the_dir_it_made() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("home/prefixes/fails");

        let err = Environment::create(
            &paths,
            &GlobalConfig::default(),
            &Runner::fake("wine"),
            &new("fails", &prefix),
            &Token::new(),
            Output::Log(&paths.runtime_dir("fails").join("create.log")),
            |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, Error::PrefixCreation { .. }), "{err}");
        assert!(!prefix.exists());
        assert!(!paths.environment_config("fails").exists());
    }

    #[test]
    fn canceling_prefix_creation_from_another_thread_removes_partial_state() {
        use std::thread;
        use std::time::{Duration, Instant};

        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("home/prefixes/canceled");
        let marker = prefix.join("partial");
        let runner_path = root.path().join("fake-wine");
        write_script(
            &runner_path,
            "#!/bin/sh\ncase \"$1\" in\n  wineboot)\n    printf partial > \"$WINEPREFIX/partial\"\n    trap 'exit 0' TERM\n    while :; do sleep 1; done\n    ;;\n  reg|-w|-k) exit 0 ;;\nesac\nexit 0\n",
        );
        let runner = Runner::wine_build("fake".to_owned(), runner_path.clone(), runner_path);
        let token = Token::new();
        let worker_token = token.clone();
        let new = new("canceled", &prefix);
        let worker_paths = paths.clone();
        let worker_runner = runner.clone();
        let worker = thread::spawn(move || {
            Environment::create(
                &worker_paths,
                &GlobalConfig::default(),
                &worker_runner,
                &new,
                &worker_token,
                Output::Log(&worker_paths.runtime_dir("canceled").join("create.log")),
                |_| {},
            )
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "fake wineboot did not start");
        token.cancel();

        let error = worker.join().unwrap().unwrap_err();
        assert!(
            matches!(error, Error::Canceled("prefix creation")),
            "{error}"
        );
        assert!(!prefix.exists());
        assert!(!paths.environment_config("canceled").exists());
    }

    #[test]
    fn empty_dir_keeps_going_after_a_failed_entry() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let stuck = root.path().join("stuck");
        fs::create_dir(&stuck).unwrap();
        fs::write(stuck.join("inner"), "x").unwrap();
        fs::set_permissions(&stuck, fs::Permissions::from_mode(0o500)).unwrap();
        for name in ["a", "b", "c"] {
            fs::write(root.path().join(name), "x").unwrap();
        }

        let result = empty_dir(root.path());
        fs::set_permissions(&stuck, fs::Permissions::from_mode(0o700)).unwrap();
        // Running as root removes the stuck directory too.
        if result.is_ok() {
            return;
        }
        let left: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["stuck"]);
    }

    #[test]
    fn canceling_into_an_existing_empty_dir_empties_it_and_keeps_it() {
        use std::thread;
        use std::time::{Duration, Instant};

        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("home/prefixes/existing");
        fs::create_dir_all(&prefix).unwrap();
        let outside = root.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep"), "keep").unwrap();
        let marker = prefix.join("partial");
        let runner_path = root.path().join("fake-wine");
        // Leaves a subdirectory and a symlink out of the prefix behind.
        write_script(
            &runner_path,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  wineboot)\n    mkdir -p \"$WINEPREFIX/sub/deeper\"\n    ln -s '{}' \"$WINEPREFIX/link\"\n    printf partial > \"$WINEPREFIX/partial\"\n    trap 'exit 0' TERM\n    while :; do sleep 1; done\n    ;;\n  reg|-w|-k) exit 0 ;;\nesac\nexit 0\n",
                outside.display()
            ),
        );
        let runner = Runner::wine_build("fake".to_owned(), runner_path.clone(), runner_path);
        let token = Token::new();
        let worker_token = token.clone();
        let new = new("existing", &prefix);
        let worker_paths = paths.clone();
        let worker_runner = runner.clone();
        let worker = thread::spawn(move || {
            Environment::create(
                &worker_paths,
                &GlobalConfig::default(),
                &worker_runner,
                &new,
                &worker_token,
                Output::Log(&worker_paths.runtime_dir("existing").join("create.log")),
                |_| {},
            )
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "fake wineboot did not start");
        token.cancel();

        let error = worker.join().unwrap().unwrap_err();
        assert!(
            matches!(error, Error::Canceled("prefix creation")),
            "{error}"
        );
        assert!(prefix.is_dir());
        assert_eq!(fs::read_dir(&prefix).unwrap().count(), 0);
        assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "keep");
        assert!(!paths.environment_config("existing").exists());
    }

    #[test]
    fn names_lists_toml_files_with_valid_names() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        assert!(names(&paths).unwrap().is_empty());

        let dir = paths.environments_dir();
        fs::create_dir_all(dir.join("dir.toml")).unwrap();
        for file in ["zed.toml", "broken.toml", "notes.txt", "bad name.toml"] {
            fs::write(dir.join(file), "not even toml").unwrap();
        }
        assert_eq!(names(&paths).unwrap(), ["broken", "zed"]);

        assert!(Environment::ensure_exists(&paths, "broken").is_ok());
        assert!(matches!(
            Environment::ensure_exists(&paths, "dir"),
            Err(Error::UnknownEnvironment(_))
        ));
        assert!(matches!(
            Environment::ensure_exists(&paths, "../zed"),
            Err(Error::InvalidName(_))
        ));
    }

    #[test]
    fn load_reports_missing_and_invalid_names() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        assert!(matches!(
            Environment::load(&paths, "nope"),
            Err(Error::UnknownEnvironment(_))
        ));
        assert!(matches!(
            Environment::load(&paths, "../escape"),
            Err(Error::InvalidName(_))
        ));
    }

    #[test]
    #[ignore = "runs wine"]
    fn creates_a_real_prefix() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("home/prefixes/real");

        let env = Environment::create(
            &paths,
            &GlobalConfig::default(),
            &Runner::system().unwrap(),
            &new("real", &prefix),
            &Token::new(),
            Output::Log(&paths.runtime_dir("real").join("create.log")),
            |_| {},
        )
        .unwrap();
        assert!(prefix::is_prefix(&prefix));
        assert_eq!(env.config.wine().unwrap().arch, Arch::Win64);
    }

    #[test]
    #[ignore = "runs wine"]
    fn failed_creation_removes_the_new_dir() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("home/prefixes/win32");

        let mut options = new("win32", &prefix);
        options.arch = Some(Arch::Win32);
        let err = Environment::create(
            &paths,
            &GlobalConfig::default(),
            &Runner::system().unwrap(),
            &options,
            &Token::new(),
            Output::Log(&paths.runtime_dir("win32").join("create.log")),
            |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, Error::PrefixCreation { .. }), "{err}");
        assert!(!prefix.exists());
        assert!(!paths.environment_config("win32").exists());
    }
}
