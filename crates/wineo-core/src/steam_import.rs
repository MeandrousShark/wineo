//! Planning and registering installed Steam games as proxied environments.

use std::collections::HashSet;
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
use crate::operation::Locks;
use crate::paths::Paths;
use crate::steam::{self, Install, InstallState, Manifest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryState {
    Installed,
    Updating,
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub app_id: String,
    pub title: Option<String>,
    pub size_on_disk: Option<u64>,
    pub state: EntryState,
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

/// Lists importable games and malformed manifests from the active install.
/// Tool ids are hidden even when their manifests cannot be read.
pub fn listing(paths: &Paths, global: &GlobalConfig) -> anyhow::Result<Vec<Entry>> {
    check_portable(paths)?;
    let install = active_install(paths, global)?;
    let imported = imported_games(paths)?;
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for item in steam::manifests(&install) {
        if !seen.insert(item.app_id.clone()) || steam::is_tool(&item.app_id, "") {
            continue;
        }
        let imported_as = imported
            .iter()
            .find(|(app_id, _)| app_id == &item.app_id)
            .map(|(_, name)| name.clone());
        match item.manifest {
            Ok(manifest) if !manifest.is_tool() => {
                let state = match manifest.state() {
                    InstallState::Installed => EntryState::Installed,
                    InstallState::Updating => EntryState::Updating,
                    InstallState::NotInstalled => continue,
                };
                entries.push(Entry {
                    app_id: item.app_id,
                    title: Some(manifest.name),
                    size_on_disk: Some(manifest.size_on_disk),
                    state,
                    imported_as,
                });
            }
            Ok(_) => {}
            Err(error) => entries.push(Entry {
                app_id: item.app_id,
                title: None,
                size_on_disk: None,
                state: EntryState::Unreadable(error),
                imported_as,
            }),
        }
    }
    Ok(entries)
}

/// Resolves one Steam app id without writing anything. A previously imported
/// app is a skip, regardless of which name the caller requested for it.
pub fn plan(
    paths: &Paths,
    global: &GlobalConfig,
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
    let manifest = manifest_for(&install, app_id)?;
    if manifest.is_tool() {
        anyhow::bail!("Steam lists {app_id} as a tool.");
    }
    if manifest.state() == InstallState::NotInstalled {
        anyhow::bail!("Steam lists {app_id} as not installed.");
    }
    let name = name
        .map(str::to_owned)
        .unwrap_or_else(|| default_name(&manifest.name, None));
    environment::check_new_name(paths, &name)?;
    let state = manifest.state();
    Ok(PlanOutcome::Ready(Plan {
        app_id: app_id.to_owned(),
        title: manifest.name,
        name,
        size_on_disk: manifest.size_on_disk,
        state,
    }))
}

/// Creates the config, then copies any valid Steam cached art. Art errors are
/// returned as warnings and never undo the registered environment.
pub fn apply(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &Plan,
    cancel: &Token,
) -> anyhow::Result<ApplyOutcome> {
    check_portable(paths)?;
    crate::import::check_cancelled(cancel)?;
    let locks = Locks::new(paths);
    let _app_guard = locks.steam_import(&plan.app_id)?;
    crate::import::check_cancelled(cancel)?;
    if let Some((_, existing)) = imported_games(paths)?
        .into_iter()
        .find(|(id, _)| id == &plan.app_id)
    {
        return Ok(ApplyOutcome::AlreadyImported(existing));
    }
    let install = active_install(paths, global)?;
    let manifest = manifest_for(&install, &plan.app_id)?;
    if manifest.state() == InstallState::NotInstalled {
        anyhow::bail!("Steam lists {} as not installed.", plan.app_id);
    }
    environment::check_new_name(paths, &plan.name)?;
    let path = paths.environment_config(&plan.name);
    let mut document = config::new_document();
    document["kind"] = value("proxied");
    document["launcher"] = value(Launcher::Steam.as_str());
    document["app_id"] = value(&plan.app_id);
    document["title"] = value(&manifest.name);
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

fn manifest_for(install: &Install, app_id: &str) -> anyhow::Result<Manifest> {
    if let Some(item) = steam::manifests(install)
        .into_iter()
        .find(|item| item.app_id == app_id)
    {
        return item.manifest.map_err(anyhow::Error::msg);
    }
    match steam::app_state(Some(install), app_id) {
        steam::AppState::Unavailable(path) => {
            anyhow::bail!("Steam's library at {} isn't available.", path.display())
        }
        steam::AppState::Unknown(reason) => anyhow::bail!("{reason}."),
        _ => anyhow::bail!("Steam lists {app_id} as not installed."),
    }
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
        match plan(paths, global, app_id, name).unwrap() {
            PlanOutcome::Ready(plan) => plan,
            PlanOutcome::AlreadyImported(_) => panic!("app was already imported"),
        }
    }

    fn imported(paths: &Paths, plan: &Plan) -> Applied {
        match apply(paths, &GlobalConfig::default(), plan, &Token::new()).unwrap() {
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
        let entries = listing(&paths, &GlobalConfig::default()).unwrap();
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
            listing(&paths, &global).unwrap()[0].imported_as.as_deref(),
            Some("custom")
        );
        assert!(matches!(
            plan(&paths, &global, "42", Some("another")).unwrap(),
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
            plan(&paths, &global, "2", None).unwrap_err().to_string(),
            "Steam lists 2 as not installed."
        );
        assert_eq!(
            plan(&paths, &global, "99", None).unwrap_err().to_string(),
            "Steam lists 99 as not installed."
        );
        assert_eq!(
            plan(&paths, &global, "3", None).unwrap_err().to_string(),
            "Steam lists 3 as a tool."
        );
        assert_eq!(
            plan(&paths, &global, "1", Some("one-game"))
                .unwrap()
                .already_imported(),
            Some("one-game")
        );
        manifest(&root, "4", "Four Game", 4, 4);
        assert_eq!(
            plan(&paths, &global, "4", Some("one-game"))
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

        let applied = match apply(&paths, &global, &planned, &Token::new()).unwrap() {
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
        assert!(apply(&paths, &GlobalConfig::default(), &planned, &token).is_err());
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

        let outcome = apply(&paths, &global, &planned, &Token::new()).unwrap();

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
                apply(&paths, &global, &planned, &Token::new())
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
            apply(&paths, &global, &planned, &Token::new())
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
                        apply(&paths, &global, plan, &Token::new()).unwrap()
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
            apply(&paths, &global, &planned, &Token::new())
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
            listing(&paths, &GlobalConfig::default())
                .unwrap_err()
                .to_string(),
            "Steam wasn't found."
        );
        let native = steam_root(&paths, false);
        let flatpak = steam_root(&paths, true);
        manifest(&native, "1", "Native Game", 4, 1);
        manifest(&flatpak, "2", "Flatpak Game", 4, 2);
        let mut global = GlobalConfig::default();
        assert_eq!(listing(&paths, &global).unwrap()[0].app_id, "1");
        assert_eq!(
            plan(&paths, &global, "2", None).unwrap_err().to_string(),
            "Steam lists 2 as not installed."
        );
        global.steam = Some(steam::InstallKind::Flatpak);
        assert_eq!(listing(&paths, &global).unwrap()[0].app_id, "2");
        let portable = self::paths(temp.path(), true);
        assert_eq!(
            listing(&portable, &global).unwrap_err().to_string(),
            "Steam games can't be imported into a portable folder."
        );
        assert_eq!(
            plan(&portable, &global, "2", None).unwrap_err().to_string(),
            "Steam games can't be imported into a portable folder."
        );
    }
}
