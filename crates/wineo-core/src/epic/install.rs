//! Installing an Epic game as a new environment with `epic_id`, and
//! forgetting one that didn't finish.
//!
//! `plan` reads only, apart from what Legendary itself writes when asked
//! about the game: its renewed login and the manifest it caches. `apply`
//! takes the folder's in-use lock for its whole run, writes the game's
//! record with an install mark before Legendary starts, runs Legendary once
//! for the game and once for each owned DLC, checks each run from
//! Legendary's records and what it printed, since its exit code says
//! little, then creates the environment under the record lock and clears
//! the mark. Anything that stops it after the mark leaves the game
//! `unfinished`, and installing it again carries on in the same folder,
//! where Legendary resumes its download.
//!
//! Lock order, as in `game`: the folder's in-use lock, then the record
//! lock. The Epic lock is held from before each run until Legendary starts
//! downloading, so another Epic game can start during a long download.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use super::game::{self, GameError, OfflineStart, Record, RecordLock, Work};
use super::legendary::Legendary;
use super::owned::{self, Image};
use super::{EpicLock, Error};
use crate::cancel::Token;
use crate::config::{Arch, GlobalConfig};
use crate::create::{self, CreateEvent, CreateRequest};
use crate::environment::{Environment, EnvironmentIdentity};
use crate::operation::{Locks, Operation};
use crate::paths::Paths;
use crate::runner::Runner;
use crate::targets::{self, Batch, Spec, Written};

/// The target a new game's environment gets, as a GOG game's does.
pub const TARGET: &str = "game";

/// How long a download may go without progress before it is said.
pub const STALL: Duration = Duration::from_secs(5 * 60);

/// What every Epic command but logging in and listing says in a portable
/// root.
pub const HOST_ONLY: &str = "Epic games live in the host library. Run this without --portable.";

/// Refusals a frontend may want to tell apart.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{0}")]
    Portable(&'static str),
    #[error("{app} isn't one of your Epic games. See wineo import epic --list.")]
    NotOwned { app: String },
    #[error("{title} can't be installed: {why}.")]
    Unavailable { title: String, why: &'static str },
    /// Whole on disk, and no environment owns it.
    #[error(
        "Epic game {app} is already downloaded in {}, and no environment owns it. Restore its environment with wineo restore, or forget it with wineo import epic --forget {app}.",
        .folder.display()
    )]
    Downloaded { app: String, folder: PathBuf },
    /// An install carried on can't move to another folder.
    #[error(
        "{title}'s install didn't finish, so --game finishes it in {}. Leave out --dir.",
        .folder.display()
    )]
    ResumeElsewhere { title: String, folder: PathBuf },
    #[error("{} already exists and isn't empty.", .folder.display())]
    NotEmpty { folder: PathBuf },
    #[error("{} overlaps Epic game {other}'s folder {}.", .folder.display(), .other_folder.display())]
    Overlap {
        folder: PathBuf,
        other: String,
        other_folder: PathBuf,
    },
    #[error("{what} changed since the install was planned; plan it again.")]
    Changed { what: String },
    /// A run Legendary reported as fine that didn't install.
    #[error("Legendary didn't install {label}: {detail}.")]
    Check { label: String, detail: String },
    /// Stopped by a cancel. `marked` says whether the mark was written, so
    /// the install is there to finish.
    #[error("Canceled.")]
    Canceled { marked: bool },
    #[error("wineo has no record of Epic game {app}.")]
    NoRecord { app: String },
    #[error("Epic game {app} belongs to {name}, so it can't be forgotten.")]
    Owned { app: String, name: String },
    #[error(
        "Epic game {app} belongs to {name} in the trash, so it can't be forgotten. Restore it with wineo restore, or remove it for good with wineo prune."
    )]
    Trashed { app: String, name: String },
    #[error("Forgetting Epic game {app} needs Legendary, to drop its own record of it.")]
    NeedsLegendary { app: String },
    /// Another install of the game holds its folder now.
    #[error("Another install of {title} is running. Try again once it's done.")]
    Running { title: String },
}

/// What to install, and how.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub app_name: String,
    /// Where the game goes instead of `<games_root>/<title>`.
    pub dir: Option<PathBuf>,
    /// The runner and arch a new environment gets. None uses the global
    /// defaults, like `new`.
    pub runner: Option<String>,
    pub arch: Option<Arch>,
    /// Whether the game's target gets a desktop entry.
    pub desktop: bool,
}

/// What wineo already has of a game.
#[derive(Debug, Clone)]
pub enum Existing {
    None,
    /// Installed, and `name` owns it.
    Installed {
        name: String,
        folder: PathBuf,
    },
    /// An install that didn't finish, which installing again finishes. The
    /// owner is there when the environment was created before it stopped.
    Unfinished {
        record: Record,
        owner: Option<String>,
    },
    /// Whole, with no environment owning it.
    Downloaded {
        record: Record,
    },
}

/// What wineo has of `app`, from its record and which environment names it.
/// An update or repair that didn't finish is refused as a launch is.
pub fn existing(paths: &Paths, app: &str) -> Result<Existing> {
    let record = game::peek(paths, app)?;
    let owner = game::owners(paths, app)?.into_iter().next();
    Ok(match (record, owner) {
        (None, None) => Existing::None,
        (None, Some(name)) => {
            return Err(GameError::RecordMissing {
                name,
                app: app.to_owned(),
            }
            .into());
        }
        (Some(record), owner) if record.in_flight.is_some() => {
            let work = record.in_flight.as_ref().map(|mark| mark.work);
            if work == Some(Work::Install) {
                Existing::Unfinished { record, owner }
            } else {
                return Err(GameError::Unfinished {
                    name: owner,
                    app: app.to_owned(),
                    folder: record.folder,
                    work: work.unwrap_or_default(),
                }
                .into());
            }
        }
        (Some(record), Some(name)) => Existing::Installed {
            name,
            folder: record.folder,
        },
        (Some(record), None) => Existing::Downloaded { record },
    })
}

/// A new environment, resolved while planning.
#[derive(Debug, Clone)]
pub struct Creation {
    runner: Runner,
    /// The arch asked for.
    arch: Option<Arch>,
    resolved_arch: Arch,
    adopted: bool,
}

impl Creation {
    pub fn runner(&self) -> &Runner {
        &self.runner
    }

    pub fn resolved_arch(&self) -> Arch {
        self.resolved_arch
    }
}

/// One DLC the install adds, from the owned list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedDlc {
    pub app_name: String,
    pub title: String,
}

/// An install, reviewed and ready to apply.
#[derive(Debug, Clone)]
pub struct Plan {
    app_name: String,
    title: String,
    environment: String,
    folder: PathBuf,
    resume: bool,
    has_environment: bool,
    version: Option<String>,
    download_size: Option<u64>,
    install_size: Option<u64>,
    dlc: Vec<PlannedDlc>,
    warnings: Vec<String>,
    creation: Option<Creation>,
    images: Vec<Image>,
    desktop: bool,
}

impl Plan {
    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The environment it installs as, or that owns it already.
    pub fn environment(&self) -> &str {
        &self.environment
    }

    /// Absolute and in normal form.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Whether this carries on an install that didn't finish.
    pub fn resumes(&self) -> bool {
        self.resume
    }

    /// The game's version as Epic lists it now.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// The game's own download and installed size, without DLC, which
    /// Legendary can't say ahead.
    pub fn download_size(&self) -> Option<u64> {
        self.download_size
    }

    pub fn install_size(&self) -> Option<u64> {
        self.install_size
    }

    /// The owned DLC, each installed in a run of its own.
    pub fn dlc(&self) -> &[PlannedDlc] {
        &self.dlc
    }

    /// What the review warns about: anti-cheat, an ownership token, an
    /// account at another launcher, prerequisites wineo doesn't run.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// How the environment is created. None when it exists already.
    pub fn creation(&self) -> Option<&Creation> {
        self.creation.as_ref()
    }

    /// Epic's art for the game, for after the install.
    pub fn images(&self) -> &[Image] {
        &self.images
    }

    /// How many runs of Legendary the install takes, the game's included.
    pub fn runs(&self) -> usize {
        1 + self.dlc.len()
    }
}

/// Why a game that needs another launcher's account can't be installed:
/// wineo doesn't set that account up.
const NEEDS_LINKING: &str = "it needs an account at another launcher";

/// Why the owned list says a game can't be installed, if it does.
pub fn unavailable(game: &owned::Game) -> Option<&'static str> {
    if game.third_party.is_some() {
        Some("another launcher manages it")
    } else if game.version.is_none() {
        Some("Epic has no Windows build of it")
    } else if game.partner_link.is_some() {
        Some(NEEDS_LINKING)
    } else {
        None
    }
}

/// What `info --json` says that the review shows.
#[derive(Debug, Default)]
struct Info {
    version: Option<String>,
    download_size: Option<u64>,
    install_size: Option<u64>,
    external: Option<String>,
    prerequisite: Option<Prerequisite>,
    launch_exe: Option<String>,
}

#[derive(Debug)]
struct Prerequisite {
    ids: Vec<String>,
    name: Option<String>,
    path: Option<String>,
}

/// Asks Legendary about the game, under the Epic lock, since it logs in.
fn info(legendary: &Legendary, app: &str, token: &Token) -> Result<Info, Error> {
    const COMMAND: &str = "info --json";
    let output = {
        let _lock = EpicLock::acquire(legendary.paths())?;
        legendary.run(&["info", app, "--json"], None, token)?
    };
    let problem = |problem: &str| Error::Output {
        command: COMMAND.to_owned(),
        problem: problem.to_owned(),
    };
    let value: Value =
        serde_json::from_str(&output.stdout).map_err(|_| problem("it isn't JSON"))?;
    let game = value
        .get("game")
        .and_then(Value::as_object)
        .ok_or_else(|| problem("`game` is missing or isn't an object"))?;
    let manifest = value.get("manifest").and_then(Value::as_object);
    let text = |object: Option<&serde_json::Map<String, Value>>, key: &str| {
        object
            .and_then(|object| object.get(key))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let size = |key: &str| {
        manifest
            .and_then(|manifest| manifest.get(key))
            .and_then(Value::as_u64)
    };
    let prerequisite = manifest
        .and_then(|manifest| manifest.get("prerequisites"))
        .and_then(Value::as_object)
        .map(|prerequisite| Prerequisite {
            ids: prerequisite
                .get("ids")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            name: text(Some(prerequisite), "name"),
            path: text(Some(prerequisite), "path"),
        })
        .filter(|prerequisite| prerequisite.path.is_some() || !prerequisite.ids.is_empty());
    Ok(Info {
        version: text(manifest, "build_version").or_else(|| text(Some(game), "version")),
        download_size: size("download_size"),
        install_size: size("disk_size"),
        external: text(Some(game), "external_activation"),
        prerequisite,
        launch_exe: text(manifest, "launch_exe"),
    })
}

/// What the review warns about, from the owned list and `info`.
fn warnings(game: &owned::Game, info: &Info) -> Vec<String> {
    let title = &game.title;
    let mut warnings = Vec::new();
    let anti_cheat = info.prerequisite.as_ref().is_some_and(|prerequisite| {
        prerequisite
            .ids
            .iter()
            .chain(prerequisite.path.iter())
            .any(|text| {
                text.to_ascii_lowercase().contains("easyanticheat") || text == "EAC_Service"
            })
    }) || info
        .launch_exe
        .as_deref()
        .is_some_and(|exe| exe.to_ascii_lowercase().contains("easyanticheat"));
    if anti_cheat {
        warnings.push(format!(
            "{title} uses Easy Anti-Cheat, which may keep it from running under Wine or Proton."
        ));
    }
    if game.ownership_token {
        warnings.push(format!(
            "{title} checks an Epic ownership token, which usually means Denuvo."
        ));
    }
    if let Some(prerequisite) = &info.prerequisite {
        let what = prerequisite
            .path
            .clone()
            .or_else(|| prerequisite.name.clone())
            .unwrap_or_else(|| prerequisite.ids.join(", "));
        warnings.push(format!(
            "{title} ships a prerequisite installer that wineo doesn't run: {what}."
        ));
    }
    if !game.can_run_offline {
        warnings.push(format!(
            "{title} isn't marked to run offline, so it starts only while Epic answers."
        ));
    }
    warnings
}

/// Works out the install of `game` and asks Legendary about it. `existing`
/// is what `existing` found, which the caller has already answered when it
/// is an installed game.
pub fn plan(
    legendary: &Legendary,
    request: &Request,
    game: &owned::Game,
    existing: &Existing,
    token: &Token,
) -> Result<Plan> {
    let paths = legendary.paths();
    if paths.portable_root().is_some() {
        return Err(InstallError::Portable(HOST_ONLY).into());
    }
    if let Some(why) = unavailable(game) {
        return Err(InstallError::Unavailable {
            title: game.title.clone(),
            why,
        }
        .into());
    }
    let global = GlobalConfig::load(paths)?;
    let (resume, owner, folder) = match existing {
        Existing::None => {
            let folder = crate::gog::install_dir::install_folder(
                &global.games_root(paths)?,
                &game.title,
                request.dir.as_deref(),
            )?;
            (
                false,
                None,
                check_new_folder(paths, &folder, &game.app_name)?,
            )
        }
        Existing::Unfinished { record, owner } => {
            if let Some(dir) = &request.dir
                && crate::gog::folder::canonical_new(dir)? != record.folder
            {
                return Err(InstallError::ResumeElsewhere {
                    title: game.title.clone(),
                    folder: record.folder.clone(),
                }
                .into());
            }
            (true, owner.clone(), record.folder.clone())
        }
        Existing::Downloaded { record } => {
            return Err(InstallError::Downloaded {
                app: game.app_name.clone(),
                folder: record.folder.clone(),
            }
            .into());
        }
        Existing::Installed { .. } => bail!("{} is installed already", game.app_name),
    };
    let has_environment = owner.is_some();
    let environment = match owner {
        Some(owner) => owner,
        None => crate::name::from_title(&game.title),
    };
    let creation = if has_environment {
        None
    } else {
        Some(creation(paths, request, &environment)?)
    };
    let info = info(legendary, &game.app_name, token)?;
    // The same case as a partner link, which only `info` may name.
    if info.external.is_some() {
        return Err(InstallError::Unavailable {
            title: game.title.clone(),
            why: NEEDS_LINKING,
        }
        .into());
    }
    Ok(Plan {
        app_name: game.app_name.clone(),
        title: game.title.clone(),
        environment,
        folder,
        resume,
        has_environment,
        version: info.version.clone().or_else(|| game.version.clone()),
        download_size: info.download_size,
        install_size: info.install_size,
        dlc: game
            .dlc
            .iter()
            .filter(|dlc| dlc.version.is_some())
            .map(|dlc| PlannedDlc {
                app_name: dlc.app_name.clone(),
                title: dlc.title.clone(),
            })
            .collect(),
        warnings: warnings(game, &info),
        creation,
        images: game.images.clone(),
        desktop: request.desktop,
    })
}

/// How the environment will be created, checked like `new` checks it.
fn creation(paths: &Paths, request: &Request, name: &str) -> Result<Creation> {
    let plan = create::plan(
        paths,
        CreateRequest {
            name: name.to_owned(),
            runner: request.runner.clone(),
            arch: request.arch,
            epic_id: Some(request.app_name.clone()),
            ..CreateRequest::default()
        },
    )?;
    create::check_arch(plan.runner(), request.arch)?;
    Ok(Creation {
        runner: plan.runner().clone(),
        arch: request.arch,
        resolved_arch: plan.arch(),
        adopted: plan.adopted(),
    })
}

/// Checks a new install's folder and returns it in canonical form: it may
/// not overlap another game's folder, Epic's or GOG's, and has to be new or
/// empty. `app`'s own record is passed over: one written since planning
/// began is another install of the same game, which the record lock refuses
/// with a clearer message.
fn check_new_folder(paths: &Paths, folder: &Path, app: &str) -> Result<PathBuf> {
    let folder = crate::gog::folder::canonical_new(folder)?;
    crate::gog::folder::check_overlap(paths, &folder)?;
    for (_, record) in game::list(paths)? {
        let Ok(record) = record else { continue };
        if record.app_name == app {
            continue;
        }
        if record.folder.starts_with(&folder) || folder.starts_with(&record.folder) {
            return Err(InstallError::Overlap {
                folder,
                other: record.app_name,
                other_folder: record.folder,
            }
            .into());
        }
    }
    let empty = match fs::read_dir(&folder) {
        Ok(mut entries) => entries.next().is_none(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        // A file, or a folder wineo can't list, isn't one to install into.
        Err(_) => false,
    };
    if !empty {
        return Err(InstallError::NotEmpty { folder }.into());
    }
    Ok(folder)
}

/// What an install is doing, for a frontend.
#[derive(Debug)]
pub enum Event {
    /// Run `index`, from 1, of `total` began: the game is 1.
    Started {
        index: usize,
        total: usize,
        title: String,
    },
    /// Legendary's own count of what it has done in this run.
    Progress {
        index: usize,
        total: usize,
        percent: f64,
    },
    /// Legendary printed a line that says nothing wineo reads.
    Working {
        index: usize,
        total: usize,
    },
    /// No progress for `STALL` or more. The download carries on.
    Stalled {
        index: usize,
        total: usize,
        minutes: u64,
    },
    /// A DLC that didn't install. The game installs without it.
    DlcFailed {
        title: String,
        reason: String,
    },
    Create(CreateEvent),
}

/// What `apply` did.
#[derive(Debug)]
pub struct Installed {
    pub environment: String,
    pub folder: PathBuf,
    pub title: String,
    pub resumed: bool,
    /// Whether this install created the environment.
    pub created: bool,
    /// The DLC installed, by title.
    pub dlc: Vec<String>,
    /// Each DLC that didn't install, and why.
    pub warnings: Vec<String>,
    /// The new environment's identity, for its art step to carry.
    pub identity: Option<EnvironmentIdentity>,
}

/// One run of Legendary.
struct Run<'a> {
    app: &'a str,
    title: &'a str,
    dlc: bool,
}

/// Installs the game as `plan` says. A cancel or a failure after the mark
/// leaves it, and the same request finishes the install. `stall` is how
/// long a download may go without progress before it is said, `STALL`
/// outside tests.
pub fn apply(
    legendary: &Legendary,
    plan: &Plan,
    stall: Duration,
    token: &Token,
    mut events: impl FnMut(Event),
) -> Result<Installed> {
    let paths = legendary.paths();
    if paths.portable_root().is_some() {
        return Err(InstallError::Portable(HOST_ONLY).into());
    }
    let canceled = |marked: bool| anyhow::Error::from(InstallError::Canceled { marked });
    if token.is_canceled() {
        return Err(canceled(false));
    }
    let app = plan.app_name.as_str();
    let folder = plan.folder.as_path();
    let never = || token.is_canceled();
    let _in_use = game::lock_for(paths, folder, &plan.environment, Work::Install)
        .map_err(|error| another_install(paths, plan, error))?;
    {
        let lock = RecordLock::exclusive(paths, app, &never).map_err(|error| {
            match error.downcast_ref::<GameError>() {
                Some(GameError::Canceled) => canceled(false),
                _ => error,
            }
        })?;
        let mut record = match lock.read(paths)? {
            None if !plan.resume => {
                check_new_folder(paths, folder, app)?;
                Record::new(app, folder.to_owned())
            }
            Some(record)
                if plan.resume
                    && record.folder == folder
                    && record
                        .in_flight
                        .as_ref()
                        .is_some_and(|mark| mark.work == Work::Install) =>
            {
                record
            }
            _ => {
                return Err(InstallError::Changed {
                    what: format!("Epic game {app}'s record"),
                }
                .into());
            }
        };
        check_owners(paths, plan)?;
        // Everything creating the environment checks, so a name that can't
        // be used fails now rather than after the download.
        if let Some(creation) = &plan.creation {
            check_creation(paths, plan, creation)?;
        }
        gog_folder_create(folder)?;
        if fs::canonicalize(folder).ok().as_deref() != Some(folder) {
            return Err(InstallError::Changed {
                what: folder.display().to_string(),
            }
            .into());
        }
        lock.mark(paths, &mut record, Work::Install, app)?;
    }

    let runs: Vec<Run> = std::iter::once(Run {
        app,
        title: &plan.title,
        dlc: false,
    })
    .chain(plan.dlc.iter().map(|dlc| Run {
        app: &dlc.app_name,
        title: &dlc.title,
        dlc: true,
    }))
    .collect();
    let total = runs.len();
    let mut warnings = Vec::new();
    let mut failed = Vec::new();
    for (at, run) in runs.iter().enumerate() {
        let index = at + 1;
        // An earlier attempt already finished this one.
        if installed_at(paths, run.app, folder)? {
            continue;
        }
        if run.dlc {
            let lock = RecordLock::exclusive(paths, app, &never)?;
            let mut record = lock.read(paths)?.ok_or_else(|| InstallError::Changed {
                what: format!("Epic game {app}'s record"),
            })?;
            lock.mark(paths, &mut record, Work::Install, run.app)?;
        }
        events(Event::Started {
            index,
            total,
            title: run.title.to_owned(),
        });
        let result = download(
            legendary,
            run.app,
            folder,
            index,
            total,
            stall,
            token,
            &mut events,
        );
        let result = result.and_then(|tail| check(paths, run, folder, &tail));
        match result {
            Ok(()) => {}
            Err(_) if token.is_canceled() => return Err(canceled(true)),
            Err(error) if run.dlc => {
                let reason = format!("{error:#}");
                warnings.push(format!(
                    "Couldn't install {}: {reason} The game installed without it.",
                    run.title
                ));
                events(Event::DlcFailed {
                    title: run.title.to_owned(),
                    reason,
                });
                failed.push(run.app);
            }
            Err(error) => return Err(error),
        }
    }
    if token.is_canceled() {
        return Err(canceled(true));
    }

    // Created under the record lock, after checking again that no
    // environment took the game, so a restore can't claim it meanwhile.
    let lock = RecordLock::exclusive(paths, app, &never)?;
    let mut record = lock.read(paths)?.ok_or_else(|| InstallError::Changed {
        what: format!("Epic game {app}'s record"),
    })?;
    if record.folder != folder {
        return Err(InstallError::Changed {
            what: format!("Epic game {app}'s record"),
        }
        .into());
    }
    check_owners(paths, plan)?;
    let installed = game::legendary_installed(paths)?;
    let entries = installed_entries(paths)?;
    let mut created = false;
    let mut identity = None;
    if let Some(creation) = &plan.creation {
        let made = create::run(paths, creation_request(plan, creation), token, |event| {
            events(Event::Create(event))
        })?;
        identity = made.identity;
        created = true;
    }
    let entry = entries.get(app);
    match entry.and_then(|entry| entry.offline_start()) {
        Some(start) => add_target(paths, plan, folder, &start)?,
        None => warnings.push(format!(
            "Legendary names no program inside {} to start, so {} has none yet. Pick one with: wineo setup {}",
            folder.display(),
            plan.environment,
            plan.environment
        )),
    }
    record.installed_version = installed.get(app).and_then(|entry| entry.version.clone());
    record.can_run_offline = entry.is_some_and(|entry| entry.can_run_offline);
    record.offline = entry.and_then(|entry| entry.offline_start());
    let mut dlc = Vec::new();
    record.dlc = plan
        .dlc
        .iter()
        .filter(|planned| !failed.contains(&planned.app_name.as_str()))
        .filter(|planned| {
            installed.get(&planned.app_name).is_some_and(|entry| {
                entry
                    .install_path
                    .as_deref()
                    .is_some_and(|path| same_folder(path, folder))
            })
        })
        .map(|planned| {
            dlc.push(planned.title.clone());
            game::Dlc {
                app_name: planned.app_name.clone(),
                version: installed
                    .get(&planned.app_name)
                    .and_then(|entry| entry.version.clone()),
            }
        })
        .collect();
    lock.clear_mark(paths, &mut record)?;
    Ok(Installed {
        environment: plan.environment.clone(),
        folder: folder.to_owned(),
        title: plan.title.clone(),
        resumed: plan.resume,
        created,
        dlc,
        warnings,
        identity,
    })
}

/// Creates the folder and its missing parents, as GOG's install does, so
/// its canonical form is fixed before the mark names it.
fn gog_folder_create(folder: &Path) -> Result<()> {
    crate::gog::folder::create_dir_durable(folder)
}

/// Plans the environment's creation again and takes its operation lock for
/// a moment, which is where interrupted removals and snapshot restores of
/// the name are refused.
fn check_creation(paths: &Paths, plan: &Plan, creation: &Creation) -> Result<()> {
    let planned = create::plan(paths, creation_request(plan, creation))?;
    if planned.adopted() != creation.adopted {
        return Err(InstallError::Changed {
            what: format!("{}'s prefix", plan.environment),
        }
        .into());
    }
    drop(Locks::new(paths).acquire(Operation::Create, &plan.environment, &[planned.prefix()])?);
    Ok(())
}

fn creation_request(plan: &Plan, creation: &Creation) -> CreateRequest {
    CreateRequest {
        name: plan.environment.clone(),
        resolved_runner: Some(creation.runner.clone()),
        arch: creation.arch,
        expected_adopted: Some(creation.adopted),
        epic_id: Some(plan.app_name.clone()),
        ..CreateRequest::default()
    }
}

/// Refuses an install another environment has taken the game from: only
/// the plan's own environment may own it, and only when it already did.
fn check_owners(paths: &Paths, plan: &Plan) -> Result<()> {
    let owners = game::owners(paths, &plan.app_name)?;
    let allowed = |owner: &String| plan.has_environment && *owner == plan.environment;
    if let Some(other) = owners.into_iter().find(|owner| !allowed(owner)) {
        return Err(GameError::Taken {
            name: plan.environment.clone(),
            app: plan.app_name.clone(),
            other,
        }
        .into());
    }
    if !plan.has_environment && paths.environment_config(&plan.environment).exists() {
        return Err(InstallError::Changed {
            what: format!("environment {}", plan.environment),
        }
        .into());
    }
    Ok(())
}

/// One Legendary run of `app` into `folder`, reporting its progress. Holds
/// the Epic lock until Legendary starts downloading, by which time its
/// login is done. Returns the end of what it printed.
#[allow(clippy::too_many_arguments)]
fn download(
    legendary: &Legendary,
    app: &str,
    folder: &Path,
    index: usize,
    total: usize,
    stall: Duration,
    token: &Token,
    events: &mut impl FnMut(Event),
) -> Result<Vec<String>> {
    let base = folder
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent", folder.display()))?;
    let name = folder
        .file_name()
        .ok_or_else(|| anyhow!("{} has no name", folder.display()))?;
    let (Some(base), Some(name)) = (base.to_str(), name.to_str()) else {
        bail!(
            "Legendary can't be given {}, which isn't UTF-8",
            folder.display()
        );
    };
    let mut lock = Some(EpicLock::acquire(legendary.paths())?);
    let last = Cell::new(Instant::now());
    let said_stall = Cell::new(false);
    let events = RefCell::new(events);
    let streamed = legendary.stream(
        &[
            "-y",
            "install",
            app,
            "--base-path",
            base,
            "--game-folder",
            name,
            "--skip-dlcs",
            "--disable-sdl",
        ],
        token,
        |line| {
            if started_downloading(line) {
                lock.take();
            }
            let event = match progress(line) {
                Some(percent) => {
                    last.set(Instant::now());
                    said_stall.set(false);
                    Event::Progress {
                        index,
                        total,
                        percent,
                    }
                }
                None => Event::Working { index, total },
            };
            (events.borrow_mut())(event);
        },
        || {
            let quiet = last.get().elapsed();
            if !said_stall.get() && quiet >= stall {
                said_stall.set(true);
                (events.borrow_mut())(Event::Stalled {
                    index,
                    total,
                    minutes: quiet.as_secs() / 60,
                });
            }
        },
    );
    match streamed {
        Ok(streamed) => Ok(streamed.tail),
        Err(error) if error.is_canceled() => Err(InstallError::Canceled { marked: true }.into()),
        Err(error) => Err(error.into()),
    }
}

/// Whether Legendary is past its login and downloading: its download
/// manager has started.
fn started_downloading(line: &str) -> bool {
    line.contains("[DLManager]")
}

/// The percentage in Legendary's progress line: `[DLManager] INFO: =
/// Progress: 6.38% (4506/70640), Running for 00:00:30, ETA: 00:07:20`.
pub fn progress(line: &str) -> Option<f64> {
    let (_, rest) = line.split_once("= Progress: ")?;
    let (percent, _) = rest.split_once('%')?;
    let percent: f64 = percent.trim().parse().ok()?;
    (0.0..=100.0).contains(&percent).then_some(percent)
}

/// Checks a run Legendary reported as fine: it has to be in Legendary's
/// records, in this folder, and have logged no error, since a failed write
/// is only logged.
fn check(paths: &Paths, run: &Run, folder: &Path, tail: &[String]) -> Result<()> {
    let failed = |detail: String| {
        anyhow::Error::from(InstallError::Check {
            label: run.title.to_owned(),
            detail,
        })
    };
    if let Some(line) = tail.iter().find(|line| line.contains("] ERROR:")) {
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
    match installed.get(run.app) {
        None => Err(failed(
            "it isn't in Legendary's records afterwards".to_owned(),
        )),
        Some(entry)
            if !entry
                .install_path
                .as_deref()
                .is_some_and(|path| same_folder(path, folder)) =>
        {
            Err(failed(format!(
                "Legendary's records put it somewhere other than {}",
                folder.display()
            )))
        }
        Some(_) => Ok(()),
    }
}

/// Whether Legendary already has `app` installed in `folder`.
fn installed_at(paths: &Paths, app: &str, folder: &Path) -> Result<bool> {
    Ok(game::legendary_installed(paths)?
        .get(app)
        .is_some_and(|entry| {
            entry
                .install_path
                .as_deref()
                .is_some_and(|path| same_folder(path, folder))
        }))
}

pub(super) fn same_folder(path: &Path, folder: &Path) -> bool {
    path == folder || fs::canonicalize(path).is_ok_and(|path| path == folder)
}

/// An entry of Legendary's `installed.json`: the same objects `list-installed
/// --json` prints, read from the file instead of by running Legendary.
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct Entry {
    #[serde(default)]
    executable: Option<String>,
    #[serde(default)]
    launch_parameters: Option<String>,
    #[serde(default)]
    can_run_offline: bool,
}

impl Entry {
    /// What starting the game without Legendary runs, as Legendary 0.21.1
    /// would (`get_launch_parameters` in its `core.py`): the program
    /// relative to the folder, its own parameters split Legendary's way, and
    /// the program's folder as the working folder. Epic's metadata can add
    /// parameters of its own, which only Legendary's answer carries.
    /// None when it names no program, or one outside the folder.
    fn offline_start(&self) -> Option<OfflineStart> {
        let executable = inside(self.executable.as_deref()?)?;
        let arguments = self
            .launch_parameters
            .as_deref()
            .and_then(super::launch::split_parameters)
            .unwrap_or_default();
        let working_directory = Path::new(&executable)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.to_string_lossy().into_owned());
        Some(OfflineStart {
            executable,
            arguments,
            working_directory,
        })
    }
}

/// A path Legendary gives relative to the game folder, in normal form with
/// `/` between its parts. Legendary drops leading slashes and takes `\\`
/// for `/`, as this does. None for one with nothing in it or that leads out
/// of the folder.
pub(super) fn inside(raw: &str) -> Option<String> {
    let normal = raw.replace('\\', "/");
    let mut parts = Vec::new();
    for part in Path::new(normal.trim_start_matches('/')).components() {
        match part {
            std::path::Component::Normal(part) => parts.push(part.to_str()?),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn installed_entries(paths: &Paths) -> Result<BTreeMap<String, Entry>> {
    let path = super::state_dir(paths).join("installed.json");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    serde_json::from_str(&text).with_context(|| format!("{} doesn't parse", path.display()))
}

/// Adds the game's program as the default target, unless the environment
/// already has one by that name from an earlier attempt.
fn add_target(paths: &Paths, plan: &Plan, folder: &Path, start: &OfflineStart) -> Result<()> {
    let environment = Environment::load(paths, &plan.environment)?;
    if environment.config.targets.contains_key(TARGET) {
        return Ok(());
    }
    let text = |path: PathBuf| path.to_string_lossy().into_owned();
    let mut batch = Batch::new(environment.config.targets.clone());
    batch.add(
        TARGET,
        Spec {
            exe: text(folder.join(&start.executable)),
            // Legendary gives the game's own parameters at each start.
            args: Vec::new(),
            cwd: start
                .working_directory
                .as_deref()
                .map(|dir| text(folder.join(dir))),
            name: plan.desktop.then(|| plan.title.clone()),
            desktop: plan.desktop,
        },
    )?;
    batch.set_default(Some(TARGET));
    match targets::write(paths, &plan.environment, &batch)? {
        Written::Saved => Ok(()),
        Written::Changed(_) => bail!(
            "{}'s config changed while its target was being added",
            plan.environment
        ),
    }
}

/// The game folder `forget` left in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forgotten {
    pub folder: PathBuf,
}

/// Drops wineo's and Legendary's records of a game no environment owns,
/// in the library or the trash, leaving its files. Legendary is needed only
/// when it has the game in its records.
pub fn forget(
    paths: &Paths,
    legendary: Option<&Legendary>,
    app: &str,
    token: &Token,
) -> Result<Forgotten> {
    if paths.portable_root().is_some() {
        return Err(InstallError::Portable(HOST_ONLY).into());
    }
    crate::config::validate_epic_id(app).map_err(|error| anyhow!(error))?;
    let record = game::peek(paths, app)?.ok_or_else(|| InstallError::NoRecord {
        app: app.to_owned(),
    })?;
    let folder = record.folder.clone();
    let _in_use = game::lock_for(paths, &folder, &format!("Epic game {app}"), Work::Install)?;
    let never = || token.is_canceled();
    let lock = RecordLock::exclusive(paths, app, &never)?;
    let record = lock.read(paths)?.ok_or_else(|| InstallError::NoRecord {
        app: app.to_owned(),
    })?;
    if record.folder != folder {
        return Err(InstallError::Changed {
            what: format!("Epic game {app}'s record"),
        }
        .into());
    }
    if let Some(name) = game::owners(paths, app)?.into_iter().next() {
        return Err(InstallError::Owned {
            app: app.to_owned(),
            name,
        }
        .into());
    }
    if let Some(name) = crate::trash::epic_owner(paths, app)? {
        return Err(InstallError::Trashed {
            app: app.to_owned(),
            name,
        }
        .into());
    }
    // DLC first, so a game's entry never goes while its DLC's stays.
    let installed = game::legendary_installed(paths)?;
    let mut drop: Vec<String> = installed
        .iter()
        .filter(|(name, entry)| {
            name.as_str() != app
                && entry
                    .install_path
                    .as_deref()
                    .is_some_and(|path| same_folder(path, &folder))
        })
        .map(|(name, _)| name.clone())
        .collect();
    if installed.contains_key(app) {
        drop.push(app.to_owned());
    }
    if !drop.is_empty() {
        let legendary = legendary.ok_or_else(|| InstallError::NeedsLegendary {
            app: app.to_owned(),
        })?;
        // Legendary rewrites its records in place, so no other call of it
        // runs meanwhile.
        let _epic = EpicLock::acquire(paths)?;
        for name in &drop {
            legendary.run(&["-y", "uninstall", name, "--keep-files"], None, token)?;
        }
        let left = game::legendary_installed(paths)?;
        if let Some(name) = drop.iter().find(|name| left.contains_key(name.as_str())) {
            bail!("Legendary still has {name} in its records after forgetting it");
        }
    }
    // What Legendary keeps to resume a download that didn't finish.
    let tmp = super::state_dir(paths).join("tmp");
    for name in std::iter::once(app).chain(record.dlc.iter().map(|dlc| dlc.app_name.as_str())) {
        let resume = tmp.join(format!("{name}.resume"));
        crate::files::remove_if_present(&resume)
            .with_context(|| format!("failed to remove {}", resume.display()))?;
    }
    if let Some(mark) = &record.in_flight
        && let Some(name) = mark.app.as_deref()
    {
        let resume = tmp.join(format!("{name}.resume"));
        crate::files::remove_if_present(&resume)
            .with_context(|| format!("failed to remove {}", resume.display()))?;
    }
    lock.remove(paths)?;
    Ok(Forgotten { folder })
}

/// What wineo has of a game, for listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalState {
    /// An install began and didn't finish.
    Unfinished,
    /// Whole, with no environment owning it.
    Downloaded,
    Installed,
}

#[derive(Debug, Clone)]
pub struct Local {
    pub state: LocalState,
    pub environment: Option<String>,
    pub version: Option<String>,
    /// The installed DLC's app names.
    pub dlc: Vec<String>,
}

/// Every game wineo has a record of, by app name, and a line for each record
/// that can't be read.
pub fn local(paths: &Paths) -> Result<(BTreeMap<String, Local>, Vec<String>)> {
    let mut games = BTreeMap::new();
    let mut errors = Vec::new();
    if paths.portable_root().is_some() {
        return Ok((games, errors));
    }
    for (_, record) in game::list(paths)? {
        let record = match record {
            Ok(record) => record,
            Err(error) => {
                errors.push(format!("{error:#}"));
                continue;
            }
        };
        let environment = game::owners(paths, &record.app_name)?.into_iter().next();
        let unfinished = record
            .in_flight
            .as_ref()
            .is_some_and(|mark| mark.work == Work::Install);
        let state = match (&environment, unfinished) {
            (_, true) => LocalState::Unfinished,
            (Some(_), false) => LocalState::Installed,
            (None, false) => LocalState::Downloaded,
        };
        games.insert(
            record.app_name.clone(),
            Local {
                state,
                environment,
                version: record.installed_version.clone(),
                dlc: record.dlc.iter().map(|dlc| dlc.app_name.clone()).collect(),
            },
        );
    }
    Ok((games, errors))
}

/// Whether a run that ended with `error` was canceled.
pub fn is_cancel(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<InstallError>(),
        Some(InstallError::Canceled { .. })
    ) || matches!(error.downcast_ref::<GameError>(), Some(GameError::Canceled))
        || error
            .downcast_ref::<Error>()
            .is_some_and(Error::is_canceled)
        || matches!(
            error.downcast_ref::<crate::Error>(),
            Some(crate::Error::Canceled(_))
        )
}

/// The folder held while the game's record has an install mark is another
/// install under way, not one left to finish.
fn another_install(paths: &Paths, plan: &Plan, error: anyhow::Error) -> anyhow::Error {
    if matches!(error.downcast_ref(), Some(GameError::InUse { .. }))
        && left_unfinished(paths, &plan.app_name)
    {
        return InstallError::Running {
            title: plan.title.clone(),
        }
        .into();
    }
    error
}

/// Whether it stopped after the mark was written, so it is there to finish.
pub fn left_unfinished(paths: &Paths, app: &str) -> bool {
    game::peek(paths, app)
        .ok()
        .flatten()
        .and_then(|record| record.in_flight)
        .is_some_and(|mark| mark.work == Work::Install)
}

#[cfg(test)]
mod tests;
