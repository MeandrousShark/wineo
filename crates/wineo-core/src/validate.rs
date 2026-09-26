//! Finds config mistakes and leftover state before a launch trips over them,
//! and fixes the ones that are safe to fix.
//!
//! Checks only read, and nothing launches wine or runs a program. A fix never
//! touches a config the user wrote, a prefix, or an art file. The one config a
//! fix edits is `art.toml`, and only to drop a record whose file or target is
//! gone.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Serialize, Serializer};

use crate::art::{self, Kind as ArtKind};
use crate::components::{Builds, Component, LATEST};
use crate::config::{DEFAULT_STOP_TIMEOUT, EnvironmentConfig, GlobalConfig};
use crate::desktop::{self, Entry, Pending};
use crate::environment::{self, Environment};
use crate::error::{Error, IoContext, Result, chain};
use crate::install::{self, Wanted};
use crate::installer::{self, Cleaned};
use crate::launch;
use crate::name;
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::prefix::{self, BuildPlan};
use crate::runner::{self, Runner};
use crate::runners::Runners;
use crate::runtime::{LostRunCleanup, Runtime};
use crate::templates::{Template, Templates};
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
    Art,
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
            Area::Art => "art",
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
    PrefixMissing,
    PrefixRefused,
    ExeMissing,
    CwdMissing,
    GamemodeMissing,
    NativeExeInvalid,
    TargetAmbiguous,
    LostRun,
    InstallerUnresolved,
    RestoreInterrupted,
    StateDirStray,
    DesktopEntryStray,
    DesktopOutdated,
    DesktopPortable,
    ArtConfigInvalid,
    ArtFileMissing,
    ArtTargetUnknown,
    ArtDuplicates,
    ArtDirStray,
}

impl Code {
    pub const ALL: [Code; 25] = [
        Code::GlobalConfigInvalid,
        Code::TemplateInvalid,
        Code::TemplateUnresolved,
        Code::EnvironmentConfigInvalid,
        Code::RunnerMissing,
        Code::BuildMissing,
        Code::PrefixMissing,
        Code::PrefixRefused,
        Code::ExeMissing,
        Code::CwdMissing,
        Code::GamemodeMissing,
        Code::NativeExeInvalid,
        Code::TargetAmbiguous,
        Code::LostRun,
        Code::InstallerUnresolved,
        Code::RestoreInterrupted,
        Code::StateDirStray,
        Code::DesktopEntryStray,
        Code::DesktopOutdated,
        Code::DesktopPortable,
        Code::ArtConfigInvalid,
        Code::ArtFileMissing,
        Code::ArtTargetUnknown,
        Code::ArtDuplicates,
        Code::ArtDirStray,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Code::GlobalConfigInvalid => "global_config_invalid",
            Code::TemplateInvalid => "template_invalid",
            Code::TemplateUnresolved => "template_unresolved",
            Code::EnvironmentConfigInvalid => "environment_config_invalid",
            Code::RunnerMissing => "runner_missing",
            Code::BuildMissing => "build_missing",
            Code::PrefixMissing => "prefix_missing",
            Code::PrefixRefused => "prefix_refused",
            Code::ExeMissing => "exe_missing",
            Code::CwdMissing => "cwd_missing",
            Code::GamemodeMissing => "gamemode_missing",
            Code::NativeExeInvalid => "native_exe_invalid",
            Code::TargetAmbiguous => "target_ambiguous",
            Code::LostRun => "lost_run",
            Code::InstallerUnresolved => "installer_unresolved",
            Code::RestoreInterrupted => "restore_interrupted",
            Code::StateDirStray => "state_dir_stray",
            Code::DesktopEntryStray => "desktop_entry_stray",
            Code::DesktopOutdated => "desktop_outdated",
            Code::DesktopPortable => "desktop_portable",
            Code::ArtConfigInvalid => "art_config_invalid",
            Code::ArtFileMissing => "art_file_missing",
            Code::ArtTargetUnknown => "art_target_unknown",
            Code::ArtDuplicates => "art_duplicates",
            Code::ArtDirStray => "art_dir_stray",
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
            | Code::PrefixRefused
            | Code::ExeMissing
            | Code::CwdMissing
            | Code::GamemodeMissing
            | Code::NativeExeInvalid
            | Code::RestoreInterrupted
            | Code::ArtConfigInvalid => Level::Error,
            Code::TemplateUnresolved
            | Code::PrefixMissing
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
            | Code::ArtDirStray => Level::Warning,
        }
    }

    pub fn area(self) -> Area {
        match self {
            Code::GlobalConfigInvalid => Area::GlobalConfig,
            Code::TemplateInvalid | Code::TemplateUnresolved => Area::Template,
            Code::EnvironmentConfigInvalid => Area::EnvironmentConfig,
            Code::RunnerMissing
            | Code::BuildMissing
            | Code::PrefixMissing
            | Code::PrefixRefused => Area::Wine,
            Code::ExeMissing | Code::CwdMissing | Code::GamemodeMissing => Area::Target,
            Code::NativeExeInvalid => Area::NativeTarget,
            Code::TargetAmbiguous => Area::Environment,
            Code::LostRun
            | Code::InstallerUnresolved
            | Code::RestoreInterrupted
            | Code::StateDirStray => Area::Runtime,
            Code::DesktopEntryStray | Code::DesktopOutdated | Code::DesktopPortable => {
                Area::Desktop
            }
            Code::ArtConfigInvalid
            | Code::ArtFileMissing
            | Code::ArtTargetUnknown
            | Code::ArtDuplicates
            | Code::ArtDirStray => Area::Art,
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
}

impl Fix {
    /// What the fix does, in a few words.
    pub fn label(&self) -> String {
        match self {
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
        }
    }
}

impl fmt::Display for Fix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
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
    let global = check_global(paths, &mut report);
    check_templates(paths, &global, &mut report)?;
    let names = environment::names(paths)?;
    for name in &names {
        check_one(paths, &global, name, &mut report)?;
    }
    check_strays(paths, &names, &mut report)?;
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
    if !paths.environment_config(name).is_file() {
        return Err(Error::UnknownEnvironment(name.to_owned()));
    }
    // A broken global config is only reported by `check_all`. Here it means
    // defaults, as it does for most commands.
    let global = GlobalConfig::load(paths).unwrap_or_default();
    let mut report = Report {
        environments: vec![name.to_owned()],
        ..Report::default()
    };
    check_one(paths, &global, name, &mut report)?;
    contract(paths, &mut report);
    Ok(report)
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
            report.push(Problem::new(
                Code::RestoreInterrupted,
                Some(name),
                format!(
                    "a snapshot restore was interrupted. Recover it with `wineo snapshot recover {name}`"
                ),
                None,
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
    let wine = check_wine(paths, global, &environment, report);
    check_targets(paths, global, &environment, &wine, report);
    check_runtime(paths, name, report)?;
    check_desktop(paths, global, &environment, &wine, report)?;
    check_art(paths, name, report);
    Ok(())
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
    let native = environment.config.wine().is_none();
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
        if environment
            .config
            .targets
            .values()
            .any(|target| target.desktop)
        {
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
    let pending = match (environment.config.wine(), &wine.prefix) {
        (None, _) => desktop::pending_native(paths, global, environment)?,
        (Some(_), Some(dir)) => {
            desktop::pending(paths, global, environment, wine.runner.as_ref(), dir)?
        }
        // No prefix to check against means no entries to compare either.
        (Some(_), None) => return Ok(()),
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

/// Makes one fix. It takes the same lock its command would, so a fix for a
/// busy environment fails with the usual busy error. Callers apply each fix
/// on its own, so one failing doesn't stop the rest.
///
/// `download` fetches a DXVK or vkd3d-proton build: the pinned version, or
/// the newest release for None. Downloading shows progress and can be
/// canceled, which is the front end's to do.
pub fn apply(
    paths: &Paths,
    fix: &Fix,
    download: impl FnOnce(Component, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    match fix {
        Fix::Download { component, version } => {
            let global = GlobalConfig::load_or_default(paths)?;
            let asked = version.as_deref().unwrap_or(LATEST);
            // Another environment's fix may have fetched it already.
            if Builds::new(paths, &global, *component)?
                .resolve(asked)
                .is_ok()
            {
                return Ok(());
            }
            download(*component, version.as_deref())
        }
        Fix::CleanUpLostRun { environment } => Ok(clean_up_lost_run(paths, environment)?),
        Fix::CleanUpInstaller { environment } => Ok(clean_up_installer(paths, environment)?),
        Fix::RemoveStateDir { environment } => Ok(remove_state_dir(paths, environment)?),
        Fix::RemoveDesktopEntry(entry) => Ok(remove_desktop_entry(paths, entry)?),
        Fix::RewriteDesktop { environment } => Ok(rewrite_desktop(paths, environment)?),
        Fix::DropArtRecord {
            environment,
            target,
            kind,
        } => {
            art::drop_missing_record(paths, environment, target.as_deref(), *kind)?;
            Ok(())
        }
        Fix::DropArtTarget {
            environment,
            target,
        } => {
            art::drop_unknown_target(paths, environment, target)?;
            Ok(())
        }
    }
}

/// What `wineo stop` does for a lost run, which is all there is to stop.
fn clean_up_lost_run(paths: &Paths, name: &str) -> Result<()> {
    Environment::ensure_exists(paths, name)?;
    if let Some(operation) = Locks::new(paths).busy_with(name)? {
        return Err(Error::Busy {
            subject: format!("environment `{name}`"),
            holder: operation.describe(),
        });
    }
    let stop_timeout =
        GlobalConfig::load(paths).map_or(DEFAULT_STOP_TIMEOUT, |global| global.stop_timeout);
    let runtime = Runtime::new(paths, name);
    let timeout = Duration::from_secs(u64::from(stop_timeout) + 10);
    match runtime.clean_up_lost_run(timeout, &paths.stats_log(name))? {
        LostRunCleanup::Unresolved { warning } => {
            let message = runtime.unresolved_run_message(name);
            Err(Error::Unsettled(match warning {
                Some(warning) => format!("{warning}. {message}"),
                None => message,
            }))
        }
        LostRunCleanup::NotLost | LostRunCleanup::Gone | LostRunCleanup::Killed => Ok(()),
    }
}

fn clean_up_installer(paths: &Paths, name: &str) -> Result<()> {
    Environment::ensure_exists(paths, name)?;
    let global = GlobalConfig::load_or_default(paths)?;
    let timeout = installer::clean_up_timeout(&global);
    match installer::clean_up(paths, &global, name, timeout)? {
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
    let Some(wine) = environment.config.wine() else {
        let _guard = locks.acquire(Operation::Launch, name, &[])?;
        Environment::ensure_exists(paths, name)?;
        desktop::reconcile_native(paths, &global, &environment)?;
        return Ok(());
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
    use std::ffi::OsString;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use jiff::Timestamp;

    use super::*;
    use crate::runtime::{OperationState, RunMode, RunState, RunStatus};

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
        fs::write(path, content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
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
                let result = apply(&f.paths, fix, no_download).map_err(|err| err.to_string());
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
}
