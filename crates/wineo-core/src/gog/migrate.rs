//! Moving the GOG games Heroic installed into wineo.
//!
//! `plan` reads Heroic's records and the game folder and works out what
//! migrating one game does, without writing anything or taking locks:
//! attach it to the environment that already runs it, or create one from
//! Heroic's settings through the Heroic import plans, and adopt or reclaim
//! the folder's state. A frontend may edit a new environment's plan, then
//! `apply` does it.
//!
//! Lock order: the adoption lock, the environment's operation lock, the
//! folder's in-use lock, its state lock, then the config lock.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::folder::{
    self, AdoptionLock, Committed, FolderLock, FolderState, Platform, Product, ProductStatus, Track,
};
use crate::cancel::Token;
use crate::config::{Arch, GlobalConfig};
use crate::environment::{self, Environment};
use crate::heroic::{HeroicGogGame, HeroicImport, HeroicSource, InputFingerprint};
use crate::heroic_import::{self, GamePlan, NativePlan, Options, Registered, default_name};
use crate::import::ImportProgress;
use crate::launch;
use crate::operation::{Locks, Operation};
use crate::paths::{self, Paths};
use crate::runtime::Runtime;

/// Refusals a frontend may want to tell apart. Folder refusals (owned by
/// another environment, overlapping another game, something running from
/// the folder) are `FolderError`s.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error("GOG games live in the host library. Run this without --portable.")]
    Portable,
    #[error("{name} is running. Stop it before migrating.")]
    Running { name: String },
    #[error("{0} doesn't exist.")]
    MissingFolder(PathBuf),
    #[error("{folder} has no {file}.")]
    MissingInfo { folder: PathBuf, file: String },
    #[error("{folder} holds GOG game {found}, not {expected}.")]
    IdMismatch {
        folder: PathBuf,
        expected: String,
        found: String,
    },
    #[error("{info} names no game to launch.")]
    NoPlayTask { info: PathBuf },
    #[error("{info} has a play task wineo can't use: {detail}.")]
    BadPlayTask { info: PathBuf, detail: String },
    #[error("GOG game {gog_id} matches more than one environment: {}.", names.join(", "))]
    Ambiguous { gog_id: String, names: Vec<String> },
    #[error("{name} runs GOG game {other}, not {gog_id}.")]
    OtherGame {
        name: String,
        gog_id: String,
        other: String,
    },
    #[error("{name} already has GOG game {gog_id} in {other_folder}, not {folder}.")]
    OtherFolder {
        name: String,
        gog_id: String,
        folder: PathBuf,
        other_folder: PathBuf,
    },
    #[error("{folder} holds the state of GOG game {other}, not {gog_id}.")]
    StateOtherGame {
        folder: PathBuf,
        gog_id: String,
        other: String,
    },
    /// A new Windows environment has no runner chosen yet.
    #[error("a runner is needed to import Windows games")]
    NeedsRunner,
    /// A new Windows environment's plan copies the prefix.
    #[error("A migrated game keeps Heroic's prefix where it is.")]
    PrefixCopied,
    #[error(
        "Heroic's game or wineo's environments changed since the migration was planned; plan it again"
    )]
    Changed,
}

/// What migrating one game would do.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Plan {
    /// Heroic hasn't installed it, so there's nothing to migrate. Installing
    /// comes in a later release.
    NotInstalled {
        gog_id: String,
    },
    /// `environment` already owns the game's folder.
    AlreadyMigrated {
        environment: String,
        folder: PathBuf,
    },
    Migrate(Migration),
}

/// A game to migrate and everything decided about it.
#[derive(Debug, Clone)]
pub struct Migration {
    game: HeroicGogGame,
    gog_id: u64,
    folder: PathBuf,
    launch: Launch,
    /// Heroic's prefix for a Windows game, when its settings name one.
    prefix: Option<PathBuf>,
    committed: Committed,
    state: StateChange,
    fingerprint: InputFingerprint,
    target: Target,
}

/// What happens to the folder's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateChange {
    /// The folder has no state yet, so one is written from Heroic's record.
    Adopt,
    /// The folder has an ownerless state, which is claimed as it is,
    /// including any journal, so an interrupted update stays blocked until
    /// it's finished.
    Reclaim { journal: bool },
    /// The state already names the environment.
    Keep,
}

/// The environment the game ends up in.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Target {
    /// An environment that already runs the game. It keeps every setting;
    /// only `gog_id` is added when it has none.
    Existing { name: String, records_gog_id: bool },
    /// A new Windows environment. `plan` is None until a runner is chosen
    /// with `Migration::plan_runner`.
    Windows {
        imported: HeroicImport,
        plan: Option<GamePlan>,
    },
    /// A new native environment.
    Native(NativePlan),
}

/// How the game folder says the game starts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Launch {
    /// The file that said it, `goggame-<id>.info` or `gameinfo`.
    info: PathBuf,
    exe: PathBuf,
    cwd: Option<PathBuf>,
    args: Vec<String>,
}

/// What `apply` did.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Outcome {
    /// Nothing was written.
    AlreadyMigrated {
        environment: String,
        folder: PathBuf,
    },
    Migrated(Migrated),
}

#[derive(Debug)]
pub struct Migrated {
    pub environment: String,
    pub folder: PathBuf,
    pub state: StateChange,
    /// Whether `gog_id` was written into an existing environment's config.
    pub recorded_gog_id: bool,
    /// A new environment, with its desktop entries still to write through
    /// `Registered::finish`.
    pub registered: Option<Registered>,
}

impl Migration {
    pub fn game(&self) -> &HeroicGogGame {
        &self.game
    }

    /// The game folder, canonical.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// What a first adoption records.
    pub fn committed(&self) -> &Committed {
        &self.committed
    }

    /// What happens to the folder's state, as planned. `apply` decides
    /// again under the folder's lock.
    pub fn state(&self) -> StateChange {
        self.state
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The environment's name, None for a Windows game with no runner chosen.
    pub fn environment(&self) -> Option<&str> {
        match &self.target {
            Target::Existing { name, .. } => Some(name),
            Target::Windows { plan, .. } => plan.as_ref().map(GamePlan::name),
            Target::Native(plan) => Some(plan.name()),
        }
    }

    /// Whether a new environment's name is already taken, which `apply`
    /// refuses like a Heroic import does.
    pub fn name_taken(&self, paths: &Paths) -> bool {
        match &self.target {
            Target::Existing { .. } => false,
            _ => self
                .environment()
                .is_some_and(|name| paths.environment_config(name).exists()),
        }
    }

    /// Plans a new Windows environment with `runner`, adopting Heroic's
    /// prefix where it is. `name` defaults to one suggested from the title.
    pub fn plan_runner(
        &mut self,
        paths: &Paths,
        global: &GlobalConfig,
        runner: &str,
        name: Option<String>,
        arch: Option<Arch>,
    ) -> Result<()> {
        let id = self.game.id.clone();
        let Target::Windows { imported, plan } = &mut self.target else {
            anyhow::bail!("only a new Windows environment needs a runner");
        };
        let mut planned = GamePlan::new(
            paths,
            global,
            imported.clone(),
            Vec::new(),
            runner,
            Options {
                name,
                adopt: true,
                destination: None,
                arch,
            },
        )?;
        planned.set_gog_id(&id);
        *plan = Some(planned);
        Ok(())
    }

    /// The new Windows environment's plan, to edit before applying.
    pub fn game_plan_mut(&mut self) -> Option<&mut GamePlan> {
        match &mut self.target {
            Target::Windows { plan, .. } => plan.as_mut(),
            _ => None,
        }
    }

    /// The new native environment's plan, to edit before applying.
    pub fn native_plan_mut(&mut self) -> Option<&mut NativePlan> {
        match &mut self.target {
            Target::Native(plan) => Some(plan),
            _ => None,
        }
    }
}

/// Works out what migrating the GOG game `gog_id` from `source` does. Reads
/// only, and takes no locks.
pub fn plan(paths: &Paths, source: &HeroicSource, gog_id: &str) -> Result<Plan> {
    host_only(paths)?;
    let Some(game) = source.gog_game(gog_id)? else {
        return Ok(Plan::NotInstalled {
            gog_id: gog_id.to_owned(),
        });
    };
    let id: u64 = game
        .id
        .parse()
        .with_context(|| format!("{} isn't a GOG game id", game.id))?;
    let folder = game_folder(&game)?;
    let launch = read_launch(&folder, &game)?;
    let committed = committed(&game, id);

    let (windows, native) = match game.platform {
        Platform::Windows => (
            Some(
                source
                    .inspect_gog(&game, &launch.exe, &launch.info)
                    .map(|mut imported| {
                        with_task(&mut imported, &launch);
                        imported
                    }),
            ),
            None,
        ),
        Platform::Linux => (
            None,
            Some(source.inspect_gog_native(&game, &launch.exe, &launch.info)),
        ),
    };
    let prefix = windows
        .as_ref()
        .and_then(|imported| imported.as_ref().ok())
        .map(|imported| imported.prefix.clone());
    let fingerprint = source.gog_fingerprint(&game, &launch.info)?;

    let found = match_environment(paths, &game, &launch.exe, prefix.as_deref())?;
    let target = match found {
        Found::Existing { name, gog_id: has } => {
            if has.is_some()
                && let Some(state) = already(paths, &name, &game, &folder)?
            {
                return Ok(Plan::AlreadyMigrated {
                    environment: name,
                    folder: state.folder,
                });
            }
            if Runtime::new(paths, &name).is_running()? {
                return Err(MigrateError::Running { name }.into());
            }
            Target::Existing {
                name,
                records_gog_id: has.is_none(),
            }
        }
        Found::New => match (windows, native) {
            (Some(imported), _) => Target::Windows {
                imported: imported?,
                plan: None,
            },
            (_, Some(imported)) => {
                let imported = imported?;
                let name = default_name(&imported.title, Some(&imported.exe));
                let mut plan = NativePlan::new(imported, name);
                plan.set_gog_id(&game.id);
                Target::Native(plan)
            }
            (None, None) => unreachable!("every game is Windows or native"),
        },
    };

    // A Windows game with no runner yet has no name, and only an owner of
    // that name could make a difference here.
    let owner = match &target {
        Target::Existing { name, .. } => name.clone(),
        Target::Windows { imported, .. } => default_name(&imported.title, Some(&imported.exe)),
        Target::Native(plan) => plan.name().to_owned(),
    };
    let state = decide(folder::peek(paths, &folder)?.as_ref(), &folder, &owner, id)?;
    if state == StateChange::Adopt {
        folder::check_overlap(paths, &folder)?;
    }
    Ok(Plan::Migrate(Migration {
        game,
        gog_id: id,
        folder,
        launch,
        prefix,
        committed,
        state,
        fingerprint,
        target,
    }))
}

/// Migrates the planned game: records `gog_id` on the environment, or
/// creates it with `gog_id`, then writes or reclaims the folder's state.
/// Everything is checked again under the locks first.
pub fn apply(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    migration: &Migration,
    token: &Token,
    progress: impl FnMut(ImportProgress),
) -> Result<Outcome> {
    host_only(paths)?;
    let cancel = || token.is_canceled();
    let _adoption = AdoptionLock::acquire(paths, &cancel)?;
    if !source.unchanged(&migration.game.id, &migration.fingerprint)? {
        return Err(MigrateError::Changed.into());
    }
    let found = migration.rematch(paths)?;
    match (&migration.target, found) {
        (Target::Existing { name, .. }, Found::Existing { name: found, .. }) if *name == found => {
            attach(paths, migration, name, &cancel)
        }
        (Target::Windows { .. } | Target::Native(_), Found::New) => {
            create(paths, global, source, migration, token, progress, &cancel)
        }
        _ => Err(MigrateError::Changed.into()),
    }
}

impl Migration {
    fn rematch(&self, paths: &Paths) -> Result<Found> {
        match_environment(paths, &self.game, &self.launch.exe, self.prefix.as_deref())
    }

    /// The state a first adoption writes.
    fn adopted(&self, owner: &str) -> Result<FolderState> {
        let mut state = FolderState::new(self.folder.clone(), Some(owner.to_owned()));
        state.device = Some(
            fs::metadata(&self.folder)
                .with_context(|| format!("failed to inspect {}", self.folder.display()))?
                .dev(),
        );
        state.committed = Some(self.committed.clone());
        Ok(state)
    }
}

/// Attaches the game to an existing environment.
fn attach(
    paths: &Paths,
    migration: &Migration,
    name: &str,
    cancel: &dyn Fn() -> bool,
) -> Result<Outcome> {
    let game = &migration.game;
    let _operation = Locks::new(paths).acquire(Operation::Migrate, name, &[])?;
    // Again under the operation lock, so the config can't change and no
    // launch can start from here on.
    let has_gog_id = match migration.rematch(paths)? {
        Found::Existing {
            name: found,
            gog_id,
        } if found == name => gog_id.is_some(),
        _ => return Err(MigrateError::Changed.into()),
    };
    if has_gog_id && let Some(state) = already(paths, name, game, &migration.folder)? {
        return Ok(Outcome::AlreadyMigrated {
            environment: name.to_owned(),
            folder: state.folder,
        });
    }
    if Runtime::new(paths, name).is_running()? {
        return Err(MigrateError::Running {
            name: name.to_owned(),
        }
        .into());
    }
    let _in_use = folder::lock_for_update(paths, &migration.folder, name)?;
    let lock = FolderLock::exclusive(paths, &migration.folder, cancel)?;
    let state = decide(
        lock.read()?.as_ref(),
        &migration.folder,
        name,
        migration.gog_id,
    )?;
    if state == StateChange::Adopt {
        folder::check_overlap(paths, &migration.folder)?;
    }
    // The config first: a crash before the state is written leaves an
    // environment whose state is lost, which refuses to launch until it's
    // migrated again, never a state naming an owner that doesn't know it.
    let recorded_gog_id = Environment::record_gog_id(paths, name, &game.id)?;
    write_state(&lock, migration, state, name)?;
    Ok(Outcome::Migrated(Migrated {
        environment: name.to_owned(),
        folder: migration.folder.clone(),
        state,
        recorded_gog_id,
        registered: None,
    }))
}

/// Creates a new environment for the game, with `gog_id`, then writes its
/// folder's state.
fn create(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    migration: &Migration,
    token: &Token,
    progress: impl FnMut(ImportProgress),
    cancel: &dyn Fn() -> bool,
) -> Result<Outcome> {
    let name = migration
        .environment()
        .ok_or(MigrateError::NeedsRunner)?
        .to_owned();
    if let Target::Windows {
        plan: Some(plan), ..
    } = &migration.target
        && !plan.is_adopt()
    {
        return Err(MigrateError::PrefixCopied.into());
    }

    // Checked before the environment exists, so a refusal leaves nothing
    // behind. Creating takes the new environment's operation lock, which
    // comes before the folder's, so the folder locks are let go meanwhile;
    // the adoption lock keeps other migrations out until the state is
    // written.
    {
        let _in_use = folder::lock_for_update(paths, &migration.folder, &name)?;
        let state = folder::peek(paths, &migration.folder)?;
        if decide(state.as_ref(), &migration.folder, &name, migration.gog_id)? == StateChange::Adopt
        {
            folder::check_overlap(paths, &migration.folder)?;
        }
    }

    let registered = match &migration.target {
        Target::Windows {
            plan: Some(plan), ..
        } => heroic_import::apply(paths, global, source, plan, token, progress)?,
        Target::Native(plan) => heroic_import::apply_native(paths, source, plan)?,
        _ => return Err(MigrateError::NeedsRunner.into()),
    };

    let _operation = Locks::new(paths).acquire(Operation::Migrate, &name, &[])?;
    let _in_use = folder::lock_for_update(paths, &migration.folder, &name)?;
    let lock = FolderLock::exclusive(paths, &migration.folder, cancel)?;
    let state = decide(
        lock.read()?.as_ref(),
        &migration.folder,
        &name,
        migration.gog_id,
    )?;
    if state == StateChange::Adopt {
        folder::check_overlap(paths, &migration.folder)?;
    }
    write_state(&lock, migration, state, &name)?;
    Ok(Outcome::Migrated(Migrated {
        environment: name,
        folder: migration.folder.clone(),
        state,
        recorded_gog_id: false,
        registered: Some(registered),
    }))
}

fn write_state(
    lock: &FolderLock,
    migration: &Migration,
    state: StateChange,
    owner: &str,
) -> Result<()> {
    match state {
        StateChange::Adopt => lock.write(&migration.adopted(owner)?),
        StateChange::Reclaim { .. } => lock.reclaim(owner).map(drop),
        StateChange::Keep => Ok(()),
    }
}

fn host_only(paths: &Paths) -> Result<()> {
    if paths.portable_root().is_some() {
        return Err(MigrateError::Portable.into());
    }
    Ok(())
}

/// What the folder's current state allows, for `owner`.
fn decide(
    state: Option<&FolderState>,
    folder: &Path,
    owner: &str,
    gog_id: u64,
) -> Result<StateChange> {
    let Some(state) = state else {
        return Ok(StateChange::Adopt);
    };
    if state.gog_id() != Some(gog_id) {
        return Err(MigrateError::StateOtherGame {
            folder: folder.to_owned(),
            gog_id: gog_id.to_string(),
            other: state
                .gog_id()
                .map_or_else(|| "none".to_owned(), |id| id.to_string()),
        }
        .into());
    }
    match &state.owner {
        Some(current) if current == owner => Ok(StateChange::Keep),
        Some(current) => Err(folder::FolderError::OwnedByOther {
            folder: folder.to_owned(),
            owner: current.clone(),
        }
        .into()),
        None => Ok(StateChange::Reclaim {
            journal: state.journal.is_some(),
        }),
    }
}

/// The folder `name` already owns for the game. One that isn't Heroic's
/// folder is refused, since adopting Heroic's too would give it two.
fn already(
    paths: &Paths,
    name: &str,
    game: &HeroicGogGame,
    folder: &Path,
) -> Result<Option<FolderState>> {
    let Some(state) = folder::find(paths, name, &game.id)? else {
        return Ok(None);
    };
    if state.folder != folder {
        return Err(MigrateError::OtherFolder {
            name: name.to_owned(),
            gog_id: game.id.clone(),
            folder: folder.to_owned(),
            other_folder: state.folder,
        }
        .into());
    }
    Ok(Some(state))
}

/// Heroic's install folder, canonical. It has to exist.
fn game_folder(game: &HeroicGogGame) -> Result<PathBuf> {
    match fs::metadata(&game.folder) {
        Ok(meta) if meta.is_dir() && game.folder.is_absolute() => folder::canonical(&game.folder),
        Ok(_) => Err(MigrateError::MissingFolder(game.folder.clone()).into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(MigrateError::MissingFolder(game.folder.clone()).into())
        }
        Err(error) => {
            Err(error).with_context(|| format!("failed to inspect {}", game.folder.display()))
        }
    }
}

/// What a first adoption records, from Heroic's record.
fn committed(game: &HeroicGogGame, gog_id: u64) -> Committed {
    let track = match (game.platform, &game.build_id) {
        (Platform::Windows, Some(_)) => Track::Galaxy,
        _ => Track::Installer,
    };
    let base = Product {
        id: gog_id,
        version: (track == Track::Installer && !game.version.is_empty())
            .then(|| game.version.clone()),
        build: (track == Track::Galaxy)
            .then(|| game.build_id.clone())
            .flatten(),
        status: ProductStatus::Installed,
    };
    let dlc = game
        .dlc
        .iter()
        .filter(|id| **id != gog_id)
        .map(|id| Product {
            id: *id,
            version: None,
            build: None,
            status: ProductStatus::Installed,
        });
    Committed {
        gog_id,
        platform: game.platform,
        language: game.language.clone(),
        track,
        products: std::iter::once(base).chain(dlc).collect(),
        files: Vec::new(),
    }
}

/// Checks the folder holds the game Heroic says, and reads how it starts.
fn read_launch(folder: &Path, game: &HeroicGogGame) -> Result<Launch> {
    match game.platform {
        Platform::Linux => {
            let info = folder.join("gameinfo");
            let text = match fs::read_to_string(&info) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(MigrateError::MissingInfo {
                        folder: folder.to_owned(),
                        file: "gameinfo".to_owned(),
                    }
                    .into());
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to read {}", info.display()));
                }
            };
            // The fifth line is the product id.
            let found = text.lines().nth(4).unwrap_or_default().trim();
            if found != game.id {
                return Err(mismatch(folder, game, found));
            }
            Ok(Launch {
                info,
                exe: folder.join("start.sh"),
                cwd: None,
                args: Vec::new(),
            })
        }
        Platform::Windows => {
            let file = format!("goggame-{}.info", game.id);
            let info = folder.join(&file);
            let text = match fs::read_to_string(&info) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(match other_info(folder) {
                        Some(found) => mismatch(folder, game, &found),
                        None => MigrateError::MissingInfo {
                            folder: folder.to_owned(),
                            file,
                        }
                        .into(),
                    });
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to read {}", info.display()));
                }
            };
            let root: Value = serde_json::from_str(&text)
                .with_context(|| format!("{} isn't valid JSON", info.display()))?;
            let found = root.get("gameId").map(id_text).unwrap_or_default();
            if found != game.id {
                return Err(mismatch(folder, game, &found));
            }
            windows_launch(folder, info, &root)
        }
    }
}

fn mismatch(folder: &Path, game: &HeroicGogGame, found: &str) -> anyhow::Error {
    MigrateError::IdMismatch {
        folder: folder.to_owned(),
        expected: game.id.clone(),
        found: if found.is_empty() {
            "none".to_owned()
        } else {
            found.to_owned()
        },
    }
    .into()
}

/// GOG writes ids as strings, but a number means the same.
fn id_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

/// The game id in another `goggame-*.info` in the folder, so a folder of
/// the wrong game says so instead of looking empty.
fn other_info(folder: &Path) -> Option<String> {
    let mut infos: Vec<PathBuf> = fs::read_dir(folder)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("goggame-") && name.ends_with(".info"))
        })
        .collect();
    infos.sort();
    let text = fs::read_to_string(infos.first()?).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    Some(root.get("gameId").map(id_text).unwrap_or_default())
}

/// The primary play task, or else the first file task, like Heroic.
fn windows_launch(folder: &Path, info: PathBuf, root: &Value) -> Result<Launch> {
    let tasks: Vec<&Value> = root
        .get("playTasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|task| task.get("type").and_then(Value::as_str) == Some("FileTask"))
        .collect();
    let task = tasks
        .iter()
        .find(|task| task.get("isPrimary").and_then(Value::as_bool) == Some(true))
        .or_else(|| tasks.first())
        .ok_or_else(|| MigrateError::NoPlayTask { info: info.clone() })?;
    let bad = |detail: String| MigrateError::BadPlayTask {
        info: info.clone(),
        detail,
    };
    let path = task
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| bad("it has no path".to_owned()))?;
    let exe = inside(folder, path).ok_or_else(|| bad(format!("{path} is outside the game")))?;
    let cwd = match task
        .get("workingDir")
        .and_then(Value::as_str)
        .filter(|dir| !dir.is_empty())
    {
        Some(dir) => {
            Some(inside(folder, dir).ok_or_else(|| bad(format!("{dir} is outside the game")))?)
        }
        None => None,
    };
    let args = match task
        .get("arguments")
        .and_then(Value::as_str)
        .filter(|args| !args.trim().is_empty())
    {
        Some(args) => crate::heroic_args::parse(args)
            .map_err(|error| bad(format!("its arguments can't be read ({error})")))?,
        None => Vec::new(),
    };
    Ok(Launch {
        info,
        exe,
        cwd,
        args,
    })
}

/// A play task path, which is relative to the folder and may use
/// backslashes, as a path inside it. None for one that leads out.
fn inside(folder: &Path, raw: &str) -> Option<PathBuf> {
    let normal = raw.replace('\\', "/");
    let relative = Path::new(&normal);
    let mut path = folder.to_owned();
    for part in relative.components() {
        match part {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (path != folder).then_some(path)
}

/// Adds the play task to what Heroic's settings give: its working
/// directory, and its arguments before Heroic's launch arguments. A custom
/// executable in Heroic only replaces the exe.
fn with_task(imported: &mut HeroicImport, launch: &Launch) {
    if launch.cwd.is_some() {
        imported.cwd = launch.cwd.clone();
    }
    imported.args = launch
        .args
        .iter()
        .cloned()
        .chain(std::mem::take(&mut imported.args))
        .collect();
}

/// The environment a game already runs in, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Found {
    Existing {
        name: String,
        gog_id: Option<String>,
    },
    New,
}

/// Finds the environment for the game: first one with its `gog_id`, then
/// one whose default target runs the game's exe, which for a Windows game
/// also has to use Heroic's prefix for it. More than one, or one for
/// another GOG game, is refused.
fn match_environment(
    paths: &Paths,
    game: &HeroicGogGame,
    exe: &Path,
    prefix: Option<&Path>,
) -> Result<Found> {
    let exe = paths::resolve(exe);
    let prefix = prefix.map(paths::resolve);
    let mut by_id = Vec::new();
    let mut by_paths = Vec::new();
    for name in environment::names(paths)? {
        let Ok(environment) = Environment::load(paths, &name) else {
            continue;
        };
        if environment.config.gog_id.as_deref() == Some(game.id.as_str()) {
            by_id.push(name);
        } else if runs(paths, &environment, game.platform, &exe, prefix.as_deref()) {
            by_paths.push((name, environment.config.gog_id.clone()));
        }
    }
    let ambiguous = |names: Vec<String>| MigrateError::Ambiguous {
        gog_id: game.id.clone(),
        names,
    };
    match by_id.len() {
        0 => {}
        1 => {
            return Ok(Found::Existing {
                name: by_id.remove(0),
                gog_id: Some(game.id.clone()),
            });
        }
        _ => return Err(ambiguous(by_id).into()),
    }
    match by_paths.len() {
        0 => Ok(Found::New),
        1 => {
            let (name, gog_id) = by_paths.remove(0);
            match gog_id {
                Some(other) => Err(MigrateError::OtherGame {
                    name,
                    gog_id: game.id.clone(),
                    other,
                }
                .into()),
                None => Ok(Found::Existing { name, gog_id: None }),
            }
        }
        _ => Err(ambiguous(by_paths.into_iter().map(|(name, _)| name).collect()).into()),
    }
}

/// Whether `environment`'s default target runs `exe`, and for a Windows
/// game, in `prefix`.
fn runs(
    paths: &Paths,
    environment: &Environment,
    platform: Platform,
    exe: &Path,
    prefix: Option<&Path>,
) -> bool {
    let Ok((_, target)) = launch::resolve_target(environment, None) else {
        return false;
    };
    let runs_exe = paths
        .resolve_stored(&target.exe)
        .ok()
        .flatten()
        .is_some_and(|stored| paths::resolve(&stored) == exe);
    if !runs_exe {
        return false;
    }
    match platform {
        Platform::Linux => environment.config.is_native(),
        // Without Heroic's prefix nothing can be matched this way.
        Platform::Windows => prefix.is_some_and(|prefix| {
            !environment.config.is_native()
                && environment
                    .prefix(paths)
                    .is_ok_and(|own| paths::resolve(&own) == prefix)
        }),
    }
}

#[cfg(test)]
mod tests;
