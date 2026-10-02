//! DXVK and vkd3d-proton builds, installed by extracting them into their
//! drop-in roots the way wine runners are.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::GlobalConfig;
use crate::error::{Error, IoContext, Result};
use crate::name;
use crate::paths::Paths;

/// The reserved name meaning whatever build is newest.
pub const LATEST: &str = "latest";

/// One of the two Direct3D implementations an environment can install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Dxvk,
    Vkd3d,
}

impl Component {
    pub const ALL: [Component; 2] = [Component::Dxvk, Component::Vkd3d];

    pub fn as_str(self) -> &'static str {
        match self {
            Component::Dxvk => "dxvk",
            Component::Vkd3d => "vkd3d",
        }
    }

    /// The 64-bit and 32-bit directories the build ships. The two projects
    /// disagree on what to call the 32-bit one.
    pub fn arch_dirs(self) -> (&'static str, &'static str) {
        match self {
            Component::Dxvk => ("x64", "x32"),
            Component::Vkd3d => ("x64", "x86"),
        }
    }

    /// How the component is written in a message, as opposed to the config
    /// spelling `as_str` gives.
    pub fn label(self) -> &'static str {
        match self {
            Component::Dxvk => "DXVK",
            Component::Vkd3d => "vkd3d-proton",
        }
    }

    /// The DLLs wineo points at the build with a native override. Fixed,
    /// unlike the files it copies, because these are what the component is
    /// meant to take over.
    pub fn overrides(self) -> &'static [&'static str] {
        match self {
            Component::Dxvk => &["d3d8", "d3d9", "d3d10core", "d3d11", "dxgi"],
            Component::Vkd3d => &["d3d12", "d3d12core"],
        }
    }

    /// A DLL that has to be there for a directory to be this component.
    fn marker(self) -> &'static str {
        match self {
            Component::Dxvk => "dxgi.dll",
            Component::Vkd3d => "d3d12.dll",
        }
    }
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a directory in a component root is not usable as a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    Reserved,
    InvalidName,
    Missing64,
    Missing32,
}

impl Skipped {
    /// Which files were missing depends on the component, so the reason is
    /// built rather than being one fixed string.
    pub fn describe(self, component: Component) -> String {
        let (wide, narrow) = component.arch_dirs();
        let marker = component.marker();
        match self {
            Skipped::Reserved => format!("`{LATEST}` is reserved for the newest build"),
            Skipped::InvalidName => {
                "not a usable name: use letters, digits, `-`, `_`, and `.`".to_owned()
            }
            Skipped::Missing64 => format!("no {wide}/{marker}"),
            Skipped::Missing32 => format!("no {narrow}/{marker}"),
        }
    }
}

/// An extracted build, ready to copy DLLs out of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    component: Component,
    name: String,
    dir: PathBuf,
}

impl Build {
    pub fn component(&self) -> Component {
        self.component
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Everything in a component root, usable or not.
#[derive(Debug, Default)]
pub struct Listing {
    pub builds: Vec<Build>,
    pub skipped: Vec<(String, Skipped)>,
}

/// The directory one component's builds are extracted into.
#[derive(Debug, Clone)]
pub struct Builds {
    component: Component,
    root: PathBuf,
}

impl Builds {
    pub fn new(paths: &Paths, global: &GlobalConfig, component: Component) -> Result<Self> {
        let configured = match component {
            Component::Dxvk => &global.defaults.dxvk_root,
            Component::Vkd3d => &global.defaults.vkd3d_root,
        };
        let root = match configured {
            Some(root) => paths.config_path(root)?,
            None => match component {
                Component::Dxvk => paths.default_dxvk_root(),
                Component::Vkd3d => paths.default_vkd3d_root(),
            },
        };
        Ok(Self::at(component, root))
    }

    pub fn at(component: Component, root: PathBuf) -> Self {
        Self { component, root }
    }

    pub fn component(&self) -> Component {
        self.component
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The build a config's `dxvk` or `vkd3d` value names. `latest` is
    /// resolved here rather than written back, so extracting a newer build is
    /// all it takes to move to it.
    pub fn resolve(&self, name: &str) -> Result<Build> {
        if name == LATEST {
            return self.latest();
        }
        if !name::is_valid_runner(name) {
            return Err(Error::InvalidBuildName {
                component: self.component.as_str(),
                name: name.to_owned(),
            });
        }
        let dir = self.root.join(name);
        match check(self.component, &dir) {
            Ok(build) => Ok(build),
            Err(_) if !dir.is_dir() => Err(Error::UnknownBuild {
                component: self.component.as_str(),
                name: name.to_owned(),
                installed: self.installed()?,
                root: self.root.clone(),
            }),
            Err(reason) => Err(Error::NotABuild {
                component: self.component.as_str(),
                name: name.to_owned(),
                reason: reason.describe(self.component),
            }),
        }
    }

    pub fn resolve_locked(
        &self,
        paths: &Paths,
        name: &str,
    ) -> Result<(Build, crate::build_lock::Guard)> {
        self.lock_resolved(paths, name, self.resolve(name)?)
    }

    pub(crate) fn lock_resolved(
        &self,
        paths: &Paths,
        name: &str,
        mut build: Build,
    ) -> Result<(Build, crate::build_lock::Guard)> {
        for attempt in 0..2 {
            let guard = crate::build_lock::shared(paths, self.component.into(), build.name())?;
            if build.dir().is_dir() {
                return Ok((build, guard));
            }
            drop(guard);
            if attempt == 0 && name == LATEST {
                build = self.resolve(name)?;
            } else {
                return Err(Error::UnknownBuild {
                    component: self.component.as_str(),
                    name: build.name().to_owned(),
                    installed: self.installed()?,
                    root: self.root.clone(),
                });
            }
        }
        unreachable!("the second missing build returns an error")
    }

    /// The newest installed build, by the version numbers in its name.
    pub fn latest(&self) -> Result<Build> {
        self.list()?
            .builds
            .into_iter()
            .max_by(|a, b| {
                version_key(&a.name)
                    .cmp(&version_key(&b.name))
                    .then_with(|| a.name.cmp(&b.name))
            })
            .ok_or_else(|| Error::NoBuilds {
                component: self.component.as_str(),
                root: self.root.clone(),
            })
    }

    /// Everything in the root, sorted by name. A missing root just means
    /// nothing is installed yet.
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
            if name == LATEST {
                listing.skipped.push((name.to_owned(), Skipped::Reserved));
            } else if !name::is_valid_runner(name) {
                listing
                    .skipped
                    .push((name.to_owned(), Skipped::InvalidName));
            } else {
                match check(self.component, &path) {
                    Ok(build) => listing.builds.push(build),
                    Err(reason) => listing.skipped.push((name.to_owned(), reason)),
                }
            }
        }
        listing.builds.sort_by(|a, b| a.name.cmp(&b.name));
        listing.skipped.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(listing)
    }

    fn installed(&self) -> Result<Vec<String>> {
        Ok(self
            .list()?
            .builds
            .into_iter()
            .map(|build| build.name)
            .collect())
    }
}

/// Whether `dir` holds a build of `component`. Both architectures are required:
/// a prefix gets 32-bit DLLs too, and half a build would install silently wrong.
fn check(component: Component, dir: &Path) -> std::result::Result<Build, Skipped> {
    let (wide, narrow) = component.arch_dirs();
    let marker = component.marker();
    if !dir.join(wide).join(marker).is_file() {
        return Err(Skipped::Missing64);
    }
    if !dir.join(narrow).join(marker).is_file() {
        return Err(Skipped::Missing32);
    }
    Ok(Build {
        component,
        name: dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned(),
        dir: dir.to_owned(),
    })
}

/// Every run of digits in a name, in order, so builds compare by version
/// rather than as strings. `dxvk-2.10` gives `[2, 10]`, which sorts above
/// `dxvk-2.7`'s `[2, 7]` where a plain string compare gets it backwards. A
/// name with no digits gives an empty key and sorts below every other.
fn version_key(name: &str) -> Vec<u64> {
    let mut key = Vec::new();
    let mut rest = name;
    while let Some(start) = rest.find(|c: char| c.is_ascii_digit()) {
        let digits = &rest[start..];
        let end = digits
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(digits.len());
        // A name long enough to overflow is not a version, so it just sorts high.
        key.push(digits[..end].parse().unwrap_or(u64::MAX));
        rest = &digits[end..];
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builds(root: &Path, component: Component) -> Builds {
        Builds::at(component, root.join(component.as_str()))
    }

    /// A build of stub DLL files, enough for discovery without a real one.
    fn install(root: &Path, component: Component, name: &str, dirs: &[&str]) {
        let dir = root.join(component.as_str()).join(name);
        for arch in dirs {
            let arch = dir.join(arch);
            fs::create_dir_all(&arch).unwrap();
            fs::write(arch.join(component.marker()), "").unwrap();
        }
        if dirs.is_empty() {
            fs::create_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn latest_re_resolves_after_the_selected_build_disappears() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        for component in Component::ALL {
            let (wide, narrow) = component.arch_dirs();
            install(root.path(), component, "1.0", &[wide, narrow]);
            install(root.path(), component, "2.0", &[wide, narrow]);
            let builds = builds(root.path(), component);
            let selected = builds.resolve(LATEST).unwrap();
            fs::remove_dir_all(selected.dir()).unwrap();
            let (build, _guard) = builds.lock_resolved(&paths, LATEST, selected).unwrap();
            assert_eq!(build.name(), "1.0");
            assert!(
                crate::build_lock::try_exclusive(&paths, component.into(), "1.0")
                    .unwrap()
                    .is_none()
            );
        }
    }

    fn dxvk(root: &Path, name: &str) {
        install(root, Component::Dxvk, name, &["x64", "x32"]);
    }

    fn vkd3d(root: &Path, name: &str) {
        install(root, Component::Vkd3d, name, &["x64", "x86"]);
    }

    fn skipped(listing: &Listing, name: &str) -> Option<Skipped> {
        listing
            .skipped
            .iter()
            .find(|(found, _)| found == name)
            .map(|(_, reason)| *reason)
    }

    #[test]
    fn a_missing_root_just_means_nothing_is_installed() {
        let root = tempfile::tempdir().unwrap();
        for component in Component::ALL {
            let listing = builds(root.path(), component).list().unwrap();
            assert!(listing.builds.is_empty(), "{component}");
            assert!(listing.skipped.is_empty(), "{component}");
        }
    }

    #[test]
    fn a_build_needs_both_architectures() {
        let root = tempfile::tempdir().unwrap();
        dxvk(root.path(), "2.7");
        install(root.path(), Component::Dxvk, "no-32", &["x64"]);
        install(root.path(), Component::Dxvk, "no-64", &["x32"]);
        install(root.path(), Component::Dxvk, "empty", &[]);

        let listing = builds(root.path(), Component::Dxvk).list().unwrap();
        let names: Vec<&str> = listing.builds.iter().map(Build::name).collect();
        assert_eq!(names, ["2.7"]);
        assert_eq!(skipped(&listing, "no-32"), Some(Skipped::Missing32));
        assert_eq!(skipped(&listing, "no-64"), Some(Skipped::Missing64));
        assert_eq!(skipped(&listing, "empty"), Some(Skipped::Missing64));
    }

    #[test]
    fn the_two_projects_disagree_on_the_32_bit_directory() {
        let root = tempfile::tempdir().unwrap();
        // vkd3d-proton ships x86, DXVK ships x32, so each only reads its own.
        vkd3d(root.path(), "2.14");
        install(
            root.path(),
            Component::Vkd3d,
            "x32-not-x86",
            &["x64", "x32"],
        );

        let listing = builds(root.path(), Component::Vkd3d).list().unwrap();
        let names: Vec<&str> = listing.builds.iter().map(Build::name).collect();
        assert_eq!(names, ["2.14"]);
        assert_eq!(skipped(&listing, "x32-not-x86"), Some(Skipped::Missing32));
    }

    #[test]
    fn unusable_names_are_skipped() {
        let root = tempfile::tempdir().unwrap();
        dxvk(root.path(), "2.7");
        dxvk(root.path(), LATEST);
        dxvk(root.path(), ".tmp-extract");
        dxvk(root.path(), "bad name");

        let listing = builds(root.path(), Component::Dxvk).list().unwrap();
        let names: Vec<&str> = listing.builds.iter().map(Build::name).collect();
        assert_eq!(names, ["2.7"]);
        assert_eq!(skipped(&listing, LATEST), Some(Skipped::Reserved));
        assert_eq!(
            skipped(&listing, ".tmp-extract"),
            Some(Skipped::InvalidName)
        );
        assert_eq!(skipped(&listing, "bad name"), Some(Skipped::InvalidName));
    }

    #[test]
    fn resolving_finds_the_build_directory() {
        let root = tempfile::tempdir().unwrap();
        dxvk(root.path(), "dxvk-2.7");
        let build = builds(root.path(), Component::Dxvk)
            .resolve("dxvk-2.7")
            .unwrap();
        assert_eq!(build.name(), "dxvk-2.7");
        assert_eq!(build.component(), Component::Dxvk);
        assert_eq!(build.dir(), root.path().join("dxvk/dxvk-2.7"));
    }

    #[test]
    fn an_unknown_build_says_what_is_installed() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), Component::Dxvk);

        let message = builds.resolve("nope").unwrap_err().to_string();
        assert!(message.contains("is not installed"), "{message}");
        assert!(message.contains("dxvk"), "{message}");
        assert!(
            message.contains("Get one with: wineo download dxvk"),
            "{message}"
        );

        dxvk(root.path(), "2.7");
        let message = builds.resolve("nope").unwrap_err().to_string();
        assert!(message.contains("2.7"), "{message}");
    }

    #[test]
    fn a_half_extracted_build_says_what_was_missing() {
        let root = tempfile::tempdir().unwrap();
        install(root.path(), Component::Dxvk, "half", &["x64"]);
        let err = builds(root.path(), Component::Dxvk)
            .resolve("half")
            .unwrap_err();
        assert!(
            matches!(&err, Error::NotABuild { name, .. } if name == "half"),
            "{err}"
        );
        assert!(err.to_string().contains("x32/dxgi.dll"), "{err}");
    }

    #[test]
    fn an_unusable_name_is_refused_before_looking_at_the_disk() {
        let root = tempfile::tempdir().unwrap();
        for name in ["..", ".", ".hidden", "has space", "a/b"] {
            assert!(
                matches!(
                    builds(root.path(), Component::Dxvk).resolve(name),
                    Err(Error::InvalidBuildName { .. })
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn latest_compares_versions_as_numbers() {
        let root = tempfile::tempdir().unwrap();
        dxvk(root.path(), "dxvk-2.7");
        dxvk(root.path(), "dxvk-2.10");
        // A plain string sort would put 2.7 last and get this backwards.
        let build = builds(root.path(), Component::Dxvk)
            .resolve(LATEST)
            .unwrap();
        assert_eq!(build.name(), "dxvk-2.10");
    }

    #[test]
    fn latest_puts_a_name_with_no_version_last() {
        let root = tempfile::tempdir().unwrap();
        dxvk(root.path(), "nightly");
        assert_eq!(
            builds(root.path(), Component::Dxvk)
                .resolve(LATEST)
                .unwrap()
                .name(),
            "nightly"
        );
        dxvk(root.path(), "dxvk-1.0");
        assert_eq!(
            builds(root.path(), Component::Dxvk)
                .resolve(LATEST)
                .unwrap()
                .name(),
            "dxvk-1.0"
        );
    }

    #[test]
    fn latest_is_stable_when_two_names_carry_the_same_numbers() {
        let root = tempfile::tempdir().unwrap();
        dxvk(root.path(), "a-2.7");
        dxvk(root.path(), "b-2.7");
        let builds = builds(root.path(), Component::Dxvk);
        let first = builds.resolve(LATEST).unwrap();
        assert_eq!(first.name(), "b-2.7");
        assert_eq!(builds.resolve(LATEST).unwrap(), first);
    }

    #[test]
    fn latest_with_nothing_installed_says_where_to_extract_to() {
        let root = tempfile::tempdir().unwrap();
        let err = builds(root.path(), Component::Dxvk)
            .resolve(LATEST)
            .unwrap_err();
        assert!(matches!(err, Error::NoBuilds { .. }), "{err}");
        let message = err.to_string();
        assert!(message.contains("dxvk"), "{message}");
        assert!(
            message.contains("Get one with: wineo download dxvk"),
            "{message}"
        );
    }

    #[test]
    fn version_keys_split_every_run_of_digits() {
        assert_eq!(version_key("dxvk-2.10"), [2, 10]);
        assert_eq!(version_key("2.7"), [2, 7]);
        assert_eq!(version_key("dxvk-gplasync-v2.6"), [2, 6]);
        assert!(version_key("nightly").is_empty());
        // Too long to be a version, so it sorts high rather than panicking.
        assert_eq!(version_key(&"9".repeat(40)), [u64::MAX]);
    }

    #[test]
    fn the_roots_follow_the_global_config() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| std::ffi::OsString::from("/home/me")))
                .unwrap();

        for (component, default) in [(Component::Dxvk, "dxvk"), (Component::Vkd3d, "vkd3d")] {
            let builds = Builds::new(&paths, &GlobalConfig::default(), component).unwrap();
            assert_eq!(
                builds.root(),
                Path::new("/home/me/.local/share/wineo").join(default)
            );
        }

        let mut global = GlobalConfig::default();
        global.defaults.dxvk_root = Some(root.path().join("elsewhere").display().to_string());
        let moved = Builds::new(&paths, &global, Component::Dxvk).unwrap();
        assert_eq!(moved.root(), root.path().join("elsewhere"));
        // Moving one root leaves the other where it was.
        let other = Builds::new(&paths, &global, Component::Vkd3d).unwrap();
        assert_eq!(other.root(), Path::new("/home/me/.local/share/wineo/vkd3d"));
    }
}
