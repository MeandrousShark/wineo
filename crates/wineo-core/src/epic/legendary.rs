//! Finding the Legendary to run, and running it. In order: `[epic]
//! legendary`, then wineo's own copy, then `legendary` on `PATH`. The
//! setting is used as given, and one that can't be used is an error, never
//! a reason to look further.

use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};

use super::run::{self, Output};
use super::{Env, Error, FLOOR, PYTHON_FLOOR, Version, own_copy};
use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::error::IoContext;
use crate::files;
use crate::paths::Paths;

/// For a call that asks Epic something.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// For `--version`, which asks nobody.
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);

/// For `python3 --version`.
const PYTHON_TIMEOUT: Duration = Duration::from_secs(10);

/// Written once into Legendary's state folder: no notice about a newer
/// Legendary, which isn't the user's to act on, and no aliases made up
/// for every game.
const CONFIG_INI: &str =
    "[Legendary]\ndisable_update_notice = true\ndisable_auto_aliasing = true\n";

/// Which of the three places the Legendary in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Setting,
    Own,
    Path,
}

#[derive(Debug, Clone)]
enum Program {
    /// Run as given.
    Direct(PathBuf),
    /// wineo's own copy, a zipapp run by the first `python3` on `PATH`.
    /// Never `python3 -I`, which would drop the file from Python's import
    /// path.
    Zipapp { python: PathBuf, file: PathBuf },
}

/// A Legendary that is new enough, ready to run.
pub struct Legendary {
    program: Program,
    version: Version,
    source: Source,
    paths: Paths,
    env: Env,
    timeout: Duration,
}

impl fmt::Debug for Legendary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Legendary")
            .field("program", &self.program)
            .field("version", &self.version)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// What looking for Legendary found.
#[derive(Debug)]
pub enum Found {
    Usable(Box<Legendary>),
    /// No setting, no copy of wineo's own, and nothing new enough on
    /// `PATH`, so wineo's own copy can be offered.
    Missing(Missing),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    /// The `legendary` on `PATH` that was passed over for being too old,
    /// to name in the offer.
    pub older_on_path: Option<(PathBuf, Version)>,
}

/// Looks for Legendary in order, with wineo's own environment.
pub fn find(paths: &Paths, global: &GlobalConfig, token: &Token) -> Result<Found, Error> {
    find_with(
        paths,
        global.epic.legendary.as_deref(),
        Env::current(),
        token,
    )
}

pub(crate) fn find_with(
    paths: &Paths,
    setting: Option<&str>,
    env: Env,
    token: &Token,
) -> Result<Found, Error> {
    if let Some(value) = setting {
        return from_setting(paths, value, env, token)
            .map(|legendary| Found::Usable(Box::new(legendary)));
    }
    let own = own_copy::file(paths, own_copy::PINNED.version);
    // Python that can't run wineo's copy leaves `PATH` to try, and is the
    // error only when that has nothing either.
    let mut python_failed = None;
    // Only a real file: a symlink there could name anything, unchecked.
    if fs::symlink_metadata(&own).is_ok_and(|meta| meta.file_type().is_file()) {
        match python_with(&env, token) {
            Ok(python) => {
                let version =
                    Version::parse(own_copy::PINNED.version).expect("the pinned version parses");
                return Ok(Found::Usable(Box::new(Legendary::new(
                    Program::Zipapp { python, file: own },
                    version,
                    Source::Own,
                    paths,
                    env,
                ))));
            }
            Err(error @ Error::Python) => python_failed = Some(error),
            Err(error) => return Err(error),
        }
    }
    let mut older_on_path = None;
    if let Some(program) = env.find_program("legendary") {
        let mut legendary = Legendary::new(
            Program::Direct(program.clone()),
            FLOOR,
            Source::Path,
            paths,
            env,
        );
        match legendary.ask_version(token) {
            Ok(version) if version >= FLOOR => {
                legendary.version = version;
                return Ok(Found::Usable(Box::new(legendary)));
            }
            Ok(version) => older_on_path = Some((program, version)),
            Err(error) if error.is_canceled() => return Err(error),
            // One that can't say its version is no use, and isn't the
            // user's choice the way the setting is.
            Err(_) => {}
        }
    }
    match python_failed {
        Some(error) => Err(error),
        None => Ok(Found::Missing(Missing { older_on_path })),
    }
}

fn from_setting(paths: &Paths, value: &str, env: Env, token: &Token) -> Result<Legendary, Error> {
    let shown = |path: Option<&Path>| match path {
        Some(path) => paths.contract_text(&path.display().to_string()),
        None => value.to_owned(),
    };
    let unusable = |path: Option<&Path>, reason: String| Error::Setting {
        shown: shown(path),
        reason,
    };
    if value.is_empty() {
        return Err(unusable(None, "it is empty".to_owned()));
    }
    let path = paths
        .config_path(value)
        .map_err(|error| unusable(None, error.to_string()))?;
    match fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(unusable(Some(&path), "there is nothing there".to_owned()));
        }
        Err(error) => return Err(unusable(Some(&path), error.to_string())),
        Ok(meta) if !meta.is_file() => {
            return Err(unusable(Some(&path), "it isn't a file".to_owned()));
        }
        Ok(_) if !crate::runner::is_executable(&path) => {
            return Err(unusable(Some(&path), "it isn't executable".to_owned()));
        }
        Ok(_) => {}
    }
    let mut legendary = Legendary::new(
        Program::Direct(path.clone()),
        FLOOR,
        Source::Setting,
        paths,
        env,
    );
    let version = match legendary.ask_version(token) {
        Ok(version) => version,
        Err(error) if error.is_canceled() => return Err(error),
        Err(error) => return Err(unusable(Some(&path), error.to_string())),
    };
    if version < FLOOR {
        return Err(Error::SettingTooOld {
            shown: shown(Some(&path)),
            found: version,
        });
    }
    legendary.version = version;
    Ok(legendary)
}

/// The first `python3` on `PATH`, when it is new enough to run wineo's own
/// copy of Legendary. Checked before that copy is offered.
pub fn python(token: &Token) -> Result<PathBuf, Error> {
    python_with(&Env::current(), token)
}

pub(crate) fn python_with(env: &Env, token: &Token) -> Result<PathBuf, Error> {
    let python = env.find_program("python3").ok_or(Error::Python)?;
    let mut command = Command::new(&python);
    command.arg("--version").env_clear().envs(env.for_python());
    let output = match run::run(command, "python3 --version", PYTHON_TIMEOUT, token, None) {
        Ok(output) => output,
        Err(error) if error.is_canceled() => return Err(error),
        Err(_) => return Err(Error::Python),
    };
    // Python 3 prints it on stdout, very old ones on stderr.
    let text = if output.stdout.trim().is_empty() {
        output.stderr
    } else {
        output.stdout
    };
    let version = text
        .trim()
        .strip_prefix("Python ")
        .and_then(Version::parse)
        .ok_or(Error::Python)?;
    if (version.major, version.minor) < PYTHON_FLOOR {
        return Err(Error::Python);
    }
    Ok(python)
}

impl Legendary {
    fn new(program: Program, version: Version, source: Source, paths: &Paths, env: Env) -> Self {
        Self {
            program,
            version,
            source,
            paths: paths.clone(),
            env,
            timeout: CALL_TIMEOUT,
        }
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// The Legendary file itself, for saying which one is in use.
    pub fn path(&self) -> &Path {
        match &self.program {
            Program::Direct(path) => path,
            Program::Zipapp { file, .. } => file,
        }
    }

    pub(crate) fn paths(&self) -> &Paths {
        &self.paths
    }

    #[cfg(test)]
    pub(crate) fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    fn ask_version(&self, token: &Token) -> Result<Version, Error> {
        let output = self.run_for(&["--version"], None, VERSION_TIMEOUT, token)?;
        parse_version(&output.stdout).ok_or_else(|| Error::Output {
            command: "--version".to_owned(),
            problem: "it didn't print a version".to_owned(),
        })
    }

    /// Runs Legendary with `args`, then `secret` as one more argument. Only
    /// the plain arguments ever name the command in an error.
    pub(crate) fn run(
        &self,
        args: &[&str],
        secret: Option<&str>,
        token: &Token,
    ) -> Result<Output, Error> {
        self.run_for(args, secret, self.timeout, token)
    }

    fn run_for(
        &self,
        args: &[&str],
        secret: Option<&str>,
        timeout: Duration,
        token: &Token,
    ) -> Result<Output, Error> {
        let state = super::state_dir(&self.paths);
        prepare_state(&state)?;
        super::private_dir(&super::cache_dir(&self.paths))?;
        let (mut command, _in_use) = match &self.program {
            Program::Direct(path) => (Command::new(path), None),
            Program::Zipapp { python, file } => {
                let in_use = InUse::hold(file)?;
                let mut command = Command::new(python);
                command.arg(file);
                (command, Some(in_use))
            }
        };
        command
            .args(args)
            .args(secret.map(OsStr::new))
            .env_clear()
            .envs(self.env.for_legendary(&self.paths))
            .current_dir(&state);
        run::run(command, &args.join(" "), timeout, token, secret)
    }
}

/// Legendary's `--version` line: `legendary version "0.21.1", codename
/// "Lowlife"`.
fn parse_version(stdout: &str) -> Option<Version> {
    let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
    let line = lines.next()?;
    if lines.next().is_some() {
        return None;
    }
    let rest = line.trim().strip_prefix("legendary version \"")?;
    let (version, _) = rest.split_once('"')?;
    Version::parse(version)
}

/// Makes Legendary's state folder and writes its `config.ini` the first
/// time. Legendary owns the file after that.
fn prepare_state(state: &Path) -> Result<(), Error> {
    super::private_dir(state)?;
    let config = state.join("config.ini");
    if !config.exists() {
        files::write_unique_private(&config, CONFIG_INI.as_bytes())
            .context(|| format!("failed to write {}", config.display()))?;
    }
    Ok(())
}

/// A shared lock on wineo's own copy while Legendary runs from it, since
/// Python reopens the file by path as it goes. Removing an older copy
/// takes it exclusively and leaves the copy alone when it can't.
struct InUse {
    /// The lock lives as long as this descriptor.
    _file: File,
}

impl InUse {
    fn hold(file: &Path) -> Result<Self, Error> {
        let opened = File::open(file).context(|| format!("failed to open {}", file.display()))?;
        flock(&opened, FlockOperation::LockShared)
            .map_err(std::io::Error::from)
            .context(|| format!("failed to lock {}", file.display()))?;
        Ok(Self { _file: opened })
    }
}

impl Env {
    /// Python's own variables cleared, as for Legendary.
    fn for_python(&self) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
        self.0
            .iter()
            .filter(|(name, _)| !name.as_encoded_bytes().starts_with(b"PYTHON"))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests;
