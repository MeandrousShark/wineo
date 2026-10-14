//! Planning the import of a whole Heroic library.

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::components::Component;
use crate::config::{Arch, GlobalConfig};
use crate::environment::{self, Environment, HeroicIdSnapshot};
use crate::heroic::{
    Global, HeroicCandidate, HeroicGameKind, HeroicImport, HeroicIssue, HeroicIssueKind,
    HeroicListing, HeroicNativeImport, HeroicRunnerKind, HeroicSharedPrefix, HeroicSource, Inputs,
    Library,
};
use crate::paths::{self, Paths};
use crate::runners::Runners;

use super::native::exe_problem;
use super::{GamePlan, NativePlan, Options, Policy, SkipReason, Warning};

/// Every Heroic configuration wineo can find, or just the one at `explicit`.
/// Finding none is not an error, so the caller says how to point at one.
pub fn sources(paths: &Paths, explicit: Option<&str>) -> crate::Result<Vec<HeroicSource>> {
    if let Some(path) = explicit {
        return Ok(vec![HeroicSource::from_config_dir(paths.absolute(path)?)]);
    }
    HeroicSource::discover_from_env()
}

/// Heroic's list, with a native game whose exe can't be run marked as not
/// importable, the same way a launch would refuse it.
pub fn listing(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
) -> crate::Result<HeroicListing> {
    let mut listing = source.list_candidates()?;
    for candidate in &mut listing.candidates {
        if candidate.kind != HeroicGameKind::Native || native_blocker(candidate).is_some() {
            continue;
        }
        let Some(exe) = candidate.executable.clone() else {
            continue;
        };
        let name = default_name(&candidate.title, Some(&exe));
        if let Some(message) = exe_problem(paths, global, &name, &BTreeMap::new(), &exe) {
            candidate.issues.push(HeroicIssue {
                kind: HeroicIssueKind::Missing,
                field: Some("executable".to_owned()),
                message,
            });
        }
    }
    Ok(listing)
}

/// What keeps a native candidate from being imported. Unsupported settings,
/// like wrappers, are only reported, as they are for Windows games.
fn native_blocker(candidate: &HeroicCandidate) -> Option<&HeroicIssue> {
    candidate.issues.iter().find(|issue| {
        (matches!(
            issue.kind,
            HeroicIssueKind::Missing | HeroicIssueKind::Invalid
        ) && issue.field.as_deref() != Some("launcherArgs"))
            || (issue.kind == HeroicIssueKind::Unsupported
                && matches!(issue.field.as_deref(), Some("platform") | Some("runner")))
    })
}

/// The issue that makes `candidate` unimportable, the same rule guided
/// selection and a bulk import both filter candidates by. `None` means it can
/// be imported.
pub fn blocker(candidate: &HeroicCandidate) -> Option<&HeroicIssue> {
    if candidate.kind == HeroicGameKind::Native
        && !candidate
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("platform"))
    {
        return native_blocker(candidate);
    }
    if candidate.kind != HeroicGameKind::Windows {
        return candidate
            .issues
            .iter()
            .find(|issue| issue.kind == HeroicIssueKind::Unsupported);
    }
    candidate.issues.iter().find(|issue| {
        (matches!(
            issue.kind,
            HeroicIssueKind::Missing | HeroicIssueKind::Invalid
        ) && issue.field.as_deref() != Some("launcherArgs"))
            || (issue.field.as_deref() == Some("runner")
                && issue.kind == HeroicIssueKind::Unsupported)
    })
}

/// The environment name a game would get if nobody chose one.
pub fn default_name(title: &str, executable: Option<&Path>) -> String {
    let exe = executable
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    crate::targets::suggest_name(title, &exe)
}

/// An environment already on disk, as much as matching a Heroic candidate
/// against it needs.
#[derive(Clone)]
struct ExistingEnvironment {
    name: String,
    heroic_id: Option<String>,
    snapshot: Option<HeroicIdSnapshot>,
    /// None for a native environment.
    prefix: Option<PathBuf>,
    /// Every target's exe, expanded and resolved the same way as `prefix`.
    target_exes: Vec<PathBuf>,
}

fn existing_environments(paths: &Paths) -> crate::Result<Vec<ExistingEnvironment>> {
    let mut found = Vec::new();
    for name in environment::names(paths)? {
        let Ok((loaded, snapshot)) = Environment::load_for_heroic_match(paths, &name) else {
            continue;
        };
        let prefix = match loaded.prefix(paths) {
            Ok(prefix) => Some(prefix),
            Err(crate::error::Error::NoPrefix(_)) => None,
            Err(_) => continue,
        };
        let target_exes = loaded
            .config
            .targets
            .values()
            .filter_map(|target| paths.resolve_stored(&target.exe).ok().flatten())
            .map(|exe| paths::resolve(&exe))
            .collect();
        found.push(ExistingEnvironment {
            name,
            heroic_id: loaded.config.heroic_id.clone(),
            snapshot: Some(snapshot),
            prefix: prefix.as_deref().map(paths::resolve),
            target_exes,
        });
    }
    Ok(found)
}

/// Which existing environment a Heroic candidate was already imported as.
struct ExistingMatch {
    name: String,
    /// Whether that environment predates recording the Heroic id, and so
    /// still needs it backfilled.
    needs_id: bool,
}

/// Matches a Heroic candidate to an environment already imported from it: by
/// its recorded id first, then for older imports, by the prefix it adopted,
/// a target that runs its exe, or its default name.
fn match_existing(
    existing: &[ExistingEnvironment],
    candidate_id: &str,
    candidate_prefix: Option<&Path>,
    candidate_exe: Option<&Path>,
    default_name: &str,
) -> Option<ExistingMatch> {
    if let Some(environment) = existing
        .iter()
        .find(|environment| environment.heroic_id.as_deref() == Some(candidate_id))
    {
        return Some(ExistingMatch {
            name: environment.name.clone(),
            needs_id: false,
        });
    }
    if let Some(prefix) = candidate_prefix {
        let resolved = paths::resolve(prefix);
        if let Some(environment) = existing.iter().find(|environment| {
            environment.heroic_id.is_none() && environment.prefix.as_ref() == Some(&resolved)
        }) {
            return Some(ExistingMatch {
                name: environment.name.clone(),
                needs_id: true,
            });
        }
    }
    if let Some(exe) = candidate_exe {
        let resolved = paths::resolve(exe);
        if let Some(environment) = existing.iter().find(|environment| {
            environment.heroic_id.is_none() && environment.target_exes.contains(&resolved)
        }) {
            return Some(ExistingMatch {
                name: environment.name.clone(),
                needs_id: true,
            });
        }
    }
    existing
        .iter()
        .find(|environment| environment.heroic_id.is_none() && environment.name == default_name)
        .map(|environment| ExistingMatch {
            name: environment.name.clone(),
            needs_id: true,
        })
}

/// Records `heroic_id` on an environment imported before the id was kept, so
/// later matching finds it directly. Planning only queues these, since it
/// never writes.
#[derive(Debug, Clone)]
pub struct Backfill {
    pub environment: String,
    pub heroic_id: String,
    snapshot: HeroicIdSnapshot,
}

impl Backfill {
    /// Writes the id, unless the environment changed since it was matched.
    pub fn record(&self, paths: &Paths) -> crate::Result<()> {
        Environment::record_heroic_id(paths, &self.environment, &self.heroic_id, &self.snapshot)
    }
}

/// Notes that `matched` needs its id recorded, and treats it as recorded from
/// here on so a later candidate can't claim the same environment.
fn queue_backfill(
    existing: &mut [ExistingEnvironment],
    backfills: &mut Vec<Backfill>,
    matched: &ExistingMatch,
    candidate_id: &str,
) {
    let environment = existing
        .iter_mut()
        .find(|environment| environment.name == matched.name)
        .expect("matched environment is present");
    backfills.push(Backfill {
        environment: matched.name.clone(),
        heroic_id: candidate_id.to_owned(),
        snapshot: environment
            .snapshot
            .clone()
            .expect("disk snapshot is present"),
    });
    environment.heroic_id = Some(candidate_id.to_owned());
}

/// What a bulk plan is made with.
#[derive(Debug, Clone, Default)]
pub struct LibraryOptions {
    /// Keep every prefix where it is instead of copying it.
    pub adopt: bool,
    /// Resolves a missing registry architecture marker.
    pub arch: Option<Arch>,
}

/// A game a bulk plan would import. A plan is large, but a library only holds
/// a few dozen of them, so they aren't boxed.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Game {
    Windows(GamePlan),
    Native(NativePlan),
}

impl Game {
    pub fn name(&self) -> &str {
        match self {
            Game::Windows(plan) => plan.name(),
            Game::Native(plan) => plan.name(),
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Game::Windows(plan) => &plan.heroic().title,
            Game::Native(plan) => &plan.heroic().title,
        }
    }

    pub fn warnings(&self) -> Vec<Warning> {
        match self {
            Game::Windows(plan) => plan.warnings(),
            Game::Native(plan) => plan.warnings(),
        }
    }
}

/// A game left out of a bulk import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub name: String,
    pub title: String,
    pub reason: SkipReason,
}

/// A game that's ready except for one missing DXVK or vkd3d-proton build, so
/// it's held back until the frontend decides whether to get it.
#[derive(Debug, Clone)]
pub struct PendingGame {
    plan: GamePlan,
    component: Component,
    version: Option<String>,
    /// The resolve error's own message, hint included, reused as the skip
    /// reason if the build doesn't turn up.
    reason: String,
}

impl PendingGame {
    pub fn new(
        plan: GamePlan,
        component: Component,
        version: Option<String>,
        reason: String,
    ) -> Self {
        Self {
            plan,
            component,
            version,
            reason,
        }
    }

    pub fn plan(&self) -> &GamePlan {
        &self.plan
    }

    /// The build it's waiting for.
    pub fn component(&self) -> Component {
        self.component
    }

    /// The exact version asked for, or None for the latest.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Looks again at what's installed, after something may have been
    /// downloaded. A game may still be waiting, on a different build.
    fn recheck(&mut self, paths: &Paths, global: &GlobalConfig) {
        match self.plan.missing_build(paths, global) {
            Ok(None) => {}
            Ok(Some(missing)) => {
                self.component = missing.component;
                self.version = missing.version.clone();
                self.reason = missing.message();
            }
            Err(error) => self.reason = error.to_string(),
        }
    }
}

/// One line of a bulk plan.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Entry {
    Ready(Game),
    Pending(PendingGame),
    Skipped(Skipped),
}

/// The games waiting on one kind of build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOffer {
    pub component: Component,
    /// Each version asked for, once, in the order the games come. None is a
    /// game that takes the latest.
    pub versions: Vec<Option<String>>,
    /// How many games are waiting.
    pub games: usize,
}

/// A whole Heroic library sorted into what a bulk import would do with each
/// game.
#[derive(Debug, Clone)]
pub struct LibraryPlan {
    runner: Option<String>,
    entries: Vec<Entry>,
    already: Vec<String>,
    backfills: Vec<Backfill>,
}

impl LibraryPlan {
    /// A plan assembled from entries a frontend sorted itself.
    // Only the CLI's prompt tests need to build plans by hand, so it's kept
    // out of the documented API that frontends use.
    #[doc(hidden)]
    pub fn from_entries(runner: Option<String>, already: Vec<String>, entries: Vec<Entry>) -> Self {
        Self {
            runner,
            entries,
            already,
            backfills: Vec::new(),
        }
    }

    /// The runner Windows games are imported with. None when every game is
    /// native.
    pub fn runner(&self) -> Option<&str> {
        self.runner.as_deref()
    }

    /// Every game in the order to show them: native games first, then Windows
    /// ones, and games that waited for a build last.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The names of environments that were already imported from a game in the
    /// library.
    pub fn already(&self) -> &[String] {
        &self.already
    }

    /// The environments matched to a game that predate the game's id being
    /// recorded. Recording them is `record_backfills`.
    pub fn backfills(&self) -> &[Backfill] {
        &self.backfills
    }

    /// Records the Heroic id on environments that were imported before ids
    /// were kept. It's harmless metadata that only helps matching later, so a
    /// frontend may write it whether or not anything is imported.
    pub fn record_backfills(&self, paths: &Paths) -> crate::Result<()> {
        for backfill in &self.backfills {
            backfill.record(paths)?;
        }
        Ok(())
    }

    /// The builds games are waiting on, one entry per kind, in dependency
    /// order.
    pub fn pending_builds(&self) -> Vec<BuildOffer> {
        let pending: Vec<&PendingGame> = self
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Pending(game) => Some(game),
                _ => None,
            })
            .collect();
        Component::ALL
            .into_iter()
            .filter_map(|component| {
                let games: Vec<&&PendingGame> = pending
                    .iter()
                    .filter(|game| game.component == component)
                    .collect();
                if games.is_empty() {
                    return None;
                }
                let mut versions = Vec::new();
                for game in &games {
                    let version = game.version.clone();
                    if !versions.contains(&version) {
                        versions.push(version);
                    }
                }
                Some(BuildOffer {
                    component,
                    versions,
                    games: games.len(),
                })
            })
            .collect()
    }

    /// Looks again at what's installed for every game that's waiting, after
    /// something may have been downloaded.
    pub fn recheck(&mut self, paths: &Paths, global: &GlobalConfig) {
        for entry in &mut self.entries {
            if let Entry::Pending(game) = entry {
                game.recheck(paths, global);
            }
        }
    }

    /// Settles every game that's waiting: ready if its builds are installed by
    /// now, otherwise skipped for lack of them. Settled games move to the end.
    pub fn settle(&mut self, paths: &Paths, global: &GlobalConfig) {
        let mut settled = Vec::new();
        let mut kept = Vec::new();
        for entry in std::mem::take(&mut self.entries) {
            match entry {
                Entry::Pending(game) => settled.push(game),
                entry => kept.push(entry),
            }
        }
        for game in settled {
            if game.plan.check_builds(paths, global).is_ok() {
                kept.push(Entry::Ready(Game::Windows(game.plan)));
            } else {
                kept.push(Entry::Skipped(Skipped {
                    name: game.plan.name().to_owned(),
                    title: game.plan.heroic().title.clone(),
                    reason: SkipReason::Unavailable(game.reason),
                }));
            }
        }
        self.entries = kept;
    }

    /// The games to import, Windows ones first and native ones last.
    pub fn into_ready(self) -> Vec<Game> {
        let (windows, native): (Vec<Game>, Vec<Game>) = self
            .entries
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Ready(game) => Some(game),
                _ => None,
            })
            .partition(|game| matches!(game, Game::Windows(_)));
        windows.into_iter().chain(native).collect()
    }

    /// How many games would be imported.
    pub fn ready_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Ready(_)))
            .count()
    }
}

/// Sorts every game in `listing` into one that's already imported, one that's
/// skipped for a reason, or one that's ready to import, with `runner` for the
/// Windows ones. It only reads: environments that need Heroic's id recorded are
/// queued in the plan as backfills.
pub fn plan_library(
    paths: &Paths,
    global: &GlobalConfig,
    listing: &HeroicListing,
    runner: Option<&str>,
    options: &LibraryOptions,
) -> anyhow::Result<LibraryPlan> {
    plan_candidates(paths, global, listing, runner, options, |_| true)
}

/// Like `plan_library`, but only for the games whose ids are in `selected`.
/// Games outside the selection still count when warning about a shared
/// prefix, since they use it all the same.
pub fn plan_selection(
    paths: &Paths,
    global: &GlobalConfig,
    listing: &HeroicListing,
    selected: &BTreeSet<String>,
    runner: Option<&str>,
    options: &LibraryOptions,
) -> anyhow::Result<LibraryPlan> {
    plan_candidates(paths, global, listing, runner, options, |candidate| {
        selected.contains(&candidate.id)
    })
}

fn plan_candidates(
    paths: &Paths,
    global: &GlobalConfig,
    listing: &HeroicListing,
    runner: Option<&str>,
    options: &LibraryOptions,
    selected: impl Fn(&HeroicCandidate) -> bool,
) -> anyhow::Result<LibraryPlan> {
    // Read when the first game needs inspecting, so a library that's all
    // imported already reads nothing, and then reused for every other game.
    let library = LazyLibrary::new(&listing.source);
    let all_windows: Vec<&HeroicCandidate> = listing
        .candidates
        .iter()
        .filter(|candidate| candidate.kind != HeroicGameKind::Native)
        .collect();
    let (natives, windows): (Vec<&HeroicCandidate>, Vec<&HeroicCandidate>) = listing
        .candidates
        .iter()
        .filter(|candidate| selected(candidate))
        .partition(|candidate| candidate.kind == HeroicGameKind::Native);
    let runner = match runner {
        Some(name) => Some((name, Runners::new(paths, global)?.resolve(name)?)),
        None => None,
    };
    anyhow::ensure!(
        runner.is_some() || windows.is_empty(),
        "a runner is needed to import Windows games"
    );

    let mut existing = existing_environments(paths)?;
    let mut already = Vec::new();
    let mut backfills = Vec::new();
    let mut entries = Vec::new();
    let mut ready_natives: Vec<(String, String)> = Vec::new();
    for candidate in &natives {
        let default = default_name(&candidate.title, candidate.executable.as_deref());
        if let Some(matched) = match_existing(
            &existing,
            &candidate.id,
            None,
            candidate.executable.as_deref(),
            &default,
        ) {
            if matched.needs_id {
                queue_backfill(&mut existing, &mut backfills, &matched, &candidate.id);
            }
            already.push(matched.name);
            continue;
        }
        let taken = |name: &str| {
            existing.iter().any(|environment| environment.name == name)
                || ready_natives.iter().any(|(taken, _)| taken == name)
        };
        let entry = plan_native(paths, global, &library, candidate, taken);
        if let Entry::Ready(game) = &entry {
            ready_natives.push((game.name().to_owned(), candidate.id.clone()));
        }
        entries.push(entry);
    }
    // Stands in for the native games about to be imported, so a Windows game
    // can't take one of their names. The id keeps them from being matched.
    let mut claimed = existing.clone();
    for (name, id) in ready_natives {
        claimed.push(ExistingEnvironment {
            name,
            heroic_id: Some(id),
            snapshot: None,
            prefix: None,
            target_exes: Vec::new(),
        });
    }
    let mut runner_name = None;
    if let Some((name, runner)) = &runner {
        runner_name = Some((*name).to_owned());
        let mut claimed = claimed;
        let shared_by_prefix = shared_prefixes(&all_windows);
        let mut used_names: BTreeSet<String> = BTreeSet::new();
        let mut root_checked = false;
        for candidate in &windows {
            let default = default_name(&candidate.title, candidate.executable.as_deref());
            if let Some(matched) = match_existing(
                &claimed,
                &candidate.id,
                candidate.prefix.as_deref(),
                candidate.executable.as_deref(),
                &default,
            ) {
                if matched.needs_id {
                    queue_backfill(&mut claimed, &mut backfills, &matched, &candidate.id);
                }
                already.push(matched.name);
                continue;
            }
            // A broken prefix root fails the whole import, not one game at a
            // time, so it can't be left to plan_windows, which turns errors
            // into skips. Adopting needs no root, and a library that's all
            // imported already never gets here.
            if !options.adopt && !root_checked {
                global.prefix_root(paths)?;
                root_checked = true;
            }
            entries.push(plan_windows(
                paths,
                global,
                &library,
                candidate,
                &WindowsContext {
                    runner_name: name,
                    runner,
                    options,
                    shared_by_prefix: &shared_by_prefix,
                    taken: &claimed,
                },
                &mut used_names,
            )?);
        }
    }
    Ok(LibraryPlan {
        runner: runner_name,
        entries,
        already,
        backfills,
    })
}

/// The other Heroic games that share each prefix.
fn shared_prefixes(candidates: &[&HeroicCandidate]) -> BTreeMap<PathBuf, Vec<HeroicSharedPrefix>> {
    let mut shared_by_prefix: BTreeMap<PathBuf, Vec<HeroicSharedPrefix>> = BTreeMap::new();
    for candidate in candidates {
        if let Some(prefix) = candidate.prefix.as_deref() {
            shared_by_prefix
                .entry(paths::resolve(prefix))
                .or_default()
                .push(HeroicSharedPrefix {
                    id: candidate.id.clone(),
                    title: candidate.title.clone(),
                });
        }
    }
    for group in shared_by_prefix.values_mut() {
        group.sort_by(|a, b| a.title.cmp(&b.title).then(a.id.cmp(&b.id)));
    }
    shared_by_prefix
}

/// Sorts one native candidate into a game that's ready, or a skip with its
/// reason. `taken` says whether a name is already in use.
fn plan_native(
    paths: &Paths,
    global: &GlobalConfig,
    library: &LazyLibrary,
    candidate: &HeroicCandidate,
    taken: impl Fn(&str) -> bool,
) -> Entry {
    let default = default_name(&candidate.title, candidate.executable.as_deref());
    let skip = |name: String, reason: SkipReason| {
        Entry::Skipped(Skipped {
            name,
            title: candidate.title.clone(),
            reason,
        })
    };
    if let Some(issue) = native_blocker(candidate) {
        return skip(default, SkipReason::Blocked(issue.message.clone()));
    }
    let imported = match library.inspect_native(&candidate.id) {
        Ok(imported) => imported,
        Err(error) => return skip(default, SkipReason::Unreadable(error)),
    };
    if imported
        .issues
        .iter()
        .any(|issue| issue.field.as_deref() == Some("launcherArgs"))
    {
        return skip(default, SkipReason::LauncherArgs);
    }
    if let Some(issue) = imported
        .issues
        .iter()
        .find(|issue| issue.kind == HeroicIssueKind::Invalid)
    {
        return skip(default, SkipReason::Blocked(issue.message.clone()));
    }
    let name = default;
    if taken(&name) {
        return skip(name.clone(), SkipReason::NameTaken(name));
    }
    if let Some(problem) = exe_problem(paths, global, &name, &imported.env, &imported.exe) {
        return skip(name, SkipReason::Blocked(problem));
    }
    Entry::Ready(Game::Native(NativePlan::new(imported, name)))
}

/// Heroic's library and global settings for a bulk plan, each read once for
/// all its games. A failure to read one is kept as the message every game it
/// applies to is skipped with, in the order inspecting a game would hit them:
/// the library, then the game's place in it, then the global settings.
struct LazyLibrary<'a> {
    source: &'a HeroicSource,
    library: OnceCell<Result<Library, String>>,
    global: OnceCell<Result<Global, String>>,
}

impl<'a> LazyLibrary<'a> {
    fn new(source: &'a HeroicSource) -> Self {
        Self {
            source,
            library: OnceCell::new(),
            global: OnceCell::new(),
        }
    }

    fn inputs(&self, id: &str) -> Result<Inputs<'_>, String> {
        let library = self
            .library
            .get_or_init(|| {
                self.source
                    .read_library()
                    .and_then(|library| {
                        library.entries()?;
                        Ok(library)
                    })
                    .map_err(|error| format!("{error:#}"))
            })
            .as_ref()
            .map_err(Clone::clone)?;
        library.entry(id).map_err(|error| format!("{error:#}"))?;
        let global = self
            .global
            .get_or_init(|| {
                self.source
                    .read_global()
                    .map_err(|error| format!("{error:#}"))
            })
            .as_ref()
            .map_err(Clone::clone)?;
        Ok(Inputs { library, global })
    }

    fn inspect(&self, id: &str) -> Result<HeroicImport, String> {
        self.source
            .inspect_in(self.inputs(id)?, id)
            .map_err(|error| format!("{error:#}"))
    }

    fn inspect_native(&self, id: &str) -> Result<HeroicNativeImport, String> {
        self.source
            .inspect_native_in(self.inputs(id)?, id)
            .map_err(|error| format!("{error:#}"))
    }
}

/// What every Windows game in a bulk plan is planned with.
struct WindowsContext<'a> {
    runner_name: &'a str,
    runner: &'a crate::runner::Runner,
    options: &'a LibraryOptions,
    shared_by_prefix: &'a BTreeMap<PathBuf, Vec<HeroicSharedPrefix>>,
    /// The environments, and the native games about to become ones, whose
    /// names are taken.
    taken: &'a [ExistingEnvironment],
}

/// Sorts one Windows candidate into a game that's ready, one waiting on a
/// build, or a skip with its reason.
fn plan_windows(
    paths: &Paths,
    global: &GlobalConfig,
    library: &LazyLibrary,
    candidate: &HeroicCandidate,
    context: &WindowsContext,
    used_names: &mut BTreeSet<String>,
) -> anyhow::Result<Entry> {
    let default = default_name(&candidate.title, candidate.executable.as_deref());
    let skip = |name: String, title: String, reason: SkipReason| {
        Ok(Entry::Skipped(Skipped {
            name,
            title,
            reason,
        }))
    };
    if let Some(issue) = blocker(candidate) {
        return skip(
            default,
            candidate.title.clone(),
            SkipReason::Blocked(issue.message.clone()),
        );
    }
    let imported = match library.inspect(&candidate.id) {
        Ok(imported) => imported,
        Err(error) => {
            return skip(
                default,
                candidate.title.clone(),
                SkipReason::Unreadable(error),
            );
        }
    };
    let shared: Vec<HeroicSharedPrefix> = context
        .shared_by_prefix
        .get(&paths::resolve(&imported.prefix))
        .into_iter()
        .flatten()
        .filter(|other| other.id != candidate.id)
        .cloned()
        .collect();
    let candidate_is_proton = imported.runner.kind == HeroicRunnerKind::Proton;
    if candidate_is_proton != context.runner.is_proton() {
        return skip(
            default,
            imported.title,
            SkipReason::NeedsRunner {
                proton: candidate_is_proton,
            },
        );
    }
    let title = imported.title.clone();
    let name = crate::targets::suggest_name(&imported.title, &imported.exe.to_string_lossy());
    let mut plan = match GamePlan::with_runner(
        paths,
        global,
        imported,
        shared,
        context.runner_name,
        context.runner.clone(),
        Options {
            name: Some(name.clone()),
            adopt: context.options.adopt,
            destination: None,
            arch: context.options.arch,
        },
    ) {
        Ok(plan) => plan,
        Err(error) => {
            return skip(default, title, SkipReason::Unreadable(format!("{error:#}")));
        }
    };
    plan.resolve(Policy::Defaults);
    if let Some(reason) = plan.skip_reason() {
        return skip(default, title, reason);
    }
    if context
        .taken
        .iter()
        .any(|environment| environment.name == name)
        || used_names.contains(&name)
    {
        let reason = SkipReason::NameTaken(name.clone());
        return skip(name, title, reason);
    }
    used_names.insert(name.clone());
    match plan.missing_build(paths, global) {
        Ok(None) => Ok(Entry::Ready(Game::Windows(plan))),
        Ok(Some(missing)) => Ok(Entry::Pending(PendingGame::new(
            plan,
            missing.component,
            missing.version.clone(),
            missing.message(),
        ))),
        Err(error) => skip(name, title, SkipReason::Unavailable(format!("{error:#}"))),
    }
}

#[cfg(test)]
mod tests;
