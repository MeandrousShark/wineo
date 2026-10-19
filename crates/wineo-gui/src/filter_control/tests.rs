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

/// The widget operations `task` asks for.
fn run_operations(task: Task<Message>) -> Vec<Box<dyn iced::advanced::widget::Operation>> {
    use iced::futures::StreamExt;
    let Some(stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    iced::futures::executor::block_on(stream.collect::<Vec<_>>())
        .into_iter()
        .filter_map(|action| match action {
            iced_runtime::Action::Widget(operation) => Some(operation),
            _ => None,
        })
        .collect()
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
fn source_filters_keep_any_chosen_source() {
    let (_root, mut app) = loaded();
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
    assert_eq!(shown(&app), ["hades"]);
    let _ = filter(&mut app, Event::ToggleSource("Windows"));
    assert_eq!(shown(&app), ["hades", "notepad"]);
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
    assert_eq!(shown(&app), ["notepad"]);
}

#[test]
fn tag_filters_keep_cards_with_any_chosen_tag_within_the_sources() {
    let (_root, mut app) = loaded();
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    assert_eq!(shown(&app), ["hades"]);
    let _ = filter(&mut app, Event::ToggleTag("tool".into()));
    assert_eq!(shown(&app), ["hades", "notepad"]);
    // Sources and tags both have to match.
    let _ = filter(&mut app, Event::ToggleSource("Windows"));
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
    let _ = filter(&mut app, Event::ToggleSource("GOG"));
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
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
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
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
    let _ = filter(&mut app, Event::ToggleShowHidden);
    let _ = filter(&mut app, Event::Clear);
    assert_eq!(
        fs::read_to_string(app.paths.library_file()).unwrap(),
        original
    );
}

#[test]
fn the_button_counts_source_and_tag_filters() {
    let (_root, mut app) = loaded();
    assert_eq!(button_label(&app), "Filters");
    let _ = app.update(Message::SearchChanged("a".into()));
    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert_eq!(button_label(&app), "Filters");
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    let _ = filter(&mut app, Event::ToggleTag("tool".into()));
    assert_eq!(button_label(&app), "Filters (3)");
}

#[test]
fn clear_filters_clears_search_sources_and_tags_but_not_the_sort_or_show_hidden() {
    let (_root, mut app) = loaded();
    let _ = app.update(Message::Sort(crate::sort_control::Event::FlipDirection));
    let _ = app.update(Message::SearchChanged("zzz".into()));
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    let _ = filter(&mut app, Event::ToggleShowHidden);
    assert!(app.filters_clearable());
    let _ = filter(&mut app, Event::Clear);
    assert!(app.search.is_empty());
    assert!(app.filters.sources.is_empty());
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
fn the_popup_offers_the_library_sources_and_tags_in_order() {
    let (_root, app) = loaded();
    assert_eq!(
        app.offered(),
        (
            vec!["GOG", "Native", "Steam", "Windows"],
            ["game", "Indie", "rpg", "tool"].map(str::to_owned).to_vec()
        )
    );
}

#[test]
fn a_chosen_filter_the_library_lost_stays_on_and_offered() {
    let (_root, mut app) = loaded();
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
    let _ = filter(&mut app, Event::ToggleTag("tool".into()));
    let lost: Vec<Card> = library()
        .into_iter()
        .filter(|card| card.name != "hades" && card.name != "notepad")
        .collect();
    let _ = app.update(Message::Loaded(Ok(lost)));
    assert!(app.filters.sources.contains("Steam"));
    assert!(app.filters.tags.contains("tool"));
    let (sources, tags) = app.offered();
    assert_eq!(sources, ["GOG", "Native", "Steam", "Windows"]);
    assert_eq!(tags, ["game", "Indie", "rpg", "tool"]);
    assert!(shown(&app).is_empty());
    // It can still be turned off from the popup.
    let _ = filter(&mut app, Event::ToggleTag("tool".into()));
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
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
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
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
        Some("library-filter-source-gog")
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
    let mut laid = focus_after(&app, run_operations(Task::batch(focus)));
    assert_eq!(
        laid.focused(),
        [iced::widget::Id::new("library-filter-source-gog")]
    );
    let messages = laid.event(&press(Named::Enter));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Filter(Event::ToggleSource("GOG"))))
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
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
    let _ = filter(&mut app, Event::ToggleMenu);
    let _ = filter(&mut app, Event::Clear);
    assert!(app.filters.sources.is_empty());
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
    let _ = filter(&mut app, Event::ToggleSource("Steam"));
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
        ("source", |app| {
            let _ = filter(app, Event::ToggleSource("Steam"));
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
