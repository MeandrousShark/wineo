use std::time::{Duration, Instant};

use iced::advanced::widget::Tree;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::{Rectangle, Size};
use wineo_core::runtime::State;

use super::*;
use crate::layout_probe::{Laid, lay_out, relay_out};
use crate::sort_control::tests::press;

fn fixture(text: Option<&str>) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    if let Some(text) = text {
        std::fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
        std::fs::write(paths.library_file(), text).unwrap();
    }
    (root, App::boot(paths).0)
}

/// An app showing four games in a 1200 px window at `size` percent, over
/// `text` as its `library.toml` when there is one.
pub(crate) fn library_with(text: Option<&str>, size: u16) -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture(text);
    let cards = (0..4)
        .map(|index| crate::tests::card(&format!("game-{index:03}"), State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    at_size(&mut app, size);
    (root, app)
}

pub(crate) fn library(size: u16) -> (tempfile::TempDir, App) {
    library_with(None, size)
}

/// A 1200 px window at `size` percent, which leaves it fewer logical px.
pub(crate) fn at_size(app: &mut App, size: u16) {
    app.interface_size = size;
    let _ = app.window_resized(crate::app_window::launch_size(size));
}

pub(crate) fn redraw() -> iced::Event {
    iced::Event::Window(iced::window::Event::RedrawRequested(Instant::now()))
}

/// Lays the window out and lets the toolbar report its folds.
pub(crate) fn settle(app: &mut App) {
    let mut laid = lay_out(app.view(), app.window_size);
    let messages = laid.event(&redraw());
    drop(laid);
    for message in messages {
        let _ = app.update(message);
    }
}

pub(crate) fn bounds(laid: &mut Laid<'_, Message>, id: &'static str) -> Option<Rectangle> {
    let found = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = found.clone();
    laid.run(Box::new(iced::advanced::widget::operation::map(
        crate::library_menu::bounds_of(id),
        move |bounds| *sink.lock().unwrap() = bounds,
    )));
    *found.lock().unwrap()
}

pub(crate) fn shown(app: &App, id: &'static str) -> Option<Rectangle> {
    bounds(&mut lay_out(app.view(), app.window_size), id)
}

/// Runs `task` against `laid`, answering its widget operations, and
/// returns the messages it sent.
pub(crate) fn drive(laid: &mut Laid<'_, Message>, task: Task<Message>) -> Vec<Message> {
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
            tokio::time::timeout(Duration::from_millis(500), stream.next()).await
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

/// The window's widget state between updates, as the runtime keeps it.
pub(crate) struct Window(Option<Tree>);

impl Window {
    /// Hands `event` to the overlays with the cursor at `point`, or none,
    /// then what they publish to the app.
    pub(crate) fn overlay_event(
        &mut self,
        app: &mut App,
        event: iced::Event,
        point: Option<iced::Point>,
    ) {
        let cursor = point.map_or(
            iced::mouse::Cursor::Unavailable,
            iced::mouse::Cursor::Available,
        );
        let mut laid = self.lay_out(app);
        let messages = laid.overlay_event_at(&event, cursor);
        self.0 = Some(laid.into_tree());
        self.send(app, messages);
        self.frame(app);
    }

    pub(crate) fn overlays(&mut self, app: &App) -> Vec<Rectangle> {
        let mut laid = self.lay_out(app);
        let found = laid.overlay_bounds();
        self.0 = Some(laid.into_tree());
        found
    }

    pub(crate) fn new(app: &mut App) -> Self {
        let mut window = Self(None);
        window.frame(app);
        window
    }

    /// Lays the view out over the kept state and runs a redraw, handing
    /// what it publishes to the app.
    pub(crate) fn frame(&mut self, app: &mut App) {
        let mut laid = self.lay_out(app);
        let messages = laid.event(&redraw());
        self.0 = Some(laid.into_tree());
        self.send(app, messages);
    }

    pub(crate) fn lay_out<'a>(&mut self, app: &'a App) -> Laid<'a, Message> {
        match self.0.take() {
            Some(tree) => relay_out(app.view(), tree, app.window_size),
            None => lay_out(app.view(), app.window_size),
        }
    }

    /// Hands each message to the app and runs the tasks it returns, and
    /// whatever they send, against the window.
    pub(crate) fn send(&mut self, app: &mut App, messages: Vec<Message>) {
        let mut queue = std::collections::VecDeque::from(messages);
        while let Some(message) = queue.pop_front() {
            let task = app.update(message);
            let mut laid = self.lay_out(app);
            let sent = drive(&mut laid, task);
            self.0 = Some(laid.into_tree());
            queue.extend(sent);
        }
    }

    /// A key press as the window delivers it: to the widgets first, then
    /// to the app with whether one captured it.
    pub(crate) fn key(&mut self, app: &mut App, named: Named, modifiers: Modifiers) {
        let mut laid = self.lay_out(app);
        let mut event = press(named);
        if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            modifiers: held, ..
        }) = &mut event
        {
            *held = modifiers;
        }
        let (messages, status) = laid.event_with_status(&event, iced::mouse::Cursor::Unavailable);
        self.0 = Some(laid.into_tree());
        let mut messages = messages;
        messages.push(Message::Key(Key::Named(named), modifiers, status));
        self.send(app, messages);
        self.frame(app);
    }

    pub(crate) fn press(&mut self, app: &mut App, named: Named) {
        self.key(app, named, Modifiers::empty());
    }

    pub(crate) fn focused(&mut self, app: &App) -> Vec<iced::widget::Id> {
        let mut laid = self.lay_out(app);
        let focused = laid.focused();
        self.0 = Some(laid.into_tree());
        focused
    }

    pub(crate) fn bounds(&mut self, app: &App, id: &'static str) -> Option<Rectangle> {
        let mut laid = self.lay_out(app);
        let found = bounds(&mut laid, id);
        self.0 = Some(laid.into_tree());
        found
    }

    /// A left click at `point`, pressed and released.
    pub(crate) fn click(&mut self, app: &mut App, point: iced::Point) {
        use iced::mouse::{Button, Cursor, Event};
        for event in [
            Event::ButtonPressed(Button::Left),
            Event::ButtonReleased(Button::Left),
        ] {
            let mut laid = self.lay_out(app);
            let messages = laid.event_at(&iced::Event::Mouse(event), Cursor::Available(point));
            self.0 = Some(laid.into_tree());
            self.send(app, messages);
        }
        self.frame(app);
    }

    pub(crate) fn focus(&mut self, app: &App, id: &'static str) {
        let mut laid = self.lay_out(app);
        laid.run(Box::new(
            iced::advanced::widget::operation::focusable::focus(iced::widget::Id::new(id)),
        ));
        self.0 = Some(laid.into_tree());
    }
}

pub(crate) fn ids(id: &'static str) -> Vec<iced::widget::Id> {
    vec![iced::widget::Id::new(id)]
}

/// Opens the menu the way Enter on More does.
fn open(app: &mut App, window: &mut Window) {
    window.send(app, vec![Message::More(Event::ToggleMenu)]);
    window.frame(app);
    assert!(app.more_menu_shown(), "the menu opened");
}

fn all() -> Folds {
    Control::ALL
        .into_iter()
        .fold(Folds::default(), |folds, control| folds.with(control))
}

fn folds(controls: &[Control]) -> Folds {
    controls
        .iter()
        .fold(Folds::default(), |folds, control| folds.with(*control))
}

/// Where the toolbar's controls must end: the window's edge less its
/// padding.
pub(crate) fn right_edge(app: &App) -> f32 {
    app.window_size.width - 16.0
}

pub(crate) fn right(bounds: Rectangle) -> f32 {
    bounds.x + bounds.width
}

#[test]
fn a_1200_px_window_folds_more_as_the_interface_grows() {
    // At 100% everything shows and search keeps its full width.
    let (_root, mut app) = library(100);
    settle(&mut app);
    assert!(app.toolbar_folds.is_empty());
    assert_eq!(shown(&app, MORE_ID), None);
    assert_eq!(
        shown(&app, crate::SEARCH_ID).unwrap().width,
        crate::grid::SEARCH_WIDTH
    );
    let dice = shown(&app, crate::random_pick::BUTTON_ID).unwrap();
    let filters = shown(&app, crate::filter_control::FILTER_BUTTON_ID).unwrap();

    // 150% leaves 800 logical px: the controls fold from the right until
    // the row fits, the toggle and zoom first.
    let (_root, mut app) = library(150);
    settle(&mut app);
    assert_eq!(
        app.toolbar_folds,
        folds(&[
            Control::Titles,
            Control::Zoom,
            Control::Layout,
            Control::Dice
        ])
    );
    let more = shown(&app, MORE_ID).expect("More shows");
    let shown_filters = shown(&app, crate::filter_control::FILTER_BUTTON_ID).unwrap();
    assert_eq!(more.size(), Size::new(32.0, 32.0));
    assert_eq!(more.x, right(shown_filters) + 8.0, "More follows Filters");
    assert!(right(more) <= right_edge(&app));
    // One fold fewer wouldn't fit: the dice back in front of More.
    assert!(right(more) + 8.0 + dice.width > right_edge(&app));
    assert_eq!(
        shown(&app, crate::SEARCH_ID).unwrap().width,
        crate::grid::SEARCH_MIN_WIDTH
    );
    for id in [
        crate::random_pick::BUTTON_ID,
        crate::layout_switch::ID,
        zoom_control::SLIDER_ID,
        zoom_control::POSTER_ONLY_ID,
    ] {
        assert_eq!(shown(&app, id), None, "{id} folded");
    }

    // 200% leaves 600: everything after Sort and its direction folds.
    let (_root, mut app) = library(200);
    settle(&mut app);
    assert_eq!(app.toolbar_folds, all());
    let direction = shown(&app, crate::sort_control::DIRECTION_BUTTON_ID).unwrap();
    let more = shown(&app, MORE_ID).unwrap();
    assert_eq!(more.x, right(direction) + 8.0);
    assert_eq!(shown(&app, crate::filter_control::FILTER_BUTTON_ID), None);
    // One fold fewer wouldn't fit either: Filters back in front of More.
    assert!(right(direction) + 8.0 + filters.width + 8.0 + more.width > right_edge(&app));
    // Even then the 160 px field doesn't fit, so search, last, is its icon,
    // and the row fits.
    assert!(app.search_icon);
    assert_eq!(shown(&app, crate::SEARCH_ID), None);
    let icon = shown(&app, crate::search_drop::ICON_ID).expect("the search icon");
    assert_eq!(icon.size(), Size::new(32.0, 32.0));
    assert!(right(more) <= right_edge(&app));
    assert!(
        icon.x + crate::grid::SEARCH_MIN_WIDTH + (right(more) - right(icon)) > right_edge(&app)
    );
}

#[test]
fn search_shrinks_before_anything_folds() {
    let (_root, mut app) = library(100);
    // Just narrow enough that search gives up some width, not all of it.
    let _ = app.window_resized(Size::new(1100.0, 800.0));
    settle(&mut app);
    assert!(app.toolbar_folds.is_empty());
    let width = shown(&app, crate::SEARCH_ID).unwrap().width;
    assert!(width < crate::grid::SEARCH_WIDTH && width > crate::grid::SEARCH_MIN_WIDTH);
}

#[test]
fn the_list_folds_only_what_it_shows() {
    let (_root, mut app) = library(150);
    let _ = app.set_layout(crate::grid_motion::ViewLayout::List);
    settle(&mut app);
    // No zoom or toggle to fold, so the layout switch and dice go first.
    assert_eq!(app.toolbar_folds, folds(&[Control::Layout, Control::Dice]));
    let more = shown(&app, MORE_ID).unwrap();
    let filters = shown(&app, crate::filter_control::FILTER_BUTTON_ID).unwrap();
    assert_eq!(more.x, right(filters) + 8.0);

    let (_root, mut app) = library(200);
    let _ = app.set_layout(crate::grid_motion::ViewLayout::List);
    settle(&mut app);
    assert_eq!(
        app.toolbar_folds,
        folds(&[Control::Filters, Control::Dice, Control::Layout])
    );
    assert_eq!(app.more_ids(), [FILTERS_ID, RANDOM_ID, LAYOUT_ID]);
}

const LONG: &str = "A really long preset name for the couch";

#[test]
fn a_long_preset_name_folds_sooner() {
    let text = format!("schema_version = 1\n\n[presets.\"{LONG}\"]\n");
    let (_root, mut app) = library_with(Some(&text), 100);
    // Room for every control with search squeezed, but not much more.
    let _ = app.window_resized(Size::new(1015.0, 800.0));
    let mut window = Window::new(&mut app);
    assert!(app.toolbar_folds.is_empty());
    let plain = shown(&app, crate::filter_control::FILTER_BUTTON_ID).unwrap();

    window.send(
        &mut app,
        vec![Message::Filter(crate::filter_control::Event::ToggleMenu)],
    );
    assert!(app.filter_menu_shown());
    window.send(
        &mut app,
        vec![Message::Filter(crate::filter_control::Event::Preset(
            crate::presets::Event::Apply(LONG.to_owned()),
        ))],
    );
    let label = crate::filter_control::button_label(&app);
    assert!(label.starts_with("Filters (A really"), "{label}");
    settle(&mut app);
    assert!(!app.toolbar_folds.is_empty(), "the wider label folds");
    let named = shown(&app, crate::filter_control::FILTER_BUTTON_ID).unwrap();
    assert!(named.width > plain.width);
    assert!(right(shown(&app, MORE_ID).unwrap()) <= right_edge(&app));
}

#[test]
fn the_menu_holds_the_folded_rows_in_toolbar_order() {
    let (_root, mut app) = library(200);
    settle(&mut app);
    assert_eq!(
        app.more_ids(),
        [FILTERS_ID, RANDOM_ID, LAYOUT_ID, ZOOM_ID, TITLES_ID]
    );
    let (_root, mut app) = library(150);
    settle(&mut app);
    assert_eq!(app.more_ids(), [RANDOM_ID, LAYOUT_ID, ZOOM_ID, TITLES_ID]);
    // A dice with nothing to pick from is off in the menu too.
    let _ = app.update(Message::SearchChanged("nothing matches this".into()));
    assert_eq!(app.more_ids(), [LAYOUT_ID, ZOOM_ID, TITLES_ID]);
}

#[test]
fn the_menu_opens_under_more_right_aligned() {
    let (_root, mut app) = library(150);
    let mut window = Window::new(&mut app);
    let more = window.bounds(&app, MORE_ID).unwrap();
    open(&mut app, &mut window);
    let position = crate::grid::menu_position(app.more_menu_position, app.window_size, 0.0);
    assert_eq!(position.x + WIDTH, right(more));
    assert_eq!(position.y, more.y + more.height + 4.0);
    assert!(app.more_menu);
}

#[test]
fn the_menu_takes_the_sort_menus_keys() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    open(&mut app, &mut window);
    // It opens on the first row.
    assert_eq!(window.focused(&app), ids(FILTERS_ID));
    window.press(&mut app, Named::ArrowDown);
    assert_eq!(window.focused(&app), ids(RANDOM_ID));
    window.press(&mut app, Named::Tab);
    assert_eq!(window.focused(&app), ids(LAYOUT_ID));
    // Left and Right flip the focused segment, which keeps its focus.
    window.press(&mut app, Named::ArrowRight);
    assert_eq!(app.layout, crate::grid_motion::ViewLayout::List);
    assert_eq!(window.focused(&app), ids(LAYOUT_ID));
    window.press(&mut app, Named::ArrowLeft);
    assert_eq!(app.layout, crate::grid_motion::ViewLayout::Grid);
    assert!(app.more_menu, "the segment leaves the menu open");
    window.press(&mut app, Named::ArrowDown);
    assert_eq!(window.focused(&app), ids(ZOOM_ID));
    // Right and Left step the focused slider.
    window.press(&mut app, Named::ArrowRight);
    assert_eq!(app.zoom, 110);
    window.press(&mut app, Named::ArrowLeft);
    assert_eq!(app.zoom, 100);
    window.press(&mut app, Named::ArrowDown);
    assert_eq!(window.focused(&app), ids(TITLES_ID));
    // Space acts and the check row leaves the menu open.
    window.press(&mut app, Named::Space);
    assert!(app.poster_only);
    assert!(app.more_menu);
    window.press(&mut app, Named::Enter);
    assert!(!app.poster_only);
    // Down from the last row goes back to the first, Up the other way.
    window.press(&mut app, Named::ArrowDown);
    assert_eq!(window.focused(&app), ids(FILTERS_ID));
    window.press(&mut app, Named::ArrowUp);
    assert_eq!(window.focused(&app), ids(TITLES_ID));
    window.key(&mut app, Named::Tab, Modifiers::SHIFT);
    assert_eq!(window.focused(&app), ids(ZOOM_ID));
    // Esc closes it and gives More its focus back.
    window.press(&mut app, Named::Escape);
    assert!(!app.more_menu);
    assert_eq!(window.focused(&app), ids(MORE_ID));
}

#[test]
fn the_arrows_in_the_menu_never_reach_the_library() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    open(&mut app, &mut window);
    window.press(&mut app, Named::ArrowRight);
    window.press(&mut app, Named::ArrowLeft);
    assert_eq!(app.selected, None);
    assert_eq!(app.layout, crate::grid_motion::ViewLayout::Grid);
}

#[test]
fn the_filters_row_opens_the_popup_under_more() {
    let (_root, mut app) = library(150);
    // Filters folds too once its label grows past what's left.
    app.filters.stores.insert(wineo_core::source::Store::Steam);
    app.filters
        .platforms
        .insert(wineo_core::source::Platform::Windows);
    app.reorder();
    let _ = app.window_resized(Size::new(720.0, 600.0));
    let mut window = Window::new(&mut app);
    let more = window.bounds(&app, MORE_ID).unwrap();
    open(&mut app, &mut window);
    window.press(&mut app, Named::Enter);
    assert!(!app.more_menu);
    assert!(app.filter_menu_shown(), "the popup opened");
    let position = crate::grid::menu_position(app.filter_menu_position, app.window_size, 200.0);
    assert_eq!(position.x + crate::grid::ACTION_MENU_WIDTH, right(more));
    // It works like the button's: its first row has focus, and Esc goes
    // back to More, which holds Filters.
    assert!(!window.focused(&app).is_empty());
    window.press(&mut app, Named::Escape);
    assert!(!app.filter_menu);
    assert_eq!(window.focused(&app), ids(MORE_ID));
}

#[test]
fn the_filters_row_reads_like_the_button() {
    let (_root, mut app) = library(200);
    app.filters.stores.insert(wineo_core::source::Store::Steam);
    app.reorder();
    settle(&mut app);
    assert_eq!(crate::filter_control::button_label(&app), "Filters (1)");
    // The filter tab hangs from the button, so it waits for it.
    assert!(app.filter_tab().is_none());
    let mut window = Window::new(&mut app);
    open(&mut app, &mut window);
    assert_eq!(window.focused(&app), ids(FILTERS_ID));
    let row = window.bounds(&app, FILTERS_ID).expect("the Filters row");
    assert_eq!(row.height, ui::BUTTON_HEIGHT);
}

#[test]
fn the_pick_row_picks_and_closes() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    open(&mut app, &mut window);
    window.press(&mut app, Named::ArrowDown);
    window.press(&mut app, Named::Enter);
    assert!(!app.more_menu);
    assert!(app.selected.is_some(), "a game was picked");
    assert!(app.panel_open);
    assert_eq!(window.focused(&app), ids(MORE_ID));
}

#[test]
fn the_pick_rows_menu_key_opens_the_dice_options_under_more() {
    let (_root, mut app) = library(150);
    let mut window = Window::new(&mut app);
    let more = window.bounds(&app, MORE_ID).unwrap();
    open(&mut app, &mut window);
    assert_eq!(window.focused(&app), ids(RANDOM_ID));
    window.key(&mut app, Named::F10, Modifiers::SHIFT);
    assert!(!app.more_menu);
    assert!(app.random_menu_shown());
    let position =
        crate::grid::menu_position_of(app.random_menu_position, app.window_size, 272.0, 0.0);
    assert_eq!(position.x + 272.0, right(more));
    window.press(&mut app, Named::Escape);
    assert!(!app.random_menu);
    assert_eq!(window.focused(&app), ids(MORE_ID));
    // The Menu key does the same.
    open(&mut app, &mut window);
    window.press(&mut app, Named::ContextMenu);
    assert!(app.random_menu_shown());
}

#[test]
fn the_zoom_row_snaps_like_the_toolbar_slider() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    open(&mut app, &mut window);
    let slider = window.bounds(&app, ZOOM_ID).expect("the menu's slider");
    assert_eq!(slider.width, zoom_control::WIDTH);
    // A press 6 points past 100% lands in the snap zone.
    let x = zoom_control::x_of(slider, 106.0);
    window.click(&mut app, iced::Point::new(x, slider.center_y()));
    assert_eq!(app.zoom, 100);
    let x = zoom_control::x_of(slider, 150.0);
    window.click(&mut app, iced::Point::new(x, slider.center_y()));
    assert_eq!(app.zoom, 150);
    assert!(app.more_menu);
}

#[test]
fn the_rows_send_what_their_toolbar_controls_send() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    open(&mut app, &mut window);
    // Clicking the list side of the menu's segment.
    let segment = window.bounds(&app, LAYOUT_ID).unwrap();
    window.click(
        &mut app,
        iced::Point::new(segment.x + 8.0, segment.center_y()),
    );
    assert_eq!(app.layout, crate::grid_motion::ViewLayout::List);
    // Saved like the toolbar's.
    let saved = std::fs::read_to_string(app.paths.library_file()).unwrap();
    assert!(saved.contains("layout = \"list\""), "{saved}");
    // The list has no zoom, so its rows leave the menu.
    assert_eq!(app.more_ids(), [FILTERS_ID, RANDOM_ID, LAYOUT_ID]);
}

#[test]
fn only_one_toolbar_menu_is_open_at_a_time() {
    let (_root, mut app) = library(150);
    let mut window = Window::new(&mut app);
    open(&mut app, &mut window);
    // Another menu's open request finds More open and stands down.
    window.send(
        &mut app,
        vec![Message::Sort(crate::sort_control::Event::ToggleMenu)],
    );
    assert!(!app.sort_menu);
    assert!(app.more_menu);
    window.press(&mut app, Named::Escape);
    // With Sort open, More's request stands down the same way.
    window.send(
        &mut app,
        vec![Message::Sort(crate::sort_control::Event::ToggleMenu)],
    );
    assert!(app.sort_menu);
    window.send(&mut app, vec![Message::More(Event::ToggleMenu)]);
    assert!(!app.more_menu);
    assert!(app.sort_menu);
    // Opening it closes the rest.
    app.sort_menu = false;
    app.random_menu = false;
    open(&mut app, &mut window);
    window.send(&mut app, vec![Message::More(Event::OpenFilters)]);
    assert!(app.filter_menu);
    assert!(!app.more_menu);
}

#[test]
fn more_is_one_tab_stop_where_the_folded_controls_were() {
    let (_root, mut app) = library(150);
    settle(&mut app);
    let mut laid = lay_out(app.view(), app.window_size);
    let mut order = Vec::new();
    for _ in 0..12 {
        laid.run(Box::new(
            iced::advanced::widget::operation::focusable::focus_next(),
        ));
        order.push(laid.focused());
    }
    let at = |id: &'static str| order.iter().position(|focused| *focused == ids(id));
    let filters = at(crate::filter_control::FILTER_BUTTON_ID).unwrap();
    assert_eq!(at(MORE_ID), Some(filters + 1), "{order:?}");
    for id in [
        crate::random_pick::BUTTON_ID,
        crate::layout_switch::ID,
        zoom_control::SLIDER_ID,
        zoom_control::POSTER_ONLY_ID,
    ] {
        assert_eq!(at(id), None, "{id} isn't a Tab stop while folded");
    }
}

#[test]
fn focus_survives_a_fold_change() {
    let (_root, mut app) = library(100);
    let mut window = Window::new(&mut app);
    window.focus(&app, crate::SEARCH_ID);
    // Typing in search and growing the interface keeps the field focused.
    window.send(&mut app, vec![Message::SearchChanged("game".into())]);
    at_size(&mut app, 200);
    window.frame(&mut app);
    assert_eq!(app.toolbar_folds, all());
    assert_eq!(window.focused(&app), ids(crate::SEARCH_ID));
    at_size(&mut app, 100);
    window.frame(&mut app);
    assert!(app.toolbar_folds.is_empty());
    assert_eq!(window.focused(&app), ids(crate::SEARCH_ID));
    assert_eq!(app.search, "game");

    // A focused control that folds hands its focus to More.
    window.focus(&app, zoom_control::SLIDER_ID);
    at_size(&mut app, 150);
    window.frame(&mut app);
    assert_eq!(window.focused(&app), ids(MORE_ID));
    // And a control that stays keeps it.
    window.focus(&app, crate::sort_control::SORT_BUTTON_ID);
    at_size(&mut app, 200);
    window.frame(&mut app);
    assert_eq!(
        window.focused(&app),
        ids(crate::sort_control::SORT_BUTTON_ID)
    );
}

#[test]
fn an_open_menu_keeps_its_rows_through_a_fold_change() {
    let (_root, mut app) = library(150);
    let mut window = Window::new(&mut app);
    open(&mut app, &mut window);
    assert_eq!(app.more_ids(), [RANDOM_ID, LAYOUT_ID, ZOOM_ID, TITLES_ID]);
    // Narrower: what folded joins the menu.
    at_size(&mut app, 200);
    window.frame(&mut app);
    assert!(app.more_menu);
    assert_eq!(
        app.more_ids(),
        [FILTERS_ID, RANDOM_ID, LAYOUT_ID, ZOOM_ID, TITLES_ID]
    );
    // Wider: the rows stay while it's open.
    at_size(&mut app, 150);
    window.frame(&mut app);
    assert!(app.more_menu);
    assert_eq!(app.more_ids().len(), 5);
    // Nothing folded leaves no More, so the menu closes.
    at_size(&mut app, 100);
    window.frame(&mut app);
    assert!(!app.more_menu);
}

#[test]
fn more_shows_the_selected_look_while_its_menu_is_open() {
    let (_root, mut app) = library(150);
    settle(&mut app);
    let more = shown(&app, MORE_ID).unwrap();
    // A point inside the button's left border.
    let pixel = |app: &App| {
        let size = app.window_size;
        let pixels = crate::layout_probe::render(&mut app.view(), size);
        let (x, y) = ((more.x + 0.5) as usize, more.center_y() as usize);
        let at = (y * size.width as usize + x) * 4;
        pixels[at..at + 3].to_vec()
    };
    let closed = pixel(&app);
    app.more_menu = true;
    app.more_rows = app.toolbar_folds;
    let open = pixel(&app);
    assert_ne!(closed, open);
    let primary = app.palette.primary.into_rgba8();
    let near = |channel: usize| (i32::from(open[channel]) - i32::from(primary[channel])).abs() < 40;
    assert!((0..3).all(near), "{open:?} reads as {primary:?}");
}
