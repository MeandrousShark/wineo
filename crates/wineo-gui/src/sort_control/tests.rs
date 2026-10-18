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
        Some("Can't save to library.toml: it can't be read. Fix it, or repair it in Checkup.")
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
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    assert!(!app.sort_menu);
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
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
