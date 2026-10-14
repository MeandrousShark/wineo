//! GOG game folders: one state file per folder, holding who owns it, what was
//! last installed there, and a write-ahead journal while work is in flight.
//!
//! State files live in the host data dir, keyed by the SHA-256 of the
//! folder's canonical path. Each folder has two host-wide locks: the state
//! lock (`FolderLock`), held briefly, and every change to the state file is
//! made holding it exclusively; and the in-use lock (`InUseLock`), held
//! shared by a launch for its whole run and taken exclusively, without
//! waiting, by update and migration.
//!
//! Lock order: the adoption lock (migrations and fresh installs), an
//! environment's operation lock, then in-use locks in key order, then state
//! locks in key order, then config locks.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::files;
use crate::name;
use crate::paths::Paths;

mod holders;
mod in_use;
mod protect;

pub use in_use::{InUseGuard, InUseLock, lock_for_migration, lock_for_update};
pub use protect::{folder_holding, game_folder, precheck, protect};

/// The state file format. A file with any other version is refused.
pub const VERSION: u32 = 1;

const LOCK_RETRY: Duration = Duration::from_millis(50);

/// Everything wineo knows about one GOG game folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderState {
    pub version: u32,
    /// The environment that owns the game. None after `rm`, or before a
    /// trash restore or launch claims it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Canonical, and what the file name is derived from.
    pub folder: PathBuf,
    /// The filesystem the folder was on at the last commit. None means
    /// unknown, which keeps `validate --fix` from ever removing the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<u64>,
    /// Absent until a first install finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed: Option<Committed>,
    /// Present only while work is in flight. Any journal blocks launches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<Journal>,
}

/// The last finished install or update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Committed {
    pub gog_id: u64,
    pub platform: Platform,
    pub language: String,
    pub track: Track,
    /// The base game first, then each DLC in the order it was added. A file
    /// shipped by several products comes from the latest one.
    #[serde(default)]
    pub products: Vec<Product>,
    /// Native games only, and empty for a migrated game until its first
    /// update.
    #[serde(default)]
    pub files: Vec<WinningEntry>,
    /// How the game got here: `install` when wineo installed it. None means
    /// it was migrated, as every state from before this field was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

/// Who put a game in its folder, when it wasn't a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Install,
}

/// What an operation will commit, enough to finish it without asking GOG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub gog_id: u64,
    pub platform: Platform,
    pub language: String,
    pub track: Track,
    /// In order, with each product's intended version and status.
    #[serde(default)]
    pub products: Vec<Product>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Windows,
}

/// How updates are compared: by offline installer version or Galaxy build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    Installer,
    Galaxy,
}

impl Track {
    /// The word for it, as stored.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installer => "installer",
            Self::Galaxy => "galaxy",
        }
    }
}

/// A base game or DLC in a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    pub id: u64,
    /// The installer version, on the installer track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The build id, on the Galaxy track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// GOG's code for the language this product was installed in, when it
    /// isn't the folder's, like a DLC that fell back to English. None means
    /// the folder's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// GOG's code for the language a Windows product's `goggame-<id>.info`
    /// reported after installing, only when it isn't the one asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_language: Option<String>,
    pub status: ProductStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProductStatus {
    Installed,
    Failed,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Symlink,
    Directory,
}

/// A file a product shipped that won, recorded after split files are
/// assembled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WinningEntry {
    /// Relative to the folder.
    pub path: String,
    #[serde(rename = "type")]
    pub kind: EntryType,
    /// Files only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    /// Symlinks only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub mode: u32,
    pub product: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalKind {
    Install,
    Update,
    Reinstall,
    Repair,
}

/// Work in flight, written durably before the folder is touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    /// Names the operation's temporary files, `.wineo-<operation>-<n>`.
    pub operation: String,
    pub kind: JournalKind,
    pub target: Target,
    #[serde(default)]
    pub packages: Vec<Package>,
    /// Every entry any attempt of this operation is about to write, kept
    /// across failed attempts.
    #[serde(default)]
    pub attempted: Vec<Attempted>,
}

/// An exact GOG file an operation uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub product: u64,
    pub download_id: String,
    pub file_id: String,
    pub size: u64,
    /// None when GOG publishes no checksum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempted {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: EntryType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl FolderState {
    /// A state for a folder with nothing committed yet.
    pub fn new(folder: PathBuf, owner: Option<String>) -> Self {
        Self {
            version: VERSION,
            owner,
            folder,
            device: None,
            committed: None,
            journal: None,
        }
    }

    /// The game's GOG id: the committed one, or the journal's while a first
    /// install is in flight.
    pub fn gog_id(&self) -> Option<u64> {
        self.committed
            .as_ref()
            .map(|committed| committed.gog_id)
            .or_else(|| self.journal.as_ref().map(|journal| journal.target.gog_id))
    }

    /// Whether this is the game `gog_id`, a config's value.
    pub fn is_game(&self, gog_id: &str) -> bool {
        gog_id
            .parse::<u64>()
            .ok()
            .is_some_and(|id| self.gog_id() == Some(id))
    }

    /// How messages name the game: its owner, or its GOG id.
    pub fn label(&self) -> String {
        match (&self.owner, self.gog_id()) {
            (Some(owner), _) => owner.clone(),
            (None, Some(id)) => format!("GOG game {id}"),
            (None, None) => format!("the GOG game in {}", self.folder.display()),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let state: Self =
            toml::from_str(text).map_err(|error| anyhow!("{}", error.message().trim_end()))?;
        state.check()?;
        Ok(state)
    }

    pub fn to_toml(&self) -> Result<String> {
        self.check()?;
        Ok(toml::to_string_pretty(self)?)
    }

    fn check(&self) -> Result<()> {
        if self.version != VERSION {
            bail!("unsupported GOG folder state version {}", self.version);
        }
        check_folder(&self.folder)?;
        if let Some(owner) = &self.owner
            && !name::is_valid(owner)
        {
            bail!("invalid GOG folder owner {owner}");
        }
        if let Some(committed) = &self.committed {
            for entry in &committed.files {
                check_entry(
                    &entry.path,
                    entry.kind,
                    entry.md5.is_some() && entry.size.is_some(),
                    entry.target.is_some(),
                    entry.size.is_some() || entry.md5.is_some(),
                )?;
            }
        }
        if let Some(journal) = &self.journal {
            if journal.operation.is_empty()
                || !journal
                    .operation
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                bail!("invalid GOG operation id {}", journal.operation);
            }
            for entry in &journal.attempted {
                // A Windows install names only the folder itself, since its
                // installers don't say which files they write.
                if entry.path == "."
                    && entry.kind == EntryType::Directory
                    && entry.md5.is_none()
                    && entry.target.is_none()
                {
                    continue;
                }
                check_entry(
                    &entry.path,
                    entry.kind,
                    entry.md5.is_some(),
                    entry.target.is_some(),
                    entry.md5.is_some(),
                )?;
            }
        }
        Ok(())
    }
}

/// Files carry content details, symlinks a target, and directories neither.
fn check_entry(
    path: &str,
    kind: EntryType,
    has_content: bool,
    has_target: bool,
    has_any_content: bool,
) -> Result<()> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        bail!("invalid GOG game file path {path}");
    }
    let fits = match kind {
        EntryType::File => has_content && !has_target,
        EntryType::Symlink => has_target && !has_any_content,
        EntryType::Directory => !has_target && !has_any_content,
    };
    if !fits {
        bail!("GOG game file {path} has fields that don't match its type");
    }
    Ok(())
}

/// Refuses a folder path that isn't in normal form: relative, or with `.`,
/// `..`, repeated, or trailing separators, any of which would give the same
/// folder a second key.
pub fn check_folder(folder: &Path) -> Result<()> {
    let normal: PathBuf = folder.components().collect();
    if !folder.is_absolute()
        || normal.as_os_str() != folder.as_os_str()
        || folder
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        bail!("GOG folder {} isn't a canonical path", folder.display());
    }
    Ok(())
}

/// The state file name for a canonical folder.
pub fn key(folder: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(folder.as_os_str().as_encoded_bytes())
    )
}

pub fn state_path(paths: &Paths, folder: &Path) -> PathBuf {
    paths
        .gog_folders_dir()
        .join(format!("{}.toml", key(folder)))
}

/// The folder as state files name it. The folder has to exist.
pub fn canonical(folder: &Path) -> Result<PathBuf> {
    fs::canonicalize(folder).with_context(|| format!("failed to resolve {}", folder.display()))
}

/// Refusals a frontend may want to tell apart.
#[derive(Debug, thiserror::Error)]
pub enum FolderError {
    #[error(
        "{name} is mid-update. Finish it with wineo update {name}, or remove it anyway with --force."
    )]
    RemoveMidUpdate { name: String },
    #[error("{name} is mid-update. Finish it with wineo update {name}.")]
    UnsetMidUpdate { name: String },
    #[error("{folder} belongs to {owner}.")]
    OwnedByOther { folder: PathBuf, owner: String },
    #[error("{folder} overlaps {other}'s game folder {other_folder}.")]
    Overlap {
        folder: PathBuf,
        other: String,
        other_folder: PathBuf,
    },
    /// A fresh install into a folder that already has files and isn't this
    /// environment's own unfinished install.
    #[error("{folder} already exists and isn't empty.")]
    NotEmpty { folder: PathBuf },
    /// A title with nothing left once it is made safe for a folder name.
    #[error("{title:?} can't be made into a folder name.")]
    UnusableTitle { title: String },
    #[error("No GOG state for {0}.")]
    NoState(PathBuf),
    #[error("{name}'s GOG state can't be checked: {detail}.")]
    Unreadable { name: String, detail: String },
    #[error("Canceled.")]
    Canceled,
    #[error(
        "{name}'s GOG state is missing. Migrate it again with wineo import gog --game {gog_id}, or stop tracking it with wineo set {name} --unset gog_id."
    )]
    StateMissing { name: String, gog_id: String },
    /// A launch found the folder's in-use lock held by an update or
    /// migration.
    #[error("{name}'s game folder is being updated.")]
    Updating { name: String },
    #[error("{name}'s game folder is mid-update. Finish it with wineo update {name}.")]
    MidUpdate { name: String },
    #[error(
        "GOG game {gog_id}'s folder is mid-update. Migrate it again with wineo import gog --game {gog_id} and finish it."
    )]
    OwnerlessMidUpdate { gog_id: u64 },
    /// An install whose state was written but whose environment's config
    /// wasn't yet.
    #[error(
        "{name}'s GOG install didn't finish. Install GOG game {gog_id} again as {name} to finish it."
    )]
    UnfinishedInstall { name: String, gog_id: u64 },
    /// An unfinished install whose environment was removed. Installing again
    /// only finishes an environment's own install.
    #[error(
        "GOG game {gog_id}'s install in {folder} didn't finish, and no environment owns it. Delete the folder, forget it with wineo import gog --forget {folder}, and install the game again."
    )]
    OwnerlessInstall { gog_id: u64, folder: PathBuf },
    /// A folder with no files but a GOG state of its own.
    #[error(
        "{folder} is already a GOG game folder. Forget it with wineo import gog --forget {folder} first."
    )]
    Tracked { folder: PathBuf },
    /// An update or migration found something running from the folder.
    /// `holders` names the environments running from it, sorted, and is
    /// empty when none could be named.
    #[error("{}", in_use_message(.name, .holders, *.action))]
    InUse {
        name: String,
        holders: Vec<String>,
        action: InUseAction,
    },
    /// More than one ownerless folder of an environment's game is among the
    /// paths it names, so which one is its own can't be told.
    #[error(
        "More than one game folder could be {name}'s: {}.",
        .folders.iter().map(|folder| folder.display().to_string()).collect::<Vec<_>>().join(", ")
    )]
    SeveralFolders { name: String, folders: Vec<PathBuf> },
    /// A state file that would cover a launched path can't be read.
    #[error("The GOG state for {folder} can't be checked: {detail}.")]
    UnreadableFolder { folder: PathBuf, detail: String },
}

/// What a refused in-use lock was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InUseAction {
    Update,
    Migrate,
    Forget,
}

/// The refusal for an update or migration of `name`'s game folder while
/// something runs from it.
fn in_use_message(name: &str, holders: &[String], action: InUseAction) -> String {
    let (who, verb, stop) = match holders {
        [] => return format!("Something is running from {name}'s game folder. Stop it first."),
        [one] => (one.clone(), "is", "it"),
        [one, two] => (format!("{one} and {two}"), "are", "them"),
        [rest @ .., last] => (format!("{}, and {last}", rest.join(", ")), "are", "them"),
    };
    let doing = match action {
        InUseAction::Update => "updating",
        InUseAction::Migrate => "migrating",
        InUseAction::Forget => "forgetting",
    };
    format!("{who} {verb} running from this game folder. Stop {stop} before {doing} {name}.")
}

/// Holds a folder's state lock until dropped.
pub struct FolderLock {
    file: File,
    folder: PathBuf,
    state: PathBuf,
    dir: PathBuf,
    exclusive: bool,
}

impl FolderLock {
    /// Waits for the folder's lock shared, polling so `cancel` still works.
    pub fn shared(paths: &Paths, folder: &Path, cancel: &dyn Fn() -> bool) -> Result<Self> {
        Self::take(paths, folder, false, cancel)
    }

    /// Waits for the folder's lock exclusively, polling so `cancel` still
    /// works. Every state change needs this.
    pub fn exclusive(paths: &Paths, folder: &Path, cancel: &dyn Fn() -> bool) -> Result<Self> {
        Self::take(paths, folder, true, cancel)
    }

    fn take(
        paths: &Paths,
        folder: &Path,
        exclusive: bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Self> {
        check_folder(folder)?;
        let file = lock_file(&paths.gog_folder_lock(&key(folder)), exclusive, cancel)?;
        Ok(Self {
            file,
            folder: folder.to_owned(),
            state: state_path(paths, folder),
            dir: paths.gog_folders_dir(),
            exclusive,
        })
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The folder's state, if it has one.
    pub fn read(&self) -> Result<Option<FolderState>> {
        let Some(state) = read_file(&self.state)? else {
            return Ok(None);
        };
        if state.folder != self.folder {
            bail!(
                "{} names {}, not {}",
                self.state.display(),
                state.folder.display(),
                self.folder.display()
            );
        }
        Ok(Some(state))
    }

    /// Replaces the state in one durable write.
    pub fn write(&self, state: &FolderState) -> Result<()> {
        self.require_exclusive()?;
        if state.folder != self.folder {
            bail!(
                "state for {} written under the wrong lock",
                state.folder.display()
            );
        }
        failpoint("write.before")?;
        write_durable(
            &self.dir,
            &self.state,
            &key(&self.folder),
            &state.to_toml()?,
        )?;
        failpoint("write.after")
    }

    /// Finishes an operation: replaces `[committed]`, drops `[journal]`,
    /// and records the folder's filesystem, in one write.
    pub fn commit(&self, committed: Committed) -> Result<FolderState> {
        let mut state = self
            .read()?
            .ok_or_else(|| FolderError::NoState(self.folder.clone()))?;
        // Finishing an install records it; anything later keeps what was
        // recorded, so an update of an installed game still says so.
        let source = match &state.journal {
            Some(journal) if journal.kind == JournalKind::Install => Some(Source::Install),
            _ => state.committed.as_ref().and_then(|old| old.source),
        };
        state.committed = Some(Committed {
            source: committed.source.or(source),
            ..committed
        });
        state.journal = None;
        state.device = Some(fs::metadata(&self.folder)?.dev());
        self.write(&state)?;
        Ok(state)
    }

    /// Sets the owner to none, keeping everything else.
    pub fn release(&self) -> Result<()> {
        let Some(mut state) = self.read()? else {
            return Ok(());
        };
        if state.owner.is_some() {
            state.owner = None;
            self.write(&state)?;
        }
        Ok(())
    }

    /// Sets the owner to `name`, keeping `[committed]` and any journal, so
    /// a half-updated folder stays blocked until it's finished. Refused when
    /// another environment owns it.
    pub fn reclaim(&self, name: &str) -> Result<FolderState> {
        let mut state = self
            .read()?
            .ok_or_else(|| FolderError::NoState(self.folder.clone()))?;
        match &state.owner {
            Some(owner) if owner == name => {}
            Some(owner) => {
                return Err(FolderError::OwnedByOther {
                    folder: self.folder.clone(),
                    owner: owner.clone(),
                }
                .into());
            }
            None => {
                state.owner = Some(name.to_owned());
                self.write(&state)?;
            }
        }
        Ok(state)
    }

    /// Removes the state file.
    fn remove(&self) -> Result<()> {
        self.require_exclusive()?;
        // Temporaries first, so a cleanup failure never leaves the state
        // removed without the directory synced.
        remove_stale_temps(&self.dir, &key(&self.folder))?;
        if files::remove_if_present(&self.state)? {
            sync_dir(&self.dir)?;
        }
        Ok(())
    }

    fn require_exclusive(&self) -> Result<()> {
        if !self.exclusive {
            bail!("GOG folder state changed under a shared lock");
        }
        Ok(())
    }
}

impl Drop for FolderLock {
    fn drop(&mut self) {
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

/// Opens and flocks `path`, polling so `cancel` still works.
fn lock_file(path: &Path, exclusive: bool, cancel: &dyn Fn() -> bool) -> Result<File> {
    let dir = path.parent().expect("a lock has a parent");
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let operation = if exclusive {
        FlockOperation::NonBlockingLockExclusive
    } else {
        FlockOperation::NonBlockingLockShared
    };
    loop {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        loop {
            match flock(&file, operation) {
                Ok(()) => break,
                Err(Errno::WOULDBLOCK) => {
                    if cancel() {
                        return Err(FolderError::Canceled.into());
                    }
                    thread::sleep(LOCK_RETRY);
                }
                Err(Errno::INTR) => {}
                Err(error) => {
                    return Err(io::Error::from(error))
                        .with_context(|| format!("failed to lock {}", path.display()));
                }
            }
        }
        // wineo never deletes lock files, but a hand cleanup might, which
        // would leave this lock on a file nobody else sees.
        let held = file.metadata()?;
        match fs::metadata(path) {
            Ok(now) if now.dev() == held.dev() && now.ino() == held.ino() => return Ok(file),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        drop(file);
    }
}

/// Held by a migration or a fresh install from before it checks for
/// overlapping folders until it has written the state of the folder it
/// adopts or creates, so two of them in nested folders can't both pass.
/// Only they take it, and before any other lock.
pub struct AdoptionLock {
    file: File,
}

impl AdoptionLock {
    /// Waits for the lock, polling so `cancel` still works.
    pub fn acquire(paths: &Paths, cancel: &dyn Fn() -> bool) -> Result<Self> {
        let file = lock_file(&paths.gog_adoption_lock(), true, cancel)?;
        Ok(Self { file })
    }
}

impl Drop for AdoptionLock {
    fn drop(&mut self) {
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

fn read_file(path: &Path) -> Result<Option<FolderState>> {
    match fs::read_to_string(path) {
        Ok(text) => FolderState::parse(&text).map(Some).map_err(|error| {
            anyhow!(
                "{} isn't a valid GOG folder state ({error:#})",
                path.display()
            )
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Every state file, read without locks, so the answer can be stale.
/// Unreadable files are returned as errors rather than skipped.
pub fn list(paths: &Paths) -> Result<Vec<(PathBuf, Result<FolderState>)>> {
    let dir = paths.gog_folders_dir();
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
        if name.starts_with('.') || !name.ends_with(".toml") {
            continue;
        }
        let state = read_file(&path).and_then(|state| {
            let state = state.ok_or_else(|| anyhow!("{} disappeared", path.display()))?;
            if path.file_stem().and_then(|stem| stem.to_str()) != Some(&key(&state.folder)) {
                bail!(
                    "{} names {}, which isn't its key",
                    path.display(),
                    state.folder.display()
                );
            }
            Ok(state)
        });
        found.push((path, state));
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(found)
}

fn readable(paths: &Paths) -> Result<Vec<FolderState>> {
    list(paths)?.into_iter().map(|(_, state)| state).collect()
}

/// A folder's state read without its lock, so the answer can be stale. For
/// planning, which mustn't leave lock files behind.
pub fn peek(paths: &Paths, folder: &Path) -> Result<Option<FolderState>> {
    check_folder(folder)?;
    let Some(state) = read_file(&state_path(paths, folder))? else {
        return Ok(None);
    };
    if state.folder != folder {
        bail!(
            "{} names {}, not {}",
            state_path(paths, folder).display(),
            state.folder.display(),
            folder.display()
        );
    }
    Ok(Some(state))
}

/// Every folder whose state names `name` as owner, whatever its game.
/// With `strict`, an unreadable state file fails, since it could be one of
/// them. Without, it's skipped, for an environment that tracks no game.
pub fn owned_by(paths: &Paths, name: &str, strict: bool) -> Result<Vec<FolderState>> {
    let mut owned = Vec::new();
    for (_, state) in list(paths)? {
        match state {
            Ok(state) if state.owner.as_deref() == Some(name) => owned.push(state),
            Ok(_) => {}
            Err(error) if strict => {
                return Err(FolderError::Unreadable {
                    name: name.to_owned(),
                    detail: format!("{error:#}"),
                }
                .into());
            }
            Err(_) => {}
        }
    }
    Ok(owned)
}

/// Whether `name`'s config could name a GOG game, so its folder states have
/// to be readable. Only a config that reads as TOML without `gog_id` can't.
pub fn may_track_game(paths: &Paths, name: &str) -> bool {
    match fs::read_to_string(paths.environment_config(name)) {
        Ok(text) => text
            .parse::<toml_edit::DocumentMut>()
            .map_or(true, |doc| doc.get("gog_id").is_some()),
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

/// An environment's game: the folder whose state names it as owner and is
/// the game `gog_id`.
pub fn find(paths: &Paths, name: &str, gog_id: &str) -> Result<Option<FolderState>> {
    let mut found = owned_by(paths, name, true)?
        .into_iter()
        .filter(|state| state.is_game(gog_id));
    let first = found.next();
    if let Some(other) = found.next() {
        bail!(
            "{name} owns more than one folder for GOG game {gog_id}: {} and {}",
            first.expect("checked").folder.display(),
            other.folder.display()
        );
    }
    Ok(first)
}

/// Refuses a folder that is inside, or around, another state's folder.
pub fn check_overlap(paths: &Paths, folder: &Path) -> Result<()> {
    for state in readable(paths)? {
        if state.folder != folder
            && (state.folder.starts_with(folder) || folder.starts_with(&state.folder))
        {
            return Err(FolderError::Overlap {
                folder: folder.to_owned(),
                other: state.label(),
                other_folder: state.folder,
            }
            .into());
        }
    }
    Ok(())
}

/// The canonical form of a folder that may not exist yet: its deepest
/// existing ancestor resolved, with the missing names after it. That is what
/// `canonical` gives once the folder is created, so state files and the
/// overlap check agree with it.
fn canonical_new(folder: &Path) -> Result<PathBuf> {
    if !folder.is_absolute() {
        bail!("{} isn't an absolute path.", folder.display());
    }
    let mut missing = Vec::new();
    let mut existing = folder;
    // A dangling symlink exists as far as this is concerned, and fails to
    // resolve, rather than being walked past.
    let mut base = loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => break canonical(existing)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let (Some(name), Some(parent)) = (existing.file_name(), existing.parent()) else {
                    bail!("{} isn't a usable folder path.", folder.display());
                };
                missing.push(name);
                existing = parent;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {}", existing.display()));
            }
        }
    };
    base.extend(missing.iter().rev());
    Ok(base)
}

/// Checks a fresh install of environment `name` into `folder`, Windows or
/// native, and returns the folder in canonical form. It's refused when the
/// folder overlaps another game's, or exists and isn't empty, unless it is
/// the folder of `name`'s own unfinished install. Read without locks, so it
/// is for planning; the install rechecks under the folder's lock.
pub fn check_fresh_install(paths: &Paths, folder: &Path, name: &str) -> Result<PathBuf> {
    let folder = canonical_new(folder)?;
    check_overlap(paths, &folder)?;
    let empty = match fs::read_dir(&folder) {
        Ok(mut entries) => entries.next().is_none(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        // A file, or a folder wineo can't list, isn't one to install into.
        Err(_) => false,
    };
    if empty {
        return Ok(folder);
    }
    let own_install = peek(paths, &folder)?.is_some_and(|state| {
        state.owner.as_deref() == Some(name)
            && state
                .journal
                .is_some_and(|journal| journal.kind == JournalKind::Install)
    });
    if own_install {
        Ok(folder)
    } else {
        Err(FolderError::NotEmpty { folder }.into())
    }
}

/// Why a launch from a folder with a journal is refused: finish it with
/// `wineo update`, or, for an install whose environment has no config yet
/// or no owner, install again.
pub fn unfinished(paths: &Paths, state: &FolderState) -> FolderError {
    let gog_id = state.gog_id().unwrap_or_default();
    let install = state
        .journal
        .as_ref()
        .is_some_and(|journal| journal.kind == JournalKind::Install);
    match &state.owner {
        Some(owner) if install && !owner_exists(paths, owner).unwrap_or(true) => {
            FolderError::UnfinishedInstall {
                name: owner.clone(),
                gog_id,
            }
        }
        Some(owner) => FolderError::MidUpdate {
            name: owner.clone(),
        },
        None if install => FolderError::OwnerlessInstall {
            gog_id,
            folder: state.folder.clone(),
        },
        None => FolderError::OwnerlessMidUpdate { gog_id },
    }
}

/// Whether `state` is an install that stopped before its environment's
/// config was written, which installing again finishes.
pub fn install_without_config(paths: &Paths, state: &FolderState) -> bool {
    state
        .journal
        .as_ref()
        .is_some_and(|journal| journal.kind == JournalKind::Install)
        && state
            .owner
            .as_deref()
            .is_some_and(|owner| !owner_exists(paths, owner).unwrap_or(true))
}

/// Sets `owner` to `name` on an existing state, under its lock.
pub fn reclaim(
    paths: &Paths,
    folder: &Path,
    name: &str,
    cancel: &dyn Fn() -> bool,
) -> Result<FolderState> {
    FolderLock::exclusive(paths, folder, cancel)?.reclaim(name)
}

/// Sets `owner` to none on an existing state, under its lock.
pub fn release(paths: &Paths, folder: &Path, cancel: &dyn Fn() -> bool) -> Result<()> {
    FolderLock::exclusive(paths, folder, cancel)?.release()
}

/// Removes a folder's state on purpose. Refused while an environment owns
/// it, except an install that stopped before its environment's config was
/// written, which nothing else can abandon; that one is refused while
/// anything runs from the folder. Returns whether there was one. The
/// folder's files stay either way.
pub fn forget(paths: &Paths, folder: &Path, cancel: &dyn Fn() -> bool) -> Result<bool> {
    // The in-use lock comes before the state lock, and a running install
    // holds both, so it is tried first without waiting.
    let in_use = match peek(paths, folder)? {
        Some(state) if configless_install(paths, &state) => {
            let owner = state.owner.unwrap_or_default();
            let lock = InUseLock::new(paths, folder)?;
            Some(lock.try_exclusive()?.ok_or_else(|| FolderError::InUse {
                name: owner,
                holders: lock.holders(),
                action: InUseAction::Forget,
            })?)
        }
        _ => None,
    };
    let lock = FolderLock::exclusive(paths, folder, cancel)?;
    let Some(state) = lock.read()? else {
        return Ok(false);
    };
    if (in_use.is_none() || !configless_install(paths, &state))
        && let Some(owner) = state.owner
    {
        return Err(FolderError::OwnedByOther {
            folder: folder.to_owned(),
            owner,
        }
        .into());
    }
    lock.remove()?;
    Ok(true)
}

/// Whether `state` is a first install that stopped before its owner's
/// config was written.
fn configless_install(paths: &Paths, state: &FolderState) -> bool {
    state.committed.is_none()
        && state
            .journal
            .as_ref()
            .is_some_and(|journal| journal.kind == JournalKind::Install)
        && state
            .owner
            .as_deref()
            .is_some_and(|owner| !paths.environment_config(owner).exists())
}

/// The device of `path`, None when it doesn't exist, or an error when that
/// can't be told, like a permission problem.
pub type DeviceOf<'a> = &'a dyn Fn(&Path) -> io::Result<Option<u64>>;

/// The real device lookup.
pub fn device_of(path: &Path) -> io::Result<Option<u64>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta.dev())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether a missing folder was really deleted: its nearest existing parent
/// is on the filesystem the state recorded. An unmounted drive leaves the
/// parent on another one, so its state stays, as does anything that can't
/// be looked at.
pub fn surely_deleted(state: &FolderState, device_of: DeviceOf<'_>) -> bool {
    let Some(recorded) = state.device else {
        return false;
    };
    if !matches!(device_of(&state.folder), Ok(None)) {
        return false;
    }
    let mut current = state.folder.parent();
    while let Some(dir) = current {
        match device_of(dir) {
            Ok(Some(device)) => return device == recorded,
            Ok(None) => current = dir.parent(),
            Err(_) => return false,
        }
    }
    false
}

/// Removes a state that is safe to drop: no journal, no owner, and its
/// folder surely deleted. Checked again under the lock. Returns whether it
/// was removed.
pub fn remove_if_deleted(
    paths: &Paths,
    folder: &Path,
    device_of: DeviceOf<'_>,
    cancel: &dyn Fn() -> bool,
) -> Result<bool> {
    let lock = FolderLock::exclusive(paths, folder, cancel)?;
    let Some(state) = lock.read()? else {
        return Ok(false);
    };
    if state.journal.is_some() || state.owner.is_some() || !surely_deleted(&state, device_of) {
        return Ok(false);
    }
    lock.remove()?;
    Ok(true)
}

/// Sets the owner to none when it names an environment with no config,
/// keeping any journal. Host root only. Returns whether it changed.
pub fn release_missing_owner(
    paths: &Paths,
    folder: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<bool> {
    if paths.portable_root().is_some() {
        bail!("GOG games live in the host library. Run this without --portable.");
    }
    let lock = FolderLock::exclusive(paths, folder, cancel)?;
    let Some(state) = lock.read()? else {
        return Ok(false);
    };
    // Released, an unfinished install could no longer be finished by
    // installing again under its name.
    if install_without_config(paths, &state) {
        return Ok(false);
    }
    match &state.owner {
        Some(owner) if !owner_exists(paths, owner)? => {
            lock.release()?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn owner_exists(paths: &Paths, owner: &str) -> Result<bool> {
    let config = paths.environment_config(owner);
    match fs::symlink_metadata(&config) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("failed to inspect {}", config.display())),
    }
}

/// Locks every folder `name` owns, exclusively and in key order, and
/// returns the locks with each one's state. Refuses when the owned set
/// changes while locking.
pub fn lock_owned(
    paths: &Paths,
    name: &str,
    strict: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<(FolderLock, FolderState)>> {
    let mut folders: Vec<PathBuf> = owned_by(paths, name, strict)?
        .into_iter()
        .map(|state| state.folder)
        .collect();
    folders.sort_by_key(|folder| key(folder));
    let mut held = Vec::new();
    for folder in &folders {
        let lock = FolderLock::exclusive(paths, folder, cancel)?;
        match lock.read()? {
            Some(state) if state.owner.as_deref() == Some(name) => held.push((lock, state)),
            _ => bail!("{name}'s GOG folders changed while locking; retry"),
        }
    }
    if owned_by(paths, name, strict)?.len() != held.len() {
        bail!("{name}'s GOG folders changed while locking; retry");
    }
    Ok(held)
}

/// Writes `text` to `path` through a synced temporary file, then syncs the
/// directory, so a crash leaves either the old file or the new one.
fn write_durable(dir: &Path, path: &Path, key: &str, text: &str) -> Result<()> {
    create_dir_durable(dir)?;
    remove_stale_temps(dir, key)?;
    let mut temp = tempfile::Builder::new()
        .prefix(&temp_prefix(key))
        .tempfile_in(dir)
        .with_context(|| format!("failed to write in {}", dir.display()))?;
    temp.write_all(text.as_bytes())?;
    temp.as_file().sync_all()?;
    failpoint("write.before_rename")?;
    temp.persist(path).map_err(|error| error.error)?;
    sync_dir(dir)
}

fn temp_prefix(key: &str) -> String {
    format!(".state-{key}-")
}

/// Removes temporary files an interrupted write of this folder's state left.
/// Only called under the folder's exclusive lock, so none is in use.
fn remove_stale_temps(dir: &Path, key: &str) -> Result<()> {
    let prefix = temp_prefix(key);
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
            .is_some_and(|name| name.starts_with(&prefix))
        {
            fs::remove_file(entry.path())
                .with_context(|| format!("failed to remove {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Creates `dir` and any missing parents, syncing each new entry's parent.
pub(crate) fn create_dir_durable(dir: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut current = dir;
    while !current.exists() {
        missing.push(current.to_owned());
        current = current
            .parent()
            .ok_or_else(|| anyhow!("{} has no parent", dir.display()))?;
    }
    for path in missing.into_iter().rev() {
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("failed to create {}", path.display()));
            }
        }
        sync_dir(path.parent().expect("created under a parent"))?;
    }
    Ok(())
}

fn sync_dir(dir: &Path) -> Result<()> {
    files::sync_dir(dir).with_context(|| format!("failed to sync {}", dir.display()))
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// The armed point, and how many times it passes before it fails.
    static FAILPOINT: std::cell::Cell<Option<(&'static str, usize)>> =
        const { std::cell::Cell::new(None) };
}

/// Makes the next write on this thread fail at `at`, once.
#[cfg(any(test, feature = "test-support"))]
pub fn set_failpoint(at: Option<&'static str>) {
    FAILPOINT.with(|point| point.set(at.map(|at| (at, 0))));
}

/// Makes the write on this thread fail at `at` after passing it `skip`
/// times, once, so a test can stop between any two writes of a loop.
#[cfg(any(test, feature = "test-support"))]
pub fn set_failpoint_nth(at: &'static str, skip: usize) {
    FAILPOINT.with(|point| point.set(Some((at, skip))));
}

/// Whether a failpoint is still waiting to fire on this thread.
#[cfg(any(test, feature = "test-support"))]
pub fn failpoint_armed() -> bool {
    FAILPOINT.with(|point| point.get().is_some())
}

pub(crate) fn failpoint(_at: &str) -> Result<()> {
    #[cfg(any(test, feature = "test-support"))]
    FAILPOINT.with(|point| {
        match point.get() {
            Some((at, 0)) if at == _at => {
                point.set(None);
                bail!("injected GOG folder interruption at {_at}");
            }
            Some((at, skip)) if at == _at => point.set(Some((at, skip - 1))),
            _ => {}
        }
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests;
