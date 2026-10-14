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

    let PlanOutcome::Ready(planned) = plan(&paths, &global, owned, "292030", None).unwrap() else {
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

#[test]
fn a_new_config_never_replaces_one_already_there() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("environments/game.toml");
    write_new(&path, "kind = 'native'\n").unwrap();
    assert!(write_new(&path, "kind = 'other'\n").is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "kind = 'native'\n");
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

fn wanted(app_id: &str, title: Option<&str>, imported_as: Option<&str>) -> Wanted {
    Wanted {
        app_id: app_id.to_owned(),
        title: title.map(str::to_owned),
        imported_as: imported_as.map(str::to_owned),
    }
}

fn events(planned: Planned<'_>) -> String {
    match planned {
        Planned::Ready(plan) => format!("ready {}", plan.name()),
        Planned::AlreadyImported(name) => format!("already {name}"),
    }
}

#[test]
fn plan_batch_reports_in_order_and_fails_a_repeated_name() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path(), false);
    let root = steam_root(&paths, false);
    manifest(&root, "1", "Alpha", 4, 1);
    manifest(&root, "2", "Alpha", 4, 1);
    manifest(&root, "3", "Done Game", 4, 1);
    let global = GlobalConfig::default();
    imported(&paths, &ready(&paths, &global, "3", Some("done")));

    let mut seen = Vec::new();
    let batch = plan_batch(
        &paths,
        &global,
        OwnedGames::Unloaded,
        [
            wanted("1", Some("Alpha"), None),
            wanted("9", Some("Gone Game"), Some("listed")),
            wanted("2", Some("Alpha"), None),
            wanted("3", Some("Done Game"), None),
            wanted("4", Some("Gone Game"), None),
            wanted("5", None, None),
        ],
        None,
        |planned| {
            seen.push(events(planned));
            Ok(())
        },
    )
    .unwrap();

    let alpha = default_name("Alpha", None);
    assert_eq!(
        seen,
        [
            format!("ready {alpha}"),
            "already listed".to_owned(),
            "already done".to_owned()
        ]
    );
    assert_eq!(batch.ready.len(), 1);
    assert_eq!(batch.ready[0].app_id(), "1");
    assert_eq!(batch.already, ["listed", "done"]);
    let failures: Vec<_> = batch
        .failures
        .iter()
        .map(|failure| (failure.app_id.as_str(), failure.name.as_str()))
        .collect();
    let gone = default_name("Gone Game", None);
    assert_eq!(
        failures,
        [("2", alpha.as_str()), ("4", gone.as_str()), ("5", "5")]
    );
    assert_eq!(
        batch.failures[0].reason,
        format!("environment `{alpha}` already exists")
    );
}

#[test]
fn plan_batch_names_a_failure_by_the_wanted_name() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path(), false);
    steam_root(&paths, false);
    let batch = plan_batch(
        &paths,
        &GlobalConfig::default(),
        OwnedGames::Unloaded,
        [wanted("4", Some("Gone Game"), None)],
        Some("mine"),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(batch.failures[0].name, "mine");
}

#[test]
fn a_callback_error_stops_planning() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path(), false);
    let root = steam_root(&paths, false);
    manifest(&root, "1", "One", 4, 1);
    manifest(&root, "2", "Two", 4, 1);
    let mut calls = 0;
    let error = plan_batch(
        &paths,
        &GlobalConfig::default(),
        OwnedGames::Unloaded,
        [wanted("1", None, None), wanted("2", None, None)],
        None,
        |_| {
            calls += 1;
            anyhow::bail!("stop")
        },
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "stop");
    assert_eq!(calls, 1);
}

fn two_ready(paths: &Paths) -> Batch {
    let root = steam_root(paths, false);
    manifest(&root, "1", "One", 4, 1);
    manifest(&root, "2", "Two", 4, 1);
    let global = GlobalConfig::default();
    Batch {
        ready: vec![
            ready(paths, &global, "1", Some("one")),
            ready(paths, &global, "2", Some("two")),
        ],
        already: vec!["planned".to_owned()],
        failures: vec![Failure {
            app_id: "0".to_owned(),
            name: "zero".to_owned(),
            reason: "planning failed".to_owned(),
        }],
    }
}

#[test]
fn apply_batch_reports_in_order_and_leaves_skips_out_of_the_total() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path(), false);
    let batch = two_ready(&paths);
    let global = GlobalConfig::default();
    imported(&paths, &ready(&paths, &global, "2", Some("elsewhere")));

    let mut seen = Vec::new();
    let report = apply_batch(
        &paths,
        &global,
        OwnedGames::Unloaded,
        batch,
        &Token::new(),
        |applying| {
            seen.push(match applying {
                Applying::Starting(plan) => format!("start {}", plan.name()),
                Applying::Imported(plan, applied) => {
                    format!("imported {} {}", plan.name(), applied.environment.name)
                }
                Applying::AlreadyImported(name) => format!("already {name}"),
            });
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(
        seen,
        [
            "start one",
            "imported one one",
            "start two",
            "already elsewhere"
        ]
    );
    assert_eq!(report.done, ["one"]);
    assert_eq!(report.total, 1);
    assert_eq!(report.already, ["elsewhere"]);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].name, "zero");
    assert!(!report.canceled);
}

#[test]
fn a_callback_error_stops_applying() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path(), false);
    let batch = two_ready(&paths);
    let error = apply_batch(
        &paths,
        &GlobalConfig::default(),
        OwnedGames::Unloaded,
        batch,
        &Token::new(),
        |applying| match applying {
            Applying::Imported(..) => anyhow::bail!("stop"),
            _ => Ok(()),
        },
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "stop");
    assert!(paths.environment_config("one").exists());
    assert!(!paths.environment_config("two").exists());
}

#[test]
fn cancellation_stops_at_the_next_game_and_keeps_what_finished() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path(), false);
    let batch = two_ready(&paths);
    let token = Token::new();
    let report = apply_batch(
        &paths,
        &GlobalConfig::default(),
        OwnedGames::Unloaded,
        batch.clone(),
        &token,
        |applying| {
            if let Applying::Imported(..) = applying {
                token.cancel();
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(report.canceled);
    assert_eq!(report.done, ["one"]);
    assert_eq!(report.total, 2);
    assert!(!paths.environment_config("two").exists());

    let report = apply_batch(
        &paths,
        &GlobalConfig::default(),
        OwnedGames::Unloaded,
        Batch {
            ready: batch.ready[1..].to_vec(),
            ..Batch::default()
        },
        &token,
        |_| panic!("nothing starts once canceled"),
    )
    .unwrap();
    assert!(report.canceled);
    assert!(report.done.is_empty());
    assert!(!paths.environment_config("two").exists());
}
