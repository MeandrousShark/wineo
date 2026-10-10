//! Planning and registering installed Steam games as proxied environments.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Read, Write};
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
        let keyring = SecretServiceKeyring::for_spec(&steam::key::KEY_SPEC);
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
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .with_context(|| format!("failed to write {}", path.display()))
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
mod tests {
    use std::ffi::OsString;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};

    use image::{ImageFormat, Rgba, RgbaImage};

    use super::*;

    fn paths(root: &Path, portable: bool) -> Paths {
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(&home)),
            portable.then(|| root.join("portable")).as_deref(),
        )
        .unwrap()
    }

    fn steam_root(paths: &Paths, flatpak: bool) -> PathBuf {
        let root = if flatpak {
            paths
                .home()
                .join(".var/app/com.valvesoftware.Steam/.local/share/Steam")
        } else {
            paths.home().join(".local/share/Steam")
        };
        fs::create_dir_all(root.join("steamapps")).unwrap();
        root
    }

    fn manifest(root: &Path, app_id: &str, title: &str, flags: u32, size: u64) {
        fs::write(
            root.join(format!("steamapps/appmanifest_{app_id}.acf")),
            format!(
                "\"AppState\" {{ \"appid\" \"{app_id}\" \"name\" \"{title}\" \"StateFlags\" \"{flags}\" \"SizeOnDisk\" \"{size}\" }}"
            ),
        )
        .unwrap();
    }

    fn ready(paths: &Paths, global: &GlobalConfig, app_id: &str, name: Option<&str>) -> Plan {
        match plan(paths, global, OwnedGames::Unloaded, app_id, name).unwrap() {
            PlanOutcome::Ready(plan) => plan,
            PlanOutcome::AlreadyImported(_) => panic!("app was already imported"),
        }
    }

    fn imported(paths: &Paths, plan: &Plan) -> Applied {
        match apply(
            paths,
            &GlobalConfig::default(),
            OwnedGames::Unloaded,
            plan,
            &Token::new(),
        )
        .unwrap()
        {
            ApplyOutcome::Imported(applied) => applied,
            ApplyOutcome::AlreadyImported(_) => panic!("app was already imported"),
        }
    }

    fn png() -> Vec<u8> {
        let image = RgbaImage::from_pixel(2, 2, Rgba([17, 23, 42, 255]));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn list_keeps_healthy_games_beside_bad_manifests_and_hides_tools() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "10", "Happy Game", 4, 1024);
        manifest(&root, "11", "Not Installed", 2, 0);
        manifest(&root, "228980", "Steamworks Common Redistributables", 4, 12);
        manifest(&root, "12", "Proton 9.0", 4, 12);
        fs::write(root.join("steamapps/appmanifest_13.acf"), "bad {").unwrap();
        let entries = listing(&paths, &GlobalConfig::default(), OwnedGames::Unloaded).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].app_id, "10");
        assert_eq!(entries[0].title.as_deref(), Some("Happy Game"));
        assert_eq!(entries[0].size_on_disk, Some(1024));
        assert_eq!(entries[0].state, EntryState::Installed);
        assert_eq!(entries[1].app_id, "13");
        assert!(matches!(entries[1].state, EntryState::Unreadable(_)));
        let planned = ready(&paths, &GlobalConfig::default(), "10", None);
        imported(&paths, &planned);
        assert!(paths.environment_config(&planned.name).exists());
    }

    #[test]
    fn plan_and_apply_write_the_proxied_config_and_match_a_custom_name() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "42", "My Game", 4, 4096);
        let global = GlobalConfig::default();
        let planned = ready(&paths, &global, "42", Some("custom"));
        assert!(!paths.environment_config("custom").exists());
        let applied = imported(&paths, &planned);
        assert_eq!(applied.environment.name, "custom");
        assert!(applied.warnings.is_empty());
        assert_eq!(
            fs::read_to_string(paths.environment_config("custom")).unwrap(),
            "schema_version = 1\nkind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"42\"\ntitle = \"My Game\"\ntags = [\"game\"]\ndesktop = false\n"
        );
        assert_eq!(
            listing(&paths, &global, OwnedGames::Unloaded).unwrap()[0]
                .imported_as
                .as_deref(),
            Some("custom")
        );
        assert!(matches!(
            plan(&paths, &global, OwnedGames::Unloaded, "42", Some("another")).unwrap(),
            PlanOutcome::AlreadyImported(name) if name == "custom"
        ));
    }

    #[test]
    fn names_tools_and_missing_games_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "1", "One Game", 4, 1);
        manifest(&root, "2", "Two Game", 2, 2);
        manifest(&root, "3", "Proton Experimental", 4, 3);
        let global = GlobalConfig::default();
        let first = ready(&paths, &global, "1", None);
        assert_eq!(first.name, "one-game");
        imported(&paths, &first);
        assert_eq!(
            plan(&paths, &global, OwnedGames::Unloaded, "2", None)
                .unwrap_err()
                .to_string(),
            "Steam lists 2 as not installed."
        );
        assert_eq!(
            plan(&paths, &global, OwnedGames::Unloaded, "99", None)
                .unwrap_err()
                .to_string(),
            "Steam lists 99 as not installed."
        );
        assert_eq!(
            plan(&paths, &global, OwnedGames::Unloaded, "3", None)
                .unwrap_err()
                .to_string(),
            "Steam lists 3 as a tool."
        );
        assert_eq!(
            plan(&paths, &global, OwnedGames::Unloaded, "1", Some("one-game"))
                .unwrap()
                .already_imported(),
            Some("one-game")
        );
        manifest(&root, "4", "Four Game", 4, 4);
        assert_eq!(
            plan(&paths, &global, OwnedGames::Unloaded, "4", Some("one-game"))
                .unwrap_err()
                .to_string(),
            "environment `one-game` already exists"
        );
    }

    #[test]
    fn cached_art_is_checked_and_a_bad_file_only_warns() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "42", "My Game", 4, 1);
        let cache = root.join("appcache/librarycache/42");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("library_600x900.jpg"), png()).unwrap();
        fs::write(cache.join("library_hero.jpg"), b"invalid image").unwrap();
        fs::write(cache.join("logo.png"), png()).unwrap();
        let planned = ready(&paths, &GlobalConfig::default(), "42", None);
        let applied = imported(&paths, &planned);
        assert_eq!(applied.art, vec![Kind::Grid, Kind::Logo]);
        assert_eq!(applied.warnings.len(), 1);
        assert!(applied.warnings[0].contains("hero"));
        assert!(paths.art_dir(&planned.name).join("grid.png").exists());
        assert!(paths.art_dir(&planned.name).join("logo.png").exists());
        assert!(!paths.art_dir(&planned.name).join("hero.jpg").exists());
        assert!(cache.join("library_600x900.jpg").exists());
    }

    #[test]
    fn apply_uses_the_reselected_install_for_title_and_cached_art() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let native = steam_root(&paths, false);
        let flatpak = steam_root(&paths, true);
        manifest(&native, "42", "Native Title", 4, 1);
        manifest(&flatpak, "42", "Flatpak Title", 4, 2);
        let native_art = png();
        let mut flatpak_art = Cursor::new(Vec::new());
        RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 255]))
            .write_to(&mut flatpak_art, ImageFormat::Png)
            .unwrap();
        let flatpak_art = flatpak_art.into_inner();
        for (root, bytes) in [(&native, &native_art), (&flatpak, &flatpak_art)] {
            let cache = root.join("appcache/librarycache/42");
            fs::create_dir_all(&cache).unwrap();
            fs::write(cache.join("library_600x900.jpg"), bytes).unwrap();
        }
        let mut global = GlobalConfig::default();
        let planned = ready(&paths, &global, "42", None);
        assert_eq!(planned.title(), "Native Title");
        global.steam = Some(steam::InstallKind::Flatpak);

        let applied = match apply(
            &paths,
            &global,
            OwnedGames::Unloaded,
            &planned,
            &Token::new(),
        )
        .unwrap()
        {
            ApplyOutcome::Imported(applied) => applied,
            ApplyOutcome::AlreadyImported(_) => panic!("app was already imported"),
        };

        assert_eq!(applied.art, [Kind::Grid]);
        assert!(applied.warnings.is_empty());
        assert_eq!(
            applied
                .environment
                .config
                .proxied()
                .unwrap()
                .title
                .as_deref(),
            Some("Flatpak Title")
        );
        assert_eq!(
            fs::read(paths.art_dir(planned.name()).join("grid.png")).unwrap(),
            flatpak_art
        );
    }

    #[test]
    fn cached_art_with_a_valid_header_and_corrupt_body_is_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "42", "My Game", 4, 1);
        let cache = root.join("appcache/librarycache/42");
        fs::create_dir_all(&cache).unwrap();
        let corrupt = &png()[..16];
        assert_eq!(art::detect(corrupt), Some(art::Format::Png));
        fs::write(cache.join("library_600x900.jpg"), corrupt).unwrap();
        let planned = ready(&paths, &GlobalConfig::default(), "42", None);
        let applied = imported(&paths, &planned);
        assert!(applied.art.is_empty());
        assert_eq!(applied.warnings.len(), 1);
        assert!(paths.environment_config(&planned.name).exists());
        assert!(!paths.art_dir(&planned.name).exists());
    }

    #[test]
    fn every_bad_art_file_warns_without_blocking_import() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "42", "My Game", 4, 1);
        let cache = root.join("appcache/librarycache/42");
        fs::create_dir_all(&cache).unwrap();
        for file in ["library_600x900.jpg", "library_hero.jpg", "logo.png"] {
            fs::write(cache.join(file), b"invalid image").unwrap();
        }
        let planned = ready(&paths, &GlobalConfig::default(), "42", None);
        let applied = imported(&paths, &planned);
        assert!(paths.environment_config(&planned.name).exists());
        assert!(applied.art.is_empty());
        assert_eq!(applied.warnings.len(), 3);
        assert!(!paths.art_dir(&planned.name).exists());
    }

    #[test]
    fn cancellation_before_apply_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "42", "My Game", 4, 1);
        let planned = ready(&paths, &GlobalConfig::default(), "42", None);
        let token = Token::new();
        token.cancel();
        assert!(
            apply(
                &paths,
                &GlobalConfig::default(),
                OwnedGames::Unloaded,
                &planned,
                &token
            )
            .is_err()
        );
        assert!(!paths.environment_config(&planned.name).exists());
    }

    #[test]
    fn apply_skips_an_app_imported_under_another_name_after_planning() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "42", "My Game", 4, 1);
        let cache = root.join("appcache/librarycache/42");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("logo.png"), png()).unwrap();
        let global = GlobalConfig::default();
        let planned = ready(&paths, &global, "42", Some("reviewed"));
        let other = ready(&paths, &global, "42", Some("another-name"));
        imported(&paths, &other);
        let config = fs::read(paths.environment_config("another-name")).unwrap();
        let art = fs::read(paths.art_dir("another-name").join("logo.png")).unwrap();

        let outcome = apply(
            &paths,
            &global,
            OwnedGames::Unloaded,
            &planned,
            &Token::new(),
        )
        .unwrap();

        assert!(matches!(outcome, ApplyOutcome::AlreadyImported(name) if name == "another-name"));
        assert_eq!(environment::names(&paths).unwrap(), ["another-name"]);
        assert!(!paths.environment_config("reviewed").exists());
        assert!(!paths.art_dir("reviewed").exists());
        assert_eq!(
            fs::read(paths.environment_config("another-name")).unwrap(),
            config
        );
        assert_eq!(
            fs::read(paths.art_dir("another-name").join("logo.png")).unwrap(),
            art
        );
    }

    #[test]
    fn apply_refuses_a_game_uninstalled_after_planning() {
        for remove_manifest in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let paths = paths(temp.path(), false);
            let root = steam_root(&paths, false);
            manifest(&root, "42", "My Game", 4, 1);
            let global = GlobalConfig::default();
            let planned = ready(&paths, &global, "42", None);
            if remove_manifest {
                fs::remove_file(root.join("steamapps/appmanifest_42.acf")).unwrap();
            } else {
                manifest(&root, "42", "My Game", 2048, 1);
            }

            assert_eq!(
                apply(
                    &paths,
                    &global,
                    OwnedGames::Unloaded,
                    &planned,
                    &Token::new()
                )
                .unwrap_err()
                .to_string(),
                "Steam lists 42 as not installed."
            );
            assert!(!paths.environment_config(&planned.name).exists());
            assert!(!paths.art_dir(&planned.name).exists());
        }
    }

    #[test]
    fn apply_rechecks_the_active_install_from_global_config() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let native = steam_root(&paths, false);
        steam_root(&paths, true);
        manifest(&native, "42", "My Game", 4, 1);
        let mut global = GlobalConfig::default();
        let planned = ready(&paths, &global, "42", None);
        global.steam = Some(steam::InstallKind::Flatpak);

        assert_eq!(
            apply(
                &paths,
                &global,
                OwnedGames::Unloaded,
                &planned,
                &Token::new()
            )
            .unwrap_err()
            .to_string(),
            "Steam lists 42 as not installed."
        );
        assert!(!paths.environment_config(&planned.name).exists());
    }

    #[test]
    fn concurrent_apply_registers_the_app_only_once() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "42", "My Game", 4, 1);
        let global = GlobalConfig::default();
        let plans = [
            ready(&paths, &global, "42", Some("one")),
            ready(&paths, &global, "42", Some("two")),
        ];
        let barrier = std::sync::Barrier::new(2);
        let outcomes = std::thread::scope(|scope| {
            let handles: Vec<_> = plans
                .iter()
                .map(|plan| {
                    scope.spawn(|| {
                        barrier.wait();
                        apply(&paths, &global, OwnedGames::Unloaded, plan, &Token::new()).unwrap()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        let names = environment::names(&paths).unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, ApplyOutcome::Imported(_)))
                .count(),
            1
        );
        let skipped: Vec<_> = outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                ApplyOutcome::AlreadyImported(name) => Some(name),
                ApplyOutcome::Imported(_) => None,
            })
            .collect();
        assert_eq!(skipped, [&names[0]]);
    }

    #[test]
    fn apply_preserves_a_name_taken_after_planning() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "42", "My Game", 4, 1);
        let global = GlobalConfig::default();
        let planned = ready(&paths, &global, "42", None);
        let path = paths.environment_config(&planned.name);
        write_new(&path, "kind = 'native'\n").unwrap();

        assert_eq!(
            apply(
                &paths,
                &global,
                OwnedGames::Unloaded,
                &planned,
                &Token::new()
            )
            .unwrap_err()
            .to_string(),
            "environment `my-game` already exists"
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "kind = 'native'\n");
    }

    #[test]
    fn active_install_and_portable_rules_apply_to_listing_and_plan() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        assert_eq!(
            listing(&paths, &GlobalConfig::default(), OwnedGames::Unloaded)
                .unwrap_err()
                .to_string(),
            "Steam wasn't found."
        );
        let native = steam_root(&paths, false);
        let flatpak = steam_root(&paths, true);
        manifest(&native, "1", "Native Game", 4, 1);
        manifest(&flatpak, "2", "Flatpak Game", 4, 2);
        let mut global = GlobalConfig::default();
        assert_eq!(
            listing(&paths, &global, OwnedGames::Unloaded).unwrap()[0].app_id,
            "1"
        );
        assert_eq!(
            plan(&paths, &global, OwnedGames::Unloaded, "2", None)
                .unwrap_err()
                .to_string(),
            "Steam lists 2 as not installed."
        );
        global.steam = Some(steam::InstallKind::Flatpak);
        assert_eq!(
            listing(&paths, &global, OwnedGames::Unloaded).unwrap()[0].app_id,
            "2"
        );
        let portable = self::paths(temp.path(), true);
        assert_eq!(
            listing(&portable, &global, OwnedGames::Unloaded)
                .unwrap_err()
                .to_string(),
            "Steam games can't be imported into a portable folder."
        );
        assert_eq!(
            plan(&portable, &global, OwnedGames::Unloaded, "2", None)
                .unwrap_err()
                .to_string(),
            "Steam games can't be imported into a portable folder."
        );
    }

    fn owned_list(games: &[(&str, &str)]) -> Owned {
        Owned {
            games: games
                .iter()
                .map(|(app_id, name)| OwnedGame {
                    app_id: (*app_id).to_owned(),
                    name: (*name).to_owned(),
                })
                .collect(),
            warning: None,
        }
    }

    fn hidden() -> LoadError {
        LoadError(crate::steam::owned::OwnedError::Hidden)
    }

    #[test]
    fn list_adds_owned_games_that_are_not_installed_after_the_installed_ones() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "10", "Happy Game", 4, 1024);
        manifest(&root, "11", "Queued Game", 2, 0);
        manifest(&root, "12", "Going Game", 4 | 2048, 0);
        manifest(&root, "13", "Downloading Game", 1026, 0);
        manifest(&root, "228980", "Steamworks Common Redistributables", 4, 12);
        let global = GlobalConfig::default();
        let owned = owned_list(&[
            ("500", "zebra Crossing"),
            ("10", "Happy Game"),
            ("11", "Queued Game"),
            ("12", "Going Game"),
            ("228980", "Steamworks Common Redistributables"),
            ("400", "Apple Orchard"),
            ("400", "Apple Orchard"),
        ]);

        let entries = listing(&paths, &global, OwnedGames::Loaded(&owned)).unwrap();

        let shown: Vec<_> = entries
            .iter()
            .map(|entry| (entry.app_id.as_str(), entry.state.clone()))
            .collect();
        // Installed games come first, then the rest by title.
        assert_eq!(
            shown,
            [
                ("10", EntryState::Installed),
                ("13", EntryState::Installing),
                ("400", EntryState::NotInstalled),
                ("12", EntryState::Uninstalling),
                ("11", EntryState::InstallQueued),
                ("500", EntryState::NotInstalled),
            ]
        );
        let owned_only = entries.iter().find(|entry| entry.app_id == "500").unwrap();
        assert_eq!(owned_only.title.as_deref(), Some("zebra Crossing"));
        assert_eq!(owned_only.size_on_disk, None);
    }

    #[test]
    fn list_without_owned_data_keeps_the_installed_view() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "10", "Happy Game", 4, 1024);
        manifest(&root, "11", "Queued Game", 2, 0);
        let global = GlobalConfig::default();
        let failed = hidden();
        for owned in [OwnedGames::Unloaded, OwnedGames::Failed(&failed)] {
            let entries = listing(&paths, &global, owned).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].app_id, "10");
        }
    }

    #[test]
    fn an_owned_game_that_is_not_installed_imports_with_its_owned_title() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        let cache = root.join("appcache/librarycache/292030");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("library_600x900.jpg"), png()).unwrap();
        let global = GlobalConfig::default();
        let owned = owned_list(&[("292030", "The Witcher 3")]);
        let owned = OwnedGames::Loaded(&owned);

        let PlanOutcome::Ready(planned) = plan(&paths, &global, owned, "292030", None).unwrap()
        else {
            panic!("expected a plan");
        };
        assert_eq!(planned.title(), "The Witcher 3");
        assert_eq!(planned.name(), "the-witcher-3");
        assert_eq!(planned.state(), InstallState::NotInstalled);
        assert_eq!(planned.size_on_disk(), 0);

        let ApplyOutcome::Imported(applied) =
            apply(&paths, &global, owned, &planned, &Token::new()).unwrap()
        else {
            panic!("expected an import");
        };
        assert_eq!(applied.art, [Kind::Grid]);
        assert_eq!(
            fs::read_to_string(paths.environment_config("the-witcher-3")).unwrap(),
            "schema_version = 1\nkind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"292030\"\ntitle = \"The Witcher 3\"\ntags = [\"game\"]\ndesktop = false\n"
        );
    }

    #[test]
    fn a_queued_install_imports_through_the_owned_list() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "11", "Manifest Title", 2, 0);
        let global = GlobalConfig::default();
        let owned = owned_list(&[("11", "Owned Title")]);

        let planned = ready_with(&paths, &global, OwnedGames::Loaded(&owned), "11");

        assert_eq!(planned.title(), "Owned Title");
        assert_eq!(
            plan(&paths, &global, OwnedGames::Unloaded, "11", None)
                .unwrap_err()
                .to_string(),
            "Steam lists 11 as not installed."
        );
    }

    fn ready_with(paths: &Paths, global: &GlobalConfig, owned: OwnedGames<'_>, id: &str) -> Plan {
        match plan(paths, global, owned, id, None).unwrap() {
            PlanOutcome::Ready(plan) => plan,
            PlanOutcome::AlreadyImported(_) => panic!("app was already imported"),
        }
    }

    #[test]
    fn an_id_neither_installed_nor_owned_is_refused_and_a_failed_list_says_why() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        steam_root(&paths, false);
        let global = GlobalConfig::default();
        let owned = owned_list(&[("292030", "The Witcher 3")]);
        let empty = owned_list(&[]);
        for list in [&owned, &empty] {
            assert_eq!(
                plan(&paths, &global, OwnedGames::Loaded(list), "12345", None)
                    .unwrap_err()
                    .to_string(),
                "App 12345 isn't installed or owned."
            );
        }
        let failed = hidden();
        assert_eq!(
            plan(&paths, &global, OwnedGames::Failed(&failed), "12345", None)
                .unwrap_err()
                .to_string(),
            "Couldn't get owned games: the key can't see this library"
        );
        assert_eq!(
            plan(&paths, &global, OwnedGames::Unloaded, "12345", None)
                .unwrap_err()
                .to_string(),
            "Steam lists 12345 as not installed."
        );
        assert!(environment::names(&paths).unwrap().is_empty());
    }

    #[test]
    fn installed_games_import_with_no_owned_list_at_all() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "10", "Happy Game", 4, 1024);
        manifest(&root, "20", "Other Game", 4, 1024);
        let global = GlobalConfig::default();
        let failed = hidden();
        for (id, owned) in [
            ("10", OwnedGames::Unloaded),
            ("20", OwnedGames::Failed(&failed)),
        ] {
            let planned = ready_with(&paths, &global, owned, id);
            assert_eq!(planned.state(), InstallState::Installed);
            assert!(matches!(
                apply(&paths, &global, owned, &planned, &Token::new()).unwrap(),
                ApplyOutcome::Imported(_)
            ));
        }
    }

    #[test]
    fn apply_rechecks_ownership_under_the_lock_without_fetching() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        steam_root(&paths, false);
        let global = GlobalConfig::default();
        let owned = owned_list(&[("292030", "The Witcher 3")]);
        let planned = ready_with(&paths, &global, OwnedGames::Loaded(&owned), "292030");
        let other = owned_list(&[]);

        for list in [
            OwnedGames::Loaded(&other),
            OwnedGames::Unloaded,
            OwnedGames::Failed(&hidden()),
        ] {
            assert!(apply(&paths, &global, list, &planned, &Token::new()).is_err());
        }
        assert!(!paths.environment_config(planned.name()).exists());
    }

    #[test]
    fn an_unreadable_library_blocks_an_unowned_id_but_not_an_owned_one() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        let gone = temp.path().join("unmounted");
        fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\" {{ \"1\" {{ \"path\" \"{}\" }} }}",
                gone.display()
            ),
        )
        .unwrap();
        let global = GlobalConfig::default();
        let owned = owned_list(&[("292030", "The Witcher 3")]);

        assert_eq!(
            plan(&paths, &global, OwnedGames::Loaded(&owned), "5", None)
                .unwrap_err()
                .to_string(),
            format!("Steam's library at {} isn't available.", gone.display())
        );
        ready_with(&paths, &global, OwnedGames::Loaded(&owned), "292030");
    }

    #[test]
    fn needs_owned_only_when_nothing_on_disk_settles_the_game() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        manifest(&root, "10", "Happy Game", 4, 1024);
        manifest(&root, "11", "Queued Game", 2, 0);
        fs::write(
            root.join("steamapps/appmanifest_12.acf"),
            "\"AppState\" { \"appid\" \"12\"",
        )
        .unwrap();
        let global = GlobalConfig::default();

        assert!(!needs_owned(&paths, &global, "10"));
        assert!(needs_owned(&paths, &global, "11"));
        assert!(needs_owned(&paths, &global, "99"));
        // A manifest that can't be read is final, whatever the owned list says.
        assert!(!needs_owned(&paths, &global, "12"));

        let planned = ready(&paths, &global, "10", None);
        imported(&paths, &planned);
        assert!(!needs_owned(&paths, &global, "10"));
    }

    #[test]
    fn needs_owned_is_false_without_steam() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        assert!(!needs_owned(&paths, &GlobalConfig::default(), "99"));
    }

    #[test]
    fn list_carries_the_install_percent() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        fs::write(
            root.join("steamapps/appmanifest_13.acf"),
            "\"AppState\" { \"appid\" \"13\" \"name\" \"Downloading\" \"StateFlags\" \"1026\" \"BytesToDownload\" \"200\" \"BytesDownloaded\" \"50\" }",
        )
        .unwrap();
        manifest(&root, "14", "Done Game", 4, 1);

        let entries = listing(&paths, &GlobalConfig::default(), OwnedGames::Unloaded).unwrap();

        let progress: Vec<_> = entries
            .iter()
            .map(|entry| (entry.app_id.as_str(), entry.install_progress))
            .collect();
        assert_eq!(progress, [("13", Some(25)), ("14", None)]);
    }

    const STEAM_ID: u64 = 76_561_197_960_265_729;

    fn login(root: &Path, users: &[(u64, bool)]) {
        let body: String = users
            .iter()
            .map(|(id, recent)| format!("\"{id}\" {{ \"MostRecent\" \"{}\" }}", u8::from(*recent)))
            .collect();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/loginusers.vdf"),
            format!("\"users\" {{ {body} }}"),
        )
        .unwrap();
    }

    fn with_key(name: &str) -> Option<OsString> {
        (name == steam::key::KEY_VAR).then(|| OsString::from("SECRETKEY"))
    }

    fn cache(paths: &Paths, fetched_at: i64, games: &[(&str, &str)]) {
        let games: Vec<_> = games
            .iter()
            .map(|(app_id, name)| serde_json::json!({ "app_id": app_id, "name": name }))
            .collect();
        let path = paths.steam_owned_cache(STEAM_ID);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            serde_json::json!({ "fetched_at": fetched_at, "games": games }).to_string(),
        )
        .unwrap();
    }

    fn unreachable_api() -> Result<SteamApi, ApiError> {
        SteamApi::with_base(&steam::api::fixture::refused())
    }

    fn owned_data(paths: &Paths, refresh: bool) -> anyhow::Result<OwnedData> {
        OwnedData::load_with(
            paths,
            &GlobalConfig::default(),
            refresh,
            true,
            with_key,
            &crate::key_store::NoKeyring,
            unreachable_api,
        )
    }

    #[test]
    fn owned_data_without_a_key_never_asks_steam() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        login(&root, &[(STEAM_ID, true)]);
        let data = OwnedData::load_with(
            &paths,
            &GlobalConfig::default(),
            true,
            true,
            |_| None,
            &crate::key_store::NoKeyring,
            || panic!("no key means no request"),
        )
        .unwrap();
        assert!(matches!(data, OwnedData::NoKey));
        assert!(matches!(data.games(), OwnedGames::Unloaded));
        assert_eq!(data.problem(), None);
    }

    #[test]
    fn owned_data_uses_a_fresh_cache_and_falls_back_on_refresh_failures() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        login(&root, &[(STEAM_ID, true)]);

        let failed = owned_data(&paths, false).unwrap();
        assert!(matches!(failed.games(), OwnedGames::Failed(_)));
        assert_eq!(
            failed.problem().as_deref(),
            Some("Couldn't get owned games: can't reach Steam")
        );

        cache(
            &paths,
            jiff::Timestamp::now().as_second(),
            &[("20", "Owned")],
        );
        let fresh = owned_data(&paths, false).unwrap();
        let OwnedGames::Loaded(owned) = fresh.games() else {
            panic!("expected the cached list")
        };
        assert_eq!(owned.games[0].app_id, "20");
        assert_eq!(fresh.problem(), None);

        let stale = owned_data(&paths, true).unwrap();
        assert!(matches!(stale.games(), OwnedGames::Loaded(_)));
        assert_eq!(
            stale.problem().as_deref(),
            Some("Couldn't refresh owned games: can't reach Steam")
        );
    }

    #[test]
    fn owned_data_fails_when_no_account_is_most_recent() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path(), false);
        let root = steam_root(&paths, false);
        login(&root, &[(STEAM_ID, false), (STEAM_ID + 1, false)]);
        let error = owned_data(&paths, false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Steam doesn't say which account was used last"
        );
    }
}
