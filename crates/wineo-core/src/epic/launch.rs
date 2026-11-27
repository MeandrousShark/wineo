//! Starting an installed Epic game: asking Legendary what to run, once per
//! start. Its answer holds Epic's own arguments, which carry a one-use code
//! and the account's name and id, so they travel only as a `Secret`.
//!
//! When Epic can't be asked, a game marked to run offline still starts:
//! through Legendary's offline answer while logged in, and from wineo's
//! record otherwise, since Legendary can't answer at all while logged out.
//! A game that isn't marked refuses.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::account::{self, Status};
use super::game::{self, GameError, Record};
use super::legendary::{self, Found, Legendary};
use super::{EpicLock, Error};
use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::paths::Paths;

/// Epic's launch arguments. They go to the game's command line and to a
/// detached run's supervisor on its pipe, and nowhere else: `Debug` shows
/// none of them, and nothing saves them.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(Vec<String>);

impl Secret {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn reveal(&self) -> &[String] {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn new(arguments: &[&str]) -> Self {
        Self(
            arguments
                .iter()
                .map(|&argument| argument.to_owned())
                .collect(),
        )
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// What a start of an Epic game runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// The game's program, inside its folder. None keeps the target's own,
    /// for a record that names none.
    pub program: Option<PathBuf>,
    /// The game's own parameters, which go before the target's arguments.
    pub parameters: Vec<String>,
    /// Variables Legendary lists, which the environment's own override.
    pub environment: BTreeMap<String, String>,
    /// Epic's arguments, last on the command line.
    pub epic: Secret,
    /// Said before the start when Epic couldn't be asked.
    pub notice: Option<String>,
}

/// Why Epic couldn't be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Why {
    /// Logged in, but asking failed. Holds what Legendary said.
    NoAnswer(String),
    LoggedOut,
    /// No Legendary anywhere wineo looks.
    NoLegendary,
    NoPython,
    /// A Legendary that can't be run or can't say who is logged in.
    Unusable(String),
}

impl Why {
    fn notice(&self, name: &str) -> String {
        let why = match self {
            Self::NoAnswer(_) => "Epic didn't answer",
            Self::LoggedOut => "Not logged in to Epic",
            Self::NoLegendary => "Legendary isn't installed",
            Self::NoPython => "Legendary needs Python 3.10 or newer",
            Self::Unusable(_) => "Legendary can't be used",
        };
        format!("{why}, so {name} starts offline. Online features may not work.")
    }

    fn refusal(&self, name: &str) -> LaunchError {
        let unmarked = format!("{name} isn't marked to run offline");
        LaunchError::NeedsEpic(match self {
            Self::NoAnswer(detail) => format!("{unmarked}, and Epic didn't answer: {detail}"),
            Self::LoggedOut => format!(
                "{unmarked}, so it needs an Epic login. Log in with wineo import epic --login."
            ),
            Self::NoLegendary => format!(
                "{unmarked}, so it needs Legendary to start. Get it with wineo import epic --login."
            ),
            Self::NoPython => {
                format!("{unmarked}, and Epic needs Python 3.10 or newer to run Legendary.")
            }
            Self::Unusable(detail) => format!("{unmarked}, and Legendary can't be used: {detail}"),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// A game not marked to run offline, when Epic can't be asked.
    #[error("{0}")]
    NeedsEpic(String),
    #[error(
        "Legendary puts {name}'s game in {found}, not {folder}. Repair it with wineo update {name} --repair."
    )]
    Folder {
        name: String,
        found: String,
        folder: String,
    },
    #[error("Legendary names a program outside {name}'s game folder.")]
    Outside { name: String },
}

/// Asks Legendary what starting `app`, the game `name` owns, runs. Holds
/// the Epic lock while it asks, and never offers to download Legendary.
pub fn ask(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    app: &str,
    token: &Token,
) -> Result<Start> {
    let record = game::peek(paths, app)?.ok_or_else(|| GameError::RecordMissing {
        name: name.to_owned(),
        app: app.to_owned(),
    })?;
    ask_with(
        paths,
        name,
        &record,
        || legendary::find(paths, global, token),
        token,
    )
}

pub(crate) fn ask_with(
    paths: &Paths,
    name: &str,
    record: &Record,
    find: impl FnOnce() -> Result<Found, Error>,
    token: &Token,
) -> Result<Start> {
    let legendary = match find() {
        Ok(Found::Usable(legendary)) => legendary,
        Ok(Found::Missing(_)) => return plain(name, record, Why::NoLegendary),
        Err(error) if error.is_canceled() => return Err(canceled()),
        Err(Error::Python) => return plain(name, record, Why::NoPython),
        Err(error) => return plain(name, record, Why::Unusable(error.to_string())),
    };
    let _lock = EpicLock::acquire(paths)?;
    let error = match answer(&legendary, record, false, token) {
        Ok(raw) => return raw.start(name, record, None),
        Err(error) if error.is_canceled() => return Err(canceled()),
        Err(error) => error,
    };
    // A logout and a login Epic no longer takes look alike from here: the
    // second leaves Legendary logged out too.
    match account::status(&legendary, token) {
        Ok(Status::LoggedOut) => plain(name, record, Why::LoggedOut),
        Ok(Status::LoggedIn { .. }) => {
            let why = Why::NoAnswer(error.to_string());
            if !record.can_run_offline {
                return Err(why.refusal(name).into());
            }
            match answer(&legendary, record, true, token) {
                Ok(raw) => raw.start(name, record, Some(why.notice(name))),
                Err(error) if error.is_canceled() => Err(canceled()),
                Err(error) => Err(anyhow::Error::from(error).context(format!(
                    "Epic didn't answer, and {name} couldn't start offline"
                ))),
            }
        }
        Err(error) if error.is_canceled() => Err(canceled()),
        Err(error) => plain(name, record, Why::Unusable(error.to_string())),
    }
}

fn canceled() -> anyhow::Error {
    crate::Error::Canceled("launch").into()
}

/// A start from wineo's record alone, with no Epic arguments.
fn plain(name: &str, record: &Record, why: Why) -> Result<Start> {
    if !record.can_run_offline {
        return Err(why.refusal(name).into());
    }
    let (program, parameters) = match &record.offline {
        Some(offline) => (
            Some(record.folder.join(&offline.executable)),
            offline.arguments.clone(),
        ),
        None => (None, Vec::new()),
    };
    Ok(Start {
        program,
        parameters,
        environment: BTreeMap::new(),
        epic: Secret::default(),
        notice: Some(why.notice(name)),
    })
}

/// `legendary launch --json`, read. `--no-wine` keeps Legendary's own Wine
/// settings out of the answer, and `--skip-version-check` lets a game with
/// an update waiting start, which Legendary otherwise refuses.
fn answer(
    legendary: &Legendary,
    record: &Record,
    offline: bool,
    token: &Token,
) -> Result<Raw, Error> {
    let app = record.app_name.as_str();
    let mut args = vec!["launch", app, "--json", "--no-wine"];
    args.push(if offline {
        "--offline"
    } else {
        "--skip-version-check"
    });
    let output = legendary.run(&args, None, token)?;
    // serde's messages quote what they couldn't read, which can be Epic's
    // arguments, so none of it is passed on.
    serde_json::from_str(&output.stdout).map_err(|_| Error::Output {
        command: args.join(" "),
        problem: "it isn't the launch answer wineo expects".to_owned(),
    })
}

/// The parts of Legendary's answer wineo uses. Never `Debug`: it holds
/// Epic's arguments as plain strings.
#[derive(Deserialize)]
struct Raw {
    game_directory: PathBuf,
    game_executable: String,
    #[serde(default)]
    game_parameters: Vec<String>,
    #[serde(default)]
    egl_parameters: Vec<String>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
}

impl Raw {
    fn start(self, name: &str, record: &Record, notice: Option<String>) -> Result<Start> {
        if !super::install::same_folder(&self.game_directory, &record.folder) {
            return Err(LaunchError::Folder {
                name: name.to_owned(),
                found: self.game_directory.display().to_string(),
                folder: record.folder.display().to_string(),
            }
            .into());
        }
        let program =
            super::install::inside(&self.game_executable).ok_or_else(|| LaunchError::Outside {
                name: name.to_owned(),
            })?;
        Ok(Start {
            program: Some(record.folder.join(program)),
            parameters: self.game_parameters,
            environment: self.environment,
            epic: Secret(self.egl_parameters),
            notice,
        })
    }
}

/// Splits a game's launch parameters as Legendary does, with Python's
/// `shlex.split(text, posix=False)`: on whitespace, with a quote opening a
/// word running to the matching quote, both kept, and ending the word there.
/// A quote inside a word is an ordinary character, and a backslash always
/// is. None for a quote left open, which Legendary skips with a warning.
pub(crate) fn split_parameters(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| is_space(*c)).is_some() {}
        let Some(first) = chars.next() else {
            return Some(words);
        };
        let mut word = String::from(first);
        if first == '"' || first == '\'' {
            loop {
                let c = chars.next()?;
                word.push(c);
                if c == first {
                    break;
                }
            }
        } else {
            while let Some(c) = chars.next_if(|c| !is_space(*c)) {
                word.push(c);
            }
        }
        words.push(word);
    }
}

/// Python shlex's whitespace.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// Whether a launch of `target` is the game's own start, the one Epic is
/// asked about. Other targets start plainly.
pub fn is_main_target(target: Option<&str>) -> bool {
    target == Some(super::install::TARGET)
}

#[cfg(test)]
mod tests;
