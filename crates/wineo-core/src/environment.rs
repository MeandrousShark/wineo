use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, value};

use crate::config::{Arch, EnvironmentConfig, GlobalConfig};
use crate::error::{Error, IoContext, Result};
use crate::name;
use crate::operation::Locks;
use crate::paths::Paths;
use crate::prefix;
use crate::runner::Runner;
use crate::templates::Template;

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
    /// What `--template` named, already merged with the explicit flags.
    pub template: Option<Template>,
}

impl NewEnvironment {
    /// Where the prefix will be. Needed before creation, so the prefix can be
    /// locked against another command claiming it at the same time.
    pub fn prefix_dir(&self, paths: &Paths, global: &GlobalConfig) -> Result<PathBuf> {
        match &self.dir {
            Some(dir) => paths.absolute(dir),
            None => Ok(global.prefix_root(paths)?.join(&self.name)),
        }
    }
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

    /// Checks that an environment exists without parsing its config, for
    /// commands that shouldn't be blocked by a broken one.
    pub fn ensure_exists(paths: &Paths, name: &str) -> Result<()> {
        check_name(name)?;
        if paths.environment_config(name).is_file() {
            Ok(())
        } else {
            Err(Error::UnknownEnvironment(name.to_owned()))
        }
    }

    /// Creates the prefix, or adopts an existing one, then writes the environment config.
    /// `on_create_prefix` runs just before a new prefix is initialized, since that takes a while.
    pub fn create(
        paths: &Paths,
        global: &GlobalConfig,
        runner: &Runner,
        new: &NewEnvironment,
        on_create_prefix: impl FnOnce(&Path),
    ) -> Result<Self> {
        check_name(&new.name)?;
        let path = paths.environment_config(&new.name);
        if path.exists() {
            return Err(Error::EnvironmentExists(new.name.clone()));
        }
        let dir = new.prefix_dir(paths, global)?;
        let prefix_value = paths.contract(&dir)?;

        // Proton keeps its prefix in `pfx/` under the directory it is given.
        let wine_prefix = runner.wine_prefix(&dir);
        let arch = if prefix::is_prefix(&wine_prefix) {
            let arch = adopted_arch(
                &wine_prefix,
                new.arch.unwrap_or(global.defaults.arch),
                new.arch,
            )?;
            // Wine outside wineo could still build menu entries in an adopted
            // prefix otherwise. It might be in use, so this doesn't wait_idle.
            let runtime = paths.runtime_dir(&new.name);
            fs::create_dir_all(&runtime)
                .context(|| format!("failed to create {}", runtime.display()))?;
            prefix::disable_menu_builder(runner, &dir, &runtime.join("create.log"))?;
            arch
        } else {
            let arch = new.arch.unwrap_or(global.defaults.arch);
            let created = prefix::prepare_dir(&dir)?;
            on_create_prefix(&dir);
            let runtime = paths.runtime_dir(&new.name);
            let result = fs::create_dir_all(&runtime)
                .context(|| format!("failed to create {}", runtime.display()))
                .and_then(|()| prefix::create(runner, &dir, arch, &runtime.join("create.log")));
            if let Err(err) = result {
                if created {
                    // Best effort, the error that got us here matters more.
                    let _ = fs::remove_dir_all(&dir);
                }
                return Err(err);
            }
            arch
        };

        // A finished prefix is kept even if this write fails, so a retry adopts it.
        let mut doc = DocumentMut::new();
        doc["prefix"] = value(prefix_value);
        doc["runner"] = value(runner.selection());
        doc["arch"] = value(arch.as_str());
        if let Some(template) = &new.template {
            template.apply(&mut doc);
        }
        let _guard = Locks::new(paths).config(&path)?;
        write_new(&path, &doc.to_string())?;
        Self::load(paths, &new.name)
    }

    pub fn prefix(&self, paths: &Paths) -> Result<PathBuf> {
        paths.absolute(&self.config.prefix)
    }
}

/// Names of all environments, sorted. Configs aren't parsed, so a broken one is still listed.
pub fn names(paths: &Paths) -> Result<Vec<String>> {
    let dir = paths.environments_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).context(|| format!("failed to read {}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let path = entry
            .context(|| format!("failed to read {}", dir.display()))?
            .path();
        if path.extension() != Some(OsStr::new("toml")) || !path.is_file() {
            continue;
        }
        if let Some(name) = path.file_stem().and_then(OsStr::to_str)
            && name::is_valid(name)
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// What else refers to a prefix. An environment whose config can't be read
/// could refer to it too, so those are reported instead of assumed harmless.
#[derive(Debug, Default, PartialEq)]
pub struct Sharing {
    pub names: Vec<String>,
    pub unreadable: Vec<String>,
}

impl Sharing {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.unreadable.is_empty()
    }
}

/// Environments other than `name` that use `prefix`. Paths are compared
/// resolved, so two spellings of one directory are recognized as the same.
pub fn sharing_prefix(paths: &Paths, name: &str, prefix: &Path) -> Result<Sharing> {
    let wanted = crate::paths::resolve(prefix);
    let mut sharing = Sharing::default();
    for other in names(paths)? {
        if other == name {
            continue;
        }
        match Environment::load(paths, &other).and_then(|env| env.prefix(paths)) {
            Ok(other_prefix) if crate::paths::resolve(&other_prefix) == wanted => {
                sharing.names.push(other);
            }
            Ok(_) => {}
            Err(_) => sharing.unreadable.push(other),
        }
    }
    Ok(sharing)
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
            |_| {},
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
        assert!(paths.runtime_dir("thing").join("create.log").exists());

        assert!(matches!(
            Environment::create(
                &paths,
                &GlobalConfig::default(),
                &Runner::fake("wine"),
                &new("thing", &prefix),
                |_| {},
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
                &options,
                |_| {},
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
                &new("stuff", &dir),
                |_| {},
            ),
            Err(Error::NotAPrefix(_))
        ));
        assert!(dir.join("notes.txt").exists());
    }

    #[test]
    fn names_lists_toml_files_with_valid_names() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        assert!(names(&paths).unwrap().is_empty());

        let dir = paths.environments_dir();
        fs::create_dir_all(dir.join("dir.toml")).unwrap();
        for file in ["zed.toml", "broken.toml", "notes.txt", "bad name.toml"] {
            fs::write(dir.join(file), "not even toml").unwrap();
        }
        assert_eq!(names(&paths).unwrap(), ["broken", "zed"]);

        assert!(Environment::ensure_exists(&paths, "broken").is_ok());
        assert!(matches!(
            Environment::ensure_exists(&paths, "dir"),
            Err(Error::UnknownEnvironment(_))
        ));
        assert!(matches!(
            Environment::ensure_exists(&paths, "../zed"),
            Err(Error::InvalidName(_))
        ));
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
            &Runner::system().unwrap(),
            &new("real", &prefix),
            |_| {},
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
            &Runner::system().unwrap(),
            &options,
            |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, Error::PrefixCreation { .. }), "{err}");
        assert!(!prefix.exists());
        assert!(!paths.environment_config("win32").exists());
    }
}
