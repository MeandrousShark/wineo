use std::fs;
use std::time::{Duration, Instant};

use iced::keyboard::{Key, Modifiers, key::Named};
use jiff::Timestamp;
use wineo_core::runtime::State;
use wineo_core::source::{Facts, Source};

use super::*;
use crate::grid_motion;
use crate::library::Card;
use crate::tests::card;

/// An app on its own throwaway HOME, so a saved view or a write can't leak
/// between tests.
fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    (root, App::boot(paths).0)
}

fn paths(root: &tempfile::TempDir) -> Paths {
    let home = root.path().to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
}

fn write_library(paths: &Paths, text: &str) {
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), text).unwrap();
}

fn at(seconds: i64) -> Option<Timestamp> {
    Some(Timestamp::from_second(1_700_000_000 + seconds).unwrap())
}

fn played(name: &str, hours: i64, last: i64) -> Card {
    let mut card = card(name, State::Inactive, hours * 3600);
    card.last_played_at = at(last);
    card
}

/// The plan's library: hades and balatro are favorites, notepad has never
/// been played, and steam-game's playtime couldn't be read.
fn library() -> Vec<Card> {
    let mut hades = played("hades", 40, 90);
    hades.source = Source::new(Facts {
        steam: true,
        ..Facts::default()
    });
    hades.favorite = true;
    hades.added = at(40);
    let mut papers = played("papers-please", 6, 60);
    papers.display_title = "Papers, Please".to_owned();
    papers.source = Source::new(Facts {
        gog_id: Some("1"),
        ..Facts::default()
    });
    papers.added = at(10);
    let mut notepad = card("notepad", State::Inactive, 0);
    notepad.added = at(30);
    let mut balatro = played("balatro", 20, 100);
    balatro.source = Source::new(Facts {
        native: true,
        ..Facts::default()
    });
    balatro.favorite = true;
    let mut demo = played("old-demo", 1, 10);
    demo.added = at(20);
    let mut steam = card("steam-game", State::Inactive, 0);
    steam.playtime_unknown = true;
    steam.source = hades.source;
    steam.added = at(50);
    let mut cards = vec![hades, papers, notepad, balatro, demo, steam];
    cards.sort_by(|a, b| a.name.cmp(&b.name));
    cards
}

fn loaded() -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    let _ = app.update(Message::Loaded(Ok(library())));
    (root, app)
}

fn shown(app: &App) -> Vec<&str> {
    app.filtered()
        .iter()
        .map(|card| card.name.as_str())
        .collect()
}

fn sort(app: &mut App, sort: SortKey, descending: bool) {
    if app.query.sort != sort {
        let _ = app.update(Message::Sort(Event::Pick(sort)));
    }
    if app.query.descending != descending {
        let _ = app.update(Message::Sort(Event::FlipDirection));
    }
}

fn key(app: &mut App, named: Named) {
    let _ = app.update(Message::KeyWithFocus(
        Key::Named(named),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
}

/// Runs the write in flight for one field, as its task would, and hands
/// the result back to the app.
fn finish_write(app: &mut App, field: fn(&Writes) -> Option<Field>) {
    let written = field(&app.view_writes).expect("a write in flight");
    let result = save(&app.paths, &app.shown, written);
    let _ = app.update(Message::Sort(Event::Saved(written, result)));
}

fn sort_write(writes: &Writes) -> Option<Field> {
    writes.sort.map(Field::Sort)
}

fn descending_write(writes: &Writes) -> Option<Field> {
    writes.descending.map(Field::Descending)
}

fn favorites_write(writes: &Writes) -> Option<Field> {
    writes.favorites_first.map(Field::FavoritesFirst)
}

#[test]
fn every_key_and_direction_orders_the_grid() {
    let (_root, mut app) = loaded();
    let cases: [(SortKey, bool, [&str; 6]); 10] = [
        (
            SortKey::Name,
            false,
            [
                "balatro",
                "hades",
                "notepad",
                "old-demo",
                "papers-please",
                "steam-game",
            ],
        ),
        (
            SortKey::Name,
            true,
            [
                "steam-game",
                "papers-please",
                "old-demo",
                "notepad",
                "hades",
                "balatro",
            ],
        ),
        (
            SortKey::Playtime,
            true,
            [
                "hades",
                "balatro",
                "papers-please",
                "old-demo",
                "notepad",
                "steam-game",
            ],
        ),
        (
            SortKey::Playtime,
            false,
            [
                "notepad",
                "old-demo",
                "papers-please",
                "balatro",
                "hades",
                "steam-game",
            ],
        ),
        (
            SortKey::LastPlayed,
            true,
            [
                "balatro",
                "hades",
                "papers-please",
                "old-demo",
                "notepad",
                "steam-game",
            ],
        ),
        (
            SortKey::LastPlayed,
            false,
            [
                "notepad",
                "old-demo",
                "papers-please",
                "hades",
                "balatro",
                "steam-game",
            ],
        ),
        (
            SortKey::Added,
            true,
            [
                "steam-game",
                "hades",
                "notepad",
                "old-demo",
                "papers-please",
                "balatro",
            ],
        ),
        (
            SortKey::Added,
            false,
            [
                "papers-please",
                "old-demo",
                "notepad",
                "hades",
                "steam-game",
                "balatro",
            ],
        ),
        (
            SortKey::Source,
            false,
            [
                "papers-please",
                "balatro",
                "hades",
                "steam-game",
                "notepad",
                "old-demo",
            ],
        ),
        (
            SortKey::Source,
            true,
            [
                "notepad",
                "old-demo",
                "hades",
                "steam-game",
                "balatro",
                "papers-please",
            ],
        ),
    ];
    for (key, descending, expected) in cases {
        sort(&mut app, key, descending);
        assert_eq!(shown(&app), expected, "{key:?} descending {descending}");
    }
}

#[test]
fn unknown_steam_playtime_is_last_both_ways() {
    let (_root, mut app) = loaded();
    for key in [SortKey::Playtime, SortKey::LastPlayed] {
        for descending in [false, true] {
            sort(&mut app, key, descending);
            assert_eq!(shown(&app).last(), Some(&"steam-game"), "{key:?}");
        }
    }
}

#[test]
fn favorites_first_leads_only_while_on() {
    let (_root, mut app) = loaded();
    sort(&mut app, SortKey::Playtime, false);
    let _ = app.update(Message::Sort(Event::ToggleFavoritesFirst));
    assert!(app.query.favorites_first);
    assert_eq!(
        shown(&app),
        [
            "balatro",
            "hades",
            "notepad",
            "old-demo",
            "papers-please",
            "steam-game"
        ]
    );
    let _ = app.update(Message::Sort(Event::ToggleFavoritesFirst));
    assert_eq!(
        shown(&app),
        [
            "notepad",
            "old-demo",
            "papers-please",
            "balatro",
            "hades",
            "steam-game"
        ]
    );
}

#[test]
fn search_narrows_the_sorted_order() {
    let (_root, mut app) = loaded();
    sort(&mut app, SortKey::Playtime, true);
    let _ = app.update(Message::SearchChanged("PAPERS, P".into()));
    assert_eq!(shown(&app), ["papers-please"]);
    let _ = app.update(Message::SearchChanged("e".into()));
    assert_eq!(
        shown(&app),
        [
            "hades",
            "papers-please",
            "old-demo",
            "notepad",
            "steam-game"
        ]
    );
}

#[test]
fn a_resort_keeps_the_selection_and_scrolls_to_it() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("old-demo".into()));
    assert!(app.panel_open);
    app.scroll_offset = 400.0;
    let generation = app.grid_motion.generation();
    sort(&mut app, SortKey::Playtime, true);
    assert_eq!(app.selected.as_deref(), Some("old-demo"));
    assert!(app.panel_open);
    // It's shown, so the grid stays put until the card reports where it
    // landed.
    assert_eq!(app.scroll_offset, 400.0);
    let now = app.grid_motion.generation();
    assert_ne!(now, generation);
    let bounds = grid_motion::CardBounds {
        card: iced::Rectangle::new(
            iced::Point::new(32.0, 1650.0),
            iced::Size::new(160.0, 300.0),
        ),
        viewport: iced::Rectangle::new(iced::Point::new(0.0, 50.0), iced::Size::new(800.0, 600.0)),
        offset: iced::Vector::new(0.0, 400.0),
    };
    let _ = app.update(Message::GridCardLocated(
        "old-demo".into(),
        generation,
        bounds,
    ));
    assert_eq!(app.scroll_offset, 400.0, "bounds from before the sort");
    let _ = app.update(Message::GridCardLocated("old-demo".into(), now, bounds));
    assert_eq!(app.scroll_offset, 1300.0);
}

#[test]
fn a_resort_without_a_shown_selection_goes_back_to_the_top() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("old-demo".into()));
    let _ = app.update(Message::SearchChanged("hades".into()));
    app.scroll_offset = 400.0;
    sort(&mut app, SortKey::Playtime, true);
    assert_eq!(app.selected.as_deref(), Some("old-demo"));
    assert_eq!(app.scroll_offset, 0.0);
}

#[test]
fn a_resort_snaps_cards_in_motion() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("hades".into()));
    assert!(app.grid_motion.active());
    let generation = app.grid_motion.generation();
    let _ = app.scroll_selection();
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some("hades"));
    let _ = app.update(Message::Sort(Event::FlipDirection));
    assert!(!app.grid_motion.active());
    assert_ne!(app.grid_motion.generation(), generation);
    // The old scroll-after-settle can't fire later with stale bounds.
    assert!(app.grid_scroll_after_settle.is_none());
    let _ = app.update(Message::GridTick(
        generation,
        Instant::now() + Duration::from_millis(310),
    ));
    assert!(!app.grid_motion.active());
    assert_eq!(app.grid_motion.progress(0), 1.0);
    assert_eq!(app.grid_motion.progress(5), 1.0);
}

#[test]
fn arrow_keys_move_through_the_sorted_order() {
    let (_root, mut app) = loaded();
    app.viewport_width = 10_000.0;
    sort(&mut app, SortKey::Playtime, true);
    let order: Vec<String> = shown(&app).into_iter().map(str::to_owned).collect();
    let _ = app.update(Message::Select(order[0].clone()));
    for expected in &order[1..] {
        key(&mut app, Named::ArrowRight);
        assert_eq!(app.selected.as_ref(), Some(expected));
    }
    key(&mut app, Named::ArrowLeft);
    assert_eq!(app.selected.as_ref(), Some(&order[4]));
}

#[test]
fn the_order_rebuilds_on_each_kind_of_change_and_not_on_an_unchanged_rescan() {
    let (_root, mut app) = fixture();
    let rebuilds = |app: &App| app.order.rebuilds;
    let before = rebuilds(&app);
    let _ = app.update(Message::Loaded(Ok(library())));
    assert_eq!(rebuilds(&app), before + 1, "new cards");

    // An unchanged rescan never reaches the app.
    let scan = crate::rescan::Scan {
        revisions: app.shown.revisions(),
        library: Some(Ok(library())),
        ..Default::default()
    };
    let scan = app.shown.current(scan);
    assert!(scan.library.is_some());
    assert!(app.shown.changes(scan, Timestamp::now()).is_none());
    let _ = app.update(Message::Loaded(Ok(library())));
    assert_eq!(rebuilds(&app), before + 1, "the same cards again");
    let mut running = library();
    running[0].state = State::Running;
    let _ = app.update(Message::Loaded(Ok(running)));
    assert_eq!(
        rebuilds(&app),
        before + 1,
        "a change the order doesn't read"
    );
    let mut starred = library();
    starred[4].favorite = true;
    let _ = app.update(Message::Loaded(Ok(starred)));
    assert_eq!(rebuilds(&app), before + 2, "a change it does read");

    let _ = app.update(Message::SearchChanged("a".into()));
    assert_eq!(rebuilds(&app), before + 3, "search");
    let _ = app.update(Message::Sort(Event::Pick(SortKey::Added)));
    assert_eq!(rebuilds(&app), before + 4, "key");
    let _ = app.update(Message::Sort(Event::Pick(SortKey::Added)));
    assert_eq!(rebuilds(&app), before + 4, "the same key");
    let _ = app.update(Message::Sort(Event::FlipDirection));
    assert_eq!(rebuilds(&app), before + 5, "direction");
    let _ = app.update(Message::Sort(Event::ToggleFavoritesFirst));
    assert_eq!(rebuilds(&app), before + 6, "favorites first");

    // Drawing and moving only read the cache.
    let _ = app.view();
    key(&mut app, Named::ArrowRight);
    assert_eq!(rebuilds(&app), before + 6);
    assert_eq!(app.order.misses.get(), 0);
}

#[test]
fn boot_applies_the_saved_view_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    let app = App::boot(paths.clone()).0;
    assert_eq!(app.query, crate::order::Query::default());
    assert!(!paths.library_file().exists());

    let text = "# kept\n[view]\nsort = \"playtime\"\ndescending = true\nfavorites_first = true\n";
    write_library(&paths, text);
    let modified = fs::metadata(paths.library_file())
        .unwrap()
        .modified()
        .unwrap();
    let mut app = App::boot(paths.clone()).0;
    assert_eq!(
        app.query,
        crate::order::Query {
            sort: SortKey::Playtime,
            descending: true,
            favorites_first: true,
        }
    );
    let _ = app.update(Message::Loaded(Ok(library())));
    assert_eq!(shown(&app)[..2], ["hades", "balatro"]);
    assert_eq!(fs::read_to_string(paths.library_file()).unwrap(), text);
    assert_eq!(
        fs::metadata(paths.library_file())
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert!(app.view_writes.sort.is_none());
    assert!(app.view_writes.descending.is_none());
    assert!(app.view_writes.favorites_first.is_none());
}

#[test]
fn a_rescan_or_another_writer_never_changes_the_open_view() {
    let (_root, mut app) = loaded();
    sort(&mut app, SortKey::Playtime, true);
    write_library(&app.paths, "[view]\nsort = \"name\"\ndescending = false\n");
    let scan = crate::rescan::read(&app.paths.clone(), &app.shown.clone(), false, None);
    let _ = app.update(Message::Rescanned(Box::new(scan)));
    let _ = app.update(Message::Refresh);
    let _ = app.update(Message::Loaded(Ok(library())));
    assert_eq!(app.query.sort, SortKey::Playtime);
    assert!(app.query.descending);
    assert_eq!(shown(&app)[0], "hades");
}

#[test]
fn each_control_writes_only_its_field() {
    let (_root, mut app) = loaded();
    let paths = app.paths.clone();
    // Another window saved these after this one opened.
    write_library(
        &paths,
        "[view]\n# theirs\ndescending = true\nfavorites_first = true\n\n[environments.hades]\nfavorite = true\n",
    );

    let _ = app.update(Message::Sort(Event::Pick(SortKey::Source)));
    assert!(app.view_writes.descending.is_none());
    assert!(app.view_writes.favorites_first.is_none());
    finish_write(&mut app, sort_write);
    assert_eq!(
        fs::read_to_string(paths.library_file()).unwrap(),
        "schema_version = 1\n[view]\n# theirs\ndescending = true\nfavorites_first = true\nsort = \"source\"\n\n[environments.hades]\nfavorite = true\n"
    );

    // This window's direction was ascending, so flipping it writes
    // descending, which the file already has, and nothing else.
    let _ = app.update(Message::Sort(Event::FlipDirection));
    assert!(app.view_writes.sort.is_none());
    let before = fs::read_to_string(paths.library_file()).unwrap();
    finish_write(&mut app, descending_write);
    assert_eq!(fs::read_to_string(paths.library_file()).unwrap(), before);

    let _ = app.update(Message::Sort(Event::FlipDirection));
    finish_write(&mut app, descending_write);
    let _ = app.update(Message::Sort(Event::ToggleFavoritesFirst));
    let _ = app.update(Message::Sort(Event::ToggleFavoritesFirst));
    finish_write(&mut app, favorites_write);
    // The second toggle moved on while the first was saving.
    finish_write(&mut app, favorites_write);
    assert!(app.view_writes.favorites_first.is_none());
    let saved = wineo_core::library::load(&paths);
    assert_eq!(
        saved.view,
        wineo_core::library::View {
            sort: SortKey::Source,
            descending: false,
            favorites_first: false,
        }
    );
    assert!(saved.environments["hades"].favorite);
    assert!(app.feature_notice.is_none());
}

#[test]
fn rapid_flips_end_with_the_file_matching_the_final_state() {
    let (_root, mut app) = loaded();
    for _ in 0..4 {
        let _ = app.update(Message::Sort(Event::FlipDirection));
    }
    assert!(!app.query.descending);
    // Only the first flip's write started; the rest wait for it.
    assert_eq!(app.view_writes.descending, Some(true));
    finish_write(&mut app, descending_write);
    assert!(wineo_core::library::load(&app.paths).view.descending);
    assert_eq!(app.view_writes.descending, Some(false));
    let _ = app.update(Message::Sort(Event::FlipDirection));
    let _ = app.update(Message::Sort(Event::FlipDirection));
    finish_write(&mut app, descending_write);
    assert_eq!(app.view_writes.descending, None);
    assert!(!wineo_core::library::load(&app.paths).view.descending);
}

#[test]
fn a_write_to_an_unreadable_file_keeps_the_view_says_why_and_refreshes_the_cache() {
    let (_root, mut app) = loaded();
    let broken = "[view\nsort = \"name\"\n";
    write_library(&app.paths, broken);
    let reads = app.shown.library_reads();
    let _ = app.update(Message::Sort(Event::Pick(SortKey::Playtime)));
    finish_write(&mut app, sort_write);
    assert_eq!(app.query.sort, SortKey::Playtime);
    assert_eq!(shown(&app)[0], "notepad");
    assert_eq!(
        app.feature_notice.as_ref().map(|(text, _)| text.as_str()),
        Some(
            "Couldn't save the sort: Can't save to library.toml: it can't be read. Fix it, or repair it in Checkup."
        )
    );
    assert_eq!(app.shown.library_reads(), reads + 1);
    assert_eq!(
        fs::read_to_string(app.paths.library_file()).unwrap(),
        broken
    );
    assert!(app.view_writes.sort.is_none());
}

#[test]
fn a_write_that_errors_keeps_the_view_says_why_and_refreshes_the_cache() {
    let (_root, mut app) = loaded();
    // A directory where the file should be fails the write.
    fs::create_dir_all(app.paths.library_file()).unwrap();
    let reads = app.shown.library_reads();
    let _ = app.update(Message::Sort(Event::FlipDirection));
    finish_write(&mut app, descending_write);
    assert!(app.query.descending);
    assert_eq!(shown(&app)[0], "steam-game");
    let (notice, _) = app.feature_notice.clone().expect("a notice");
    assert!(notice.starts_with("Couldn't save the sort: "), "{notice}");
    assert!(notice.contains("library.toml"), "{notice}");
    assert_eq!(app.shown.library_reads(), reads + 1);
    assert!(app.view_writes.descending.is_none());
}

#[test]
fn a_successful_write_refreshes_the_cache() {
    let (_root, mut app) = loaded();
    let reads = app.shown.library_reads();
    let _ = app.update(Message::Sort(Event::ToggleFavoritesFirst));
    finish_write(&mut app, favorites_write);
    assert_eq!(app.shown.library_reads(), reads + 1);
    assert!(wineo_core::library::load(&app.paths).view.favorites_first);
}

#[test]
fn the_menu_opens_at_the_anchor_and_escape_closes_it() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("hades".into()));
    let _ = app.update(Message::MenuAnchor(iced::Point::new(500.0, 30.0)));
    let _ = app.update(Message::Sort(Event::ToggleMenu));
    assert!(app.sort_menu_shown());
    assert_eq!(app.sort_menu_position, iced::Point::new(500.0, 30.0));
    let _ = app.view();

    // The grid behind it doesn't move, and Escape closes the menu before
    // the panel.
    key(&mut app, Named::ArrowRight);
    key(&mut app, Named::Enter);
    assert_eq!(app.selected.as_deref(), Some("hades"));
    assert!(app.operation.is_none());
    key(&mut app, Named::Escape);
    assert!(!app.sort_menu);
    assert!(app.panel_open);

    let _ = app.update(Message::Sort(Event::ToggleMenu));
    let _ = app.update(Message::Sort(Event::Pick(SortKey::Added)));
    assert!(!app.sort_menu);
    assert_eq!(app.query.sort, SortKey::Added);
    let _ = app.update(Message::Sort(Event::ToggleMenu));
    let _ = app.update(Message::Sort(Event::ToggleFavoritesFirst));
    assert!(app.sort_menu, "the toggle leaves the menu open");
    let _ = app.update(Message::Sort(Event::CloseMenu));
    assert!(!app.sort_menu);
}

#[test]
fn the_sort_menu_and_the_action_menu_replace_each_other() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Context("hades".into()));
    let _ = app.update(Message::Sort(Event::ToggleMenu));
    assert!(app.action_menu.is_none());
    assert!(app.sort_menu);
    let _ = app.update(Message::Context("hades".into()));
    assert!(!app.sort_menu);
    assert_eq!(app.action_menu.as_deref(), Some("hades"));
}

#[test]
fn leaving_the_library_closes_the_menu_and_overlays_block_the_controls() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Sort(Event::ToggleMenu));
    // The sidebar waits while the menu is open; anything that still leaves
    // the Library, like a page opened by its own work, closes it.
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    assert_eq!(app.section, crate::sidebar::Section::Library);
    app.section = crate::sidebar::Section::Trash;
    let _ = app.update(Message::Hover(None));
    assert!(!app.sort_menu);
    app.section = crate::sidebar::Section::Library;
    let _ = app.update(Message::Hover(None));
    assert!(!app.sort_menu_shown());

    let _ = app.update(Message::Add);
    assert!(app.flow.is_some());
    let _ = app.update(Message::Sort(Event::FlipDirection));
    let _ = app.update(Message::Sort(Event::ToggleMenu));
    assert!(!app.query.descending);
    assert!(!app.sort_menu);
    assert!(app.view_writes.descending.is_none());
}

#[test]
fn tab_and_arrows_cycle_the_menu_rows() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Sort(Event::ToggleMenu));
    let _ = focus_button::take_cycles();
    key(&mut app, Named::ArrowDown);
    key(&mut app, Named::Tab);
    key(&mut app, Named::ArrowUp);
    assert_eq!(focus_button::take_cycles().len(), 3);
}

#[test]
fn delete_leaves_the_selected_card_alone_while_the_menu_is_open() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("hades".into()));
    let _ = app.update(Message::Sort(Event::ToggleMenu));
    let _ = app.update(Message::Key(
        Key::Named(Named::Delete),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    // What the focus check behind Delete answers with nothing focused.
    let _ = app.update(Message::RemoveFocus(
        iced::event::Status::Ignored,
        false,
        false,
    ));
    key(&mut app, Named::Delete);
    assert!(app.removal.is_none());
    let _ = app.update(Message::Sort(Event::CloseMenu));
    let _ = app.update(Message::RemoveFocus(
        iced::event::Status::Ignored,
        false,
        false,
    ));
    assert!(app.removal.is_some());
}

/// What a task does once the runtime has it: the messages it sends back,
/// the widget operations it asks for, and whether it exits.
struct Ran {
    messages: Vec<Message>,
    operations: Vec<Box<dyn iced::advanced::widget::Operation>>,
    exits: bool,
}

/// How long a test waits for a write's result before failing. Well past a
/// config lock wait, so a slow machine never drops a real completion.
const SAVE_PATIENCE: Duration = Duration::from_secs(30);

/// Runs `task` the way the runtime would, on a runtime with a blocking pool,
/// until it has sent `saves` write results. Widget operations are kept only
/// when `keep` says so. There's no window to run them in, so a task waiting
/// on an operation's answer would wait forever: once every write result is
/// in, the run ends when nothing new arrives for a moment.
fn run_keeping(task: Task<Message>, keep: bool, saves: usize) -> Ran {
    use iced::futures::StreamExt;
    let mut ran = Ran {
        messages: Vec::new(),
        operations: Vec::new(),
        exits: false,
    };
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        assert_eq!(saves, 0, "the task does nothing, so no write can finish");
        return ran;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut seen = 0;
        loop {
            let waiting = seen < saves;
            let patience = if waiting {
                SAVE_PATIENCE
            } else {
                Duration::from_millis(500)
            };
            let action = match tokio::time::timeout(patience, stream.next()).await {
                Ok(Some(action)) => action,
                Ok(None) if waiting => panic!("the task ended after {seen} of {saves} writes"),
                Err(_) if waiting => {
                    panic!("only {seen} of {saves} writes finished within {SAVE_PATIENCE:?}")
                }
                _ => break,
            };
            match action {
                iced_runtime::Action::Output(message) => {
                    if matches!(message, Message::Sort(Event::Saved(..))) {
                        seen += 1;
                    }
                    ran.messages.push(message);
                }
                iced_runtime::Action::Widget(operation) if keep => ran.operations.push(operation),
                iced_runtime::Action::Exit => ran.exits = true,
                _ => {}
            }
        }
    });
    ran
}

/// `run_keeping` without the widget operations.
fn run(task: Task<Message>, saves: usize) -> Ran {
    run_keeping(task, false, saves)
}

/// The messages the sort's writes sent back.
fn saved(ran: Ran) -> Vec<Message> {
    ran.messages
        .into_iter()
        .filter(|message| matches!(message, Message::Sort(Event::Saved(..))))
        .collect()
}

fn press(named: Named) -> iced::Event {
    let key = Key::Named(named);
    iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: iced::keyboard::key::Physical::Unidentified(
            iced::keyboard::key::NativeCode::Unidentified,
        ),
        location: iced::keyboard::Location::Standard,
        modifiers: Modifiers::empty(),
        text: None,
        repeat: false,
    })
}

/// Lays out the window and runs `operations` in it, as the runtime does
/// after an update.
fn focus_after<'a>(
    app: &'a App,
    operations: Vec<Box<dyn iced::advanced::widget::Operation>>,
) -> crate::layout_probe::Laid<'a, Message> {
    let mut laid = crate::layout_probe::lay_out(app.view(), iced::Size::new(1200.0, 800.0));
    for operation in operations {
        laid.run(operation);
    }
    laid
}

#[test]
fn a_control_writes_off_the_ui_thread_through_its_task() {
    let (_root, mut app) = loaded();
    let task = app.update(Message::Sort(Event::FlipDirection));
    // The update only started the write.
    assert!(!app.paths.library_file().exists());
    assert_eq!(app.view_writes.descending, Some(true));
    let messages = saved(run(task, 1));
    assert!(matches!(
        messages.as_slice(),
        [Message::Sort(Event::Saved(Field::Descending(true), Ok(())))]
    ));
    for message in messages {
        assert_eq!(run(app.update(message), 0).messages.len(), 0);
    }
    assert!(app.view_writes.descending.is_none());
    assert!(wineo_core::library::load(&app.paths).view.descending);
}

#[test]
fn closing_waits_until_the_file_has_the_final_view() {
    let (_root, mut app) = loaded();
    let first = app.update(Message::Sort(Event::FlipDirection));
    let _ = app.update(Message::Sort(Event::FlipDirection));
    assert!(!app.query.descending);
    let close = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(app.closing);
    assert!(app.close_quietly, "nothing to cancel, so no prompt shows");
    assert!(!run(close, 0).exits);

    // The first write lands, and the final value still has to be written.
    let messages = saved(run(first, 1));
    assert_eq!(messages.len(), 1);
    let reissue = app.update(messages.into_iter().next().unwrap());
    assert_eq!(app.view_writes.descending, Some(false));
    let ran = run(reissue, 1);
    assert!(!ran.exits);
    let messages = saved(ran);
    assert_eq!(messages.len(), 1);

    let done = app.update(messages.into_iter().next().unwrap());
    assert!(run(done, 0).exits);
    assert!(!wineo_core::library::load(&app.paths).view.descending);
}

#[test]
fn closing_still_ends_when_the_write_fails() {
    let (_root, mut app) = loaded();
    write_library(&app.paths, "[view\n");
    let write = app.update(Message::Sort(Event::ToggleFavoritesFirst));
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    let messages = saved(run(write, 1));
    let done = app.update(messages.into_iter().next().unwrap());
    assert!(run(done, 0).exits);
}

#[test]
fn closing_with_nothing_to_save_exits_at_once() {
    let (_root, mut app) = loaded();
    assert!(
        run(
            app.update(Message::Window(iced::window::Event::CloseRequested)),
            0
        )
        .exits
    );
}

#[test]
fn opening_the_menu_from_the_keyboard_focuses_the_current_key() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Sort(Event::Pick(SortKey::Playtime)));
    let ran = run_keeping(
        iced::widget::operation::focus(iced::widget::Id::new(SORT_BUTTON_ID)),
        true,
        0,
    );
    let mut laid = focus_after(&app, ran.operations);
    assert_eq!(laid.focused(), [iced::widget::Id::new(SORT_BUTTON_ID)]);
    let messages = laid.event(&press(Named::Enter));
    drop(laid);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Sort(Event::ToggleMenu)))
    );

    let ran = run_keeping(app.update(Message::Sort(Event::ToggleMenu)), true, 0);
    assert!(app.sort_menu_shown());
    let mut laid = focus_after(&app, ran.operations);
    assert_eq!(
        laid.focused(),
        [iced::widget::Id::new(row_id(SortKey::Playtime))]
    );
    // A second Enter picks the focused row rather than closing the menu.
    let messages = laid.event(&press(Named::Enter));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Sort(Event::Pick(SortKey::Playtime))))
    );
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, Message::Sort(Event::ToggleMenu)))
    );
}

/// A removed environment, so there's an Undo toast.
fn with_toast(app: &mut App) {
    let config = app.paths.environment_config("gone");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "kind = 'native'\n").unwrap();
    let entry = wineo_core::trash::remove(&app.paths, "gone", false).unwrap();
    app.toast = Some(crate::removal::Toast::new(entry));
}

#[test]
fn shortcuts_wait_while_the_menu_is_open() {
    let (_root, mut app) = loaded();
    with_toast(&mut app);
    let _ = app.update(Message::Sort(Event::ToggleMenu));

    let _ = app.update(Message::Key(
        Key::Character("n".into()),
        Modifiers::CTRL,
        iced::event::Status::Ignored,
    ));
    assert!(app.flow.is_none(), "Ctrl+N");

    crate::shortcuts::tests::press(&mut app, "2", Modifiers::CTRL);
    assert_eq!(app.section, crate::sidebar::Section::Library, "Ctrl+2");

    // The shortcut layer publishes Undo before any key handler sees Ctrl+Z.
    let _ = app.update(Message::Undo);
    let _ = app.update(Message::Key(
        Key::Character("z".into()),
        Modifiers::CTRL,
        iced::event::Status::Ignored,
    ));
    assert!(app.change.is_none(), "Undo");
    assert!(app.sort_menu_shown());
}

#[test]
fn tab_cycles_the_menu_rows_even_with_an_undo_toast() {
    let (_root, mut app) = loaded();
    with_toast(&mut app);
    let _ = app.update(Message::Sort(Event::ToggleMenu));
    let task = app.update(Message::Key(
        Key::Named(Named::Tab),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    let focused = focus_after(&app, run_keeping(task, true, 0).operations).focused();
    assert_eq!(focused, [iced::widget::Id::new(row_id(SortKey::Name))]);
    assert_ne!(focused, [iced::widget::Id::new(focus_button::UNDO_ID)]);
}

#[test]
fn closing_gives_up_on_a_stalled_write_after_the_bound() {
    let (_root, mut app) = loaded();
    let write = app.update(Message::Sort(Event::FlipDirection));
    let _ = app.update(Message::Sort(Event::FlipDirection));
    let asked = Instant::now();
    assert!(
        !run(
            app.update(Message::Window(iced::window::Event::CloseRequested)),
            0
        )
        .exits
    );
    let due = app.close_wake.due().expect("a wakeup for the bound");
    assert!(due >= asked + crate::app_window::SORT_SAVE_WAIT);
    assert!(due <= Instant::now() + crate::app_window::SORT_SAVE_WAIT);

    // A rescan stalled while holding the caches keeps the write from
    // finishing.
    let watch = app.shown.clone();
    let held = watch.hold_caches();
    let writer = std::thread::spawn(move || saved(run(write, 1)));
    let stale = app.update(Message::CloseWaitOver(app.close_wake.generation() - 1));
    assert!(!run(stale, 0).exits);
    let over = app.update(Message::CloseWaitOver(app.close_wake.generation()));
    assert!(run(over, 0).exits);

    // The write finishing after the window chose to exit only drops: no
    // rewrite of the newer value and no notice.
    drop(held);
    let late = writer.join().unwrap();
    assert_eq!(late.len(), 1);
    for message in late {
        assert!(run(app.update(message), 0).messages.is_empty());
    }
    assert!(!app.view_writes.pending());
    assert!(app.feature_notice.is_none());
}

#[test]
fn a_second_close_while_waiting_exits_at_once() {
    let (_root, mut app) = loaded();
    let _write = app.update(Message::Sort(Event::FlipDirection));
    let close = || Message::Window(iced::window::Event::CloseRequested);
    assert!(!run(app.update(close()), 0).exits);
    assert!(run(app.update(close()), 0).exits);
    assert!(app.view_writes.pending(), "the write was given up on");
}

#[test]
fn a_close_that_cancels_foreground_work_still_bounds_the_sort_wait() {
    let (_root, mut app) = loaded();
    let _stalled = app.update(Message::Sort(Event::FlipDirection));
    let _ = app.update(Message::Select("hades".into()));
    app.operation = Some(crate::Operation {
        name: "hades".into(),
        kind: crate::OperationKind::Start,
        token: Some(wineo_core::cancel::Token::new()),
        cancelled: false,
        status: "Starting...".into(),
        install_done: None,
        installing: false,
        after_hook: false,
    });
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(app.close_prompt && !app.close_quietly);
    assert!(!run(app.update(Message::CancelAndClose), 0).exits);
    assert!(app.closing);
    assert!(app.close_by.is_none(), "the launch is still running");

    let asked = Instant::now();
    let finished = app.update(Message::StartFinished(
        "hades".into(),
        Err(crate::panel::PanelError {
            text: "canceled".into(),
            cleanup: false,
            output: None,
        }),
    ));
    assert!(!run(finished, 0).exits);
    assert!(app.operation.is_none());
    let due = app.close_wake.due().expect("the sort wait is bounded");
    assert!(due >= asked + crate::app_window::SORT_SAVE_WAIT);
    let over = app.update(Message::CloseWaitOver(app.close_wake.generation()));
    assert!(run(over, 0).exits);
}

/// A launch, Add, and their keys, which start foreground work.
fn try_to_start_work(app: &mut App) {
    let _ = app.update(Message::Play);
    let _ = app.update(Message::Add);
    let _ = app.update(Message::Key(
        Key::Character("n".into()),
        Modifiers::CTRL,
        iced::event::Status::Ignored,
    ));
    key(app, Named::Enter);
}

#[test]
fn a_quiet_close_takes_the_input_while_it_waits() {
    // Without a close, the same input starts a launch.
    let (_root, mut open) = loaded();
    let _ = open.update(Message::Select("hades".into()));
    try_to_start_work(&mut open);
    assert!(open.operation.is_some());

    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("hades".into()));
    let _stalled = app.update(Message::Sort(Event::FlipDirection));
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(app.close_quietly && app.close_by.is_some());
    try_to_start_work(&mut app);
    let _ = app.update(Message::Sort(Event::FlipDirection));
    crate::shortcuts::tests::press(&mut app, "3", Modifiers::CTRL);
    assert!(app.operation.is_none());
    assert!(app.flow.is_none());
    assert!(app.query.descending);
    assert_eq!(app.section, crate::sidebar::Section::Library);
    // An unseen layer takes the mouse; no prompt is drawn.
    let _ = app.view();
}

#[test]
fn the_direction_tooltip_names_the_current_order_of_each_key() {
    let cases = [
        (SortKey::Name, false, "A to Z"),
        (SortKey::Name, true, "Z to A"),
        (SortKey::Source, false, "Sources A to Z"),
        (SortKey::Source, true, "Sources Z to A"),
        (SortKey::Playtime, true, "Most played first"),
        (SortKey::Playtime, false, "Least played first"),
        (SortKey::LastPlayed, true, "Most recent first"),
        (SortKey::LastPlayed, false, "Least recent first"),
        (SortKey::Added, true, "Newest first"),
        (SortKey::Added, false, "Oldest first"),
    ];
    for (key, descending, expected) in cases {
        assert_eq!(direction_label(key, descending), expected);
    }
}

#[test]
fn the_direction_button_still_flips_from_the_keyboard_with_its_tooltip() {
    let (_root, app) = loaded();
    let ran = run_keeping(
        iced::widget::operation::focus(iced::widget::Id::new(DIRECTION_BUTTON_ID)),
        true,
        0,
    );
    let mut laid = focus_after(&app, ran.operations);
    assert_eq!(laid.focused(), [iced::widget::Id::new(DIRECTION_BUTTON_ID)]);
    let messages = laid.event(&press(Named::Enter));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Sort(Event::FlipDirection)))
    );
}
