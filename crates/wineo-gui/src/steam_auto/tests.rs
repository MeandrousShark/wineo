use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use wineo_core::library::{self as saved, Recorded};

use super::*;
use crate::library::{Loaded, load_library_and_problem};
use crate::rescan::Scan;

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    (root, App::boot(paths).0)
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

/// What an explicit reload does when its read lands.
fn reload(app: &mut App) {
    let (loaded, problem) = load_library_and_problem(&app.paths);
    let Loaded { cards, discovery } = loaded.unwrap();
    let generation = app.library_generation;
    let _ = app.update(Message::LibraryScanned(
        generation,
        Box::new(discovery),
        Ok(cards),
        problem,
    ));
}

/// What a periodic rescan does.
fn rescan(app: &mut App) -> Option<Scan> {
    let changes = app.shown.changes(scan(app), jiff::Timestamp::now())?;
    let _ = app.update(Message::Rescanned(Box::new(changes.clone())));
    Some(changes)
}

/// The app with Steam baselined on game 1, nothing pending, and the setting
/// as asked.
fn settled(on: bool) -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    steam_game(&root, "1", "Old");
    reload(&mut app);
    saved::apply_changes(&app.paths, &scan(&app).discovery.unwrap().proposed).unwrap();
    app.shown.refresh_library(&app.paths);
    saved::set_auto_import(&app.paths, on).unwrap();
    app.shown.refresh_library(&app.paths);
    reload(&mut app);
    assert!(!app.steam_auto.running());
    (root, app)
}

/// Runs the pass `discovery_read` started, and lands its result. The task was
/// dropped with the update, which is why this does its work by hand.
fn finish(app: &mut App, token: &Token) {
    let discovery = scan(app).discovery.unwrap();
    let report = run_pass(&app.paths, &app.shown, &discovery.pending, token);
    let _ = app.update(Message::AutoImported(Ok(report)));
}

fn offered(app: &App) -> Vec<String> {
    app.steam_offer()
        .unwrap_or_default()
        .iter()
        .map(|game| game.app_id.clone())
        .collect()
}

fn reason(app: &App, id: &str) -> Option<Recorded> {
    saved::load(&app.paths).steam.games.get(id).copied()
}

fn card_names(app: &App) -> Vec<String> {
    app.cards.iter().map(|card| card.name.clone()).collect()
}

fn taken(app: &App, name: &str) {
    let config = app.paths.environment_config(name);
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(config, "schema_version = 1\nkind = 'native'\n").unwrap();
}

fn read_only<T>(app: &App, body: impl FnOnce() -> T) -> T {
    let dir = app.paths.library_file().parent().unwrap().to_owned();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let result = body();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    result
}

#[test]
fn a_new_install_is_imported_and_never_reaches_the_banner() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    let found = rescan(&mut app).expect("a new install is news");
    assert_eq!(found.discovery.unwrap().pending.len(), 1);
    assert!(app.steam_auto.running());
    assert!(offered(&app).is_empty(), "the scan that found it");

    finish(&mut app, &Token::new());
    assert!(!app.steam_auto.running());
    assert!(offered(&app).is_empty(), "the finish");
    assert!(app.refreshing, "a read follows, for the new card");
    reload(&mut app);
    assert!(offered(&app).is_empty(), "the read after");
    assert!(!app.steam_auto.running());
    assert_eq!(reason(&app, "2"), Some(Recorded::Imported));
    assert_eq!(card_names(&app).len(), 1);
    assert!(app.feature_notice.is_none());
}

#[test]
fn the_first_load_holds_a_pending_game_from_the_first_frame() {
    let (root, _app) = settled(true);
    steam_game(&root, "2", "New");
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    let mut next = App::boot(paths).0;
    reload(&mut next);
    assert!(next.steam_auto.running());
    assert!(offered(&next).is_empty());
}

#[test]
fn off_starts_nothing_and_writes_nothing() {
    let (root, mut app) = settled(false);
    steam_game(&root, "2", "New");
    let before = fs::read(app.paths.library_file()).unwrap();
    rescan(&mut app).unwrap();
    assert!(!app.steam_auto.running());
    assert_eq!(offered(&app), ["2"]);
    assert_eq!(fs::read(app.paths.library_file()).unwrap(), before);
}

#[test]
fn turning_it_on_takes_what_is_pending_and_leaves_the_baseline() {
    let (root, mut app) = settled(false);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    assert_eq!(offered(&app), ["2"]);

    saved::set_auto_import(&app.paths, true).unwrap();
    app.shown.refresh_library(&app.paths);
    let change = rescan(&mut app).expect("the setting alone reaches the app");
    assert!(change.library.is_none());
    assert!(app.steam_auto.running());
    assert!(offered(&app).is_empty());

    finish(&mut app, &Token::new());
    reload(&mut app);
    assert_eq!(reason(&app, "2"), Some(Recorded::Imported));
    assert_eq!(reason(&app, "1"), Some(Recorded::Baseline));
    assert_eq!(card_names(&app).len(), 1, "game 1 stays out");
}

#[test]
fn a_setting_turned_off_before_the_claim_wins_over_the_scan() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    assert!(app.steam_auto.running());
    let token = Token::new();
    let discovery = app.shown.clone();
    let games = scan(&app).discovery.unwrap().pending;

    saved::set_auto_import(&app.paths, false).unwrap();
    let report = run_pass(&app.paths, &discovery, &games, &token);
    assert_eq!(report, Report::default());
    assert_eq!(reason(&app, "2"), None);
    let _ = app.update(Message::AutoImported(Ok(report)));
    reload(&mut app);
    assert_eq!(offered(&app), ["2"]);
    assert!(!app.steam_auto.running());
}

#[test]
fn a_dismissal_from_another_process_wins_over_a_queued_pass() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    let games = scan(&app).discovery.unwrap().pending;

    saved::dismiss_game(&app.paths, "2").unwrap();
    let report = run_pass(&app.paths, &app.shown, &games, &Token::new());
    let _ = app.update(Message::AutoImported(Ok(report)));
    reload(&mut app);
    assert_eq!(reason(&app, "2"), Some(Recorded::Dismissed));
    assert!(card_names(&app).is_empty());
    assert!(offered(&app).is_empty());
}

#[test]
fn only_one_pass_runs_and_a_game_found_meanwhile_gets_the_next_one() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    let games = scan(&app).discovery.unwrap().pending;
    let first = app.steam_auto.running.clone().unwrap();

    steam_game(&root, "3", "Newer");
    let current = scan(&app).discovery.unwrap();
    let _ = app.discovery_read(current);
    let running = app.steam_auto.running.as_ref().unwrap();
    first.cancel();
    assert!(running.is_canceled(), "still the first pass");
    assert_eq!(
        offered(&app),
        Vec::<String>::new(),
        "held for the next pass"
    );

    let report = run_pass(&app.paths, &app.shown, &games, &Token::new());
    assert_eq!(report.imported, ["2"]);
    let _ = app.update(Message::AutoImported(Ok(report)));
    assert!(!app.steam_auto.running());
    reload(&mut app);
    assert!(app.steam_auto.running(), "the read after finds game 3");
    finish(&mut app, &Token::new());
    assert_eq!(reason(&app, "3"), Some(Recorded::Imported));
}

#[test]
fn a_failed_import_is_offered_and_left_alone_until_the_next_launch() {
    let (root, mut app) = settled(true);
    taken(
        &app,
        &wineo_core::heroic_import::default_name("Clash", None),
    );
    steam_game(&root, "2", "Clash");
    steam_game(&root, "3", "Fine");
    reload(&mut app);
    assert!(app.steam_auto.running());
    finish(&mut app, &Token::new());
    assert_eq!(reason(&app, "2"), None, "back to pending");
    assert_eq!(reason(&app, "3"), Some(Recorded::Imported));
    assert_eq!(offered(&app), ["2"], "offered as soon as it's given up on");

    reload(&mut app);
    assert_eq!(offered(&app), ["2"]);
    assert!(!app.steam_auto.running(), "not tried again this session");

    let current = scan(&app).discovery.unwrap();
    assert_eq!(current.pending.len(), 1);
    let _ = app.discovery_read(current);
    assert!(!app.steam_auto.running(), "nor on any later scan");
    assert_eq!(offered(&app), ["2"]);
    assert_eq!(reason(&app, "2"), None);

    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    let mut next = App::boot(paths).0;
    reload(&mut next);
    assert!(next.steam_auto.running(), "the next launch tries again");
}

#[test]
fn an_unwritable_record_imports_nothing_says_so_once_and_doesnt_loop() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    let games = scan(&app).discovery.unwrap().pending;
    let refreshing = app.refreshing;

    let attempt = |app: &App| {
        read_only(app, || {
            run_pass(&app.paths, &app.shown, &games, &Token::new())
        })
    };
    let report = attempt(&app);
    assert!(report.stopped.is_some());
    let _ = app.update(Message::AutoImported(Ok(report)));
    let notice = app.feature_notice.take().expect("the failure shows");
    assert!(
        notice
            .0
            .contains("Couldn't import new Steam games: can't update library.toml: "),
        "{}",
        notice.0
    );
    assert_eq!(offered(&app), ["2"], "the banner takes over");
    assert_eq!(
        app.refreshing, refreshing,
        "no read, which would only retry"
    );
    assert!(!app.steam_auto.running());
    assert!(
        rescan(&mut app).is_none_or(|scan| scan.discovery.is_none()),
        "nothing brings the same list back on its own"
    );
    assert!(!app.steam_auto.running());
    assert!(card_names(&app).is_empty());

    // Coming back to the window tries once, quietly, and the banner stays up
    // through it.
    let _ = app.update(Message::Refresh);
    reload(&mut app);
    assert!(app.steam_auto.running());
    assert_eq!(offered(&app), ["2"]);
    let report = attempt(&app);
    let _ = app.update(Message::AutoImported(Ok(report)));
    assert!(
        app.feature_notice.is_none(),
        "one notice per run of failures"
    );

    // Once it can be written, the game goes the usual way and a new run of
    // failures would be said again.
    let _ = app.update(Message::Refresh);
    reload(&mut app);
    finish(&mut app, &Token::new());
    assert_eq!(reason(&app, "2"), Some(Recorded::Imported));
    assert!(!app.steam_auto.stuck);
    reload(&mut app);
    assert!(offered(&app).is_empty());
}

#[test]
fn a_pass_that_cant_start_counts_as_stopped() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    let games = scan(&app).discovery.unwrap().pending;
    let global = app.paths.global_config();
    fs::create_dir_all(global.parent().unwrap()).unwrap();
    fs::write(&global, "not toml at all").unwrap();
    let report = run_pass(&app.paths, &app.shown, &games, &Token::new());
    assert!(report.stopped.is_some());
    let _ = app.update(Message::AutoImported(Ok(report)));
    assert!(app.feature_notice.is_some());
    assert_eq!(offered(&app), ["2"]);
    assert_eq!(reason(&app, "2"), None);
}

#[test]
fn a_panicked_pass_counts_as_stopped() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    let _ = app.update(Message::AutoImported(Err("it panicked".to_owned())));
    assert!(!app.steam_auto.running());
    assert_eq!(offered(&app), ["2"]);
}

#[test]
fn portable_and_unusable_files_and_no_steam_start_nothing() {
    use wineo_core::steam::Discovery;

    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let portable = root.path().join("portable");
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| home.as_os_str().to_owned()),
        Some(&portable),
    )
    .unwrap();
    let mut app = App::boot(paths).0;
    saved::set_auto_import(&app.paths, true).unwrap();
    steam_game(&root, "2", "New");
    assert_eq!(scan(&app).discovery, Some(Discovery::default()));
    let _ = app.discovery_read(Discovery {
        pending: vec![PendingGame {
            app_id: "2".to_owned(),
            title: "New".to_owned(),
        }],
        auto_import: true,
        ..Discovery::default()
    });
    assert!(
        !app.steam_auto.running(),
        "not even a stray result starts one"
    );

    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    fs::write(app.paths.library_file(), "[steam\nbroken").unwrap();
    reload(&mut app);
    assert!(!app.steam_auto.running());

    let (_root, mut app) = fixture();
    saved::set_auto_import(&app.paths, true).unwrap();
    reload(&mut app);
    assert!(!app.steam_auto.running());
    assert!(
        fs::read_to_string(app.paths.library_file())
            .unwrap()
            .contains("auto_import")
    );
}

#[test]
fn closing_during_a_pass_cancels_it_and_waits_for_it() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    let games = scan(&app).discovery.unwrap().pending;
    let token = app.steam_auto.running.clone().unwrap();

    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(token.is_canceled());
    assert!(
        app.closing && app.close_quietly,
        "no prompt, a bounded wait"
    );
    assert!(app.close_by.is_some());
    assert!(!app.exiting);
    assert_eq!(app.abandoned_work(), ["a Steam import"]);

    let report = run_pass(&app.paths, &app.shown, &games, &token);
    assert!(report.canceled);
    assert_eq!(reason(&app, "2"), None);
    let _ = app.update(Message::AutoImported(Ok(report)));
    assert!(!app.steam_auto.running());
    assert!(!app.exit_waits());
    assert!(!app.refreshing, "a closing window reads nothing more");
    assert!(app.abandoned_work().is_empty());
}

#[test]
fn cancel_and_close_and_a_late_result_after_exit_are_safe() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    let token = app.steam_auto.running.clone().unwrap();
    let _ = app.update(Message::CancelAndClose);
    assert!(token.is_canceled());
    let _ = app.update(Message::ForceCloseWork);
    assert!(app.watchdog.started().is_some());
    let _ = app.update(Message::AutoImported(Ok(Report::default())));
    assert!(!app.steam_auto.running());
}

#[test]
fn a_closing_window_starts_no_pass() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    app.closing = true;
    reload(&mut app);
    assert!(!app.steam_auto.running());
}

#[test]
fn a_game_given_up_on_can_be_dismissed_from_the_banner_during_a_later_pass() {
    let (root, mut app) = settled(true);
    taken(
        &app,
        &wineo_core::heroic_import::default_name("Clash", None),
    );
    steam_game(&root, "2", "Clash");
    reload(&mut app);
    finish(&mut app, &Token::new());
    reload(&mut app);
    steam_game(&root, "3", "Fine");
    reload(&mut app);
    assert!(app.steam_auto.running());
    assert_eq!(offered(&app), ["2"]);

    saved::dismiss_games(&app.paths, &["2".to_owned()]).unwrap();
    let _ = app.update(Message::NudgeDismissed(vec!["2".to_owned()], Ok(())));
    assert!(offered(&app).is_empty());
    finish(&mut app, &Token::new());
    reload(&mut app);
    assert_eq!(reason(&app, "2"), Some(Recorded::Dismissed));
    assert_eq!(reason(&app, "3"), Some(Recorded::Imported));
}

/// A Steam environment the record doesn't have, which a scan proposes adding.
fn imported_elsewhere(app: &App, id: &str) {
    let config = app.paths.environment_config("elsewhere");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        format!(
            "schema_version = 1\nkind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"{id}\"\ntitle = \"Elsewhere\"\ntags = [\"game\"]\ndesktop = false\n"
        ),
    )
    .unwrap();
}

/// Lands what a scan's record write would do.
fn record_proposal(app: &mut App) {
    let proposed = scan(app).discovery.unwrap().proposed;
    let result = crate::app_library::write_steam_record(&app.paths, &app.shown, &proposed);
    let _ = app.update(Message::SteamRecorded(result));
}

#[test]
fn a_stuck_pass_isnt_restarted_by_a_redelivered_proposal_and_recovers_when_it_lands() {
    let (root, mut app) = settled(true);
    imported_elsewhere(&app, "9");
    steam_game(&root, "2", "New");
    reload(&mut app);
    assert!(
        !scan(&app).discovery.unwrap().proposed.is_empty(),
        "a proposal is outstanding"
    );
    assert!(app.steam_auto.running());
    let games = scan(&app).discovery.unwrap().pending;

    let pass = read_only(&app, || {
        run_pass(&app.paths, &app.shown, &games, &Token::new())
    });
    assert!(pass.stopped.is_some());
    let _ = app.update(Message::AutoImported(Ok(pass)));
    record_proposal_failed(&mut app);

    let mut started = 0;
    let mut redelivered = 0;
    for _ in 0..5 {
        let found = rescan(&mut app).and_then(|scan| scan.discovery);
        redelivered += usize::from(found.is_some());
        started += usize::from(app.steam_auto.running());
        assert_eq!(offered(&app), ["2"]);
        record_proposal_failed(&mut app);
    }
    assert_eq!(redelivered, 5, "the failed write keeps bringing it back");
    assert_eq!(started, 0, "and none of those starts a pass");
    assert_eq!(reason(&app, "2"), None);

    // The file can be written again: the proposal lands, the next discovery
    // starts the pass, and the game imports.
    rescan(&mut app).unwrap();
    assert!(!app.steam_auto.running());
    record_proposal(&mut app);
    assert!(!app.steam_auto.stuck);
    assert_eq!(reason(&app, "9"), Some(Recorded::Imported));
    assert!(rescan(&mut app).unwrap().discovery.is_some());
    assert!(app.steam_auto.running());
    assert!(offered(&app).is_empty());
    finish(&mut app, &Token::new());
    assert_eq!(reason(&app, "2"), Some(Recorded::Imported));
}

fn record_proposal_failed(app: &mut App) {
    let proposed = scan(app).discovery.unwrap().proposed;
    let result = read_only(app, || {
        crate::app_library::write_steam_record(&app.paths, &app.shown, &proposed)
    });
    assert!(result.is_err());
    let _ = app.update(Message::SteamRecorded(result));
}

#[test]
fn while_stuck_a_changed_list_or_a_focus_reload_may_try_again() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    let games = scan(&app).discovery.unwrap().pending;
    let stopped = Report {
        stopped: Some("no".to_owned()),
        ..Report::default()
    };
    let _ = app.update(Message::AutoImported(Ok(stopped.clone())));

    reload(&mut app);
    assert!(!app.steam_auto.running(), "the same list waits");

    steam_game(&root, "3", "Newer");
    reload(&mut app);
    assert!(app.steam_auto.running(), "a changed list tries");
    let _ = app.update(Message::AutoImported(Ok(stopped)));
    reload(&mut app);
    assert!(!app.steam_auto.running());

    let _ = app.update(Message::Window(iced::window::Event::Unfocused));
    let _ = app.update(Message::Window(iced::window::Event::Focused));
    reload(&mut app);
    assert!(app.steam_auto.running(), "coming back to the window tries");
    assert_eq!(games.len(), 1);
}

/// A pass whose import didn't happen and whose cleanup couldn't write.
fn left_claimed(app: &mut App, report: Report) {
    saved::claim_game(&app.paths, "2", |_| true).unwrap();
    app.shown.refresh_library(&app.paths);
    let _ = app.update(Message::AutoImported(Ok(report)));
}

/// Runs the retry the last discovery started.
fn finish_release(app: &mut App) {
    let ids: Vec<String> = app.steam_auto.unreleased.iter().cloned().collect();
    let released = release_claims(&app.paths, &app.shown, &ids, &Token::new());
    let _ = app.update(Message::ClaimsReleased(Ok(released)));
}

#[test]
fn a_failed_game_whose_claim_stuck_is_offered_once_the_record_recovers() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    left_claimed(
        &mut app,
        Report {
            failures: vec![wineo_core::steam_import::Failure {
                app_id: "2".to_owned(),
                name: "new".to_owned(),
                reason: "no".to_owned(),
            }],
            unreleased: vec!["2".to_owned()],
            ..Report::default()
        },
    );
    reload(&mut app);
    assert_eq!(reason(&app, "2"), Some(Recorded::Claimed));
    assert!(offered(&app).is_empty(), "hidden while it's claimed");
    assert!(app.steam_auto.running(), "with nothing pending, it retries");

    // Still unwritable: quiet, and the next scan comes back without a change.
    let ids: Vec<String> = app.steam_auto.unreleased.iter().cloned().collect();
    let released = read_only(&app, || {
        release_claims(&app.paths, &app.shown, &ids, &Token::new())
    });
    assert!(released.is_empty());
    let _ = app.update(Message::ClaimsReleased(Ok(released)));
    assert!(!app.steam_auto.running());
    assert_eq!(app.steam_auto.unreleased.len(), 1);
    assert!(app.feature_notice.is_none());
    assert!(rescan(&mut app).unwrap().discovery.is_some());
    assert!(app.steam_auto.running());

    finish_release(&mut app);
    assert_eq!(reason(&app, "2"), None);
    assert!(app.steam_auto.unreleased.is_empty());
    assert!(app.refreshing, "a read follows, for the game");
    reload(&mut app);
    assert_eq!(offered(&app), ["2"], "offered, not imported again");
    assert!(!app.steam_auto.running());
}

#[test]
fn a_canceled_game_whose_claim_stuck_goes_back_to_the_pass() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    left_claimed(
        &mut app,
        Report {
            canceled: true,
            unreleased: vec!["2".to_owned()],
            ..Report::default()
        },
    );
    reload(&mut app);
    assert!(app.steam_auto.running());
    finish_release(&mut app);
    reload(&mut app);
    assert!(
        app.steam_auto.running(),
        "not a failure, so it's taken again"
    );
    assert!(offered(&app).is_empty());
    finish(&mut app, &Token::new());
    assert_eq!(reason(&app, "2"), Some(Recorded::Imported));
}

#[test]
fn a_retry_runs_with_the_setting_off_and_touches_only_its_own_claims() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    left_claimed(
        &mut app,
        Report {
            canceled: true,
            unreleased: vec!["2".to_owned()],
            ..Report::default()
        },
    );
    saved::set_auto_import(&app.paths, false).unwrap();
    saved::record_games(&app.paths, &["3".to_owned()], Recorded::Claimed).unwrap();
    saved::dismiss_game(&app.paths, "2").unwrap();
    reload(&mut app);
    assert!(app.steam_auto.running());
    finish_release(&mut app);
    assert_eq!(reason(&app, "2"), Some(Recorded::Dismissed));
    assert_eq!(
        reason(&app, "3"),
        Some(Recorded::Claimed),
        "another window's"
    );
}

#[test]
fn a_new_launch_doesnt_sweep_claims() {
    let (root, app) = settled(true);
    saved::record_games(&app.paths, &["2".to_owned()], Recorded::Claimed).unwrap();
    steam_game(&root, "2", "New");
    let mut next = App::boot(app.paths.clone()).0;
    reload(&mut next);
    assert!(!next.steam_auto.running());
    assert_eq!(reason(&next, "2"), Some(Recorded::Claimed));
}

/// The toast, once a pass has imported `ids` and the reload has landed.
fn import_pass(root: &tempfile::TempDir, app: &mut App, games: &[(&str, &str)]) -> Report {
    for (id, title) in games {
        steam_game(root, id, title);
    }
    rescan(app).expect("new installs are news");
    let discovery = scan(app).discovery.unwrap();
    let report = run_pass(&app.paths, &app.shown, &discovery.pending, &Token::new());
    let _ = app.update(Message::AutoImported(Ok(report.clone())));
    reload(app);
    report
}

fn clicked(app: &mut App) {
    let _ = app.update(Message::ImportToast);
}

#[test]
fn one_import_toasts_its_title_and_a_click_opens_the_panel() {
    let (root, mut app) = settled(true);
    let before = Instant::now();
    import_pass(&root, &mut app, &[("2", "New")]);
    assert_eq!(app.import_toast.text(), Some("Imported New"));
    let until = app.import_toast.expires_at().expect("the Library is shown");
    assert!(until >= before + Duration::from_secs(8));
    assert!(app.feature_notice.is_none());
    assert_eq!(app.selected, None);

    clicked(&mut app);
    let name = app.cards[0].name.clone();
    assert_eq!(app.selected, Some(name));
    assert!(app.panel_open);
    assert!(app.import_toast.text().is_none(), "a click dismisses it");
}

#[test]
fn several_imports_toast_a_count_and_a_click_does_nothing() {
    let (root, mut app) = settled(true);
    import_pass(&root, &mut app, &[("2", "New"), ("3", "Newer")]);
    assert_eq!(app.import_toast.text(), Some("Imported 2 Steam games"));

    clicked(&mut app);
    assert_eq!(app.selected, None);
    assert!(!app.panel_open);
    assert_eq!(app.import_toast.text(), Some("Imported 2 Steam games"));
}

#[test]
fn a_pass_with_no_imports_shows_no_toast() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    let failure = wineo_core::steam_import::Failure {
        app_id: "2".to_owned(),
        name: "new".to_owned(),
        reason: "no".to_owned(),
    };
    let reports = [
        Report {
            failures: vec![failure],
            ..Report::default()
        },
        Report {
            stopped: Some("can't".to_owned()),
            ..Report::default()
        },
        Report {
            canceled: true,
            ..Report::default()
        },
    ];
    for report in reports {
        let _ = app.update(Message::AutoImported(Ok(report)));
        app.refreshing = false;
        reload(&mut app);
        assert!(app.import_toast.text().is_none());
    }
}

#[test]
fn imports_beside_failures_still_toast() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    let discovery = scan(&app).discovery.unwrap();
    let mut report = run_pass(&app.paths, &app.shown, &discovery.pending, &Token::new());
    report.failures.push(wineo_core::steam_import::Failure {
        app_id: "9".to_owned(),
        name: "nine".to_owned(),
        reason: "no".to_owned(),
    });
    let _ = app.update(Message::AutoImported(Ok(report)));
    reload(&mut app);
    assert_eq!(app.import_toast.text(), Some("Imported New"));
}

#[test]
fn an_id_without_a_card_is_left_out() {
    let (root, mut app) = settled(true);
    steam_game(&root, "2", "New");
    rescan(&mut app).unwrap();
    let discovery = scan(&app).discovery.unwrap();
    let mut report = run_pass(&app.paths, &app.shown, &discovery.pending, &Token::new());
    report.imported.push("404".to_owned());
    let _ = app.update(Message::AutoImported(Ok(report)));
    reload(&mut app);
    assert_eq!(app.import_toast.text(), Some("Imported New"));

    let gone = Report {
        imported: vec!["404".to_owned()],
        ..Report::default()
    };
    let _ = app.update(Message::ImportToast);
    let _ = app.update(Message::AutoImported(Ok(gone)));
    app.refreshing = false;
    reload(&mut app);
    assert!(app.import_toast.text().is_none());
}

#[test]
fn a_toast_finished_behind_another_section_starts_on_the_library() {
    let (root, mut app) = settled(true);
    app.section = crate::sidebar::Section::Trash;
    import_pass(&root, &mut app, &[("2", "New")]);
    assert_eq!(app.import_toast.text(), Some("Imported New"));
    assert!(app.import_toast.waiting());
    assert!(app.import_toast.expires_at().is_none());
    assert!(app.import_toast_view().is_none(), "not drawn while waiting");
    clicked(&mut app);
    assert_eq!(app.selected, None, "a click can't reach it");
    assert_eq!(app.import_toast.text(), Some("Imported New"));

    let before = Instant::now();
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert!(!app.import_toast.waiting());
    let until = app.import_toast.expires_at().unwrap();
    assert!(until >= before + Duration::from_secs(8));
    assert!(until <= Instant::now() + Duration::from_secs(8));
    assert_eq!(app.next_toast_deadline(), Some(until));
    assert!(app.import_toast_view().is_some());
}

#[test]
fn the_toast_expires_after_its_eight_seconds() {
    let (root, mut app) = settled(true);
    import_pass(&root, &mut app, &[("2", "New")]);
    let until = app.import_toast.expires_at().unwrap();
    assert_eq!(app.toast_wake.due(), Some(until));

    app.import_toast
        .set_deadline(Instant::now() + Duration::from_secs(5));
    let _ = app.update(Message::ToastHover(false));
    assert!(app.import_toast.text().is_some());

    app.import_toast
        .set_deadline(Instant::now() - Duration::from_millis(1));
    let _ = app.update(Message::ToastHover(false));
    assert!(app.import_toast.text().is_none());
    assert_eq!(app.next_toast_deadline(), None);
}

#[test]
fn a_covered_window_ignores_the_click() {
    let (root, mut app) = settled(true);
    import_pass(&root, &mut app, &[("2", "New")]);
    app.close_prompt = true;
    clicked(&mut app);
    assert_eq!(app.selected, None);
    assert_eq!(app.import_toast.text(), Some("Imported New"));
}

#[test]
fn a_newer_pass_replaces_the_toast() {
    let (root, mut app) = settled(true);
    import_pass(&root, &mut app, &[("2", "New")]);
    import_pass(&root, &mut app, &[("3", "Newer")]);
    assert_eq!(app.import_toast.text(), Some("Imported Newer"));
}

#[test]
fn the_toast_stacks_with_the_undo_toast_and_the_notice() {
    let (root, mut app) = settled(true);
    import_pass(&root, &mut app, &[("2", "New")]);
    let name = app.cards[0].name.clone();
    let entry = wineo_core::trash::remove(&app.paths, &name, false).unwrap();
    app.toast = Some(crate::removal::Toast::new(entry));
    app.flag_notice("Something".to_owned());
    let _ = app.update(Message::ToastHover(false));
    assert!(app.toast.is_some());
    assert!(app.feature_notice.is_some());
    assert_eq!(app.import_toast.text(), Some("Imported New"));
    let _ = app.view();
}
