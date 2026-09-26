use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{GlobalConfig, Kind, Target, validate_env_var};
use crate::environment::Environment;
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::prefix;
use crate::process::GroupOwnership;
use crate::runner::{self, Runner, with_menu_builder_disabled};
use crate::runners::Runners;
use crate::winpath;

/// The binary gamemode ships, which is not what the setting is called.
pub(crate) const GAMEMODE: &str = "gamemoderun";
/// What wineo hands umu itself on a Proton launch, so none of it can be
/// overridden from a config or the command line.
const MANAGED: [&str; 3] = ["PROTONPATH", "GAMEID", "STORE"];
/// Endings that mark a Windows program, which a native environment can't run.
const WINDOWS_EXTENSIONS: [&str; 4] = [".exe", ".msi", ".bat", ".lnk"];

/// Command line overrides for launching a target.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Overrides {
    pub target: Option<String>,
    pub env: BTreeMap<String, String>,
    pub args: Vec<String>,
}

/// What a launch runs on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Host {
    /// A Windows program, through wine or umu, in `prefix`.
    Wine { runner: Runner, prefix: PathBuf },
    /// A Linux program, run directly. Everything it starts shares its process
    /// group, which is how the run is tracked and ended.
    Native { exe: PathBuf },
}

impl Host {
    pub fn runner(&self) -> Option<&Runner> {
        match self {
            Host::Wine { runner, .. } => Some(runner),
            Host::Native { .. } => None,
        }
    }

    pub fn prefix(&self) -> Option<&Path> {
        match self {
            Host::Wine { prefix, .. } => Some(prefix),
            Host::Native { .. } => None,
        }
    }

    /// What is spawned, for naming it in an error.
    pub fn launcher(&self) -> String {
        match self {
            Host::Wine { runner, .. } => runner.launcher().to_owned(),
            Host::Native { exe } => exe.display().to_string(),
        }
    }

    /// Whether other processes can outlive the launched program. A native
    /// launcher can hand off to the game and exit, like a wine one.
    pub fn outlives_program(&self) -> bool {
        match self {
            Host::Wine { runner, .. } => runner.outlives_program(),
            Host::Native { .. } => true,
        }
    }

    /// Waits until the run is over: the prefix is idle, or for a native run,
    /// its whole process group is gone.
    pub(crate) fn wait_idle(
        &self,
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Result<()> {
        match self {
            Host::Wine { runner, prefix } => runner.wait_idle(prefix),
            Host::Native { .. } => runner::wait_for_live_group(group, boot_id, leader_start),
        }
    }

    pub(crate) fn wait_settled_live(
        &self,
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Result<()> {
        match self {
            Host::Wine { runner, prefix } => {
                runner.wait_settled_live(prefix, group, boot_id, leader_start)
            }
            Host::Native { .. } => runner::wait_for_live_group(group, boot_id, leader_start),
        }
    }

    /// Asks programs to close. A native run gets SIGTERM to its group.
    pub(crate) fn end_session_live(
        &self,
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Result<GroupOwnership> {
        match self {
            Host::Wine { runner, prefix } => {
                runner.end_session_live(prefix, group, boot_id, leader_start)
            }
            Host::Native { .. } => runner::signal_live_group(
                group,
                boot_id,
                leader_start,
                rustix::process::Signal::TERM,
            ),
        }
    }

    /// Kills whatever is left. A native run gets SIGKILL to its group.
    pub(crate) fn kill_live(
        &self,
        group: Option<u32>,
        boot_id: Option<&str>,
        leader_start: Option<u64>,
    ) -> Result<GroupOwnership> {
        match self {
            Host::Wine { runner, prefix } => runner.kill_live(prefix, group, boot_id, leader_start),
            Host::Native { .. } => runner::signal_live_group(
                group,
                boot_id,
                leader_start,
                rustix::process::Signal::KILL,
            ),
        }
    }
}

/// A fully resolved program launch inside an environment.
#[derive(Debug, Serialize, Deserialize)]
pub struct Launch {
    pub host: Host,
    pub target: Option<String>,
    /// Everything passed to `wine`, or to a native program.
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Merged from the global config, the environment, and the overrides.
    pub env: BTreeMap<String, String>,
    /// The resolved `gamemoderun`, when this launch asked for gamemode. The
    /// path is kept rather than the flag, so a detached run cannot end up
    /// resolving a different one from a different PATH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamemode: Option<PathBuf>,
    /// What umu looks protonfixes up by. Only a game launch carries these: a
    /// tool has no reason to trigger a game's fix script, which can install
    /// things. Heroic draws the same line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gameid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

impl Launch {
    pub fn target(
        paths: &Paths,
        global: &GlobalConfig,
        environment: &Environment,
        overrides: &Overrides,
    ) -> Result<Self> {
        let (name, target) = resolve_target(environment, overrides.target.as_deref())?;
        let Kind::Wine(wine) = &environment.config.kind else {
            return native_target(paths, global, environment, name, target, overrides);
        };
        let (runner, prefix) = open(paths, global, environment)?;
        // Windows paths and drive_c live in the wine prefix, which is `pfx/`
        // under a Proton environment's configured one.
        let wine_prefix = runner.wine_prefix(&prefix);
        let program = resolve_program(paths, &wine_prefix, &target.exe, false)?;
        let cwd = match &target.cwd {
            Some(cwd) => resolve_cwd(paths, &wine_prefix, cwd)?,
            None => program.default_cwd(&wine_prefix),
        };
        let mut args = program.wine_args;
        args.extend(target.args.iter().cloned());
        args.extend(overrides.args.iter().cloned());
        // Resolved here rather than at spawn time, so `start` refuses before it
        // hands anything to a supervisor.
        let gamemode = gamemode_path(global, environment, target)?;
        Ok(Self {
            env: merge_env(&runner, global, environment, &overrides.env)?,
            gameid: runner.is_proton().then(|| wine.gameid.clone()).flatten(),
            store: runner.is_proton().then(|| wine.store.clone()).flatten(),
            host: Host::Wine { runner, prefix },
            target: Some(name.to_owned()),
            args,
            cwd,
            gamemode,
        })
    }

    /// What `target` checks that doesn't need the prefix, so a typo or a bad
    /// config fails before a missing prefix is built. A native launch needs no
    /// prefix or runner, so it is checked in full and `runner` is unused.
    pub fn check_target(
        paths: &Paths,
        global: &GlobalConfig,
        environment: &Environment,
        runner: &Runner,
        overrides: &Overrides,
    ) -> Result<()> {
        if environment.config.wine().is_none() {
            return Self::check_native_target(paths, global, environment, overrides);
        }
        let (_, target) = resolve_target(environment, overrides.target.as_deref())?;
        check_unix_program(paths, &target.exe, false)?;
        if let Some(cwd) = &target.cwd
            && !winpath::is_windows_path(cwd)
            && !paths.config_path(cwd)?.is_dir()
        {
            return Err(Error::CwdNotFound(cwd.to_owned()));
        }
        gamemode_path(global, environment, target)?;
        merge_env(runner, global, environment, &overrides.env)?;
        Ok(())
    }

    /// Everything `target` checks for a native environment, which is all of it.
    pub fn check_native_target(
        paths: &Paths,
        global: &GlobalConfig,
        environment: &Environment,
        overrides: &Overrides,
    ) -> Result<()> {
        Self::target(paths, global, environment, overrides).map(|_| ())
    }

    /// What `exec` checks that doesn't need the prefix, like `check_target`.
    pub fn check_exec(
        paths: &Paths,
        global: &GlobalConfig,
        environment: &Environment,
        runner: &Runner,
        program: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<()> {
        check_unix_program(paths, program, true)?;
        merge_env(runner, global, environment, env)?;
        Ok(())
    }

    /// Runs an arbitrary program. Relative paths are resolved against the
    /// current directory, since that's where the user typed them.
    pub fn exec(
        paths: &Paths,
        global: &GlobalConfig,
        environment: &Environment,
        program: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<Self> {
        let (runner, prefix) = open(paths, global, environment)?;
        let wine_prefix = runner.wine_prefix(&prefix);
        let program = resolve_program(paths, &wine_prefix, program, true)?;
        let cwd = program.default_cwd(&wine_prefix);
        let mut wine_args = program.wine_args;
        wine_args.extend(args.iter().cloned());
        Ok(Self {
            env: merge_env(&runner, global, environment, env)?,
            host: Host::Wine { runner, prefix },
            target: None,
            args: wine_args,
            cwd,
            // Running a tool in the prefix is not playing a game, so neither
            // gamemode nor the game's protonfixes apply.
            gamemode: None,
            gameid: None,
            store: None,
        })
    }

    pub fn command(&self) -> Command {
        let mut command = match &self.host {
            Host::Wine { runner, prefix } => runner
                .command(prefix, self.gamemode.as_deref())
                .expect("the runner was resolved when this launch was built"),
            Host::Native { exe } => runner::wrapped(exe, self.gamemode.as_deref()),
        };
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .envs(&self.env);
        // Left unset when this launch has none, so umu falls back to
        // `umu-default` and applies no fixes.
        if let Some(gameid) = &self.gameid {
            command.env("GAMEID", gameid);
        }
        if let Some(store) = &self.store {
            command.env("STORE", store);
        }
        command
    }
}

/// Whether a launch runs under gamemode: the target's setting wins over the
/// environment's, which wins over the global one, and unset everywhere is off.
pub fn gamemode(global: &GlobalConfig, environment: &Environment, target: Option<&Target>) -> bool {
    target
        .and_then(|target| target.gamemode)
        .or(environment.config.gamemode)
        .or(global.gamemode)
        .unwrap_or(false)
}

fn gamemode_path(
    global: &GlobalConfig,
    environment: &Environment,
    target: &Target,
) -> Result<Option<PathBuf>> {
    if !gamemode(global, environment, Some(target)) {
        return Ok(None);
    }
    runner::find_on_path(GAMEMODE)
        .map(Some)
        .ok_or_else(|| Error::GamemodeMissing(environment.name.clone()))
}

/// Parses a `-e KEY=VALUE` flag for a command that always runs in a prefix.
pub fn parse_env_assignment(assignment: &str) -> Result<(String, String)> {
    let (key, value) = parse_launch_env_assignment(assignment)?;
    check_override(true, &key, &value)?;
    Ok((key, value))
}

/// Parses a `-e KEY=VALUE` flag for launching a target. `WINEPREFIX` is only
/// refused once the launch turns out to need a prefix.
pub fn parse_launch_env_assignment(assignment: &str) -> Result<(String, String)> {
    let (key, value) = assignment
        .split_once('=')
        .ok_or_else(|| Error::InvalidEnvAssignment(assignment.to_owned()))?;
    check_override(false, key, value)?;
    Ok((key.to_owned(), value.to_owned()))
}

fn check_override(wine: bool, key: &str, value: &str) -> Result<()> {
    if wine && key == "WINEPREFIX" {
        return Err(managed(key));
    }
    validate_env_var(key, value).map_err(Error::InvalidEnv)
}

fn managed(key: &str) -> Error {
    Error::InvalidEnv(format!("{key} is managed by wineo and can't be overridden"))
}

fn open(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> Result<(Runner, PathBuf)> {
    let wine = environment.wine()?;
    let prefix = crate::paths::resolve(&environment.prefix(paths)?);
    let runner = Runners::new(paths, global)?.resolve(&wine.runner)?;
    // Wine would silently create a fresh prefix here otherwise.
    if !prefix::is_prefix(&runner.wine_prefix(&prefix)) {
        return Err(Error::MissingPrefix(prefix));
    }
    Ok((runner, prefix))
}

/// A native target: its exe run directly, with no prefix and no runner.
fn native_target(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
    name: &str,
    target: &Target,
    overrides: &Overrides,
) -> Result<Launch> {
    let env = merge(None, global, environment, &overrides.env)?;
    let (exe, default_cwd) = resolve_native_exe(
        paths,
        &environment.name,
        &target.exe,
        &native_path_var(&env),
    )?;
    let cwd = match &target.cwd {
        Some(cwd) => Some(paths.config_path(cwd)?)
            .filter(|dir| dir.is_dir())
            .ok_or_else(|| Error::CwdNotFound(cwd.to_owned()))?,
        None => default_cwd,
    };
    let mut args = target.args.clone();
    args.extend(overrides.args.iter().cloned());
    let gamemode = gamemode_path(global, environment, target)?;
    Ok(Launch {
        env,
        host: Host::Native { exe },
        target: Some(name.to_owned()),
        args,
        cwd,
        gamemode,
        gameid: None,
        store: None,
    })
}

/// Checks a native target's exe as it is written, resolving it the way a
/// launch would, on the PATH the global and environment `[env]` give it.
pub fn check_native_exe(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &str,
    env: &BTreeMap<String, String>,
    exe: &str,
) -> Result<PathBuf> {
    let mut merged = global.env.clone();
    merged.extend(env.clone());
    resolve_native_exe(paths, environment, exe, &native_path_var(&merged)).map(|(exe, _)| exe)
}

/// The PATH a native program gets: its own from `env`, or else wineo's, which
/// it inherits.
pub fn native_path_var(env: &BTreeMap<String, String>) -> OsString {
    match env.get("PATH") {
        Some(path) => OsString::from(path),
        None => std::env::var_os("PATH").unwrap_or_default(),
    }
}

/// Where a native target's exe is, and the directory it runs in unless the
/// target says otherwise: the exe's own, or home for one found on PATH.
pub fn resolve_native_exe(
    paths: &Paths,
    environment: &str,
    exe: &str,
    path_var: &OsStr,
) -> Result<(PathBuf, PathBuf)> {
    let lower = exe.to_ascii_lowercase();
    if winpath::is_windows_path(exe) || WINDOWS_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
        return Err(Error::NativeWindowsTarget(environment.to_owned()));
    }
    if !exe.contains('/') && !exe.starts_with('~') {
        let found =
            native_on_path(path_var, exe).ok_or_else(|| Error::NotOnPath(exe.to_owned()))?;
        return Ok((found, paths.expand("~")));
    }
    // Outside a portable folder a relative path would depend on where wineo
    // happened to run from, as a relative prefix would.
    if paths.portable_root().is_none() && !paths.expand(exe).is_absolute() {
        return Err(Error::RelativeExe(exe.to_owned()));
    }
    let path = paths.config_path(exe)?;
    if !path.is_file() {
        return Err(Error::ExeNotFound(exe.to_owned()));
    }
    if !runner::is_executable(&path) {
        let shown = paths
            .contract(&path)
            .unwrap_or_else(|_| path.display().to_string());
        return Err(Error::NotExecutable(shown));
    }
    let dir = path
        .parent()
        .map_or_else(|| paths.expand("~"), Path::to_owned);
    Ok((path, dir))
}

/// Looks a bare name up on PATH the way a shell would. Unlike
/// `runner::find_on_path`, symlinks are kept, since a program can behave
/// differently depending on the name it is run as.
fn native_on_path(path_var: &OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| runner::is_executable(candidate))
}

fn resolve_target<'a>(
    environment: &'a Environment,
    requested: Option<&str>,
) -> Result<(&'a str, &'a Target)> {
    let targets = &environment.config.targets;
    if let Some(requested) = requested {
        return targets
            .get_key_value(requested)
            .map(|(name, target)| (name.as_str(), target))
            .ok_or_else(|| Error::UnknownTarget {
                environment: environment.name.clone(),
                target: requested.to_owned(),
            });
    }
    if let Some((name, target)) = targets.iter().find(|(_, target)| target.default) {
        return Ok((name, target));
    }
    let mut all = targets.iter();
    match (all.next(), all.next()) {
        (Some((name, target)), None) => Ok((name, target)),
        (None, _) => Err(Error::NoTargets(environment.name.clone())),
        (Some(_), Some(_)) => Err(Error::AmbiguousTarget {
            environment: environment.name.clone(),
            targets: targets.keys().cloned().collect(),
        }),
    }
}

#[derive(Debug, PartialEq)]
struct Program {
    wine_args: Vec<String>,
    /// The Unix directory holding the program, when it's a path rather than a bare name.
    dir: Option<PathBuf>,
}

impl Program {
    fn default_cwd(&self, prefix: &Path) -> PathBuf {
        self.dir.clone().unwrap_or_else(|| prefix.join("drive_c"))
    }
}

fn resolve_program(
    paths: &Paths,
    prefix: &Path,
    exe: &str,
    relative_to_cwd: bool,
) -> Result<Program> {
    let not_found = || Error::ExeNotFound(exe.to_owned());
    let msi = exe.to_ascii_lowercase().ends_with(".msi");

    if winpath::is_windows_path(exe) {
        let unix = winpath::to_unix(prefix, exe)
            .filter(|unix| unix.is_file())
            .ok_or_else(not_found)?;
        return Ok(Program {
            wine_args: program_args(exe.to_owned(), msi),
            dir: unix.parent().map(Path::to_owned),
        });
    }

    if !is_path(exe, relative_to_cwd) {
        // Bare names like `notepad` or `winecfg` are looked up by wine itself.
        return Ok(Program {
            wine_args: program_args(exe.to_owned(), msi),
            dir: None,
        });
    }

    // A target comes from a config, so it resolves the way config paths do.
    let unix = if relative_to_cwd {
        paths.absolute(exe)?
    } else {
        paths.config_path(exe)?
    };
    if !unix.is_file() {
        return Err(not_found());
    }
    let arg = match msi.then(|| z_drive_path(prefix, &unix)).flatten() {
        Some(dos) => dos,
        None => unix
            .to_str()
            .ok_or_else(|| Error::NonUtf8Path(unix.clone()))?
            .to_owned(),
    };
    Ok(Program {
        wine_args: program_args(arg, msi),
        dir: unix.parent().map(Path::to_owned),
    })
}

fn is_path(exe: &str, relative_to_cwd: bool) -> bool {
    exe.contains('/') || exe.starts_with('~') || (relative_to_cwd && Path::new(exe).is_file())
}

/// Fails fast on an installer path that doesn't exist, before the prefix is
/// created. Windows paths and bare names aren't resolvable yet, so they pass;
/// `resolve_program` checks those for real once the prefix is open.
pub fn check_program_path(paths: &Paths, program: &str) -> Result<()> {
    check_unix_program(paths, program, true)
}

/// The Unix path half of `resolve_program`, which needs no prefix.
pub(crate) fn check_unix_program(paths: &Paths, exe: &str, relative_to_cwd: bool) -> Result<()> {
    if winpath::is_windows_path(exe) || !is_path(exe, relative_to_cwd) {
        return Ok(());
    }
    let unix = if relative_to_cwd {
        paths.absolute(exe)?
    } else {
        paths.config_path(exe)?
    };
    if unix.is_file() {
        Ok(())
    } else {
        Err(Error::ExeNotFound(exe.to_owned()))
    }
}

fn program_args(program: String, msi: bool) -> Vec<String> {
    if msi {
        vec!["msiexec".to_owned(), "/i".to_owned(), program]
    } else {
        vec![program]
    }
}

/// msiexec doesn't understand Unix paths the way the wine loader does, so an
/// installer outside the prefix is handed over through the `Z:` drive.
fn z_drive_path(prefix: &Path, unix: &Path) -> Option<String> {
    let root = fs::canonicalize(prefix.join("dosdevices/z:")).ok()?;
    if root != Path::new("/") {
        return None;
    }
    Some(format!("Z:{}", unix.to_str()?.replace('/', "\\")))
}

fn resolve_cwd(paths: &Paths, prefix: &Path, cwd: &str) -> Result<PathBuf> {
    let dir = if winpath::is_windows_path(cwd) {
        winpath::to_unix(prefix, cwd)
    } else {
        Some(paths.config_path(cwd)?)
    };
    dir.filter(|dir| dir.is_dir())
        .ok_or_else(|| Error::CwdNotFound(cwd.to_owned()))
}

/// The environment a program runs with: the global and environment `[env]`
/// tables, then the command line overrides.
pub fn merge_env(
    runner: &Runner,
    global: &GlobalConfig,
    environment: &Environment,
    overrides: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    merge(Some(runner), global, environment, overrides)
}

/// `merge_env` for a launch with or without a runner. Nothing is added or
/// rewritten for a native one, which gets no WINE variables.
fn merge(
    runner: Option<&Runner>,
    global: &GlobalConfig,
    environment: &Environment,
    overrides: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    for (key, value) in overrides {
        check_override(runner.is_some(), key, value)?;
    }
    let mut env = global.env.clone();
    env.extend(environment.config.env.clone());
    env.extend(overrides.clone());
    let Some(runner) = runner else {
        return Ok(env);
    };
    // Only a Proton launch has these, and wineo decides all of them: the
    // runner root it resolved, and the environment's own protonfixes settings.
    if runner.is_proton()
        && let Some(key) = MANAGED.iter().find(|key| env.contains_key(**key))
    {
        return Err(managed(key));
    }
    // Applied after Runner::wine, so a user-set value here would otherwise
    // override the one it adds.
    if let Some(value) = env.get_mut("WINEDLLOVERRIDES") {
        *value = with_menu_builder_disabled(Some(value));
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::config::EnvironmentConfig;

    struct Fixture {
        _root: tempfile::TempDir,
        paths: Paths,
        prefix: PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().canonicalize().unwrap();
        let prefix = home.join("prefix");
        let app = prefix.join("drive_c/Program Files/App");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("app.exe"), "").unwrap();
        fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        symlink("../drive_c", prefix.join("dosdevices/c:")).unwrap();
        symlink("/", prefix.join("dosdevices/z:")).unwrap();
        fs::write(home.join("setup.exe"), "").unwrap();
        fs::write(home.join("setup.msi"), "").unwrap();

        let lookup_home = home.clone();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(lookup_home.as_os_str()))
        })
        .unwrap();
        Fixture {
            _root: root,
            paths,
            prefix,
        }
    }

    #[test]
    fn launch_snapshots_the_concrete_prefix_while_environment_keeps_its_alias() {
        let f = fixture();
        fs::write(f.prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        let alias = f.prefix.parent().unwrap().join("prefix-alias");
        symlink(&f.prefix, &alias).unwrap();

        let wine_dir = f.prefix.parent().unwrap().join("runners/fake/bin");
        fs::create_dir_all(&wine_dir).unwrap();
        for name in ["wine", "wineserver"] {
            let path = wine_dir.join(name);
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let config_text = format!(
            "prefix = \"{}\"\nrunner = \"fake\"\narch = \"win64\"\n",
            alias.display()
        );
        let environment = Environment {
            name: "app".to_owned(),
            path: PathBuf::from("/app.toml"),
            config: EnvironmentConfig::parse(Path::new("/app.toml"), &config_text).unwrap(),
        };
        let mut global = GlobalConfig::default();
        global.defaults.runner_root = Some("~/runners".to_owned());

        let configured_prefix = environment.prefix(&f.paths).unwrap();
        let (_, launch_prefix) = open(&f.paths, &global, &environment).unwrap();
        assert_eq!(configured_prefix, alias);
        assert_eq!(launch_prefix, f.prefix.canonicalize().unwrap());
    }

    fn environment(text: &str) -> Environment {
        let text = format!("prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n{text}");
        Environment {
            name: "app".to_owned(),
            path: PathBuf::from("/app.toml"),
            config: EnvironmentConfig::parse(Path::new("/app.toml"), &text).unwrap(),
        }
    }

    #[test]
    fn gamemode_takes_the_most_specific_setting() {
        let off = GlobalConfig::default();
        let on = GlobalConfig {
            gamemode: Some(true),
            ..GlobalConfig::default()
        };
        let quiet = environment("[targets.app]\nexe = \"app\"\n");
        let loud = environment("gamemode = true\n\n[targets.app]\nexe = \"app\"\n");
        let mixed =
            environment("gamemode = true\n\n[targets.app]\nexe = \"app\"\ngamemode = false\n");
        let target = |env: &Environment| env.config.targets.get("app").cloned();

        // Unset everywhere is off.
        assert!(!gamemode(&off, &quiet, target(&quiet).as_ref()));
        // Each level turns it on for the one below.
        assert!(gamemode(&on, &quiet, target(&quiet).as_ref()));
        assert!(gamemode(&off, &loud, target(&loud).as_ref()));
        // And the more specific level can turn it back off.
        assert!(!gamemode(&on, &mixed, target(&mixed).as_ref()));
        assert!(!gamemode(&off, &mixed, target(&mixed).as_ref()));
        // With no target in play, the environment decides.
        assert!(gamemode(&off, &mixed, None));
        assert!(!gamemode(&off, &quiet, None));
    }

    #[test]
    fn target_resolution_order() {
        let env = environment(
            "[targets.a]\nexe = \"a\"\n[targets.b]\nexe = \"b\"\ndefault = true\n[targets.c]\nexe = \"c\"\n",
        );
        assert_eq!(resolve_target(&env, Some("c")).unwrap().0, "c");
        assert_eq!(resolve_target(&env, None).unwrap().0, "b");
        assert!(matches!(
            resolve_target(&env, Some("zzz")),
            Err(Error::UnknownTarget { .. })
        ));

        let single = environment("[targets.only]\nexe = \"x\"\n");
        assert_eq!(resolve_target(&single, None).unwrap().0, "only");

        let none = environment("");
        assert!(matches!(
            resolve_target(&none, None),
            Err(Error::NoTargets(_))
        ));

        let ambiguous = environment("[targets.a]\nexe = \"a\"\n[targets.b]\nexe = \"b\"\n");
        let err = resolve_target(&ambiguous, None).unwrap_err();
        assert!(err.to_string().contains("a, b"), "{err}");
    }

    #[test]
    fn windows_paths_resolve_inside_the_prefix() {
        let f = fixture();
        let program =
            resolve_program(&f.paths, &f.prefix, r"c:\program files\app\APP.exe", false).unwrap();
        assert_eq!(program.wine_args, [r"c:\program files\app\APP.exe"]);
        assert_eq!(
            program.dir.as_deref(),
            Some(f.prefix.join("drive_c/Program Files/App").as_path())
        );
        assert!(matches!(
            resolve_program(
                &f.paths,
                &f.prefix,
                r"C:\Program Files\App\missing.exe",
                false
            ),
            Err(Error::ExeNotFound(_))
        ));
        assert!(matches!(
            resolve_program(&f.paths, &f.prefix, r"C:\Program Files\App", false),
            Err(Error::ExeNotFound(_))
        ));
    }

    #[test]
    fn bare_names_go_straight_to_wine() {
        let f = fixture();
        let program = resolve_program(&f.paths, &f.prefix, "notepad", false).unwrap();
        assert_eq!(
            program,
            Program {
                wine_args: vec!["notepad".to_owned()],
                dir: None
            }
        );
        assert_eq!(program.default_cwd(&f.prefix), f.prefix.join("drive_c"));
    }

    #[test]
    fn unix_paths_expand_and_must_exist() {
        let f = fixture();
        let program = resolve_program(&f.paths, &f.prefix, "~/setup.exe", false).unwrap();
        let home = f.prefix.parent().unwrap();
        assert_eq!(
            program.wine_args,
            [home.join("setup.exe").to_str().unwrap()]
        );
        assert_eq!(program.dir.as_deref(), Some(home));
        assert!(matches!(
            resolve_program(&f.paths, &f.prefix, "~/nope.exe", false),
            Err(Error::ExeNotFound(_))
        ));
    }

    #[test]
    fn msi_files_go_through_msiexec() {
        let f = fixture();
        let home = f.prefix.parent().unwrap();
        let program = resolve_program(&f.paths, &f.prefix, "~/setup.msi", false).unwrap();
        let dos = format!(
            "Z:{}",
            home.join("setup.msi").to_str().unwrap().replace('/', "\\")
        );
        assert_eq!(program.wine_args, ["msiexec", "/i", dos.as_str()]);

        let bare = resolve_program(&f.paths, &f.prefix, "thing.MSI", false).unwrap();
        assert_eq!(bare.wine_args, ["msiexec", "/i", "thing.MSI"]);
    }

    #[test]
    fn check_program_path_passes_bare_names_and_windows_paths() {
        let f = fixture();
        assert!(check_program_path(&f.paths, "notepad").is_ok());
        assert!(check_program_path(&f.paths, r"C:\Program Files\App\missing.exe").is_ok());
    }

    #[test]
    fn check_program_path_requires_existing_unix_paths() {
        let f = fixture();
        let home = f.prefix.parent().unwrap();
        assert!(check_program_path(&f.paths, "~/setup.exe").is_ok());
        assert!(matches!(
            check_program_path(&f.paths, "~/nope.exe"),
            Err(Error::ExeNotFound(_))
        ));

        let existing = home.join("setup.exe");
        assert!(check_program_path(&f.paths, existing.to_str().unwrap()).is_ok());
        let missing = home.join("nope.exe");
        assert!(matches!(
            check_program_path(&f.paths, missing.to_str().unwrap()),
            Err(Error::ExeNotFound(_))
        ));
    }

    #[test]
    fn target_checks_need_no_prefix() {
        let f = fixture();
        let global = GlobalConfig::default();
        let runner = Runner::fake("wine");
        let check = |text: &str, target: Option<&str>| {
            let overrides = Overrides {
                target: target.map(str::to_owned),
                ..Default::default()
            };
            Launch::check_target(&f.paths, &global, &environment(text), &runner, &overrides)
        };
        let windows = "[targets.app]\nexe = 'C:\\Games\\app.exe'\ncwd = 'C:\\Games'\n";
        assert!(check(windows, None).is_ok());
        assert!(check("[targets.app]\nexe = \"~/setup.exe\"\n", None).is_ok());
        assert!(matches!(check("", None), Err(Error::NoTargets(_))));
        assert!(matches!(
            check(windows, Some("nope")),
            Err(Error::UnknownTarget { .. })
        ));
        assert!(matches!(
            check("[targets.app]\nexe = \"~/missing.exe\"\n", None),
            Err(Error::ExeNotFound(_))
        ));
        assert!(matches!(
            check("[targets.app]\nexe = \"app\"\ncwd = \"~/missing\"\n", None),
            Err(Error::CwdNotFound(_))
        ));
    }

    #[test]
    fn exec_checks_need_no_prefix() {
        let f = fixture();
        let check = |program: &str| {
            Launch::check_exec(
                &f.paths,
                &GlobalConfig::default(),
                &environment(""),
                &Runner::fake("wine"),
                program,
                &BTreeMap::new(),
            )
        };
        assert!(check("notepad").is_ok());
        assert!(check(r"C:\missing.exe").is_ok());
        assert!(matches!(
            check("/nonexistent/setup.exe"),
            Err(Error::ExeNotFound(_))
        ));
    }

    #[test]
    fn cwd_accepts_windows_and_unix_paths() {
        let f = fixture();
        assert_eq!(
            resolve_cwd(&f.paths, &f.prefix, r"C:\Program Files").unwrap(),
            f.prefix.join("drive_c/Program Files")
        );
        assert_eq!(
            resolve_cwd(&f.paths, &f.prefix, "~/prefix").unwrap(),
            f.prefix
        );
        assert!(matches!(
            resolve_cwd(&f.paths, &f.prefix, r"C:\nope"),
            Err(Error::CwdNotFound(_))
        ));
    }

    #[test]
    fn env_precedence_is_global_then_environment_then_overrides() {
        let global = GlobalConfig::parse(
            Path::new("/g.toml"),
            "[env]\nA = \"global\"\nB = \"global\"\nC = \"global\"\n",
        )
        .unwrap();
        let env = environment("[env]\nB = \"env\"\nC = \"env\"\n");
        let overrides = BTreeMap::from([("C".to_owned(), "cli".to_owned())]);
        let merged = merge_env(&Runner::fake("wine"), &global, &env, &overrides).unwrap();
        assert_eq!(merged["A"], "global");
        assert_eq!(merged["B"], "env");
        assert_eq!(merged["C"], "cli");
    }

    #[test]
    fn merged_wine_dll_overrides_disables_the_menu_builder() {
        let global = GlobalConfig::default();
        let env = environment("[env]\nWINEDLLOVERRIDES = \"foo.dll=n\"\n");
        let merged = merge_env(&Runner::fake("wine"), &global, &env, &BTreeMap::new()).unwrap();
        assert_eq!(
            merged["WINEDLLOVERRIDES"],
            "foo.dll=n;winemenubuilder.exe=d"
        );

        let opted_in = BTreeMap::from([(
            "WINEDLLOVERRIDES".to_owned(),
            "winemenubuilder.exe=n".to_owned(),
        )]);
        let merged = merge_env(&Runner::fake("wine"), &global, &env, &opted_in).unwrap();
        assert_eq!(merged["WINEDLLOVERRIDES"], "winemenubuilder.exe=n");

        let none = environment("");
        let merged = merge_env(&Runner::fake("wine"), &global, &none, &BTreeMap::new()).unwrap();
        assert!(!merged.contains_key("WINEDLLOVERRIDES"));
    }

    #[test]
    fn env_assignments_are_validated() {
        assert_eq!(
            parse_env_assignment("DXVK_HUD=fps=1").unwrap(),
            ("DXVK_HUD".to_owned(), "fps=1".to_owned())
        );
        assert!(matches!(
            parse_env_assignment("NOEQUALS"),
            Err(Error::InvalidEnvAssignment(_))
        ));
        assert!(parse_env_assignment("=x").is_err());
        assert!(parse_env_assignment("WINEPREFIX=/x").is_err());
    }

    fn native(name: &str, text: &str) -> Environment {
        let text = format!("kind = \"native\"\n{text}");
        Environment {
            name: name.to_owned(),
            path: PathBuf::from(format!("/{name}.toml")),
            config: EnvironmentConfig::parse(Path::new(&format!("/{name}.toml")), &text).unwrap(),
        }
    }

    fn script(path: &Path, mode: u32) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn native_launch(paths: &Paths, exe: &str) -> Result<Launch> {
        let env = native("dusk", &format!("[targets.game]\nexe = '{exe}'\n"));
        Launch::target(paths, &GlobalConfig::default(), &env, &Overrides::default())
    }

    #[test]
    fn a_native_exe_by_absolute_or_home_path_runs_in_its_own_dir() {
        let f = fixture();
        let home = f.prefix.parent().unwrap();
        let exe = home.join("Games/Dusk/dusk.sh");
        script(&exe, 0o755);

        let launch = native_launch(&f.paths, exe.to_str().unwrap()).unwrap();
        assert!(matches!(&launch.host, Host::Native { exe: found } if *found == exe));
        assert_eq!(launch.cwd, home.join("Games/Dusk"));
        assert!(launch.host.runner().is_none());
        assert!(launch.host.prefix().is_none());

        let launch = native_launch(&f.paths, "~/Games/Dusk/dusk.sh").unwrap();
        assert!(matches!(&launch.host, Host::Native { exe: found } if *found == exe));
        assert_eq!(launch.cwd, home.join("Games/Dusk"));

        assert!(matches!(
            native_launch(&f.paths, "~/Games/Dusk/missing.sh"),
            Err(Error::ExeNotFound(_))
        ));
    }

    #[test]
    fn a_bare_native_name_is_looked_up_on_path_and_runs_in_home() {
        let f = fixture();
        let home = f.prefix.parent().unwrap();
        let first = home.join("first");
        let second = home.join("second");
        script(&second.join("dusk"), 0o755);
        // Not executable, so a shell would skip it too.
        script(&first.join("dusk"), 0o644);
        let path_var =
            std::env::join_paths([PathBuf::from("relative"), first.clone(), second.clone()])
                .unwrap();
        assert_eq!(native_on_path(&path_var, "dusk"), Some(second.join("dusk")));
        assert_eq!(native_on_path(&path_var, "nope"), None);

        // Something every system has on PATH.
        let launch = native_launch(&f.paths, "sh").unwrap();
        let Host::Native { exe } = &launch.host else {
            panic!("not native");
        };
        assert!(exe.is_absolute() && exe.ends_with("sh"), "{exe:?}");
        assert_eq!(launch.cwd, home);
        assert!(matches!(
            native_launch(&f.paths, "wineo-surely-not-installed"),
            Err(Error::NotOnPath(_))
        ));
    }

    #[test]
    fn a_native_exe_that_is_not_executable_is_refused() {
        let f = fixture();
        let home = f.prefix.parent().unwrap();
        script(&home.join("Games/foo/start.sh"), 0o644);
        let err = native_launch(&f.paths, "~/Games/foo/start.sh").unwrap_err();
        assert_eq!(
            err.to_string(),
            "~/Games/foo/start.sh is not executable. Run chmod +x on it."
        );
    }

    #[test]
    fn a_native_target_cannot_be_a_windows_program() {
        let f = fixture();
        let home = f.prefix.parent().unwrap();
        // Even an executable one on disk.
        script(&home.join("game.EXE"), 0o755);
        for exe in [
            r"C:\Games\game.exe",
            "c:/games/game",
            "~/game.EXE",
            "setup.msi",
            "~/run.bat",
            "/x/Game.Lnk",
        ] {
            let err = native_launch(&f.paths, exe).unwrap_err();
            assert_eq!(
                err.to_string(),
                "dusk is native, so its target can't be a Windows program",
                "{exe}"
            );
        }
    }

    #[test]
    fn a_relative_native_exe_only_works_in_a_portable_folder() {
        let f = fixture();
        assert!(matches!(
            native_launch(&f.paths, "bin/game"),
            Err(Error::RelativeExe(_))
        ));

        let root = f.prefix.parent().unwrap().join("usb");
        script(&root.join("bin/game"), 0o755);
        let home = f.prefix.parent().unwrap().to_owned();
        let portable = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            Some(&root),
        )
        .unwrap();
        let launch = native_launch(&portable, "bin/game").unwrap();
        let root = root.canonicalize().unwrap();
        assert!(
            matches!(&launch.host, Host::Native { exe } if crate::paths::resolve(exe) == root.join("bin/game")),
            "{:?}",
            launch.host
        );
    }

    #[test]
    fn a_native_launch_applies_env_and_args_and_adds_no_wine_variables() {
        let f = fixture();
        let home = f.prefix.parent().unwrap();
        let exe = home.join("dusk.sh");
        script(&exe, 0o755);
        let global = GlobalConfig::parse(
            Path::new("/g.toml"),
            "[env]\nA = \"global\"\nB = \"global\"\n",
        )
        .unwrap();
        let env = native(
            "dusk",
            &format!(
                "[env]\nB = \"env\"\nWINEPREFIX = \"/mine\"\nWINEDLLOVERRIDES = \"x=n\"\n[targets.game]\nexe = '{}'\nargs = [\"-a\"]\ncwd = '~'\n",
                exe.display()
            ),
        );
        let overrides = Overrides {
            env: BTreeMap::from([("C".to_owned(), "cli".to_owned())]),
            args: vec!["-b".to_owned()],
            ..Default::default()
        };
        let launch = Launch::target(&f.paths, &global, &env, &overrides).unwrap();
        assert_eq!(launch.args, ["-a", "-b"]);
        assert_eq!(launch.cwd, home);
        assert_eq!(launch.env["A"], "global");
        assert_eq!(launch.env["B"], "env");
        assert_eq!(launch.env["C"], "cli");
        // Passed through as written, with nothing wineo adds or rewrites.
        assert_eq!(launch.env["WINEPREFIX"], "/mine");
        assert_eq!(launch.env["WINEDLLOVERRIDES"], "x=n");

        let command = launch.command();
        assert_eq!(command.get_program(), exe.as_os_str());
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["-a", "-b"]);
        assert_eq!(command.get_current_dir(), Some(home));
        let set: BTreeMap<_, _> = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(|v| v.to_owned())))
            .collect();
        let wine: Vec<_> = set
            .keys()
            .filter(|key| key.to_string_lossy().starts_with("WINE"))
            .collect();
        assert_eq!(wine, ["WINEDLLOVERRIDES", "WINEPREFIX"]);
        assert_eq!(
            set[OsStr::new("WINEDLLOVERRIDES")].as_deref(),
            Some(OsStr::new("x=n"))
        );
        assert!(!set.contains_key(OsStr::new("PROTONPATH")));
    }

    #[test]
    fn gamemode_wraps_a_native_command() {
        let launch = Launch {
            host: Host::Native {
                exe: PathBuf::from("/usr/bin/dusklight"),
            },
            target: Some("game".to_owned()),
            args: vec!["-x".to_owned()],
            cwd: PathBuf::from("/"),
            env: BTreeMap::new(),
            gamemode: Some(PathBuf::from("/usr/bin/gamemoderun")),
            gameid: None,
            store: None,
        };
        let command = launch.command();
        assert_eq!(command.get_program(), "/usr/bin/gamemoderun");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["/usr/bin/dusklight", "-x"]);
        assert_eq!(command.get_envs().count(), 0);
    }

    #[test]
    fn native_target_checks_cover_the_exe_without_a_runner() {
        let f = fixture();
        let env = native("dusk", "[targets.game]\nexe = 'game.exe'\n");
        assert!(matches!(
            Launch::check_target(
                &f.paths,
                &GlobalConfig::default(),
                &env,
                &Runner::fake("wine"),
                &Overrides::default()
            ),
            Err(Error::NativeWindowsTarget(_))
        ));
        assert!(matches!(
            Launch::check_native_target(
                &f.paths,
                &GlobalConfig::default(),
                &native("dusk", ""),
                &Overrides::default()
            ),
            Err(Error::NoTargets(_))
        ));
    }

    #[test]
    fn exec_still_needs_a_prefix() {
        let f = fixture();
        let env = native("dusk", "");
        assert!(matches!(
            Launch::exec(
                &f.paths,
                &GlobalConfig::default(),
                &env,
                "sh",
                &[],
                &BTreeMap::new()
            ),
            Err(Error::NoPrefix(_))
        ));
    }

    #[test]
    fn a_bare_native_name_uses_the_path_the_game_gets() {
        let f = fixture();
        let home = f.prefix.parent().unwrap();
        let from_global = home.join("global-bin");
        let from_env = home.join("env-bin");
        let from_cli = home.join("cli-bin");
        for dir in [&from_global, &from_env, &from_cli] {
            script(&dir.join("dusk"), 0o755);
        }
        script(&from_global.join("only-global"), 0o755);
        let global = GlobalConfig::parse(
            Path::new("/g.toml"),
            &format!("[env]\nPATH = \"{}\"\n", from_global.display()),
        )
        .unwrap();
        let launch = |env_path: Option<&Path>, cli_path: Option<&Path>, exe: &str| {
            let table = env_path
                .map(|dir| format!("[env]\nPATH = \"{}\"\n", dir.display()))
                .unwrap_or_default();
            let env = native("dusk", &format!("{table}[targets.game]\nexe = '{exe}'\n"));
            let overrides = Overrides {
                env: cli_path
                    .map(|dir| BTreeMap::from([("PATH".to_owned(), dir.display().to_string())]))
                    .unwrap_or_default(),
                ..Default::default()
            };
            Launch::target(&f.paths, &global, &env, &overrides).map(|launch| launch.host)
        };
        let exe_of = |host: Result<Host>| match host.unwrap() {
            Host::Native { exe } => exe,
            Host::Wine { .. } => panic!("not native"),
        };
        assert_eq!(exe_of(launch(None, None, "dusk")), from_global.join("dusk"));
        assert_eq!(
            exe_of(launch(Some(&from_env), None, "dusk")),
            from_env.join("dusk")
        );
        assert_eq!(
            exe_of(launch(Some(&from_env), Some(&from_cli), "dusk")),
            from_cli.join("dusk")
        );
        assert!(matches!(
            launch(Some(&from_env), None, "only-global"),
            Err(Error::NotOnPath(_))
        ));
    }

    #[test]
    fn wineprefix_overrides_are_refused_only_for_wine() {
        let overrides = BTreeMap::from([("WINEPREFIX".to_owned(), "/x".to_owned())]);
        let global = GlobalConfig::default();
        assert!(parse_launch_env_assignment("WINEPREFIX=/x").is_ok());
        assert!(parse_launch_env_assignment("A=B=C").is_ok());
        assert!(parse_launch_env_assignment("=x").is_err());
        let err =
            merge_env(&Runner::fake("wine"), &global, &environment(""), &overrides).unwrap_err();
        assert!(
            err.to_string().contains("WINEPREFIX is managed by wineo"),
            "{err}"
        );
        let merged = merge(None, &global, &native("dusk", ""), &overrides).unwrap();
        assert_eq!(merged["WINEPREFIX"], "/x");
    }
}
