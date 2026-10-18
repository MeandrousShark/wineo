//! Planning and registering installed Steam games as proxied environments.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::Context;
use image::{ImageFormat, ImageReader, Limits};
use toml_edit::{Array, value};

use crate::art::{self, Kind};
use crate::cancel::Token;
use crate::config::{self, GlobalConfig, Launcher};
use crate::environment::{self, Environment};
use crate::heroic_import::default_name;
use crate::key_store::Keyring;
use crate::operation::Locks;
use crate::paths::Paths;
use crate::secret_service::SecretServiceKeyring;
use crate::steam::api::{ApiError, SteamApi};
use crate::steam::owned::{self, LoadError, Loaded, Owned, OwnedError, OwnedGame};
use crate::steam::{self, AppState, Install, InstallState};

/// What a command knows about the owned list. It is loaded once per command
/// and passed in, so nothing here fetches.
#[derive(Debug, Clone, Copy)]
pub enum OwnedGames<'a> {
    /// No list was asked for, or there is no key, so only installed games
    /// can be imported.
    Unloaded,
    /// The list was asked for and couldn't be loaded.
    Failed(&'a LoadError),
    Loaded(&'a Owned),
}

impl OwnedGames<'_> {
    fn find(&self, app_id: &str) -> Option<&OwnedGame> {
        match self {
            OwnedGames::Loaded(owned) => owned.games.iter().find(|game| game.app_id == app_id),
            OwnedGames::Unloaded | OwnedGames::Failed(_) => None,
        }
    }
}

/// The line listings show when there is no Steam Web API key.
pub const NO_KEY_HINT: &str = "Add a Steam Web API key to see uninstalled games.";

/// What a frontend knows about the owned games, loaded once by whatever wants
/// uninstalled games. The CLI and the desktop app share it, so both look for
/// the key and fall back the same way.
#[derive(Debug, Clone)]
pub enum OwnedData {
    /// Nothing was asked for, or Steam isn't found, which the import reports.
    Unloaded,
    /// There is no Steam Web API key.
    NoKey,
    /// The key couldn't be looked up, like a dismissed keyring prompt.
    KeyFailed(String),
    Failed(LoadError),
    Loaded(Owned),
}

impl OwnedData {
    /// Looks for the key and, with one, loads the list.
    ///
    /// Several accounts with none marked most recent is an error, since there
    /// is no telling whose library to ask for. With `save` false (a dry run)
    /// the saved list is left alone.
    pub fn load(
        paths: &Paths,
        global: &GlobalConfig,
        refresh: bool,
        save: bool,
    ) -> anyhow::Result<Self> {
        let keyring = SecretServiceKeyring::steam();
        Self::load_with(
            paths,
            global,
            refresh,
            save,
            |var| std::env::var_os(var),
            &keyring,
            SteamApi::from_env,
        )
    }

    /// `load` with the key lookup and the API passed in, for tests.
    pub fn load_with(
        paths: &Paths,
        global: &GlobalConfig,
        refresh: bool,
        save: bool,
        lookup: impl Fn(&str) -> Option<OsString>,
        keyring: &dyn Keyring,
        api: impl FnOnce() -> Result<SteamApi, ApiError>,
    ) -> anyhow::Result<Self> {
        let key = match steam::key::find_key(paths, lookup, keyring) {
            Ok(Some(key)) => key,
            Ok(None) => return Ok(Self::NoKey),
            Err(error) => return Ok(Self::KeyFailed(error.to_string())),
        };
        let Some(install) = steam::active_for(paths, global) else {
            return Ok(Self::Unloaded);
        };
        let steam_id = steam::active_steam_id(&install).map_err(anyhow::Error::msg)?;
        let api = match api() {
            Ok(api) => api,
            Err(error) => return Ok(Self::Failed(LoadError(OwnedError::Api(error)))),
        };
        let load = if save {
            owned::load
        } else {
            owned::load_without_saving
        };
        Ok(
            match load(paths, steam_id, Some(&key.value), &api, refresh) {
                Ok(Loaded::Owned(owned)) => Self::Loaded(owned),
                Ok(Loaded::NoKey) => Self::NoKey,
                Err(error) => Self::Failed(error),
            },
        )
    }

    pub fn games(&self) -> OwnedGames<'_> {
        match self {
            Self::Unloaded | Self::NoKey | Self::KeyFailed(_) => OwnedGames::Unloaded,
            Self::Failed(error) => OwnedGames::Failed(error),
            Self::Loaded(owned) => OwnedGames::Loaded(owned),
        }
    }

    /// What is wrong with the list, for listings that go on without it or
    /// with an old one. A missing key isn't a problem here; see
    /// `NO_KEY_HINT`.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::KeyFailed(reason) => Some(format!("Couldn't get owned games: {reason}")),
            Self::Failed(error) => Some(error.to_string()),
            Self::Loaded(Owned {
                warning: Some(warning),
                ..
            }) => Some(warning.clone()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryState {
    Installed,
    Updating,
    /// Steam is downloading it, and it isn't playable yet.
    Installing,
    InstallQueued,
    Uninstalling,
    /// Owned, with nothing on disk.
    NotInstalled,
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub app_id: String,
    pub title: Option<String>,
    pub size_on_disk: Option<u64>,
    pub state: EntryState,
    /// How far an install is, from 0 to 100, while it is installing and
    /// Steam has written totals.
    pub install_progress: Option<u8>,
    pub imported_as: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub app_id: String,
    pub title: String,
    pub name: String,
    pub size_on_disk: u64,
    pub state: InstallState,
}

impl Plan {
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn size_on_disk(&self) -> u64 {
        self.size_on_disk
    }

    pub fn state(&self) -> InstallState {
        self.state
    }
}

#[derive(Debug, Clone)]
pub enum PlanOutcome {
    Ready(Plan),
    AlreadyImported(String),
}

impl PlanOutcome {
    pub fn already_imported(&self) -> Option<&str> {
        match self {
            PlanOutcome::AlreadyImported(name) => Some(name),
            PlanOutcome::Ready(_) => None,
        }
    }
}

#[derive(Debug)]
pub struct Applied {
    pub environment: Environment,
    pub art: Vec<Kind>,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ApplyOutcome {
    Imported(Applied),
    AlreadyImported(String),
}

/// Lists importable games and malformed manifests from the active install,
/// then the owned games that aren't installed. Tool ids are hidden even when
/// their manifests cannot be read.
pub fn listing(
    paths: &Paths,
    global: &GlobalConfig,
    owned: OwnedGames<'_>,
) -> anyhow::Result<Vec<Entry>> {
    check_portable(paths)?;
    let install = active_install(paths, global)?;
    let imported = imported_games(paths)?;
    let imported_as = |app_id: &str| {
        imported
            .iter()
            .find(|(id, _)| id == app_id)
            .map(|(_, name)| name.clone())
    };
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    // Manifests that say the game isn't there, with what they do say. Only
    // the owned list can bring them back into the listing.
    let mut absent = HashMap::new();
    for item in steam::manifests(&install) {
        if !seen.insert(item.app_id.clone()) || steam::is_tool(&item.app_id, "") {
            continue;
        }
        let imported_as = imported_as(&item.app_id);
        match item.manifest {
            Ok(manifest) if !manifest.is_tool() => {
                let app_state = manifest.app_state();
                let state = entry_state(&app_state);
                let install_progress = match app_state {
                    AppState::Installing(percent) => percent,
                    _ => None,
                };
                if manifest.state() == InstallState::NotInstalled {
                    absent.insert(item.app_id, state);
                    continue;
                }
                entries.push(Entry {
                    app_id: item.app_id,
                    title: Some(manifest.name),
                    size_on_disk: Some(manifest.size_on_disk),
                    state,
                    install_progress,
                    imported_as,
                });
            }
            Ok(_) => {}
            Err(error) => entries.push(Entry {
                app_id: item.app_id,
                title: None,
                size_on_disk: None,
                state: EntryState::Unreadable(error),
                install_progress: None,
                imported_as,
            }),
        }
    }
    if let OwnedGames::Loaded(owned) = owned {
        let mut taken = HashSet::new();
        let mut rest: Vec<&OwnedGame> = owned
            .games
            .iter()
            .filter(|game| !steam::is_tool(&game.app_id, &game.name))
            .filter(|game| {
                (!seen.contains(&game.app_id) || absent.contains_key(&game.app_id))
                    && taken.insert(game.app_id.as_str())
            })
            .collect();
        // The API's order is arbitrary, and a whole library is long.
        rest.sort_by_cached_key(|game| (game.name.to_lowercase(), game.app_id.clone()));
        for game in rest {
            entries.push(Entry {
                app_id: game.app_id.clone(),
                title: Some(game.name.clone()),
                size_on_disk: None,
                state: absent
                    .get(&game.app_id)
                    .cloned()
                    .unwrap_or(EntryState::NotInstalled),
                install_progress: None,
                imported_as: imported_as(&game.app_id),
            });
        }
    }
    Ok(entries)
}

fn entry_state(state: &AppState) -> EntryState {
    match state {
        AppState::Installed => EntryState::Installed,
        AppState::Updating => EntryState::Updating,
        AppState::Installing(_) => EntryState::Installing,
        AppState::InstallQueued => EntryState::InstallQueued,
        AppState::Uninstalling => EntryState::Uninstalling,
        AppState::NotInstalled | AppState::Unavailable(_) | AppState::Unknown(_) => {
            EntryState::NotInstalled
        }
    }
}

/// Resolves one Steam app id without writing anything. A previously imported
/// app is a skip, regardless of which name the caller requested for it. A game
/// that isn't installed is resolved from the owned list.
pub fn plan(
    paths: &Paths,
    global: &GlobalConfig,
    owned: OwnedGames<'_>,
    app_id: &str,
    name: Option<&str>,
) -> anyhow::Result<PlanOutcome> {
    check_portable(paths)?;
    let install = active_install(paths, global)?;
    if let Some((_, existing)) = imported_games(paths)?
        .into_iter()
        .find(|(id, _)| id == app_id)
    {
        return Ok(PlanOutcome::AlreadyImported(existing));
    }
    let game = resolve(&install, owned, app_id)?;
    if game.tool {
        anyhow::bail!("Steam lists {app_id} as a tool.");
    }
    let name = name
        .map(str::to_owned)
        .unwrap_or_else(|| default_name(&game.title, None));
    environment::check_new_name(paths, &name)?;
    Ok(PlanOutcome::Ready(Plan {
        app_id: app_id.to_owned(),
        title: game.title,
        name,
        size_on_disk: game.size_on_disk,
        state: game.state,
    }))
}

/// Whether importing `app_id` would look at the owned list, because nothing
/// on disk settles it. Callers load the list only when this is true, so
/// importing an installed game never needs a key or the network.
pub fn needs_owned(paths: &Paths, global: &GlobalConfig, app_id: &str) -> bool {
    let Ok(install) = active_install(paths, global) else {
        return false;
    };
    if imported_games(paths).is_ok_and(|games| games.iter().any(|(id, _)| id == app_id)) {
        return false;
    }
    match steam::manifests(&install)
        .into_iter()
        .find(|item| item.app_id == app_id)
    {
        Some(item) => item
            .manifest
            .is_ok_and(|manifest| manifest.state() == InstallState::NotInstalled),
        None => true,
    }
}

/// What Steam, or failing that the owned list, says about one app.
struct Game {
    title: String,
    size_on_disk: u64,
    state: InstallState,
    tool: bool,
}

fn resolve(install: &Install, owned: OwnedGames<'_>, app_id: &str) -> anyhow::Result<Game> {
    let found = steam::manifests(install)
        .into_iter()
        .find(|item| item.app_id == app_id);
    if let Some(item) = found {
        let manifest = item.manifest.map_err(anyhow::Error::msg)?;
        let state = manifest.state();
        if state != InstallState::NotInstalled {
            return Ok(Game {
                tool: manifest.is_tool(),
                title: manifest.name,
                size_on_disk: manifest.size_on_disk,
                state,
            });
        }
    } else if owned.find(app_id).is_none() {
        // A library that can't be read might hold the game, which is worth
        // saying unless the owned list settles it.
        match steam::app_state(Some(install), app_id) {
            AppState::Unavailable(path) => {
                anyhow::bail!("Steam's library at {} isn't available.", path.display())
            }
            AppState::Unknown(reason) => anyhow::bail!("{reason}."),
            _ => {}
        }
    }
    match owned {
        OwnedGames::Loaded(_) => match owned.find(app_id) {
            Some(game) => Ok(Game {
                tool: steam::is_tool(app_id, &game.name),
                title: game.name.clone(),
                size_on_disk: 0,
                state: InstallState::NotInstalled,
            }),
            None => anyhow::bail!("App {app_id} isn't installed or owned."),
        },
        OwnedGames::Failed(error) => anyhow::bail!("{error}"),
        OwnedGames::Unloaded => anyhow::bail!("Steam lists {app_id} as not installed."),
    }
}

/// Creates the config, then copies any valid Steam cached art. Art errors are
/// returned as warnings and never undo the registered environment.
pub fn apply(
    paths: &Paths,
    global: &GlobalConfig,
    owned: OwnedGames<'_>,
    plan: &Plan,
    cancel: &Token,
) -> anyhow::Result<ApplyOutcome> {
    check_portable(paths)?;
    crate::import::check_cancelled(cancel)?;
    let locks = Locks::new(paths);
    let _app_guard = locks.steam_app(&plan.app_id)?;
    crate::import::check_cancelled(cancel)?;
    if let Some((_, existing)) = imported_games(paths)?
        .into_iter()
        .find(|(id, _)| id == &plan.app_id)
    {
        return Ok(ApplyOutcome::AlreadyImported(existing));
    }
    let install = active_install(paths, global)?;
    let game = resolve(&install, owned, &plan.app_id)?;
    environment::check_new_name(paths, &plan.name)?;
    let path = paths.environment_config(&plan.name);
    let mut document = config::new_document();
    document["kind"] = value("proxied");
    document["launcher"] = value(Launcher::Steam.as_str());
    document["app_id"] = value(&plan.app_id);
    document["title"] = value(&game.title);
    let mut tags = Array::new();
    tags.push("game");
    document["tags"] = value(tags);
    document["desktop"] = value(false);
    let _guard = locks.config(&path)?;
    crate::import::check_cancelled(cancel)?;
    write_new(&path, &document.to_string())?;
    let environment = Environment::load(paths, &plan.name)?;
    let mut art = Vec::new();
    let mut warnings = Vec::new();
    if let Some(warning) = crate::library_lifecycle::fresh_warning(paths, &plan.name) {
        warnings.push(warning);
    }
    for (kind, source) in steam::art_files(&install, &plan.app_id) {
        if cancel.is_canceled() {
            break;
        }
        match store_cached_art(paths, &plan.name, kind, &source) {
            Ok(()) => art.push(kind),
            Err(error) => warnings.push(format!("Could not import {} art: {error:#}.", kind)),
        }
    }
    Ok(ApplyOutcome::Imported(Applied {
        environment,
        art,
        warnings,
    }))
}

/// A game a batch couldn't plan or import, and the name it would have had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub app_id: String,
    pub name: String,
    pub reason: String,
}

/// One game asked for in a batch.
#[derive(Debug, Clone)]
pub struct Wanted {
    pub app_id: String,
    /// Names a game that fails before a plan names it.
    pub title: Option<String>,
    /// What the listing already found it imported as, so it isn't planned.
    pub imported_as: Option<String>,
}

/// A batch planned for review, before anything is written.
#[derive(Debug, Clone, Default)]
pub struct Batch {
    pub ready: Vec<Plan>,
    pub already: Vec<String>,
    pub failures: Vec<Failure>,
}

/// What `plan_batch` found for one game, as it goes.
pub enum Planned<'a> {
    Ready(&'a Plan),
    AlreadyImported(&'a str),
}

/// Plans each game in order, calling `planned` for each one that will import
/// or already has. `name` applies to every game, so it only suits a batch of
/// one. When two games land on the same name the first keeps it and the rest
/// fail, as they would once it exists. An error from `planned` stops the batch.
pub fn plan_batch(
    paths: &Paths,
    global: &GlobalConfig,
    owned: OwnedGames<'_>,
    games: impl IntoIterator<Item = Wanted>,
    name: Option<&str>,
    mut planned: impl FnMut(Planned<'_>) -> anyhow::Result<()>,
) -> anyhow::Result<Batch> {
    let mut batch = Batch::default();
    let mut names = HashSet::new();
    for game in games {
        if let Some(existing) = game.imported_as {
            planned(Planned::AlreadyImported(&existing))?;
            batch.already.push(existing);
            continue;
        }
        match plan(paths, global, owned, &game.app_id, name) {
            Ok(PlanOutcome::Ready(plan)) if names.insert(plan.name().to_owned()) => {
                planned(Planned::Ready(&plan))?;
                batch.ready.push(plan);
            }
            Ok(PlanOutcome::Ready(plan)) => batch.failures.push(Failure {
                app_id: game.app_id,
                reason: crate::Error::EnvironmentExists(plan.name.clone()).to_string(),
                name: plan.name,
            }),
            Ok(PlanOutcome::AlreadyImported(existing)) => {
                planned(Planned::AlreadyImported(&existing))?;
                batch.already.push(existing);
            }
            Err(error) => {
                let name = match (name, &game.title) {
                    (Some(name), _) => name.to_owned(),
                    (None, Some(title)) => default_name(title, None),
                    (None, None) => game.app_id.clone(),
                };
                batch.failures.push(Failure {
                    app_id: game.app_id,
                    name,
                    reason: format!("{error:#}"),
                });
            }
        }
    }
    Ok(batch)
}

/// What `apply_batch` is doing with one game, as it goes.
pub enum Applying<'a> {
    Starting(&'a Plan),
    Imported(&'a Plan, &'a Applied),
    /// Something else imported it since it was planned.
    AlreadyImported(&'a str),
}

/// How a batch went. Games found already imported while applying leave the
/// total, so it only counts what this batch could have imported.
#[derive(Debug, Clone, Default)]
pub struct BatchReport {
    pub done: Vec<String>,
    pub total: usize,
    pub already: Vec<String>,
    pub failures: Vec<Failure>,
    pub warnings: Vec<String>,
    pub canceled: bool,
}

impl From<Batch> for BatchReport {
    /// A batch with nothing applied yet, keeping its planning failures.
    fn from(batch: Batch) -> Self {
        Self {
            total: batch.ready.len(),
            failures: batch.failures,
            ..Self::default()
        }
    }
}

/// Imports each ready game in order, calling `each` as it goes. Cancellation
/// stops at the next game and reports what finished. An error from `each`
/// stops the batch and is returned instead of a report.
pub fn apply_batch(
    paths: &Paths,
    global: &GlobalConfig,
    owned: OwnedGames<'_>,
    mut batch: Batch,
    cancel: &Token,
    mut each: impl FnMut(Applying<'_>) -> anyhow::Result<()>,
) -> anyhow::Result<BatchReport> {
    let ready = std::mem::take(&mut batch.ready);
    let mut report = BatchReport {
        total: ready.len(),
        ..batch.into()
    };
    for plan in ready {
        if cancel.is_canceled() {
            report.canceled = true;
            break;
        }
        each(Applying::Starting(&plan))?;
        match apply(paths, global, owned, &plan, cancel) {
            Ok(ApplyOutcome::Imported(applied)) => {
                each(Applying::Imported(&plan, &applied))?;
                report.done.push(applied.environment.name);
                report.warnings.extend(applied.warnings);
            }
            Ok(ApplyOutcome::AlreadyImported(name)) => {
                each(Applying::AlreadyImported(&name))?;
                report.total -= 1;
                report.already.push(name);
            }
            Err(_) if cancel.is_canceled() => {
                report.canceled = true;
                break;
            }
            Err(error) => report.failures.push(Failure {
                app_id: plan.app_id,
                name: plan.name,
                reason: format!("{error:#}"),
            }),
        }
    }
    if cancel.is_canceled() {
        report.canceled = true;
    }
    Ok(report)
}

fn check_portable(paths: &Paths) -> anyhow::Result<()> {
    if paths.portable_root().is_some() {
        anyhow::bail!("Steam games can't be imported into a portable folder.");
    }
    Ok(())
}

fn active_install(paths: &Paths, global: &GlobalConfig) -> anyhow::Result<Install> {
    steam::active_for(paths, global).ok_or_else(|| anyhow::anyhow!("Steam wasn't found."))
}

fn imported_games(paths: &Paths) -> crate::Result<Vec<(String, String)>> {
    let mut found = Vec::new();
    for name in environment::names(paths)? {
        let Ok(environment) = Environment::load(paths, &name) else {
            continue;
        };
        if let Some(proxied) = environment.config.proxied()
            && proxied.launcher == Launcher::Steam
        {
            found.push((proxied.app_id.clone(), name));
        }
    }
    Ok(found)
}

fn write_new(path: &Path, contents: &str) -> anyhow::Result<()> {
    crate::import::write_config_once(path, contents)
}

fn store_cached_art(paths: &Paths, name: &str, kind: Kind, source: &Path) -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    fs::File::open(source)
        .with_context(|| format!("failed to read {}", source.display()))?
        .take(art::MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", source.display()))?;
    if bytes.len() as u64 > art::MAX_BYTES {
        anyhow::bail!("{} is too large", source.display());
    }
    let format = art::detect(&bytes)
        .filter(|format| kind.accepts(*format))
        .ok_or_else(|| anyhow::anyhow!("{} isn't a valid image", source.display()))?;
    let image_format = match format {
        art::Format::Png => ImageFormat::Png,
        art::Format::Jpeg => ImageFormat::Jpeg,
        art::Format::Webp => ImageFormat::WebP,
        _ => anyhow::bail!("{} isn't a valid image", source.display()),
    };
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), image_format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(8192 * 8192 * 4);
    reader.limits(limits);
    reader
        .decode()
        .with_context(|| format!("{} isn't a valid image", source.display()))?;
    let original = format!(
        "steam-{}-{}",
        source
            .parent()
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy(),
        source.file_name().unwrap_or_default().to_string_lossy()
    );
    art::set_manual_bytes(paths, name, None, kind, &bytes, &original)?;
    Ok(())
}

#[cfg(test)]
mod tests;
