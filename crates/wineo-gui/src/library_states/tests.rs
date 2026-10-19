use std::fs;
use std::time::Duration;

use iced::Task;
use iced::keyboard::key::Named;
use wineo_core::paths::Paths;

use super::*;
use crate::filter_control::tests::library;
use crate::sort_control::tests::{focus_after, press};

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    (root, App::boot(paths).0)
}

fn paths(root: &tempfile::TempDir) -> Paths {
    let home = root.path().to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
}

fn loaded() -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    let _ = app.update(Message::Loaded(Ok(library())));
    (root, app)
}

fn filter(app: &mut App, event: Event) {
    let _ = app.update(Message::Filter(event));
}

fn write_library(paths: &Paths, text: &str) {
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), text).unwrap();
}

fn native(paths: &Paths, name: &str) {
    let config = paths.environment_config(name);
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(config, "schema_version = 1\nkind = 'native'\n").unwrap();
}

/// Lays out the window, so a state that can't be drawn fails here.
fn draw(app: &App) {
    let _ = crate::layout_probe::layout(&mut app.view(), app.window_size);
}

/// The bounds of the focus control `id` in the laid out window.
fn bounds(app: &App, id: &'static str) -> Option<iced::Rectangle> {
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    let found = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = found.clone();
    laid.run(Box::new(iced::advanced::widget::operation::map(
        crate::library_menu::bounds_of(id),
        move |bounds| *sink.lock().unwrap() = bounds,
    )));
    *found.lock().unwrap()
}

/// Focuses the control `id` and presses Enter on it.
fn press_control(app: &App, id: &'static str) -> Vec<Message> {
    let focus: Task<Message> = iced::widget::operation::focus(iced::widget::Id::new(id));
    let operations = iced_runtime::task::into_stream(focus)
        .map(|stream| {
            use iced::futures::StreamExt;
            iced::futures::executor::block_on(stream.collect::<Vec<_>>())
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|action| match action {
            iced_runtime::Action::Widget(operation) => Some(operation),
            _ => None,
        })
        .collect();
    focus_after(app, operations).event(&press(Named::Enter))
}

#[test]
fn the_empty_library_state_shows_only_with_no_environments_once_loaded() {
    let (_root, mut app) = fixture();
    assert_eq!(app.empty_state(), None);
    let _ = app.update(Message::Loaded(Ok(Vec::new())));
    assert_eq!(app.empty_state(), Some(Empty::Library));
    // Filters don't turn an empty library into another state.
    filter(&mut app, Event::ToggleShowHidden);
    let _ = app.update(Message::SearchChanged("x".into()));
    assert_eq!(app.empty_state(), Some(Empty::Library));
    draw(&app);
}

#[test]
fn a_populated_grid_never_shows_an_empty_state() {
    let (_root, mut app) = loaded();
    assert_eq!(app.empty_state(), None);
    filter(&mut app, Event::ToggleTag("rpg".into()));
    assert_eq!(app.empty_state(), None);
    filter(&mut app, Event::ToggleShowHidden);
    filter(&mut app, Event::ToggleSource("Windows"));
    assert_eq!(app.empty_state(), None);
    let _ = app.update(Message::SearchChanged("demo".into()));
    assert_eq!(app.empty_state(), None);
    assert!(bounds(&app, SHOW_HIDDEN_ID).is_none());
    assert!(bounds(&app, CLEAR_ID).is_none());
}

fn all_hidden() -> Vec<crate::library::Card> {
    let mut cards = library();
    for card in &mut cards {
        card.hidden = true;
    }
    cards
}

#[test]
fn everything_hidden_counts_them_and_show_hidden_brings_them_back() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::Loaded(Ok(all_hidden())));
    assert_eq!(app.empty_state(), Some(Empty::AllHidden { hidden: 5 }));
    assert_eq!(all_hidden_note(5), "5 hidden. Show them to unhide any.");
    assert_eq!(all_hidden_note(1), "1 hidden. Show it to unhide it.");
    draw(&app);

    let messages = press_control(&app, SHOW_HIDDEN_ID);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Filter(Event::ShowHidden)))
    );
    filter(&mut app, Event::ShowHidden);
    assert!(app.filters.show_hidden);
    assert_eq!(app.empty_state(), None);
    assert_eq!(app.filtered().len(), 5);
}

#[test]
fn everything_hidden_with_a_search_or_filter_on_is_no_matches() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::Loaded(Ok(all_hidden())));
    let _ = app.update(Message::SearchChanged("rpg".into()));
    assert_eq!(
        app.empty_state(),
        Some(Empty::NoMatches { hidden_matching: 2 })
    );
    let _ = app.update(Message::SearchChanged(String::new()));
    filter(&mut app, Event::ToggleSource("Steam"));
    assert_eq!(
        app.empty_state(),
        Some(Empty::NoMatches { hidden_matching: 1 })
    );
}

#[test]
fn no_matches_says_how_many_hidden_games_match() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::SearchChanged("zzz".into()));
    assert_eq!(
        app.empty_state(),
        Some(Empty::NoMatches { hidden_matching: 0 })
    );
    draw(&app);
    assert!(
        bounds(&app, SHOW_HIDDEN_ID).is_none(),
        "nothing hidden matches"
    );
    assert!(bounds(&app, CLEAR_ID).is_some());

    let _ = app.update(Message::SearchChanged(String::new()));
    filter(&mut app, Event::ToggleSource("Windows"));
    filter(&mut app, Event::ToggleTag("rpg".into()));
    assert_eq!(
        app.empty_state(),
        Some(Empty::NoMatches { hidden_matching: 1 })
    );
    assert_eq!(matching_note(1), "1 hidden game matches");
    assert_eq!(matching_note(2), "2 hidden games match");
    assert!(bounds(&app, SHOW_HIDDEN_ID).is_some());

    // Show hidden shows the match; Show hidden on, nothing more to say.
    let messages = press_control(&app, SHOW_HIDDEN_ID);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Filter(Event::ShowHidden)))
    );
    filter(&mut app, Event::ShowHidden);
    assert_eq!(app.empty_state(), None);
    filter(&mut app, Event::ToggleSource("Windows"));
    filter(&mut app, Event::ToggleSource("GOG"));
    assert_eq!(
        app.empty_state(),
        Some(Empty::NoMatches { hidden_matching: 0 })
    );
}

#[test]
fn clear_filters_from_no_matches_shows_the_library_again() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::SearchChanged("zzz".into()));
    filter(&mut app, Event::ToggleSource("Steam"));
    let messages = press_control(&app, CLEAR_ID);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Filter(Event::Clear)))
    );
    filter(&mut app, Event::Clear);
    assert_eq!(app.empty_state(), None);
    assert_eq!(app.filtered().len(), 4);
}

#[test]
fn the_empty_state_stays_clear_of_the_open_panel() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("hades".into()));
    let _ = app.update(Message::SearchChanged("zzz".into()));
    assert!(app.panel_open);
    let clear = bounds(&app, CLEAR_ID).expect("Clear filters is drawn");
    assert!(
        clear.x + clear.width <= app.window_size.width - crate::grid_motion::DOCK_WIDTH,
        "{clear:?}"
    );
}

const BROKEN: &str = "schema_version = 1\nthis is broken\n[environments.game]\nhidden = true\n";

#[test]
fn the_banner_shows_from_boot_with_the_validate_message() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    write_library(&paths, BROKEN);
    let app = App::boot(paths).0;
    let problem = app.library_problem.clone().expect("boot read the problem");
    let text = banner_text(&problem);
    assert!(
        text.starts_with("library.toml can't be read (line 2: "),
        "{text}"
    );
    assert!(
        text.ends_with(
            "; favorites and hidden games are off until it's fixed. Fix it, or repair it in Checkup."
        ),
        "{text}"
    );
    assert!(app.library_banner().is_some());
    draw(&app);
}

#[test]
fn a_newer_file_points_to_an_update_not_a_repair() {
    let problem = wineo_core::library::Problem {
        reason: "library.toml was written by a newer wineo; update wineo to use it.".into(),
        line: None,
        newer_version: true,
    };
    assert_eq!(
        banner_text(&problem),
        "library.toml was written by a newer wineo; update wineo to use it. Favorites and hidden games are off until then."
    );
}

/// Runs the reload `task` on a runtime with a blocking pool and hands what
/// it sends to the app.
fn finish_reload(app: &mut App, task: Task<Message>) {
    use iced::futures::StreamExt;
    let stream = iced_runtime::task::into_stream(task).expect("the reload runs");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let actions = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), stream.collect::<Vec<_>>())
            .await
            .expect("the reload finishes")
    });
    for action in actions {
        if let iced_runtime::Action::Output(message) = action {
            let _ = app.update(message);
        }
    }
}

#[test]
fn an_explicit_reload_shows_and_clears_the_banner() {
    let (_root, mut app) = fixture();
    native(&app.paths, "game");
    let task = app.reload_library();
    finish_reload(&mut app, task);
    assert_eq!(app.library_problem, None);
    assert_eq!(app.cards.len(), 1);

    write_library(&app.paths, BROKEN);
    let task = app.reload_library();
    finish_reload(&mut app, task);
    assert!(app.library_problem.is_some());
    assert!(app.library_banner().is_some());
    // The cards still show, with no hidden flags, so the game launches.
    assert!(!app.cards[0].hidden);
    assert_eq!(app.filtered().len(), 1);
    // The rescan compares against what the reload showed.
    let scan = crate::rescan::read(&app.paths, &app.shown, false, None);
    assert!(app.shown.changes(scan, jiff::Timestamp::now()).is_none());

    write_library(&app.paths, "schema_version = 1\n");
    let task = app.reload_library();
    finish_reload(&mut app, task);
    assert_eq!(app.library_problem, None);
    assert!(app.library_banner().is_none());
}

#[test]
fn a_stale_reload_leaves_the_banner_alone() {
    let (_root, mut app) = loaded();
    let problem = wineo_core::library::Problem {
        reason: "expected `=`".into(),
        line: Some(2),
        newer_version: false,
    };
    let stale = app.library_generation;
    let _ = app.reload_library();
    let _ = app.update(Message::LibraryRead(stale, Ok(library()), Some(problem)));
    assert_eq!(app.library_problem, None);
}

#[test]
fn a_rescan_shows_and_clears_the_banner() {
    let (_root, mut app) = fixture();
    native(&app.paths, "game");
    let _ = app.update(Message::Loaded(
        crate::library::load_library(&app.paths).map_err(|error| error.to_string()),
    ));
    assert!(app.library_banner().is_none());

    write_library(&app.paths, BROKEN);
    let scan = crate::rescan::read(&app.paths, &app.shown, false, None);
    let scan = app
        .shown
        .changes(scan, jiff::Timestamp::now())
        .expect("the problem is new");
    let _ = app.update(Message::Rescanned(Box::new(scan)));
    assert!(app.library_banner().is_some());
    draw(&app);

    write_library(&app.paths, "schema_version = 1\n");
    let scan = crate::rescan::read(&app.paths, &app.shown, false, None);
    let scan = app
        .shown
        .changes(scan, jiff::Timestamp::now())
        .expect("the problem cleared");
    let _ = app.update(Message::Rescanned(Box::new(scan)));
    assert!(app.library_banner().is_none());
}
