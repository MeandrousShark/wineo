use std::fs;

use iced::futures::StreamExt;
use wineo_core::runtime::State;
use wineo_core::trash::{self, PendingEntry};

use super::*;
use crate::App;
use crate::sidebar::Section;
use crate::tests::card;

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    (root, App::boot(paths).0)
}

fn native(paths: &Paths, name: &str) {
    let config = paths.environment_config(name);
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(config, "schema_version = 1\nkind = 'native'\n").unwrap();
}

fn library(paths: &Paths) -> Result<Vec<Card>, String> {
    load_library(paths).map_err(|error| error.to_string())
}

/// The app after its first library and badge loads.
fn shown(names: &[&str]) -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    for name in names {
        native(&app.paths, name);
    }
    let _ = app.update(Message::Loaded(library(&app.paths)));
    let _ = app.update(Message::TrashBadgeLoaded(TrashBadge::read(&app.paths)));
    (root, app)
}

fn rescan(app: &App, trash_page: bool) -> Scan {
    read(&app.paths, &app.shown, trash_page, app.art_page.as_deref())
}

fn now() -> jiff::Timestamp {
    jiff::Timestamp::now()
}

fn names(app: &App) -> Vec<&str> {
    app.cards.iter().map(|card| card.name.as_str()).collect()
}

#[test]
fn nothing_is_reported_before_the_first_load_is_shown() {
    let (_root, app) = fixture();
    native(&app.paths, "game");
    assert!(app.shown.changes(rescan(&app, false), now()).is_none());
    assert!(app.shown.changes(rescan(&app, true), now()).is_none());
}

#[test]
fn an_unchanged_rescan_reports_nothing_and_a_change_is_applied() {
    let (_root, mut app) = shown(&["game", "old"]);
    assert!(app.shown.changes(rescan(&app, false), now()).is_none());
    trash::remove(&app.paths, "old", false).unwrap();
    let scan = app.shown.changes(rescan(&app, false), now()).unwrap();
    assert!(scan.library.is_some());
    assert!(matches!(scan.trash, Some(Trash::Badge(_))));
    let _ = app.update(Message::Rescanned(Box::new(scan)));
    assert_eq!(names(&app), ["game"]);
    assert_eq!(app.trash_badge.count, Some(1));
    assert!(app.shown.changes(rescan(&app, false), now()).is_none());
}

#[test]
fn only_the_changed_part_is_reported() {
    let (_root, app) = shown(&["game"]);
    native(&app.paths, "new");
    let scan = app.shown.changes(rescan(&app, false), now()).unwrap();
    assert!(scan.library.is_some());
    assert!(scan.trash.is_none());
}

#[test]
fn every_card_field_counts_as_a_change() {
    let watch = Watch::default();
    let base = card("game", State::Inactive, 0);
    watch.show_library(Ok(vec![base.clone()]));
    let scan = |cards: Vec<Card>| Scan {
        library: Some(Ok(cards)),
        ..Scan::default()
    };
    assert!(watch.changes(scan(vec![base.clone()]), now()).is_none());
    let mut running = base.clone();
    running.state = State::Running;
    let mut hook = base.clone();
    hook.busy_holder = Some(wineo_core::operation::Operation::Hook(
        wineo_core::hooks::Phase::After,
    ));
    let mut art = base.clone();
    art.art = Some(crate::tests::art_file("/tmp/grid.png", 20));
    let mut renamed = base.clone();
    renamed.display_title = "Game".into();
    for changed in [running, hook, art, renamed] {
        assert!(watch.changes(scan(vec![changed]), now()).is_some());
    }
    assert!(
        watch
            .changes(
                Scan {
                    library: Some(Err("unreadable".into())),
                    ..Scan::default()
                },
                now(),
            )
            .is_some()
    );
}

#[test]
fn a_shown_error_is_replaced_once_the_library_reads_again() {
    let (_root, mut app) = shown(&["game"]);
    let _ = app.update(Message::Loaded(Err("broken".into())));
    let scan = app.shown.changes(rescan(&app, false), now()).unwrap();
    let _ = app.update(Message::Rescanned(Box::new(scan)));
    assert!(app.error.is_none());
}

#[test]
fn the_trash_page_compares_its_entries_and_their_age_labels() {
    let (_root, mut app) = shown(&["a", "b"]);
    trash::remove(&app.paths, "a", false).unwrap();
    let _ = app.update(Message::Section(Section::Trash));
    let snapshot = crate::trash_work::read(&app.paths);
    let _ = app.update(Message::Trash(crate::trash_page::Event::Loaded(
        app.trash_generation,
        snapshot.clone(),
    )));
    let page = |snapshot: Result<Snapshot, String>| Scan {
        trash: Some(Trash::Page(snapshot)),
        ..Scan::default()
    };
    let shown_at = now();
    assert!(
        app.shown
            .changes(page(snapshot.clone()), shown_at)
            .is_none()
    );
    let later = shown_at + jiff::SignedDuration::from_secs(120);
    assert!(app.shown.changes(page(snapshot.clone()), later).is_some());

    let pending = |ids: &[&str]| {
        let mut snapshot = snapshot.clone().unwrap();
        snapshot.pending = ids
            .iter()
            .map(|id| PendingEntry {
                id: (*id).into(),
                name: (*id).into(),
                finished: false,
            })
            .collect();
        Ok(snapshot)
    };
    app.shown.show_trash(pending(&["x", "y"]), shown_at);
    assert!(
        app.shown
            .changes(page(pending(&["y", "x"])), shown_at)
            .is_none()
    );
    assert!(app.shown.changes(page(pending(&["x"])), shown_at).is_some());

    trash::remove(&app.paths, "b", false).unwrap();
    let scan = app.shown.changes(rescan(&app, true), now()).unwrap();
    let _ = app.update(Message::Rescanned(Box::new(scan)));
    let Some(Ok(snapshot)) = &app.trash_page.snapshot else {
        panic!("the page shows its snapshot");
    };
    assert_eq!(snapshot.entries.len(), 2);
    assert_eq!(app.trash_badge.count, Some(2));
}

#[test]
fn an_explicit_reload_in_flight_wins_and_the_change_is_not_lost() {
    let (_root, mut app) = shown(&["game"]);
    native(&app.paths, "new");
    let scan = app.shown.changes(rescan(&app, false), now()).unwrap();
    let _ = app.update(Message::Refresh);
    let generation = app.library_generation;
    let _ = app.update(Message::Rescanned(Box::new(scan)));
    assert!(app.refreshing, "the explicit reload is still awaited");
    assert_eq!(app.library_generation, generation);
    assert_eq!(names(&app), ["game"]);
    // The dropped result still differs from the screen, so it comes again.
    assert!(app.shown.changes(rescan(&app, false), now()).is_some());
    let _ = app.update(Message::LibraryReloaded(generation, library(&app.paths)));
    assert_eq!(names(&app), ["game", "new"]);
    assert!(app.shown.changes(rescan(&app, false), now()).is_none());
}

#[test]
fn unfocused_pauses_the_rescan_and_focus_reloads_at_once() {
    let (_root, mut app) = shown(&["game"]);
    assert!(app.focused, "focused until the window says otherwise");
    let _ = app.update(Message::Window(iced::window::Event::Focused));
    assert!(
        !app.refreshing && !app.trash_refreshing,
        "no reload without a change of focus"
    );
    let _ = app.update(Message::Window(iced::window::Event::Unfocused));
    assert!(!app.focused);
    assert!(!app.refreshing && !app.trash_refreshing);
    let _ = app.update(Message::Window(iced::window::Event::Focused));
    assert!(app.focused);
    assert!(app.refreshing && app.trash_refreshing);
}

#[test]
fn a_scan_read_before_an_explicit_reload_finished_is_dropped() {
    let (_root, mut app) = shown(&["game"]);
    let stale = rescan(&app, false);
    native(&app.paths, "new");
    let _ = app.update(Message::Refresh);
    let _ = app.update(Message::LibraryReloaded(
        app.library_generation,
        library(&app.paths),
    ));
    assert_eq!(names(&app), ["game", "new"]);
    let stale = app.shown.changes(stale, now()).unwrap();
    assert!(!app.refreshing);
    let _ = app.update(Message::Rescanned(Box::new(stale)));
    assert_eq!(names(&app), ["game", "new"], "the newer reload stays");
    // A change after the dropped scan still arrives with the next one.
    native(&app.paths, "later");
    let next = app.shown.changes(rescan(&app, false), now()).unwrap();
    let _ = app.update(Message::Rescanned(Box::new(next)));
    assert_eq!(names(&app), ["game", "later", "new"]);
}

#[test]
fn any_explicit_reload_makes_earlier_scans_stale_when_it_starts() {
    let (_root, mut app) = shown(&["game"]);
    native(&app.paths, "new");
    let stale = app.shown.changes(rescan(&app, false), now()).unwrap();
    // Like an Art save, which reloads without marking `refreshing`.
    let _ = app.reload_library();
    assert!(!app.refreshing);
    let _ = app.update(Message::Rescanned(Box::new(stale)));
    assert_eq!(names(&app), ["game"], "the scan predates the reload");

    let (_root, mut app) = shown(&["game", "old"]);
    trash::remove(&app.paths, "old", false).unwrap();
    let stale = app.shown.changes(rescan(&app, false), now()).unwrap();
    let _ = app.reload_badge();
    app.trash_refreshing = false;
    let _ = app.update(Message::Rescanned(Box::new(stale)));
    assert_eq!(app.trash_badge.count, Some(0));
    let stale = app.shown.changes(rescan(&app, false), now()).unwrap();
    let _ = app.reload_trash_page();
    app.trash_refreshing = false;
    let _ = app.update(Message::Rescanned(Box::new(stale)));
    assert_eq!(app.trash_badge.count, Some(0));
}

#[test]
fn only_one_scan_reads_at_a_time_and_a_busy_tick_skips() {
    let (_root, app) = shown(&["game"]);
    let guard = app.shown.begin_scan().unwrap();
    assert!(app.shown.begin_scan().is_none());
    let rescan = Rescan {
        paths: app.paths.clone(),
        watch: app.shown.clone(),
        trash_page: false,
        art_page: None,
    };
    paused(async {
        let mut stream = Box::pin(stream(&rescan));
        native(&app.paths, "new");
        let skipped = tokio::time::timeout(INTERVAL * 3, stream.next()).await;
        assert!(skipped.is_err(), "ticks skip while another read runs");
        // The read finishing elsewhere, as a dropped stream's would.
        std::thread::spawn(move || drop(guard)).join().unwrap();
        let message = tokio::time::timeout(INTERVAL * 2, stream.next())
            .await
            .expect("the next tick scans again")
            .unwrap();
        assert!(matches!(message, Message::Rescanned(_)));
    });
    assert!(app.shown.begin_scan().is_some(), "released after the read");
}

fn art_app() -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    wineo_core::environment::Environment::create_native(
        &app.paths,
        &wineo_core::config::GlobalConfig::default(),
        "game",
        None,
        false,
        |_| {},
    )
    .unwrap();
    let _ = app.update(Message::Loaded(library(&app.paths)));
    let _ = app.update(Message::TrashBadgeLoaded(TrashBadge::read(&app.paths)));
    app.selected = Some("game".into());
    let _ = app.update(Message::OpenArt);
    assert_eq!(app.art_page.as_deref(), Some("game"));
    load_art(&mut app);
    (root, app)
}

/// Delivers the Art page's load, as its task would.
fn load_art(app: &mut App) {
    let page = &app.art_pages["game"];
    let event = art_page::Event::Loaded(
        page.load_sequence,
        art_page::load_snapshot(&app.paths, "game"),
    );
    let _ = app.update(Message::ArtPage("game".into(), page.generation, event));
}

#[test]
fn an_open_art_page_sees_icon_and_game_only_changes() {
    let (_root, mut app) = art_app();
    assert!(app.shown.changes(rescan(&app, false), now()).is_none());
    fs::create_dir_all(app.paths.art_dir("game")).unwrap();
    let icon = app.paths.art_dir("game").join("icon.png");
    let edits: [&dyn Fn(&App); 3] = [
        &|_| fs::write(&icon, b"one").unwrap(),
        &|_| fs::write(&icon, b"other").unwrap(),
        &|app| wineo_core::art::save_game(&app.paths, "game", 42).unwrap(),
    ];
    for edit in edits {
        edit(&app);
        let changed = app.shown.changes(rescan(&app, false), now()).unwrap();
        assert!(changed.library.is_none(), "only the Art page changed");
        assert!(changed.art.is_some());
        let sequence = app.art_pages["game"].load_sequence;
        let _ = app.update(Message::Rescanned(Box::new(changed)));
        assert_ne!(app.art_pages["game"].load_sequence, sequence, "it reloads");
        load_art(&mut app);
        assert!(app.shown.changes(rescan(&app, false), now()).is_none());
    }
}

#[test]
fn a_recreated_environment_closes_its_art_page() {
    let (_root, mut app) = art_app();
    let config = app.paths.environment_config("game");
    let text = fs::read(&config).unwrap();
    fs::remove_file(&config).unwrap();
    fs::write(&config, text).unwrap();
    let changed = app.shown.changes(rescan(&app, false), now()).unwrap();
    let _ = app.update(Message::Rescanned(Box::new(changed)));
    assert!(app.art_page.is_none());
}

#[test]
fn input_counts_as_focus_when_no_focused_event_comes() {
    use iced::window::Event::{Focused, Unfocused};
    let press = || iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left));
    let key = iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
        modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
        physical_key: iced::keyboard::key::Physical::Unidentified(
            iced::keyboard::key::NativeCode::Unidentified,
        ),
        location: iced::keyboard::Location::Standard,
        modifiers: iced::keyboard::Modifiers::empty(),
        text: None,
        repeat: false,
    });
    let moved = iced::Event::Mouse(iced::mouse::Event::CursorMoved {
        position: iced::Point::ORIGIN,
    });
    assert!(
        crate::window_or_press(moved).is_none(),
        "only presses count"
    );
    assert!(matches!(
        crate::window_or_press(key),
        Some(Message::Pressed)
    ));
    // Events reach update in the order they happened, through one mapping.
    let run = |events: Vec<iced::Event>| {
        let (_root, mut app) = shown(&["game"]);
        for event in events {
            let _ = app.update(crate::window_or_press(event).unwrap());
        }
        (app.focused, app.refreshing && app.trash_refreshing)
    };
    let window = iced::Event::Window;
    assert_eq!(
        run(vec![window(Focused), window(Unfocused), press()]),
        (true, true),
        "a press after losing focus counts as focus"
    );
    assert_eq!(
        run(vec![window(Focused), press(), window(Unfocused)]),
        (false, false),
        "a press before losing focus doesn't"
    );
}

fn paused<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(future)
}

#[test]
fn the_stream_stays_silent_until_something_changes() {
    let (_root, app) = shown(&["game"]);
    let rescan = Rescan {
        paths: app.paths.clone(),
        watch: app.shown.clone(),
        trash_page: false,
        art_page: None,
    };
    paused(async {
        let mut stream = Box::pin(stream(&rescan));
        let quiet = tokio::time::timeout(INTERVAL * 5, stream.next()).await;
        assert!(quiet.is_err(), "unchanged rescans send nothing");
        native(&app.paths, "new");
        let message = tokio::time::timeout(INTERVAL * 2, stream.next())
            .await
            .expect("a change arrives within one interval")
            .unwrap();
        let Message::Rescanned(scan) = message else {
            panic!("unexpected message");
        };
        assert!(scan.library.is_some() && scan.trash.is_none());
    });
}
