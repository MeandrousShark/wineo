//! wineo's own record of each Epic game, and the locks around it.
//!
//! Legendary's `installed.json` can't be the only record: it isn't written
//! safely, Legendary can't be asked anything while logged out, and nothing
//! in it says an install or update is under way. So wineo keeps one small
//! file per game under `<host data>/epic/games/`, named by the SHA-256 of
//! the app name and replaced through a synced temporary file. Its in-flight
//! mark is written before Legendary starts and cleared only once the work
//! checks out, so a crash or a kill leaves the game refusing to launch.
//!
//! Which environment owns a game is its config's `epic_id`, never the
//! record. One environment owns a game: `set` can't change the key, copies
//! drop it, and a trash restore refuses one another environment has taken.
//!
//! Lock order, after an environment's operation lock: the game folder's
//! in-use lock, held shared by launches for their whole run and taken
//! exclusively, without waiting, by install, update, and repair; then the
//! record lock, held briefly around writes and checks of who owns a game.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::EnvironmentConfig;
use crate::gog::folder::{self as gog_folder, InUseAction, InUseGuard, InUseLock};
use crate::paths::Paths;

/// The record format this wineo writes. A record without one reads as this;
/// a newer one is refused, since its fields may mean something else.
pub const VERSION: u32 = 1;

/// What wineo knows about one Epic game. Every field but the app and the
/// folder may be missing, as in a record written when an install began.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default = "current_version")]
    pub version: u32,
    /// Epic's app name, the game's `epic_id`.
    pub app_name: String,
    /// Absolute and in normal form. The folder's in-use lock is keyed on it.
    pub folder: PathBuf,
    /// The base game's version, once an install has finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    /// From Legendary's `can_run_offline`, the publisher's flag.
    #[serde(default)]
    pub can_run_offline: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dlc: Vec<Dlc>,
    /// What an offline start runs when Legendary can't be asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline: Option<OfflineStart>,
    /// Present only while an install, update, or repair is under way, or
    /// after one that didn't finish. Any mark blocks launches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<InFlight>,
}

fn current_version() -> u32 {
    VERSION
}

/// An installed DLC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dlc {
    pub app_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// The program and arguments a start needs without Legendary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfflineStart {
    /// Relative to the game folder, as Legendary gives it.
    pub executable: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    /// Relative to the game folder. None means the folder itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

/// Work under way on a game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InFlight {
    #[serde(default)]
    pub work: Work,
    /// The app being worked on: the game's own app name or a DLC's. None
    /// in a mark that didn't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Work {
    Install,
    Update,
    Repair,
    /// A mark this wineo can't name, from a newer one or a hand edit. It
    /// still blocks launches.
    #[default]
    #[serde(other)]
    Unknown,
}

impl Work {
    fn in_use_action(self) -> InUseAction {
        match self {
            Self::Install => InUseAction::Install,
            Self::Update | Self::Unknown => InUseAction::Update,
            Self::Repair => InUseAction::Repair,
        }
    }

    /// How a message says it is happening: "is being updated".
    fn being(self) -> &'static str {
        match self {
            Self::Install => "installed",
            Self::Update | Self::Unknown => "updated",
            Self::Repair => "repaired",
        }
    }
}

impl Record {
    /// A record for a game whose install is about to begin.
    pub fn new(app_name: &str, folder: PathBuf) -> Self {
        Self {
            version: VERSION,
            app_name: app_name.to_owned(),
            folder,
            installed_version: None,
            can_run_offline: false,
            dlc: Vec::new(),
            offline: None,
            in_flight: None,
        }
    }

    /// Reads a record leniently: fields it lacks take their defaults, and
    /// fields it doesn't know are ignored.
    pub fn parse(text: &str) -> Result<Self> {
        let record: Self =
            toml::from_str(text).map_err(|error| anyhow!("{}", error.message().trim_end()))?;
        record.check()?;
        Ok(record)
    }

    pub fn to_toml(&self) -> Result<String> {
        self.check()?;
        Ok(toml::to_string_pretty(self)?)
    }

    fn check(&self) -> Result<()> {
        if self.version > VERSION {
            bail!(
                "Epic game record version {} is from a newer wineo",
                self.version
            );
        }
        crate::config::validate_epic_id(&self.app_name).map_err(|error| anyhow!(error))?;
        check_folder(&self.folder)?;
        Ok(())
    }
}

/// Refuses a folder that isn't absolute or isn't in normal form, either of
/// which would give one folder a second lock.
pub fn check_folder(folder: &Path) -> Result<()> {
    let normal: PathBuf = folder.components().collect();
    if !folder.is_absolute()
        || normal.as_os_str() != folder.as_os_str()
        || folder
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        bail!(
            "Epic game folder {} isn't a canonical path",
            folder.display()
        );
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The record's file name for a game, without `.toml`. App names can hold
/// anything a config string can, so they never name a file themselves.
pub fn key(app_name: &str) -> String {
    digest(app_name.as_bytes())
}

fn folder_key(folder: &Path) -> String {
    digest(folder.as_os_str().as_encoded_bytes())
}

pub fn record_path(paths: &Paths, app_name: &str) -> PathBuf {
    paths
        .epic_games_dir()
        .join(format!("{}.toml", key(app_name)))
}

/// Refusals a frontend may want to tell apart.
#[derive(Debug, thiserror::Error)]
pub enum GameError {
    /// An in-flight mark, left by work that is running or didn't finish.
    #[error("{}", unfinished_message(.name.as_deref(), .app, .folder, *.work))]
    Unfinished {
        /// The environment that owns the game, when one does.
        name: Option<String>,
        app: String,
        folder: PathBuf,
        work: Work,
    },
    #[error("{name}'s Epic record is missing. Repair it with wineo update {name} --repair.")]
    RecordMissing { name: String, app: String },
    #[error("{name}'s Epic record can't be checked: {detail}.")]
    Unreadable { name: String, detail: String },
    /// A record that can't be read whose folder, read leniently, holds a
    /// launched path. The detail is validate's `epic_record_invalid`
    /// message.
    #[error("{detail}")]
    Invalid { detail: String },
    /// A launch found the folder's in-use lock held by an install, update,
    /// or repair.
    #[error("{label}'s Epic game is being {}.", .work.being())]
    Busy { label: String, work: Work },
    /// Work found something running from the folder. `holders` names the
    /// environments running from it, sorted, and is empty when none could
    /// be named.
    #[error("{}", in_use_message(.label, .holders, *.work))]
    InUse {
        label: String,
        holders: Vec<String>,
        work: Work,
    },
    /// `wineo rm` of an environment whose game's install didn't finish.
    #[error(
        "{name}'s Epic install didn't finish. Finish it with wineo import epic --game {app}, or remove it anyway with --force."
    )]
    RemoveMidInstall { name: String, app: String },
    /// `wineo rm` of an environment whose game is mid-update or mid-repair.
    #[error(
        "{name} is mid-{}. Finish it with wineo update {name}{}, or remove it anyway with --force.",
        if *.work == Work::Repair { "repair" } else { "update" },
        if *.work == Work::Repair { " --repair" } else { "" }
    )]
    RemoveMidWork { name: String, work: Work },
    #[error("{name}'s Epic game {app} belongs to {other} now.")]
    Taken {
        name: String,
        app: String,
        other: String,
    },
    #[error("Canceled.")]
    Canceled,
}

fn unfinished_message(name: Option<&str>, app: &str, folder: &Path, work: Work) -> String {
    match (name, work) {
        (Some(name), Work::Install) => format!(
            "{name}'s Epic install didn't finish. Finish it with wineo import epic --game {app}."
        ),
        (Some(name), Work::Update) => {
            format!("{name}'s game folder is mid-update. Finish it with wineo update {name}.")
        }
        (Some(name), Work::Repair) => format!(
            "{name}'s game folder is mid-repair. Finish it with wineo update {name} --repair."
        ),
        (Some(name), Work::Unknown) => format!(
            "{name}'s game folder has Epic work that didn't finish. Repair it with wineo update {name} --repair."
        ),
        (None, Work::Install) => format!(
            "Epic game {app}'s install in {} didn't finish. Finish it with wineo import epic --game {app}.",
            folder.display()
        ),
        (None, _) => format!(
            "Epic game {app}'s folder {} is mid-{}. Finish it from the environment that owns it.",
            folder.display(),
            match work {
                Work::Repair => "repair",
                _ => "update",
            }
        ),
    }
}

fn in_use_message(label: &str, holders: &[String], work: Work) -> String {
    // No name means another install, update, or repair as often as a game
    // whose holder record couldn't be written.
    if holders.is_empty() {
        return format!("Something else is using {label}'s game folder. Try again once it's done.");
    }
    gog_folder::in_use_message(label, holders, work.in_use_action())
}

fn read_file(path: &Path) -> Result<Option<Record>> {
    match fs::read_to_string(path) {
        Ok(text) => Record::parse(&text).map(Some).map_err(|error| {
            anyhow!(
                "{} isn't a valid Epic game record ({error:#})",
                path.display()
            )
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// A game's record read without its lock, so the answer can be stale.
pub fn peek(paths: &Paths, app_name: &str) -> Result<Option<Record>> {
    let path = record_path(paths, app_name);
    let record = read_file(&path)?;
    if let Some(record) = &record
        && record.app_name != app_name
    {
        bail!(
            "{} names {}, not {app_name}",
            path.display(),
            record.app_name
        );
    }
    Ok(record)
}

/// Every record, read without locks. Unreadable files are returned as
/// errors rather than skipped.
pub fn list(paths: &Paths) -> Result<Vec<(PathBuf, Result<Record>)>> {
    let dir = paths.epic_games_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", dir.display()));
        }
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // Dot files are a write's temporary files.
        if name.starts_with('.') || !name.ends_with(".toml") {
            continue;
        }
        let record = read_file(&path).and_then(|record| {
            let record = record.ok_or_else(|| anyhow!("{} disappeared", path.display()))?;
            if path.file_stem().and_then(|stem| stem.to_str()) != Some(&key(&record.app_name)) {
                bail!(
                    "{} names {}, which isn't its key",
                    path.display(),
                    record.app_name
                );
            }
            Ok(record)
        });
        found.push((path, record));
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(found)
}

/// One game in Legendary's own record of what it installed, as much as
/// wineo compares with its records.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Installed {
    #[serde(default)]
    pub install_path: Option<PathBuf>,
    #[serde(default)]
    pub version: Option<String>,
}

/// Legendary's `installed.json` in wineo's Legendary folder, by app name,
/// read from the file rather than by running Legendary. No file means
/// nothing installed.
pub fn legendary_installed(paths: &Paths) -> Result<BTreeMap<String, Installed>> {
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

/// The environments whose config names `app_name` as their `epic_id`,
/// sorted. One whose config can't be read is skipped: it names no game
/// until it can be.
pub fn owners(paths: &Paths, app_name: &str) -> Result<Vec<String>> {
    let mut owners = Vec::new();
    for name in crate::environment::names(paths)? {
        if epic_id_of(paths, &name).as_deref() == Some(app_name) {
            owners.push(name);
        }
    }
    Ok(owners)
}

/// Whether `name`'s config could name an Epic game, so the records have to
/// be readable for it. As GOG's `may_track_game`: only a config that reads
/// as TOML without `epic_id` can't.
pub fn may_own_game(paths: &Paths, name: &str) -> bool {
    match fs::read_to_string(paths.environment_config(name)) {
        Ok(text) => text
            .parse::<toml_edit::DocumentMut>()
            .map_or(true, |doc| doc.get("epic_id").is_some()),
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

/// Whether `name`'s config reads as TOML with `epic_id`, whether or not it
/// is a valid config: evidence that an Epic game exists.
pub fn names_epic_id(paths: &Paths, name: &str) -> bool {
    fs::read_to_string(paths.environment_config(name))
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .is_some_and(|doc| doc.get("epic_id").is_some())
}

/// An environment's `epic_id`, if its config can be read.
pub fn epic_id_of(paths: &Paths, name: &str) -> Option<String> {
    let path = paths.environment_config(name);
    let text = fs::read_to_string(&path).ok()?;
    EnvironmentConfig::parse(&path, &text).ok()?.epic_id
}

/// Holds a game's record lock until dropped.
pub struct RecordLock {
    file: File,
    app_name: String,
    exclusive: bool,
}

impl RecordLock {
    /// Waits for the lock shared, polling so `cancel` still works.
    pub fn shared(paths: &Paths, app_name: &str, cancel: &dyn Fn() -> bool) -> Result<Self> {
        Self::take(paths, app_name, false, cancel)
    }

    /// Waits for the lock exclusively, polling so `cancel` still works.
    /// Every write needs this, and so does every change of who owns the
    /// game.
    pub fn exclusive(paths: &Paths, app_name: &str, cancel: &dyn Fn() -> bool) -> Result<Self> {
        Self::take(paths, app_name, true, cancel)
    }

    fn take(
        paths: &Paths,
        app_name: &str,
        exclusive: bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Self> {
        crate::config::validate_epic_id(app_name).map_err(|error| anyhow!(error))?;
        let file = gog_folder::lock_file(&paths.epic_game_lock(&key(app_name)), exclusive, cancel)
            .map_err(|error| match error.downcast_ref() {
                Some(gog_folder::FolderError::Canceled) => GameError::Canceled.into(),
                _ => error,
            })?;
        Ok(Self {
            file,
            app_name: app_name.to_owned(),
            exclusive,
        })
    }

    pub fn read(&self, paths: &Paths) -> Result<Option<Record>> {
        peek(paths, &self.app_name)
    }

    /// Replaces the record in one durable write.
    pub fn write(&self, paths: &Paths, record: &Record) -> Result<()> {
        self.require_exclusive()?;
        if record.app_name != self.app_name {
            bail!(
                "record for {} written under the wrong lock",
                record.app_name
            );
        }
        write_durable(paths, &self.app_name, &record.to_toml()?)
    }

    /// Sets the in-flight mark, before any work starts.
    pub fn mark(&self, paths: &Paths, record: &mut Record, work: Work, app: &str) -> Result<()> {
        record.in_flight = Some(InFlight {
            work,
            app: Some(app.to_owned()),
        });
        self.write(paths, record)
    }

    /// Clears the in-flight mark once the work has checked out.
    pub fn clear_mark(&self, paths: &Paths, record: &mut Record) -> Result<()> {
        record.in_flight = None;
        self.write(paths, record)
    }

    /// Removes the record, if there is one.
    pub fn remove(&self, paths: &Paths) -> Result<bool> {
        self.require_exclusive()?;
        let dir = paths.epic_games_dir();
        remove_stale_temps(&dir, &temp_prefix(&self.app_name))?;
        let removed = crate::files::remove_if_present(&record_path(paths, &self.app_name))?;
        if removed {
            crate::files::sync_dir(&dir)?;
        }
        Ok(removed)
    }

    fn require_exclusive(&self) -> Result<()> {
        if !self.exclusive {
            bail!("Epic game record changed under a shared lock");
        }
        Ok(())
    }
}

impl Drop for RecordLock {
    fn drop(&mut self) {
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

fn temp_prefix(app_name: &str) -> String {
    format!(".{}-", key(app_name))
}

/// Writes through a synced temporary file in the same folder, then syncs
/// the folder, so a crash leaves either the old record or the new one.
fn write_durable(paths: &Paths, app_name: &str, text: &str) -> Result<()> {
    let dir = paths.epic_games_dir();
    gog_folder::create_dir_durable(&dir)?;
    let prefix = temp_prefix(app_name);
    remove_stale_temps(&dir, &prefix)?;
    let mut temp = tempfile::Builder::new()
        .prefix(&prefix)
        .tempfile_in(&dir)
        .with_context(|| format!("failed to write in {}", dir.display()))?;
    temp.write_all(text.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(record_path(paths, app_name))
        .map_err(|error| error.error)?;
    crate::files::sync_dir(&dir)?;
    Ok(())
}

/// Removes what an interrupted write of one record left. Only called under
/// that record's exclusive lock, so none is in use.
fn remove_stale_temps(dir: &Path, prefix: &str) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", dir.display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(prefix))
        {
            fs::remove_file(entry.path())
                .with_context(|| format!("failed to remove {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// A game folder's in-use lock.
pub fn in_use_lock(paths: &Paths, folder: &Path) -> Result<InUseLock> {
    check_folder(folder)?;
    let key = folder_key(folder);
    Ok(InUseLock::at(
        paths.epic_folder_in_use_lock(&key),
        paths.epic_folder_holders(&key),
        folder,
    ))
}

/// Takes a game folder's in-use lock exclusively for `work`, never waiting:
/// refused while anything runs from the folder, or while other work holds
/// it. `label` is how the game is named in the refusal.
pub fn lock_for(paths: &Paths, folder: &Path, label: &str, work: Work) -> Result<InUseGuard> {
    let lock = in_use_lock(paths, folder)?;
    lock.try_exclusive()?.ok_or_else(|| {
        GameError::InUse {
            label: label.to_owned(),
            holders: lock.holders(),
            work,
        }
        .into()
    })
}

/// The records a launch of `name` touches, by folder key, which is lock
/// order: its own game's when it has `epic_id`, and any whose folder holds
/// one of `touched`, the host paths it names.
struct Touched<'a> {
    name: &'a str,
    epic_id: Option<&'a str>,
    records: BTreeMap<String, Record>,
}

impl<'a> Touched<'a> {
    fn find(
        paths: &Paths,
        name: &'a str,
        epic_id: Option<&'a str>,
        touched: &[PathBuf],
    ) -> Result<Self> {
        // Epic games live in the host library only, so a portable
        // environment's `epic_id` names nothing it could own.
        let epic_id = epic_id.filter(|_| paths.portable_root().is_none());
        let mut records = BTreeMap::new();
        if let Some(app) = epic_id {
            let record = peek(paths, app)
                .map_err(|error| GameError::Unreadable {
                    name: name.to_owned(),
                    detail: format!("{error:#}"),
                })?
                .ok_or_else(|| GameError::RecordMissing {
                    name: name.to_owned(),
                    app: app.to_owned(),
                })?;
            records.insert(folder_key(&record.folder), record);
        }
        let resolved: Vec<PathBuf> = touched
            .iter()
            .filter(|path| path.is_absolute())
            .map(|path| crate::paths::resolve(path))
            .collect();
        if !resolved.is_empty() {
            let covers = |folder: &Path| resolved.iter().any(|path| path.starts_with(folder));
            for (path, record) in list(paths)? {
                match record {
                    Ok(record) if covers(&record.folder) => {
                        records.entry(folder_key(&record.folder)).or_insert(record);
                    }
                    Ok(_) => {}
                    // Fails closed, as GOG does for a state it can't read,
                    // when the broken record still names a folder the launch
                    // runs from. One that names none can't block every
                    // launch; validate reports it.
                    Err(error) if claimed_folder(&path).is_some_and(|folder| covers(&folder)) => {
                        return Err(GameError::Invalid {
                            detail: format!("{error:#}"),
                        }
                        .into());
                    }
                    Err(_) => {}
                }
            }
        }
        Ok(Self {
            name,
            epic_id,
            records,
        })
    }

    fn own(&self, record: &Record) -> bool {
        self.epic_id == Some(record.app_name.as_str())
    }

    /// Who a message names for a record: this launch's environment for its
    /// own game, else the environment that owns it, if any.
    fn owner(&self, paths: &Paths, record: &Record) -> Option<String> {
        if self.own(record) {
            return Some(self.name.to_owned());
        }
        owners(paths, &record.app_name)
            .ok()
            .and_then(|owners| owners.into_iter().next())
    }

    fn label(&self, paths: &Paths, record: &Record) -> String {
        self.owner(paths, record)
            .unwrap_or_else(|| format!("Epic game {}", record.app_name))
    }

    /// Takes every folder's in-use lock shared as this launch's
    /// environment, refusing one that work holds.
    fn lock_in_use(&self, paths: &Paths) -> Result<Vec<InUseGuard>> {
        let mut guards = Vec::new();
        for record in self.records.values() {
            match in_use_lock(paths, &record.folder)?.try_shared(self.name)? {
                Some(guard) => guards.push(guard),
                None => {
                    // The work marks the record before it starts, so the
                    // mark says what it is.
                    let work = peek(paths, &record.app_name)
                        .ok()
                        .flatten()
                        .and_then(|record| record.in_flight)
                        .map_or(Work::Update, |mark| mark.work);
                    return Err(GameError::Busy {
                        label: self.label(paths, record),
                        work,
                    }
                    .into());
                }
            }
        }
        Ok(guards)
    }

    /// Refuses a record that is gone, when it is this launch's own game, or
    /// that carries an in-flight mark.
    fn check(&self, paths: &Paths, found: &Record, now: Option<Record>) -> Result<()> {
        let Some(record) = now else {
            if self.own(found) {
                return Err(GameError::RecordMissing {
                    name: self.name.to_owned(),
                    app: found.app_name.clone(),
                }
                .into());
            }
            // Forgotten since it was found, so no longer a game folder.
            return Ok(());
        };
        if let Some(mark) = &record.in_flight {
            return Err(GameError::Unfinished {
                name: self.owner(paths, &record),
                app: record.app_name.clone(),
                folder: record.folder.clone(),
                work: mark.work,
            }
            .into());
        }
        Ok(())
    }
}

/// The folder a record that doesn't read names, if it names one at all.
fn claimed_folder(path: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(path).ok()?;
    let table: toml::Table = text.parse().ok()?;
    let folder = PathBuf::from(table.get("folder")?.as_str()?);
    folder.is_absolute().then_some(folder)
}

/// How many times a launch looks for its folders again when one became a
/// game folder while it was locking, before giving up.
const ATTEMPTS: usize = 3;

/// Finds the folders a launch touches and takes their in-use locks. They
/// are found without locks, so they're looked for again once locked: a
/// record written meanwhile for a folder the launch runs from means
/// starting over, which keeps the locks in key order.
fn lock_touched<'a>(
    paths: &Paths,
    name: &'a str,
    epic_id: Option<&'a str>,
    touched: &[PathBuf],
) -> Result<(Touched<'a>, Vec<InUseGuard>)> {
    for _ in 0..ATTEMPTS {
        let found = Touched::find(paths, name, epic_id, touched)?;
        between_find_and_lock();
        let guards = found.lock_in_use(paths)?;
        let again = Touched::find(paths, name, epic_id, touched)?;
        if again
            .records
            .keys()
            .all(|key| found.records.contains_key(key))
        {
            return Ok((again, guards));
        }
    }
    bail!("{name}'s Epic game folders changed while locking; retry")
}

#[cfg(test)]
thread_local! {
    static BETWEEN: std::cell::RefCell<Option<Box<dyn FnMut() -> bool>>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs a test's hook between finding folders and locking them. It stays
/// armed while it returns true.
fn between_find_and_lock() {
    #[cfg(test)]
    if let Some(mut hook) = BETWEEN.with(|hook| hook.borrow_mut().take())
        && hook()
    {
        BETWEEN.with(|slot| *slot.borrow_mut() = Some(hook));
    }
}

/// What `protect` would refuse, found without keeping any lock, so a launch
/// can be refused before its hooks run.
pub fn precheck(
    paths: &Paths,
    name: &str,
    epic_id: Option<&str>,
    touched: &[PathBuf],
) -> Result<()> {
    let found = Touched::find(paths, name, epic_id, touched)?;
    drop(found.lock_in_use(paths)?);
    for record in found.records.values() {
        found.check(paths, record, Some(record.clone()))?;
    }
    Ok(())
}

/// Checks a launch of `name` and takes the in-use lock of every Epic game
/// folder it runs from, in key order: its own game's when `epic_id` is set,
/// and any folder holding one of `touched`. Refuses a game with an
/// in-flight mark, one being worked on now, and one whose record is gone.
/// The caller holds the guards until the program exits.
pub fn protect(
    paths: &Paths,
    name: &str,
    epic_id: Option<&str>,
    touched: &[PathBuf],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<InUseGuard>> {
    let (found, guards) = lock_touched(paths, name, epic_id, touched)?;
    for record in found.records.values() {
        let now = RecordLock::shared(paths, &record.app_name, cancel)?
            .read(paths)
            .map_err(|error| GameError::Unreadable {
                name: found.label(paths, record),
                detail: format!("{error:#}"),
            })?;
        found.check(paths, record, now)?;
    }
    Ok(guards)
}

/// What removing `name` holds: its Epic game's record lock, so no work on
/// the game can mark it meanwhile. None when it names no game with a record.
/// Refuses a game with a mark unless `force`, which leaves the mark for
/// installing again to finish.
pub fn lock_for_remove(
    paths: &Paths,
    name: &str,
    force: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<Option<RecordLock>> {
    let Some(app) = epic_id_of(paths, name) else {
        return Ok(None);
    };
    let lock = RecordLock::exclusive(paths, &app, cancel)?;
    let Some(record) = lock.read(paths)? else {
        return Ok(None);
    };
    if !force && let Some(mark) = &record.in_flight {
        return Err(match mark.work {
            Work::Install => GameError::RemoveMidInstall {
                name: name.to_owned(),
                app,
            },
            work => GameError::RemoveMidWork {
                name: name.to_owned(),
                work,
            },
        }
        .into());
    }
    Ok(Some(lock))
}

/// Refuses bringing `name` back with `epic_id` while another environment
/// owns that game. Hold the returned lock until the config is back, so no
/// install can take the game in between.
pub fn claim_for_restore(paths: &Paths, name: &str, epic_id: &str) -> Result<RecordLock> {
    let lock = RecordLock::exclusive(paths, epic_id, &|| false)?;
    if let Some(other) = owners(paths, epic_id)?
        .into_iter()
        .find(|owner| owner != name)
    {
        return Err(GameError::Taken {
            name: name.to_owned(),
            app: epic_id.to_owned(),
            other,
        }
        .into());
    }
    Ok(lock)
}

#[cfg(test)]
mod tests;
