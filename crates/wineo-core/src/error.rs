use std::io;
use std::path::PathBuf;

use crate::config::Arch;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{context}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },

    #[error("HOME is not set to an absolute path")]
    NoHome,

    #[error("{} is not valid UTF-8, which config files require", .0.display())]
    NonUtf8Path(PathBuf),

    #[error("invalid name `{0}`: use letters, digits, `-`, and `_`")]
    InvalidName(String),

    #[error("invalid config {}", .path.display())]
    ConfigSyntax {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("invalid config {}", .path.display())]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: Box<toml_edit::TomlError>,
    },

    #[error(transparent)]
    Edit(#[from] crate::edit::EditError),

    #[error("invalid config {}: {message}", .path.display())]
    ConfigInvalid { path: PathBuf, message: String },

    #[error("invalid state file {}", .path.display())]
    StateSyntax {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("{subject} is busy: {holder}")]
    Busy {
        subject: String,
        holder: &'static str,
    },

    #[error("environment `{0}` does not exist")]
    UnknownEnvironment(String),

    #[error("environment `{0}` already exists")]
    EnvironmentExists(String),

    #[error(
        "there is no template `{name}`{}",
        if .available.is_empty() {
            format!(", and none are installed in {}", .dir.display())
        } else {
            format!(", the templates are: {}", .available.join(", "))
        }
    )]
    UnknownTemplate {
        name: String,
        available: Vec<String>,
        dir: PathBuf,
    },

    #[error("invalid runner name `{0}`: use letters, digits, `-`, `_`, and `.`")]
    InvalidRunnerName(String),

    #[error(
        "runner `{name}` is not installed{}",
        if .installed.is_empty() {
            format!(", and nothing is installed in {}", .root.display())
        } else {
            format!(", installed runners are: {}", .installed.join(", "))
        }
    )]
    UnknownRunner {
        name: String,
        installed: Vec<String>,
        root: PathBuf,
    },

    #[error("`{name}` is not a wine build: {reason}")]
    NotARunner { name: String, reason: &'static str },

    #[error("invalid {component} name `{name}`: use letters, digits, `-`, `_`, and `.`")]
    InvalidBuildName {
        component: &'static str,
        name: String,
    },

    #[error(
        "{component} build `{name}` is not installed{}",
        if .installed.is_empty() {
            format!(", and nothing is installed in {}", .root.display())
        } else {
            format!(", installed builds are: {}", .installed.join(", "))
        }
    )]
    UnknownBuild {
        component: &'static str,
        name: String,
        installed: Vec<String>,
        root: PathBuf,
    },

    #[error("`{name}` is not a {component} build: {reason}")]
    NotABuild {
        component: &'static str,
        name: String,
        reason: String,
    },

    #[error(
        "no {component} builds are installed, so `latest` has nothing to pick. Extract one into {}",
        .root.display()
    )]
    NoBuilds {
        component: &'static str,
        root: PathBuf,
    },

    #[error("`{0}` was not found on PATH")]
    ProgramNotFound(&'static str),

    #[error(
        "gamemode is on for `{0}`, but it was not found or is not installed.\nInstall gamemode, or turn it off with `wineo set {0} gamemode=false`"
    )]
    GamemodeMissing(String),

    #[error("environment `{environment}` has no target `{target}`")]
    UnknownTarget { environment: String, target: String },

    #[error(
        "environment `{0}` has no targets, add one with `wineo set {0} targets.<target>.exe=<path>`"
    )]
    NoTargets(String),

    #[error(
        "environment `{environment}` has several targets and none is the default, pick one with --target: {}",
        .targets.join(", ")
    )]
    AmbiguousTarget {
        environment: String,
        targets: Vec<String>,
    },

    #[error("`{0}` does not exist")]
    ExeNotFound(String),

    #[error("working directory `{0}` does not exist")]
    CwdNotFound(String),

    #[error("expected KEY=VALUE, got `{0}`")]
    InvalidEnvAssignment(String),

    #[error("{0}")]
    InvalidEnv(String),

    #[error("{} is not empty and is not a wine prefix", .0.display())]
    NotAPrefix(PathBuf),

    #[error("prefix {} is missing or is not a wine prefix", .0.display())]
    MissingPrefix(PathBuf),

    #[error("prefix {} is {actual}, not {requested}", .path.display())]
    ArchMismatch {
        path: PathBuf,
        actual: Arch,
        requested: Arch,
    },

    #[error("{0}")]
    StartFailed(String),

    #[error("still running after {0} seconds")]
    StillRunning(u64),

    #[error("wine failed to create the prefix{}, full output in {}", .reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default(), .log.display())]
    PrefixCreation {
        reason: Option<String>,
        log: PathBuf,
    },

    #[error("wine failed to write the menu builder override, full output in {}", .log.display())]
    MenuBuilderOverride { log: PathBuf },

    #[error("wine failed to set the `{0}` DLL override")]
    DllOverride(String),

    #[error(
        "`{component}` only means something to wine, and `{environment}` runs on {runner}, which bundles its own"
    )]
    ComponentOnProton {
        component: &'static str,
        environment: String,
        runner: String,
    },

    #[error(
        "vkd3d-proton uses DXVK's DXGI rather than shipping its own, so `vkd3d` needs `dxvk` set as well. Set one with `wineo set {0} dxvk=latest`"
    )]
    Vkd3dNeedsDxvk(String),
}

/// Formats an error followed by its sources, like anyhow's `{:#}`.
pub fn chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(err) = source {
        text.push_str(": ");
        text.push_str(&err.to_string());
        source = err.source();
    }
    text
}

pub(crate) trait IoContext<T> {
    fn context(self, context: impl FnOnce() -> String) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn context(self, context: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|source| Error::Io {
            context: context(),
            source,
        })
    }
}
