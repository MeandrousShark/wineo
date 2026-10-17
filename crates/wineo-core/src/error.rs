use std::io;
use std::path::PathBuf;

use crate::config::{Arch, Launcher};
use crate::operation::Operation;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// An environment that has no prefix of its own to work on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoPrefix {
    pub name: String,
    pub owner: PrefixOwner,
}

/// Who has the prefix an environment lacks, if anyone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixOwner {
    /// A native environment, which runs with no prefix at all.
    None,
    /// A proxied environment, whose launcher manages any prefix.
    Launcher(Launcher),
}

impl std::fmt::Display for NoPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.owner {
            PrefixOwner::None => write!(
                f,
                "{} is a native environment, so it has no prefix",
                self.name
            ),
            PrefixOwner::Launcher(launcher) => write!(
                f,
                "{} manages {}'s prefix.",
                launcher.display_name(),
                self.name
            ),
        }
    }
}

/// Printed under the parser's message for a key this wineo doesn't know.
pub const UNKNOWN_KEY_HINT: &str =
    "A newer version of wineo may have written it; update wineo or remove the key.";

/// Printed under an unsupported `schema_version`.
pub const NEWER_VERSION_HINT: &str =
    "A newer version of wineo wrote it; update wineo to use this file.";

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

    #[error("the portable folder path is empty")]
    EmptyPortable,

    #[error("portable prefixes must be directly in {}", .0.display())]
    PrefixOutsidePortable(PathBuf),

    #[error("a portable prefix_root must be {}", .0.display())]
    PortablePrefixRoot(PathBuf),

    #[error("desktop entries aren't supported for portable environments")]
    PortableDesktop,

    #[error("unrelated desktop shortcut at {} was not overwritten", .0.display())]
    DesktopEntryConflict(PathBuf),

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

    /// Not a `#[source]`, so the hint can follow the parser's message when the
    /// chain is printed on one line.
    #[error("invalid config {}: {}\n{UNKNOWN_KEY_HINT}", .path.display(), .detail.to_string().trim_end())]
    ConfigUnknownKey {
        path: PathBuf,
        detail: Box<toml::de::Error>,
    },

    #[error("invalid config {}", .path.display())]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: Box<toml_edit::TomlError>,
    },

    #[error("unsupported schema_version {found} in {}\n{NEWER_VERSION_HINT}", .path.display())]
    SchemaVersion { path: PathBuf, found: String },

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

    #[error("{}", busy_message(.subject, *.holder, .holder_label))]
    Busy {
        subject: String,
        holder: Option<Operation>,
        holder_label: &'static str,
    },

    #[error("could not inspect environments: {}", .0.join(", "))]
    UnreadableEnvironments(Vec<String>),

    #[error("{inner}'s prefix is inside {outer}'s prefix. Move one of them first.")]
    NestedPrefix { inner: String, outer: String },

    #[error("{} is already {owner}'s prefix. Pick another directory.", .path.display())]
    PrefixTaken { path: PathBuf, owner: String },

    #[error("{0} canceled")]
    Canceled(&'static str),

    #[error("{0} is already running")]
    AlreadyRunning(String),

    #[error("Launch canceled: the before hook exited with code {0}.")]
    BeforeHookFailed(i32),

    /// Keeps the underlying error's message and causes, so output matches
    /// what the transport reported.
    #[error(transparent)]
    Network(Box<dyn std::error::Error + Send + Sync>),

    #[error("environment `{0}` does not exist")]
    UnknownEnvironment(String),

    #[error("environment `{0}` changed before artwork could be written")]
    ArtEnvironmentChanged(String),

    #[error("environment `{0}` already exists")]
    EnvironmentExists(String),

    #[error("Proton prefixes are always win64, so --arch win32 can't be used with `{0}`")]
    ProtonWin32(String),

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

    #[error("{name} is used by {users}.")]
    BuildInUse { name: String, users: String },

    #[error("{}", unreadable_configs(.name, .errors))]
    UnreadableConfigs { name: String, errors: Vec<String> },

    #[error("{0} is not a managed build and cannot be removed")]
    UnmanagedBuild(String),

    #[error("{name} belongs to {launcher}.")]
    ExternalBuild {
        name: String,
        launcher: crate::external_runners::Source,
    },

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

    #[error("cannot resolve runner alias `{name}`: {reason}")]
    RunnerAlias { name: String, reason: String },

    #[error("`{name}` is not a wine build: {reason}")]
    NotARunner { name: String, reason: &'static str },

    #[error("invalid {component} name `{name}`: use letters, digits, `-`, `_`, and `.`")]
    InvalidBuildName {
        component: &'static str,
        name: String,
    },

    #[error(
        "{component} build `{name}` is not installed{}\nGet one with: wineo download {component}",
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
        "no {component} builds are installed, so `latest` has nothing to pick. Extract one into {}\nGet one with: wineo download {component}",
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

    #[error(
        "gamescope is on for `{0}`, but it was not found or is not installed.\nInstall gamescope, or turn it off with `wineo set {0} gamescope=false`"
    )]
    GamescopeMissing(String),

    #[error(
        "mangohud is on for `{0}`, but it was not found or is not installed.\nInstall MangoHud, or turn it off with `wineo set {0} mangohud=false`"
    )]
    MangohudMissing(String),

    #[error("wayland is on for {environment}, but {runner} doesn't support it")]
    WaylandUnsupported { environment: String, runner: String },

    #[error("{} is larger than 32 MiB", .0.display())]
    ArtTooLarge(PathBuf),

    #[error("{} is not a {accepted} image", .path.display())]
    ArtFormat { path: PathBuf, accepted: String },

    #[error("{} could not be decoded as an image", .path.display())]
    IconDecode {
        path: PathBuf,
        #[source]
        source: Box<image::ImageError>,
    },

    #[error("`{spot}` has no {kind} art")]
    NoArt {
        spot: String,
        kind: crate::art::Kind,
    },

    #[error("no SteamGridDB key, run `wineo art key` or set WINEO_STEAMGRIDDB_KEY")]
    NoSteamGridDbKey,

    #[error("the keyring failed: {0}")]
    Keyring(String),

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

    #[error("{0} is native, so its target can't be a Windows program")]
    NativeWindowsTarget(String),

    #[error("{0} is not executable. Run chmod +x on it.")]
    NotExecutable(String),

    #[error("invalid exe `{0}`: use an absolute path or one starting with ~/")]
    RelativeExe(String),

    #[error("`{0}` was not found on PATH")]
    NotOnPath(String),

    #[error("working directory `{0}` does not exist")]
    CwdNotFound(String),

    #[error("expected KEY=VALUE, got `{0}`")]
    InvalidEnvAssignment(String),

    #[error("{0}")]
    InvalidEnv(String),

    #[error("{0}")]
    NoPrefix(NoPrefix),

    #[error("{0} is already running (started from Steam).")]
    SteamAlreadyRunning(String),

    #[error("invalid prefix `{0}`: use an absolute path or one starting with ~/")]
    RelativePrefix(String),

    #[error("{} is not empty and is not a wine prefix", .0.display())]
    NotAPrefix(PathBuf),

    #[error("prefix {} is missing or is not a wine prefix", .0.display())]
    MissingPrefix(PathBuf),

    #[error("{} holds a {found} prefix, but runner {runner} needs a {needed} one", .path.display())]
    PrefixLayout {
        path: PathBuf,
        runner: String,
        found: &'static str,
        needed: &'static str,
    },

    #[error("prefix {} is missing, and so is {}", .prefix.display(), .parent.display())]
    PrefixParentMissing { prefix: PathBuf, parent: PathBuf },

    /// An empty prefix dir outside the prefix root could be an unmounted
    /// drive's mount point, so it is only built in once the user confirms.
    #[error("prefix {} is empty", .0.display())]
    EmptyPrefix(PathBuf),

    /// Same reason as `EmptyPrefix`: an empty parent could be the mount point.
    #[error("prefix {} is missing, and {} is empty", .prefix.display(), .parent.display())]
    EmptyPrefixParent { prefix: PathBuf, parent: PathBuf },

    #[error("prefix {} is {actual}, not {requested}", .path.display())]
    ArchMismatch {
        path: PathBuf,
        actual: Arch,
        requested: Arch,
    },

    #[error("{name} is running, stop it {action}")]
    Running { name: String, action: &'static str },

    #[error("{name} is already running target `{running}`, stop it before starting `{requested}`")]
    DifferentTargetRunning {
        name: String,
        running: String,
        requested: String,
    },

    #[error(
        "{0}'s last run was lost, so wine may still be running in its prefix. Clean it up with `wineo stop {0}` first"
    )]
    LostRun(String),

    #[error(
        "{0}'s last installer did not finish cleanly, so wine may still be running in its prefix. Clean it up with `wineo stop {0}` first"
    )]
    Unresolved(String),

    #[error("{0}'s prefix changed while this was being prepared, so nothing ran. Try again")]
    Changed(String),

    #[error("{0}")]
    StartFailed(String),

    #[error("still running after {0} seconds")]
    StillRunning(u64),

    /// Whatever was waited on could not be confirmed to have stopped.
    #[error("{0}")]
    Unsettled(String),

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

fn busy_message(subject: &str, holder: Option<Operation>, label: &str) -> String {
    if matches!(holder, Some(Operation::Hook(_))) {
        let subject = subject
            .strip_prefix("environment `")
            .and_then(|name| name.strip_suffix('`'))
            .unwrap_or(subject);
        format!("{subject} is busy {label}")
    } else {
        format!("{subject} is busy: {label}")
    }
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

/// A summary line, then each config's own error, which already names its file.
fn unreadable_configs(name: &str, errors: &[String]) -> String {
    let count = match errors {
        [_] => "a config".to_owned(),
        _ => format!("{} configs", errors.len()),
    };
    let mut text = format!("{name} is kept, since {count} can't be read");
    for error in errors {
        text.push('\n');
        text.push_str(error);
    }
    text
}

/// Collapses every run of whitespace, newlines included, to one space.
pub fn one_line(text: impl std::fmt::Display) -> String {
    text.to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
