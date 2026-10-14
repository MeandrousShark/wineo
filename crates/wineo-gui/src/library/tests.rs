use super::*;

fn target(desktop: bool, name: Option<&str>) -> Target {
    Target {
        exe: "app.exe".into(),
        args: Vec::new(),
        cwd: None,
        default: false,
        gamemode: None,
        wayland: None,
        mangohud: None,
        gamescope: None,
        hooks: None,
        desktop,
        name: name.map(str::to_owned),
    }
}

#[test]
fn display_title_uses_only_the_default_targets_explicit_desktop_name() {
    let targets = BTreeMap::from([
        ("app".to_owned(), target(true, Some("My App"))),
        ("tool".to_owned(), target(true, Some("Setup Tool"))),
    ]);

    assert_eq!(display_title("env_id", &targets, Some("app")), "My App");
    assert_eq!(
        display_title("env_id", &targets, Some("tool")),
        "Setup Tool"
    );
}

#[test]
fn display_title_falls_back_for_missing_disabled_or_unnamed_targets() {
    let targets = BTreeMap::from([
        ("no-entry".to_owned(), target(false, Some("Ignored"))),
        ("no-title".to_owned(), target(true, None)),
        ("empty-title".to_owned(), target(true, Some(""))),
    ]);

    for default in [
        None,
        Some("missing"),
        Some("no-entry"),
        Some("no-title"),
        Some("empty-title"),
    ] {
        assert_eq!(display_title("env_id", &targets, default), "env_id");
    }
}

#[test]
fn missing_stats_use_the_kind_specific_fallback() {
    let never = ("never played".into(), "0".into(), "never".into());
    assert_eq!(fallback_stats(Some(CardKind::Wine)), never);
    assert_eq!(fallback_stats(Some(CardKind::Native)), never);
    assert_eq!(fallback_stats(None), never);
    assert_eq!(
        fallback_stats(Some(CardKind::Proxied)),
        ("unknown".into(), "-".into(), "unknown".into())
    );
}

#[test]
fn launch_button_label_uses_the_exact_game_tag() {
    let card = |tags: Vec<&str>| Card {
        name: "env_id".into(),
        kind: None,
        display_title: "My App".into(),
        steam: None,
        state: State::Inactive,
        lost: false,
        prefix: None,
        busy: false,
        busy_holder: None,
        unresolved: false,
        error: None,
        playtime: "never played".into(),
        sessions: "0".to_owned(),
        last_played: "never".into(),
        tags: tags.into_iter().map(str::to_owned).collect(),
        targets: Vec::new(),
        default_target: None,
        art: None,
        panel_art: HashMap::new(),
        gog: None,
    };

    assert_eq!(card(vec!["game"]).launch_button_label(), "Play");
    assert_eq!(card(vec!["Game"]).launch_button_label(), "Launch");
    assert_eq!(card(vec!["work"]).launch_button_label(), "Launch");
}

#[test]
fn cards_know_their_kind() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    for (name, text) in [
        (
            "bg3",
            "kind = 'proxied'\nlauncher = 'steam'\napp_id = '1086940'\n",
        ),
        ("dusk", "kind = 'native'\n"),
        (
            "notes",
            "prefix = '/tmp/notes'\nrunner = 'wine'\narch = 'win64'\n",
        ),
        ("broken", "not toml ["),
    ] {
        fs::write(paths.environment_config(name), text).unwrap();
    }
    let cards = load_library(&paths).unwrap();
    let kind = |name: &str| cards.iter().find(|card| card.name == name).unwrap().kind;
    assert_eq!(kind("bg3"), Some(CardKind::Proxied));
    assert_eq!(kind("dusk"), Some(CardKind::Native));
    assert_eq!(kind("notes"), Some(CardKind::Wine));
    assert_eq!(kind("broken"), None);
    let bg3 = cards.iter().find(|card| card.name == "bg3").unwrap();
    assert!(bg3.prefix.is_none());
    assert_eq!(bg3.default_target.as_deref(), Some("steam"));
}

#[test]
fn steam_cards_read_live_state_title_size_proton_and_implicit_art() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let steam = root.path().join(".local/share/Steam");
    let steamapps = steam.join("steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::create_dir_all(steam.join("config")).unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(paths.environment_config("game"),
        "kind = 'proxied'\nlauncher = 'steam'\napp_id = '123'\ntitle = 'Saved title'\ntags = ['game']\n").unwrap();
    fs::write(steam.join("config/config.vdf"),
        r#""InstallConfigStore" { "Software" { "Valve" { "Steam" { "CompatToolMapping" { "123" { "name" "proton_experimental" } } } } } }"#).unwrap();
    let art_dir = paths.art_dir("game");
    fs::create_dir_all(&art_dir).unwrap();
    for name in ["grid.png", "hero.png", "logo.png"] {
        fs::write(art_dir.join(name), b"art").unwrap();
    }
    for (flags, state, playable, dimmed) in [
        (4, "installed", true, false),
        (1030, "updating", true, false),
        (0, "not installed", false, true),
        (2, "install queued", false, true),
    ] {
        fs::write(steamapps.join("appmanifest_123.acf"), format!(
            r#""AppState" {{ "appid" "123" "name" "Live title" "installdir" "Game" "StateFlags" "{flags}" "SizeOnDisk" "2048" }}"#)).unwrap();
        let cards = load_library(&paths).unwrap();
        let card = &cards[0];
        assert_eq!(card.display_title, "Live title");
        assert_eq!(
            card.state_word(),
            if state == "installed" {
                "inactive"
            } else {
                state
            }
        );
        assert_eq!(card.install_allows_play(), playable);
        assert_eq!(card.not_installed(), dimmed);
        assert!(card.install_warning().is_none());
        let details = card.steam.as_ref().unwrap();
        assert_eq!(details.size, Some(2048));
        assert_eq!(details.proton.as_deref(), Some("proton_experimental"));
        assert_eq!(card.targets, ["steam"]);
        assert!(card.art.is_some());
        assert!(card.panel_art["steam"].hero.is_some());
        assert!(card.panel_art["steam"].logo.is_some());
    }
    fs::remove_file(steamapps.join("appmanifest_123.acf")).unwrap();
    let card = load_library(&paths).unwrap().remove(0);
    assert_eq!(card.display_title, "Saved title");
    assert_eq!(card.state_word(), "not installed");
    assert!(card.not_installed());
    assert!(!card.install_allows_play());
    assert_eq!(card.steam.unwrap().size, None);

    fs::write(
        steamapps.join("libraryfolders.vdf"),
        format!(
            r#""libraryfolders" {{ "1" {{ "path" "{}" }} }}"#,
            root.path().join("missing-library").display()
        ),
    )
    .unwrap();
    let card = load_library(&paths).unwrap().remove(0);
    assert_eq!(card.state_word(), "unavailable");
    assert!(!card.not_installed());
    assert!(!card.install_allows_play());
    assert!(card.install_warning().unwrap().contains("isn't available"));

    fs::write(steamapps.join("appmanifest_123.acf"), "broken {").unwrap();
    let card = load_library(&paths).unwrap().remove(0);
    assert_eq!(card.state_word(), "unknown");
    assert!(!card.not_installed());
    assert!(!card.install_allows_play());
    assert!(card.install_warning().is_some());
}

#[test]
fn steam_install_progress_flows_from_the_manifest_to_the_card() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let steamapps = root.path().join(".local/share/Steam/steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("game"),
        "kind = 'proxied'\nlauncher = 'steam'\napp_id = '123'\ntags = ['game']\n",
    )
    .unwrap();
    let card = |flags: u32, bytes: &str| {
        fs::write(
            steamapps.join("appmanifest_123.acf"),
            format!(
                r#""AppState" {{ "appid" "123" "name" "Game" "StateFlags" "{flags}" {bytes} }}"#
            ),
        )
        .unwrap();
        load_library(&paths).unwrap().remove(0)
    };

    let installing = card(1026, r#""BytesToDownload" "200" "BytesDownloaded" "50""#);
    assert_eq!(installing.install_progress(), Some(25));
    assert_eq!(installing.state_word(), "installing");
    assert_eq!(installing.state_label(), "installing 25%");
    assert!(!installing.install_allows_play());
    assert!(!installing.installable());
    assert!(!installing.uninstallable());

    // Steam hasn't written totals yet, or the install is paused.
    let unknown = card(1026, r#""BytesToDownload" "0" "BytesDownloaded" "0""#);
    assert_eq!(unknown.install_progress(), None);
    assert_eq!(unknown.state_label(), "installing");

    let queued = card(2, "");
    assert_eq!(queued.install_progress(), None);
    assert_eq!(queued.state_label(), "install queued");
    assert!(queued.installable());
    assert!(!queued.uninstallable());

    let installed = card(4, "");
    assert_eq!(installed.install_progress(), None);
    assert_eq!(installed.state_label(), "inactive");
    assert!(!installed.installable());
    assert!(installed.uninstallable());
    assert!(card(1030, "").uninstallable());

    fs::remove_file(steamapps.join("appmanifest_123.acf")).unwrap();
    let missing = load_library(&paths).unwrap().remove(0);
    assert_eq!(missing.state_label(), "not installed");
    assert!(missing.installable());
    assert!(!missing.uninstallable());
}

#[test]
fn load_library_reads_each_trash_manifest_once() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    for index in 0..5 {
        let name = format!("gone-{index}");
        fs::write(paths.environment_config(&name), "kind = 'native'\n").unwrap();
        wineo_core::trash::remove(&paths, &name, false).unwrap();
    }
    for index in 0..4 {
        fs::write(
            paths.environment_config(&format!("kept-{index}")),
            "kind = 'native'\n",
        )
        .unwrap();
    }

    wineo_core::trash::take_manifest_reads();
    let cards = load_library(&paths).unwrap();
    assert_eq!(cards.len(), 4);
    assert!(cards.iter().all(|card| card.kind == Some(CardKind::Native)));
    assert_eq!(wineo_core::trash::take_manifest_reads(), 5);
}

#[test]
fn load_library_never_reads_the_owned_list() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let steam = root.path().join(".local/share/Steam");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    fs::create_dir_all(steam.join("config")).unwrap();
    let steam_id = 76_561_197_960_265_729_u64;
    fs::write(
        steam.join("config/loginusers.vdf"),
        format!(r#""users" {{ "{steam_id}" {{ "MostRecent" "1" }} }}"#),
    )
    .unwrap();
    // A saved key and a stale owned list naming the game. Loading the list
    // would retitle the card from it, or fetch and rewrite the cache.
    let key = wineo_core::steam::key::key_file(&paths);
    fs::create_dir_all(key.parent().unwrap()).unwrap();
    fs::write(&key, "SECRET\n").unwrap();
    let cache = paths.steam_owned_cache(steam_id);
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    let text = r#"{"fetched_at":1,"games":[{"app_id":"123","name":"From the owned list"}]}"#;
    fs::write(&cache, text).unwrap();
    let modified = fs::metadata(&cache).unwrap().modified().unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("game"),
        "kind = 'proxied'\nlauncher = 'steam'\napp_id = '123'\n",
    )
    .unwrap();

    let card = load_library(&paths).unwrap().remove(0);
    assert_eq!(card.display_title, "game");
    assert_eq!(card.state_word(), "not installed");
    assert_eq!(fs::read_to_string(&cache).unwrap(), text);
    assert_eq!(fs::metadata(&cache).unwrap().modified().unwrap(), modified);
    assert_eq!(fs::read_dir(cache.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn steam_card_compatibility_tool_lookup_ignores_case() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let steam = root.path().join(".local/share/Steam");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    fs::create_dir_all(steam.join("config")).unwrap();
    fs::create_dir_all(paths.environments_dir()).unwrap();
    fs::write(
        paths.environment_config("game"),
        "kind = 'proxied'\nlauncher = 'steam'\napp_id = 'AbC'\n",
    )
    .unwrap();
    fs::write(
        steam.join("config/config.vdf"),
        r#""InstallConfigStore" { "Software" { "Valve" { "Steam" { "CompatToolMapping" { "aBc" { "name" "GE-Proton" } "0" { "name" "proton_default" } } } } } }"#,
    )
    .unwrap();

    let cards = load_library(&paths).unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(
        cards[0].steam.as_ref().unwrap().proton.as_deref(),
        Some("GE-Proton")
    );
}

#[test]
fn missing_art_file_has_no_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("grid.png");
    fs::write(&path, b"first").unwrap();
    let art = ArtFile::resolve(path.clone()).unwrap();
    assert_eq!(art.identity.len, 5);
    fs::remove_file(&path).unwrap();
    assert!(ArtFile::resolve(path).is_none());
}
