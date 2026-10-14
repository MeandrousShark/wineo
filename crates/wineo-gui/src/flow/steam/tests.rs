use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::Path;

use ::image::{ImageFormat, Rgba, RgbaImage};
use iced::keyboard::key::Named;
use tempfile::TempDir;
use wineo_core::steam_import::PlanOutcome;

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

fn manifest(root: &Path, app_id: &str, title: &str, flags: u32) {
    fs::write(
        root.join(format!("steamapps/appmanifest_{app_id}.acf")),
        format!(
            "\"AppState\" {{ \"appid\" \"{app_id}\" \"name\" \"{title}\" \"StateFlags\" \"{flags}\" \"installdir\" \"{title}\" \"SizeOnDisk\" \"1024\" }}"
        ),
    )
    .unwrap();
}

#[test]
fn page_import_flow_drops_stale_results() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let steam = temp.path().join("home/.local/share/Steam");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    manifest(&steam, "1", "Alpha", 4);
    let (mut page, _) = Page::new(paths.clone());
    let listed_generation = page.generation;

    let _ = page.update(Event::Listed(listed_generation - 1, Ok(Listing::default())));
    assert!(page.listing.is_none());
    assert!(page.waiting());
    let _ = page.update(Event::Listed(listed_generation, load_listing(&paths)));
    assert_eq!(page.step, Step::Games);
    assert!(!page.waiting());
    assert_eq!(page.listing.as_ref().unwrap().games.len(), 1);

    let _ = page.update(Event::Toggle(0));
    let _ = page.update(Event::ImportSelected);
    assert_eq!(page.step, Step::Details);
    assert_eq!(page.name, "alpha");
    let _ = page.update(Event::Review);
    let planned_generation = page.generation;
    assert!(page.waiting());
    let _ = page.update(Event::Planned(listed_generation, Review::default()));
    assert_eq!(page.step, Step::Details);
    assert!(page.review.is_none());
    assert!(page.waiting());
    let review = plan_review(
        &paths,
        page.listing.as_ref().unwrap().games.clone(),
        Some(page.name.clone()),
        &OwnedData::Unloaded,
    );
    let _ = page.update(Event::Planned(planned_generation, review));
    assert_eq!(page.step, Step::Review);
    assert!(!page.waiting());
    assert_eq!(page.review.as_ref().unwrap().ready.len(), 1);

    let _ = page.update(Event::Import);
    let finished_generation = page.generation;
    assert_eq!(page.step, Step::Working);
    assert!(page.busy());
    assert!(page.waiting());
    let _ = page.update(Event::Finished(planned_generation, Ok(Report::default())));
    assert_eq!(page.step, Step::Working);
    assert!(page.busy());
    assert!(page.report.is_none());
    let _ = page.update(Event::Progress(
        finished_generation,
        "Importing Alpha...".into(),
    ));
    assert_eq!(page.status, "Importing Alpha...");
    let report = run_import(
        &paths,
        page.review.clone().unwrap(),
        &OwnedData::Unloaded,
        page.token.as_ref().unwrap(),
        |_| {},
    );
    let _ = page.update(Event::Finished(finished_generation, report));
    assert_eq!(page.step, Step::Finished);
    assert!(!page.busy());
    assert!(!page.waiting());
    assert_eq!(
        report_lines(page.report.as_ref().unwrap()),
        ["Imported 1 of 1 game."]
    );
    assert!(paths.environment_config("alpha").is_file());
    let _ = page.update(Event::BackToLibrary);
    assert!(matches!(
        page.take_action(),
        Some(Action::Library {
            name: None,
            launch: false
        })
    ));
}

#[test]
fn page_ignores_progress_after_finished() {
    let temp = TempDir::new().unwrap();
    let (mut page, _) = Page::new(paths(temp.path(), false));
    page.step = Step::Working;
    page.token = Some(Token::new());
    let generation = page.generation;
    let _ = page.update(Event::Finished(generation, Ok(Report::default())));
    let _ = page.update(Event::Progress(generation, "Importing Alpha...".into()));
    assert_eq!(page.step, Step::Finished);
    assert!(!page.busy());
    assert!(!page.waiting());
    assert!(page.status.is_empty());
    assert!(page.report.is_some());
}

#[test]
fn page_does_not_import_an_empty_or_already_imported_review() {
    let temp = TempDir::new().unwrap();
    for already in [Vec::new(), vec!["alpha".to_owned()]] {
        let (mut page, _) = Page::new(paths(temp.path(), false));
        let _ = page.update(Event::Planned(
            page.generation,
            Review {
                already: already.clone(),
                ..Review::default()
            },
        ));
        let generation = page.generation;
        let _ = page.submit();
        let _ = page.update(Event::Import);
        assert_eq!(page.step, Step::Review);
        assert_eq!(page.generation, generation);
        assert!(!page.busy());
        assert!(!page.waiting());
        assert!(page.report.is_none());
        assert_eq!(page.review.as_ref().unwrap().already, already);
    }
}

#[test]
fn lists_only_unimported_installed_games_and_keeps_healthy_manifests() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let steam = temp.path().join("home/.local/share/Steam");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    manifest(&steam, "1", "Alpha", 4);
    manifest(&steam, "2", "Beta", 4);
    manifest(&steam, "3", "Gamma", 0);
    fs::write(steam.join("steamapps/appmanifest_4.acf"), "broken").unwrap();
    let global = GlobalConfig::load(&paths).unwrap();
    let PlanOutcome::Ready(plan) =
        steam_import::plan(&paths, &global, OwnedData::Unloaded.games(), "1", None).unwrap()
    else {
        panic!("expected ready plan")
    };
    steam_import::apply(
        &paths,
        &global,
        OwnedData::Unloaded.games(),
        &plan,
        &Token::new(),
    )
    .unwrap();

    let listing = load_listing(&paths).unwrap();
    assert_eq!(listing.games.len(), 1);
    assert_eq!(listing.games[0].app_id, "2");
    assert_eq!(listing.issues.len(), 1);
    assert!(listing.issues[0].contains("4"));
}

#[test]
fn notes_tell_installing_from_updating() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let steam = temp.path().join("home/.local/share/Steam");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    manifest(&steam, "1", "Alpha", 4);
    manifest(&steam, "2", "Beta", 1028);
    manifest(&steam, "3", "Gamma", 1026);
    let notes: Vec<_> = load_listing(&paths)
        .unwrap()
        .games
        .iter()
        .map(|game| game.note)
        .collect();
    assert_eq!(
        notes,
        [None, Some("Updating in Steam"), Some("Installing in Steam"),]
    );
}

#[test]
fn plans_and_imports_selected_games_with_cli_summary() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let steam = temp.path().join("home/.local/share/Steam");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    manifest(&steam, "1", "Alpha", 4);
    manifest(&steam, "2", "Beta", 4);
    let cache = steam.join("appcache/librarycache/1");
    fs::create_dir_all(&cache).unwrap();
    let mut art = Cursor::new(Vec::new());
    RgbaImage::from_pixel(2, 2, Rgba([17, 23, 42, 255]))
        .write_to(&mut art, ImageFormat::Png)
        .unwrap();
    fs::write(cache.join("library_600x900.jpg"), art.into_inner()).unwrap();
    let listing = load_listing(&paths).unwrap();
    assert_eq!(
        listing.games[0].poster.as_deref(),
        Some(cache.join("library_600x900.jpg").as_path())
    );
    let review = plan_review(&paths, listing.games, None, &OwnedData::Unloaded);
    assert_eq!(review.ready.len(), 2);
    let report = run_import(&paths, review, &OwnedData::Unloaded, &Token::new(), |_| {}).unwrap();
    assert_eq!(report_lines(&report), ["Imported 2 of 2 games."]);
    assert!(paths.environment_config("alpha").is_file());
    assert!(paths.environment_config("beta").is_file());
    assert!(paths.art_dir("alpha").join("grid.png").is_file());
}

fn listed(count: usize) -> Listing {
    Listing {
        games: (0..count)
            .map(|index| Candidate {
                app_id: index.to_string(),
                title: format!("Game {index}"),
                size: 1,
                note: None,
                installed: true,
                poster: None,
            })
            .collect(),
        ..Listing::default()
    }
}

#[test]
fn every_step_sits_inside_the_dialog_padding() {
    use crate::layout_probe::{bounds_at, probe, within};
    let temp = TempDir::new().unwrap();
    let (mut page, _) = Page::new(paths(temp.path(), false));
    page.status.clear();
    let mut listing = listed(40);
    listing.games[0].title =
        "Grand Theft Auto IV: The Complete Edition and Episodes from Liberty City".to_owned();
    listing.games[3].installed = false;
    listing.games[3].note = Some("Updating in Steam");
    listing.issues = vec!["A Steam library folder couldn't be read.".to_owned()];
    page.listing = Some(listing);
    page.grid.chosen = ["0", "3", "7"].map(str::to_owned).into();
    page.name = "grand-theft-auto-iv-the-complete-edition".to_owned();
    let failure = Failure {
        app_id: "12210".to_owned(),
        name: "grand-theft-auto-iv-the-complete-edition".to_owned(),
        reason: "Steam no longer lists the install folder for this game, so it can't be imported"
            .to_owned(),
    };
    page.review = Some(Review {
        ready: Vec::new(),
        already: vec!["portal-2".to_owned()],
        failures: vec![failure.clone()],
    });
    page.report = Some(Report {
        done: vec!["half-life-2".to_owned()],
        total: 3,
        already: vec!["portal-2".to_owned()],
        failures: vec![failure],
        warnings: vec!["Desktop entry: the menu folder couldn't be written".to_owned()],
        canceled: true,
    });
    // The smallest dialog the window allows, a small one, and the widest.
    for (width, height) in [(280.0, 240.0), (360.0, 280.0), (900.0, 720.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        for step in [
            Step::Games,
            Step::Details,
            Step::Review,
            Step::Working,
            Step::Finished,
        ] {
            page.step = step;
            page.token = (step == Step::Working).then(Token::new);
            let mut view = page.view(Palette::DARK, width);
            let probe = probe(&mut view, iced::Size::new(width, height));
            let node = &probe.node;
            let spilled = probe.outside(inset);
            assert_eq!(probe.scrollables.len(), 1, "one scrolling region");
            assert!(
                spilled.is_empty(),
                "{step:?} at {width}x{height}: {spilled:?}"
            );
            let back = bounds_at(node, &[0, 0, 0]);
            assert!(within(back, inset), "{step:?}: {back:?}");
            if step != Step::Games {
                let footer = bounds_at(node, &[0, 2, 0]);
                assert!(within(footer, inset), "{step:?}: {footer:?}");
            }
        }
    }
}

#[test]
fn a_short_dialog_keeps_the_actions_in_frame_and_the_grid_scrolling() {
    use crate::layout_probe::{bounds_at, probe, within};
    let temp = TempDir::new().unwrap();
    let (mut page, _) = Page::new(paths(temp.path(), false));
    page.status.clear();
    page.listing = Some(listed(40));
    page.step = Step::Games;
    for (width, height) in [(280.0, 240.0), (360.0, 200.0), (900.0, 240.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view(Palette::DARK, width);
        let probe = probe(&mut view, iced::Size::new(width, height));
        assert_eq!(probe.scrollables.len(), 1, "one scrolling region");
        let (viewport, content) = probe.scrollables[0];
        assert!(viewport.height > 0.0, "{width}x{height}: {viewport:?}");
        assert!(
            (content.y - viewport.y).abs() < 0.01 && content.height > viewport.height,
            "{width}x{height}: {viewport:?} {content:?}"
        );
        let actions = bounds_at(&probe.node, &[0, 1, 1]);
        assert!(within(actions, inset), "{width}x{height}: {actions:?}");
        assert!(
            actions.y >= viewport.y + viewport.height - 0.01,
            "{width}x{height}: {actions:?} under {viewport:?}"
        );
    }
}

#[test]
fn only_keyboard_focus_draws_the_ring() {
    let temp = TempDir::new().unwrap();
    let (mut page, _) = Page::new(paths(temp.path(), false));
    page.status.clear();
    page.listing = Some(listed(3));
    let none = keyboard::Modifiers::empty();
    let _ = page.update(Event::Toggle(1));
    assert_eq!(page.grid.focus, Some(1));
    assert!(!page.grid.focus_ring(1));
    let _ = page.key(&Key::Named(Named::ArrowRight), none, 900.0);
    assert!(page.grid.focus_ring(2));
    let _ = page.update(Event::Toggle(0));
    assert!(!page.grid.focus_ring(0));
    let _ = page.key(&Key::Named(Named::Space), none, 900.0);
    assert!(page.grid.focus_ring(0));
}

#[test]
fn mouse_back_and_reopening_show_no_ring_until_a_key() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let none = keyboard::Modifiers::empty();
    let (mut page, _) = Page::new(paths.clone());
    page.status.clear();
    page.listing = Some(listed(3));
    let _ = page.key(&Key::Named(Named::ArrowRight), none, 900.0);
    assert!(page.grid.focus_ring(0));
    page.step = Step::Details;
    let _ = page.update(Event::Back);
    assert_eq!(page.step, Step::Games);
    assert!(!page.grid.focus_ring(0));
    let _ = page.key(&Key::Named(Named::ArrowRight), none, 900.0);
    assert!(page.grid.focus_ring(1));

    // Leaving from the grid closes the page, and Add opens a fresh one.
    let mut flow = super::super::Page::Choose(paths.clone());
    let _ = flow.update(super::super::Event::Steam, false);
    let super::super::Page::Steam(reopened) = &mut flow else {
        panic!("expected the Steam page");
    };
    reopened.listing = Some(listed(3));
    reopened.status.clear();
    assert!((0..3).all(|index| !reopened.grid.focus_ring(index)));
    let _ = flow.key(&Key::Named(Named::ArrowDown), none, 900.0);
    let super::super::Page::Steam(reopened) = &flow else {
        unreachable!()
    };
    assert!(reopened.grid.focus_ring(0));
}

#[test]
fn grid_keys_toggle_and_select_every_listed_game() {
    let temp = TempDir::new().unwrap();
    let (mut page, _) = Page::new(paths(temp.path(), false));
    page.status.clear();
    page.listing = Some(Listing {
        games: vec![
            Candidate {
                app_id: "1".to_owned(),
                title: "Alpha".to_owned(),
                size: 1,
                note: None,
                installed: true,
                poster: None,
            },
            Candidate {
                app_id: "2".to_owned(),
                title: "Beta".to_owned(),
                size: 2,
                note: Some("Updating in Steam"),
                installed: true,
                poster: None,
            },
        ],
        ..Listing::default()
    });
    let none = keyboard::Modifiers::empty();
    let key = |page: &mut Page, key: Key, modifiers| page.key(&key, modifiers, 900.0);
    assert!(key(&mut page, Key::Named(Named::ArrowRight), none).is_some());
    assert_eq!(page.grid.focus, Some(0));
    let _ = key(&mut page, Key::Named(Named::Space), none);
    assert_eq!(
        page.grid
            .chosen
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["1"]
    );
    let _ = key(&mut page, Key::Named(Named::ArrowRight), none);
    let _ = key(&mut page, Key::Named(Named::Space), none);
    assert_eq!(
        page.grid
            .chosen
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
    let _ = page.update(Event::SelectNone);
    let _ = key(
        &mut page,
        Key::Character("a".into()),
        keyboard::Modifiers::CTRL,
    );
    assert_eq!(
        page.grid
            .chosen
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert!(key(&mut page, Key::Named(Named::Enter), none).is_some());
    assert_eq!(page.status, "Checking Steam games...");
}

#[test]
fn planning_failure_is_listed_but_excluded_from_cli_total() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let steam = temp.path().join("home/.local/share/Steam");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    manifest(&steam, "1", "Alpha", 4);
    manifest(&steam, "2", "Alpha", 4);
    let review = plan_review(
        &paths,
        load_listing(&paths).unwrap().games,
        None,
        &OwnedData::Unloaded,
    );
    assert_eq!(review.ready.len(), 1);
    assert_eq!(review.failures.len(), 1);
    let report = run_import(&paths, review, &OwnedData::Unloaded, &Token::new(), |_| {}).unwrap();
    assert_eq!(
        report_lines(&report),
        ["Imported 1 of 1 game.", "1 failed: alpha"]
    );
}

#[test]
fn apply_time_imports_are_reported_without_counting_them_twice() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let steam = temp.path().join("home/.local/share/Steam");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    manifest(&steam, "1", "Alpha", 4);
    manifest(&steam, "2", "Beta", 4);
    let review = plan_review(
        &paths,
        load_listing(&paths).unwrap().games,
        None,
        &OwnedData::Unloaded,
    );
    let global = GlobalConfig::load(&paths).unwrap();
    steam_import::apply(
        &paths,
        &global,
        wineo_core::steam_import::OwnedGames::Unloaded,
        &review.ready[0],
        &Token::new(),
    )
    .unwrap();

    let report = run_import(&paths, review, &OwnedData::Unloaded, &Token::new(), |_| {}).unwrap();
    assert_eq!(
        report_lines(&report),
        ["Imported 1 of 1 game.", "1 already imported: alpha"]
    );
}

#[test]
fn portable_mode_uses_the_cli_refusal() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), true);
    assert_eq!(
        load_listing(&paths).unwrap_err(),
        "Steam games can't be imported into a portable folder."
    );
}

#[test]
fn summary_keeps_apply_time_skips_out_of_the_total() {
    let report = Report {
        done: vec!["alpha".to_owned()],
        total: 2,
        already: vec!["beta".to_owned()],
        failures: vec![Failure {
            app_id: "3".to_owned(),
            name: "gamma".to_owned(),
            reason: "broken manifest".to_owned(),
        }],
        ..Report::default()
    };
    assert_eq!(
        report_lines(&report),
        [
            "Imported 1 of 2 games.",
            "1 already imported: beta",
            "1 failed: gamma"
        ]
    );
}

const STEAM_ID: u64 = 76_561_197_960_265_729;

fn login(steam: &Path, users: &[(u64, bool)]) {
    let body: String = users
        .iter()
        .map(|(id, recent)| format!("\"{id}\" {{ \"MostRecent\" \"{}\" }}", u8::from(*recent)))
        .collect();
    fs::create_dir_all(steam.join("config")).unwrap();
    fs::write(
        steam.join("config/loginusers.vdf"),
        format!("\"users\" {{ {body} }}"),
    )
    .unwrap();
}

fn owned_cache(paths: &Paths, fetched_at: i64, games: &[(&str, &str)]) {
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

/// Steam's API at an address nothing listens on.
fn unreachable() -> Result<wineo_core::steam::api::SteamApi, wineo_core::steam::api::ApiError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    wineo_core::steam::api::SteamApi::with_base(&base)
}

fn owned_with_key(
    key: bool,
) -> impl FnOnce(&Paths, &GlobalConfig, bool) -> anyhow::Result<OwnedData> {
    move |paths, global, refresh| {
        OwnedData::load_with(
            paths,
            global,
            refresh,
            true,
            |name| {
                (key && name == wineo_core::steam::key::KEY_VAR)
                    .then(|| OsString::from("SECRETKEY"))
            },
            &wineo_core::key_store::NoKeyring,
            unreachable,
        )
    }
}

/// A Steam root with Alpha installed and an account to look up.
fn owned_fixture(temp: &TempDir) -> (Paths, PathBuf) {
    let paths = paths(temp.path(), false);
    let steam = temp.path().join("home/.local/share/Steam");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    manifest(&steam, "1", "Alpha", 4);
    login(&steam, &[(STEAM_ID, true)]);
    (paths, steam)
}

fn listed_page(paths: &Paths) -> Page {
    let (mut page, _) = Page::new(paths.clone());
    let generation = page.generation;
    let _ = page.update(Event::Listed(generation, load_listing(paths)));
    assert!(
        page.owned_loading,
        "the owned list loads once the page lists"
    );
    assert!(page.spinning());
    assert!(!page.waiting(), "installed games stay usable meanwhile");
    page
}

#[test]
fn owned_games_are_listed_after_installed_ones_dimmed_and_import() {
    let temp = TempDir::new().unwrap();
    let (paths, steam) = owned_fixture(&temp);
    let now = jiff::Timestamp::now().as_second();
    owned_cache(
        &paths,
        now,
        &[
            ("3", "Gamma"),
            ("1", "Alpha"),
            ("2", "Beta"),
            ("1493710", "Proton Experimental"),
        ],
    );
    let cache = steam.join("appcache/librarycache/2");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("library_600x900.jpg"), b"art").unwrap();

    let mut page = listed_page(&paths);
    let _ = page.update(Event::Toggle(0));
    let result = load_owned(&paths, false, owned_with_key(true));
    let _ = page.update(Event::OwnedListed(page.owned_generation, result));
    assert!(!page.owned_loading);
    assert_eq!(page.owned_note, None);
    let games = page.listing.as_ref().unwrap().games.clone();
    let shown: Vec<_> = games
        .iter()
        .map(|game| (game.app_id.as_str(), game.installed))
        .collect();
    assert_eq!(shown, [("1", true), ("2", false), ("3", false)]);
    assert_eq!(
        games[1].poster.as_deref(),
        Some(cache.join("library_600x900.jpg").as_path())
    );
    assert_eq!(games[1].note, None);
    // The installed game picked before the list landed stays picked.
    assert!(page.grid.chosen.contains("1"));

    let _ = page.update(Event::SelectAll);
    assert_eq!(page.grid.chosen.len(), 3);
    let selected = games.clone();
    let review = plan_review(&paths, selected, None, page.owned.as_ref().unwrap());
    assert_eq!(review.ready.len(), 3, "{:?}", review.failures);
    assert_eq!(review.ready[1].state(), InstallState::NotInstalled);
    let report = run_import(
        &paths,
        review,
        page.owned.as_ref().unwrap(),
        &Token::new(),
        |_| {},
    )
    .unwrap();
    assert_eq!(report_lines(&report), ["Imported 3 of 3 games."]);
    let config = fs::read_to_string(paths.environment_config("beta")).unwrap();
    assert!(config.contains("app_id = \"2\""));
    assert!(config.contains("title = \"Beta\""));
}

#[test]
fn without_a_key_installed_games_show_with_a_link_to_settings() {
    let temp = TempDir::new().unwrap();
    let (paths, _) = owned_fixture(&temp);
    let mut page = listed_page(&paths);
    let result = load_owned(&paths, false, owned_with_key(false));
    let _ = page.update(Event::OwnedListed(page.owned_generation, result));
    assert_eq!(page.owned_note, Some(OwnedNote::NoKey));
    assert_eq!(
        NO_KEY_HINT,
        "Add a Steam Web API key to see uninstalled games."
    );
    assert_eq!(page.listing.as_ref().unwrap().games.len(), 1);
    assert!(!page.refresh_available());
    let _ = page.update(Event::Refresh);
    assert!(!page.owned_loading);

    let _ = page.update(Event::OpenSteamKey);
    assert!(matches!(page.take_action(), Some(Action::SteamKey)));
}

#[test]
fn a_failed_fetch_without_a_cache_lists_installed_games_and_says_why() {
    let temp = TempDir::new().unwrap();
    let (paths, _) = owned_fixture(&temp);
    let mut page = listed_page(&paths);
    let result = load_owned(&paths, false, owned_with_key(true));
    let _ = page.update(Event::OwnedListed(page.owned_generation, result));
    assert_eq!(
        page.owned_note,
        Some(OwnedNote::Problem(
            "Couldn't get owned games: can't reach Steam".to_owned()
        ))
    );
    assert_eq!(page.listing.as_ref().unwrap().games.len(), 1);
    assert!(page.refresh_available());
}

#[test]
fn refresh_forces_a_fetch_and_falls_back_to_the_cache() {
    let temp = TempDir::new().unwrap();
    let (paths, _) = owned_fixture(&temp);
    let now = jiff::Timestamp::now().as_second();
    owned_cache(&paths, now, &[("2", "Beta")]);
    let mut page = listed_page(&paths);
    let first = page.owned_generation;
    let result = load_owned(&paths, false, owned_with_key(true));
    let _ = page.update(Event::OwnedListed(first, result));
    assert_eq!(page.owned_note, None);

    let _ = page.update(Event::Refresh);
    assert!(page.owned_loading);
    assert_ne!(page.owned_generation, first);
    // An answer from before the refresh can't land on top of it.
    let _ = page.update(Event::OwnedListed(first, Err("stale".to_owned())));
    assert!(page.owned_loading);
    let result = load_owned(&paths, true, owned_with_key(true));
    let _ = page.update(Event::OwnedListed(page.owned_generation, result));
    assert_eq!(
        page.owned_note,
        Some(OwnedNote::Problem(
            "Couldn't refresh owned games: can't reach Steam".to_owned()
        ))
    );
    let games = &page.listing.as_ref().unwrap().games;
    assert_eq!(games.len(), 2);
    assert!(!games[1].installed);
}

#[test]
fn several_accounts_with_none_most_recent_show_that_error() {
    let temp = TempDir::new().unwrap();
    let (paths, steam) = owned_fixture(&temp);
    login(&steam, &[(STEAM_ID, false), (STEAM_ID + 1, false)]);
    let mut page = listed_page(&paths);
    let result = load_owned(&paths, false, owned_with_key(true));
    let _ = page.update(Event::OwnedListed(page.owned_generation, result));
    assert_eq!(
        page.owned_note,
        Some(OwnedNote::Problem(
            "Steam doesn't say which account was used last".to_owned()
        ))
    );
    assert_eq!(page.listing.as_ref().unwrap().games.len(), 1);
    assert!(page.owned.is_none());
}

#[test]
fn art_progress_counts_posters_after_the_delay_and_ignores_stale_ones() {
    let temp = TempDir::new().unwrap();
    let (paths, steam) = owned_fixture(&temp);
    let cache = steam.join("appcache/librarycache/1");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("library_600x900.jpg"), b"art").unwrap();
    let mut page = listed_page(&paths);
    let wanted = page.posters_to_load(page.listing.as_ref().unwrap());
    let total = wanted.len();
    assert!(total >= 1);

    assert_eq!(page.grid.progress.shown(), None, "not before the delay");
    assert!(page.grid.progress.pending_wakeup().is_some());
    let _ = page.update(Event::ProgressDue(page.grid.progress.wakeup_generation()));
    assert_eq!(page.grid.progress.shown(), Some((0, total)));
    crate::layout_probe::layout(
        &mut page.view(Palette::DARK, 680.0),
        iced::Size::new(680.0, 560.0),
    );

    let generation = page.grid.poster_generation;
    let _ = page.update(Event::Poster(
        generation.wrapping_sub(1),
        wanted[0].0.clone(),
        Err("late".into()),
    ));
    assert_eq!(page.grid.progress.shown(), Some((0, total)));
    for (key, _) in wanted {
        let _ = page.update(Event::Poster(generation, key, Err("bad".into())));
    }
    assert_eq!(page.grid.progress.shown(), None, "hidden at completion");
    assert_eq!(page.grid.progress.pending_wakeup(), None);
}

#[test]
fn art_progress_never_shows_when_the_posters_finish_before_the_delay() {
    let temp = TempDir::new().unwrap();
    let (paths, steam) = owned_fixture(&temp);
    let cache = steam.join("appcache/librarycache/1");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("library_600x900.jpg"), b"art").unwrap();
    let mut page = listed_page(&paths);
    let wanted = page.posters_to_load(page.listing.as_ref().unwrap());
    let late = page.grid.progress.wakeup_generation();
    let generation = page.grid.poster_generation;
    for (key, _) in wanted {
        let _ = page.update(Event::Poster(generation, key, Err("bad".into())));
    }
    assert_eq!(page.grid.progress.pending_wakeup(), None);
    let _ = page.update(Event::ProgressDue(late));
    assert_eq!(page.grid.progress.shown(), None);
}

#[test]
fn refresh_reloads_posters_whose_install_state_flipped_and_retries_failures() {
    let temp = TempDir::new().unwrap();
    let (paths, steam) = owned_fixture(&temp);
    let now = jiff::Timestamp::now().as_second();
    owned_cache(&paths, now, &[("1", "Alpha"), ("2", "Beta")]);
    for app_id in ["1", "2"] {
        let cache = steam.join(format!("appcache/librarycache/{app_id}"));
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("library_600x900.jpg"), b"art").unwrap();
    }
    let keys = |page: &Page| {
        let mut keys: Vec<_> = page
            .posters_to_load(page.listing.as_ref().unwrap())
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        keys.sort();
        keys
    };
    let handle = || image::Handle::from_rgba(1, 1, vec![10, 80, 240, 255]);

    let mut page = listed_page(&paths);
    let result = load_owned(&paths, false, owned_with_key(true));
    let _ = page.update(Event::OwnedListed(page.owned_generation, result));
    assert_eq!(
        keys(&page),
        [("1".to_owned(), false), ("2".to_owned(), true)]
    );
    let generation = page.grid.poster_generation;
    let _ = page.update(Event::Poster(
        generation,
        ("1".into(), false),
        Err("bad".into()),
    ));
    let _ = page.update(Event::Poster(generation, ("2".into(), true), Ok(handle())));
    assert_eq!(keys(&page), [("1".to_owned(), false)]);

    // Steam installs Beta behind wineo's back.
    manifest(&steam, "2", "Beta", 4);
    let _ = page.update(Event::Refresh);
    let result = load_owned(&paths, true, owned_with_key(true));
    let _ = page.update(Event::OwnedListed(page.owned_generation, result));
    let games = &page.listing.as_ref().unwrap().games;
    assert!(games.iter().all(|game| game.installed));
    // The gray Beta poster isn't reused, and the failed Alpha one is retried.
    assert_eq!(
        keys(&page),
        [("1".to_owned(), false), ("2".to_owned(), false)]
    );
    assert!(!page.posters.contains_key(&("2".to_owned(), false)));
}
