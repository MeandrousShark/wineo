//! Desktop entries wineo writes for targets that ask for one.
//!
//! An entry runs `wineo start`, never wine directly, so a launch from the
//! desktop's menu carries everything the environment does: gamemode, DXVK, and
//! umu all apply exactly as they do from a terminal. That is also why the
//! `Exec` line needs no quoting, since environment and target names are limited
//! to letters, digits, `-`, and `_`.
//!
//! Entries are a reconciled setting rather than something a verb creates, so a
//! hand-edited config needs no command afterwards. The `X-Wineo-*` keys below
//! are how an entry is recognized as wineo's, which means no separate manifest
//! and means an entry whose target has gone can still be found and removed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use sha2::{Digest, Sha256};

use crate::art::{self, ArtConfig};
use crate::art_icon;
use crate::config::{GlobalConfig, Kind, Target};
use crate::environment::Environment;
use crate::error::{Error, IoContext, Result};
use crate::files::{self, write_atomic};
use crate::launch;
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::programs;
use crate::runner::Runner;
use crate::runners::Runners;
use crate::runtime::Runtime;
use crate::steam;
use crate::winpath;

const ENVIRONMENT_KEY: &str = "X-Wineo-Environment";
const TARGET_KEY: &str = "X-Wineo-Target";
/// Where the icon files on disk came from, so a changed or removed art file
/// is noticed without decoding anything.
const ICON_KEY: &str = "X-Wineo-Icon";
/// Prefixed so wineo's entries never collide with anything else installed, and
/// so a stray one is recognizable before its contents are read.
const FILE_PREFIX: &str = "wineo-";

/// What reconciliation did, for the caller to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Wrote {
        target: String,
        icon: bool,
    },
    Updated {
        target: String,
    },
    Removed {
        target: String,
    },
    /// The target's icon art could not be decoded, so the exe's icon is used.
    /// Reported once per art file, alongside the entry's own change.
    IconArtUnreadable {
        target: String,
    },
    /// A target asked for an entry that cannot be made. Reported rather than
    /// returned as an error, because one broken target must not stop a launch
    /// of a different one. `wineo set` refuses it up front instead.
    Skipped {
        target: String,
        reason: String,
    },
    /// Something the caller should say, not an entry that changed: the global
    /// config was passed over for defaults, which the entries were built from.
    Warning(String),
}

/// Writes the entries a config now asks for. Run after the write, so an entry
/// is built from the config as it ends up rather than as it was. `held` says
/// the caller already has the environment to itself.
pub fn apply(paths: &Paths, name: &str, held: bool) -> Result<Vec<Change>> {
    let loaded = GlobalConfig::load_or_default(paths)?;
    let mut changes = apply_with(paths, name, held, &loaded.config)?;
    changes.extend(loaded.warning.map(Change::Warning));
    Ok(changes)
}

/// `apply` with a global config the caller has already loaded, and reported
/// on if it fell back to defaults.
pub fn apply_with(
    paths: &Paths,
    name: &str,
    held: bool,
    global: &GlobalConfig,
) -> Result<Vec<Change>> {
    let Ok(environment) = Environment::load(paths, name) else {
        return Ok(Vec::new());
    };
    match &environment.config.kind {
        Kind::Wine(_) => {}
        Kind::Native => return reconcile_native(paths, global, &environment),
        Kind::Proxied(_) => return reconcile_proxied(paths, global, &environment),
    }
    let Ok(prefix) = environment.prefix(paths) else {
        return Ok(Vec::new());
    };
    // Pulling out an icon runs wine in the prefix, which has to wait for
    // whatever holds it exclusively, like an installer, and for what an
    // unresolved one may have left running. The entry is still written, and
    // gets its icon on a later launch.
    let launch = (!held)
        .then(|| {
            Locks::new(paths)
                .acquire(Operation::Launch, name, &[&prefix])
                .ok()
        })
        .flatten();
    let may_run_wine =
        held || (launch.is_some() && Runtime::new(paths, name).check_resolved(name).is_ok());
    let runner = may_run_wine
        .then(|| Runners::new(paths, global).ok())
        .flatten()
        .and_then(|runners| {
            runners
                .resolve_locked(paths, &environment.wine().ok()?.runner)
                .ok()
        });
    reconcile(
        paths,
        global,
        &environment,
        runner.as_ref().map(|(runner, _)| runner),
        &prefix,
    )
}

/// An entry on disk that wineo wrote.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub environment: String,
    pub target: String,
    pub name: String,
    pub path: PathBuf,
}

/// Brings the entries for `environment` in line with its config. Returns what
/// changed, and nothing at all when the entries already match.
///
/// `runner` is only needed to pull an icon out of the prefix, so a missing one
/// costs the icon rather than the entry.
pub fn reconcile(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: &Path,
) -> Result<Vec<Change>> {
    reconcile_with(paths, global, environment, runner, Some(prefix))
}

/// `reconcile` for a native environment. Its icons come from art only, since
/// only wine can pull one out of an executable.
pub fn reconcile_native(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> Result<Vec<Change>> {
    reconcile_with(paths, global, environment, None, None)
}

/// Reconciles the implicit target of a proxied environment. Its icon can only
/// come from wineo's art store, and its launch goes through wineo so hooks run.
pub fn reconcile_proxied(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> Result<Vec<Change>> {
    reconcile_with(paths, global, environment, None, None)
}

/// What reconciling would change, worked out without writing anything or
/// running wine, so `validate` can say whether the entries are out of date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Write { target: String },
    Update { target: String },
    Remove { target: String },
}

/// `reconcile` as a dry run. A target whose entry can't be made is left out,
/// since reconciling would skip it too.
pub fn pending(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: &Path,
) -> Result<Vec<Pending>> {
    pending_with(paths, global, environment, runner, Some(prefix))
}

/// `pending` for a native environment.
pub fn pending_native(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> Result<Vec<Pending>> {
    pending_with(paths, global, environment, None, None)
}

/// `pending` for a proxied environment.
pub fn pending_proxied(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> Result<Vec<Pending>> {
    pending_with(paths, global, environment, None, None)
}

fn pending_with(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: Option<&Path>,
) -> Result<Vec<Pending>> {
    if supported(paths).is_err() {
        return Ok(Vec::new());
    }
    pending_in(paths, global, environment, runner, prefix, &list(paths)?)
}

/// `pending` for any kind against entries `list` already gave, so a caller
/// checking many environments reads the applications folder once. The
/// caller has checked the root is `supported`.
pub(crate) fn pending_in(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: Option<&Path>,
    listed: &[Entry],
) -> Result<Vec<Pending>> {
    let plan = plan(paths, global, environment, runner, prefix, listed)?;
    let mut pending = Vec::new();
    for (target, draft) in plan.wanted {
        match draft.map(|drafted| drafted.draft) {
            Ok(Some(draft)) if draft.existing.is_some() => pending.push(Pending::Update { target }),
            Ok(Some(_)) => pending.push(Pending::Write { target }),
            Ok(None) | Err(_) => {}
        }
    }
    pending.extend(plan.leftover.into_iter().map(|entry| Pending::Remove {
        target: entry.target,
    }));
    Ok(pending)
}

fn reconcile_with(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: Option<&Path>,
) -> Result<Vec<Change>> {
    // The entries on the host belong to XDG environments, even one with the
    // same name, so a portable one only reports what it asked for.
    if supported(paths).is_err() {
        return Ok(requested_targets(environment)
            .into_iter()
            .map(|target| Change::Skipped {
                target,
                reason: Error::PortableDesktop.to_string(),
            })
            .collect());
    }
    let plan = plan(paths, global, environment, runner, prefix, &list(paths)?)?;
    let mut changes = Vec::new();
    for (target_name, draft) in plan.wanted {
        let written = draft.and_then(|drafted| {
            let mut changes = match drafted.draft {
                Some(draft) => write(paths, environment, runner, prefix, &target_name, draft)?,
                None => Vec::new(),
            };
            changes.extend(drafted.art_warning.map(|error| {
                Change::Warning(format!(
                    "art for {target_name} could not be read, so its icon was left as it is: {error}"
                ))
            }));
            Ok(changes)
        });
        match written {
            Ok(written) => changes.extend(written),
            Err(reason) => changes.push(Change::Skipped {
                target: target_name,
                reason: reason.to_string(),
            }),
        }
    }
    for entry in plan.leftover {
        remove_file(&entry.path)?;
        remove_icons(paths, &environment.name, &entry.target)?;
        changes.push(Change::Removed {
            target: entry.target,
        });
    }
    Ok(changes)
}

/// What reconciling an environment involves, before anything is written.
struct Plan {
    /// Each target asking for an entry, with the draft of it, or None when the
    /// entry on disk already matches.
    wanted: Vec<(String, Result<Drafted>)>,
    /// Entries for a target that stopped asking for one, or one that was
    /// removed from the config entirely.
    leftover: Vec<Entry>,
}

fn plan(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: Option<&Path>,
    listed: &[Entry],
) -> Result<Plan> {
    let mut wanted = Vec::new();
    match &environment.config.kind {
        Kind::Wine(_) | Kind::Native => {
            for (target_name, target) in &environment.config.targets {
                if target.desktop {
                    let draft = draft(
                        paths,
                        global,
                        environment,
                        runner,
                        prefix,
                        target_name,
                        target,
                    );
                    wanted.push((target_name.clone(), draft));
                }
            }
        }
        Kind::Proxied(proxied) if proxied.desktop => {
            wanted.push((
                proxied.launcher.as_str().to_owned(),
                draft_proxied(paths, global, environment, proxied),
            ));
        }
        Kind::Proxied(_) => {}
    }
    let proxied_target = match &environment.config.kind {
        Kind::Proxied(proxied) => Some(proxied.launcher.as_str()),
        Kind::Wine(_) | Kind::Native => None,
    };
    let leftover = listed
        .iter()
        .filter(|entry| entry.environment == environment.name)
        .filter(|entry| {
            proxied_target.is_none_or(|target| entry.target == target)
                && !wanted.iter().any(|(target, _)| *target == entry.target)
        })
        .cloned()
        .collect();
    Ok(Plan { wanted, leftover })
}

/// Removes every entry belonging to `environment`, for `wineo rm`. Returns the
/// targets whose entries went.
pub fn remove_all(paths: &Paths, environment: &str) -> Result<Vec<String>> {
    if supported(paths).is_err() {
        return Ok(Vec::new());
    }
    let mut removed = Vec::new();
    for entry in owned_by(paths, environment)? {
        remove_file(&entry.path)?;
        remove_icons(paths, environment, &entry.target)?;
        removed.push(entry.target);
    }
    Ok(removed)
}

/// Removes one entry and its icons, for an entry whose environment is gone.
/// The file is left alone if it is no longer that entry.
pub fn remove_entry(paths: &Paths, entry: &Entry) -> Result<()> {
    supported(paths)?;
    // The names come from a file anyone could have written, and end up in
    // icon paths.
    for name in [&entry.environment, &entry.target] {
        if !crate::name::is_valid(name) {
            return Err(Error::InvalidName(name.clone()));
        }
    }
    if read_entry(&entry.path)
        .is_some_and(|found| found.environment == entry.environment && found.target == entry.target)
    {
        remove_file(&entry.path)?;
    }
    remove_icons(paths, &entry.environment, &entry.target)
}

/// Refuses portable mode, whose environments never get desktop entries.
pub fn supported(paths: &Paths) -> Result<()> {
    match paths.portable_root() {
        Some(_) => Err(Error::PortableDesktop),
        None => Ok(()),
    }
}

/// The desktop targets the config currently asks wineo to reconcile.
pub(crate) fn requested_targets(environment: &Environment) -> Vec<String> {
    match &environment.config.kind {
        Kind::Wine(_) | Kind::Native => environment
            .config
            .targets
            .iter()
            .filter(|(_, target)| target.desktop)
            .map(|(name, _)| name.clone())
            .collect(),
        Kind::Proxied(proxied) if proxied.desktop => {
            vec![proxied.launcher.as_str().to_owned()]
        }
        Kind::Proxied(_) => Vec::new(),
    }
}

// How often the applications folder was read on this thread, so a test can
// hold a check of many environments to one read.
#[cfg(test)]
thread_local! {
    pub(crate) static LISTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Every entry wineo has written, whatever environment it belongs to.
pub fn list(paths: &Paths) -> Result<Vec<Entry>> {
    #[cfg(test)]
    LISTS.set(LISTS.get() + 1);
    let dir = paths.applications_dir();
    let mut entries = Vec::new();
    for found in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = found.path();
        let name = found.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".desktop") {
            continue;
        }
        if let Some(entry) = read_entry(&path) {
            entries.push(entry);
        }
    }
    entries.sort();
    Ok(entries)
}

fn owned_by(paths: &Paths, environment: &str) -> Result<Vec<Entry>> {
    Ok(list(paths)?
        .into_iter()
        .filter(|entry| entry.environment == environment)
        .collect())
}

/// Parses back the keys wineo wrote. Anything without them is not wineo's,
/// whatever it is called.
fn read_entry(path: &Path) -> Option<Entry> {
    let text = fs::read_to_string(path).ok()?;
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(str::to_owned)
    };
    Some(Entry {
        environment: value(ENVIRONMENT_KEY)?,
        target: value(TARGET_KEY)?,
        name: value("Name")
            .map(|name| unescape(&name))
            .unwrap_or_default(),
        path: path.to_owned(),
    })
}

fn ensure_entry_available(path: &Path, environment: &str, target: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_)
            if read_entry(path).is_some_and(|entry| {
                entry.environment == environment && entry.target == target
            }) =>
        {
            Ok(())
        }
        Ok(_) => Err(Error::DesktopEntryConflict(path.to_owned())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(Error::DesktopEntryConflict(path.to_owned())),
    }
}

/// What reading a target's entry came to.
struct Drafted {
    /// None when the entry on disk already matches.
    draft: Option<Draft>,
    /// Why the target's art couldn't be read. The entry's icon is left as it
    /// is, and the caller says so.
    art_warning: Option<String>,
}

/// Everything about a target's entry that is known without writing it.
struct Draft {
    path: PathBuf,
    existing: Option<String>,
    existing_stamp: Option<String>,
    name: String,
    exe: String,
    exec: String,
    wine: bool,
    gaming: bool,
    art: Option<IconArt>,
    art_stamp: Option<String>,
    had_icon: bool,
    stale_art: bool,
    /// The art couldn't be read, so the icon is neither replaced nor removed
    /// and the entry keeps the stamp it has.
    art_unreadable: bool,
    art_only: bool,
}

impl Draft {
    fn render(&self, environment: &str, target: &str, icon: bool) -> String {
        let stamp = self.stamp(icon);
        render(&Desired {
            environment,
            target,
            name: &self.name,
            exe: &self.exe,
            exec: &self.exec,
            wine: self.wine,
            gaming: self.gaming,
            icon,
            stamp,
        })
    }

    fn stamp(&self, icon: bool) -> Option<&str> {
        if self.art_unreadable && self.existing.is_some() {
            return self.existing_stamp.as_deref();
        }
        match &self.art_stamp {
            Some(stamp) => Some(stamp),
            None => icon.then_some("exe"),
        }
    }
}

/// Reads what a target's entry should be. None when the entry on disk already
/// matches, which is what makes a reconcile on every launch cheap.
fn draft(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: Option<&Path>,
    target_name: &str,
    target: &Target,
) -> Result<Drafted> {
    match prefix {
        Some(prefix) => check(paths, prefix, &target.exe)?,
        None => {
            launch::check_native_exe(
                paths,
                global,
                &environment.name,
                &environment.config.env,
                &target.exe,
            )?;
        }
    }

    let path = entry_path(paths, &environment.name, target_name);
    let existing = fs::read_to_string(&path).ok();
    let existing_stamp = existing.as_deref().and_then(stamp_of).map(str::to_owned);
    let name = target
        .name
        .clone()
        .unwrap_or_else(|| target_name.to_owned());
    let gaming = runner.is_some_and(Runner::is_proton)
        || launch::gamemode(global, environment, Some(target))
        || launch::gamescope(environment, Some(target)).is_some()
        || launch::mangohud(global, environment, Some(target));

    let (art, art_warning) = match icon_art(paths, &environment.name, target_name)? {
        IconLookup::Art(art) => (Some(art), None),
        IconLookup::None => (None, None),
        IconLookup::Unreadable(error) => (None, Some(error)),
    };
    let art_stamp = art.as_ref().map(|art| format!("art:{}", art.hash));
    let had_icon = has_icon(paths, &environment.name, target_name);
    // Icons from art that has since gone are the art's, not the exe's.
    let stale_art = art.is_none()
        && art_warning.is_none()
        && existing_stamp
            .as_deref()
            .is_some_and(|stamp| stamp.starts_with("art:"));
    let draft = Draft {
        path,
        existing,
        existing_stamp,
        name,
        exe: target.exe.clone(),
        exec: format!("wineo start {} --target {target_name}", environment.name),
        wine: prefix.is_some(),
        gaming,
        art,
        art_stamp,
        had_icon,
        stale_art,
        art_unreadable: art_warning.is_some(),
        art_only: false,
    };
    // With an icon already there, the text below matches and nothing runs.
    let current = !draft.stale_art
        && draft.existing.as_deref()
            == Some(
                draft
                    .render(&environment.name, target_name, had_icon)
                    .as_str(),
            );
    Ok(Drafted {
        draft: (!current).then_some(draft),
        art_warning,
    })
}

fn draft_proxied(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    proxied: &crate::config::ProxiedConfig,
) -> Result<Drafted> {
    let target_name = proxied.launcher.as_str();
    let live_title = steam::active_for(paths, global).and_then(|install| {
        steam::manifests(&install)
            .into_iter()
            .find(|entry| entry.app_id == proxied.app_id)
            .and_then(|entry| entry.manifest.ok())
            .map(|manifest| manifest.name)
            .filter(|title| !title.is_empty())
    });
    let name = live_title
        .or_else(|| proxied.title.clone())
        .unwrap_or_else(|| environment.name.clone());
    let path = entry_path(paths, &environment.name, target_name);
    ensure_entry_available(&path, &environment.name, target_name)?;
    let existing = fs::read_to_string(&path).ok();
    let existing_stamp = existing.as_deref().and_then(stamp_of).map(str::to_owned);
    let (art, art_warning) = match icon_art(paths, &environment.name, target_name)? {
        IconLookup::Art(art) => (Some(art), None),
        IconLookup::None => (None, None),
        IconLookup::Unreadable(error) => (None, Some(error)),
    };
    let art_stamp = art.as_ref().map(|art| format!("art:{}", art.hash));
    let had_icon = has_icon(paths, &environment.name, target_name);
    let stale_art = art.is_none()
        && art_warning.is_none()
        && existing_stamp
            .as_deref()
            .is_some_and(|stamp| stamp.starts_with("art:"));
    let draft = Draft {
        path,
        existing,
        existing_stamp,
        name,
        exe: String::new(),
        exec: format!("wineo start {}", environment.name),
        wine: false,
        gaming: true,
        art,
        art_stamp,
        had_icon,
        stale_art,
        art_unreadable: art_warning.is_some(),
        art_only: true,
    };
    let current = !draft.stale_art
        && !(draft.art_only && draft.art.is_none() && !draft.art_unreadable && draft.had_icon)
        && draft.existing.as_deref()
            == Some(
                draft
                    .render(&environment.name, target_name, had_icon)
                    .as_str(),
            );
    Ok(Drafted {
        draft: (!current).then_some(draft),
        art_warning,
    })
}

fn write(
    paths: &Paths,
    environment: &Environment,
    runner: Option<&Runner>,
    prefix: Option<&Path>,
    target_name: &str,
    draft: Draft,
) -> Result<Vec<Change>> {
    if matches!(&environment.config.kind, Kind::Proxied(_)) {
        ensure_entry_available(&draft.path, &environment.name, target_name)?;
    }
    let extract = || {
        runner.zip(prefix).is_some_and(|(runner, prefix)| {
            extract_icon(
                paths,
                runner,
                prefix,
                &environment.name,
                target_name,
                &draft.exe,
            )
            .is_some()
        })
    };
    let had_icon = draft.had_icon;
    let stale_art = draft.stale_art;
    let existing_stamp = draft.existing_stamp.as_deref();
    let mut unreadable = false;
    let icon = match &draft.art {
        Some(art) => {
            let changed = existing_stamp != draft.art_stamp.as_deref();
            if changed || !had_icon {
                match art_icon::install(
                    &art.path,
                    art.format,
                    &paths.icons_dir(),
                    &icon_name(&environment.name, target_name),
                ) {
                    Ok(written) => {
                        // Another size or format left behind would win over
                        // the new one in some lookups.
                        for stale in icon_files(paths, &environment.name, target_name) {
                            if stale != written {
                                remove_file(&stale)?;
                            }
                        }
                        true
                    }
                    Err(Error::IconDecode { .. }) => {
                        // Only said when the file is new to this entry, so a bad
                        // file is not reported on every launch.
                        unreadable = changed;
                        let art_made =
                            existing_stamp.is_some_and(|stamp| stamp.starts_with("art:"));
                        if art_made || draft.art_only {
                            remove_icons(paths, &environment.name, target_name)?;
                        }
                        !draft.art_only && (had_icon && !art_made || extract())
                    }
                    Err(err) => return Err(err),
                }
            } else {
                true
            }
        }
        // Whatever icon is there stays, since nothing can be said about what
        // the art would have been. An entry with none yet still gets the
        // exe's, which a readable art.toml later replaces.
        None if draft.art_unreadable && (had_icon || draft.art_only) => had_icon,
        None => {
            if stale_art || draft.art_only {
                remove_icons(paths, &environment.name, target_name)?;
            }
            // Only attempted when the entry is being written anyway, so a
            // target with no icon to find does not pay for wine starting up
            // on every launch.
            !draft.art_only && (had_icon && !stale_art || extract())
        }
    };
    let text = draft.render(&environment.name, target_name, icon);
    write_atomic(&draft.path, text)?;

    let mut changes = vec![match draft.existing {
        Some(_) => Change::Updated {
            target: target_name.to_owned(),
        },
        None => Change::Wrote {
            target: target_name.to_owned(),
            icon,
        },
    }];
    if unreadable {
        changes.push(Change::IconArtUnreadable {
            target: target_name.to_owned(),
        });
    }
    Ok(changes)
}

/// The icon art a target's entry uses, and the stamp that tells whether it
/// changed.
struct IconArt {
    path: PathBuf,
    format: art::Format,
    hash: String,
}

/// What a target's icon art came to.
enum IconLookup {
    Art(IconArt),
    None,
    /// `art.toml` couldn't be read, with why. That isn't "no art": the icon
    /// the entry has is left alone rather than replaced from guesswork.
    Unreadable(String),
}

fn icon_art(paths: &Paths, environment: &str, target: &str) -> Result<IconLookup> {
    let art_dir = paths.art_dir(environment);
    let config = match ArtConfig::load(&art_dir) {
        Ok(config) => config,
        Err(error) => return Ok(IconLookup::Unreadable(crate::error::chain(&error))),
    };
    let Some(resolved) = art::resolve(&art_dir, &config, Some(target), art::Kind::Icon)? else {
        return Ok(IconLookup::None);
    };
    let path = resolved.found.path;
    let bytes = fs::read(&path).context(|| format!("failed to read {}", path.display()))?;
    let mut hash = format!("{:x}", Sha256::digest(&bytes));
    hash.truncate(16);
    Ok(IconLookup::Art(IconArt {
        path,
        format: resolved.found.format,
        hash,
    }))
}

/// The `X-Wineo-Icon` value an existing entry was written with.
fn stamp_of(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.strip_prefix(ICON_KEY)?.strip_prefix('='))
}

struct Desired<'a> {
    environment: &'a str,
    target: &'a str,
    name: &'a str,
    exe: &'a str,
    exec: &'a str,
    /// Whether wine runs it, which decides the window class.
    wine: bool,
    gaming: bool,
    icon: bool,
    stamp: Option<&'a str>,
}

fn render(desired: &Desired) -> String {
    let Desired {
        environment,
        target,
        name,
        exe,
        exec,
        wine,
        gaming,
        icon,
        stamp,
    } = desired;
    // `Software` is not a registered XDG category, so a registered one leads
    // and keeps the entry out of a catch-all menu group.
    let categories = if *gaming {
        "Game;"
    } else {
        "Utility;Software;"
    };
    let mut text = String::from("[Desktop Entry]\n");
    text.push_str("Type=Application\n");
    text.push_str(&format!("Name={}\n", clean(name)));
    text.push_str(&format!(
        "Comment=Runs in the wineo environment {environment}\n"
    ));
    text.push_str(&format!("Exec={exec}\n"));
    if *icon {
        text.push_str(&format!("Icon={}\n", icon_name(environment, target)));
    }
    if let Some(stamp) = stamp {
        text.push_str(&format!("{ICON_KEY}={stamp}\n"));
    }
    text.push_str("Terminal=false\n");
    text.push_str(&format!("Categories={categories}\n"));
    // A native program names its own window class, which can't be known
    // from its file name.
    if *wine && let Some(class) = wm_class(exe) {
        // Without this the launcher shows a second, generic icon for the
        // running window instead of matching it to this entry.
        text.push_str(&format!("StartupWMClass={class}\n"));
    }
    text.push_str(&format!("{ENVIRONMENT_KEY}={environment}\n"));
    text.push_str(&format!("{TARGET_KEY}={target}\n"));
    text
}

/// A desktop entry value is a single line, so anything that would break out of
/// one goes. Names come from shortcuts, which can hold whatever an installer put there.
fn clean(value: &str) -> String {
    // The spec reads a backslash in a string value as an escape, so a literal
    // one has to be doubled or `AC\DC` would come back as `AC` plus a stray
    // escape.
    value
        .chars()
        .filter(|char| !char.is_control())
        .collect::<String>()
        .trim()
        .replace('\\', "\\\\")
}

/// Reverses the string escapes of the Desktop Entry spec, so a name reads back
/// as the text it was written from.
fn unescape(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(char) = chars.next() {
        if char != '\\' {
            out.push(char);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// What wine reports as the window class, which is the executable's file name.
fn wm_class(exe: &str) -> Option<String> {
    let file = exe.rsplit(['\\', '/']).next()?.trim();
    if file.is_empty() {
        return None;
    }
    Some(if file.contains('.') {
        file.to_owned()
    } else {
        // A bare name like `notepad` is run as `notepad.exe`.
        format!("{file}.exe")
    })
}

fn entry_path(paths: &Paths, environment: &str, target: &str) -> PathBuf {
    paths
        .applications_dir()
        .join(format!("{FILE_PREFIX}{environment}-{target}.desktop"))
}

/// Named after the environment and target rather than the executable, so the
/// entry's `Icon` line is known before the icon has been pulled out.
fn icon_name(environment: &str, target: &str) -> String {
    format!("{FILE_PREFIX}{environment}-{target}")
}

fn remove_file(path: &Path) -> Result<()> {
    files::remove_if_present(path)
        .map(drop)
        .context(|| format!("failed to remove {}", path.display()))
}

/// The icon theme directories wineo may have put an icon in. Which sizes exist
/// depends on what the executable carried, so they are discovered, not assumed.
fn icon_files(paths: &Paths, environment: &str, target: &str) -> Vec<PathBuf> {
    let hicolor = paths.icons_dir().join("hicolor");
    let name = icon_name(environment, target);
    let mut found = Vec::new();
    for size in fs::read_dir(&hicolor).into_iter().flatten().flatten() {
        for extension in ["png", "svg"] {
            let candidate = size.path().join("apps").join(format!("{name}.{extension}"));
            if candidate.is_file() {
                found.push(candidate);
            }
        }
    }
    found.sort();
    found
}

fn has_icon(paths: &Paths, environment: &str, target: &str) -> bool {
    !icon_files(paths, environment, target).is_empty()
}

fn remove_icons(paths: &Paths, environment: &str, target: &str) -> Result<()> {
    for path in icon_files(paths, environment, target) {
        remove_file(&path)?;
    }
    Ok(())
}

/// Refuses an executable that is not really there, so a typo never becomes an
/// entry that fails the moment it is clicked. A bare name like `notepad` is
/// left alone, since wine is the one that looks those up. `prefix` is the
/// configured directory, and the wine prefix inside it is worked out from the
/// layout rather than the runner, which may be gone.
pub fn check(paths: &Paths, prefix: &Path, exe: &str) -> Result<()> {
    let wine_prefix = crate::prefix::wine_prefix_in(prefix).unwrap_or_else(|| prefix.to_owned());
    let found = if winpath::is_windows_path(exe) {
        winpath::to_unix(&wine_prefix, exe).is_some_and(|unix| unix.is_file())
    } else if exe.contains('/') {
        paths.config_path(exe).is_ok_and(|unix| unix.is_file())
    } else {
        true
    };
    if found {
        Ok(())
    } else {
        Err(Error::ExeNotFound(exe.to_owned()))
    }
}

/// Pulls the executable's icon out with wine's own menu builder, the one 0001
/// otherwise keeps switched off.
///
/// Wine only processes a `.lnk` that sits in a Start Menu or on a Desktop, and
/// writes both an icon theme and a `.desktop` of its own. So this finds the
/// shortcut already pointing at the executable, points `XDG_DATA_HOME` at a
/// temporary directory, and keeps only the icons: wineo writes its own entry,
/// and wine's never reaches the real applications directory.
fn extract_icon(
    paths: &Paths,
    runner: &Runner,
    prefix: &Path,
    environment: &str,
    target: &str,
    exe: &str,
) -> Option<()> {
    let _build_guard = crate::build_lock::runner(paths, runner).ok()?;
    let wine_prefix = runner.wine_prefix(prefix);
    let shortcut = shortcut_for(&wine_prefix, exe)?;
    let scratch = Scratch::new(paths).ok()?;

    let status = runner
        .command(prefix, &[])
        .ok()?
        .args(["winemenubuilder.exe", "-a", "-r", &shortcut])
        // Set outright rather than merged: this command exists only to run the
        // menu builder, so an inherited value that disables it has nothing to say.
        .env("WINEDLLOVERRIDES", "winemenubuilder.exe=b")
        .env("XDG_DATA_HOME", scratch.path())
        .env("XDG_CONFIG_HOME", scratch.path().join("config"))
        .env("WINEDEBUG", "-all")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }

    // Wine picks the icon's file name itself, from a hash it does not report,
    // so it is read back out of the entry wine wrote beside it.
    let written = wine_icon_name(&scratch.path().join("applications"))?;
    let mut copied = 0;
    let hicolor = scratch.path().join("icons/hicolor");
    for size in fs::read_dir(&hicolor).into_iter().flatten().flatten() {
        let from = size.path().join("apps").join(format!("{written}.png"));
        if !from.is_file() {
            continue;
        }
        let to = paths
            .icons_dir()
            .join("hicolor")
            .join(size.file_name())
            .join("apps")
            .join(format!("{}.png", icon_name(environment, target)));
        if to
            .parent()
            .is_some_and(|dir| fs::create_dir_all(dir).is_err())
        {
            continue;
        }
        if fs::copy(&from, &to).is_ok() {
            copied += 1;
        }
    }
    (copied > 0).then_some(())
}

/// A directory for wine to write an icon theme into, under wineo's own state so
/// nothing depends on the system temp dir, and removed however this call ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(paths: &Paths) -> Result<Self> {
        let dir = paths
            .icon_scratch_dir()
            .join(std::process::id().to_string());
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).context(|| format!("failed to create {}", dir.display()))?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Reads the shortcut icon, ignoring file associations Wine may refresh too.
fn wine_icon_name(dir: &Path) -> Option<String> {
    let mut stack = vec![dir.to_owned()];
    while let Some(dir) = stack.pop() {
        for found in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = found.path();
            if found.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "desktop")
                && let Ok(text) = fs::read_to_string(&path)
                && !text.lines().any(|line| line.starts_with("MimeType="))
                && let Some(icon) = text
                    .lines()
                    .find_map(|line| line.strip_prefix("Icon="))
                    .map(str::trim)
                && !icon.is_empty()
            {
                return Some(icon.to_owned());
            }
        }
    }
    None
}

/// A Start Menu shortcut already pointing at `exe`, which is the only thing
/// wine's menu builder will take an icon from.
fn shortcut_for(wine_prefix: &Path, exe: &str) -> Option<String> {
    if !winpath::is_windows_path(exe) {
        return None;
    }
    let windows = exe.to_ascii_lowercase();
    let listing = programs::scan(wine_prefix).ok()?;
    listing
        .programs
        .into_iter()
        .find(|program| program.exe.to_ascii_lowercase() == windows)
        .and_then(|program| program.lnk)
}

#[cfg(test)]
mod tests;
