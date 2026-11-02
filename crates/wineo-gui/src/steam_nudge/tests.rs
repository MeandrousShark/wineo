use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::steam_import::{self, OwnedData, PlanOutcome};

use super::*;
use crate::library::load_library;
use crate::rescan::Scan;

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    (root, App::boot(paths).0)
}

fn cards(app: &App) -> Result<Vec<crate::library::Card>, String> {
    load_library(&app.paths).map_err(|error| error.to_string())
}

/// An installed Steam game, in a Steam folder under the temporary home.
fn steam_game(root: &tempfile::TempDir, id: &str, title: &str) {
    let steamapps = root.path().join(".local/share/Steam/steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::write(
        steamapps.join(format!("appmanifest_{id}.acf")),
        format!(r#""AppState" {{ "appid" "{id}" "name" "{title}" "StateFlags" "4" }}"#),
    )
    .unwrap();
}

fn scan(app: &App) -> Scan {
    crate::rescan::read(&app.paths, &app.shown, false, None)
}

/// What a periodic rescan does: reports what differs from the screen, and the
/// app applies it.
fn rescan(app: &mut App) -> Option<Scan> {
    let changes = app.shown.changes(scan(app), jiff::Timestamp::now())?;
    let _ = app.update(Message::Rescanned(Box::new(changes.clone())));
    Some(changes)
}

/// The app with Steam baselined on `old` and nothing pending, after its
/// first load of `names` as native environments.
fn baselined(names: &[&str]) -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    for name in names {
        let config = app.paths.environment_config(name);
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(config, "schema_version = 1\nkind = 'native'\n").unwrap();
    }
    let _ = app.update(Message::Loaded(cards(&app)));
    steam_game(&root, "1", "Old");
    let proposed = scan(&app).discovery.unwrap().proposed;
    saved::apply_changes(&app.paths, &proposed).unwrap();
    app.shown.refresh_library(&app.paths);
    let settled = scan(&app).discovery.unwrap();
    let _ = app.discovery_read(settled);
    (root, app)
}

fn pending(app: &App) -> Vec<&str> {
    app.steam_nudge
        .pending()
        .iter()
        .map(|game| game.app_id.as_str())
        .collect()
}

fn draw(app: &App) {
    let _ = crate::layout_probe::layout(&mut app.view(), app.window_size);
}

fn bounds(app: &App, id: &'static str) -> Option<iced::Rectangle> {
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    let found = Arc::new(Mutex::new(None));
    let sink = found.clone();
    laid.run(Box::new(iced::advanced::widget::operation::map(
        crate::library_menu::bounds_of(id),
        move |bounds| *sink.lock().unwrap() = bounds,
    )));
    *found.lock().unwrap()
}

fn shown(app: &App) -> bool {
    bounds(app, IMPORT_ID).is_some() && bounds(app, DISMISS_ID).is_some()
}

/// Imports `id` as the Steam review would.
fn import(app: &App, id: &str) {
    let global = GlobalConfig::load(&app.paths).unwrap();
    let owned = OwnedData::Unloaded;
    let PlanOutcome::Ready(plan) =
        steam_import::plan(&app.paths, &global, owned.games(), id, None).unwrap()
    else {
        panic!("{id} was already imported");
    };
    steam_import::apply(&app.paths, &global, owned.games(), &plan, &Token::new()).unwrap();
}

/// What the app does when a flow returns to the Library.
fn flow_returns(app: &mut App) {
    let discovery = scan(app).discovery.unwrap();
    let _ = app.update(Message::FlowRead(
        app.flow_generation,
        crate::flow::Action::Library {
            name: None,
            launch: false,
        },
        Box::new(discovery),
        cards(app),
    ));
}

/// Runs the write a dismissal started, and tells the app how it went.
fn write_dismissal(app: &mut App, ids: &[String]) -> Result<(), String> {
    let result = dismiss(&app.paths, &app.shown, ids);
    let _ = app.update(Message::NudgeDismissed(ids.to_vec(), result.clone()));
    result
}

fn ids(of: &[&str]) -> Vec<String> {
    of.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
fn the_words_name_one_game_and_count_several() {
    let game = |id: &str, title: &str| PendingGame {
        app_id: id.into(),
        title: title.into(),
    };
    assert_eq!(words(&[game("1", "Hades")]), "1 new Steam game: Hades");
    assert_eq!(
        words(&[game("1", "Hades"), game("2", "Celeste")]),
        "2 new Steam games"
    );
}

#[test]
fn the_banner_shows_while_something_is_pending_and_only_then() {
    let (root, mut app) = baselined(&["game"]);
    assert!(app.steam_banner().is_none());
    assert!(!shown(&app));

    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();
    assert_eq!(pending(&app), ["2"]);
    assert!(shown(&app));
    draw(&app);

    // Imported elsewhere, it stops being offered on the next scan.
    import(&app, "2");
    rescan(&mut app).unwrap();
    assert!(pending(&app).is_empty());
    assert!(!shown(&app));
}

#[test]
fn an_empty_library_still_shows_the_offer() {
    let (root, mut app) = baselined(&[]);
    assert_eq!(
        app.empty_state(),
        Some(crate::library_states::Empty::Library)
    );
    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();
    assert!(shown(&app));
    assert_eq!(
        app.empty_state(),
        Some(crate::library_states::Empty::Library)
    );
    draw(&app);
}

#[test]
fn a_new_install_with_identical_cards_updates_the_banner() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    let changes = rescan(&mut app).unwrap();
    assert!(changes.library.is_none(), "no card changed");
    assert!(shown(&app));
}

#[test]
fn it_sits_below_the_other_banners_and_hides_with_an_unusable_library_toml() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();
    app.trash_badge.unreadable = vec![None];
    let trash = bounds(&app, crate::library_states::TRASH_CHECKUP_ID).unwrap();
    let nudge = bounds(&app, IMPORT_ID).unwrap();
    assert!(trash.y < nudge.y, "{trash:?} {nudge:?}");

    // A file that can't be used can't record a choice, so nothing is offered.
    let path = app.paths.library_file();
    fs::write(&path, "[environments.game\n").unwrap();
    rescan(&mut app).unwrap();
    assert!(app.library_problem.is_some());
    assert!(!shown(&app));
    assert!(bounds(&app, crate::library_states::CHECKUP_ID).is_some());
}

#[test]
fn the_buttons_take_keyboard_focus_in_order_before_the_grid() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    let ids: Vec<String> = crate::focus_button::focusables(&mut laid)
        .into_iter()
        .flatten()
        .collect();
    let at = |id: &str| ids.iter().position(|it| it.contains(id)).unwrap();
    assert!(at(IMPORT_ID) < at(DISMISS_ID), "{ids:?}");
    assert!(at(crate::grid::ADD_ID) < at(IMPORT_ID), "{ids:?}");

    let pressed = crate::library_states::tests::press_control(&app, IMPORT_ID);
    assert!(matches!(pressed.as_slice(), [Message::NudgeImport(ids)] if ids == &["2"]));
    let pressed = crate::library_states::tests::press_control(&app, DISMISS_ID);
    assert!(matches!(pressed.as_slice(), [Message::NudgeDismiss(ids)] if ids == &["2"]));
}

#[test]
fn import_opens_the_review_for_the_banners_games() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    steam_game(&root, "3", "Celeste");
    rescan(&mut app).unwrap();
    let before = app.flow_generation;
    let offered = ids(&["2", "3"]);

    let _ = app.update(Message::NudgeImport(offered.clone()));
    assert_ne!(app.flow_generation, before);
    assert_eq!(
        app.flow.as_ref().and_then(crate::flow::Page::steam_ids),
        Some(offered.as_slice())
    );
}

#[test]
fn import_does_nothing_while_something_else_is_open() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();
    app.flow = Some(crate::flow::Page::choose(app.paths.clone()));
    let generation = app.flow_generation;
    let _ = app.update(Message::NudgeImport(ids(&["2"])));
    assert_eq!(app.flow_generation, generation);
    assert!(app.flow.as_ref().unwrap().steam_ids().is_none());
}

#[test]
fn a_cancelled_review_leaves_the_games_offered() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();
    let _ = app.update(Message::NudgeImport(ids(&["2"])));
    let generation = app.flow_generation;

    // Back from the games leaves for the Library, which reads it all again.
    let _ = app.update(Message::Flow(generation, crate::flow::Event::Back));
    assert!(app.flow.is_none());
    flow_returns(&mut app);
    assert_eq!(pending(&app), ["2"]);
    assert!(shown(&app));
    assert!(!saved::load(&app.paths).steam.contains("2"));
}

#[test]
fn a_successful_import_clears_its_game_and_a_partial_one_leaves_the_rest() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    steam_game(&root, "3", "Celeste");
    rescan(&mut app).unwrap();
    assert_eq!(pending(&app), ["2", "3"]);

    import(&app, "2");
    flow_returns(&mut app);
    assert_eq!(pending(&app), ["3"]);
    assert_eq!(
        words(app.steam_nudge.pending()),
        "1 new Steam game: Celeste"
    );
    assert!(shown(&app));
    assert_eq!(
        saved::load(&app.paths).steam.games.get("2"),
        Some(&saved::Recorded::Imported)
    );
    assert!(!saved::load(&app.paths).steam.contains("3"));

    import(&app, "3");
    flow_returns(&mut app);
    assert!(pending(&app).is_empty());
    assert!(!shown(&app));
}

#[test]
fn dismiss_records_every_game_and_clears_the_banner_without_a_scan() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    steam_game(&root, "3", "Celeste");
    rescan(&mut app).unwrap();
    let shown_ids = ids(&["2", "3"]);

    let _ = app.update(Message::NudgeDismiss(shown_ids.clone()));
    assert!(app.steam_nudge.writing(), "buttons wait for the write");
    assert!(bounds(&app, DISMISS_ID).is_none());
    write_dismissal(&mut app, &shown_ids).unwrap();

    assert!(!app.steam_nudge.writing());
    assert!(pending(&app).is_empty());
    assert!(!shown(&app));
    let record = saved::load(&app.paths).steam;
    for id in ["2", "3"] {
        assert_eq!(record.games.get(id), Some(&saved::Recorded::Dismissed));
    }
    // Nothing left to tell the app about.
    assert!(
        app.shown
            .changes(scan(&app), jiff::Timestamp::now())
            .is_none()
    );
}

#[test]
fn a_game_that_arrives_after_the_banner_was_drawn_is_not_dismissed_by_it() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();
    // The press, as the banner was drawn.
    let pressed = crate::library_states::tests::press_control(&app, DISMISS_ID);

    steam_game(&root, "3", "Celeste");
    rescan(&mut app).unwrap();
    assert_eq!(pending(&app), ["2", "3"]);

    for message in pressed {
        let _ = app.update(message);
    }
    write_dismissal(&mut app, &ids(&["2"])).unwrap();
    assert_eq!(pending(&app), ["3"]);
    assert!(shown(&app));
    let record = saved::load(&app.paths).steam;
    assert!(record.contains("2"));
    assert!(!record.contains("3"));
}

#[test]
fn a_dismissal_keeps_a_baseline_and_a_decision_made_elsewhere() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();
    // Another window imported it between the scan and the press.
    saved::finish_import(&app.paths, "2").unwrap();
    write_dismissal(&mut app, &ids(&["2", "1"])).unwrap();
    let record = saved::load(&app.paths).steam;
    assert_eq!(record.games.get("2"), Some(&saved::Recorded::Imported));
    assert_eq!(record.games.get("1"), Some(&saved::Recorded::Baseline));
}

#[test]
fn a_failed_dismissal_keeps_the_banner_and_says_so_once_without_retrying() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();

    let dir = app.paths.library_file().parent().unwrap().to_owned();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let _ = app.update(Message::NudgeDismiss(ids(&["2"])));
    let failed = write_dismissal(&mut app, &ids(&["2"]));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(failed.is_err());

    assert!(!app.steam_nudge.writing());
    assert_eq!(pending(&app), ["2"]);
    assert!(shown(&app));
    let notice = app.feature_notice.take().expect("the failure shows");
    assert!(notice.0.contains("dismiss"), "{}", notice.0);
    assert!(!saved::load(&app.paths).steam.contains("2"));
    // It ends there: no task is waiting to try again.
    let task = app.nudge_dismissed(&ids(&["2"]), Err("boom".into()));
    assert_eq!(task.units(), 0);
}

#[test]
fn a_dismissal_from_another_process_clears_the_banner_on_the_next_scan() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();
    assert!(shown(&app));

    saved::dismiss_game(&app.paths, "2").unwrap();
    rescan(&mut app).unwrap();
    assert!(pending(&app).is_empty());
    assert!(!shown(&app));
}

#[test]
fn an_older_read_cant_bring_a_dismissed_game_back() {
    let (root, mut app) = baselined(&["game"]);
    steam_game(&root, "2", "Hades");
    rescan(&mut app).unwrap();
    // A scan read before the write lands after it.
    let stale = scan(&app);
    assert_eq!(stale.discovery.as_ref().unwrap().pending.len(), 1);
    write_dismissal(&mut app, &ids(&["2"])).unwrap();
    let _ = app.update(Message::Rescanned(Box::new(stale)));
    assert!(pending(&app).is_empty());
}

#[test]
fn nothing_is_offered_without_steam_or_in_portable_mode() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::Loaded(cards(&app)));
    rescan(&mut app);
    assert!(pending(&app).is_empty());
    assert!(!shown(&app));

    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let portable = root.path().join("portable");
    let paths = Paths::select(
        {
            let home = home.clone();
            move |key| (key == "HOME").then(|| home.as_os_str().to_owned())
        },
        Some(&portable),
    )
    .unwrap();
    let mut app = App::boot(paths).0;
    let _ = app.update(Message::Loaded(Ok(Vec::new())));
    let steamapps = home.join(".local/share/Steam/steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::write(
        steamapps.join("appmanifest_5.acf"),
        r#""AppState" { "appid" "5" "name" "Five" "StateFlags" "4" }"#,
    )
    .unwrap();
    rescan(&mut app);
    assert!(pending(&app).is_empty());
    assert!(!shown(&app));
}
