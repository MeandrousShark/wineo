//! Finds config mistakes and leftover state before a launch trips over them,
//! and fixes the ones that are safe to fix.
//!
//! Checks only read, and nothing launches wine or runs a program. Fixes never
//! edit an environment config, except explicit snapshot recovery, which restores
//! the saved bytes. The winetricks fix installs into an idle
//! prefix; art fixes only drop records whose file or target is gone.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Serialize, Serializer};

use crate::art::{self, Kind as ArtKind};
use crate::components::{Builds, Component, LATEST};
use crate::config::{EnvironmentConfig, GlobalConfig, Kind};
use crate::desktop::{self, Entry, Pending};
use crate::environment::{self, Environment};
use crate::error::{Error, IoContext, Result, chain};
use crate::files;
use crate::gog::folder::{self, DeviceOf, FolderState};
use crate::install::{self, Wanted};
use crate::installer::{self, Cleaned};
use crate::launch;
use crate::name;
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::prefix::{self, BuildPlan};
use crate::runner::{self, Runner};
use crate::runners::Runners;
use crate::runtime::Runtime;
use crate::snapshot;
use crate::steam::{self, AppState};
use crate::templates::{Template, Templates};
use crate::trash::{self, Phase};
use crate::winetricks;
use crate::winpath;

/// How bad a problem is. An error means something will fail: a command
/// refuses the config, or a launch can't work. A warning is leftover or odd
/// state that doesn't stop anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What part of the setup a problem is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    GlobalConfig,
    Template,
    EnvironmentConfig,
    Wine,
    Target,
    NativeTarget,
    Environment,
    Runtime,
    Desktop,
    Steam,
    Art,
    Trash,
    Gog,
}

impl Area {
    pub fn as_str(self) -> &'static str {
        match self {
            Area::GlobalConfig => "global_config",
            Area::Template => "template",
            Area::EnvironmentConfig => "environment_config",
            Area::Wine => "wine",
            Area::Target => "target",
            Area::NativeTarget => "native_target",
            Area::Environment => "environment",
            Area::Runtime => "runtime",
            Area::Desktop => "desktop",
            Area::Steam => "steam",
            Area::Art => "art",
            Area::Trash => "trash",
            Area::Gog => "gog",
        }
    }
}

impl fmt::Display for Area {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One list generates the enum, `ALL`, and the accessors, so they can't drift apart.
macro_rules! codes {
    ($($variant:ident => $text:literal, $level:path, $area:path;)*) => {
        /// Which check found a problem. Fixed, so a script or the GUI can match on it
        /// without parsing messages.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum Code {
            $($variant,)*
        }

        impl Code {
            pub const ALL: [Code; [$(stringify!($variant)),*].len()] = [$(Code::$variant),*];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Code::$variant => $text,)*
                }
            }

            /// Every check has one level, so it is decided here rather than per problem.
            pub fn level(self) -> Level {
                match self {
                    $(Code::$variant => $level,)*
                }
            }

            pub fn area(self) -> Area {
                match self {
                    $(Code::$variant => $area,)*
                }
            }
        }
    };
}

codes! {
    GlobalConfigInvalid => "global_config_invalid", Level::Error, Area::GlobalConfig;
    TemplateInvalid => "template_invalid", Level::Error, Area::Template;
    TemplateUnresolved => "template_unresolved", Level::Warning, Area::Template;
    EnvironmentConfigInvalid => "environment_config_invalid", Level::Error, Area::EnvironmentConfig;
    RunnerMissing => "runner_missing", Level::Error, Area::Wine;
    BuildMissing => "build_missing", Level::Error, Area::Wine;
    WinetricksMissing => "winetricks_missing", Level::Warning, Area::Wine;
    WinetricksUnavailable => "winetricks_unavailable", Level::Error, Area::Wine;
    PrefixMissing => "prefix_missing", Level::Warning, Area::Wine;
    PrefixRefused => "prefix_refused", Level::Error, Area::Wine;
    ExeMissing => "exe_missing", Level::Error, Area::Target;
    CwdMissing => "cwd_missing", Level::Error, Area::Target;
    GamemodeMissing => "gamemode_missing", Level::Error, Area::Target;
    GamescopeMissing => "gamescope_missing", Level::Error, Area::Target;
    MangohudMissing => "mangohud_missing", Level::Error, Area::Target;
    WaylandUnsupported => "wayland_unsupported", Level::Error, Area::Target;
    NativeExeInvalid => "native_exe_invalid", Level::Error, Area::NativeTarget;
    TargetAmbiguous => "target_ambiguous", Level::Warning, Area::Environment;
    LostRun => "lost_run", Level::Warning, Area::Runtime;
    InstallerUnresolved => "installer_unresolved", Level::Warning, Area::Runtime;
    RestoreInterrupted => "restore_interrupted", Level::Error, Area::Runtime;
    StateDirStray => "state_dir_stray", Level::Warning, Area::Runtime;
    DesktopEntryStray => "desktop_entry_stray", Level::Warning, Area::Desktop;
    DesktopOutdated => "desktop_outdated", Level::Warning, Area::Desktop;
    DesktopPortable => "desktop_portable", Level::Warning, Area::Desktop;
    SteamNotFound => "steam_not_found", Level::Error, Area::Steam;
    SteamInstallMissing => "steam_install_missing", Level::Error, Area::Steam;
    SteamLibraryUnavailable => "steam_library_unavailable", Level::Warning, Area::Steam;
    SteamDataUnknown => "steam_data_unknown", Level::Error, Area::Steam;
    ArtConfigInvalid => "art_config_invalid", Level::Error, Area::Art;
    ArtFileMissing => "art_file_missing", Level::Warning, Area::Art;
    ArtTargetUnknown => "art_target_unknown", Level::Warning, Area::Art;
    ArtDuplicates => "art_duplicates", Level::Warning, Area::Art;
    ArtDirStray => "art_dir_stray", Level::Warning, Area::Art;
    StatsStray => "stats_stray", Level::Warning, Area::Runtime;
    TrashInterrupted => "trash_interrupted", Level::Error, Area::Trash;
    TrashInvalid => "trash_invalid", Level::Error, Area::Trash;
    GogStateInvalid => "gog_state_invalid", Level::Error, Area::Gog;
    GogStateMissing => "gog_state_missing", Level::Error, Area::Gog;
    GogFolderMissing => "gog_folder_missing", Level::Warning, Area::Gog;
    GogFolderTaken => "gog_folder_taken", Level::Error, Area::Gog;
    GogMidUpdate => "gog_mid_update", Level::Error, Area::Gog;
    GogOwnerMissing => "gog_owner_missing", Level::Warning, Area::Gog;
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A fix `apply` can make. Serialized as its label, which is how `--json`
/// shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Downloads a DXVK or vkd3d-proton build: the one a config pins, or the
    /// newest release for `latest`. Not tied to an environment, so two that
    /// need the same build share one fix.
    Download {
        component: Component,
        version: Option<String>,
    },
    InstallWinetricks {
        environment: String,
    },
    CleanUpLostRun {
        environment: String,
    },
    CleanUpInstaller {
        environment: String,
    },
    /// Removes the state dir of an environment that no longer exists.
    RemoveStateDir {
        environment: String,
    },
    /// Removes an entry whose environment no longer exists, and its icons.
    RemoveDesktopEntry(Entry),
    /// Rewrites an environment's entries, like `start` does.
    RewriteDesktop {
        environment: String,
    },
    /// Drops an `art.toml` record whose file is gone.
    DropArtRecord {
        environment: String,
        target: Option<String>,
        kind: ArtKind,
    },
    /// Drops the `art.toml` records of a target that no longer exists.
    DropArtTarget {
        environment: String,
        target: String,
    },
    /// Removes a play log left by an environment that no longer exists.
    RemoveStatsLog {
        environment: String,
    },
    RecoverTrash {
        id: String,
    },
    /// Rolls back an interrupted restore only through `apply_single`.
    RecoverSnapshot {
        environment: String,
    },
    /// Removes the state of a GOG folder that was surely deleted.
    ForgetGogFolder {
        folder: PathBuf,
    },
    /// Sets a GOG folder's owner to none when that environment is gone.
    ReleaseGogFolder {
        folder: PathBuf,
        owner: String,
    },
}

impl Fix {
    /// Whether this fix can be included in a batch.
    pub fn is_batch_safe(&self) -> bool {
        !matches!(self, Self::RecoverSnapshot { .. })
    }

    /// What the fix does, in a few words.
    pub fn label(&self) -> String {
        match self {
            Fix::InstallWinetricks { environment } => {
                format!("Install {environment}'s missing winetricks verbs")
            }
            Fix::Download {
                component,
                version: Some(version),
            } => format!("Download {} {version}", component.label()),
            Fix::Download {
                component,
                version: None,
            } => format!("Download the latest {}", component.label()),
            Fix::CleanUpLostRun { environment } => format!("Clean up {environment}'s lost run"),
            Fix::CleanUpInstaller { environment } => {
                format!("Clean up after {environment}'s unfinished installer")
            }
            Fix::RemoveStateDir { environment } => format!("Remove {environment}'s leftover state"),
            Fix::RemoveDesktopEntry(entry) if entry.target == entry.environment => {
                format!("Remove {}'s desktop entry", entry.environment)
            }
            Fix::RemoveDesktopEntry(entry) => format!(
                "Remove {}'s desktop entry for {}",
                entry.environment, entry.target
            ),
            Fix::RewriteDesktop { environment } => {
                format!("Rewrite {environment}'s desktop entries")
            }
            Fix::DropArtRecord {
                environment,
                target,
                kind,
            } => match target {
                Some(target) => format!("Drop {environment}'s missing {kind} record for {target}"),
                None => format!("Drop {environment}'s missing {kind} record"),
            },
            Fix::DropArtTarget {
                environment,
                target,
            } => format!("Drop {environment}'s art records for {target}"),
            Fix::RemoveStatsLog { environment } => {
                format!("Remove {environment}'s leftover play stats")
            }
            Fix::RecoverTrash { id } => format!("Recover interrupted trash entry {id}"),
            Fix::RecoverSnapshot { environment } => {
                format!("Roll back {environment}'s interrupted snapshot restore")
            }
            Fix::ForgetGogFolder { folder } => {
                format!("Forget the deleted GOG folder {}", folder.display())
            }
            Fix::ReleaseGogFolder { folder, owner } => {
                format!("Clear {owner} as the owner of {}", folder.display())
            }
        }
    }
}

impl fmt::Display for Fix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Warning(String),
}

impl Serialize for Fix {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// One thing validate found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// None for the global config, templates, and stray items.
    pub environment: Option<String>,
    pub area: Area,
    pub level: Level,
    pub code: Code,
    pub message: String,
    pub fix: Option<Fix>,
    /// The name a stray item was left by, for grouping it in a report.
    /// `environment` stays None, since that environment is gone.
    #[serde(skip)]
    pub stray: Option<String>,
    /// The template a template problem is in, for grouping it in a report.
    #[serde(skip)]
    pub template: Option<String>,
}

impl Problem {
    fn new(code: Code, environment: Option<&str>, message: String, fix: Option<Fix>) -> Self {
        Self {
            environment: environment.map(str::to_owned),
            area: code.area(),
            level: code.level(),
            code,
            message,
            fix,
            stray: None,
            template: None,
        }
    }

    fn stray(code: Code, name: &str, message: String, fix: Option<Fix>) -> Self {
        Self {
            stray: Some(name.to_owned()),
            ..Self::new(code, None, message, fix)
        }
    }

    fn template(code: Code, name: &str, message: String) -> Self {
        Self {
            template: Some(name.to_owned()),
            ..Self::new(code, None, message, None)
        }
    }
}

/// What a run of validate checked and found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Every environment checked, sorted.
    pub environments: Vec<String>,
    /// Every template file checked. The shipped ones aren't.
    pub templates: Vec<String>,
    pub problems: Vec<Problem>,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.count(Level::Error)
    }

    pub fn warnings(&self) -> usize {
        self.count(Level::Warning)
    }

    /// Distinct batch-safe fixes in problem order. Shared downloads are offered once.
    pub fn fixes(&self) -> Vec<Fix> {
        let mut fixes = Vec::new();
        for fix in self
            .problems
            .iter()
            .filter_map(|problem| problem.fix.as_ref())
            .filter(|fix| fix.is_batch_safe())
        {
            if !fixes.contains(fix) {
                fixes.push(fix.clone());
            }
        }
        fixes
    }

    fn count(&self, level: Level) -> usize {
        self.problems
            .iter()
            .filter(|problem| problem.level == level)
            .count()
    }

    fn push(&mut self, problem: Problem) {
        self.problems.push(problem);
    }
}

/// Checks the global config, every template, every environment, and what is
/// left behind by environments that no longer exist, in the active root only.
pub fn check_all(paths: &Paths) -> Result<Report> {
    let mut report = Report::default();
    let pending = check_trash(paths, None, &mut report)?;
    let global = check_global(paths, &mut report);
    check_templates(paths, &global, &mut report)?;
    let names = environment::names(paths)?;
    let entries = listed_entries(paths)?;
    for name in &names {
        if !pending.contains(name) {
            check_one(paths, &global, name, &entries, &mut report)?;
        }
    }
    check_gog(
        paths,
        None,
        &names,
        &pending,
        &folder::device_of,
        &mut report,
    );
    let mut protected = names.clone();
    protected.extend(pending);
    if !report
        .problems
        .iter()
        .any(|problem| problem.code == Code::TrashInvalid)
    {
        check_strays(paths, &protected, &entries, &mut report)?;
    }
    report.environments = names;
    contract(paths, &mut report);
    Ok(report)
}

/// Paths in messages are shown with `~` for the home dir, as elsewhere.
fn contract(paths: &Paths, report: &mut Report) {
    for problem in &mut report.problems {
        problem.message = paths.contract_text(&problem.message);
    }
}

/// Checks one environment: its config, targets, runtime state, desktop
/// entries, and art.
pub fn check_environment(paths: &Paths, name: &str) -> Result<Report> {
    if !name::is_valid(name) {
        return Err(Error::InvalidName(name.to_owned()));
    }
    let mut report = Report {
        environments: vec![name.to_owned()],
        ..Report::default()
    };
    let pending = check_trash(paths, Some(name), &mut report)?;
    if pending.contains(name) {
        return Ok(report);
    }
    if !paths.environment_config(name).is_file() {
        return Err(Error::UnknownEnvironment(name.to_owned()));
    }
    // A broken global config is only reported by `check_all`. This report
    // only displays problems and changes nothing, so defaults are enough.
    let global = GlobalConfig::load(paths).unwrap_or_default();
    let entries = listed_entries(paths)?;
    check_one(paths, &global, name, &entries, &mut report)?;
    let names = environment::names(paths)?;
    check_gog(
        paths,
        Some(name),
        &names,
        &pending,
        &folder::device_of,
        &mut report,
    );
    contract(paths, &mut report);
    Ok(report)
}

/// GOG folder states, and environments whose `gog_id` names a game. Host
/// root only, since GOG games live in the host library. With `selected`,
/// only that environment and the folders it owns.
fn check_gog(
    paths: &Paths,
    selected: Option<&str>,
    names: &[String],
    pending: &BTreeSet<String>,
    device_of: DeviceOf<'_>,
    report: &mut Report,
) {
    if paths.portable_root().is_some() {
        return;
    }
    // A state that can't be read may be the selected environment's, unless
    // its config can't name a game.
    let show_invalid = selected.is_none_or(|name| folder::may_track_game(paths, name));
    let listed = match folder::list(paths) {
        Ok(listed) => listed,
        Err(err) => {
            if show_invalid {
                report.push(Problem::new(
                    Code::GogStateInvalid,
                    None,
                    format!("GOG folder states can't be read: {err:#}"),
                    None,
                ));
            }
            return;
        }
    };
    let mut states = Vec::new();
    for (_, state) in listed {
        match state {
            Ok(state) => states.push(state),
            Err(err) if show_invalid => report.push(Problem::new(
                Code::GogStateInvalid,
                None,
                format!("{err:#}"),
                None,
            )),
            Err(_) => {}
        }
    }
    let known = |owner: &str| names.iter().any(|name| name == owner) || pending.contains(owner);
    for state in &states {
        if selected.is_some() && state.owner.as_deref() != selected {
            continue;
        }
        let environment = state.owner.as_deref().filter(|owner| known(owner));
        check_gog_folder(paths, state, environment, device_of, report);
        // An install whose config isn't written yet is finished by
        // installing again, which releasing its owner would prevent.
        if let Some(owner) = &state.owner
            && !known(owner)
            && !folder::install_without_config(paths, state)
        {
            report.push(Problem::new(
                Code::GogOwnerMissing,
                None,
                format!(
                    "GOG game folder {} names {owner}, which doesn't exist.",
                    state.folder.display()
                ),
                Some(Fix::ReleaseGogFolder {
                    folder: state.folder.clone(),
                    owner: owner.clone(),
                }),
            ));
        }
    }
    for name in names {
        if selected.is_some_and(|selected| selected != name) || pending.contains(name) {
            continue;
        }
        let Some(gog_id) = Environment::load(paths, name)
            .ok()
            .and_then(|environment| environment.config.gog_id)
        else {
            continue;
        };
        let game: Vec<&FolderState> = states
            .iter()
            .filter(|state| state.is_game(&gog_id))
            .collect();
        // An ownerless folder is claimed again by the next launch or update.
        if game
            .iter()
            .any(|state| state.owner.as_deref().is_none_or(|owner| owner == name))
        {
            continue;
        }
        let (code, message) = match game.first() {
            Some(taken) => (
                Code::GogFolderTaken,
                format!(
                    "{name}'s GOG game folder {} belongs to {}.",
                    taken.folder.display(),
                    taken.owner.as_deref().unwrap_or_default()
                ),
            ),
            None => (
                Code::GogStateMissing,
                format!(
                    "{name}'s GOG state is missing. Migrate it again with wineo import gog --game {gog_id}, or stop tracking it with wineo set {name} --unset gog_id."
                ),
            ),
        };
        report.push(Problem::new(code, Some(name), message, None));
    }
}

/// A folder mid-update, or one that can't be found.
fn check_gog_folder(
    paths: &Paths,
    state: &FolderState,
    environment: Option<&str>,
    device_of: DeviceOf<'_>,
    report: &mut Report,
) {
    let folder = state.folder.display();
    if state.journal.is_some() {
        let message = match environment {
            Some(owner) => match state
                .journal
                .as_ref()
                .and_then(folder::Journal::galaxy_install)
            {
                Some(gog_id) => folder::FolderError::MidInstall {
                    name: owner.to_owned(),
                    gog_id,
                }
                .to_string(),
                None => format!(
                    "{owner}'s game folder is mid-update. Finish it with wineo update {owner}."
                ),
            },
            None => match folder::unfinished(paths, state) {
                refusal @ (folder::FolderError::UnfinishedInstall { .. }
                | folder::FolderError::OwnerlessInstall { .. }) => refusal.to_string(),
                _ => format!(
                    "GOG game folder {folder} is mid-update. Migrate it again with wineo import gog and finish it."
                ),
            },
        };
        report.push(Problem::new(Code::GogMidUpdate, environment, message, None));
    }
    match device_of(&state.folder) {
        Ok(Some(_)) => {}
        Ok(None) => {
            let safe = state.owner.is_none()
                && state.journal.is_none()
                && folder::surely_deleted(state, device_of);
            report.push(Problem::new(
                Code::GogFolderMissing,
                environment,
                format!("GOG game folder {folder} is missing."),
                safe.then(|| Fix::ForgetGogFolder {
                    folder: state.folder.clone(),
                }),
            ));
        }
        // Can't tell whether it's there, so it's never forgotten.
        Err(err) => report.push(Problem::new(
            Code::GogFolderMissing,
            environment,
            format!("GOG game folder {folder} can't be checked: {err}."),
            None,
        )),
    }
}

fn check_trash(
    paths: &Paths,
    selected: Option<&str>,
    report: &mut Report,
) -> Result<BTreeSet<String>> {
    let mut pending = BTreeSet::new();
    let mut marked = BTreeSet::new();
    match trash::pending_entries(paths) {
        Ok(entries) => {
            for entry in entries {
                pending.insert(entry.name.clone());
                marked.insert(entry.id.clone());
                if selected.is_some_and(|name| name != entry.name) {
                    continue;
                }
                report.push(Problem::new(
                    Code::TrashInterrupted,
                    Some(&entry.name),
                    format!(
                        "trash entry {} has pending recovery{}; run `wineo validate --fix`",
                        entry.id,
                        if entry.finished { " (finalizing)" } else { "" }
                    ),
                    Some(Fix::RecoverTrash { id: entry.id }),
                ));
            }
        }
        Err(err) => report.push(Problem::new(
            Code::TrashInvalid,
            None,
            format!("pending trash markers cannot be inspected: {err:#}"),
            None,
        )),
    }
    match trash::list(paths) {
        Ok(entries) => {
            for entry in entries {
                if entry.phase == Phase::Trashed || marked.contains(&entry.id) {
                    continue;
                }
                pending.insert(entry.name.clone());
                if selected.is_some_and(|name| name != entry.name) {
                    continue;
                }
                report.push(Problem::new(
                    Code::TrashInterrupted,
                    Some(&entry.name),
                    format!(
                        "trash entry {} has interrupted {:?}; recover with `wineo validate --fix`",
                        entry.id, entry.phase
                    ),
                    Some(Fix::RecoverTrash { id: entry.id }),
                ));
            }
        }
        Err(err) => report.push(Problem::new(
            Code::TrashInvalid,
            None,
            format!("trash storage cannot be inspected: {err:#}"),
            None,
        )),
    }
    Ok(pending)
}

fn check_global(paths: &Paths, report: &mut Report) -> GlobalConfig {
    match GlobalConfig::load(paths) {
        Ok(global) => global,
        Err(err) => {
            report.push(Problem::new(
                Code::GlobalConfigInvalid,
                None,
                chain(&err),
                None,
            ));
            GlobalConfig::default()
        }
    }
}

fn check_templates(paths: &Paths, global: &GlobalConfig, report: &mut Report) -> Result<()> {
    let templates = Templates::new(paths);
    for listed in templates.list()? {
        if listed.built_in {
            continue;
        }
        match templates.resolve(&listed.name) {
            Ok(template) => check_template(paths, global, &listed.name, &template, report),
            Err(err) => report.push(Problem::template(
                Code::TemplateInvalid,
                &listed.name,
                chain(&err),
            )),
        }
        report.templates.push(listed.name);
    }
    Ok(())
}

/// A template's picks are only resolved when `new` uses it, so one that no
/// longer resolves is a warning rather than an error.
fn check_template(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    template: &Template,
    report: &mut Report,
) {
    let mut unresolved = |err: Error| {
        report.push(Problem::template(
            Code::TemplateUnresolved,
            name,
            format!("template {name}: {}", chain(&err)),
        ));
    };
    let runners = match Runners::new(paths, global) {
        Ok(runners) => runners,
        Err(err) => return unresolved(err),
    };
    let fallback = &global.defaults.runner;
    let runner = match template.runner_name(&runners, fallback) {
        Ok(Some(named)) => match runners.resolve(&named) {
            Ok(runner) => Some(runner),
            Err(err) => {
                unresolved(err);
                None
            }
        },
        // The default runner is the global config's to get right, not the template's.
        Ok(None) => runners.resolve(fallback).ok(),
        Err(err) => {
            unresolved(err);
            None
        }
    };
    // Proton bundles its own, so `new` leaves the template's picks out.
    if runner.as_ref().is_some_and(Runner::is_proton) {
        return;
    }
    for (component, asked) in [
        (Component::Dxvk, &template.dxvk),
        (Component::Vkd3d, &template.vkd3d),
    ] {
        if let Some(asked) = asked
            && let Err(err) =
                Builds::new(paths, global, component).and_then(|builds| builds.resolve(asked))
        {
            unresolved(err);
        }
    }
}

/// What the Wine checks found out, for the checks that depend on it.
#[derive(Default)]
struct WineState {
    runner: Option<Runner>,
    prefix: Option<PathBuf>,
    /// A prefix is there to check `C:\` paths against.
    usable: bool,
}

/// The desktop entries wineo wrote, read once for every check that needs them.
fn listed_entries(paths: &Paths) -> Result<Vec<Entry>> {
    if desktop::supported(paths).is_ok() {
        desktop::list(paths)
    } else {
        Ok(Vec::new())
    }
}

fn check_one(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    entries: &[Entry],
    report: &mut Report,
) -> Result<()> {
    // Every command refuses the environment until it is recovered, and the
    // config may be half restored, so nothing else is worth checking.
    let marker = paths.restore_marker(name);
    match fs::symlink_metadata(&marker) {
        Ok(_) => {
            let (message, fix) = match snapshot::inspect_recovery(paths, name) {
                Ok(()) => (
                    format!(
                        "a snapshot restore was interrupted. Recover it with `wineo snapshot recover {name}`"
                    ),
                    Some(Fix::RecoverSnapshot {
                        environment: name.to_owned(),
                    }),
                ),
                Err(err) => (
                    format!(
                        "a snapshot restore was interrupted. Recover it with `wineo snapshot recover {name}`. Invalid recovery marker: {err:#}"
                    ),
                    None,
                ),
            };
            report.push(Problem::new(
                Code::RestoreInterrupted,
                Some(name),
                message,
                fix,
            ));
            return Ok(());
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).context(|| format!("failed to inspect {}", marker.display()));
        }
    }

    let path = paths.environment_config(name);
    let Some(text) = files::read_to_string_optional(&path)
        .context(|| format!("failed to read {}", path.display()))?
    else {
        // Removed since it was listed.
        return Ok(());
    };
    let config = match EnvironmentConfig::parse(&path, &text) {
        Ok(config) => config,
        Err(err) => {
            report.push(Problem::new(
                Code::EnvironmentConfigInvalid,
                Some(name),
                chain(&err),
                None,
            ));
            return Ok(());
        }
    };
    let environment = Environment {
        name: name.to_owned(),
        path,
        config,
    };
    match &environment.config.kind {
        Kind::Wine(_) | Kind::Native => {}
        Kind::Proxied(_) => {
            check_runtime(paths, name, report)?;
            check_proxied_steam(paths, global, &environment, report);
            check_desktop(
                paths,
                global,
                &environment,
                &WineState::default(),
                entries,
                report,
            )?;
            check_art(paths, name, report);
            return Ok(());
        }
    }
    let wine = check_wine(paths, global, &environment, report);
    check_targets(paths, global, &environment, &wine, report);
    check_runtime(paths, name, report)?;
    check_desktop(paths, global, &environment, &wine, entries, report)?;
    check_art(paths, name, report);
    Ok(())
}

fn check_proxied_steam(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    report: &mut Report,
) {
    let Kind::Proxied(proxied) = &environment.config.kind else {
        return;
    };
    let name = environment.name.as_str();
    let active = steam::active_for(paths, global);
    let Some(install) = active.as_ref() else {
        let (code, message) = if global.steam.is_some() {
            (
                Code::SteamInstallMissing,
                "The configured Steam install isn't available.".to_owned(),
            )
        } else {
            (Code::SteamNotFound, "Steam isn't found.".to_owned())
        };
        report.push(Problem::new(code, Some(name), message, None));
        return;
    };
    match steam::app_state(Some(install), &proxied.app_id) {
        // A game Steam doesn't have yet is a normal import now, and `status`
        // already says where an install stands.
        AppState::Installed
        | AppState::Updating
        | AppState::NotInstalled
        | AppState::Installing(_)
        | AppState::InstallQueued
        | AppState::Uninstalling => {}
        AppState::Unavailable(library) => report.push(Problem::new(
            Code::SteamLibraryUnavailable,
            Some(name),
            format!("Steam library at {} is unavailable.", library.display()),
            None,
        )),
        AppState::Unknown(_) => report.push(Problem::new(
            Code::SteamDataUnknown,
            Some(name),
            format!("Steam data for {name} is unreadable or invalid."),
            None,
        )),
    }
}

fn check_wine(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    report: &mut Report,
) -> WineState {
    let Some(wine) = environment.config.wine() else {
        return WineState::default();
    };
    let name = environment.name.as_str();
    let mut push = |code, message| report.push(Problem::new(code, Some(name), message, None));

    let runner = match Runners::new(paths, global).and_then(|runners| runners.resolve(&wine.runner))
    {
        Ok(runner) => Some(runner),
        Err(err) => {
            push(
                Code::RunnerMissing,
                format!("{}\nGet one with: wineo download", chain(&err)),
            );
            None
        }
    };
    let prefix = match environment.prefix(paths) {
        Ok(prefix) => Some(prefix),
        Err(err) => {
            push(Code::PrefixRefused, chain(&err));
            None
        }
    };

    let mut usable = false;
    if let Some(dir) = &prefix {
        match &runner {
            Some(runner) => {
                let plan = global
                    .prefix_root(paths)
                    .and_then(|root| prefix::build_plan(paths, &root, runner, dir, false));
                match plan {
                    Ok(BuildPlan::Ready) => usable = true,
                    Ok(BuildPlan::Build) => push(
                        Code::PrefixMissing,
                        format!(
                            "prefix {} is missing, `wineo start` will build it",
                            dir.display()
                        ),
                    ),
                    Err(err @ (Error::EmptyPrefix(_) | Error::EmptyPrefixParent { .. })) => push(
                        Code::PrefixRefused,
                        format!("{err}. Run `wineo setup {name}` to create it there."),
                    ),
                    Err(err) => push(Code::PrefixRefused, chain(&err)),
                }
            }
            // With the runner unknown, the layout says whether there is one.
            None => usable = prefix::holds_prefix(dir),
        }
    }

    if let Some(runner) = &runner
        && !runner.is_proton()
    {
        let wanted = Wanted::from_config(&environment.config);
        // A prefix that already has the pinned builds doesn't need them
        // extracted any more, since their DLLs were copied in.
        let current = usable
            && prefix
                .as_deref()
                .is_some_and(|dir| install::is_current(runner, dir, &wanted).unwrap_or(false));
        for component in Component::ALL {
            let Some(asked) = wanted.get(component).filter(|_| !current) else {
                continue;
            };
            let Err(err) =
                Builds::new(paths, global, component).and_then(|builds| builds.resolve(asked))
            else {
                continue;
            };
            let fix = match &err {
                Error::NoBuilds { .. } => Some(Fix::Download {
                    component,
                    version: None,
                }),
                Error::UnknownBuild { name, .. } => Some(Fix::Download {
                    component,
                    version: Some(name.clone()),
                }),
                _ => None,
            };
            report.push(Problem::new(
                Code::BuildMissing,
                Some(name),
                chain(&err),
                fix,
            ));
        }
    }

    if !environment.config.winetricks.is_empty() {
        if let Some(runner) = &runner
            && let Err(err) = winetricks::check_available(runner)
        {
            report.push(Problem::new(
                Code::WinetricksUnavailable,
                Some(name),
                err.to_string(),
                None,
            ));
        }
        // A missing prefix is already reported, and `start` builds it before
        // installing, so the verbs wait for that.
        if let (Some(runner), Some(prefix), true) = (&runner, &prefix, usable) {
            match winetricks::missing_for(runner, prefix, &environment.config.winetricks) {
                Ok(missing) if !missing.is_empty() => report.push(Problem::new(
                    Code::WinetricksMissing,
                    Some(name),
                    format!("winetricks verbs missing: {}", missing.join(", ")),
                    Some(Fix::InstallWinetricks {
                        environment: name.to_owned(),
                    }),
                )),
                Err(err) => report.push(Problem::new(
                    Code::WinetricksMissing,
                    Some(name),
                    format!("cannot read winetricks log: {err}"),
                    None,
                )),
                Ok(_) => {}
            }
        }
    }

    WineState {
        runner,
        prefix,
        usable,
    }
}

fn check_targets(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    wine: &WineState,
    report: &mut Report,
) {
    let name = environment.name.as_str();
    let native = match &environment.config.kind {
        Kind::Native => true,
        Kind::Wine(_) => false,
        Kind::Proxied(_) => return,
    };
    let targets = &environment.config.targets;
    let mut push = |code, message| report.push(Problem::new(code, Some(name), message, None));

    for (target_name, target) in targets {
        let exe = &target.exe;
        let not_found = || format!("target {target_name}: {exe} not found");
        if native {
            if let Err(err) =
                launch::check_native_exe(paths, global, name, &environment.config.env, exe)
            {
                push(
                    Code::NativeExeInvalid,
                    format!("target {target_name}: {}", chain(&err)),
                );
            }
        } else if winpath::is_windows_path(exe) {
            // A missing prefix is already one problem, so its paths aren't more.
            if wine.usable
                && let Some(dir) = &wine.prefix
                && desktop::check(paths, dir, exe).is_err()
            {
                push(Code::ExeMissing, not_found());
            }
        } else {
            match launch::check_unix_program(paths, exe, false) {
                Ok(()) => {}
                Err(Error::ExeNotFound(_)) => push(Code::ExeMissing, not_found()),
                Err(err) => push(
                    Code::ExeMissing,
                    format!("target {target_name}: {}", chain(&err)),
                ),
            }
        }

        if let Some(cwd) = &target.cwd
            && (native || !winpath::is_windows_path(cwd))
            && !paths.config_path(cwd).is_ok_and(|dir| dir.is_dir())
        {
            push(
                Code::CwdMissing,
                format!("target {target_name}: working directory {cwd} not found"),
            );
        }

        if launch::gamemode(global, environment, Some(target))
            && runner::find_on_path(launch::GAMEMODE).is_none()
        {
            push(
                Code::GamemodeMissing,
                format!(
                    "target {target_name}: gamemode is on, but {} was not found",
                    launch::GAMEMODE
                ),
            );
        }
        let gamescope = launch::gamescope(environment, Some(target)).is_some();
        if gamescope && runner::find_on_path("gamescope").is_none() {
            push(
                Code::GamescopeMissing,
                format!("target {target_name}: gamescope is on, but gamescope was not found"),
            );
        }
        if launch::mangohud(global, environment, Some(target)) {
            let binary = if gamescope { "mangoapp" } else { "mangohud" };
            if runner::find_on_path(binary).is_none() {
                push(
                    Code::MangohudMissing,
                    format!("target {target_name}: mangohud is on, but {binary} was not found"),
                );
            }
        }
        if launch::wayland(global, environment, Some(target))
            && let Some(runner) = &wine.runner
            && !runner.supports_wayland()
        {
            push(
                Code::WaylandUnsupported,
                format!(
                    "target {target_name}: wayland is on, but {} doesn't support it",
                    runner.name()
                ),
            );
        }
    }

    if targets.len() > 1 && !targets.values().any(|target| target.default) {
        push(
            Code::TargetAmbiguous,
            "several targets and none is the default, so `wineo start` needs --target".to_owned(),
        );
    }
}

fn check_runtime(paths: &Paths, name: &str, report: &mut Report) -> Result<()> {
    let runtime = Runtime::new(paths, name);
    match runtime.status() {
        Ok(status) if status.lost => report.push(Problem::new(
            Code::LostRun,
            Some(name),
            "the last run was lost, so what it started may still be running".to_owned(),
            Some(Fix::CleanUpLostRun {
                environment: name.to_owned(),
            }),
        )),
        Ok(_) => {}
        // `stop` can't tell what an unreadable state owned, so there is no fix.
        Err(err @ Error::StateSyntax { .. }) => {
            report.push(Problem::new(Code::LostRun, Some(name), chain(&err), None));
        }
        Err(err) => return Err(err),
    }
    match runtime.read_operation() {
        Ok(Some(_)) => report.push(Problem::new(
            Code::InstallerUnresolved,
            Some(name),
            "the last installer did not finish cleanly, so what it started may still be running"
                .to_owned(),
            Some(Fix::CleanUpInstaller {
                environment: name.to_owned(),
            }),
        )),
        Ok(None) => {}
        Err(err @ Error::StateSyntax { .. }) => report.push(Problem::new(
            Code::InstallerUnresolved,
            Some(name),
            chain(&err),
            None,
        )),
        Err(err) => return Err(err),
    }
    Ok(())
}

fn check_desktop(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    wine: &WineState,
    entries: &[Entry],
    report: &mut Report,
) -> Result<()> {
    let name = environment.name.as_str();
    if paths.portable_root().is_some() {
        if !desktop::requested_targets(environment).is_empty() {
            report.push(Problem::new(
                Code::DesktopPortable,
                Some(name),
                "desktop = true is set, but portable environments never get desktop entries"
                    .to_owned(),
                None,
            ));
        }
        return Ok(());
    }
    let pending = match (&environment.config.kind, &wine.prefix) {
        (Kind::Native | Kind::Proxied(_), _) => {
            desktop::pending_in(paths, global, environment, None, None, entries)?
        }
        (Kind::Wine(_), Some(dir)) => desktop::pending_in(
            paths,
            global,
            environment,
            wine.runner.as_ref(),
            Some(dir),
            entries,
        )?,
        // No prefix to check against means no entries to compare either.
        (Kind::Wine(_), None) => return Ok(()),
    };
    if pending.is_empty() {
        return Ok(());
    }
    let targets: BTreeSet<&str> = pending
        .iter()
        .map(|pending| match pending {
            Pending::Write { target } | Pending::Update { target } | Pending::Remove { target } => {
                target.as_str()
            }
        })
        .collect();
    report.push(Problem::new(
        Code::DesktopOutdated,
        Some(name),
        format!(
            "desktop entries for {} are out of date with the config",
            targets.into_iter().collect::<Vec<_>>().join(", ")
        ),
        Some(Fix::RewriteDesktop {
            environment: name.to_owned(),
        }),
    ));
    Ok(())
}

fn check_art(paths: &Paths, name: &str, report: &mut Report) {
    let listing = match art::list(paths, name) {
        Ok(listing) => listing,
        Err(err) => {
            report.push(Problem::new(
                Code::ArtConfigInvalid,
                Some(name),
                chain(&err),
                None,
            ));
            return;
        }
    };
    let spot = |target: &Option<String>| match target {
        Some(target) => format!("art for {target}"),
        None => "art".to_owned(),
    };
    let file = |path: &Path| {
        path.file_name().map_or_else(
            || path.display().to_string(),
            |file| file.to_string_lossy().into_owned(),
        )
    };
    for warning in listing.warnings {
        let problem = match warning {
            art::Warning::MissingFile { target, kind, path } => Problem::new(
                Code::ArtFileMissing,
                Some(name),
                format!("{}: {} is recorded but missing", spot(&target), file(&path)),
                Some(Fix::DropArtRecord {
                    environment: name.to_owned(),
                    target,
                    kind,
                }),
            ),
            art::Warning::UnknownTarget(target) => Problem::new(
                Code::ArtTargetUnknown,
                Some(name),
                format!("art: records target {target}, which no longer exists"),
                Some(Fix::DropArtTarget {
                    environment: name.to_owned(),
                    target,
                }),
            ),
            art::Warning::Duplicates {
                target,
                used,
                ignored,
                ..
            } => {
                let ignored: Vec<String> = ignored.iter().map(|path| file(path)).collect();
                Problem::new(
                    Code::ArtDuplicates,
                    Some(name),
                    format!(
                        "{}: {} is used, so {} {} ignored",
                        spot(&target),
                        file(&used),
                        ignored.join(", "),
                        if ignored.len() == 1 { "is" } else { "are" }
                    ),
                    None,
                )
            }
        };
        report.push(problem);
    }
}

/// State dirs, desktop entries, and art dirs left by environments that no
/// longer exist.
fn check_strays(
    paths: &Paths,
    names: &[String],
    entries: &[Entry],
    report: &mut Report,
) -> Result<()> {
    let known: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    // Import logs lived here before they moved to a dot directory, so it
    // holds logs rather than a removed environment's state.
    let legacy_imports = paths.runtime_dir("imports");

    for (dir, stray) in subdirs(&parent(&paths.runtime_dir("x")))? {
        if known.contains(stray.as_str()) || dir == legacy_imports {
            continue;
        }
        let fix = name::is_valid(&stray).then(|| Fix::RemoveStateDir {
            environment: stray.clone(),
        });
        report.push(Problem::stray(
            Code::StateDirStray,
            &stray,
            format!("state dir {} has no environment", dir.display()),
            fix,
        ));
    }

    if desktop::supported(paths).is_ok() {
        for entry in entries {
            if known.contains(entry.environment.as_str()) {
                continue;
            }
            let file = entry.path.file_name().map_or_else(
                || entry.path.display().to_string(),
                |file| file.to_string_lossy().into_owned(),
            );
            let fix = (name::is_valid(&entry.environment) && name::is_valid(&entry.target))
                .then(|| Fix::RemoveDesktopEntry(entry.clone()));
            report.push(Problem::stray(
                Code::DesktopEntryStray,
                &entry.environment,
                format!("desktop entry {file} has no environment"),
                fix,
            ));
        }
    }

    for (dir, stray) in subdirs(&parent(&paths.art_dir("x")))? {
        if known.contains(stray.as_str()) {
            continue;
        }
        report.push(Problem::stray(
            Code::ArtDirStray,
            &stray,
            format!("art dir {} has no environment", dir.display()),
            None,
        ));
    }

    for (file, stray) in files_with_extension(&parent(&paths.stats_log("x")), "jsonl")? {
        if known.contains(stray.as_str()) {
            continue;
        }
        let fix = name::is_valid(&stray).then(|| Fix::RemoveStatsLog {
            environment: stray.clone(),
        });
        report.push(Problem::stray(
            Code::StatsStray,
            &stray,
            format!("play stats {} has no environment", file.display()),
            fix,
        ));
    }
    Ok(())
}

fn parent(path: &Path) -> PathBuf {
    path.parent()
        .expect("a per-environment dir always has a parent")
        .to_owned()
}

/// Directories in `dir` with their names, leaving out dotted ones, which are
/// wineo's own bookkeeping and never an environment's.
fn subdirs(dir: &Path) -> Result<Vec<(PathBuf, String)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).context(|| format!("failed to read {}", dir.display())),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("failed to read {}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        found.push((entry.path(), name));
    }
    found.sort();
    Ok(found)
}

/// Files directly in `dir` ending in `extension`, with the name they'd belong
/// to (the file name without it), leaving out dotted ones.
fn files_with_extension(dir: &Path, extension: &str) -> Result<Vec<(PathBuf, String)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).context(|| format!("failed to read {}", dir.display())),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("failed to read {}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let Some(stem) = name
            .strip_suffix(&format!(".{extension}"))
            .map(str::to_owned)
        else {
            continue;
        };
        found.push((path, stem));
    }
    found.sort();
    Ok(found)
}

/// Applies an explicitly chosen single fix, including non-cancellable snapshot recovery.
/// Batch callers must use `Report::fixes` and `apply` instead.
pub fn apply_single(
    paths: &Paths,
    fix: &Fix,
    token: &crate::cancel::Token,
    winetricks_output: winetricks::EnsureOutput<'_>,
    winetricks_events: impl FnMut(winetricks::WinetricksEvent),
    download: impl FnOnce(Component, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<Vec<Note>> {
    if let Fix::RecoverSnapshot { environment } = fix {
        snapshot::recover(paths, environment)?;
        return Ok(Vec::new());
    }
    apply(
        paths,
        fix,
        token,
        winetricks_output,
        winetricks_events,
        download,
    )
}

/// Makes one batch-safe fix, taking the same locks its command would.
/// Snapshot recovery is refused here; use `apply_single` for explicit rollback.
/// Callers apply each fix on its own, so one failing doesn't stop the rest.
///
/// `download` fetches a DXVK or vkd3d-proton build: the pinned version, or
/// the newest release for None. Downloading shows progress and can be
/// canceled, which is the front end's to do.
pub fn apply(
    paths: &Paths,
    fix: &Fix,
    token: &crate::cancel::Token,
    winetricks_output: winetricks::EnsureOutput<'_>,
    winetricks_events: impl FnMut(winetricks::WinetricksEvent),
    download: impl FnOnce(Component, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<Vec<Note>> {
    match fix {
        Fix::InstallWinetricks { environment } => install_winetricks(
            paths,
            environment,
            token,
            winetricks_output,
            winetricks_events,
        )?,
        Fix::Download { component, version } => {
            // The report that offered this fix already carries
            // `global_config_invalid` for a file that fell back to defaults.
            let global = GlobalConfig::load_or_default(paths)?.config;
            let asked = version.as_deref().unwrap_or(LATEST);
            // Another environment's fix may have fetched it already.
            if Builds::new(paths, &global, *component)?
                .resolve(asked)
                .is_ok()
            {
                return Ok(Vec::new());
            }
            download(*component, version.as_deref())?;
        }
        Fix::CleanUpLostRun { environment } => {
            return clean_up_lost_run(paths, environment).map_err(Into::into);
        }
        Fix::CleanUpInstaller { environment } => clean_up_installer(paths, environment)?,
        Fix::RemoveStateDir { environment } => remove_state_dir(paths, environment)?,
        Fix::RemoveDesktopEntry(entry) => remove_desktop_entry(paths, entry)?,
        Fix::RewriteDesktop { environment } => rewrite_desktop(paths, environment)?,
        Fix::DropArtRecord {
            environment,
            target,
            kind,
        } => {
            art::drop_missing_record(paths, environment, target.as_deref(), *kind)?;
        }
        Fix::DropArtTarget {
            environment,
            target,
        } => {
            art::drop_unknown_target(paths, environment, target)?;
        }
        Fix::RemoveStatsLog { environment } => remove_stats_log(paths, environment)?,
        Fix::RecoverTrash { id } => {
            return Ok(trash::recover_noting(paths, id)?
                .into_iter()
                .map(Note::Warning)
                .collect());
        }
        Fix::RecoverSnapshot { .. } => {
            anyhow::bail!("snapshot recovery must be applied by itself with apply_single");
        }
        Fix::ForgetGogFolder { folder } => {
            let cancel = || token.is_canceled();
            if !folder::remove_if_deleted(paths, folder, &folder::device_of, &cancel)? {
                anyhow::bail!("{} changed. Run wineo validate again.", folder.display());
            }
        }
        Fix::ReleaseGogFolder { folder, .. } => {
            let cancel = || token.is_canceled();
            if !folder::release_missing_owner(paths, folder, &cancel)? {
                anyhow::bail!("{} changed. Run wineo validate again.", folder.display());
            }
        }
    }
    Ok(Vec::new())
}

fn install_winetricks(
    paths: &Paths,
    name: &str,
    token: &crate::cancel::Token,
    output: winetricks::EnsureOutput<'_>,
    mut events: impl FnMut(winetricks::WinetricksEvent),
) -> anyhow::Result<()> {
    winetricks::ensure(paths, name, &Default::default(), token, output, |event| {
        if let winetricks::EnsureEvent::Winetricks(event) = event {
            events(event);
        }
    })
}

/// What `wineo stop` does for a lost run, which is all there is to stop.
fn clean_up_lost_run(paths: &Paths, name: &str) -> Result<Vec<Note>> {
    Environment::ensure_exists(paths, name)?;
    if let Some(operation) = launch::busy_with(paths, name)? {
        return Err(Error::Busy {
            subject: format!("environment `{name}`"),
            holder: Some(operation),
            holder_label: operation.describe(),
        });
    }
    let (stop_timeout, _) = launch::stop_timeout(paths);
    let timeout = Duration::from_secs(u64::from(stop_timeout) + 10);
    match launch::clean_up_lost_run(paths, name, timeout)? {
        launch::LostRunOutcome::Unresolved { warning, message } => {
            Err(Error::Unsettled(match warning {
                Some(warning) => format!("{warning}. {message}"),
                None => message,
            }))
        }
        launch::LostRunOutcome::NotLost => Ok(Vec::new()),
        launch::LostRunOutcome::Gone { warning } | launch::LostRunOutcome::Killed { warning } => {
            Ok(warning.into_iter().map(Note::Warning).collect())
        }
    }
}

fn clean_up_installer(paths: &Paths, name: &str) -> Result<()> {
    Environment::ensure_exists(paths, name)?;
    // As for `Fix::Download`, the report already said a fallback happened.
    match launch::clean_up_installer(paths, name)?.0 {
        Some(Cleaned::Untracked | Cleaned::Unresolved) => {
            Err(Error::Unsettled(installer::unresolved_message(paths, name)))
        }
        Some(Cleaned::Stopped | Cleaned::Gone) | None => Ok(()),
    }
}

/// Holds the lock `rm` would, so the name can't be reused halfway through.
fn remove_state_dir(paths: &Paths, name: &str) -> Result<()> {
    if !name::is_valid(name) {
        return Err(Error::InvalidName(name.to_owned()));
    }
    let _guard = Locks::new(paths).acquire(Operation::Remove, name, &[])?;
    if paths.environment_config(name).exists() {
        return Err(Error::EnvironmentExists(name.to_owned()));
    }
    let runtime = Runtime::new(paths, name);
    // What a run or installer left running is only tracked in here.
    runtime.check_quiet(name)?;
    let dir = runtime.dir();
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).context(|| format!("failed to remove {}", dir.display())),
    }
}

fn remove_stats_log(paths: &Paths, name: &str) -> Result<()> {
    if !name::is_valid(name) {
        return Err(Error::InvalidName(name.to_owned()));
    }
    let _guard = Locks::new(paths).acquire(Operation::Remove, name, &[])?;
    if paths.environment_config(name).exists() {
        return Err(Error::EnvironmentExists(name.to_owned()));
    }
    let path = paths.stats_log(name);
    files::remove_if_present(&path)
        .map(drop)
        .context(|| format!("failed to remove {}", path.display()))
}

fn remove_desktop_entry(paths: &Paths, entry: &Entry) -> Result<()> {
    if !name::is_valid(&entry.environment) {
        return Err(Error::InvalidName(entry.environment.clone()));
    }
    let _guard = Locks::new(paths).acquire(Operation::Remove, &entry.environment, &[])?;
    if paths.environment_config(&entry.environment).exists() {
        return Err(Error::EnvironmentExists(entry.environment.clone()));
    }
    desktop::remove_entry(paths, entry)
}

/// What `start` does to the entries, under the lock it takes.
fn rewrite_desktop(paths: &Paths, name: &str) -> Result<()> {
    let environment = Environment::load(paths, name)?;
    // As for `Fix::Download`, the report already said a fallback happened.
    let global = GlobalConfig::load_or_default(paths)?.config;
    let locks = Locks::new(paths);
    let wine = match &environment.config.kind {
        Kind::Wine(wine) => wine,
        Kind::Native => {
            let _guard = locks.acquire(Operation::Launch, name, &[])?;
            Environment::ensure_exists(paths, name)?;
            desktop::reconcile_native(paths, &global, &environment)?;
            return Ok(());
        }
        Kind::Proxied(_) => {
            let _guard = locks.acquire(Operation::Launch, name, &[])?;
            Environment::ensure_exists(paths, name)?;
            desktop::reconcile_proxied(paths, &global, &environment)?;
            return Ok(());
        }
    };
    let prefix = environment.prefix(paths)?;
    let _guard = locks.acquire(Operation::Launch, name, &[&prefix])?;
    Environment::ensure_exists(paths, name)?;
    // Pulling an icon out runs wine in the prefix, which is left alone while
    // a lost run or installer may still be in it. The entries are still
    // written, and get their icons on a later launch.
    let runner = Runtime::new(paths, name)
        .check_resolved(name)
        .is_ok()
        .then(|| {
            Runners::new(paths, &global)
                .ok()?
                .resolve(&wine.runner)
                .ok()
        })
        .flatten();
    desktop::reconcile(paths, &global, &environment, runner.as_ref(), &prefix)?;
    Ok(())
}

#[cfg(test)]
mod tests;
