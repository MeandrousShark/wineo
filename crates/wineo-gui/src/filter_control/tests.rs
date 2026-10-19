use std::fs;

use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::paths::Paths;
use wineo_core::runtime::State;
use wineo_core::source::{Facts, Source};

use super::*;
use crate::grid_motion;
use crate::library::Card;
use crate::sort_control::tests::{answers, focus_after, press};
use crate::tests::card;

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    (root, App::boot(paths).0)
}

fn tagged(name: &str, source: Facts<'_>, tags: &[&str]) -> Card {
    let mut card = card(name, State::Inactive, 0);
    card.source = Source::new(source);
    card.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
    card
}

const STEAM: Facts<'static> = Facts {
    steam: true,
    native: false,
    gog_id: None,
    heroic_id: None,
    store_hint: None,
};
const GOG: Facts<'static> = Facts {
    steam: false,
    native: false,
    gog_id: Some("1"),
    heroic_id: None,
    store_hint: None,
};
const NATIVE: Facts<'static> = Facts {
    steam: false,
    native: true,
    gog_id: None,
    heroic_id: None,
    store_hint: None,
};
const WINDOWS: Facts<'static> = Facts {
    steam: false,
    native: false,
    gog_id: None,
    heroic_id: None,
    store_hint: None,
};

/// hades and old-demo are tagged rpg, old-demo is hidden, and notepad is
/// the only tool.
pub(crate) fn library() -> Vec<Card> {
    let mut demo = tagged("old-demo", WINDOWS, &["game", "rpg"]);
    demo.hidden = true;
    let mut cards = vec![
        tagged("hades", STEAM, &["game", "rpg"]),
        tagged("papers-please", GOG, &["game", "Indie"]),
        tagged("notepad", WINDOWS, &["tool"]),
        tagged("balatro", NATIVE, &["game", "Indie"]),
        demo,
    ];
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

fn filter(app: &mut App, event: Event) -> Task<Message> {
    app.update(Message::Filter(event))
}

fn key(app: &mut App, named: Named) {
    let _ = app.update(Message::KeyWithFocus(
        Key::Named(named),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
}

fn counts(app: &App) -> crate::order::Counts {
    app.order.counts(app.order_inputs())
}

/// The widget operations `task` asks for, without answering them, so a
/// task chained after one never starts.
fn run_operations(task: Task<Message>) -> Vec<Box<dyn iced::advanced::widget::Operation>> {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let mut operations = Vec::new();
    iced::futures::executor::block_on(async {
        while let Some(Some(action)) = iced::futures::FutureExt::now_or_never(stream.next()) {
            if let iced_runtime::Action::Widget(operation) = action {
                operations.push(operation);
            }
        }
    });
    operations
}

/// Runs `task` against `laid` the way the runtime does, answering each
/// widget operation as it comes so chained work follows, and returns the
/// messages it sent.
fn drive(laid: &mut crate::layout_probe::Laid<'_, Message>, task: Task<Message>) -> Vec<Message> {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut messages = Vec::new();
    runtime.block_on(async {
        while let Ok(Some(action)) =
            tokio::time::timeout(std::time::Duration::from_millis(500), stream.next()).await
        {
            match action {
                iced_runtime::Action::Output(message) => messages.push(message),
                iced_runtime::Action::Widget(operation) => laid.run(operation),
                _ => {}
            }
        }
    });
    messages
}

#[test]
fn hidden_cards_leave_the_grid_and_search_unless_show_hidden_is_on() {
    let (_root, mut app) = loaded();
    assert_eq!(
        shown(&app),
        ["balatro", "hades", "notepad", "papers-please"]
    );
    let _ = app.update(Message::SearchChanged("demo".into()));
    assert!(shown(&app).is_empty());

    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert_eq!(shown(&app), ["old-demo"]);
    let _ = app.update(Message::SearchChanged(String::new()));
    assert_eq!(
        shown(&app),
        ["balatro", "hades", "notepad", "old-demo", "papers-please"]
    );
}

#[test]
fn store_filters_keep_any_chosen_store() {
    let (_root, mut app) = loaded();
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    assert_eq!(shown(&app), ["hades"]);
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    assert_eq!(shown(&app), ["hades", "papers-please"]);
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    assert_eq!(shown(&app), ["papers-please"]);
}

#[test]
fn platform_filters_keep_any_chosen_platform_and_skip_steam_games() {
    let (_root, mut app) = loaded();
    let _ = filter(&mut app, Event::TogglePlatform("Windows"));
    assert_eq!(shown(&app), ["notepad", "papers-please"]);
    let _ = filter(&mut app, Event::TogglePlatform("Native"));
    assert_eq!(shown(&app), ["balatro", "notepad", "papers-please"]);
    let _ = filter(&mut app, Event::TogglePlatform("Windows"));
    assert_eq!(shown(&app), ["balatro"]);
}

/// A native GOG game like RimWorld, a proxied Steam game, a Windows GOG
/// game, and a game with neither store nor platform filters to match.
fn mixed() -> Vec<Card> {
    let native_gog = Facts {
        native: true,
        gog_id: Some("2"),
        ..Facts::default()
    };
    let mut cards = vec![
        tagged("rimworld", native_gog, &["game"]),
        tagged("hades", STEAM, &["game"]),
        tagged("papers-please", GOG, &["game", "Indie"]),
        tagged("notepad", WINDOWS, &["tool"]),
        tagged("balatro", NATIVE, &["game", "Indie"]),
    ];
    cards.sort_by(|a, b| a.name.cmp(&b.name));
    cards
}

fn mixed_app() -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    let _ = app.update(Message::Loaded(Ok(mixed())));
    (root, app)
}

#[test]
fn a_native_gog_game_matches_both_gog_and_native() {
    let (_root, mut app) = mixed_app();
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    assert_eq!(shown(&app), ["papers-please", "rimworld"]);
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    let _ = filter(&mut app, Event::TogglePlatform("Native"));
    assert_eq!(shown(&app), ["balatro", "rimworld"]);
    // Both groups have to match.
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    assert_eq!(shown(&app), ["rimworld"]);
}

#[test]
fn a_proxied_steam_game_matches_steam_but_neither_platform() {
    let (_root, mut app) = mixed_app();
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    assert_eq!(shown(&app), ["hades"]);
    let _ = filter(&mut app, Event::TogglePlatform("Windows"));
    assert!(shown(&app).is_empty());
    let _ = filter(&mut app, Event::TogglePlatform("Windows"));
    let _ = filter(&mut app, Event::TogglePlatform("Native"));
    assert!(shown(&app).is_empty());
    // Every platform checked still leaves the Steam game out.
    let _ = filter(&mut app, Event::Clear);
    let _ = filter(&mut app, Event::TogglePlatform("Native"));
    let _ = filter(&mut app, Event::TogglePlatform("Windows"));
    assert_eq!(
        shown(&app),
        ["balatro", "notepad", "papers-please", "rimworld"]
    );
}

#[test]
fn a_game_with_no_store_matches_no_store_entry() {
    let (_root, mut app) = mixed_app();
    for store in ["Steam", "GOG", "Epic", "Amazon"] {
        let _ = filter(&mut app, Event::ToggleStore(store));
        assert!(
            !shown(&app).contains(&"notepad") && !shown(&app).contains(&"balatro"),
            "{store}"
        );
        let _ = filter(&mut app, Event::ToggleStore(store));
    }
}

#[test]
fn store_platform_and_tag_filters_all_have_to_match() {
    let (_root, mut app) = mixed_app();
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    let _ = filter(&mut app, Event::TogglePlatform("Windows"));
    assert_eq!(shown(&app), ["papers-please"]);
    let _ = filter(&mut app, Event::ToggleTag("Indie".into()));
    assert_eq!(shown(&app), ["papers-please"]);
    let _ = filter(&mut app, Event::ToggleTag("Indie".into()));
    let _ = filter(&mut app, Event::ToggleTag("tool".into()));
    assert!(shown(&app).is_empty());
    // Tags within the group are any-of, and stores still narrow them.
    let _ = filter(&mut app, Event::ToggleTag("Indie".into()));
    assert_eq!(shown(&app), ["papers-please"]);
}

#[test]
fn tag_filters_keep_cards_with_any_chosen_tag_within_the_platforms() {
    let (_root, mut app) = loaded();
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    assert_eq!(shown(&app), ["hades"]);
    let _ = filter(&mut app, Event::ToggleTag("tool".into()));
    assert_eq!(shown(&app), ["hades", "notepad"]);
    // Platforms and tags both have to match.
    let _ = filter(&mut app, Event::TogglePlatform("Windows"));
    assert_eq!(shown(&app), ["notepad"]);
    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert_eq!(shown(&app), ["notepad", "old-demo"]);
}

#[test]
fn search_matches_tags_as_well_as_titles_and_identifiers() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::SearchChanged("RPG".into()));
    assert_eq!(shown(&app), ["hades"]);
    let _ = app.update(Message::SearchChanged("indie".into()));
    assert_eq!(shown(&app), ["balatro", "papers-please"]);
    let _ = app.update(Message::SearchChanged("pad".into()));
    assert_eq!(shown(&app), ["notepad"]);
}

#[test]
fn filtered_cards_sort_like_everything_else() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Sort(crate::sort_control::Event::FlipDirection));
    let _ = filter(&mut app, Event::ToggleTag("game".into()));
    assert_eq!(shown(&app), ["papers-please", "hades", "balatro"]);
}

#[test]
fn the_rebuild_counts_shown_hidden_and_hidden_matches() {
    let (_root, mut app) = loaded();
    let expect = |shown, hidden_matching| crate::order::Counts {
        shown,
        hidden_matching,
        hidden: 1,
    };
    assert_eq!(counts(&app), expect(4, 1));
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    assert_eq!(counts(&app), expect(1, 1));
    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert_eq!(counts(&app), expect(2, 0));
    let _ = filter(&mut app, Event::ToggleShowHidden);
    let _ = app.update(Message::SearchChanged("tool".into()));
    assert_eq!(counts(&app), expect(0, 0));
    // Every read came from the rebuild, not a count on the spot.
    let rebuilds = app.order.rebuilds;
    let _ = app.view();
    let _ = app.empty_state();
    let _ = filter(&mut app, Event::ToggleMenu);
    let _ = app.view();
    assert_eq!(app.order.rebuilds, rebuilds);
    assert_eq!(app.order.misses.get(), 0);
}

#[test]
fn filter_changes_and_new_hidden_flags_or_tags_rebuild_the_order() {
    let (_root, mut app) = loaded();
    let rebuilds = |app: &App| app.order.rebuilds;
    let before = rebuilds(&app);
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    assert_eq!(rebuilds(&app), before + 1, "source");
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    assert_eq!(rebuilds(&app), before + 2, "tag");
    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert_eq!(rebuilds(&app), before + 3, "show hidden");
    let _ = filter(&mut app, Event::Clear);
    assert_eq!(rebuilds(&app), before + 4, "clear");

    // A rescan with the same cards keeps the order; a hidden flag or a tag
    // that changed rebuilds it.
    let _ = app.update(Message::Loaded(Ok(library())));
    assert_eq!(rebuilds(&app), before + 4);
    let mut unhidden = library();
    unhidden[3].hidden = false;
    let _ = app.update(Message::Loaded(Ok(unhidden)));
    assert_eq!(rebuilds(&app), before + 5, "hidden flag");
    let mut retagged = library();
    retagged[3].hidden = false;
    retagged[0].tags.push("cards".into());
    let _ = app.update(Message::Loaded(Ok(retagged)));
    assert_eq!(rebuilds(&app), before + 6, "tags");
    assert!(
        app.order
            .options(app.order_inputs())
            .tags
            .contains(&"cards".to_owned())
    );
    assert_eq!(app.order.misses.get(), 0);
}

#[test]
fn filters_are_never_saved_and_reset_on_launch() {
    let (_root, mut app) = loaded();
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    let _ = filter(&mut app, Event::ToggleShowHidden);
    let _ = filter(&mut app, Event::Clear);
    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert!(!app.paths.library_file().exists());
    let relaunched = App::boot(app.paths.clone()).0;
    assert_eq!(relaunched.filters, crate::order::Filters::default());
}

#[test]
fn filters_leave_an_existing_file_alone() {
    let (_root, mut app) = loaded();
    fs::create_dir_all(app.paths.library_file().parent().unwrap()).unwrap();
    let original = "schema_version = 1\n[view]\nsort = \"name\"\n";
    fs::write(app.paths.library_file(), original).unwrap();
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    let _ = filter(&mut app, Event::ToggleShowHidden);
    let _ = filter(&mut app, Event::Clear);
    assert_eq!(
        fs::read_to_string(app.paths.library_file()).unwrap(),
        original
    );
}

#[test]
fn the_button_counts_store_platform_and_tag_filters() {
    let (_root, mut app) = loaded();
    assert_eq!(button_label(&app), "Filters");
    let _ = app.update(Message::SearchChanged("a".into()));
    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert_eq!(button_label(&app), "Filters");
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    let _ = filter(&mut app, Event::ToggleTag("tool".into()));
    assert_eq!(button_label(&app), "Filters (3)");
    let _ = filter(&mut app, Event::TogglePlatform("Windows"));
    let _ = filter(&mut app, Event::TogglePlatform("Native"));
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    assert_eq!(button_label(&app), "Filters (6)");
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    assert_eq!(button_label(&app), "Filters (5)");
}

#[test]
fn clear_filters_clears_search_stores_platforms_and_tags_but_not_the_sort_or_show_hidden() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Sort(crate::sort_control::Event::FlipDirection));
    let _ = app.update(Message::SearchChanged("zzz".into()));
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    let _ = filter(&mut app, Event::TogglePlatform("Windows"));
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert!(app.filters_clearable());
    let _ = filter(&mut app, Event::Clear);
    assert!(app.search.is_empty());
    assert!(app.filters.stores.is_empty());
    assert!(app.filters.platforms.is_empty());
    assert!(app.filters.tags.is_empty());
    assert!(app.filters.show_hidden);
    assert!(app.query.descending);
    assert!(!app.filters_clearable());
}

#[test]
fn clear_filters_in_the_popup_closes_it_and_focuses_the_filters_button() {
    let (_root, mut app) = loaded();
    let _ = filter(&mut app, Event::ToggleMenu);
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    let task = filter(&mut app, Event::Clear);
    assert!(!app.filter_menu);
    let mut laid = focus_after(&app, run_operations(task));
    assert_eq!(laid.focused(), [iced::widget::Id::new(FILTER_BUTTON_ID)]);
}

#[test]
fn the_popup_offers_the_library_stores_platforms_and_tags_in_order() {
    let (_root, app) = loaded();
    assert_eq!(
        app.offered(),
        Offered {
            stores: vec!["GOG", "Steam"],
            platforms: vec!["Native", "Windows"],
            tags: ["game", "Indie", "rpg", "tool"].map(str::to_owned).to_vec(),
        }
    );
}

#[test]
fn the_popup_lists_only_present_values_and_omits_empty_groups() {
    let (_root, mut app) = fixture();
    // Only Steam games: a store and no platform.
    let steam_only = vec![tagged("hades", STEAM, &[])];
    let _ = app.update(Message::Loaded(Ok(steam_only)));
    assert_eq!(
        app.offered(),
        Offered {
            stores: vec!["Steam"],
            platforms: vec![],
            tags: vec![],
        }
    );
    assert_eq!(app.filter_menu_ids()[0], "library-filter-store-steam");
    let ids = app.filter_menu_ids();
    assert!(!ids.iter().any(|id| id.contains("platform")), "{ids:?}");
    let _ = filter(&mut app, Event::ToggleMenu);
    let _ = app.view();

    // Only store-less games: platforms and no store.
    let plain = vec![
        tagged("notepad", WINDOWS, &[]),
        tagged("balatro", NATIVE, &[]),
    ];
    let _ = app.update(Message::Loaded(Ok(plain)));
    let offered = app.offered();
    assert!(offered.stores.is_empty());
    assert_eq!(offered.platforms, ["Native", "Windows"]);
    assert_eq!(
        app.filter_menu_ids()[..2],
        [
            "library-filter-platform-native",
            "library-filter-platform-windows"
        ]
    );
}

#[test]
fn the_popup_rows_run_stores_then_platforms_then_tags() {
    let (_root, app) = loaded();
    let ids = app.filter_menu_ids();
    assert_eq!(
        ids[..6],
        [
            "library-filter-store-gog",
            "library-filter-store-steam",
            "library-filter-platform-native",
            "library-filter-platform-windows",
            "library-filter-tag-game",
            "library-filter-tag-Indie",
        ]
    );
}

#[test]
fn a_chosen_filter_the_library_lost_stays_on_and_offered() {
    let (_root, mut app) = loaded();
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    let _ = filter(&mut app, Event::ToggleTag("tool".into()));
    let lost: Vec<Card> = library()
        .into_iter()
        .filter(|card| card.name != "hades" && card.name != "notepad")
        .collect();
    let _ = app.update(Message::Loaded(Ok(lost)));
    assert!(app.filters.stores.contains("Steam"));
    assert!(app.filters.tags.contains("tool"));
    let offered = app.offered();
    assert_eq!(offered.stores, ["GOG", "Steam"]);
    assert_eq!(offered.platforms, ["Native", "Windows"]);
    assert_eq!(offered.tags, ["game", "Indie", "rpg", "tool"]);
    assert!(shown(&app).is_empty());
    // It can still be turned off from the popup.
    let _ = filter(&mut app, Event::ToggleTag("tool".into()));
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    assert_eq!(shown(&app), ["balatro", "papers-please"]);
}

#[test]
fn the_popup_opens_at_the_anchor_takes_the_keys_and_escape_closes_it() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("hades".into()));
    let _ = app.update(Message::MenuAnchor(iced::Point::new(600.0, 30.0)));
    let _ = filter(&mut app, Event::ToggleMenu);
    assert!(app.filter_menu_shown());
    assert_eq!(app.filter_menu_position, iced::Point::new(600.0, 30.0));
    let _ = app.view();

    // The grid behind it doesn't move, launch, or remove, the sidebar
    // waits, and Escape closes the popup before the panel.
    key(&mut app, Named::ArrowRight);
    key(&mut app, Named::Enter);
    let _ = app.update(Message::Key(
        Key::Named(Named::Delete),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    let _ = app.update(Message::RemoveFocus(
        iced::event::Status::Ignored,
        false,
        false,
    ));
    key(&mut app, Named::Delete);
    assert!(app.removal.is_none());
    assert_eq!(app.selected.as_deref(), Some("hades"));
    assert!(app.operation.is_none());
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    assert_eq!(app.section, crate::sidebar::Section::Library);

    let task = app.update(Message::Key(
        Key::Named(Named::Escape),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    assert!(!app.filter_menu);
    assert!(app.panel_open);
    let focused = focus_after(&app, run_operations(task)).focused();
    assert_eq!(focused, [iced::widget::Id::new(FILTER_BUTTON_ID)]);

    // Toggles leave it open; a click outside closes it.
    let _ = filter(&mut app, Event::ToggleMenu);
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert!(app.filter_menu);
    let _ = filter(&mut app, Event::CloseMenu);
    assert!(!app.filter_menu);

    // Delete works again once it's closed.
    let _ = app.update(Message::RemoveFocus(
        iced::event::Status::Ignored,
        false,
        false,
    ));
    assert!(app.removal.is_some());
}

#[test]
fn only_one_menu_is_open_at_a_time() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Context("hades".into()));
    let _ = filter(&mut app, Event::ToggleMenu);
    assert!(app.filter_menu);
    assert!(app.action_menu.is_none());

    let _ = app.update(Message::Sort(crate::sort_control::Event::ToggleMenu));
    assert!(app.sort_menu);
    assert!(!app.filter_menu);

    let _ = filter(&mut app, Event::ToggleMenu);
    assert!(app.filter_menu);
    assert!(!app.sort_menu);

    let _ = app.update(Message::Context("hades".into()));
    assert!(!app.filter_menu);
    assert_eq!(app.action_menu.as_deref(), Some("hades"));
}

#[test]
fn tab_and_arrows_cycle_the_popup_rows() {
    let (_root, mut app) = loaded();
    let _ = filter(&mut app, Event::ToggleMenu);
    let _ = focus_button::take_cycles();
    key(&mut app, Named::ArrowDown);
    key(&mut app, Named::Tab);
    key(&mut app, Named::ArrowUp);
    assert_eq!(focus_button::take_cycles().len(), 3);
    // Clear filters joins the rows only when there's something to clear.
    assert!(!app.filter_menu_ids().contains(&CLEAR_ID.to_owned()));
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    let ids = app.filter_menu_ids();
    assert_eq!(
        ids.first().map(String::as_str),
        Some("library-filter-store-gog")
    );
    assert!(ids.contains(&"library-filter-tag-rpg".to_owned()));
    assert_eq!(ids.last().map(String::as_str), Some(CLEAR_ID));
}

/// The Filters button's bounds in the laid out window.
fn filter_button_bounds(app: &App) -> Rectangle {
    let mut laid = crate::layout_probe::lay_out(app.view(), iced::Size::new(1200.0, 800.0));
    let found = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = found.clone();
    laid.run(Box::new(iced::advanced::widget::operation::map(
        crate::library_menu::bounds_of(FILTER_BUTTON_ID),
        move |bounds| *sink.lock().unwrap() = bounds,
    )));
    let bounds = *found.lock().unwrap();
    bounds.expect("the Filters button registers its bounds")
}

#[test]
fn the_popup_opened_from_the_keyboard_sits_under_the_filters_button() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::MenuAnchor(iced::Point::new(900.0, 600.0)));
    let task = filter(&mut app, Event::ToggleMenuFromKeyboard);
    let mut focus = Vec::new();
    for message in answers(&app, task) {
        focus.push(app.update(message));
    }
    assert!(app.filter_menu_shown());
    let button = filter_button_bounds(&app);
    assert_eq!(
        app.filter_menu_position,
        iced::Point::new(button.x - 4.0, button.y + button.height)
    );
    // Focus moves to the first row, so Enter toggles it instead of closing.
    let mut laid = crate::layout_probe::lay_out(app.view(), iced::Size::new(1200.0, 800.0));
    drive(&mut laid, Task::batch(focus));
    assert_eq!(
        laid.focused(),
        [iced::widget::Id::new("library-filter-store-gog")]
    );
    let messages = laid.event(&press(Named::Enter));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Filter(Event::ToggleStore("GOG"))))
    );
}

#[test]
fn the_popup_stays_inside_a_short_window_and_scrolls() {
    let (_root, mut app) = loaded();
    app.window_size = iced::Size::new(900.0, 260.0);
    let _ = app.update(Message::MenuAnchor(iced::Point::new(10.0, 40.0)));
    let _ = filter(&mut app, Event::ToggleMenu);
    let probe = crate::layout_probe::probe(&mut app.view(), app.window_size);
    let (viewport, content) = probe
        .scrollables
        .iter()
        .copied()
        .find(|(viewport, _)| viewport.width < ACTION_MENU_WIDTH)
        .expect("the popup scrolls");
    assert!(
        viewport.y + viewport.height <= 260.0 - ACTION_MENU_MARGIN,
        "{viewport:?}"
    );
    assert!(content.height > viewport.height, "{content:?}");
}

fn press_filters(app: &mut App) -> u64 {
    let _ = filter(app, Event::ToggleMenuFromKeyboard);
    app.filter_opening
        .pending
        .expect("a request waits for the button")
}

fn reply(app: &mut App, request: u64) {
    let bounds = Rectangle::new(iced::Point::new(500.0, 20.0), iced::Size::new(90.0, 32.0));
    let _ = filter(app, Event::OpenUnder(request, Some(bounds)));
}

#[test]
fn a_late_keyboard_reply_is_dropped_when_something_else_took_the_window() {
    let (_root, mut app) = loaded();
    // Another menu opened meanwhile.
    let request = press_filters(&mut app);
    let _ = app.update(Message::Sort(crate::sort_control::Event::ToggleMenu));
    reply(&mut app, request);
    assert!(!app.filter_menu);
    assert!(app.sort_menu);
    let _ = app.update(Message::Sort(crate::sort_control::Event::CloseMenu));

    let request = press_filters(&mut app);
    let _ = app.update(Message::Context("hades".into()));
    reply(&mut app, request);
    assert!(!app.filter_menu);
    assert_eq!(app.action_menu.as_deref(), Some("hades"));
    let _ = app.update(Message::CloseMenu);

    // A second press, a close, or a click cancels it.
    let request = press_filters(&mut app);
    let _ = filter(&mut app, Event::ToggleMenuFromKeyboard);
    assert!(app.filter_opening.pending.is_none());
    reply(&mut app, request);
    assert!(!app.filter_menu);
    let request = press_filters(&mut app);
    let _ = filter(&mut app, Event::CloseMenu);
    reply(&mut app, request);
    assert!(!app.filter_menu);
    let request = press_filters(&mut app);
    let _ = filter(&mut app, Event::ToggleMenu);
    let _ = filter(&mut app, Event::CloseMenu);
    reply(&mut app, request);
    assert!(!app.filter_menu);

    // Leaving the Library or closing the window.
    let request = press_filters(&mut app);
    app.section = crate::sidebar::Section::Trash;
    reply(&mut app, request);
    assert!(!app.filter_menu);
    app.section = crate::sidebar::Section::Library;
    let request = press_filters(&mut app);
    app.closing = true;
    reply(&mut app, request);
    assert!(!app.filter_menu);
    app.closing = false;

    // The current reply opens it, once.
    let request = press_filters(&mut app);
    reply(&mut app, request);
    assert!(app.filter_menu_shown());
    assert_eq!(app.filter_menu_position, iced::Point::new(496.0, 52.0));
    let _ = filter(&mut app, Event::CloseMenu);
    reply(&mut app, request);
    assert!(!app.filter_menu);
}

#[test]
fn opening_one_menu_drops_the_other_menus_pending_keyboard_open() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Sort(
        crate::sort_control::Event::ToggleMenuFromKeyboard,
    ));
    let sort_request = app.sort_opening.pending.unwrap();
    let _ = filter(&mut app, Event::ToggleMenu);
    let _ = filter(&mut app, Event::CloseMenu);
    let _ = app.update(Message::Sort(crate::sort_control::Event::OpenUnder(
        sort_request,
        None,
    )));
    assert!(!app.sort_menu);

    let request = press_filters(&mut app);
    let _ = app.update(Message::Sort(crate::sort_control::Event::ToggleMenu));
    let _ = app.update(Message::Sort(crate::sort_control::Event::CloseMenu));
    reply(&mut app, request);
    assert!(!app.filter_menu);
}

#[test]
fn overlays_block_the_filter_controls() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Add);
    assert!(app.flow.is_some());
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    let _ = filter(&mut app, Event::ToggleMenu);
    let _ = filter(&mut app, Event::Clear);
    assert!(app.filters.stores.is_empty());
    assert!(app.filters.platforms.is_empty());
    assert!(!app.filter_menu);
}

#[test]
fn a_filter_change_keeps_the_selection_and_scrolls_to_it() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("papers-please".into()));
    app.scroll_offset = 400.0;
    let generation = app.grid_motion.generation();
    let _ = filter(&mut app, Event::ToggleTag("game".into()));
    assert_eq!(app.selected.as_deref(), Some("papers-please"));
    assert!(app.panel_open);
    // It's shown, so the grid stays put until the card reports where it
    // landed.
    assert_eq!(app.scroll_offset, 400.0);
    let now = app.grid_motion.generation();
    assert_ne!(now, generation);
    let bounds = grid_motion::CardBounds {
        card: Rectangle::new(
            iced::Point::new(32.0, 1650.0),
            iced::Size::new(160.0, 300.0),
        ),
        viewport: Rectangle::new(iced::Point::new(0.0, 50.0), iced::Size::new(800.0, 600.0)),
        offset: iced::Vector::new(0.0, 400.0),
    };
    let _ = app.update(Message::GridCardLocated(
        "papers-please".into(),
        now,
        bounds,
    ));
    assert_eq!(app.scroll_offset, 1300.0);
}

#[test]
fn a_filter_or_show_hidden_off_that_removes_the_selection_keeps_it_with_its_panel() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("papers-please".into()));
    let _ = filter(&mut app, Event::ToggleStore("Steam"));
    assert!(!shown(&app).contains(&"papers-please"));
    assert_eq!(app.selected.as_deref(), Some("papers-please"));
    assert!(app.panel_open);
    let _ = filter(&mut app, Event::Clear);

    let _ = filter(&mut app, Event::ToggleShowHidden);
    let _ = app.update(Message::Select("old-demo".into()));
    assert!(app.panel_open);
    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert!(!shown(&app).contains(&"old-demo"));
    assert_eq!(app.selected.as_deref(), Some("old-demo"));
    assert!(app.panel_open);
}

#[test]
fn enter_does_nothing_unless_the_selected_card_is_shown() {
    type Hide = fn(&mut App);
    let hides: [(&str, Hide); 4] = [
        ("search", |app| {
            let _ = app.update(Message::SearchChanged("hades".into()));
        }),
        ("store", |app| {
            let _ = filter(app, Event::ToggleStore("Steam"));
        }),
        ("tag", |app| {
            let _ = filter(app, Event::ToggleTag("rpg".into()));
        }),
        ("show hidden", |app| {
            let _ = filter(app, Event::ToggleShowHidden);
        }),
    ];
    for (why, hide) in hides {
        let (_root, mut app) = loaded();
        let selected = if why == "show hidden" {
            let _ = filter(&mut app, Event::ToggleShowHidden);
            "old-demo"
        } else {
            "notepad"
        };
        let _ = app.update(Message::Select(selected.into()));
        hide(&mut app);
        assert!(!app.selected_shown(), "{why}");

        // With the panel open, Enter doesn't launch.
        assert!(app.panel_open, "{why}");
        key(&mut app, Named::Enter);
        assert!(app.operation.is_none(), "{why}");
        assert!(app.panel_error.is_none(), "{why}");

        // With it closed, Enter doesn't open it.
        let _ = app.update(Message::ClosePanel);
        key(&mut app, Named::Enter);
        assert!(!app.panel_open, "{why}");
        assert_eq!(app.selected.as_deref(), Some(selected), "{why}");

        // Shown again, Enter opens it as before.
        let _ = app.update(Message::SearchChanged(String::new()));
        let _ = filter(&mut app, Event::Clear);
        let _ = filter(&mut app, Event::ShowHidden);
        assert!(app.selected_shown(), "{why}");
        key(&mut app, Named::Enter);
        assert!(app.panel_open, "{why}");
    }
}

#[test]
fn arrows_on_an_empty_grid_change_nothing() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("hades".into()));
    let _ = app.update(Message::SearchChanged("zzz".into()));
    assert!(shown(&app).is_empty());
    for arrow in [
        Named::ArrowLeft,
        Named::ArrowRight,
        Named::ArrowUp,
        Named::ArrowDown,
    ] {
        key(&mut app, arrow);
        assert_eq!(app.selected.as_deref(), Some("hades"), "{arrow:?}");
        assert!(app.panel_open, "{arrow:?}");
    }
    // With cards shown again, a selection the grid doesn't show moves to
    // the first card, as before.
    let _ = app.update(Message::SearchChanged("pad".into()));
    key(&mut app, Named::ArrowRight);
    assert_eq!(app.selected.as_deref(), Some("notepad"));
}

#[test]
fn down_from_the_search_box_on_an_empty_grid_changes_nothing() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Select("hades".into()));
    let _ = app.update(Message::SearchChanged("zzz".into()));
    assert!(shown(&app).is_empty());
    let task = app.update(Message::KeyWithFocus(
        Key::Named(Named::ArrowDown),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        true,
    ));
    assert_eq!(app.selected.as_deref(), Some("hades"));
    assert!(app.panel_open);
    assert!(app.search_focused);
    // Nothing unfocuses the search box.
    assert!(run_operations(task).is_empty());

    // With cards shown, Down still moves to the first one.
    let _ = app.update(Message::SearchChanged("pad".into()));
    let _ = app.update(Message::KeyWithFocus(
        Key::Named(Named::ArrowDown),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        true,
    ));
    assert_eq!(app.selected.as_deref(), Some("notepad"));
}

/// The popup's scroll offset and the focused row's bounds, in a short window
/// after `keys` presses of `named`.
fn after_keys(app: &mut App, named: Named, keys: usize) -> (f32, Rectangle, Rectangle) {
    // Arrow keys don't change the app, so their tasks can be taken first and
    // run in order against one laid out window that keeps its focus.
    let open = filter(app, Event::ToggleMenuFromKeyboard);
    let mut opened = Vec::new();
    for message in answers(app, open) {
        opened.push(app.update(message));
    }
    let presses: Vec<Task<Message>> = (0..keys)
        .map(|_| {
            app.update(Message::Key(
                Key::Named(named),
                Modifiers::empty(),
                iced::event::Status::Ignored,
            ))
        })
        .collect();
    let app = &*app;
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    drive(&mut laid, Task::batch(opened));
    for task in presses {
        drive(&mut laid, task);
    }
    let offset = laid
        .offsets()
        .into_iter()
        .rev()
        .find(|offset| *offset > 0.0);
    let focused = laid.focused();
    assert_eq!(focused.len(), 1);
    let found = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = found.clone();
    let viewport = crate::layout_probe::probe(&mut app.view(), app.window_size)
        .scrollables
        .into_iter()
        .map(|(viewport, _)| viewport)
        .find(|viewport| viewport.width < ACTION_MENU_WIDTH)
        .expect("the popup scrolls");
    struct Focused(std::sync::Arc<std::sync::Mutex<Option<Rectangle>>>);
    impl iced::advanced::widget::Operation for Focused {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }
        fn focusable(
            &mut self,
            _id: Option<&iced::widget::Id>,
            bounds: Rectangle,
            state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            if state.is_focused() {
                *self.0.lock().unwrap() = Some(bounds);
            }
        }
    }
    laid.run(Box::new(Focused(sink)));
    let row = found.lock().unwrap().expect("a row has focus");
    (offset.unwrap_or(0.0), row, viewport)
}

#[test]
fn keyboard_focus_scrolls_the_popup_to_the_focused_row() {
    let (_root, mut app) = loaded();
    app.window_size = iced::Size::new(1200.0, 260.0);
    // Down to the last row, Clear filters, which needs a filter on.
    let _ = filter(&mut app, Event::ToggleTag("tool".into()));
    let rows = app.filter_menu_ids().len();
    let (offset, row, viewport) = after_keys(&mut app, Named::ArrowDown, rows - 1);
    assert!(offset > 0.0, "the popup scrolled");
    // Content bounds don't move with the scroll, so the row's place in the
    // viewport is its bounds less the offset.
    let top = row.y - offset;
    assert!(top >= viewport.y - 0.5, "{row:?} {viewport:?} {offset}");
    assert!(
        top + row.height <= viewport.y + viewport.height + 0.5,
        "{row:?} {viewport:?} {offset}"
    );

    // Wrapping around to the first row scrolls back to the very top, so the
    // header and the Store label show with it.
    let _ = filter(&mut app, Event::CloseMenu);
    let (offset, row, viewport) = after_keys(&mut app, Named::ArrowDown, rows);
    assert_eq!(offset, 0.0);
    assert!(row.y >= viewport.y, "{row:?} {viewport:?}");
}

#[test]
fn keyboard_focus_scrolls_the_sort_menu_in_a_short_window() {
    let (_root, mut app) = loaded();
    app.window_size = iced::Size::new(1200.0, 160.0);
    let _ = app.update(Message::Sort(crate::sort_control::Event::ToggleMenu));
    // Up from no focus goes to the last row, Favorites first.
    let task = app.update(Message::Key(
        Key::Named(Named::ArrowUp),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
    drive(&mut laid, task);
    assert_eq!(
        laid.focused(),
        [iced::widget::Id::new("library-sort-favorites-first")]
    );
    assert!(laid.offsets().iter().any(|offset| *offset > 0.0));
}
