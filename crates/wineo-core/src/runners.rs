//! Wine builds installed by extracting them into the runners root, plus the
//! system wine every config starts out using, and, when turned on, builds
//! other launchers installed.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::GlobalConfig;
use crate::error::{Error, IoContext, Result};
use crate::external_runners::{self, Comparison, Folder, Source};
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

/// Where a runner offered by `Runners::candidates` comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    System,
    Installed,
    Alias,
}

/// A runner a picker can offer, by the name a config stores for it.
#[derive(Debug)]
pub struct Candidate {
    pub kind: CandidateKind,
    pub name: String,
    pub runner: Result<Runner>,
}

/// Every runner wineo can use, usable or not.
#[derive(Debug, Default)]
pub struct Listing {
    /// One per name, the copy that wins by source priority.
    pub runners: Vec<Runner>,
    /// What's unusable in wineo's own runners root.
    pub skipped: Vec<(String, Skipped)>,
    /// What's unusable in other launchers' folders.
    pub external_skipped: Vec<ExternalSkipped>,
    /// Same-named builds in lower priority sources, which are never used.
    pub hidden: Vec<Hidden>,
}

impl Listing {
    /// The sources that have a different build under `name` than the one
    /// wineo uses. Identical copies, and ones that can't be told apart
    /// cheaply, aren't included.
    pub fn different_builds(&self, name: &str) -> Vec<Source> {
        self.hidden
            .iter()
            .filter(|hidden| hidden.name == name && hidden.comparison == Comparison::Different)
            .map(|hidden| hidden.source)
            .collect()
    }
}

/// A folder in another launcher's folder that isn't a usable runner.
#[derive(Debug, Clone)]
pub struct ExternalSkipped {
    pub source: Source,
    pub directory: PathBuf,
    pub reason: Skipped,
}

/// A build that a higher priority one with the same name hides.
#[derive(Debug, Clone)]
pub struct Hidden {
    pub name: String,
    pub source: Source,
    pub directory: PathBuf,
    pub comparison: Comparison,
}

/// The directory installed wine builds are extracted into, plus other
/// launchers' folders when those are turned on.
#[derive(Debug, Clone)]
pub struct Runners {
    root: PathBuf,
    external: Vec<Folder>,
}

impl Runners {
    pub fn new(paths: &Paths, global: &GlobalConfig) -> Result<Self> {
        let mut runners = Self::at(global.runner_root(paths)?);
        // A portable folder moves between machines, and a build that only
        // this machine's Steam has would break it on the next one.
        if global.external_runners == Some(true) && paths.portable_root().is_none() {
            runners.external = external_runners::folders(paths);
        }
        Ok(runners)
    }

    pub fn at(root: PathBuf) -> Self {
        Self {
            root,
            external: Vec::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The names the other launchers' folders give the build at `directory`,
    /// in priority order. Empty when external builds are off or it isn't in
    /// one of their folders. A launcher may call a folder something wineo
    /// can't use as a name, so this is the name wineo knows the build by.
    pub(crate) fn external_names_at(&self, directory: &Path) -> Vec<String> {
        let Ok(wanted) = fs::canonicalize(directory) else {
            return Vec::new();
        };
        external_runners::entries(&self.external)
            .into_iter()
            // Reserved names resolve to something else, an alias target or
            // the system wine, so they never mean the folder.
            .filter(|entry| {
                !entry.name.is_empty()
                    && !crate::runner_alias::is_alias(&entry.name)
                    && entry.name != Runner::SYSTEM
            })
            .filter(|entry| fs::canonicalize(&entry.directory).is_ok_and(|dir| dir == wanted))
            .map(|entry| entry.name)
            .collect()
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
        let reason = match check(&dir) {
            Ok(runner) => return Ok(runner),
            Err(reason) => reason,
        };
        // The highest priority folder with the name owns it, even when it's
        // broken, so a broken build never silently runs as another one.
        if dir.is_dir() {
            return Err(Error::NotARunner {
                name: name.to_owned(),
                reason: reason.as_str(),
            });
        }
        let owner = external_runners::entries(&self.external)
            .into_iter()
            .filter(|entry| entry.name == name)
            .find_map(|entry| check_entry(&entry));
        match owner {
            Some(Ok(runner)) => Ok(runner),
            Some(Err(reason)) => Err(Error::NotARunner {
                name: name.to_owned(),
                reason: reason.as_str(),
            }),
            None => Err(Error::UnknownRunner {
                name: name.to_owned(),
                installed: self.installed()?,
                root: self.root.clone(),
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
        // A wineo build may come from another root than this one. Builds from
        // other launchers are only ever found through this one's folders.
        let runners = if runner.source() == Source::Wineo {
            Self::at(
                runner
                    .directory()
                    .and_then(Path::parent)
                    .unwrap_or(&self.root)
                    .to_owned(),
            )
        } else {
            self.clone()
        };
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

    /// The system wine, then each runner in `listing`, then each alias, the
    /// order every runner picker offers them in. Ones that don't resolve are
    /// kept, so a caller can say why or leave them out.
    pub fn candidates(&self, listing: &Listing) -> Vec<Candidate> {
        let mut candidates = vec![Candidate {
            kind: CandidateKind::System,
            name: Runner::SYSTEM.to_owned(),
            runner: Runner::system(),
        }];
        candidates.extend(listing.runners.iter().map(|runner| Candidate {
            kind: CandidateKind::Installed,
            name: runner.name().to_owned(),
            runner: Ok(runner.clone()),
        }));
        candidates.extend(crate::runner_alias::ALIASES.iter().map(|alias| Candidate {
            kind: CandidateKind::Alias,
            name: (*alias).to_owned(),
            runner: self.resolve(alias),
        }));
        candidates
    }

    /// Every runner, one per name and sorted by name, with whatever hides
    /// behind each one compared to it. A missing root just means nothing is
    /// installed yet.
    pub fn list(&self) -> Result<Listing> {
        self.collect(true)
    }

    /// `list`, but without comparing hidden builds when only names matter.
    fn collect(&self, compare: bool) -> Result<Listing> {
        let mut listing = self.list_root()?;
        if self.external.is_empty() {
            return Ok(listing);
        }
        // Each name belongs to the first folder that has it, usable or not.
        // Broken owners stay in `skipped`, so they're only placeholders here.
        let mut owners: BTreeMap<String, (Option<Runner>, Vec<Runner>)> = listing
            .runners
            .drain(..)
            .map(|runner| (runner.name().to_owned(), (Some(runner), Vec::new())))
            .collect();
        for (name, _) in &listing.skipped {
            owners.entry(name.clone()).or_default();
        }
        for entry in external_runners::entries(&self.external) {
            let Some(checked) = check_entry(&entry) else {
                continue;
            };
            let runner = match checked {
                Ok(runner) => Some(runner),
                Err(reason) => {
                    listing.external_skipped.push(ExternalSkipped {
                        source: entry.source,
                        directory: entry.directory,
                        reason,
                    });
                    None
                }
            };
            if entry.name.is_empty() {
                continue;
            }
            match owners.entry(entry.name) {
                std::collections::btree_map::Entry::Vacant(vacant) => {
                    vacant.insert((runner, Vec::new()));
                }
                std::collections::btree_map::Entry::Occupied(mut occupied) => {
                    occupied.get_mut().1.extend(runner);
                }
            }
        }
        for (name, (owner, copies)) in owners {
            if compare {
                for copy in copies {
                    listing.hidden.push(Hidden {
                        name: name.clone(),
                        source: copy.source(),
                        directory: copy.directory().map(Path::to_owned).unwrap_or_default(),
                        comparison: owner.as_ref().map_or(Comparison::Unknown, |owner| {
                            external_runners::compare(owner, &copy)
                        }),
                    });
                }
            }
            listing.runners.extend(owner);
        }
        Ok(listing)
    }

    /// Everything in wineo's own runners root, sorted by name.
    fn list_root(&self) -> Result<Listing> {
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
        // The aliases mean what `wineo download` fetched, so only wineo's own
        // builds count.
        self.list_root()?.runners.into_iter()
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
            .collect(false)?
            .runners
            .into_iter()
            .filter(Runner::is_proton)
            .max_by(|a, b| version_order(a.name(), b.name())))
    }

    fn installed(&self) -> Result<Vec<String>> {
        Ok(self
            .collect(false)?
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

/// What an entry in another launcher's folder holds. None when it doesn't
/// count at all: a Steam library folder only counts when it's really Proton,
/// since games and Valve's runtimes share it.
fn check_entry(entry: &external_runners::Entry) -> Option<std::result::Result<Runner, Skipped>> {
    let checked = if entry.name.is_empty() {
        Err(Skipped::InvalidName)
    } else if crate::runner_alias::is_alias(&entry.name) {
        Err(Skipped::AliasReserved)
    } else if entry.name == Runner::SYSTEM {
        Err(Skipped::Reserved)
    } else {
        check_named(&entry.directory, &entry.name)
    };
    if entry.proton_only && !checked.as_ref().is_ok_and(Runner::is_proton) {
        return None;
    }
    Some(checked.map(|runner| runner.with_source(entry.source)))
}

fn check(dir: &Path) -> std::result::Result<Runner, Skipped> {
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    check_named(dir, name)
}

/// What kind of runner a directory holds, if any. Proton is checked first,
/// because a Proton tree carries wine binaries under `files/bin` as well.
/// The name is separate, since another launcher's folder name may not be
/// usable as one.
fn check_named(dir: &Path, name: &str) -> std::result::Result<Runner, Skipped> {
    let name = name.to_owned();

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

/// Size of a concrete managed build, in logical file bytes.
pub fn size(paths: &Paths, name: &str) -> Result<u64> {
    let (directory, _guard) = removable(paths, name)?;
    crate::build_removal::directory_size(&directory)
}

/// Removes an unused managed build and reports bytes and skipped configs.
pub fn remove(paths: &Paths, name: &str) -> Result<crate::build_removal::Removed> {
    let directory = removable_directory(paths, name)?;
    crate::build_removal::remove(
        paths,
        &crate::build_lock::Lock::new(paths, crate::build_lock::Kind::Runner, name),
        &directory,
    )
}

fn removable_directory(paths: &Paths, name: &str) -> Result<PathBuf> {
    if name == Runner::SYSTEM || crate::runner_alias::is_alias(name) {
        return Err(Error::UnmanagedBuild(name.to_owned()));
    }
    let global = GlobalConfig::load(paths)?;
    let runners = Runners::new(paths, &global)?;
    // A root inside a launcher's folder is that launcher's, so nothing in it
    // gets swept either.
    let owner = external_runners::root_owner(paths, runners.root());
    if owner.is_none() {
        crate::build_removal::sweep_best_effort(
            paths,
            crate::build_lock::Kind::Runner,
            runners.root(),
        );
    }
    removable_directory_in(&runners, name, owner)
}

fn removable_directory_in(
    runners: &Runners,
    name: &str,
    root_owner: Option<Source>,
) -> Result<PathBuf> {
    let runner = runners.resolve(name)?;
    let launcher = match (runner.source(), root_owner) {
        (Source::Wineo, owner) => owner,
        (other, _) => Some(other),
    };
    if let Some(launcher) = launcher {
        return Err(Error::ExternalBuild {
            name: name.to_owned(),
            launcher,
        });
    }
    let directory = runners.root().join(runner.name());
    crate::build_removal::managed(&directory, name)?;
    Ok(directory)
}

fn removable(paths: &Paths, name: &str) -> Result<(PathBuf, crate::build_lock::Guard)> {
    let directory = removable_directory(paths, name)?;
    let guard = crate::build_lock::shared(paths, crate::build_lock::Kind::Runner, name)?;
    crate::build_removal::managed(&directory, name)?;
    Ok((directory, guard))
}

#[cfg(test)]
mod tests;
