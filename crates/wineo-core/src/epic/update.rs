//! Checking installed Epic games for updates, updating them, and repairing
//! them.
//!
//! A check compares wineo's record of each game with the owned list, and
//! never runs Legendary itself or reads its files. `plan` reads only. `apply`
//! works as an install does: it holds the game folder's in-use lock for its
//! whole run, so a running game refuses; it marks the record before
//! Legendary starts; it runs Legendary for the game and then for each DLC,
//! each checked afterwards from Legendary's records and what it printed,
//! since its exit code says little; then it refreshes the record and the
//! main target from Legendary's records and clears the mark. An update also
//! installs owned DLC the game doesn't have yet, including one that failed
//! before. Anything that stops it after the mark leaves the game refusing to
//! start until the same command finishes it.
//!
//! Lock order, as in `game`: the folder's in-use lock, then the record lock,
//! then the environment's config lock while the target is refreshed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};

use super::game::{self, GameError, OfflineStart, Record, RecordLock, Work};
use super::install::{self, Event, How, InstallError};
use super::legendary::Legendary;
use super::owned::Owned;
use crate::cancel::Token;
use crate::environment::Environment;
use crate::paths::Paths;

pub use crate::gog::update::{DlcState, UpdateState};

/// How one installed or owned DLC stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlcStatus {
    pub app_name: String,
    /// None when the owned list doesn't name it.
    pub title: Option<String>,
    pub installed: Option<String>,
    pub available: Option<String>,
    pub state: DlcState,
}

/// One Epic game's update status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub environment: String,
    pub app_name: String,
    pub title: String,
    /// The installed version, as wineo recorded it.
    pub installed: Option<String>,
    /// The version Epic offers now, from the owned list.
    pub available: Option<String>,
    pub state: UpdateState,
    /// Whether `wineo update` would apply it.
    pub applicable: bool,
    /// Why the versions can't be compared, as a sentence.
    pub reason: Option<String>,
    /// Work on the game that didn't finish. The game won't start until it
    /// is finished.
    pub unfinished: Option<Work>,
    pub dlc: Vec<DlcStatus>,
}

impl Status {
    /// The command that finishes the work left, when there is some.
    pub fn finish_command(&self) -> Option<String> {
        self.unfinished
            .map(|work| finish_command(&self.environment, &self.app_name, work))
    }
}

/// How unfinished `work` on `app`, which `name` owns, is finished, as the
/// messages that block its start say.
pub fn finish_command(name: &str, app: &str, work: Work) -> String {
    match work {
        Work::Install => format!("wineo import epic --game {app}"),
        Work::Update => format!("wineo update {name}"),
        Work::Repair | Work::Unknown => format!("wineo update {name} --repair"),
    }
}

/// A game that couldn't be checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckError {
    pub environment: String,
    /// None when the config that would name it can't be read.
    pub id: Option<String>,
    pub message: String,
}

/// Every Epic game's status, and the ones that couldn't be checked, each by
/// environment name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Batch {
    pub statuses: Vec<Status>,
    pub errors: Vec<CheckError>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("{name} doesn't track an Epic game.")]
    NotEpic { name: String },
    #[error("{name} names an Epic game, but its config can't be read.")]
    Unloadable { name: String },
    #[error(
        "Epic doesn't list {app} among your games, so {name} can't be updated. See wineo import epic --list."
    )]
    NotOwned { name: String, app: String },
    #[error("Epic has no Windows build of {title} now, so {name} can't be updated.")]
    NoWindowsBuild { name: String, title: String },
    /// A repair of a game whose record is gone, which Legendary has no
    /// record of either, so nothing says where its files are.
    #[error(
        "Legendary has no record of {name}'s Epic game {app} either, so its record can't be rebuilt."
    )]
    NotInLegendary { name: String, app: String },
    #[error("Legendary records {name}'s Epic game in {}, which isn't a folder wineo can use: {why}.", .folder.display())]
    BadFolder {
        name: String,
        folder: PathBuf,
        why: String,
    },
    /// A run Legendary reported as fine that didn't do its work.
    #[error("Legendary didn't {verb} {label}: {detail}.")]
    Check {
        verb: &'static str,
        label: String,
        detail: String,
    },
    #[error("{what} changed since this was planned; run it again.")]
    Changed { what: String },
    /// Stopped by a cancel. `marked` says whether the mark was written, so
    /// the work is there to finish.
    #[error("Canceled.")]
    Canceled { marked: bool },
}

/// Every environment whose config names an Epic game, sorted, with the game
/// it names. A config that names one but can't be read has None. A portable
/// root has none, since Epic games live in the host library.
pub fn environments(paths: &Paths) -> Result<Vec<(String, Option<String>)>> {
    if paths.portable_root().is_some() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for name in crate::environment::names(paths)? {
        match game::epic_id_of(paths, &name) {
            Some(app) => found.push((name, Some(app))),
            None if game::names_epic_id(paths, &name) => found.push((name, None)),
            None => {}
        }
    }
    Ok(found)
}

/// Checks the Epic game environment `name` runs against `owned`, or fails
/// with why there is no owned list to check with.
pub fn check(
    paths: &Paths,
    name: &str,
    owned: std::result::Result<&Owned, &str>,
) -> Result<Status> {
    if paths.portable_root().is_some() {
        return Err(InstallError::Portable(install::HOST_ONLY).into());
    }
    Environment::ensure_exists(paths, name)?;
    let Some(app) = game::epic_id_of(paths, name) else {
        return Err(if game::names_epic_id(paths, name) {
            UpdateError::Unloadable {
                name: name.to_owned(),
            }
        } else {
            UpdateError::NotEpic {
                name: name.to_owned(),
            }
        }
        .into());
    };
    let record = read_record(paths, name, &app)?;
    let owned = owned.map_err(|why| anyhow!("{why}"))?;
    Ok(status_of(name, &record, owned))
}

/// `check` for every Epic game, the ones that can't be checked reported in
/// `errors`.
pub fn check_all(paths: &Paths, owned: std::result::Result<&Owned, &str>) -> Result<Batch> {
    let mut batch = Batch::default();
    for (name, app) in environments(paths)? {
        match check(paths, &name, owned) {
            Ok(status) => batch.statuses.push(status),
            Err(error) => batch.errors.push(CheckError {
                environment: name,
                id: app,
                message: format!("{error:#}"),
            }),
        }
    }
    Ok(batch)
}

fn read_record(paths: &Paths, name: &str, app: &str) -> Result<Record> {
    game::peek(paths, app)
        .map_err(|error| GameError::Unreadable {
            name: name.to_owned(),
            detail: format!("{error:#}"),
        })?
        .ok_or_else(|| {
            GameError::RecordMissing {
                name: name.to_owned(),
                app: app.to_owned(),
            }
            .into()
        })
}

/// Compares `record` with what `owned` lists now. A DLC that isn't
/// installed but is owned and has a Windows build is `not_installed`, and
/// makes an update available, as a DLC that failed before does.
pub fn status_of(name: &str, record: &Record, owned: &Owned) -> Status {
    let app = record.app_name.as_str();
    let listed = owned.games.iter().find(|game| game.app_name == app);
    let mut status = Status {
        environment: name.to_owned(),
        app_name: app.to_owned(),
        title: listed.map_or_else(|| app.to_owned(), |game| game.title.clone()),
        installed: record.installed_version.clone(),
        available: listed.and_then(|game| game.version.clone()),
        state: UpdateState::UpToDate,
        applicable: false,
        reason: None,
        unfinished: record.in_flight.as_ref().map(|mark| mark.work),
        dlc: Vec::new(),
    };
    let Some(listed) = listed else {
        status.state = UpdateState::CantCompare;
        status.reason = Some(format!("Epic doesn't list {app} among your games now."));
        status.dlc = record
            .dlc
            .iter()
            .map(|dlc| DlcStatus {
                app_name: dlc.app_name.clone(),
                title: None,
                installed: dlc.version.clone(),
                available: None,
                state: DlcState::CantCompare,
            })
            .collect();
        return status;
    };
    let mut behind = false;
    for dlc in &record.dlc {
        let offer = listed
            .dlc
            .iter()
            .find(|offer| offer.app_name == dlc.app_name);
        let available = offer.and_then(|offer| offer.version.clone());
        let state = match (&available, &dlc.version) {
            (None, _) | (_, None) => DlcState::CantCompare,
            (Some(available), Some(installed)) if available == installed => DlcState::UpToDate,
            (Some(_), Some(_)) => {
                behind = true;
                DlcState::Available
            }
        };
        status.dlc.push(DlcStatus {
            app_name: dlc.app_name.clone(),
            title: offer.map(|offer| offer.title.clone()),
            installed: dlc.version.clone(),
            available,
            state,
        });
    }
    for offer in &listed.dlc {
        if record.dlc.iter().any(|dlc| dlc.app_name == offer.app_name) {
            continue;
        }
        let state = if offer.version.is_some() {
            behind = true;
            DlcState::NotInstalled
        } else {
            DlcState::Unavailable
        };
        status.dlc.push(DlcStatus {
            app_name: offer.app_name.clone(),
            title: Some(offer.title.clone()),
            installed: None,
            available: offer.version.clone(),
            state,
        });
    }
    match (&status.installed, &status.available) {
        (_, None) => {
            status.state = UpdateState::CantCompare;
            status.reason = Some(format!(
                "Epic has no Windows build of {} now.",
                listed.title
            ));
        }
        (None, _) => {
            status.state = UpdateState::CantCompare;
            status.reason = Some(format!(
                "wineo has no record of which version of {} is installed.",
                listed.title
            ));
        }
        (Some(installed), Some(available)) => {
            if installed != available || behind {
                status.state = UpdateState::Available;
                status.applicable = true;
            }
        }
    }
    status
}

/// One run of Legendary in an update or repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub app_name: String,
    pub title: String,
    pub dlc: bool,
    /// A DLC the update installs rather than updates.
    pub new: bool,
    /// The version before, when it is known.
    pub from: Option<String>,
    /// The version an update goes to, or the version a repair checks.
    pub to: Option<String>,
}

/// An update or repair, reviewed and ready to apply.
#[derive(Debug, Clone)]
pub struct Plan {
    environment: String,
    app_name: String,
    title: String,
    folder: PathBuf,
    work: Work,
    resume: bool,
    /// The record as planned against. None when it is rebuilt.
    seen: Option<Record>,
    /// The record a repair writes when wineo's was missing, rebuilt from
    /// Legendary's records.
    rebuilt: Option<Record>,
    runs: Vec<Run>,
    /// The run a repair that didn't finish carries on from.
    first: usize,
}

impl Plan {
    pub fn environment(&self) -> &str {
        &self.environment
    }

    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// `Work::Update` or `Work::Repair`.
    pub fn work(&self) -> Work {
        self.work
    }

    /// Whether this finishes work that didn't finish.
    pub fn resumes(&self) -> bool {
        self.resume
    }

    /// Whether wineo's record was missing and is rebuilt from Legendary's.
    pub fn rebuilds(&self) -> bool {
        self.rebuilt.is_some()
    }

    /// Every run, the game's first. A repair that carries on skips the ones
    /// before `first`.
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    pub fn first(&self) -> usize {
        self.first
    }
}

/// What planning found.
#[derive(Debug, Clone)]
pub enum Planned {
    UpToDate,
    Ready(Plan),
}

/// Works out an update of `name`'s Epic game, or with `repair` a check of
/// every file of it and its DLC, from wineo's record and `owned`. Leftover
/// work is finished as what it was: an update by `wineo update NAME`, and a
/// repair by either. A repair rebuilds a missing record from Legendary's.
/// Reads only.
pub fn plan(paths: &Paths, name: &str, owned: Option<&Owned>, repair: bool) -> Result<Planned> {
    let Prepared {
        app,
        seen,
        record,
        rebuilt,
        work,
        resume,
    } = prepare(paths, name, repair)?;
    let listed = owned.and_then(|owned| owned.games.iter().find(|game| game.app_name == app));
    let title = listed.map_or_else(|| app.clone(), |game| game.title.clone());
    let dlc_title = |dlc: &str| {
        listed
            .and_then(|game| game.dlc.iter().find(|offer| offer.app_name == dlc))
            .map_or_else(|| dlc.to_owned(), |offer| offer.title.clone())
    };
    let mark = record.in_flight.as_ref();
    let mut runs = Vec::new();
    let mut first = 0;
    if work == Work::Repair {
        runs.push(Run {
            app_name: app.clone(),
            title: title.clone(),
            dlc: false,
            new: false,
            from: record.installed_version.clone(),
            to: record.installed_version.clone(),
        });
        runs.extend(record.dlc.iter().map(|dlc| Run {
            app_name: dlc.app_name.clone(),
            title: dlc_title(&dlc.app_name),
            dlc: true,
            new: false,
            from: dlc.version.clone(),
            to: dlc.version.clone(),
        }));
        if resume && let Some(at) = mark.and_then(|mark| mark.app.as_deref()) {
            first = runs.iter().position(|run| run.app_name == at).unwrap_or(0);
        }
    } else {
        let listed = listed.ok_or_else(|| UpdateError::NotOwned {
            name: name.to_owned(),
            app: app.clone(),
        })?;
        let available = listed
            .version
            .clone()
            .ok_or_else(|| UpdateError::NoWindowsBuild {
                name: name.to_owned(),
                title: title.clone(),
            })?;
        if record.installed_version.as_deref() != Some(available.as_str()) {
            runs.push(Run {
                app_name: app.clone(),
                title: title.clone(),
                dlc: false,
                new: false,
                from: record.installed_version.clone(),
                to: Some(available),
            });
        }
        for offer in &listed.dlc {
            let Some(available) = &offer.version else {
                continue;
            };
            let installed = record.dlc.iter().find(|dlc| dlc.app_name == offer.app_name);
            if installed.is_some_and(|dlc| dlc.version.as_ref() == Some(available)) {
                continue;
            }
            runs.push(Run {
                app_name: offer.app_name.clone(),
                title: offer.title.clone(),
                dlc: true,
                new: installed.is_none(),
                from: installed.and_then(|dlc| dlc.version.clone()),
                to: Some(available.clone()),
            });
        }
        if runs.is_empty() && !resume {
            return Ok(Planned::UpToDate);
        }
    }
    Ok(Planned::Ready(Plan {
        environment: name.to_owned(),
        app_name: app,
        title,
        folder: record.folder.clone(),
        work,
        resume,
        seen,
        rebuilt,
        runs,
        first,
    }))
}

/// What `plan` checks before it needs the owned list: that `name` owns an
/// Epic game wineo has a record of, or with `repair` one Legendary has, and
/// that no install of it is unfinished. Reads only, and never runs
/// Legendary, so a front end can refuse before looking for it.
pub fn precheck(paths: &Paths, name: &str, repair: bool) -> Result<()> {
    prepare(paths, name, repair).map(drop)
}

struct Prepared {
    app: String,
    seen: Option<Record>,
    record: Record,
    rebuilt: Option<Record>,
    work: Work,
    resume: bool,
}

fn prepare(paths: &Paths, name: &str, repair: bool) -> Result<Prepared> {
    if paths.portable_root().is_some() {
        return Err(InstallError::Portable(install::HOST_ONLY).into());
    }
    Environment::ensure_exists(paths, name)?;
    let Some(app) = game::epic_id_of(paths, name) else {
        return Err(if game::names_epic_id(paths, name) {
            UpdateError::Unloadable {
                name: name.to_owned(),
            }
        } else {
            UpdateError::NotEpic {
                name: name.to_owned(),
            }
        }
        .into());
    };
    let seen = game::peek(paths, &app).map_err(|error| GameError::Unreadable {
        name: name.to_owned(),
        detail: format!("{error:#}"),
    })?;
    let (record, rebuilt) = match &seen {
        Some(record) => (record.clone(), None),
        None if repair => {
            let rebuilt = rebuild(paths, name, &app)?;
            (rebuilt.clone(), Some(rebuilt))
        }
        None => {
            return Err(GameError::RecordMissing {
                name: name.to_owned(),
                app,
            }
            .into());
        }
    };
    let unfinished = |work: Work| GameError::Unfinished {
        name: Some(name.to_owned()),
        app: app.clone(),
        folder: record.folder.clone(),
        work,
    };
    let (work, resume) = match record.in_flight.as_ref().map(|mark| mark.work) {
        Some(Work::Install) => return Err(unfinished(Work::Install).into()),
        Some(Work::Update) if repair => return Err(unfinished(Work::Update).into()),
        Some(Work::Update) => (Work::Update, true),
        Some(Work::Repair | Work::Unknown) => (Work::Repair, true),
        None if repair => (Work::Repair, false),
        None => (Work::Update, false),
    };
    Ok(Prepared {
        app,
        seen,
        record,
        rebuilt,
        work,
        resume,
    })
}

/// wineo's record of `app`, rebuilt from Legendary's: where it is, its
/// version, its DLC in the same folder, and how it starts offline.
fn rebuild(paths: &Paths, name: &str, app: &str) -> Result<Record> {
    let installed = game::legendary_installed(paths)?;
    let not_there = || UpdateError::NotInLegendary {
        name: name.to_owned(),
        app: app.to_owned(),
    };
    let entry = installed.get(app).ok_or_else(not_there)?;
    let recorded = entry.install_path.clone().ok_or_else(not_there)?;
    let bad = |why: String| UpdateError::BadFolder {
        name: name.to_owned(),
        folder: recorded.clone(),
        why,
    };
    let folder = fs::canonicalize(&recorded).map_err(|error| bad(error.to_string()))?;
    game::check_folder(&folder).map_err(|error| bad(format!("{error:#}")))?;
    crate::gog::folder::check_overlap(paths, &folder).map_err(|error| bad(format!("{error:#}")))?;
    for (_, other) in game::list(paths)? {
        let Ok(other) = other else { continue };
        if other.app_name != app
            && (other.folder.starts_with(&folder) || folder.starts_with(&other.folder))
        {
            return Err(bad(format!(
                "it overlaps Epic game {}'s folder {}",
                other.app_name,
                other.folder.display()
            ))
            .into());
        }
    }
    let mut record = Record::new(app, folder);
    refresh(paths, &mut record, &[])?;
    Ok(record)
}

/// Copies what Legendary's records say into `record`: the game's version,
/// whether it runs offline, how it starts offline, and its DLC's versions.
/// DLC Legendary has in the same folder that the record lacks are added,
/// except those in `failed`, whose runs didn't check out.
fn refresh(paths: &Paths, record: &mut Record, failed: &[&str]) -> Result<()> {
    let installed = game::legendary_installed(paths)?;
    let entries = install::installed_entries(paths)?;
    let app = record.app_name.clone();
    let here = |dlc: &str| {
        installed.get(dlc).filter(|entry| {
            entry
                .install_path
                .as_deref()
                .is_some_and(|path| install::same_folder(path, &record.folder))
        })
    };
    record.installed_version = installed.get(&app).and_then(|entry| entry.version.clone());
    let entry = entries.get(&app);
    record.can_run_offline = entry.is_some_and(|entry| entry.can_run_offline);
    record.offline = entry.and_then(|entry| entry.offline_start());
    let mut dlc: Vec<game::Dlc> = Vec::new();
    for old in &record.dlc {
        let version =
            here(&old.app_name).map_or_else(|| old.version.clone(), |e| e.version.clone());
        dlc.push(game::Dlc {
            app_name: old.app_name.clone(),
            version,
        });
    }
    for (name, entry) in &installed {
        if *name == app
            || failed.contains(&name.as_str())
            || dlc.iter().any(|known| known.app_name == *name)
            || here(name).is_none()
        {
            continue;
        }
        dlc.push(game::Dlc {
            app_name: name.clone(),
            version: entry.version.clone(),
        });
    }
    record.dlc = dlc;
    Ok(())
}

/// What a run changed, for the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub title: String,
    pub dlc: bool,
    pub from: Option<String>,
    /// What Legendary recorded afterwards.
    pub to: Option<String>,
}

/// What `apply` did.
#[derive(Debug)]
pub struct Updated {
    pub environment: String,
    /// Each run that checked out.
    pub changes: Vec<Change>,
    /// Each DLC that didn't, and anything else to say.
    pub warnings: Vec<String>,
    /// Whether the main target now runs another program.
    pub program_changed: bool,
}

/// Updates or repairs the game as `plan` says. A cancel or a failure of the
/// game's own run after the mark leaves the mark, and planning again
/// finishes the work. A DLC's run that fails is a warning. `stall` is how
/// long an update's download may go without progress before it is said.
pub fn apply(
    legendary: &Legendary,
    plan: &Plan,
    stall: Duration,
    token: &Token,
    mut events: impl FnMut(Event),
) -> Result<Updated> {
    let paths = legendary.paths();
    if paths.portable_root().is_some() {
        return Err(InstallError::Portable(install::HOST_ONLY).into());
    }
    if token.is_canceled() {
        return Err(UpdateError::Canceled { marked: false }.into());
    }
    let app = plan.app_name.as_str();
    let folder = plan.folder.as_path();
    let never = || token.is_canceled();
    let changed = |what: String| anyhow::Error::from(UpdateError::Changed { what });
    let record_changed = || changed(format!("Epic game {app}'s record"));
    let _in_use = game::lock_for(paths, folder, &plan.environment, plan.work)?;
    let old_start = {
        let lock = RecordLock::exclusive(paths, app, &never).map_err(|error| {
            match error.downcast_ref::<GameError>() {
                Some(GameError::Canceled) => UpdateError::Canceled { marked: false }.into(),
                _ => error,
            }
        })?;
        let now = lock.read(paths)?;
        if now != plan.seen {
            return Err(record_changed());
        }
        if game::epic_id_of(paths, &plan.environment).as_deref() != Some(app) {
            return Err(changed(format!("environment {}", plan.environment)));
        }
        let mut record = match (now, &plan.rebuilt) {
            (Some(record), None) => record,
            (None, Some(rebuilt)) => rebuilt.clone(),
            _ => return Err(record_changed()),
        };
        let old_start = record.offline.clone();
        let at = plan.runs.get(plan.first).map_or(app, |run| &run.app_name);
        lock.mark(paths, &mut record, plan.work, at)?;
        old_start
    };

    let repair = plan.work == Work::Repair;
    let total = plan.runs.len();
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut failed: Vec<&str> = Vec::new();
    for (at, run) in plan.runs.iter().enumerate().skip(plan.first) {
        let index = at + 1;
        // An earlier attempt already finished this one.
        if !repair && recorded_at(paths, &run.app_name, folder, run.to.as_deref())? {
            changes.push(change(paths, run)?);
            continue;
        }
        if at > plan.first {
            let lock = RecordLock::exclusive(paths, app, &never)?;
            let mut record = lock.read(paths)?.ok_or_else(record_changed)?;
            lock.mark(paths, &mut record, plan.work, &run.app_name)?;
        }
        events(Event::Started {
            index,
            total,
            title: run.title.clone(),
        });
        let extra: &[&str] = match (repair, run.new) {
            (true, _) => &["--repair"],
            (false, true) => &[],
            // Never a fresh install somewhere else when Legendary has lost
            // the game.
            (false, false) => &["--update-only"],
        };
        let how = How {
            extra,
            stall: (!repair).then_some(stall),
            percent: !repair,
        };
        let result = install::download(
            legendary,
            &run.app_name,
            folder,
            &how,
            (index, total),
            token,
            &mut events,
        )
        .and_then(|tail| check_run(paths, run, folder, &tail, repair));
        match result {
            Ok(()) => changes.push(change(paths, run)?),
            Err(_) if token.is_canceled() => {
                return Err(UpdateError::Canceled { marked: true }.into());
            }
            Err(error) if install::is_cancel(&error) => {
                return Err(UpdateError::Canceled { marked: true }.into());
            }
            Err(error) if run.dlc => {
                let reason = format!("{error:#}");
                let (verb, done) = match (repair, run.new) {
                    (true, _) => ("repair", "The game was repaired without it."),
                    (false, true) => ("install", "The game was updated without it."),
                    (false, false) => ("update", "The game was updated without it."),
                };
                warnings.push(format!("Couldn't {verb} {}: {reason} {done}", run.title));
                events(Event::DlcFailed {
                    title: run.title.clone(),
                    reason,
                });
                failed.push(&run.app_name);
            }
            Err(error) => return Err(error),
        }
    }
    if token.is_canceled() {
        return Err(UpdateError::Canceled { marked: true }.into());
    }

    let lock = RecordLock::exclusive(paths, app, &never)?;
    let mut record = lock.read(paths)?.ok_or_else(record_changed)?;
    if record.folder != folder {
        return Err(record_changed());
    }
    if game::epic_id_of(paths, &plan.environment).as_deref() != Some(app) {
        return Err(changed(format!("environment {}", plan.environment)));
    }
    refresh(paths, &mut record, &failed)?;
    let program_changed = match &record.offline {
        Some(start) => {
            let text = |path: PathBuf| path.to_string_lossy().into_owned();
            let cwd = |start: &OfflineStart| {
                start
                    .working_directory
                    .as_deref()
                    .map(|dir| text(folder.join(dir)))
            };
            crate::targets::refresh_epic_program(
                paths,
                &plan.environment,
                &text(folder.join(&start.executable)),
                cwd(start),
                old_start.as_ref().and_then(cwd),
            )?
        }
        None => {
            warnings.push(format!(
                "Legendary names no program inside {} to start, so {}'s main target wasn't changed.",
                folder.display(),
                plan.environment
            ));
            false
        }
    };
    lock.clear_mark(paths, &mut record)?;
    Ok(Updated {
        environment: plan.environment.clone(),
        changes,
        warnings,
        program_changed,
    })
}

/// What a run that checked out changed, with the version Legendary recorded.
fn change(paths: &Paths, run: &Run) -> Result<Change> {
    let installed = game::legendary_installed(paths)?;
    Ok(Change {
        title: run.title.clone(),
        dlc: run.dlc,
        from: run.from.clone(),
        to: installed
            .get(&run.app_name)
            .and_then(|entry| entry.version.clone())
            .or_else(|| run.to.clone()),
    })
}

/// Whether Legendary has `app` in `folder` at `version` already.
fn recorded_at(paths: &Paths, app: &str, folder: &Path, version: Option<&str>) -> Result<bool> {
    Ok(game::legendary_installed(paths)?
        .get(app)
        .is_some_and(|entry| {
            version.is_some()
                && entry.version.as_deref() == version
                && entry
                    .install_path
                    .as_deref()
                    .is_some_and(|path| install::same_folder(path, folder))
        }))
}

/// What a repair's check of the files logs as errors about the files it
/// found missing or damaged, which it then fetches again. Anything else
/// logged as an error is a failure.
fn found_by_check(line: &str) -> bool {
    [
        "ERROR: File does not match hash: ",
        "ERROR: File is missing: ",
        "ERROR: Other failure (see log), treating file as missing: ",
        "ERROR: Verification failed, ",
    ]
    .iter()
    .any(|found| line.contains(found))
}

/// Checks a run Legendary reported as fine: it has to be in Legendary's
/// records, in this folder, and have logged no error. An update has to have
/// moved off the version it started from. A repair's file check leaves a
/// repair file Legendary removes only once the missing files are fetched.
fn check_run(paths: &Paths, run: &Run, folder: &Path, tail: &[String], repair: bool) -> Result<()> {
    let verb = match (repair, run.new) {
        (true, _) => "repair",
        (false, true) => "install",
        (false, false) => "update",
    };
    let failed = |detail: String| {
        anyhow::Error::from(UpdateError::Check {
            verb,
            label: run.title.clone(),
            detail,
        })
    };
    if let Some(line) = tail
        .iter()
        .find(|line| line.contains("] ERROR:") && !(repair && found_by_check(line)))
    {
        let detail = line
            .split_once("ERROR: ")
            .map_or(line.as_str(), |(_, message)| message);
        return Err(failed(format!(
            "it logged an error ({})",
            crate::error::one_line(detail).trim_end_matches('.')
        )));
    }
    if tail
        .iter()
        .any(|line| line.contains("aborted via KeyboardInterrupt"))
    {
        return Err(failed("it was interrupted".to_owned()));
    }
    let installed =
        game::legendary_installed(paths).map_err(|error| failed(format!("{error:#}")))?;
    let Some(entry) = installed.get(&run.app_name) else {
        return Err(failed(
            "it isn't in Legendary's records afterwards".to_owned(),
        ));
    };
    if !entry
        .install_path
        .as_deref()
        .is_some_and(|path| install::same_folder(path, folder))
    {
        return Err(failed(format!(
            "Legendary's records put it somewhere other than {}",
            folder.display()
        )));
    }
    if repair {
        let left = super::state_dir(paths)
            .join("tmp")
            .join(format!("{}.repair", run.app_name));
        if left.exists() {
            return Err(failed(
                "it stopped before fetching what its check found".to_owned(),
            ));
        }
    } else if run.from.is_some() && run.from != run.to && entry.version == run.from {
        return Err(failed(format!(
            "Legendary's records still show version {}",
            entry.version.as_deref().unwrap_or("unknown")
        )));
    }
    Ok(())
}

/// Whether an update or repair that ended with `error` was canceled.
pub fn is_cancel(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<UpdateError>(),
        Some(UpdateError::Canceled { .. })
    ) || install::is_cancel(error)
}

/// Whether `app`'s record has an update or repair mark, so the work is there
/// to finish.
pub fn left_unfinished(paths: &Paths, app: &str) -> Option<Work> {
    game::peek(paths, app)
        .ok()
        .flatten()
        .and_then(|record| record.in_flight)
        .map(|mark| mark.work)
        .filter(|work| *work != Work::Install)
}

#[cfg(test)]
mod tests;
