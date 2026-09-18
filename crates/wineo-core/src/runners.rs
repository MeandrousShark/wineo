//! Wine builds installed by extracting them into the runners root, plus the
//! system wine every config starts out using.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::GlobalConfig;
use crate::error::{Error, IoContext, Result};
use crate::name;
use crate::paths::Paths;
use crate::runner::Runner;

/// Why a directory in the runners root is not usable as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    Reserved,
    InvalidName,
    NoWine,
    NoWineserver,
    NotExecutable,
}

impl Skipped {
    pub fn as_str(self) -> &'static str {
        match self {
            Skipped::Reserved => "`wine` is reserved for the system wine",
            Skipped::InvalidName => "not a usable name: use letters, digits, `-`, `_`, and `.`",
            Skipped::NoWine => "no bin/wine",
            Skipped::NoWineserver => "no bin/wineserver",
            Skipped::NotExecutable => "bin/wine and bin/wineserver are not both executable",
        }
    }
}

/// Everything in the runners root, usable or not.
#[derive(Debug, Default)]
pub struct Listing {
    pub runners: Vec<Runner>,
    pub skipped: Vec<(String, Skipped)>,
}

/// The directory installed wine builds are extracted into.
#[derive(Debug, Clone)]
pub struct Runners {
    root: PathBuf,
}

impl Runners {
    pub fn new(paths: &Paths, global: &GlobalConfig) -> Result<Self> {
        Ok(Self::at(global.runner_root(paths)?))
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The runner an environment's `runner` value names. `wine` always means
    /// the system wine, whatever is in the runners root.
    pub fn resolve(&self, name: &str) -> Result<Runner> {
        if name == Runner::SYSTEM {
            return Runner::system();
        }
        if !name::is_valid_runner(name) {
            return Err(Error::InvalidRunnerName(name.to_owned()));
        }
        let dir = self.root.join(name);
        match check(&dir) {
            Ok(runner) => Ok(runner),
            Err(reason) if reason == Skipped::NoWine && !dir.is_dir() => {
                Err(Error::UnknownRunner {
                    name: name.to_owned(),
                    installed: self.installed()?,
                    root: self.root.clone(),
                })
            }
            Err(reason) => Err(Error::NotARunner {
                name: name.to_owned(),
                reason: reason.as_str(),
            }),
        }
    }

    /// Everything in the runners root, sorted by name. A missing root just
    /// means nothing is installed yet.
    pub fn list(&self) -> Result<Listing> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Listing::default()),
            Err(err) => {
                return Err(err).context(|| format!("failed to read {}", self.root.display()));
            }
        };

        let mut listing = Listing::default();
        for entry in entries {
            let path = entry
                .context(|| format!("failed to read {}", self.root.display()))?
                .path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name == Runner::SYSTEM {
                listing.skipped.push((name.to_owned(), Skipped::Reserved));
            } else if !name::is_valid_runner(name) {
                listing
                    .skipped
                    .push((name.to_owned(), Skipped::InvalidName));
            } else {
                match check(&path) {
                    Ok(runner) => listing.runners.push(runner),
                    Err(reason) => listing.skipped.push((name.to_owned(), reason)),
                }
            }
        }
        listing.runners.sort_by(|a, b| a.name().cmp(b.name()));
        listing.skipped.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(listing)
    }

    fn installed(&self) -> Result<Vec<String>> {
        Ok(self
            .list()?
            .runners
            .into_iter()
            .map(|runner| runner.name().to_owned())
            .collect())
    }
}

/// A directory is a runner when it has both executables. Every lifecycle
/// command needs wineserver as well as wine, so one without the other is not
/// usable at all.
fn check(dir: &Path) -> std::result::Result<Runner, Skipped> {
    let (wine, wineserver) = (dir.join("bin/wine"), dir.join("bin/wineserver"));
    if !wine.exists() {
        return Err(Skipped::NoWine);
    }
    if !wineserver.exists() {
        return Err(Skipped::NoWineserver);
    }
    if !crate::runner::is_executable(&wine) || !crate::runner::is_executable(&wineserver) {
        return Err(Skipped::NotExecutable);
    }
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    Ok(Runner::at(name, wine, wineserver))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn runners(root: &Path) -> Runners {
        Runners::at(root.join("runners"))
    }

    /// A runner tree of stub executables, enough for discovery without wine.
    fn install(root: &Path, name: &str, files: &[(&str, bool)]) {
        let dir = root.join("runners").join(name);
        for (path, executable) in files {
            let path = dir.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "#!/bin/sh\n").unwrap();
            let mode = if *executable { 0o755 } else { 0o644 };
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    fn wine_build(root: &Path, name: &str) {
        install(root, name, &[("bin/wine", true), ("bin/wineserver", true)]);
    }

    fn skipped(listing: &Listing, name: &str) -> Option<Skipped> {
        listing
            .skipped
            .iter()
            .find(|(found, _)| found == name)
            .map(|(_, reason)| *reason)
    }

    #[test]
    fn a_missing_runners_root_just_means_nothing_is_installed() {
        let root = tempfile::tempdir().unwrap();
        let listing = runners(root.path()).list().unwrap();
        assert!(listing.runners.is_empty());
        assert!(listing.skipped.is_empty());
    }

    #[test]
    fn a_wine_build_is_a_runner_and_everything_else_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        wine_build(root.path(), "wine-tkg");
        wine_build(root.path(), "wine-10.0");
        install(root.path(), "half", &[("bin/wine", true)]);
        install(root.path(), "no-wine", &[("bin/wineserver", true)]);
        install(
            root.path(),
            "not-executable",
            &[("bin/wine", false), ("bin/wineserver", true)],
        );
        install(root.path(), "empty", &[("README", false)]);
        // Proton is not a wine build as far as this plan is concerned.
        install(root.path(), "GE-Proton9-20", &[("files/bin/wine", true)]);
        wine_build(root.path(), "wine");
        fs::create_dir_all(root.path().join("runners/.tmp-extract/bin")).unwrap();
        install(root.path(), "bad name", &[("bin/wine", true)]);

        let listing = runners(root.path()).list().unwrap();
        let names: Vec<&str> = listing.runners.iter().map(|runner| runner.name()).collect();
        assert_eq!(names, ["wine-10.0", "wine-tkg"]);

        assert_eq!(skipped(&listing, "half"), Some(Skipped::NoWineserver));
        assert_eq!(skipped(&listing, "no-wine"), Some(Skipped::NoWine));
        assert_eq!(
            skipped(&listing, "not-executable"),
            Some(Skipped::NotExecutable)
        );
        assert_eq!(skipped(&listing, "empty"), Some(Skipped::NoWine));
        assert_eq!(skipped(&listing, "GE-Proton9-20"), Some(Skipped::NoWine));
        assert_eq!(skipped(&listing, "wine"), Some(Skipped::Reserved));
        assert_eq!(
            skipped(&listing, ".tmp-extract"),
            Some(Skipped::InvalidName)
        );
        assert_eq!(skipped(&listing, "bad name"), Some(Skipped::InvalidName));
    }

    #[test]
    fn resolving_finds_the_absolute_binaries() {
        let root = tempfile::tempdir().unwrap();
        wine_build(root.path(), "wine-tkg");
        let runner = runners(root.path()).resolve("wine-tkg").unwrap();
        assert_eq!(runner.name(), "wine-tkg");
        assert_eq!(
            runner.wine_path(),
            root.path().join("runners/wine-tkg/bin/wine")
        );
        assert_eq!(
            runner.wineserver_path(),
            root.path().join("runners/wine-tkg/bin/wineserver")
        );
    }

    #[test]
    fn wine_always_means_the_system_one() {
        let root = tempfile::tempdir().unwrap();
        // Installed under the reserved name, and still ignored.
        wine_build(root.path(), "wine");
        let resolved = runners(root.path()).resolve("wine");
        match Runner::system() {
            Ok(system) => assert_eq!(resolved.unwrap().wine_path(), system.wine_path()),
            // No wine on this machine, so it fails the same way either way.
            Err(_) => assert!(resolved.is_err()),
        }
    }

    #[test]
    fn an_unknown_runner_says_what_is_installed() {
        let root = tempfile::tempdir().unwrap();
        let runners = runners(root.path());

        let err = runners.resolve("nope").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("is not installed"), "{message}");
        assert!(message.contains("runners"), "{message}");

        wine_build(root.path(), "wine-tkg");
        let message = runners.resolve("nope").unwrap_err().to_string();
        assert!(message.contains("wine-tkg"), "{message}");
    }

    #[test]
    fn a_directory_that_is_not_a_wine_build_says_what_was_missing() {
        let root = tempfile::tempdir().unwrap();
        install(root.path(), "half", &[("bin/wine", true)]);
        let err = runners(root.path()).resolve("half").unwrap_err();
        assert!(
            matches!(&err, Error::NotARunner { name, .. } if name == "half"),
            "{err}"
        );
        assert!(err.to_string().contains("bin/wineserver"), "{err}");
    }

    #[test]
    fn an_unusable_name_is_refused_before_looking_at_the_disk() {
        let root = tempfile::tempdir().unwrap();
        for name in ["..", ".", ".hidden", "has space", "a/b"] {
            assert!(
                matches!(
                    runners(root.path()).resolve(name),
                    Err(Error::InvalidRunnerName(_))
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn the_runner_root_follows_the_global_config() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| std::ffi::OsString::from("/home/me")))
                .unwrap();

        let default = Runners::new(&paths, &GlobalConfig::default()).unwrap();
        assert_eq!(
            default.root(),
            Path::new("/home/me/.local/share/wineo/runners")
        );

        let mut global = GlobalConfig::default();
        global.defaults.runner_root = Some(root.path().join("elsewhere").display().to_string());
        let moved = Runners::new(&paths, &global).unwrap();
        assert_eq!(moved.root(), root.path().join("elsewhere"));
    }
}
