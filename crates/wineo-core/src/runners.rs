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
    AliasReserved,
    InvalidName,
    NoWine,
    NoWineserver,
    NotExecutable,
}

impl Skipped {
    pub fn as_str(self) -> &'static str {
        match self {
            Skipped::AliasReserved => "this name is reserved for a runner alias",
            Skipped::Reserved => "`wine` is reserved for the system wine",
            Skipped::InvalidName => "not a usable name: use letters, digits, `-`, `_`, and `.`",
            Skipped::NoWine => "no bin/wine and no proton script",
            Skipped::NoWineserver => "no bin/wineserver",
            Skipped::NotExecutable => "its wine or proton program is not executable",
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
        if crate::runner_alias::is_alias(name) {
            return self.resolve_alias(name);
        }
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

    pub fn resolve_locked(
        &self,
        paths: &Paths,
        name: &str,
    ) -> Result<(Runner, Option<crate::build_lock::Guard>)> {
        self.lock_resolved(paths, name, self.resolve(name)?)
    }

    pub(crate) fn lock_resolved(
        &self,
        paths: &Paths,
        name: &str,
        mut runner: Runner,
    ) -> Result<(Runner, Option<crate::build_lock::Guard>)> {
        if runner.name() == Runner::SYSTEM {
            return Ok((runner, None));
        }
        let runners = Self::at(
            runner
                .directory()
                .and_then(Path::parent)
                .unwrap_or(&self.root)
                .to_owned(),
        );
        for attempt in 0..2 {
            let guard =
                crate::build_lock::shared(paths, crate::build_lock::Kind::Runner, runner.name())?;
            if runner.directory().is_some_and(Path::is_dir) {
                return Ok((runner, Some(guard)));
            }
            drop(guard);
            if attempt == 0 && crate::runner_alias::is_alias(name) {
                runner = runners.resolve(name)?;
            } else {
                return Err(Error::UnknownRunner {
                    name: runner.name().to_owned(),
                    installed: runners.installed()?,
                    root: runners.root.clone(),
                });
            }
        }
        unreachable!("the second missing build returns an error")
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
            if crate::runner_alias::is_alias(name) {
                listing
                    .skipped
                    .push((name.to_owned(), Skipped::AliasReserved));
            } else if name == Runner::SYSTEM {
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

    fn resolve_alias(&self, alias: &str) -> Result<Runner> {
        let failure = |reason: String| Error::RunnerAlias {
            name: alias.to_owned(),
            reason,
        };
        if self.root.join(alias).symlink_metadata().is_ok() {
            return Err(failure(format!(
                "{} already exists; rename it before using this alias",
                self.root.join(alias).display()
            )));
        }
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err(failure("only Linux x86_64 builds are supported".to_owned()));
        }
        self.list()?.runners.into_iter()
            .filter(Runner::is_proton)
            .filter(|runner| crate::runner_alias::complete_proton(&self.root.join(runner.name())))
            .filter_map(|runner| crate::runner_alias::version(&self.root, runner.name(), alias).map(|version| (version, runner)))
            .max_by(|(a, ar), (b, br)| a.cmp(b).then_with(|| ar.name().cmp(br.name())))
            .map(|(_, runner)| runner.with_selection(alias))
            .ok_or_else(|| failure(format!("no compatible stable release is installed; use `wineo download {}` or select an installed version", if alias == "GE-Proton-Latest" { "ge-proton" } else { "proton-cachyos" })))
    }

    /// The newest Proton build installed, going by the version numbers in
    /// their names, so `GE-Proton11-7` is newer than `GE-Proton9-20`.
    pub fn newest_proton(&self) -> Result<Option<Runner>> {
        Ok(self
            .list()?
            .runners
            .into_iter()
            .filter(Runner::is_proton)
            .max_by(|a, b| version_order(a.name(), b.name())))
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

/// Compares names chunk by chunk, numbers by value and the rest as text.
fn version_order(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(name: &str) -> Vec<(bool, &str)> {
        let mut chunks = Vec::new();
        let mut start = 0;
        let bytes = name.as_bytes();
        for index in 1..=bytes.len() {
            if index == bytes.len()
                || bytes[index].is_ascii_digit() != bytes[index - 1].is_ascii_digit()
            {
                chunks.push((bytes[start].is_ascii_digit(), &name[start..index]));
                start = index;
            }
        }
        chunks
    }
    for (left, right) in chunks(a).into_iter().zip(chunks(b)) {
        let order = match (left, right) {
            ((true, l), (true, r)) => l
                .parse::<u64>()
                .unwrap_or(0)
                .cmp(&r.parse::<u64>().unwrap_or(0)),
            ((_, l), (_, r)) => l.cmp(r),
        };
        if order.is_ne() {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

/// What kind of runner a directory holds, if any. Proton is checked first,
/// because a Proton tree carries wine binaries under `files/bin` as well.
fn check(dir: &Path) -> std::result::Result<Runner, Skipped> {
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();

    // The `proton` script is exactly what PROTONPATH has to point at.
    let proton = dir.join("proton");
    if proton.exists() {
        return if crate::runner::is_executable(&proton) {
            fs::canonicalize(dir)
                .map(|root| Runner::proton(name, root).with_directory(dir))
                .map_err(|_| Skipped::NotExecutable)
        } else {
            Err(Skipped::NotExecutable)
        };
    }

    // A wine build needs both: every lifecycle command uses wineserver too.
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
    let wine = fs::canonicalize(wine).map_err(|_| Skipped::NotExecutable)?;
    let wineserver = fs::canonicalize(wineserver).map_err(|_| Skipped::NotExecutable)?;
    Ok(Runner::wine_build(name, wine, wineserver).with_directory(dir))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;

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

    fn proton_build(root: &Path, name: &str, version: &str) {
        install(
            root,
            name,
            &[
                ("proton", true),
                ("files/bin/wine", true),
                ("files/bin/wineserver", true),
                ("version", false),
            ],
        );
        fs::write(
            root.join("runners").join(name).join("version"),
            format!("1717451631 {version}\n"),
        )
        .unwrap();
    }

    #[test]
    fn a_proton_tree_is_recognized_by_its_proton_script() {
        let root = tempfile::tempdir().unwrap();
        proton_build(root.path(), "GE-Proton9-20", "GE-Proton9-20");
        let runner = runners(root.path()).resolve("GE-Proton9-20").unwrap();

        assert_eq!(runner.kind_name(), "proton");
        assert!(runner.is_proton());
        // Nothing reaches for Proton's own wine: umu runs everything.
        assert_eq!(runner.wine_path(), None);
        assert_eq!(runner.wineserver_path(), None);
        // Proton keeps its prefix under the compatdata directory it is given.
        assert_eq!(
            runner.wine_prefix(Path::new("/games/hades")),
            Path::new("/games/hades/pfx")
        );
        assert_eq!(runner.version().as_deref(), Some("GE-Proton9-20"));
    }

    #[test]
    fn proton_wins_over_the_wine_binaries_in_the_same_tree() {
        let root = tempfile::tempdir().unwrap();
        // A real Proton tree has bin/wine too, and must not read as a wine build.
        proton_build(root.path(), "GE-Proton9-20", "GE-Proton9-20");
        install(
            root.path(),
            "GE-Proton9-20",
            &[("bin/wine", true), ("bin/wineserver", true)],
        );
        let runner = runners(root.path()).resolve("GE-Proton9-20").unwrap();
        assert_eq!(runner.kind_name(), "proton");
    }

    #[test]
    fn a_proton_script_that_cannot_run_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        install(root.path(), "half-proton", &[("proton", false)]);
        let listing = runners(root.path()).list().unwrap();
        assert!(listing.runners.is_empty());
        assert_eq!(
            skipped(&listing, "half-proton"),
            Some(Skipped::NotExecutable)
        );
    }

    #[test]
    fn a_proton_build_without_a_version_file_just_has_no_version() {
        let root = tempfile::tempdir().unwrap();
        install(root.path(), "nameless", &[("proton", true)]);
        let runner = runners(root.path()).resolve("nameless").unwrap();
        assert_eq!(runner.version(), None);
    }

    #[test]
    fn resolving_finds_the_absolute_binaries() {
        let root = tempfile::tempdir().unwrap();
        wine_build(root.path(), "wine-tkg");
        let runner = runners(root.path()).resolve("wine-tkg").unwrap();
        assert_eq!(runner.name(), "wine-tkg");
        assert_eq!(runner.kind_name(), "wine");
        assert_eq!(
            runner.wine_path(),
            Some(root.path().join("runners/wine-tkg/bin/wine").as_path())
        );
        assert_eq!(
            runner.wineserver_path(),
            Some(
                root.path()
                    .join("runners/wine-tkg/bin/wineserver")
                    .as_path()
            )
        );
        // A wine prefix is the configured directory itself.
        assert_eq!(runner.wine_prefix(Path::new("/p")), Path::new("/p"));
    }

    #[test]
    fn resolving_a_runner_alias_keeps_the_concrete_executable_paths() {
        let root = tempfile::tempdir().unwrap();
        let concrete = root.path().join("builds/wine-tkg");
        for name in ["wine", "wineserver"] {
            let path = concrete.join("bin").join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::create_dir_all(root.path().join("runners")).unwrap();
        symlink(&concrete, root.path().join("runners/current")).unwrap();

        let runner = runners(root.path()).resolve("current").unwrap();
        let concrete_wine = concrete.join("bin/wine").canonicalize().unwrap();
        assert_eq!(runner.name(), "current");
        assert_eq!(runner.wine_path(), Some(concrete_wine.as_path()));
    }

    #[test]
    fn wine_always_means_the_system_one() {
        let root = tempfile::tempdir().unwrap();
        // Installed under the reserved name, and still ignored.
        wine_build(root.path(), "wine");
        let resolved = runners(root.path()).resolve("wine");
        match Runner::system() {
            Ok(system) => assert_eq!(
                resolved.unwrap().wine_path().map(Path::to_owned),
                system.wine_path().map(Path::to_owned)
            ),
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

    #[test]
    fn the_newest_proton_goes_by_version_numbers() {
        let root = tempfile::tempdir().unwrap();
        assert!(runners(root.path()).newest_proton().unwrap().is_none());
        for name in ["GE-Proton9-20", "GE-Proton11-7-x86_64", "GE-Proton10-3"] {
            proton_build(root.path(), name, name);
        }
        let newest = runners(root.path()).newest_proton().unwrap().unwrap();
        assert_eq!(newest.name(), "GE-Proton11-7-x86_64");
    }
    #[test]
    fn aliases_select_stable_family_versions_and_retain_resolved_runners() {
        let root = tempfile::tempdir().unwrap();
        let installed = runners(root.path());
        assert!(installed.resolve("GE-Proton-Latest").is_err());
        for name in [
            "GE-Proton9-20",
            "GE-Proton11-7-x86_64",
            "GE-Proton11-99-rc1",
            "proton-cachyos-12.0-20260921-slr-x86_64",
            "GE-Proton99-1-aarch64",
        ] {
            proton_build(root.path(), name, name);
        }
        let before = installed.resolve("GE-Proton-Latest").unwrap();
        assert_eq!(before.name(), "GE-Proton11-7-x86_64");
        assert_eq!(before.selection(), "GE-Proton-Latest");
        let serialized = toml::to_string(&before).unwrap();
        proton_build(root.path(), "GE-Proton12-1", "GE-Proton12-1");
        proton_build(root.path(), "GE-Proton8-1", "GE-Proton8-1");
        assert_eq!(
            installed.resolve("GE-Proton-Latest").unwrap().name(),
            "GE-Proton12-1"
        );
        assert_eq!(toml::from_str::<Runner>(&serialized).unwrap(), before);
        assert_eq!(
            installed.resolve("GE-Proton9-20").unwrap().selection(),
            "GE-Proton9-20"
        );
        assert_eq!(
            installed.resolve("Proton-CachyOS-Latest").unwrap().name(),
            "proton-cachyos-12.0-20260921-slr-x86_64"
        );
    }

    #[test]
    fn alias_re_resolves_when_the_selected_build_disappears_before_locking() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        proton_build(root.path(), "GE-Proton9-20", "GE-Proton9-20");
        proton_build(root.path(), "GE-Proton10-1", "GE-Proton10-1");
        let runners = runners(root.path());
        let selected = runners.resolve("GE-Proton-Latest").unwrap();
        assert_eq!(selected.name(), "GE-Proton10-1");
        fs::remove_dir_all(root.path().join("runners/GE-Proton10-1")).unwrap();
        let (runner, _guard) = runners
            .lock_resolved(&paths, "GE-Proton-Latest", selected)
            .unwrap();
        assert_eq!(runner.name(), "GE-Proton9-20");
        assert!(
            crate::build_lock::try_exclusive(
                &paths,
                crate::build_lock::Kind::Runner,
                runner.name(),
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn a_missing_concrete_build_after_resolution_uses_the_existing_error() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        wine_build(root.path(), "wine-tkg");
        let runners = runners(root.path());
        let selected = runners.resolve("wine-tkg").unwrap();
        fs::remove_dir_all(root.path().join("runners/wine-tkg")).unwrap();
        assert!(matches!(
            runners.lock_resolved(&paths, "wine-tkg", selected),
            Err(Error::UnknownRunner { .. })
        ));
    }

    #[test]
    fn alias_receipts_allow_renames_but_not_incompatible_or_corrupt_builds() {
        let root = tempfile::tempdir().unwrap();
        proton_build(root.path(), "renamed", "GE-Proton12-1");
        let path = root.path().join("runners/renamed/.wineo-install.json");
        let mut receipt = serde_json::json!({"provider":"ge-proton", "version":"GE-Proton12-1", "architecture":"x86_64", "variant":"baseline", "stable":true});
        fs::write(&path, receipt.to_string()).unwrap();
        assert_eq!(
            runners(root.path())
                .resolve("GE-Proton-Latest")
                .unwrap()
                .name(),
            "renamed"
        );
        receipt["stable"] = false.into();
        fs::write(&path, receipt.to_string()).unwrap();
        assert!(runners(root.path()).resolve("GE-Proton-Latest").is_err());
        fs::write(&path, "broken").unwrap();
        assert!(runners(root.path()).resolve("GE-Proton-Latest").is_err());
        fs::write(root.path().join("runners/GE-Proton-Latest"), "reserved").unwrap();
        assert!(
            runners(root.path())
                .resolve("GE-Proton-Latest")
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
    }
}
