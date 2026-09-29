//! The blocking half of the settings page: reading an environment for the
//! page, saving through `settings::apply`, and opening the config file.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use iced::Task;
use wineo_core::components::{Builds, Component};
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::environment::Environment;
use wineo_core::error::Error;
use wineo_core::install::Change;
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runner_alias;
use wineo_core::runners::Runners;
use wineo_core::settings::{self, Applied, CheckError, ConfigFile};
use wineo_core::{art, launch, prefix, programs, winpath};

use super::Event;
use super::form::{Edits, Inherited, Values};
use crate::Message;
use crate::panel::PanelError;

/// A runner the page offers. `proton` is `None` when it doesn't resolve, so
/// the kind can't be checked until save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunnerChoice {
    pub(crate) name: String,
    pub(crate) proton: Option<bool>,
}

impl fmt::Display for RunnerChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// The runner the environment is on now, as `check_runner` resolves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Current {
    pub(crate) name: String,
    pub(crate) proton: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Loaded {
    pub(crate) values: Values,
    pub(crate) native: bool,
    pub(crate) arch: Arch,
    pub(crate) inherited: Inherited,
    pub(crate) runners: Vec<RunnerChoice>,
    pub(crate) current: Option<Current>,
    /// Installed builds by name, empty on a native environment.
    pub(crate) dxvk_builds: Vec<String>,
    pub(crate) vkd3d_builds: Vec<String>,
    pub(crate) heroic_id: Option<String>,
    /// Set when the global config can't be read, so the page shows defaults.
    pub(crate) global_warning: Option<String>,
    /// Core refuses desktop entries in a portable folder.
    pub(crate) portable: bool,
    /// Each target with an art dir, and where it is, which removing the
    /// target deletes.
    pub(crate) target_art: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Content {
    Form(Box<Loaded>),
    Error(String),
}

/// The environment and global config files as read, so a later read can tell
/// whether either changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Texts {
    pub(crate) environment: Option<String>,
    pub(crate) global: Option<String>,
}

impl Texts {
    pub(crate) fn read(paths: &Paths, name: &str) -> Self {
        Self {
            environment: read_text(&paths.environment_config(name)),
            global: read_text(&paths.global_config()),
        }
    }
}

/// One read of an environment.
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub(crate) texts: Texts,
    pub(crate) content: Content,
}

pub(crate) fn read_text(path: &PathBuf) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        // Unreadable reads as a change, which reloads into the error page.
        Err(err) => Some(format!("\0{err}")),
    }
}

pub(crate) fn snapshot(paths: &Paths, name: &str) -> Snapshot {
    let texts = Texts::read(paths, name);
    let content = match content(paths, name) {
        Ok(loaded) => Content::Form(Box::new(loaded)),
        Err(error) => Content::Error(format!("{error:#}")),
    };
    Snapshot { texts, content }
}

fn content(paths: &Paths, name: &str) -> anyhow::Result<Loaded> {
    let environment = Environment::load(paths, name)?;
    // A broken global config shouldn't lock every environment's page, and
    // the tick reloads the page once it's fixed.
    let (global, global_warning) = match GlobalConfig::load(paths) {
        Ok(global) => (global, None),
        Err(_) => (
            GlobalConfig::default(),
            Some(format!(
                "{} can't be read, so global settings show their defaults",
                paths.global_config().display()
            )),
        ),
    };
    let inherited = Inherited {
        gamemode: global.gamemode,
        mangohud: global.mangohud,
        wayland: global.wayland,
    };
    let mut values = Values::from_config(&environment.config);
    let wine = environment.config.wine();
    let (runners, current) = match wine {
        Some(wine) => runner_choices(paths, &global, &wine.runner),
        None => (Vec::new(), None),
    };
    // A portable prefix is stored relative to a root the field can't show,
    // and a typed path is resolved from the working directory on save.
    if wine.is_some() {
        values.prefix = paths.shown(&values.prefix);
    }
    let builds = |component| match wine {
        Some(_) => installed_builds(paths, &global, component),
        None => Vec::new(),
    };
    Ok(Loaded {
        native: wine.is_none(),
        arch: wine.map(|wine| wine.arch).unwrap_or_default(),
        values,
        inherited,
        runners,
        current,
        dxvk_builds: builds(Component::Dxvk),
        vkd3d_builds: builds(Component::Vkd3d),
        heroic_id: environment.config.heroic_id.clone(),
        global_warning,
        portable: paths.portable_root().is_some(),
        target_art: target_art(paths, name, environment.config.targets.keys()),
    })
}

fn target_art<'a>(
    paths: &Paths,
    name: &str,
    targets: impl Iterator<Item = &'a String>,
) -> BTreeMap<String, String> {
    let art_dir = paths.art_dir(name);
    targets
        .filter_map(|target| {
            let dir = art::target_dir(&art_dir, target);
            dir.is_dir().then(|| {
                let shown = paths
                    .contract(&dir)
                    .unwrap_or_else(|_| dir.display().to_string());
                (target.clone(), shown)
            })
        })
        .collect()
}

/// A program the add target form offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgramChoice {
    pub(crate) name: String,
    pub(crate) exe: String,
}

/// What the add target form can pick from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scan {
    Scanning,
    Programs {
        programs: Vec<ProgramChoice>,
        /// Held back as noise, like uninstallers, the way `setup` does.
        held_back: Vec<ProgramChoice>,
    },
    /// The prefix doesn't exist yet, so only a typed path works.
    NoPrefix,
    Failed(String),
}

/// The environment's wine prefix, which is `pfx/` inside a Proton one, once
/// it has been made.
fn wine_prefix(paths: &Paths, environment: &Environment) -> Option<PathBuf> {
    environment
        .prefix(paths)
        .ok()
        .and_then(|dir| prefix::wine_prefix_in(&dir))
}

/// Lists what's installed in the prefix, the way `wineo setup` does.
pub(crate) fn scan(paths: &Paths, name: &str) -> Scan {
    let environment = match Environment::load(paths, name) {
        Ok(environment) => environment,
        Err(error) => return Scan::Failed(PanelError::from_error(&error.into()).text),
    };
    let Some(wine_prefix) = wine_prefix(paths, &environment) else {
        return Scan::NoPrefix;
    };
    let choices = |found: &[programs::Program]| {
        found
            .iter()
            .map(|program| ProgramChoice {
                name: program.name.clone(),
                exe: program.exe.clone(),
            })
            .collect()
    };
    match programs::scan(&wine_prefix) {
        Ok(listing) => Scan::Programs {
            programs: choices(&listing.programs),
            held_back: choices(&listing.filtered),
        },
        Err(error) => Scan::Failed(PanelError::from_error(&error.into()).text),
    }
}

/// A path as typed or pasted, which may come wrapped in quotes or with the
/// backslash a shell puts before a space. Matches `setup`.
pub(crate) fn clean_path(typed: &str) -> String {
    let typed = typed.trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|quote| typed.strip_prefix(*quote)?.strip_suffix(*quote))
        .unwrap_or(typed);
    if winpath::is_windows_path(unquoted) {
        unquoted.to_owned()
    } else {
        unquoted.replace("\\ ", " ")
    }
}

/// A typed exe as a target's `exe` holds it, checked the way `setup` checks
/// one: a Windows path as typed, a Unix path inside `drive_c` as its `C:`
/// path, and any other Unix path with the home directory as `~`. A native
/// environment takes a bare name on its PATH too. Unlike `setup`, a relative
/// path is refused, since the GUI's working directory means nothing to the
/// person typing it.
pub(crate) fn resolve_exe(paths: &Paths, name: &str, typed: &str) -> Result<String, String> {
    let typed = clean_path(typed);
    if typed.is_empty() {
        return Err("Enter the path to the program.".to_owned());
    }
    let environment = Environment::load(paths, name).map_err(|error| error.to_string())?;
    let bare = !typed.contains('/') && !typed.starts_with('~');
    let windows = winpath::is_windows_path(&typed);
    if !bare && !windows && !Path::new(&typed).is_absolute() && !typed.starts_with("~/") {
        return Err("The path must be absolute or start with ~/.".to_owned());
    }
    if environment.config.wine().is_none() {
        let global = GlobalConfig::load_or_default(paths).map_err(|error| error.to_string())?;
        let stored = if bare {
            typed
        } else {
            let unix = paths.absolute(&typed).map_err(|error| error.to_string())?;
            paths
                .store(&unix)
                .unwrap_or_else(|_| unix.display().to_string())
        };
        launch::check_native_exe(paths, &global, name, &environment.config.env, &stored)
            .map_err(|error| error.to_string())?;
        return Ok(stored);
    }
    let missing = || format!("`{typed}` does not exist.");
    let wine_prefix = wine_prefix(paths, &environment);
    if windows {
        let wine_prefix = wine_prefix.as_deref().ok_or_else(missing)?;
        return match winpath::to_unix(wine_prefix, &typed) {
            Some(unix) if unix.is_file() => Ok(typed),
            _ => Err(missing()),
        };
    }
    let unix = paths.absolute(&typed).map_err(|error| error.to_string())?;
    if !unix.is_file() {
        return Err(missing());
    }
    Ok(wine_prefix
        .as_deref()
        .and_then(|wine_prefix| winpath::from_unix(wine_prefix, &unix))
        .or_else(|| paths.store(&unix).ok())
        .unwrap_or_else(|| unix.display().to_string()))
}

/// The system wine, installed runners, and aliases, in `wineo runners` order,
/// plus the configured runner when it's none of those.
fn runner_choices(
    paths: &Paths,
    global: &GlobalConfig,
    configured: &str,
) -> (Vec<RunnerChoice>, Option<Current>) {
    let Ok(runners) = Runners::new(paths, global) else {
        return (
            vec![RunnerChoice {
                name: configured.to_owned(),
                proton: None,
            }],
            None,
        );
    };
    let mut choices = vec![RunnerChoice {
        name: Runner::SYSTEM.to_owned(),
        proton: Runner::system().ok().map(|runner| runner.is_proton()),
    }];
    if let Ok(listing) = runners.list() {
        choices.extend(listing.runners.iter().map(|runner| RunnerChoice {
            name: runner.name().to_owned(),
            proton: Some(runner.is_proton()),
        }));
    }
    for alias in runner_alias::ALIASES {
        choices.push(RunnerChoice {
            name: alias.to_owned(),
            proton: runners.resolve(alias).ok().map(|runner| runner.is_proton()),
        });
    }
    if !choices.iter().any(|choice| choice.name == configured) {
        choices.push(RunnerChoice {
            name: configured.to_owned(),
            proton: runners
                .resolve(configured)
                .ok()
                .map(|runner| runner.is_proton()),
        });
    }
    let current = runners.resolve(configured).ok().map(|runner| Current {
        name: runner.name().to_owned(),
        proton: runner.is_proton(),
    });
    (choices, current)
}

/// The builds `wineo dxvk` or `wineo vkd3d` lists. An unreadable root offers
/// none, and save reports the problem if one is picked anyway.
fn installed_builds(paths: &Paths, global: &GlobalConfig, component: Component) -> Vec<String> {
    Builds::new(paths, global, component)
        .and_then(|builds| builds.list())
        .map(|listing| {
            listing
                .builds
                .iter()
                .map(|build| build.name().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Why a component can't be set on this environment's runner, in the words
/// core refuses it with.
pub(crate) fn component_refusal(
    name: &str,
    current: Option<&Current>,
    component: Component,
) -> Option<String> {
    let current = current.filter(|current| current.proton)?;
    Some(
        Error::ComponentOnProton {
            component: component.as_str(),
            environment: name.to_owned(),
            runner: current.name.clone(),
        }
        .to_string(),
    )
}

pub(crate) const KIND_MISMATCH: &str =
    "Can't switch between Wine and Proton runners. Make a new environment instead.";

/// What a save did, worded for the bar. `failed` came after `lines` had
/// already happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) lines: Vec<String>,
    pub(crate) failed: Option<String>,
}

pub(crate) fn report(name: &str, applied: Applied) -> Report {
    let mut lines: Vec<String> = applied
        .components
        .iter()
        .map(|change| match change {
            Change::Installed { component, build } => {
                format!("Installed {} {build}", component.label())
            }
            Change::Replaced {
                component,
                from,
                to,
            } => format!("Replaced {} {from} with {to}", component.label()),
            Change::Removed { component, build } => {
                format!("Removed {} {build}", component.label())
            }
        })
        .collect();
    lines.extend(applied.desktop.iter().map(|change| {
        use wineo_core::desktop::Change;
        match change {
            Change::Wrote { target, icon } => {
                let icon = if *icon { "" } else { ", with no icon" };
                format!("Added a desktop entry for {target}{icon}")
            }
            Change::Updated { target } => format!("Updated the desktop entry for {target}"),
            Change::Removed { target } => format!("Removed the desktop entry for {target}"),
            Change::IconArtUnreadable { target } => {
                format!("Icon art for {target} could not be read")
            }
            Change::Skipped { target, reason } => {
                format!("No desktop entry for {target}: {reason}")
            }
        }
    }));
    if let Some(runner) = &applied.changed_runner {
        lines.push(format!(
            "The prefix was not migrated. If it was set up by a newer wine than `{runner}`, it may not work"
        ));
    }
    if applied.running {
        lines.push(format!(
            "{name} is running, changes apply the next time it starts"
        ));
    }
    Report {
        lines,
        failed: applied
            .failed
            .map(|error| PanelError::from_error(&error).text),
    }
}

pub(crate) fn save(paths: &Paths, name: &str, edits: &Edits) -> Result<Report, String> {
    let pairs: Vec<(&str, &str)> = edits
        .pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let unset: Vec<&str> = edits.unset.iter().map(String::as_str).collect();
    match settings::apply(paths, ConfigFile::Environment(name), &pairs, &unset) {
        Ok(applied) => Ok(report(name, applied)),
        Err(error) => Err(match error.downcast_ref::<CheckError>() {
            Some(CheckError::RunnerKindMismatch { .. }) => KIND_MISMATCH.to_owned(),
            None => PanelError::from_error(&error).text,
        }),
    }
}

fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(Result<T, String>) -> Message + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(work)
                .await
                .map_err(|error| error.to_string())
        },
        done,
    )
}

pub(crate) fn load_task(paths: Paths, name: String, generation: u64) -> Task<Message> {
    blocking(
        move || snapshot(&paths, &name),
        move |result| {
            Message::Settings(Event::Loaded(
                generation,
                result.unwrap_or_else(|error| Snapshot {
                    texts: Texts::default(),
                    content: Content::Error(error),
                }),
            ))
        },
    )
}

pub(crate) fn check_task(
    paths: Paths,
    name: String,
    generation: u64,
    sequence: u64,
) -> Task<Message> {
    blocking(
        move || Texts::read(&paths, &name),
        move |result| {
            // A failed read is left for the next tick.
            match result {
                Ok(texts) => Message::Settings(Event::Checked(generation, sequence, texts)),
                Err(_) => Message::Settings(Event::CheckFailed(generation, sequence)),
            }
        },
    )
}

pub(crate) fn save_task(
    paths: Paths,
    name: String,
    edits: Edits,
    generation: u64,
) -> Task<Message> {
    blocking(
        move || save(&paths, &name, &edits),
        move |result| Message::Settings(Event::Saved(generation, result.and_then(|saved| saved))),
    )
}

pub(crate) fn open_task(path: PathBuf) -> Task<Message> {
    blocking(
        move || {
            // Waited on here, so the child is reaped rather than left a zombie.
            std::process::Command::new("xdg-open")
                .arg(&path)
                .status()
                .map_err(|error| format!("Failed to run xdg-open: {error}"))
                .and_then(|status| {
                    if status.success() {
                        Ok(())
                    } else {
                        Err(format!("xdg-open failed: {status}"))
                    }
                })
        },
        |result| Message::Settings(Event::Opened(result.and_then(|opened| opened))),
    )
}

pub(crate) fn scan_task(paths: Paths, name: String, generation: u64) -> Task<Message> {
    blocking(
        move || scan(&paths, &name),
        move |result| {
            Message::Settings(Event::Scanned(
                generation,
                result.unwrap_or_else(Scan::Failed),
            ))
        },
    )
}

pub(crate) fn resolve_task(
    paths: Paths,
    name: String,
    typed: String,
    generation: u64,
    sequence: u64,
) -> Task<Message> {
    blocking(
        move || resolve_exe(&paths, &name, &typed),
        move |result| {
            Message::Settings(Event::Resolved(
                generation,
                sequence,
                result.and_then(|resolved| resolved),
            ))
        },
    )
}
