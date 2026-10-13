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

/// Which check found a problem. Fixed, so a script or the GUI can match on it
/// without parsing messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    GlobalConfigInvalid,
    TemplateInvalid,
    TemplateUnresolved,
    EnvironmentConfigInvalid,
    RunnerMissing,
    BuildMissing,
    WinetricksMissing,
    WinetricksUnavailable,
    PrefixMissing,
    PrefixRefused,
    ExeMissing,
    CwdMissing,
    GamemodeMissing,
    GamescopeMissing,
    MangohudMissing,
    WaylandUnsupported,
    NativeExeInvalid,
    TargetAmbiguous,
    LostRun,
    InstallerUnresolved,
    RestoreInterrupted,
    StateDirStray,
    DesktopEntryStray,
    DesktopOutdated,
    DesktopPortable,
    SteamNotFound,
    SteamInstallMissing,
    SteamLibraryUnavailable,
    SteamDataUnknown,
    ArtConfigInvalid,
    ArtFileMissing,
    ArtTargetUnknown,
    ArtDuplicates,
    ArtDirStray,
    StatsStray,
    TrashInterrupted,
    TrashInvalid,
    GogStateInvalid,
    GogStateMissing,
    GogFolderMissing,
    GogFolderTaken,
    GogMidUpdate,
    GogOwnerMissing,
}

impl Code {
    pub const ALL: [Code; 43] = [
        Code::GlobalConfigInvalid,
        Code::TemplateInvalid,
        Code::TemplateUnresolved,
        Code::EnvironmentConfigInvalid,
        Code::RunnerMissing,
        Code::BuildMissing,
        Code::WinetricksMissing,
        Code::WinetricksUnavailable,
        Code::PrefixMissing,
        Code::PrefixRefused,
        Code::ExeMissing,
        Code::CwdMissing,
        Code::GamemodeMissing,
        Code::GamescopeMissing,
        Code::MangohudMissing,
        Code::WaylandUnsupported,
        Code::NativeExeInvalid,
        Code::TargetAmbiguous,
        Code::LostRun,
        Code::InstallerUnresolved,
        Code::RestoreInterrupted,
        Code::StateDirStray,
        Code::DesktopEntryStray,
        Code::DesktopOutdated,
        Code::DesktopPortable,
        Code::SteamNotFound,
        Code::SteamInstallMissing,
        Code::SteamLibraryUnavailable,
        Code::SteamDataUnknown,
        Code::ArtConfigInvalid,
        Code::ArtFileMissing,
        Code::ArtTargetUnknown,
        Code::ArtDuplicates,
        Code::ArtDirStray,
        Code::StatsStray,
        Code::TrashInterrupted,
        Code::TrashInvalid,
        Code::GogStateInvalid,
        Code::GogStateMissing,
        Code::GogFolderMissing,
        Code::GogFolderTaken,
        Code::GogMidUpdate,
        Code::GogOwnerMissing,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Code::GlobalConfigInvalid => "global_config_invalid",
            Code::TemplateInvalid => "template_invalid",
            Code::TemplateUnresolved => "template_unresolved",
            Code::EnvironmentConfigInvalid => "environment_config_invalid",
            Code::RunnerMissing => "runner_missing",
            Code::BuildMissing => "build_missing",
            Code::WinetricksMissing => "winetricks_missing",
            Code::WinetricksUnavailable => "winetricks_unavailable",
            Code::PrefixMissing => "prefix_missing",
            Code::PrefixRefused => "prefix_refused",
            Code::ExeMissing => "exe_missing",
            Code::CwdMissing => "cwd_missing",
            Code::GamemodeMissing => "gamemode_missing",
            Code::GamescopeMissing => "gamescope_missing",
            Code::MangohudMissing => "mangohud_missing",
            Code::WaylandUnsupported => "wayland_unsupported",
            Code::NativeExeInvalid => "native_exe_invalid",
            Code::TargetAmbiguous => "target_ambiguous",
            Code::LostRun => "lost_run",
            Code::InstallerUnresolved => "installer_unresolved",
            Code::RestoreInterrupted => "restore_interrupted",
            Code::StateDirStray => "state_dir_stray",
            Code::DesktopEntryStray => "desktop_entry_stray",
            Code::DesktopOutdated => "desktop_outdated",
            Code::DesktopPortable => "desktop_portable",
            Code::SteamNotFound => "steam_not_found",
            Code::SteamInstallMissing => "steam_install_missing",
            Code::SteamLibraryUnavailable => "steam_library_unavailable",
            Code::SteamDataUnknown => "steam_data_unknown",
            Code::ArtConfigInvalid => "art_config_invalid",
            Code::ArtFileMissing => "art_file_missing",
            Code::ArtTargetUnknown => "art_target_unknown",
            Code::ArtDuplicates => "art_duplicates",
            Code::ArtDirStray => "art_dir_stray",
            Code::StatsStray => "stats_stray",
            Code::TrashInterrupted => "trash_interrupted",
            Code::TrashInvalid => "trash_invalid",
            Code::GogStateInvalid => "gog_state_invalid",
            Code::GogStateMissing => "gog_state_missing",
            Code::GogFolderMissing => "gog_folder_missing",
            Code::GogFolderTaken => "gog_folder_taken",
            Code::GogMidUpdate => "gog_mid_update",
            Code::GogOwnerMissing => "gog_owner_missing",
        }
    }

    /// Every check has one level, so it is decided here rather than per problem.
    pub fn level(self) -> Level {
        match self {
            Code::GlobalConfigInvalid
            | Code::TemplateInvalid
            | Code::EnvironmentConfigInvalid
            | Code::RunnerMissing
            | Code::BuildMissing
            | Code::WinetricksUnavailable
            | Code::PrefixRefused
            | Code::ExeMissing
            | Code::CwdMissing
            | Code::GamemodeMissing
            | Code::GamescopeMissing
            | Code::MangohudMissing
            | Code::WaylandUnsupported
            | Code::NativeExeInvalid
            | Code::SteamNotFound
            | Code::SteamInstallMissing
            | Code::SteamDataUnknown
            | Code::RestoreInterrupted
            | Code::ArtConfigInvalid => Level::Error,
            Code::TrashInterrupted | Code::TrashInvalid => Level::Error,
            Code::GogStateInvalid
            | Code::GogStateMissing
            | Code::GogFolderTaken
            | Code::GogMidUpdate => Level::Error,
            Code::GogFolderMissing | Code::GogOwnerMissing => Level::Warning,
            Code::TemplateUnresolved
            | Code::PrefixMissing
            | Code::WinetricksMissing
            | Code::TargetAmbiguous
            | Code::LostRun
            | Code::InstallerUnresolved
            | Code::StateDirStray
            | Code::DesktopEntryStray
            | Code::DesktopOutdated
            | Code::DesktopPortable
            | Code::ArtFileMissing
            | Code::ArtTargetUnknown
            | Code::ArtDuplicates
            | Code::ArtDirStray
            | Code::StatsStray
            | Code::SteamLibraryUnavailable => Level::Warning,
        }
    }

    pub fn area(self) -> Area {
        match self {
            Code::GlobalConfigInvalid => Area::GlobalConfig,
            Code::TemplateInvalid | Code::TemplateUnresolved => Area::Template,
            Code::EnvironmentConfigInvalid => Area::EnvironmentConfig,
            Code::RunnerMissing
            | Code::BuildMissing
            | Code::WinetricksMissing
            | Code::WinetricksUnavailable
            | Code::PrefixMissing
            | Code::PrefixRefused => Area::Wine,
            Code::ExeMissing
            | Code::CwdMissing
            | Code::GamemodeMissing
            | Code::GamescopeMissing
            | Code::MangohudMissing
            | Code::WaylandUnsupported => Area::Target,
            Code::NativeExeInvalid => Area::NativeTarget,
            Code::TargetAmbiguous => Area::Environment,
            Code::LostRun
            | Code::InstallerUnresolved
            | Code::RestoreInterrupted
            | Code::StateDirStray
            | Code::StatsStray => Area::Runtime,
            Code::DesktopEntryStray | Code::DesktopOutdated | Code::DesktopPortable => {
                Area::Desktop
            }
            Code::SteamNotFound
            | Code::SteamInstallMissing
            | Code::SteamLibraryUnavailable
            | Code::SteamDataUnknown => Area::Steam,
            Code::ArtConfigInvalid
            | Code::ArtFileMissing
            | Code::ArtTargetUnknown
            | Code::ArtDuplicates
            | Code::ArtDirStray => Area::Art,
            Code::TrashInterrupted | Code::TrashInvalid => Area::Trash,
            Code::GogStateInvalid
            | Code::GogStateMissing
            | Code::GogFolderMissing
            | Code::GogFolderTaken
            | Code::GogMidUpdate
            | Code::GogOwnerMissing => Area::Gog,
        }
    }
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
    for name in &names {
        if !pending.contains(name) {
            check_one(paths, &global, name, &mut report)?;
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
        check_strays(paths, &protected, &mut report)?;
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
    // A broken global config is only reported by `check_all`. Here it means
    // defaults, as it does for most commands.
    let global = GlobalConfig::load(paths).unwrap_or_default();
    check_one(paths, &global, name, &mut report)?;
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
            Some(owner) => {
                format!("{owner}'s game folder is mid-update. Finish it with wineo update {owner}.")
            }
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

fn check_one(paths: &Paths, global: &GlobalConfig, name: &str, report: &mut Report) -> Result<()> {
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
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        // Removed since it was listed.
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
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
            check_desktop(paths, global, &environment, &WineState::default(), report)?;
            check_art(paths, name, report);
            return Ok(());
        }
    }
    let wine = check_wine(paths, global, &environment, report);
    check_targets(paths, global, &environment, &wine, report);
    check_runtime(paths, name, report)?;
    check_desktop(paths, global, &environment, &wine, report)?;
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
        (Kind::Native, _) => desktop::pending_native(paths, global, environment)?,
        (Kind::Proxied(_), _) => desktop::pending_proxied(paths, global, environment)?,
        (Kind::Wine(_), Some(dir)) => {
            desktop::pending(paths, global, environment, wine.runner.as_ref(), dir)?
        }
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
fn check_strays(paths: &Paths, names: &[String], report: &mut Report) -> Result<()> {
    let known: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    // Named after an environment, but not one.
    let imports = paths.import_log("x");
    let imports = imports.parent();

    for (dir, stray) in subdirs(&parent(&paths.runtime_dir("x")))? {
        if known.contains(stray.as_str()) || Some(dir.as_path()) == imports {
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
        for entry in desktop::list(paths)? {
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
            let global = GlobalConfig::load_or_default(paths)?;
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
    match launch::clean_up_installer(paths, name)? {
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
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).context(|| format!("failed to remove {}", path.display())),
    }
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
    let global = GlobalConfig::load_or_default(paths)?;
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
mod tests {
    use crate::test_script::write_script;
    use std::ffi::OsString;
    use std::os::unix::fs::symlink;

    use jiff::Timestamp;

    use super::*;
    use crate::runtime::{OperationState, RunMode, RunState, RunStatus};

    fn apply(
        paths: &Paths,
        fix: &Fix,
        download: impl FnOnce(Component, Option<&str>) -> anyhow::Result<()>,
    ) -> anyhow::Result<Vec<Note>> {
        super::apply(
            paths,
            fix,
            &crate::cancel::Token::new(),
            winetricks::EnsureOutput::Stderr,
            |_| {},
            download,
        )
    }

    #[test]
    fn report_fixes_deduplicates_shared_downloads_in_problem_order() {
        let download = Fix::Download {
            component: Component::Dxvk,
            version: Some("2.7".to_owned()),
        };
        let cleanup = Fix::RemoveStateDir {
            environment: "oldgame".to_owned(),
        };
        let other = Fix::Download {
            component: Component::Dxvk,
            version: Some("2.6".to_owned()),
        };
        let report = Report {
            problems: [
                Some(download.clone()),
                None,
                Some(cleanup.clone()),
                Some(download.clone()),
                Some(other.clone()),
                Some(cleanup.clone()),
            ]
            .into_iter()
            .map(|fix| Problem::new(Code::BuildMissing, None, "problem".to_owned(), fix))
            .collect(),
            ..Report::default()
        };
        assert_eq!(report.fixes(), [download, cleanup, other]);
        assert!(Report::default().fixes().is_empty());
    }

    #[test]
    fn a_winetricks_fix_with_log_output_reports_progress() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        executable(
            &bin.join("winetricks"),
            "#!/bin/sh\nfor verb in \"$@\"; do\n  case \"$verb\" in -q) ;; *) echo \"Executing w_do_call $verb\" >&2 ;; esac\ndone\n",
        );
        let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "validate::tests::winetricks_fix_log_child",
                "--nocapture",
            ])
            .env("WINEO_VALIDATE_LOG_TEST", "1")
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn winetricks_fix_log_child() {
        if std::env::var_os("WINEO_VALIDATE_LOG_TEST").is_none() {
            return;
        }
        let f = Fixture::new();
        f.wine("game", "winetricks = ['corefonts', 'gdiplus']\n");
        f.global("");
        let log = f.root.join("install.log");
        fs::write(&log, "stale output\n").unwrap();
        let mut started = 0;
        let mut steps = Vec::new();
        let mut lines = Vec::new();
        super::apply(
            &f.paths,
            &Fix::InstallWinetricks {
                environment: "game".to_owned(),
            },
            &crate::cancel::Token::new(),
            winetricks::EnsureOutput::Log(&log),
            |event| match event {
                winetricks::WinetricksEvent::LogStarted => {
                    started += 1;
                    assert!(!fs::read_to_string(&log).unwrap().contains("stale output"));
                }
                winetricks::WinetricksEvent::Progress(step) => {
                    assert_eq!(started, 1);
                    steps.push(step);
                }
                winetricks::WinetricksEvent::OutputLine(line) => {
                    assert_eq!(started, 1);
                    lines.push(line);
                }
                _ => {}
            },
            no_download,
        )
        .unwrap();
        assert_eq!(started, 1);
        assert_eq!(
            steps,
            [
                winetricks::Step {
                    verb: "corefonts".to_owned(),
                    position: 1,
                    total: 2
                },
                winetricks::Step {
                    verb: "gdiplus".to_owned(),
                    position: 2,
                    total: 2
                },
            ]
        );
        assert_eq!(
            lines,
            [
                "Executing w_do_call corefonts",
                "Executing w_do_call gdiplus"
            ]
        );
        assert_eq!(
            fs::read_to_string(log).unwrap(),
            "Executing w_do_call corefonts\nExecuting w_do_call gdiplus\n"
        );
    }

    const HEALTHY: &str = "[targets.game]\nexe = 'C:\\Game\\game.exe'\n";

    struct Fixture {
        _root: tempfile::TempDir,
        root: PathBuf,
        paths: Paths,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let paths = Self::xdg(&root, None);
            let fixture = Self {
                _root: dir,
                root,
                paths,
            };
            fixture.runner("fake");
            fixture
        }

        fn xdg(root: &Path, portable: Option<&Path>) -> Paths {
            let root = root.to_owned();
            Paths::select(
                move |key| {
                    let path = match key {
                        "HOME" => root.clone(),
                        "XDG_CONFIG_HOME" => root.join("config"),
                        "XDG_DATA_HOME" => root.join("data"),
                        "XDG_STATE_HOME" => root.join("state"),
                        _ => return None,
                    };
                    Some(OsString::from(path.as_os_str()))
                },
                portable,
            )
            .unwrap()
        }

        /// A wine build of stub scripts, enough to resolve without wine.
        fn runner(&self, name: &str) {
            let bin = self.paths.default_runner_root().join(name).join("bin");
            fs::create_dir_all(&bin).unwrap();
            for program in ["wine", "wineserver"] {
                executable(&bin.join(program), "#!/bin/sh\nexit 0\n");
            }
        }

        fn prefix_dir(&self, name: &str) -> PathBuf {
            self.paths.default_prefix_root().join(name)
        }

        /// A wine prefix holding `C:\Game\game.exe`.
        fn prefix(&self, name: &str) -> PathBuf {
            let prefix = self.prefix_dir(name);
            fs::create_dir_all(prefix.join("drive_c/Game")).unwrap();
            fs::create_dir_all(prefix.join("dosdevices")).unwrap();
            fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
            let _ = symlink("../drive_c", prefix.join("dosdevices/c:"));
            fs::write(prefix.join("drive_c/Game/game.exe"), b"").unwrap();
            prefix
        }

        fn write_config(&self, name: &str, text: &str) {
            let path = self.paths.environment_config(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// A Wine environment on the fake runner, its prefix built.
        fn wine(&self, name: &str, rest: &str) {
            self.prefix(name);
            self.wine_at(name, &self.prefix_dir(name), rest);
        }

        fn wine_at(&self, name: &str, prefix: &Path, rest: &str) {
            self.write_config(
                name,
                &format!(
                    "prefix = '{}'\nrunner = 'fake'\narch = 'win64'\n{rest}",
                    prefix.display()
                ),
            );
        }

        fn native(&self, name: &str, targets: &str) {
            self.write_config(name, &format!("kind = 'native'\n{targets}"));
        }

        fn proxied(&self, name: &str, app_id: &str, desktop: bool) {
            self.write_config(
                name,
                &format!(
                    "kind = 'proxied'\nlauncher = 'steam'\napp_id = '{app_id}'\ndesktop = {desktop}\n"
                ),
            );
        }

        fn steam_root(&self) -> PathBuf {
            let root = self.root.join(".local/share/Steam");
            fs::create_dir_all(root.join("steamapps")).unwrap();
            root
        }

        fn steam_manifest(&self, app_id: &str, text: &str) {
            let path = self
                .steam_root()
                .join("steamapps")
                .join(format!("appmanifest_{app_id}.acf"));
            fs::write(path, text).unwrap();
        }

        fn global(&self, text: &str) {
            let path = self.paths.global_config();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn template(&self, name: &str, text: &str) {
            let dir = self.paths.templates_dir();
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(format!("{name}.toml")), text).unwrap();
        }

        fn check(&self) -> Report {
            check_all(&self.paths).unwrap()
        }

        fn codes(&self) -> Vec<Code> {
            codes(&self.check())
        }

        fn only(&self, code: Code) -> Problem {
            let report = self.check();
            let found: Vec<&Problem> = report.problems.iter().filter(|p| p.code == code).collect();
            assert_eq!(found.len(), 1, "{:#?}", report.problems);
            found[0].clone()
        }

        /// A run whose owner died, in a group that is gone.
        fn lost_run(&self, name: &str) {
            let mut run = RunState::running(
                RunMode::Detached,
                "game".to_owned(),
                4242,
                Some(4242),
                &Default::default(),
            );
            run.native = true;
            run.boot_id = Some("an-earlier-boot".to_owned());
            run.leader_start = Some(1);
            Runtime::new(&self.paths, name).write_state(&run).unwrap();
        }

        fn unresolved_installer(&self, name: &str, group: Option<u32>) {
            let runner = Runners::at(self.paths.default_runner_root())
                .resolve("fake")
                .unwrap();
            let state = OperationState {
                operation: Operation::Install,
                program: "setup.exe".to_owned(),
                runner,
                prefix: self.prefix_dir(name),
                pid: group,
                group,
                boot_id: Some("an-earlier-boot".to_owned()),
                leader_start: Some(1),
                started_at: Timestamp::now(),
            };
            Runtime::new(&self.paths, name)
                .write_operation(&state)
                .unwrap();
        }

        fn art_dir(&self, name: &str) -> PathBuf {
            self.paths.art_dir(name)
        }

        fn stats_log(&self, name: &str) -> PathBuf {
            self.paths.stats_log(name)
        }

        fn stray_entry(&self, environment: &str, target: &str) -> (PathBuf, PathBuf) {
            let entry = self
                .paths
                .applications_dir()
                .join(format!("wineo-{environment}-{target}.desktop"));
            fs::create_dir_all(entry.parent().unwrap()).unwrap();
            fs::write(
                &entry,
                format!(
                    "[Desktop Entry]\nName={target}\nX-Wineo-Environment={environment}\nX-Wineo-Target={target}\n"
                ),
            )
            .unwrap();
            let icon = self.paths.icons_dir().join(format!(
                "hicolor/256x256/apps/wineo-{environment}-{target}.png"
            ));
            fs::create_dir_all(icon.parent().unwrap()).unwrap();
            fs::write(&icon, b"png").unwrap();
            (entry, icon)
        }

        /// Every config file a user writes, with its bytes.
        fn user_configs(&self) -> Vec<(PathBuf, Vec<u8>)> {
            let mut files = Vec::new();
            for dir in [self.paths.environments_dir(), self.paths.templates_dir()] {
                for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                    files.push((entry.path(), fs::read(entry.path()).unwrap()));
                }
            }
            if let Ok(bytes) = fs::read(self.paths.global_config()) {
                files.push((self.paths.global_config(), bytes));
            }
            files.sort();
            files
        }
    }

    fn executable(path: &Path, content: &str) {
        write_script(path, content);
    }

    fn codes(report: &Report) -> Vec<Code> {
        report.problems.iter().map(|problem| problem.code).collect()
    }

    fn no_download(_: Component, _: Option<&str>) -> anyhow::Result<()> {
        panic!("nothing should be downloaded")
    }

    fn fix(problem: &Problem) -> &Fix {
        problem.fix.as_ref().expect("a fix is offered")
    }

    #[test]
    fn a_healthy_setup_has_no_problems() {
        let f = Fixture::new();
        f.wine("game", HEALTHY);
        let tool = f.root.join("tool");
        executable(&tool, "#!/bin/sh\n");
        f.native(
            "tool",
            &format!("[targets.tool]\nexe = '{}'\n", tool.display()),
        );
        f.template("mine", "runner = 'fake'\ngamemode = false\n");

        let report = f.check();
        assert_eq!(report.problems, []);
        assert_eq!(report.environments, ["game", "tool"]);
        assert_eq!(report.templates, ["mine"]);
        assert_eq!((report.errors(), report.warnings()), (0, 0));
    }

    #[test]
    fn a_bad_gog_language_is_a_global_config_error() {
        let f = Fixture::new();
        f.global("[gog]\nlanguage = \"english\"\n");
        assert_eq!(f.codes(), [Code::GlobalConfigInvalid]);
        f.global("[defaults]\ngames_root = \"~/Library\"\n[gog]\nlanguage = \"de-AT\"\n");
        assert_eq!(f.codes(), []);
    }

    #[test]
    fn a_broken_global_config_is_an_error_and_the_rest_is_still_checked() {
        let f = Fixture::new();
        f.wine("game", "[targets.game]\nexe = 'C:\\Game\\missing.exe'\n");
        f.global("stop_timeuot = 3\n");
        assert_eq!(f.codes(), [Code::GlobalConfigInvalid, Code::ExeMissing]);
        let problem = f.only(Code::GlobalConfigInvalid);
        assert!(
            problem
                .message
                .starts_with("invalid config ~/config/wineo/config.toml: "),
            "{}",
            problem.message
        );
        assert_eq!(problem.environment, None);
        assert_eq!(problem.level, Level::Error);
        assert!(
            problem.message.contains("stop_timeuot"),
            "{}",
            problem.message
        );

        f.global("schema_version = 99\n");
        let problem = f.only(Code::GlobalConfigInvalid);
        assert!(problem.message.contains("unsupported schema_version 99"));
    }

    #[test]
    fn templates_are_parsed_and_their_picks_resolved() {
        let f = Fixture::new();
        f.template("broken", "prefix = '/x'\n");
        f.template("typo", "runer = 'fake'\n");
        f.template("gone", "runner = 'nope'\n");
        f.template("nodxvk", "runner = 'fake'\ndxvk = 'latest'\n");
        let report = f.check();
        assert_eq!(
            codes(&report),
            [
                Code::TemplateInvalid,
                Code::TemplateUnresolved,
                Code::TemplateUnresolved,
                Code::TemplateInvalid,
            ]
        );
        assert_eq!(report.errors(), 2);
        assert_eq!(report.warnings(), 2);
        assert!(report.problems[1].message.starts_with("template gone: "));
        assert_eq!(report.problems[1].template.as_deref(), Some("gone"));
        assert_eq!(report.problems[3].template.as_deref(), Some("typo"));
        assert!(report.problems[2].message.starts_with("template nodxvk: "));
        // The shipped template isn't the user's to fix.
        assert_eq!(report.templates, ["broken", "gone", "nodxvk", "typo"]);
    }

    #[test]
    fn a_config_that_fails_to_parse_skips_its_other_checks() {
        let f = Fixture::new();
        f.wine("game", "prefx = 1\n[targets.a]\nexe = '/missing'\n");
        f.lost_run("game");
        let problem = f.only(Code::EnvironmentConfigInvalid);
        assert_eq!(f.codes(), [Code::EnvironmentConfigInvalid]);
        assert_eq!(problem.environment.as_deref(), Some("game"));
        assert!(problem.message.contains("prefx"));
    }

    #[test]
    fn an_interrupted_restore_is_an_error_and_skips_the_rest() {
        let f = Fixture::new();
        f.wine("game", "prefx = 1\n");
        let marker = f.paths.restore_marker("game");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, "").unwrap();
        assert_eq!(f.codes(), [Code::RestoreInterrupted]);
        let problem = f.only(Code::RestoreInterrupted);
        assert!(problem.message.contains("wineo snapshot recover game"));
        assert_eq!(problem.fix, None);
    }

    #[test]
    fn a_well_formed_restore_marker_offers_only_explicit_recovery() {
        use std::os::unix::fs::MetadataExt;

        let f = Fixture::new();
        f.wine("game", HEALTHY);
        let prefix = f.prefix_dir("game");
        let metadata = fs::metadata(&prefix).unwrap();
        let marker = f.paths.restore_marker("game");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        let saved_config = fs::read_to_string(f.paths.environment_config("game")).unwrap();
        let contents = serde_json::json!({
            "version": 1,
            "environment": "game",
            "prefix": prefix,
            "staging": prefix.parent().unwrap().join(".wineo-restore-test"),
            "old_directory": { "device": metadata.dev(), "inode": metadata.ino() },
            "old_fingerprint": crate::import::fingerprint(&prefix).unwrap(),
            "old_config": saved_config,
        });
        fs::write(&marker, serde_json::to_vec(&contents).unwrap()).unwrap();
        let problem = f.only(Code::RestoreInterrupted);
        let wanted = Fix::RecoverSnapshot {
            environment: "game".to_owned(),
        };
        assert_eq!(problem.fix, Some(wanted.clone()));
        assert_eq!(
            wanted.label(),
            "Roll back game's interrupted snapshot restore"
        );
        assert!(!wanted.is_batch_safe());
        let report = check_environment(&f.paths, "game").unwrap();
        assert!(report.fixes().is_empty());
        assert!(apply(&f.paths, &wanted, no_download).is_err());
        assert!(marker.exists());
    }

    #[test]
    fn a_runner_that_does_not_resolve_is_an_error_with_a_hint() {
        let f = Fixture::new();
        f.prefix("game");
        f.write_config(
            "game",
            &format!(
                "prefix = '{}'\nrunner = 'nope'\narch = 'win64'\n{HEALTHY}",
                f.prefix_dir("game").display()
            ),
        );
        let problem = f.only(Code::RunnerMissing);
        assert_eq!(problem.level, Level::Error);
        assert_eq!(problem.area, Area::Wine);
        assert_eq!(problem.fix, None);
        assert!(problem.message.contains("wineo download"));
        // The prefix is still there to check `C:\` paths against.
        assert_eq!(f.codes(), [Code::RunnerMissing]);
    }

    #[test]
    fn a_build_that_does_not_resolve_offers_a_download() {
        let f = Fixture::new();
        f.wine(
            "game",
            &format!("dxvk = 'latest'\nvkd3d = 'vkd3d-proton-2.14'\n{HEALTHY}"),
        );
        let report = f.check();
        assert_eq!(codes(&report), [Code::BuildMissing, Code::BuildMissing]);
        assert_eq!(
            report.problems[0].fix,
            Some(Fix::Download {
                component: Component::Dxvk,
                version: None,
            })
        );
        assert_eq!(
            report.problems[1].fix,
            Some(Fix::Download {
                component: Component::Vkd3d,
                version: Some("vkd3d-proton-2.14".to_owned()),
            })
        );
        assert_eq!(fix(&report.problems[0]).label(), "Download the latest DXVK");
    }

    #[test]
    fn the_download_fix_passes_the_pin_and_skips_a_build_already_there() {
        let f = Fixture::new();
        let wanted = Fix::Download {
            component: Component::Dxvk,
            version: Some("dxvk-2.7".to_owned()),
        };
        let mut asked = None;
        apply(&f.paths, &wanted, |component, version| {
            asked = Some((component, version.map(str::to_owned)));
            Ok(())
        })
        .unwrap();
        assert_eq!(asked, Some((Component::Dxvk, Some("dxvk-2.7".to_owned()))));

        let build = f.paths.default_dxvk_root().join("dxvk-2.7");
        for arch in ["x64", "x32"] {
            fs::create_dir_all(build.join(arch)).unwrap();
            fs::write(build.join(arch).join("dxgi.dll"), b"").unwrap();
        }
        apply(&f.paths, &wanted, no_download).unwrap();
    }

    #[test]
    fn a_missing_prefix_start_would_build_is_a_warning_and_skips_windows_paths() {
        let f = Fixture::new();
        f.wine_at(
            "game",
            &f.prefix_dir("game"),
            "[targets.game]\nexe = 'C:\\Game\\missing.exe'\n",
        );
        let problem = f.only(Code::PrefixMissing);
        assert_eq!(problem.level, Level::Warning);
        assert_eq!(f.codes(), [Code::PrefixMissing]);
    }

    #[test]
    fn a_prefix_start_would_refuse_is_an_error() {
        let f = Fixture::new();
        let full = f.root.join("full");
        fs::create_dir_all(&full).unwrap();
        fs::write(full.join("notes.txt"), "mine").unwrap();
        f.wine_at("full", &full, HEALTHY);

        let proton = f.prefix_dir("proton");
        fs::create_dir_all(proton.join("pfx")).unwrap();
        fs::write(proton.join("pfx/system.reg"), "").unwrap();
        f.wine_at("proton", &proton, HEALTHY);

        let empty = f.root.join("mnt/empty");
        fs::create_dir_all(&empty).unwrap();
        f.wine_at("empty", &empty, "");

        let report = f.check();
        assert_eq!(
            codes(&report),
            [
                Code::PrefixRefused,
                Code::PrefixRefused,
                Code::PrefixRefused
            ]
        );
        assert!(report.problems[0].message.contains("wineo setup empty"));
        assert!(report.problems[2].message.contains("Proton"));
    }

    #[test]
    fn missing_exes_and_working_dirs_are_errors() {
        let f = Fixture::new();
        f.wine(
            "game",
            concat!(
                "[targets.windows]\nexe = 'C:\\Game\\missing.exe'\ncwd = 'C:\\nowhere'\n",
                "[targets.unix]\nexe = '/nonexistent/setup.exe'\n",
                "[targets.bare]\nexe = 'notepad'\ncwd = '/nonexistent'\ndefault = true\n",
                "[targets.fine]\nexe = 'C:\\Game\\game.exe'\n",
            ),
        );
        let report = f.check();
        let messages: Vec<(Code, &str)> = report
            .problems
            .iter()
            .map(|problem| (problem.code, problem.message.as_str()))
            .collect();
        assert_eq!(
            messages,
            [
                (
                    Code::CwdMissing,
                    "target bare: working directory /nonexistent not found"
                ),
                (
                    Code::ExeMissing,
                    "target unix: /nonexistent/setup.exe not found"
                ),
                (
                    Code::ExeMissing,
                    "target windows: C:\\Game\\missing.exe not found"
                ),
            ]
        );
        assert!(report.problems.iter().all(|p| p.area == Area::Target));
    }

    #[test]
    fn gamemode_without_gamemoderun_is_an_error() {
        let f = Fixture::new();
        f.wine("game", &format!("gamemode = true\n{HEALTHY}"));
        let installed = runner::find_on_path(launch::GAMEMODE).is_some();
        let expected: &[Code] = if installed {
            &[]
        } else {
            &[Code::GamemodeMissing]
        };
        assert_eq!(f.codes(), expected);
    }

    #[test]
    fn launch_checks_report_missing_wrappers_and_unsupported_wayland() {
        let f = Fixture::new();
        f.wine(
            "game",
            &format!("wayland = true\nmangohud = true\n[gamescope]\n{HEALTHY}"),
        );
        let report = f.check();
        let has = |code| report.problems.iter().find(|p| p.code == code);
        for (code, program) in [
            (Code::GamescopeMissing, "gamescope"),
            (Code::MangohudMissing, "mangoapp"),
        ] {
            if runner::find_on_path(program).is_none() {
                let problem = has(code).unwrap();
                assert!(problem.message.contains(program));
                assert_eq!(problem.area, Area::Target);
                assert_eq!(problem.level, Level::Error);
            }
        }
        let problem = has(Code::WaylandUnsupported).unwrap();
        assert!(problem.message.contains("fake doesn't support it"));
        assert_eq!(problem.area, Area::Target);
        assert_eq!(problem.level, Level::Error);

        f.wine("game", &format!("mangohud = true\n{HEALTHY}"));
        let report = f.check();
        if runner::find_on_path("mangohud").is_none() {
            assert!(
                report
                    .problems
                    .iter()
                    .any(|p| p.code == Code::MangohudMissing
                        && p.message.contains("mangohud was not found"))
            );
        }
        assert!(
            !report
                .problems
                .iter()
                .any(|p| p.code == Code::WaylandUnsupported)
        );

        f.write_config(
            "game",
            &format!(
                "prefix = '{}'\nrunner = 'missing'\narch = 'win64'\nwayland = true\n{HEALTHY}",
                f.prefix_dir("game").display()
            ),
        );
        let report = f.check();
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.code == Code::RunnerMissing)
        );
        assert!(
            !report
                .problems
                .iter()
                .any(|p| p.code == Code::WaylandUnsupported)
        );
    }

    #[test]
    fn native_targets_are_resolved_like_a_launch() {
        let f = Fixture::new();
        let script = f.root.join("script");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        f.native(
            "tool",
            &format!(
                concat!(
                    "[targets.windows]\nexe = 'C:\\Games\\game.exe'\ndefault = true\n",
                    "[targets.plain]\nexe = '{}'\n",
                    "[targets.gone]\nexe = '/nonexistent/tool'\n",
                ),
                script.display()
            ),
        );
        let report = f.check();
        assert_eq!(
            codes(&report),
            [
                Code::NativeExeInvalid,
                Code::NativeExeInvalid,
                Code::NativeExeInvalid
            ]
        );
        assert!(report.problems[1].message.contains("chmod +x"));
        assert!(report.problems.iter().all(|p| p.area == Area::NativeTarget));
    }

    #[test]
    fn several_targets_without_a_default_is_a_warning() {
        let f = Fixture::new();
        let two = "[targets.a]\nexe = 'notepad'\n[targets.b]\nexe = 'notepad'\n";
        f.wine("game", two);
        let problem = f.only(Code::TargetAmbiguous);
        assert_eq!(problem.level, Level::Warning);
        assert_eq!(problem.area, Area::Environment);

        f.wine("game", &format!("{two}default = true\n"));
        assert_eq!(f.codes(), []);
    }

    #[test]
    fn proxied_validate_reports_steam_not_found_and_a_missing_selected_install() {
        let f = Fixture::new();
        f.proxied("game", "77", false);
        let problem = f.only(Code::SteamNotFound);
        assert_eq!(problem.level, Level::Error);
        assert_eq!(problem.area, Area::Steam);
        assert_eq!(problem.message, "Steam isn't found.");

        f.global("steam = 'flatpak'\n");
        let problem = f.only(Code::SteamInstallMissing);
        assert_eq!(problem.level, Level::Error);
        assert_eq!(problem.area, Area::Steam);
    }

    #[test]
    fn proxied_validate_does_not_report_a_game_that_is_not_installed() {
        let f = Fixture::new();
        f.proxied("game", "77", false);
        f.steam_root();
        assert_eq!(f.codes(), []);
        // Queued, installing, and uninstalling are normal states too.
        for flags in [2, 1026, 4 | 2048] {
            f.steam_manifest(
                "77",
                &format!(
                    "\"AppState\" {{ \"appid\" \"77\" \"name\" \"Game\" \"StateFlags\" \"{flags}\" }}"
                ),
            );
            assert_eq!(f.codes(), [], "flags {flags}");
        }
    }

    #[test]
    fn a_not_installed_game_passes_while_an_unrelated_error_still_fails() {
        let f = Fixture::new();
        f.proxied("game", "77", false);
        f.proxied("broken", "78", false);
        f.steam_manifest(
            "78",
            "\"AppState\" { \"appid\" \"78\" \"StateFlags\" \"bad\" }",
        );
        let report = f.check();
        assert_eq!(codes(&report), [Code::SteamDataUnknown]);
        assert_eq!(report.problems[0].environment.as_deref(), Some("broken"));
        assert_eq!(report.errors(), 1);
    }

    #[test]
    fn proxied_validate_warns_when_a_steam_library_is_unavailable() {
        let f = Fixture::new();
        f.proxied("game", "77", false);
        let root = f.steam_root();
        let missing = f.root.join("missing-library");
        fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}",
                missing.display()
            ),
        )
        .unwrap();
        let problem = f.only(Code::SteamLibraryUnavailable);
        assert_eq!(problem.level, Level::Warning);
        assert_eq!(problem.area, Area::Steam);
        assert!(problem.message.ends_with("missing-library is unavailable."));
    }

    #[test]
    fn proxied_validate_reports_an_unreadable_manifest() {
        let f = Fixture::new();
        f.proxied("game", "77", false);
        f.steam_manifest(
            "77",
            "\"AppState\" { \"appid\" \"77\" \"StateFlags\" \"bad\" }",
        );
        let problem = f.only(Code::SteamDataUnknown);
        assert_eq!(problem.level, Level::Error);
        assert_eq!(problem.area, Area::Steam);
    }

    #[test]
    fn a_healthy_proxied_environment_validates_cleanly() {
        let f = Fixture::new();
        f.proxied("game", "77", false);
        f.steam_manifest(
            "77",
            "\"AppState\" { \"appid\" \"77\" \"name\" \"Game\" \"StateFlags\" \"4\" }",
        );
        assert_eq!(f.codes(), []);
    }

    #[test]
    fn implicit_steam_target_art_validates_cleanly() {
        let f = Fixture::new();
        f.proxied("game", "77", false);
        f.steam_manifest(
            "77",
            "\"AppState\" { \"appid\" \"77\" \"name\" \"Game\" \"StateFlags\" \"4\" }",
        );
        let art = f.art_dir("game");
        fs::create_dir_all(art.join("steam")).unwrap();
        fs::write(
            art.join("art.toml"),
            "schema_version = 1\n[targets.steam.icon]\nfile = 'icon.png'\nmanual = 'icon.png'\n",
        )
        .unwrap();
        fs::write(art.join("steam/icon.png"), b"icon").unwrap();

        assert_eq!(f.codes(), []);
    }

    #[test]
    fn proxied_validate_checks_pending_desktop_entries_and_fixes_them() {
        let f = Fixture::new();
        f.proxied("game", "77", true);
        let problem = f.only(Code::DesktopOutdated);
        assert_eq!(problem.level, Level::Warning);
        assert_eq!(problem.area, Area::Desktop);
        apply(&f.paths, fix(&problem), no_download).unwrap();
        let entry = f.paths.applications_dir().join("wineo-game-steam.desktop");
        assert!(
            fs::read_to_string(entry)
                .unwrap()
                .contains("Exec=wineo start game\n")
        );
    }

    #[test]
    fn a_lost_run_is_cleaned_up_like_stop_does() {
        let f = Fixture::new();
        f.native("tool", "");
        f.lost_run("tool");
        let problem = f.only(Code::LostRun);
        assert_eq!(problem.level, Level::Warning);
        assert_eq!(fix(&problem).label(), "Clean up tool's lost run");

        apply(&f.paths, fix(&problem), no_download).unwrap();
        let run = Runtime::new(&f.paths, "tool")
            .read_state()
            .unwrap()
            .unwrap();
        assert_eq!(run.status, RunStatus::Stopped);
        assert_eq!(f.codes(), []);
    }

    #[test]
    fn a_lost_run_reports_when_its_session_cannot_be_logged() {
        let f = Fixture::new();
        f.native("tool", "");
        f.lost_run("tool");
        fs::create_dir_all(f.paths.stats_log("tool")).unwrap();
        let problem = f.only(Code::LostRun);

        let notes = apply(&f.paths, fix(&problem), no_download).unwrap();
        assert_eq!(notes.len(), 1);
        assert!(
            matches!(&notes[0], Note::Warning(message) if message.starts_with("failed to open "))
        );
        let run = Runtime::new(&f.paths, "tool")
            .read_state()
            .unwrap()
            .unwrap();
        assert_eq!(run.status, RunStatus::Stopped);
    }

    #[test]
    fn an_unresolved_installer_is_cleaned_up_like_stop_does() {
        let f = Fixture::new();
        f.wine("game", HEALTHY);
        f.unresolved_installer("game", Some(4242));
        let problem = f.only(Code::InstallerUnresolved);
        assert_eq!(problem.level, Level::Warning);
        apply(&f.paths, fix(&problem), no_download).unwrap();
        assert!(!Runtime::new(&f.paths, "game").operation_path().exists());
        assert_eq!(f.codes(), []);
    }

    #[test]
    fn an_installer_cleanup_that_cannot_be_confirmed_fails_and_keeps_the_record() {
        let f = Fixture::new();
        f.wine("game", HEALTHY);
        f.unresolved_installer("game", None);
        let problem = f.only(Code::InstallerUnresolved);
        let err = apply(&f.paths, fix(&problem), no_download).unwrap_err();
        assert!(
            err.to_string().contains("could not confirm cleanup"),
            "{err}"
        );
        assert!(Runtime::new(&f.paths, "game").operation_path().exists());
    }

    #[test]
    fn a_state_dir_without_an_environment_is_removed() {
        let f = Fixture::new();
        f.wine("game", HEALTHY);
        let stray = f.paths.runtime_dir("oldgame");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join("output.log"), "log").unwrap();
        // wineo's own dirs aren't environments.
        fs::create_dir_all(f.paths.locks_dir()).unwrap();
        fs::create_dir_all(f.paths.import_log("x").parent().unwrap()).unwrap();
        fs::create_dir_all(f.paths.runtime_dir("game")).unwrap();

        let problem = f.only(Code::StateDirStray);
        assert_eq!(f.codes(), [Code::StateDirStray]);
        assert_eq!(
            problem.message,
            "state dir ~/state/wineo/oldgame has no environment"
        );
        assert_eq!(problem.environment, None);
        assert_eq!(problem.stray.as_deref(), Some("oldgame"));
        assert_eq!(fix(&problem).label(), "Remove oldgame's leftover state");
        apply(&f.paths, fix(&problem), no_download).unwrap();
        assert!(!stray.exists());
        assert_eq!(f.codes(), []);
    }

    #[test]
    fn a_stats_log_without_an_environment_is_removed() {
        let f = Fixture::new();
        f.wine("game", HEALTHY);
        let stray = f.stats_log("oldgame");
        fs::create_dir_all(stray.parent().unwrap()).unwrap();
        fs::write(&stray, "{}\n").unwrap();
        // A live environment's own log isn't a stray.
        fs::write(f.stats_log("game"), "{}\n").unwrap();

        let problem = f.only(Code::StatsStray);
        assert_eq!(f.codes(), [Code::StatsStray]);
        assert_eq!(problem.area, Area::Runtime);
        assert!(problem.message.contains("play stats"));
        assert!(problem.message.contains("has no environment"));
        assert_eq!(problem.environment, None);
        assert_eq!(problem.stray.as_deref(), Some("oldgame"));
        assert_eq!(
            fix(&problem).label(),
            "Remove oldgame's leftover play stats"
        );
        apply(&f.paths, fix(&problem), no_download).unwrap();
        assert!(!stray.exists());
        assert_eq!(f.codes(), []);
    }

    #[test]
    fn a_state_dir_still_tracking_a_run_is_kept() {
        let f = Fixture::new();
        f.lost_run("oldgame");
        let problem = f.only(Code::StateDirStray);
        let err = apply(&f.paths, fix(&problem), no_download).unwrap_err();
        assert!(err.to_string().contains("last run was lost"), "{err}");
        assert!(f.paths.runtime_dir("oldgame").exists());
    }

    #[test]
    fn a_desktop_entry_without_an_environment_is_removed_with_its_icons() {
        let f = Fixture::new();
        let (entry, icon) = f.stray_entry("oldgame", "oldgame");
        let problem = f.only(Code::DesktopEntryStray);
        assert_eq!(
            problem.message,
            "desktop entry wineo-oldgame-oldgame.desktop has no environment"
        );
        assert_eq!(problem.stray.as_deref(), Some("oldgame"));
        apply(&f.paths, fix(&problem), no_download).unwrap();
        assert!(!entry.exists());
        assert!(!icon.exists());
        assert_eq!(f.codes(), []);
    }

    #[test]
    fn out_of_date_desktop_entries_are_rewritten() {
        let f = Fixture::new();
        f.wine("game", &format!("{HEALTHY}desktop = true\n"));
        let problem = f.only(Code::DesktopOutdated);
        assert_eq!(
            problem.message,
            "desktop entries for game are out of date with the config"
        );
        let entry = f.paths.applications_dir().join("wineo-game-game.desktop");
        assert!(!entry.exists());

        apply(&f.paths, fix(&problem), no_download).unwrap();
        assert!(entry.exists());
        assert_eq!(f.codes(), []);

        f.wine("game", HEALTHY);
        assert_eq!(f.codes(), [Code::DesktopOutdated]);
        apply(&f.paths, fix(&f.only(Code::DesktopOutdated)), no_download).unwrap();
        assert!(!entry.exists());
    }

    #[test]
    fn a_native_environments_entries_are_checked_too() {
        let f = Fixture::new();
        let tool = f.root.join("tool");
        executable(&tool, "#!/bin/sh\n");
        f.native(
            "tool",
            &format!(
                "[targets.tool]\nexe = '{}'\ndesktop = true\n",
                tool.display()
            ),
        );
        let problem = f.only(Code::DesktopOutdated);
        apply(&f.paths, fix(&problem), no_download).unwrap();
        assert_eq!(f.codes(), []);
    }

    #[test]
    fn portable_mode_checks_only_its_own_root_and_skips_desktop_checks() {
        let f = Fixture::new();
        f.stray_entry("oldgame", "oldgame");
        let usb = f.root.join("usb");
        let paths = Fixture::xdg(&f.root, Some(&usb));
        let config = paths.environment_config("game");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            &config,
            "prefix = 'prefixes/game'\nrunner = 'fake'\narch = 'win64'\n[targets.game]\nexe = 'notepad'\ndesktop = true\n",
        )
        .unwrap();
        let report = check_all(&paths).unwrap();
        assert_eq!(codes(&report), [Code::PrefixMissing, Code::DesktopPortable]);
        assert_eq!(report.environments, ["game"]);
    }

    #[test]
    fn art_records_whose_file_or_target_is_gone_are_dropped() {
        let f = Fixture::new();
        f.wine("game", HEALTHY);
        let art = f.art_dir("game");
        fs::create_dir_all(&art).unwrap();
        let text = concat!(
            "# kept by hand\n",
            "schema_version = 1\n\n",
            "[icon] # mine\n",
            "file = \"icon.png\"\n",
            "manual = \"icon.png\"\n\n",
            "[targets.old.grid]\n",
            "file = \"grid.png\"\n",
            "manual = \"grid.png\"\n",
        );
        fs::write(art.join("art.toml"), text).unwrap();

        let report = f.check();
        assert_eq!(
            codes(&report),
            [Code::ArtFileMissing, Code::ArtTargetUnknown]
        );
        assert_eq!(
            report.problems[0].message,
            "art: icon.png is recorded but missing"
        );
        assert_eq!(
            fix(&report.problems[0]).label(),
            "Drop game's missing icon record"
        );
        assert_eq!(
            fix(&report.problems[1]).label(),
            "Drop game's art records for old"
        );
        for problem in &report.problems {
            apply(&f.paths, fix(problem), no_download).unwrap();
        }
        assert_eq!(f.codes(), []);
        let after = fs::read_to_string(art.join("art.toml")).unwrap();
        assert_eq!(after, "# kept by hand\nschema_version = 1\n");
    }

    #[test]
    fn a_dropped_record_whose_file_came_back_is_kept() {
        let f = Fixture::new();
        f.wine("game", HEALTHY);
        let art = f.art_dir("game");
        fs::create_dir_all(&art).unwrap();
        let text = "schema_version = 1\n[icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n";
        fs::write(art.join("art.toml"), text).unwrap();
        let problem = f.only(Code::ArtFileMissing);
        fs::write(art.join("icon.png"), b"png").unwrap();
        apply(&f.paths, fix(&problem), no_download).unwrap();
        assert_eq!(fs::read_to_string(art.join("art.toml")).unwrap(), text);
    }

    #[test]
    fn duplicate_art_stray_art_dirs_and_a_broken_art_toml_are_reported() {
        let f = Fixture::new();
        f.wine("game", HEALTHY);
        f.wine("other", HEALTHY);
        let art = f.art_dir("game");
        fs::create_dir_all(&art).unwrap();
        fs::write(art.join("icon.png"), b"png").unwrap();
        fs::write(art.join("icon.ico"), b"ico").unwrap();
        fs::create_dir_all(f.art_dir("oldgame")).unwrap();
        fs::create_dir_all(f.art_dir(".wineo-art-staging")).unwrap();
        fs::create_dir_all(f.art_dir("other")).unwrap();
        fs::write(f.art_dir("other").join("art.toml"), "icon = 3\n").unwrap();

        let report = f.check();
        assert_eq!(
            codes(&report),
            [
                Code::ArtDuplicates,
                Code::ArtConfigInvalid,
                Code::ArtDirStray
            ]
        );
        assert_eq!(
            report.problems[0].message,
            "art: icon.png is used, so icon.ico is ignored"
        );
        assert_eq!(report.problems[0].fix, None);
        assert_eq!(report.problems[1].level, Level::Error);
        assert_eq!(report.problems[2].fix, None);
        assert_eq!(report.problems[2].stray.as_deref(), Some("oldgame"));
    }

    #[test]
    fn a_fix_on_a_busy_environment_fails_without_stopping_the_others() {
        let f = Fixture::new();
        f.native("tool", "");
        f.lost_run("tool");
        f.native("idle", "");
        f.lost_run("idle");
        fs::create_dir_all(f.paths.runtime_dir("oldgame")).unwrap();
        f.stray_entry("gone", "gone");

        let locks = Locks::new(&f.paths);
        let _tool = locks.acquire(Operation::Create, "tool", &[]).unwrap();
        let _oldgame = locks.acquire(Operation::Snapshot, "oldgame", &[]).unwrap();

        let report = f.check();
        let fixes: Vec<&Fix> = report
            .problems
            .iter()
            .filter_map(|p| p.fix.as_ref())
            .collect();
        assert_eq!(fixes.len(), 4);
        let results: Vec<(String, std::result::Result<(), String>)> = fixes
            .into_iter()
            .map(|fix| {
                let result = apply(&f.paths, fix, no_download)
                    .map(|_| ())
                    .map_err(|err| err.to_string());
                (fix.label(), result)
            })
            .collect();
        assert_eq!(
            results,
            [
                ("Clean up idle's lost run".to_owned(), Ok(())),
                (
                    "Clean up tool's lost run".to_owned(),
                    Err("environment `tool` is busy: it is being created".to_owned())
                ),
                (
                    "Remove oldgame's leftover state".to_owned(),
                    Err(
                        "environment `oldgame` is busy: a snapshot operation is running".to_owned()
                    )
                ),
                ("Remove gone's desktop entry".to_owned(), Ok(())),
            ]
        );
        assert!(f.paths.runtime_dir("oldgame").exists());
        assert!(Runtime::new(&f.paths, "tool").status().unwrap().lost);
    }

    #[test]
    fn fixes_leave_the_users_configs_byte_for_byte_unchanged() {
        let f = Fixture::new();
        f.global("# mine\nstop_timeout = 5 # seconds\n");
        f.template("mine", "# notes\nrunner = 'fake'\n");
        f.wine(
            "game",
            &format!("# a comment\n{HEALTHY}desktop = true   # spacing kept\n"),
        );
        f.lost_run("game");
        f.unresolved_installer("game", Some(4242));
        let art = f.art_dir("game");
        fs::create_dir_all(&art).unwrap();
        fs::write(
            art.join("art.toml"),
            "[icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n[targets.old.icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n",
        )
        .unwrap();
        fs::create_dir_all(f.paths.runtime_dir("oldgame")).unwrap();
        f.stray_entry("gone", "gone");
        let before = f.user_configs();

        let report = f.check();
        let fixes: Vec<&Fix> = report
            .problems
            .iter()
            .filter_map(|p| p.fix.as_ref())
            .collect();
        assert_eq!(fixes.len(), 7, "{:#?}", report.problems);
        for fix in fixes {
            apply(&f.paths, fix, no_download).unwrap();
        }
        assert_eq!(f.user_configs(), before);
        assert_eq!(f.codes(), []);
    }

    #[test]
    fn check_environment_checks_only_that_environment() {
        let f = Fixture::new();
        f.global("stop_timeuot = 3\n");
        f.wine("game", "[targets.game]\nexe = 'C:\\Game\\missing.exe'\n");
        f.wine("other", "[targets.game]\nexe = 'C:\\Game\\missing.exe'\n");
        fs::create_dir_all(f.paths.runtime_dir("oldgame")).unwrap();
        let report = check_environment(&f.paths, "game").unwrap();
        assert_eq!(codes(&report), [Code::ExeMissing]);
        assert_eq!(report.environments, ["game"]);
        assert!(matches!(
            check_environment(&f.paths, "nope"),
            Err(Error::UnknownEnvironment(_))
        ));
    }

    #[test]
    fn codes_levels_and_areas_serialize_as_decision_six_shows() {
        for code in Code::ALL {
            let json = serde_json::to_value(code).unwrap();
            assert_eq!(json, code.as_str());
            assert_eq!(
                serde_json::to_value(code.area()).unwrap(),
                code.area().as_str()
            );
            assert_eq!(
                serde_json::to_value(code.level()).unwrap(),
                code.level().as_str()
            );
        }
        let f = Fixture::new();
        f.wine("zomboid", HEALTHY);
        let art = f.art_dir("zomboid");
        fs::create_dir_all(&art).unwrap();
        fs::write(
            art.join("art.toml"),
            "[icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n",
        )
        .unwrap();
        let problem = f.only(Code::ArtFileMissing);
        assert_eq!(
            serde_json::to_value(&problem).unwrap(),
            serde_json::json!({
                "environment": "zomboid",
                "area": "art",
                "level": "warning",
                "code": "art_file_missing",
                "message": "art: icon.png is recorded but missing",
                "fix": "Drop zomboid's missing icon record",
            })
        );
        let stray = Problem::stray(Code::ArtDirStray, "old", "gone".to_owned(), None);
        let json = serde_json::to_value(&stray).unwrap();
        assert_eq!(json["environment"], serde_json::Value::Null);
        assert_eq!(json["fix"], serde_json::Value::Null);
    }

    /// Every path under `dir` with its contents, symlinks unfollowed.
    fn tree(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
        let mut found = Vec::new();
        let mut stack = vec![dir.to_owned()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    stack.push(path.clone());
                }
                let bytes = kind.is_file().then(|| fs::read(&path).unwrap());
                found.push((path, bytes));
            }
        }
        found.sort();
        found
    }

    #[test]
    fn checking_changes_nothing_on_disk() {
        let f = Fixture::new();
        f.global("stop_timeout = 5\n");
        f.wine(
            "game",
            &format!("dxvk = 'latest'\n{HEALTHY}desktop = true\n"),
        );
        f.lost_run("game");
        f.unresolved_installer("game", Some(4242));
        fs::create_dir_all(f.paths.runtime_dir("oldgame")).unwrap();
        f.stray_entry("gone", "gone");
        let art = f.art_dir("game");
        fs::create_dir_all(&art).unwrap();
        fs::write(
            art.join("art.toml"),
            "[icon]\nfile = \"icon.png\"\nmanual = \"a\"\n",
        )
        .unwrap();

        let before = tree(&f.root);
        let report = f.check();
        assert!(report.problems.len() >= 6, "{:#?}", report.problems);
        check_environment(&f.paths, "game").unwrap();
        assert_eq!(tree(&f.root), before);
    }

    mod gog {
        use std::collections::HashMap;
        use std::os::unix::fs::MetadataExt;

        use super::*;
        use crate::gog::folder::{
            Committed, FolderLock, Journal, JournalKind, Platform, Target, Track,
        };

        const NEVER: &dyn Fn() -> bool = &|| false;
        const ID: u64 = 1094900565;

        fn state(folder: &Path, owner: Option<&str>, journal: bool) -> FolderState {
            let target = Target {
                gog_id: ID,
                platform: Platform::Linux,
                language: "en-US".to_owned(),
                track: Track::Installer,
                products: Vec::new(),
            };
            FolderState {
                device: Some(7),
                committed: Some(Committed {
                    gog_id: ID,
                    platform: target.platform,
                    language: target.language.clone(),
                    track: target.track,
                    products: Vec::new(),
                    files: Vec::new(),
                }),
                journal: journal.then(|| Journal {
                    operation: "op".to_owned(),
                    kind: JournalKind::Update,
                    target,
                    packages: Vec::new(),
                    attempted: Vec::new(),
                }),
                ..FolderState::new(folder.to_owned(), owner.map(str::to_owned))
            }
        }

        fn save(f: &Fixture, state: &FolderState) {
            FolderLock::exclusive(&f.paths, &state.folder, NEVER)
                .unwrap()
                .write(state)
                .unwrap();
        }

        fn gog_report(f: &Fixture, devices: &[(&Path, u64)]) -> Report {
            let map: HashMap<PathBuf, u64> = devices
                .iter()
                .map(|(path, device)| (path.to_path_buf(), *device))
                .collect();
            let device_of = move |path: &Path| Ok(map.get(path).copied());
            let names = environment::names(&f.paths).unwrap();
            let mut report = Report::default();
            check_gog(
                &f.paths,
                None,
                &names,
                &BTreeSet::new(),
                &device_of,
                &mut report,
            );
            report
        }

        fn apply_fix(f: &Fixture, fix: &Fix) -> anyhow::Result<Vec<Note>> {
            apply(&f.paths, fix, |_, _| Ok(()))
        }

        #[test]
        fn a_missing_folder_is_dropped_only_when_its_drive_is_mounted() {
            let f = Fixture::new();
            let folder = Path::new("/mnt/games/rimworld");
            save(&f, &state(folder, None, false));

            // Unmounted: the mount point sits on the root filesystem.
            let report = gog_report(&f, &[(Path::new("/"), 1), (Path::new("/mnt/games"), 1)]);
            assert_eq!(codes(&report), [Code::GogFolderMissing]);
            assert_eq!(
                report.problems[0].message,
                "GOG game folder /mnt/games/rimworld is missing."
            );
            assert_eq!(report.problems[0].fix, None);

            // Remounted with the folder there: nothing to report.
            let report = gog_report(
                &f,
                &[
                    (Path::new("/"), 1),
                    (Path::new("/mnt/games"), 7),
                    (folder, 7),
                ],
            );
            assert!(report.problems.is_empty(), "{:#?}", report.problems);

            // Mounted and really deleted: offered, and nothing else.
            let report = gog_report(&f, &[(Path::new("/"), 1), (Path::new("/mnt/games"), 7)]);
            assert_eq!(
                report.problems[0].fix,
                Some(Fix::ForgetGogFolder {
                    folder: folder.to_owned()
                })
            );
        }

        #[test]
        fn forgetting_a_deleted_folder_rechecks_it() {
            let f = Fixture::new();
            let folder = f.root.join("Games/gone");
            fs::create_dir_all(&folder).unwrap();
            let mut gone = state(&folder, None, false);
            gone.device = Some(fs::metadata(&folder).unwrap().dev());
            fs::remove_dir(&folder).unwrap();
            save(&f, &gone);
            let fix = f.only(Code::GogFolderMissing).fix.unwrap();
            apply_fix(&f, &fix).unwrap();
            assert!(folder::list(&f.paths).unwrap().is_empty());
            assert!(apply_fix(&f, &fix).is_err());
        }

        #[test]
        fn mid_update_folders_name_their_owner() {
            let f = Fixture::new();
            f.native("rimworld", &format!("gog_id = '{ID}'\n"));
            let folder = f.root.join("rimworld");
            fs::create_dir_all(&folder).unwrap();
            save(&f, &state(&folder, Some("rimworld"), true));
            let problem = f.only(Code::GogMidUpdate);
            assert_eq!(problem.environment.as_deref(), Some("rimworld"));
            assert_eq!(
                problem.message,
                "rimworld's game folder is mid-update. Finish it with wineo update rimworld."
            );
            assert_eq!(problem.level, Level::Error);
            assert_eq!(problem.area, Area::Gog);
            let one = check_environment(&f.paths, "rimworld").unwrap();
            assert_eq!(codes(&one), [Code::GogMidUpdate]);
        }

        #[test]
        fn a_lost_state_is_reported_with_how_to_fix_it() {
            let f = Fixture::new();
            f.native("rimworld", &format!("gog_id = '{ID}'\n"));
            let problem = f.only(Code::GogStateMissing);
            assert_eq!(
                problem.message,
                "rimworld's GOG state is missing. Migrate it again with wineo import gog --game 1094900565, or stop tracking it with wineo set rimworld --unset gog_id."
            );
            assert_eq!(
                codes(&check_environment(&f.paths, "rimworld").unwrap()),
                [Code::GogStateMissing]
            );
        }

        #[test]
        fn an_ownerless_folder_is_left_for_claiming_and_another_owner_is_reported() {
            let f = Fixture::new();
            f.native("rimworld", &format!("gog_id = '{ID}'\n"));
            f.native("copy", "");
            let folder = f.root.join("rimworld");
            fs::create_dir_all(&folder).unwrap();
            save(&f, &state(&folder, None, false));
            assert!(f.check().problems.is_empty(), "{:#?}", f.check().problems);

            save(&f, &state(&folder, Some("copy"), false));
            let problem = f.only(Code::GogFolderTaken);
            assert_eq!(
                problem.message,
                "rimworld's GOG game folder ~/rimworld belongs to copy."
            );
        }

        #[test]
        fn a_missing_owner_is_released_by_fix_keeping_any_journal() {
            for journal in [false, true] {
                let f = Fixture::new();
                let folder = f.root.join("rimworld");
                fs::create_dir_all(&folder).unwrap();
                save(&f, &state(&folder, Some("rimworld"), journal));
                let problem = f.only(Code::GogOwnerMissing);
                assert_eq!(
                    problem.message,
                    "GOG game folder ~/rimworld names rimworld, which doesn't exist."
                );
                let fix = problem.fix.unwrap();
                assert!(f.check().fixes().contains(&fix));
                apply_fix(&f, &fix).unwrap();
                let released = FolderLock::shared(&f.paths, &folder, NEVER)
                    .unwrap()
                    .read()
                    .unwrap()
                    .unwrap();
                assert_eq!(released.owner, None);
                assert_eq!(released.journal.is_some(), journal);
                assert!(!codes(&f.check()).contains(&Code::GogOwnerMissing));
            }
        }

        #[test]
        fn an_unreadable_state_is_reported() {
            let f = Fixture::new();
            let dir = f.paths.gog_folders_dir();
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("bad.toml"), "version = 9\nfolder = '/x'\n").unwrap();
            f.only(Code::GogStateInvalid);
        }

        #[test]
        fn a_portable_root_skips_host_gog_checks() {
            let f = Fixture::new();
            save(
                &f,
                &state(Path::new("/gone/rimworld"), Some("nobody"), true),
            );
            let portable = Fixture::xdg(&f.root, Some(&f.root.join("usb")));
            let report = check_all(&portable).unwrap();
            assert!(
                !report
                    .problems
                    .iter()
                    .any(|problem| problem.area == Area::Gog),
                "{:#?}",
                report.problems
            );
        }

        #[test]
        fn folders_that_cant_be_looked_at_still_give_a_full_report() {
            use std::os::unix::fs::PermissionsExt;

            let f = Fixture::new();
            f.native("lost", "gog_id = '2'\n");
            // A parent component that is now a file.
            let file = f.root.join("was-a-dir");
            fs::write(&file, "").unwrap();
            let not_a_dir = file.join("game");
            save(&f, &state(&not_a_dir, None, false));
            // A parent that can't be searched.
            let locked = f.root.join("locked");
            fs::create_dir_all(locked.join("game")).unwrap();
            let denied = locked.join("game");
            save(&f, &state(&denied, None, false));
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
            let readable = fs::symlink_metadata(&denied).is_ok();

            let report = f.check();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            let unchecked: Vec<&Problem> = report
                .problems
                .iter()
                .filter(|problem| problem.code == Code::GogFolderMissing)
                .collect();
            assert!(
                unchecked
                    .iter()
                    .any(|problem| problem.message.contains("was-a-dir/game can't be checked")),
                "{:#?}",
                report.problems
            );
            // Running as root sees through the permissions.
            if !readable {
                assert!(
                    unchecked
                        .iter()
                        .any(|problem| problem.message.contains("locked/game can't be checked")),
                    "{:#?}",
                    report.problems
                );
            }
            assert!(unchecked.iter().all(|problem| problem.fix.is_none()));
            assert!(codes(&report).contains(&Code::GogStateMissing));
        }

        #[test]
        fn validate_name_shows_an_unreadable_state_that_could_be_its_own() {
            let f = Fixture::new();
            f.native("rimworld", &format!("gog_id = '{ID}'\n"));
            f.native("plain", "");
            let dir = f.paths.gog_folders_dir();
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("bad.toml"), "version = 9\nfolder = '/x'\n").unwrap();
            let one = check_environment(&f.paths, "rimworld").unwrap();
            assert_eq!(codes(&one), [Code::GogStateInvalid, Code::GogStateMissing]);
            assert_eq!(
                one.problems[0].message.matches("bad.toml").count(),
                1,
                "{}",
                one.problems[0].message
            );
            let plain = check_environment(&f.paths, "plain").unwrap();
            assert!(plain.problems.is_empty(), "{:#?}", plain.problems);
        }

        #[test]
        fn validate_name_shows_an_unreadable_state_when_its_config_doesnt_parse() {
            let f = Fixture::new();
            f.write_config("broken", "kind = [");
            let dir = f.paths.gog_folders_dir();
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("bad.toml"), "version = 9\nfolder = '/x'\n").unwrap();
            let one = check_environment(&f.paths, "broken").unwrap();
            assert_eq!(
                codes(&one),
                [Code::EnvironmentConfigInvalid, Code::GogStateInvalid]
            );
        }
    }
}
