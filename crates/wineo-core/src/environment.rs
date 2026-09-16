use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, value};

use crate::config::{Arch, EnvironmentConfig, GlobalConfig};
use crate::error::{Error, IoContext, Result};
use crate::name;
use crate::paths::Paths;
use crate::prefix;
use crate::runner::Runner;

#[derive(Debug)]
pub struct Environment {
    pub name: String,
    pub path: PathBuf,
    pub config: EnvironmentConfig,
}

/// Options for creating an environment. Anything unset falls back to the global defaults.
#[derive(Debug, Default)]
pub struct NewEnvironment {
    pub name: String,
    pub dir: Option<String>,
    pub arch: Option<Arch>,
}

impl Environment {
    pub fn load(paths: &Paths, name: &str) -> Result<Self> {
        check_name(name)?;
        let path = paths.environment_config(name);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(Error::UnknownEnvironment(name.to_owned()));
            }
            Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
        };
        let config = EnvironmentConfig::parse(&path, &text)?;
        Ok(Self {
            name: name.to_owned(),
            path,
            config,
        })
    }

    /// Creates the prefix, or adopts an existing one, then writes the environment config.
    pub fn create(
        paths: &Paths,
        global: &GlobalConfig,
        runner: &Runner,
        new: &NewEnvironment,
    ) -> Result<Self> {
        check_name(&new.name)?;
        let path = paths.environment_config(&new.name);
        if path.exists() {
            return Err(Error::EnvironmentExists(new.name.clone()));
        }
        let dir = match &new.dir {
            Some(dir) => paths.absolute(dir)?,
            None => global.prefix_root(paths)?.join(&new.name),
        };
        let prefix_value = paths.contract(&dir)?;

        let mut created = false;
        let arch = if prefix::is_prefix(&dir) {
            adopted_arch(&dir, new.arch.unwrap_or(global.defaults.arch), new.arch)?
        } else {
            let arch = new.arch.unwrap_or(global.defaults.arch);
            created = prefix::prepare_dir(&dir)?;
            let runtime = paths.runtime_dir(&new.name);
            let result = fs::create_dir_all(&runtime)
                .context(|| format!("failed to create {}", runtime.display()))
                .and_then(|()| prefix::create(runner, &dir, arch, &runtime.join("create.log")));
            if let Err(err) = result {
                remove_if_created(created, &dir);
                return Err(err);
            }
            arch
        };

        let mut doc = DocumentMut::new();
        doc["prefix"] = value(prefix_value);
        doc["runner"] = value(runner.name());
        doc["arch"] = value(arch.as_str());
        if let Err(err) = write_new(&path, &doc.to_string()) {
            remove_if_created(created, &dir);
            return Err(err);
        }
        Self::load(paths, &new.name)
    }

    pub fn prefix(&self, paths: &Paths) -> Result<PathBuf> {
        paths.absolute(&self.config.prefix)
    }
}

fn check_name(name: &str) -> Result<()> {
    if name::is_valid(name) {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_owned()))
    }
}

/// An adopted prefix keeps the arch it was created with, so an explicit
/// `--arch` that disagrees is an error rather than silently ignored.
fn adopted_arch(dir: &Path, fallback: Arch, requested: Option<Arch>) -> Result<Arch> {
    match (prefix::detect_arch(dir)?, requested) {
        (Some(actual), Some(requested)) if actual != requested => Err(Error::ArchMismatch {
            path: dir.to_owned(),
            actual,
            requested,
        }),
        (Some(actual), _) => Ok(actual),
        (None, _) => Ok(fallback),
    }
}

fn write_new(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .context(|| format!("failed to write {}", path.display()))
}

fn remove_if_created(created: bool, dir: &Path) {
    if created {
        // Best effort, the error that got us here matters more.
        let _ = fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn test_paths(root: &Path) -> Paths {
        let root = root.to_owned();
        Paths::from_lookup(move |key| match key {
            "HOME" => Some(OsString::from(root.join("home"))),
            _ => None,
        })
        .unwrap()
    }

    fn fake_prefix(dir: &Path, arch: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("system.reg"),
            format!("WINE REGISTRY Version 2\n\n#arch={arch}\n"),
        )
        .unwrap();
    }

    fn new(name: &str, dir: &Path) -> NewEnvironment {
        NewEnvironment {
            name: name.to_owned(),
            dir: Some(dir.to_str().unwrap().to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn adopting_a_prefix_writes_config_with_its_arch() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("home/Apps/Thing");
        fake_prefix(&prefix, "win64");

        let env = Environment::create(
            &paths,
            &GlobalConfig::default(),
            &Runner::fake("wine"),
            &new("thing", &prefix),
        )
        .unwrap();
        assert_eq!(env.config.prefix, "~/Apps/Thing");
        assert_eq!(env.config.runner, "wine");
        assert_eq!(env.config.arch, Arch::Win64);
        assert_eq!(
            fs::read_to_string(paths.environment_config("thing")).unwrap(),
            "prefix = \"~/Apps/Thing\"\nrunner = \"wine\"\narch = \"win64\"\n"
        );
        assert_eq!(env.prefix(&paths).unwrap(), prefix);

        assert!(matches!(
            Environment::create(
                &paths,
                &GlobalConfig::default(),
                &Runner::fake("wine"),
                &new("thing", &prefix)
            ),
            Err(Error::EnvironmentExists(_))
        ));
    }

    #[test]
    fn adopting_with_a_conflicting_arch_fails() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("old");
        fake_prefix(&prefix, "win32");

        let mut options = new("old", &prefix);
        options.arch = Some(Arch::Win64);
        assert!(matches!(
            Environment::create(
                &paths,
                &GlobalConfig::default(),
                &Runner::fake("wine"),
                &options
            ),
            Err(Error::ArchMismatch { .. })
        ));
        assert!(!paths.environment_config("old").exists());
    }

    #[test]
    fn non_empty_non_prefix_dirs_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let dir = root.path().join("stuff");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.txt"), "keep me").unwrap();

        assert!(matches!(
            Environment::create(
                &paths,
                &GlobalConfig::default(),
                &Runner::fake("wine"),
                &new("stuff", &dir)
            ),
            Err(Error::NotAPrefix(_))
        ));
        assert!(dir.join("notes.txt").exists());
    }

    #[test]
    fn load_reports_missing_and_invalid_names() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        assert!(matches!(
            Environment::load(&paths, "nope"),
            Err(Error::UnknownEnvironment(_))
        ));
        assert!(matches!(
            Environment::load(&paths, "../escape"),
            Err(Error::InvalidName(_))
        ));
    }

    #[test]
    #[ignore = "runs wine"]
    fn creates_a_real_prefix() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("home/prefixes/real");

        let env = Environment::create(
            &paths,
            &GlobalConfig::default(),
            &Runner::resolve("wine").unwrap(),
            &new("real", &prefix),
        )
        .unwrap();
        assert!(prefix::is_prefix(&prefix));
        assert_eq!(env.config.arch, Arch::Win64);
    }

    #[test]
    #[ignore = "runs wine"]
    fn failed_creation_removes_the_new_dir() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let prefix = root.path().join("home/prefixes/win32");

        let mut options = new("win32", &prefix);
        options.arch = Some(Arch::Win32);
        let err = Environment::create(
            &paths,
            &GlobalConfig::default(),
            &Runner::resolve("wine").unwrap(),
            &options,
        )
        .unwrap_err();
        assert!(matches!(err, Error::PrefixCreation { .. }), "{err}");
        assert!(!prefix.exists());
        assert!(!paths.environment_config("win32").exists());
    }
}
