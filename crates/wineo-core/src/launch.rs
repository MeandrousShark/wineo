use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{GlobalConfig, Target, validate_env_var};

/// The binary gamemode ships, which is not what the setting is called.
const GAMEMODE: &str = "gamemoderun";
use crate::environment::Environment;
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::prefix;
use crate::runner::{self, Runner, with_menu_builder_disabled};
use crate::runners::Runners;
use crate::winpath;

/// Command line overrides for launching a target.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Overrides {
    pub target: Option<String>,
    pub env: BTreeMap<String, String>,
    pub args: Vec<String>,
}

/// A fully resolved program launch inside an environment.
#[derive(Debug, Serialize, Deserialize)]
pub struct Launch {
    pub runner: Runner,
    pub prefix: PathBuf,
    pub target: Option<String>,
    /// Everything passed to `wine`.
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Merged from the global config, the environment, and the overrides.
    pub env: BTreeMap<String, String>,
    /// The resolved `gamemoderun`, when this launch asked for gamemode. The
    /// path is kept rather than the flag, so a detached run cannot end up
    /// resolving a different one from a different PATH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamemode: Option<PathBuf>,
}

impl Launch {
    pub fn target(
        paths: &Paths,
        global: &GlobalConfig,
        environment: &Environment,
        overrides: &Overrides,
    ) -> Result<Self> {
        let (name, target) = resolve_target(environment, overrides.target.as_deref())?;
        let (runner, prefix) = open(paths, global, environment)?;
        let program = resolve_program(paths, &prefix, &target.exe, false)?;
        let cwd = match &target.cwd {
            Some(cwd) => resolve_cwd(paths, &prefix, cwd)?,
            None => program.default_cwd(&prefix),
        };
        let mut args = program.wine_args;
        args.extend(target.args.iter().cloned());
        args.extend(overrides.args.iter().cloned());
        // Resolved here rather than at spawn time, so `start` refuses before it
        // hands anything to a supervisor.
        let gamemode = if gamemode(global, environment, Some(target)) {
            Some(
                runner::find_on_path(GAMEMODE)
                    .ok_or_else(|| Error::GamemodeMissing(environment.name.clone()))?,
            )
        } else {
            None
        };
        Ok(Self {
            env: merge_env(global, environment, &overrides.env)?,
            runner,
            prefix,
            target: Some(name.to_owned()),
            args,
            cwd,
            gamemode,
        })
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
        let program = resolve_program(paths, &prefix, program, true)?;
        let cwd = program.default_cwd(&prefix);
        let mut wine_args = program.wine_args;
        wine_args.extend(args.iter().cloned());
        Ok(Self {
            env: merge_env(global, environment, env)?,
            runner,
            prefix,
            target: None,
            args: wine_args,
            cwd,
            // Running a tool in the prefix is not playing a game.
            gamemode: None,
        })
    }

    pub fn command(&self) -> Command {
        let mut command = self
            .runner
            .command(&self.prefix, self.gamemode.as_deref())
            .expect("the runner was resolved when this launch was built");
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .envs(&self.env);
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

/// Parses a `-e KEY=VALUE` flag.
pub fn parse_env_assignment(assignment: &str) -> Result<(String, String)> {
    let (key, value) = assignment
        .split_once('=')
        .ok_or_else(|| Error::InvalidEnvAssignment(assignment.to_owned()))?;
    check_override(key, value)?;
    Ok((key.to_owned(), value.to_owned()))
}

fn check_override(key: &str, value: &str) -> Result<()> {
    if key == "WINEPREFIX" {
        return Err(Error::InvalidEnv(
            "WINEPREFIX is managed by wineo and can't be overridden".to_owned(),
        ));
    }
    validate_env_var(key, value).map_err(Error::InvalidEnv)
}

fn open(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &Environment,
) -> Result<(Runner, PathBuf)> {
    let prefix = environment.prefix(paths)?;
    let runner = Runners::new(paths, global)?.resolve(&environment.config.runner)?;
    // Wine would silently create a fresh prefix here otherwise.
    if !prefix::is_prefix(&runner.wine_prefix(&prefix)) {
        return Err(Error::MissingPrefix(prefix));
    }
    if runner.is_proton() {
        return Err(Error::ProtonNotImplemented(environment.name.clone()));
    }
    Ok((runner, prefix))
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

    let unix = paths.absolute(exe)?;
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
    if winpath::is_windows_path(program) || !is_path(program, true) {
        return Ok(());
    }
    let unix = paths.absolute(program)?;
    if unix.is_file() {
        Ok(())
    } else {
        Err(Error::ExeNotFound(program.to_owned()))
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
        Some(paths.absolute(cwd)?)
    };
    dir.filter(|dir| dir.is_dir())
        .ok_or_else(|| Error::CwdNotFound(cwd.to_owned()))
}

/// The environment a program runs with: the global and environment `[env]`
/// tables, then the command line overrides.
pub fn merge_env(
    global: &GlobalConfig,
    environment: &Environment,
    overrides: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    for (key, value) in overrides {
        check_override(key, value)?;
    }
    let mut env = global.env.clone();
    env.extend(environment.config.env.clone());
    env.extend(overrides.clone());
    // Applied after Runner::wine, so a user-set value here would otherwise
    // override the one it adds.
    if let Some(value) = env.get_mut("WINEDLLOVERRIDES") {
        *value = with_menu_builder_disabled(Some(value));
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
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
        let merged = merge_env(&global, &env, &overrides).unwrap();
        assert_eq!(merged["A"], "global");
        assert_eq!(merged["B"], "env");
        assert_eq!(merged["C"], "cli");
    }

    #[test]
    fn merged_wine_dll_overrides_disables_the_menu_builder() {
        let global = GlobalConfig::default();
        let env = environment("[env]\nWINEDLLOVERRIDES = \"foo.dll=n\"\n");
        let merged = merge_env(&global, &env, &BTreeMap::new()).unwrap();
        assert_eq!(
            merged["WINEDLLOVERRIDES"],
            "foo.dll=n;winemenubuilder.exe=d"
        );

        let opted_in = BTreeMap::from([(
            "WINEDLLOVERRIDES".to_owned(),
            "winemenubuilder.exe=n".to_owned(),
        )]);
        let merged = merge_env(&global, &env, &opted_in).unwrap();
        assert_eq!(merged["WINEDLLOVERRIDES"], "winemenubuilder.exe=n");

        let none = environment("");
        let merged = merge_env(&global, &none, &BTreeMap::new()).unwrap();
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
}
