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

    #[error("invalid config {}: {message}", .path.display())]
    ConfigInvalid { path: PathBuf, message: String },

    #[error("environment `{0}` does not exist")]
    UnknownEnvironment(String),

    #[error("environment `{0}` already exists")]
    EnvironmentExists(String),

    #[error("runner `{0}` isn't supported yet, only `wine` (the system wine) works for now")]
    UnsupportedRunner(String),

    #[error("`{0}` was not found on PATH")]
    ProgramNotFound(&'static str),

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

    #[error("wine failed to create the prefix{}, full output in {}", .reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default(), .log.display())]
    PrefixCreation {
        reason: Option<String>,
        log: PathBuf,
    },
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
