//! The blocking half of the settings page: reading an environment for the
//! page, saving through `settings::apply`, and opening the config file.

use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use iced::Task;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::environment::Environment;
use wineo_core::install::Change;
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runner_alias;
use wineo_core::runners::Runners;
use wineo_core::settings::{self, Applied, CheckError, ConfigFile};

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
}

#[derive(Debug, Clone)]
pub(crate) enum Content {
    Form(Box<Loaded>),
    Error(String),
}

/// One read of an environment. `text` is the file as it was read, so a later
/// read can tell whether it changed.
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub(crate) text: Option<String>,
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
    let text = read_text(&paths.environment_config(name));
    let content = match content(paths, name) {
        Ok(loaded) => Content::Form(Box::new(loaded)),
        Err(error) => Content::Error(format!("{error:#}")),
    };
    Snapshot { text, content }
}

fn content(paths: &Paths, name: &str) -> anyhow::Result<Loaded> {
    let environment = Environment::load(paths, name)?;
    let global = GlobalConfig::load_or_default(paths)?;
    let inherited = Inherited {
        gamemode: global.gamemode,
        mangohud: global.mangohud,
        wayland: global.wayland,
    };
    let values = Values::from_config(&environment.config);
    let wine = environment.config.wine();
    let (runners, current) = match wine {
        Some(wine) => runner_choices(paths, &global, &wine.runner),
        None => (Vec::new(), None),
    };
    Ok(Loaded {
        native: wine.is_none(),
        arch: wine.map(|wine| wine.arch).unwrap_or_default(),
        values,
        inherited,
        runners,
        current,
    })
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

/// The same reason `wineo set` gives for crossing runner kinds.
pub(crate) fn kind_mismatch(name: &str, current: &str, changed: &str, kind: &str) -> String {
    format!(
        "`{name}` runs on {current} and `{changed}` is a {kind} runner. Their prefix layouts are not compatible, so make a new environment with `wineo new --runner {changed}` instead"
    )
}

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
            Some(CheckError::RunnerKindMismatch {
                current,
                changed,
                kind,
            }) => kind_mismatch(name, current, changed, kind),
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
                    text: None,
                    content: Content::Error(error),
                }),
            ))
        },
    )
}

pub(crate) fn check_task(path: PathBuf, generation: u64) -> Task<Message> {
    blocking(
        move || read_text(&path),
        move |result| {
            // A failed read is left for the next tick.
            match result {
                Ok(text) => Message::Settings(Event::Checked(generation, text)),
                Err(_) => Message::Settings(Event::CheckFailed(generation)),
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
