//! Planning the import of a whole Heroic library.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::components::Component;
use crate::config::{Arch, GlobalConfig};
use crate::environment::{self, Environment, HeroicIdSnapshot};
use crate::heroic::{
    HeroicCandidate, HeroicGameKind, HeroicIssue, HeroicIssueKind, HeroicListing, HeroicRunnerKind,
    HeroicSharedPrefix, HeroicSource,
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
    let source = &listing.source;
    let (natives, windows): (Vec<&HeroicCandidate>, Vec<&HeroicCandidate>) = listing
        .candidates
        .iter()
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
        let entry = plan_native(paths, global, source, candidate, taken);
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
        let shared_by_prefix = shared_prefixes(&windows);
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
                source,
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
    source: &HeroicSource,
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
    let imported = match source.inspect_native(&candidate.id) {
        Ok(imported) => imported,
        Err(error) => return skip(default, SkipReason::Unreadable(format!("{error:#}"))),
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
    source: &HeroicSource,
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
    let imported = match source.inspect(&candidate.id) {
        Ok(imported) => imported,
        Err(error) => {
            return skip(
                default,
                candidate.title.clone(),
                SkipReason::Unreadable(format!("{error:#}")),
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
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;
    use crate::config::Defaults;
    use crate::heroic_import::plan::tests::{fake_proton_runner, fake_wine_runner, paths};

    fn candidate(kind: HeroicGameKind, issues: Vec<HeroicIssue>) -> HeroicCandidate {
        HeroicCandidate {
            id: "game".to_owned(),
            title: "Game".to_owned(),
            kind,
            prefix: None,
            runner: None,
            executable: None,
            issues,
        }
    }

    fn issue(kind: HeroicIssueKind, field: &str, message: &str) -> HeroicIssue {
        HeroicIssue {
            kind,
            field: Some(field.to_owned()),
            message: message.to_owned(),
        }
    }

    #[test]
    fn blocker_allows_a_clean_windows_candidate() {
        assert!(blocker(&candidate(HeroicGameKind::Windows, Vec::new())).is_none());
    }

    #[test]
    fn blocker_allows_invalid_launcher_args() {
        let candidate = candidate(
            HeroicGameKind::Windows,
            vec![issue(HeroicIssueKind::Invalid, "launcherArgs", "bad args")],
        );
        assert!(blocker(&candidate).is_none());
    }

    #[test]
    fn blocker_rejects_non_windows_kinds() {
        let candidate = candidate(
            HeroicGameKind::Browser,
            vec![issue(
                HeroicIssueKind::Unsupported,
                "platform",
                "native games are not Windows prefix imports",
            )],
        );
        let issue = blocker(&candidate).unwrap();
        assert_eq!(issue.message, "native games are not Windows prefix imports");
    }

    #[test]
    fn blocker_rejects_missing_settings() {
        let candidate = candidate(
            HeroicGameKind::Windows,
            vec![issue(HeroicIssueKind::Missing, "exe", "no executable set")],
        );
        assert_eq!(blocker(&candidate).unwrap().message, "no executable set");
    }

    #[test]
    fn blocker_rejects_unsupported_runner() {
        let candidate = candidate(
            HeroicGameKind::Windows,
            vec![issue(
                HeroicIssueKind::Unsupported,
                "runner",
                "only manually added sideloaded games are supported",
            )],
        );
        assert!(blocker(&candidate).is_some());
    }

    #[test]
    fn a_native_candidate_is_blocked_by_its_own_rule() {
        let clean = candidate(HeroicGameKind::Native, Vec::new());
        assert!(blocker(&clean).is_none());
        // Unsupported settings like wrappers are only reported for a native game.
        let wrapped = candidate(
            HeroicGameKind::Native,
            vec![issue(
                HeroicIssueKind::Unsupported,
                "wrapperOptions",
                "wrappers need manual setup",
            )],
        );
        assert!(blocker(&wrapped).is_none());
        let missing = candidate(
            HeroicGameKind::Native,
            vec![issue(
                HeroicIssueKind::Missing,
                "executable",
                "executable not found",
            )],
        );
        assert_eq!(blocker(&missing).unwrap().message, "executable not found");
    }

    fn existing(
        name: &str,
        heroic_id: Option<&str>,
        prefix: &Path,
        target_exes: &[&Path],
    ) -> ExistingEnvironment {
        ExistingEnvironment {
            name: name.to_owned(),
            heroic_id: heroic_id.map(str::to_owned),
            snapshot: None,
            prefix: Some(prefix.to_owned()),
            target_exes: target_exes.iter().map(|exe| exe.to_path_buf()).collect(),
        }
    }

    #[test]
    fn match_existing_matches_by_recorded_id_first() {
        let existing = vec![existing(
            "palworld",
            Some("abc"),
            Path::new("/prefixes/palworld"),
            &[],
        )];
        let matched = match_existing(&existing, "abc", None, None, "something-else").unwrap();
        assert_eq!(matched.name, "palworld");
        assert!(!matched.needs_id);
    }

    #[test]
    fn match_existing_does_not_claim_another_id_through_legacy_fallbacks() {
        let prefix = Path::new("/prefixes/shared");
        let exe = Path::new("/games/shared.exe");
        let existing = vec![
            existing("shared", Some("game-a"), prefix, &[exe]),
            existing("legacy", None, prefix, &[exe]),
        ];
        let matched =
            match_existing(&existing, "game-b", Some(prefix), Some(exe), "shared").unwrap();
        assert_eq!(matched.name, "legacy");
        assert!(matched.needs_id);

        let recorded_only = &existing[..1];
        assert!(
            match_existing(recorded_only, "game-b", Some(prefix), Some(exe), "shared").is_none()
        );
        let matched =
            match_existing(recorded_only, "game-a", Some(prefix), Some(exe), "other").unwrap();
        assert_eq!(matched.name, "shared");
        assert!(!matched.needs_id);
    }

    #[test]
    fn a_stale_backfill_is_rejected_without_changing_the_config() {
        for change in ["config", "id", "removed"] {
            let root = tempdir().unwrap();
            let paths = paths(root.path());
            let path = paths.environment_config("legacy");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let prefix = root.path().join("shared-prefix");
            let original = format!(
                "# keep this\nprefix = {:?}\nrunner = \"wine\"\narch = \"win64\"\n",
                prefix.to_string_lossy()
            );
            fs::write(&path, &original).unwrap();
            let mut existing = existing_environments(&paths).unwrap();
            let matched =
                match_existing(&existing, "candidate", Some(&prefix), None, "other").unwrap();
            assert!(matched.needs_id);
            let mut backfills = Vec::new();
            queue_backfill(&mut existing, &mut backfills, &matched, "candidate");
            assert_eq!(backfills.len(), 1);

            let changed = match change {
                "config" => original.replace("wine", "other"),
                "id" => format!("{original}heroic_id = \"different\"\n"),
                _ => String::new(),
            };
            if change == "removed" {
                fs::remove_file(&path).unwrap();
            } else {
                fs::write(&path, &changed).unwrap();
            }
            let error = backfills[0].record(&paths).unwrap_err();
            assert!(
                error.to_string().contains("retry the import")
                    || error.to_string().contains("does not exist"),
                "{error:#}"
            );
            if change == "removed" {
                assert!(!path.exists());
            } else {
                assert_eq!(fs::read_to_string(&path).unwrap(), changed);
            }
        }
    }

    #[test]
    fn match_existing_falls_back_to_the_adopted_prefix() {
        let existing = vec![existing(
            "zomboid",
            None,
            Path::new("/prefixes/zomboid"),
            &[],
        )];
        let matched = match_existing(
            &existing,
            "different-id",
            Some(Path::new("/prefixes/zomboid")),
            None,
            "not-the-name",
        )
        .unwrap();
        assert_eq!(matched.name, "zomboid");
        assert!(matched.needs_id, "a legacy match should be backfilled");
    }

    #[test]
    fn match_existing_falls_back_to_a_target_running_the_same_exe() {
        let existing = vec![existing(
            "Zomboid",
            None,
            Path::new("/prefixes/other"),
            &[Path::new(
                "/home/me/Games/ProjectZomboid/ProjectZomboid64.exe",
            )],
        )];
        let matched = match_existing(
            &existing,
            "5HYciurSM5YdUGzasx38E7",
            Some(Path::new("/elsewhere")),
            Some(Path::new(
                "/home/me/Games/ProjectZomboid/ProjectZomboid64.exe",
            )),
            "project-zomboid",
        )
        .unwrap();
        assert_eq!(matched.name, "Zomboid");
        assert!(matched.needs_id, "a legacy match should be backfilled");
    }

    #[test]
    fn match_existing_falls_back_to_the_default_name() {
        let existing = vec![existing("hades", None, Path::new("/prefixes/other"), &[])];
        let matched = match_existing(
            &existing,
            "different-id",
            Some(Path::new("/elsewhere")),
            None,
            "hades",
        )
        .unwrap();
        assert_eq!(matched.name, "hades");
        assert!(matched.needs_id);
    }

    #[test]
    fn match_existing_returns_none_when_nothing_matches() {
        let existing = vec![existing("hades", None, Path::new("/prefixes/hades"), &[])];
        assert!(
            match_existing(
                &existing,
                "id",
                Some(Path::new("/elsewhere")),
                Some(Path::new("/elsewhere/Game.exe")),
                "not-hades"
            )
            .is_none()
        );
    }

    #[test]
    fn a_queued_backfill_claims_the_environment_for_later_candidates() {
        let prefix = Path::new("/prefixes/shared");
        let mut existing = vec![ExistingEnvironment {
            snapshot: Some(dummy_snapshot()),
            ..existing("legacy", None, prefix, &[])
        }];
        let mut backfills = Vec::new();
        let first = match_existing(&existing, "one", Some(prefix), None, "x").unwrap();
        queue_backfill(&mut existing, &mut backfills, &first, "one");
        assert!(match_existing(&existing, "two", Some(prefix), None, "x").is_none());
        let again = match_existing(&existing, "one", Some(prefix), None, "x").unwrap();
        assert!(!again.needs_id);
        assert_eq!(backfills.len(), 1);
    }

    /// A snapshot of some environment config, since there's no other way to
    /// make one.
    fn dummy_snapshot() -> HeroicIdSnapshot {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let path = paths.environment_config("dummy");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "prefix = \"/x\"\nrunner = \"wine\"\narch = \"win64\"\n",
        )
        .unwrap();
        Environment::load_for_heroic_match(&paths, "dummy")
            .unwrap()
            .1
    }

    // -- planning a library --

    /// A Heroic library under `root/heroic` and the runners wineo needs.
    struct Library {
        root: PathBuf,
        games: Vec<String>,
    }

    impl Library {
        fn new(root: &Path) -> Self {
            fs::create_dir_all(root.join("heroic/GamesConfig")).unwrap();
            fs::create_dir_all(root.join("heroic/sideload_apps")).unwrap();
            fs::write(
                root.join("heroic/config.json"),
                r#"{"defaultSettings":{"wineVersion":{"type":"wine","name":"System","bin":"wine"}}}"#,
            )
            .unwrap();
            fake_wine_runner(&root.join("runners"), "fake");
            fake_proton_runner(&root.join("runners"), "GE-Proton9-1");
            Self {
                root: root.to_owned(),
                games: Vec::new(),
            }
        }

        fn prefix(&self, name: &str) -> PathBuf {
            let prefix = self.root.join("prefixes").join(name);
            fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
            fs::write(
                prefix.join("system.reg"),
                "WINE REGISTRY Version 2\n#arch=win64\n",
            )
            .unwrap();
            prefix
        }

        /// A Windows sideload with its own prefix and the given extra settings.
        fn windows(&mut self, id: &str, title: &str, prefix: &str, settings: &str) {
            let prefix = self.prefix(prefix);
            let exe = prefix.join(format!("drive_c/Games/{id}.exe"));
            fs::write(&exe, "game").unwrap();
            fs::write(
                self.root.join(format!("heroic/GamesConfig/{id}.json")),
                format!(
                    r#"{{"{id}":{{"winePrefix":"{}"{}{settings}}}}}"#,
                    prefix.display(),
                    if settings.is_empty() { "" } else { "," }
                ),
            )
            .unwrap();
            self.games.push(format!(
                r#"{{"app_name":"{id}","title":"{title}","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}"#,
                exe.display()
            ));
        }

        fn native(&mut self, id: &str, title: &str) {
            let exe = self.root.join(format!("games/{id}"));
            fs::create_dir_all(exe.parent().unwrap()).unwrap();
            fs::write(&exe, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
            self.games.push(format!(
                r#"{{"app_name":"{id}","title":"{title}","runner":"sideload","install":{{"platform":"linux","executable":"{}"}}}}"#,
                exe.display()
            ));
        }

        fn write(&self) -> (Paths, GlobalConfig, HeroicListing) {
            fs::write(
                self.root.join("heroic/sideload_apps/library.json"),
                format!(r#"{{"games":[{}]}}"#, self.games.join(",")),
            )
            .unwrap();
            let paths = paths(&self.root);
            let global = GlobalConfig {
                defaults: Defaults {
                    runner_root: Some(self.root.join("runners").display().to_string()),
                    prefix_root: Some(self.root.join("wineo-prefixes").display().to_string()),
                    ..Defaults::default()
                },
                ..GlobalConfig::default()
            };
            let source = HeroicSource::from_config_dir_with_home(
                self.root.join("heroic"),
                self.root.clone(),
            );
            let listing = listing(&paths, &global, &source).unwrap();
            (paths, global, listing)
        }
    }

    fn names(plan: &LibraryPlan) -> Vec<String> {
        plan.entries()
            .iter()
            .map(|entry| match entry {
                Entry::Ready(game) => format!("ready {}", game.name()),
                Entry::Pending(game) => format!("pending {}", game.plan().name()),
                Entry::Skipped(skipped) => format!("skipped {}", skipped.name),
            })
            .collect()
    }

    fn install_dxvk(paths: &Paths, global: &GlobalConfig) {
        let root = crate::components::Builds::new(paths, global, Component::Dxvk)
            .unwrap()
            .root()
            .to_owned();
        for arch in ["x64", "x32"] {
            let dir = root.join("dxvk-2.7").join(arch);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("dxgi.dll"), b"fixture").unwrap();
        }
    }

    #[test]
    fn a_library_is_sorted_into_ready_pending_skipped_and_imported() {
        let root = tempdir().unwrap();
        let mut library = Library::new(root.path());
        library.windows("a", "Alpha", "p1", "");
        library.windows("b", "Bravo", "p2", r#""autoInstallDxvk":true"#);
        library.windows("c", "Charlie", "p3", r#""launcherArgs":"-a \"unbalanced""#);
        library.windows("d", "Delta", "p1", "");
        library.native("e", "Echo");
        library.windows("f", "Fox", "p4", r#""wrapperOptions":[{"exe":"x"}]"#);
        let (paths, global, listing) = library.write();
        // An environment imported before Heroic ids were recorded.
        let legacy = paths.environment_config("fox");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(
            &legacy,
            "prefix = \"/somewhere\"\nrunner = \"wine\"\narch = \"win64\"\n",
        )
        .unwrap();
        let before = fs::read_to_string(&legacy).unwrap();

        let plan = plan_library(
            &paths,
            &global,
            &listing,
            Some("fake"),
            &LibraryOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.runner(), Some("fake"));
        assert_eq!(
            names(&plan),
            [
                "ready echo",
                "ready alpha",
                "pending bravo",
                "skipped charlie",
                "ready delta",
            ]
        );
        assert_eq!(plan.already(), ["fox"]);
        let Entry::Skipped(charlie) = &plan.entries()[3] else {
            panic!("expected a skip");
        };
        assert_eq!(charlie.reason, SkipReason::LauncherArgs);
        assert_eq!(charlie.title, "Charlie");

        // Planning read the legacy environment but wrote nothing.
        assert_eq!(fs::read_to_string(&legacy).unwrap(), before);
        assert_eq!(plan.backfills().len(), 1);
        assert_eq!(plan.backfills()[0].environment, "fox");
        assert_eq!(plan.backfills()[0].heroic_id, "f");
        plan.record_backfills(&paths).unwrap();
        let after = fs::read_to_string(&legacy).unwrap();
        assert!(after.contains("heroic_id = \"f\""), "{after}");
    }

    #[test]
    fn shared_prefixes_are_warned_about_on_each_game_that_uses_one() {
        let root = tempdir().unwrap();
        let mut library = Library::new(root.path());
        library.windows("a", "Alpha", "p1", "");
        library.windows("d", "Delta", "p1", "");
        let (paths, global, listing) = library.write();
        let plan = plan_library(
            &paths,
            &global,
            &listing,
            Some("fake"),
            &LibraryOptions::default(),
        )
        .unwrap();
        let Entry::Ready(Game::Windows(alpha)) = &plan.entries()[0] else {
            panic!("expected a ready game");
        };
        assert_eq!(
            alpha.warnings(),
            vec![Warning::SharedPrefix(vec![HeroicSharedPrefix {
                id: "d".to_owned(),
                title: "Delta".to_owned()
            }])]
        );
    }

    #[test]
    fn a_proton_game_needs_a_proton_runner_and_the_other_way_around() {
        let root = tempdir().unwrap();
        let mut library = Library::new(root.path());
        library.windows("a", "Alpha", "p1", "");
        let (paths, global, listing) = library.write();
        let plan = plan_library(
            &paths,
            &global,
            &listing,
            Some("GE-Proton9-1"),
            &LibraryOptions::default(),
        );
        // The Wine game can't be imported with a Proton runner, and the
        // Proton prefix layout would be needed besides.
        let plan = plan.unwrap();
        let Entry::Skipped(skipped) = &plan.entries()[0] else {
            panic!("expected a skip");
        };
        assert_eq!(skipped.reason, SkipReason::NeedsRunner { proton: false });
    }

    #[test]
    fn a_native_only_library_needs_no_runner() {
        let root = tempdir().unwrap();
        let mut library = Library::new(root.path());
        library.native("e", "Echo");
        let (paths, global, listing) = library.write();
        let plan =
            plan_library(&paths, &global, &listing, None, &LibraryOptions::default()).unwrap();
        assert_eq!(plan.runner(), None);
        assert_eq!(names(&plan), ["ready echo"]);
        let ready = plan.into_ready();
        assert!(matches!(&ready[0], Game::Native(native) if native.name() == "echo"));

        library.windows("a", "Alpha", "p1", "");
        let (paths, global, listing) = library.write();
        assert!(plan_library(&paths, &global, &listing, None, &LibraryOptions::default()).is_err());
    }

    #[test]
    fn names_taken_by_environments_or_earlier_games_are_skipped() {
        let root = tempdir().unwrap();
        let mut library = Library::new(root.path());
        library.windows("a", "Alpha", "p1", "");
        library.windows("b", "alpha!", "p2", "");
        library.native("e", "Zulu");
        library.windows("z", "Zulu", "p3", "");
        let (paths, global, listing) = library.write();
        let plan = plan_library(
            &paths,
            &global,
            &listing,
            Some("fake"),
            &LibraryOptions::default(),
        )
        .unwrap();
        let skipped: Vec<_> = plan
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                Entry::Skipped(skipped) => Some(skipped),
                _ => None,
            })
            .collect();
        assert_eq!(skipped.len(), 2);
        assert_eq!(skipped[0].reason, SkipReason::NameTaken("alpha".to_owned()));
        assert_eq!(skipped[1].reason, SkipReason::NameTaken("zulu".to_owned()));
    }

    #[test]
    fn pending_builds_are_grouped_rechecked_and_settled() {
        let root = tempdir().unwrap();
        let mut library = Library::new(root.path());
        library.windows("a", "Alpha", "p1", "");
        library.windows("b", "Bravo", "p2", r#""autoInstallDxvk":true"#);
        library.windows("c", "Charlie", "p3", r#""autoInstallDxvk":true"#);
        library.native("e", "Echo");
        let (paths, global, listing) = library.write();
        let mut plan = plan_library(
            &paths,
            &global,
            &listing,
            Some("fake"),
            &LibraryOptions::default(),
        )
        .unwrap();
        assert_eq!(
            plan.pending_builds(),
            vec![BuildOffer {
                component: Component::Dxvk,
                versions: vec![None],
                games: 2
            }]
        );
        // Nothing changed, so rechecking leaves them waiting, with the reason.
        plan.recheck(&paths, &global);
        assert_eq!(plan.pending_builds().len(), 1);

        // Settling without the build skips them, at the end.
        let mut without = plan.clone();
        without.settle(&paths, &global);
        assert_eq!(
            names(&without),
            [
                "ready echo",
                "ready alpha",
                "skipped bravo",
                "skipped charlie"
            ]
        );
        let Entry::Skipped(skipped) = &without.entries()[2] else {
            panic!("expected a skip");
        };
        let SkipReason::Unavailable(reason) = &skipped.reason else {
            panic!("expected an unavailable build");
        };
        assert!(reason.contains("wineo download dxvk"), "{reason}");

        // With the build, the same games are ready, after the ones that never waited.
        install_dxvk(&paths, &global);
        plan.recheck(&paths, &global);
        plan.settle(&paths, &global);
        assert_eq!(
            names(&plan),
            ["ready echo", "ready alpha", "ready bravo", "ready charlie"]
        );
        assert!(plan.pending_builds().is_empty());
        let ready: Vec<String> = plan
            .into_ready()
            .iter()
            .map(|game| game.name().to_owned())
            .collect();
        // Native games are imported last.
        assert_eq!(ready, ["alpha", "bravo", "charlie", "echo"]);
    }

    #[test]
    fn pending_builds_list_each_pinned_version_once() {
        let root = tempdir().unwrap();
        let mut library = Library::new(root.path());
        library.windows("a", "Alpha", "p1", "");
        let (paths, global, listing) = library.write();
        let plan = plan_library(
            &paths,
            &global,
            &listing,
            Some("fake"),
            &LibraryOptions::default(),
        )
        .unwrap();
        let Entry::Ready(Game::Windows(alpha)) = plan.entries()[0].clone() else {
            panic!("expected a ready game");
        };
        let waiting = |version: Option<&str>| {
            Entry::Pending(PendingGame::new(
                alpha.clone(),
                Component::Vkd3d,
                version.map(str::to_owned),
                "missing".to_owned(),
            ))
        };
        let plan = LibraryPlan::from_entries(
            Some("fake".to_owned()),
            Vec::new(),
            vec![
                waiting(Some("v2")),
                waiting(None),
                waiting(Some("v2")),
                waiting(Some("v3")),
            ],
        );
        assert_eq!(
            plan.pending_builds(),
            vec![BuildOffer {
                component: Component::Vkd3d,
                versions: vec![Some("v2".to_owned()), None, Some("v3".to_owned())],
                games: 4
            }]
        );
    }

    #[test]
    fn a_broken_prefix_root_fails_the_plan_instead_of_skipping_each_game() {
        let root = tempdir().unwrap();
        let mut library = Library::new(root.path());
        library.windows("a", "Alpha", "p1", "");
        library.native("e", "Echo");
        let (paths, mut global, listing) = library.write();
        global.defaults.prefix_root = Some(String::new());

        let error = plan_library(
            &paths,
            &global,
            &listing,
            Some("fake"),
            &LibraryOptions::default(),
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("empty path"),
            "unexpected error: {error:#}"
        );

        // Adopting never reads it.
        let plan = plan_library(
            &paths,
            &global,
            &listing,
            Some("fake"),
            &LibraryOptions {
                adopt: true,
                ..LibraryOptions::default()
            },
        )
        .unwrap();
        assert_eq!(plan.ready_count(), 2);
    }

    #[test]
    fn adopting_plans_every_game_in_place() {
        let root = tempdir().unwrap();
        let mut library = Library::new(root.path());
        library.windows("a", "Alpha", "p1", "");
        let (paths, global, listing) = library.write();
        let plan = plan_library(
            &paths,
            &global,
            &listing,
            Some("fake"),
            &LibraryOptions {
                adopt: true,
                ..LibraryOptions::default()
            },
        )
        .unwrap();
        let Entry::Ready(Game::Windows(alpha)) = &plan.entries()[0] else {
            panic!("expected a ready game");
        };
        assert!(alpha.is_adopt());
        assert_eq!(alpha.request().destination, None);
    }

    #[test]
    fn a_listing_marks_a_native_game_whose_exe_cannot_run() {
        let root = tempdir().unwrap();
        let mut library = Library::new(root.path());
        library.native("e", "Echo");
        fs::remove_file(root.path().join("games/e")).unwrap();
        let (_, _, listing) = library.write();
        let issues = &listing.candidates[0].issues;
        assert!(
            issues
                .iter()
                .any(|issue| issue.kind == HeroicIssueKind::Missing
                    && issue.field.as_deref() == Some("executable")
                    && issue.message.starts_with("executable not found: ")),
            "{issues:?}"
        );
        assert!(blocker(&listing.candidates[0]).is_some());
    }

    #[test]
    fn sources_uses_an_explicit_directory_as_given() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let sources = sources(&paths, Some("~/heroic")).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].config_dir(), root.path().join("heroic"));
    }
}
