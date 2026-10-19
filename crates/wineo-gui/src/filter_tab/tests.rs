use std::sync::{Arc, Mutex};

use iced::keyboard;
use iced::keyboard::key::Named;
use iced::mouse::{self, Cursor};
use iced::{Point, Size};

use super::*;
use crate::filter_control::tests::library;
use crate::layout_probe::{Laid, bounds_at, lay_out};
use crate::sort_control::tests::{answers, press};

fn loaded(width: f32) -> App {
    let mut app = crate::tests::app();
    app.window_size = Size::new(width, 800.0);
    let _ = app.update(Message::Loaded(Ok(library())));
    app
}

fn filter(app: &mut App, event: Event) -> Task<Message> {
    app.update(Message::Filter(event))
}

fn shown(app: &App) -> Vec<&str> {
    app.filtered()
        .iter()
        .map(|card| card.name.as_str())
        .collect()
}

/// Lays out the window and lets the Filters button report where it is,
/// as the first redraw does.
fn placed(app: &mut App) {
    let mut laid = lay_out(app.view(), app.window_size);
    let redraw = iced::Event::Window(iced::window::Event::RedrawRequested(
        std::time::Instant::now(),
    ));
    let messages = laid.event(&redraw);
    drop(laid);
    for message in messages {
        if matches!(message, Message::Filter(Event::ButtonBounds(_))) {
            let _ = app.update(message);
        }
    }
}

/// Runs `task` against `laid`, answering each widget operation as it
/// comes, and returns the messages it sent.
fn drive(laid: &mut Laid<'_, Message>, task: Task<Message>) -> Vec<Message> {
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

fn bounds(laid: &mut Laid<'_, Message>, id: impl Into<String>) -> Option<Rectangle> {
    let found = Arc::new(Mutex::new(None));
    let sink = found.clone();
    let id: &'static str = Box::leak(id.into().into_boxed_str());
    laid.run(Box::new(iced::advanced::widget::operation::map(
        crate::library_menu::bounds_of(id),
        move |bounds| *sink.lock().unwrap() = bounds,
    )));
    *found.lock().unwrap()
}

/// Every focusable control's id, in tree order.
fn focusables(laid: &mut Laid<'_, Message>) -> Vec<String> {
    struct All(Arc<Mutex<Vec<String>>>);
    impl Operation for All {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn Focusable) {
            if let Some(id) = id {
                self.0.lock().unwrap().push(format!("{id:?}"));
            }
        }
    }
    let found = Arc::new(Mutex::new(Vec::new()));
    laid.run(Box::new(All(found.clone())));
    found.lock().unwrap().clone()
}

/// The tab's own controls in the laid out window.
fn tab_controls(laid: &mut Laid<'_, Message>) -> Vec<String> {
    focusables(laid)
        .into_iter()
        .filter(|id| id.contains("library-filter-tab-"))
        .collect()
}

fn debug_ids(ids: &[String]) -> Vec<String> {
    ids.iter()
        .map(|id| format!("{:?}", Id::from(id.clone())))
        .collect()
}

fn click(laid: &mut Laid<'_, Message>, at: Point) -> Vec<Message> {
    let cursor = Cursor::Available(at);
    let mut messages = laid.event_at(
        &iced::Event::Mouse(mouse::Event::CursorMoved { position: at }),
        cursor,
    );
    messages.extend(laid.event_at(
        &iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        cursor,
    ));
    messages.extend(laid.event_at(
        &iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        cursor,
    ));
    messages
}

fn focus(laid: &mut Laid<'_, Message>, id: &str) {
    laid.run(Box::new(
        iced::advanced::widget::operation::focusable::focus(Id::from(id.to_owned())),
    ));
}

#[test]
fn only_store_platform_and_tag_filters_bring_the_tab() {
    let mut app = loaded(1200.0);
    placed(&mut app);
    assert!(app.filter_tab().is_none());
    let _ = filter(&mut app, Event::ToggleShowHidden);
    let _ = app.update(Message::SearchChanged("ha".into()));
    assert!(app.filter_tab().is_none());
    let mut laid = lay_out(app.view(), app.window_size);
    assert!(tab_controls(&mut laid).is_empty());
    drop(laid);

    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    assert!(app.filter_tab().is_some());
    let mut laid = lay_out(app.view(), app.window_size);
    assert_eq!(
        tab_controls(&mut laid),
        debug_ids(&["library-filter-tab-tag-rpg".into(), CLEAR_ID.into()])
    );
}

#[test]
fn the_tab_hangs_in_the_gap_above_the_first_poster() {
    let mut app = loaded(1200.0);
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    placed(&mut app);
    let button = app.filter_button.unwrap();
    let probe = crate::layout_probe::probe(&mut app.view(), app.window_size);
    let tab = bounds_at(&probe.node, &[1, 0]);
    let (_, grid) = probe
        .scrollables
        .iter()
        .copied()
        .max_by(|a, b| a.0.width.total_cmp(&b.0.width))
        .unwrap();
    let poster_top = grid.y + crate::grid_motion::PADDING;
    // The measured gap: 8 px of toolbar padding, the rule, and 16 px of
    // grid padding.
    assert_eq!(poster_top - (button.y + button.height), 25.0);
    assert_eq!(tab.x, button.x);
    assert_eq!(tab.y, button.y + button.height - 1.0);
    assert_eq!(tab.y + tab.height, poster_top);
    let mut laid = lay_out(app.view(), app.window_size);
    let clear = bounds(&mut laid, CLEAR_ID).unwrap();
    assert!(crate::layout_probe::within(clear, tab), "{clear:?} {tab:?}");
}

#[test]
fn chips_follow_the_popup_order() {
    let mut app = loaded(1600.0);
    for event in [
        Event::ToggleTag("rpg".into()),
        Event::ToggleStore("Steam"),
        Event::TogglePlatform("Windows"),
        Event::ToggleTag("Indie".into()),
        Event::ToggleStore("GOG"),
        Event::ToggleTag("game".into()),
    ] {
        let _ = filter(&mut app, event);
    }
    assert_eq!(
        app.filter_chips(),
        [
            Chip::Store("GOG"),
            Chip::Store("Steam"),
            Chip::Platform("Windows"),
            Chip::Tag("game".into()),
            Chip::Tag("Indie".into()),
            Chip::Tag("rpg".into()),
        ]
    );
    placed(&mut app);
    let tab = app.filter_tab().unwrap();
    assert_eq!(tab.hidden(), 0);
    let mut laid = lay_out(app.view(), app.window_size);
    assert_eq!(tab_controls(&mut laid), debug_ids(&tab.ids()));
}

#[test]
fn a_chip_x_removes_only_its_filter() {
    let mut app = loaded(1200.0);
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    let _ = filter(&mut app, Event::ToggleTag("game".into()));
    placed(&mut app);
    assert_eq!(shown(&app), ["papers-please"]);

    let mut laid = lay_out(app.view(), app.window_size);
    let x = bounds(&mut laid, Chip::Store("GOG").id()).unwrap();
    let messages = click(&mut laid, x.center());
    drop(laid);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Filter(Event::Remove(Chip::Store("GOG"))))),
        "{messages:?}"
    );
    for message in messages {
        let _ = app.update(message);
    }
    assert!(app.filters.stores.is_empty());
    assert_eq!(app.filters.tags.len(), 1);
    assert_eq!(crate::filter_control::button_label(&app), "Filters (1)");
    assert_eq!(shown(&app), ["balatro", "hades", "papers-please"]);
}

#[test]
fn the_big_x_clears_every_chip_and_keeps_the_search() {
    let mut app = loaded(1200.0);
    let _ = app.update(Message::SearchChanged("a".into()));
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    let _ = filter(&mut app, Event::TogglePlatform("Native"));
    let _ = filter(&mut app, Event::ToggleTag("game".into()));
    placed(&mut app);
    let mut laid = lay_out(app.view(), app.window_size);
    let clear = bounds(&mut laid, CLEAR_ID).unwrap();
    let messages = click(&mut laid, clear.center());
    drop(laid);
    for message in messages {
        let _ = app.update(message);
    }
    assert_eq!(app.filters.active(), 0);
    assert_eq!(app.search, "a");
    assert!(app.filter_tab().is_none());
    assert_eq!(crate::filter_control::button_label(&app), "Filters");
    assert_eq!(
        shown(&app),
        ["balatro", "hades", "notepad", "papers-please"]
    );
}

#[test]
fn fit_folds_what_doesnt_fit_into_more() {
    let more = |_| 20.0;
    // Each chip takes its width plus 4 px of spacing.
    assert_eq!(fit(&[50.0, 50.0, 50.0], more, 162.0), 3);
    assert_eq!(fit(&[50.0, 50.0, 50.0], more, 161.0), 2);
    // Two chips and +1 need 54 + 54 + 24.
    assert_eq!(fit(&[50.0, 50.0, 50.0], more, 132.0), 2);
    assert_eq!(fit(&[50.0, 50.0, 50.0], more, 131.0), 1);
    assert_eq!(fit(&[50.0, 50.0, 50.0], more, 30.0), 0);
    assert_eq!(fit(&[50.0, 50.0, 50.0], more, 0.0), 0);
    assert_eq!(fit(&[], more, 0.0), 0);
}

fn every_filter(app: &mut App) {
    for event in [
        Event::ToggleStore("GOG"),
        Event::ToggleStore("Steam"),
        Event::TogglePlatform("Native"),
        Event::TogglePlatform("Windows"),
        Event::ToggleTag("game".into()),
        Event::ToggleTag("Indie".into()),
        Event::ToggleTag("rpg".into()),
        Event::ToggleTag("tool".into()),
    ] {
        let _ = filter(app, event);
    }
}

/// The laid out tab's right edge, and what +N says if it shows.
fn tab_extent(app: &App) -> (f32, Option<usize>) {
    let probe = crate::layout_probe::probe(&mut app.view(), app.window_size);
    let tab = bounds_at(&probe.node, &[1, 0]);
    let hidden = app.filter_tab().unwrap().hidden();
    (tab.x + tab.width, (hidden > 0).then_some(hidden))
}

#[test]
fn chips_fold_into_more_before_the_window_edge() {
    let mut app = loaded(1600.0);
    every_filter(&mut app);
    placed(&mut app);
    let (right, more) = tab_extent(&app);
    assert_eq!(more, None);
    assert!(right <= 1600.0 - MARGIN);

    app.window_size = Size::new(1000.0, 800.0);
    placed(&mut app);
    let tab = app.filter_tab().unwrap();
    assert!(tab.visible > 0 && tab.hidden() > 0, "{tab:?}");
    let (right, more) = tab_extent(&app);
    assert_eq!(more, Some(tab.hidden()));
    assert!(right <= 1000.0 - MARGIN, "{right}");
    // One more chip in place of +N wouldn't have fit.
    let next = chip_width(tab.chips[tab.visible].label());
    let grown = right - more_width(tab.hidden())
        + next
        + if tab.hidden() > 1 {
            SPACING + more_width(tab.hidden() - 1)
        } else {
            0.0
        };
    assert!(grown > 1000.0 - MARGIN, "{grown}");
}

#[test]
fn chips_fold_before_the_docked_panel() {
    let mut app = loaded(1600.0);
    every_filter(&mut app);
    let _ = app.update(Message::Select("balatro".into()));
    app.panel_motion.snap(true);
    placed(&mut app);
    let tab = app.filter_tab().unwrap();
    assert!(tab.hidden() > 0, "{tab:?}");
    let (right, _) = tab_extent(&app);
    let panel = 1600.0 - crate::grid_motion::DOCK_WIDTH;
    assert!(right <= panel - MARGIN, "{right}");
}

#[test]
fn more_opens_the_filters_popup() {
    let mut app = loaded(1000.0);
    every_filter(&mut app);
    placed(&mut app);
    let tab = app.filter_tab().unwrap();
    let mut laid = lay_out(app.view(), app.window_size);
    let more = bounds(&mut laid, MORE_ID).unwrap();
    let messages = click(&mut laid, more.center());
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Filter(Event::ToggleMenu))),
        "{messages:?}"
    );
    drop(laid);
    for message in messages {
        let task = app.update(message);
        for answer in answers(&app, task) {
            let _ = app.update(answer);
        }
    }
    assert!(app.filter_menu_shown());
    assert_eq!(
        app.filter_menu_position,
        crate::library_menu::anchor_under(tab.button)
    );
    let _ = filter(&mut app, Event::CloseMenu);

    // From the keyboard it opens under the Filters button, like Filters.
    let mut laid = lay_out(app.view(), app.window_size);
    focus(&mut laid, MORE_ID);
    let messages = laid.event(&press(Named::Enter));
    drop(laid);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Filter(Event::ToggleMenu))),
        "{messages:?}"
    );
    for message in messages {
        let task = app.update(message);
        for answer in answers(&app, task) {
            let _ = app.update(answer);
        }
    }
    assert!(app.filter_menu_shown());
    assert_eq!(
        app.filter_menu_position,
        crate::library_menu::anchor_under(tab.button)
    );
}

/// Removes `chip` with Enter on its x and returns what has focus after.
fn remove_from_keyboard(app: &mut App, chip: Chip) -> Vec<Id> {
    let mut laid = lay_out(app.view(), app.window_size);
    focus(&mut laid, &chip.id());
    let messages = laid.event(&press(Named::Enter));
    drop(laid);
    assert!(
        matches!(
            messages.as_slice(),
            [Message::Filter(Event::RemoveFromKeyboard(removed))] if *removed == chip
        ),
        "{messages:?}"
    );
    let task = filter(app, Event::RemoveFromKeyboard(chip));
    let mut laid = lay_out(app.view(), app.window_size);
    drive(&mut laid, task);
    laid.focused()
}

#[test]
fn removing_a_chip_from_the_keyboard_moves_focus_to_a_neighbor() {
    let mut app = loaded(1600.0);
    for event in [
        Event::ToggleStore("GOG"),
        Event::ToggleStore("Steam"),
        Event::ToggleTag("game".into()),
        Event::ToggleTag("rpg".into()),
    ] {
        let _ = filter(&mut app, event);
    }
    placed(&mut app);
    // The first goes to the next.
    assert_eq!(
        remove_from_keyboard(&mut app, Chip::Store("GOG")),
        [Id::from(Chip::Store("Steam").id())]
    );
    // A middle one goes to the next.
    assert_eq!(
        remove_from_keyboard(&mut app, Chip::Tag("game".into())),
        [Id::from(Chip::Tag("rpg".into()).id())]
    );
    // The last goes to the previous.
    assert_eq!(
        remove_from_keyboard(&mut app, Chip::Tag("rpg".into())),
        [Id::from(Chip::Store("Steam").id())]
    );
    // The only one leaves the tab, so focus goes back to Filters.
    assert_eq!(
        remove_from_keyboard(&mut app, Chip::Store("Steam")),
        [Id::new(FILTER_BUTTON_ID)]
    );
    assert!(app.filter_tab().is_none());
}

#[test]
fn removing_a_chip_from_the_keyboard_falls_to_more_when_the_next_is_folded() {
    let mut app = loaded(1600.0);
    every_filter(&mut app);
    placed(&mut app);
    // Room for GOG and +7, but not for Steam and +6 once GOG is gone.
    let room = chip_width("GOG") + SPACING + more_width(7) + SPACING;
    assert!(chip_width("Steam") + SPACING + more_width(6) + SPACING > room);
    let button = app.filter_button.unwrap();
    app.window_size.width = button.x
        + 2.0 * TAB_BORDER
        + TAB_PADDING_LEFT
        + TAB_PADDING_RIGHT
        + CLEAR_SIZE
        + room
        + MARGIN;
    placed(&mut app);
    assert_eq!(app.filter_tab().unwrap().visible, 1);
    assert_eq!(
        remove_from_keyboard(&mut app, Chip::Store("GOG")),
        [Id::new(MORE_ID)]
    );
    assert_eq!(app.filter_tab().unwrap().visible, 0);
}

#[test]
fn clearing_from_the_keyboard_returns_focus_to_filters() {
    let mut app = loaded(1200.0);
    let _ = filter(&mut app, Event::ToggleTag("rpg".into()));
    placed(&mut app);
    let task = filter(&mut app, Event::ClearChipsFromKeyboard);
    let mut laid = lay_out(app.view(), app.window_size);
    drive(&mut laid, task);
    assert_eq!(laid.focused(), [Id::new(FILTER_BUTTON_ID)]);
}

#[test]
fn tab_moves_through_the_chips_right_after_filters() {
    let mut app = loaded(1000.0);
    every_filter(&mut app);
    placed(&mut app);
    let ids = app.filter_tab().unwrap().ids();
    assert!(ids.contains(&MORE_ID.to_owned()));
    let mut laid = lay_out(app.view(), app.window_size);
    focus(&mut laid, crate::sort_control::DIRECTION_BUTTON_ID);
    let mut forward = Vec::new();
    for _ in 0..ids.len() + 2 {
        drive(&mut laid, step(&app, false));
        forward.push(laid.focused());
    }
    let mut expected = vec![vec![Id::new(FILTER_BUTTON_ID)]];
    expected.extend(ids.iter().map(|id| vec![Id::from(id.clone())]));
    assert_eq!(forward[..=ids.len()], expected[..]);
    // Past the big x, focus leaves the tab for what follows Filters in
    // the page.
    let after = &forward[ids.len() + 1];
    assert!(
        after
            .iter()
            .all(|id| !ids.iter().any(|tab| Id::from(tab.clone()) == *id)
                && *id != Id::new(FILTER_BUTTON_ID)),
        "{after:?}"
    );
    // Shift+Tab walks back the same way.
    drive(&mut laid, step(&app, true));
    assert_eq!(laid.focused(), [Id::from(CLEAR_ID)]);
    focus(&mut laid, &ids[0]);
    drive(&mut laid, step(&app, true));
    assert_eq!(laid.focused(), [Id::new(FILTER_BUTTON_ID)]);
}

#[test]
fn tab_key_without_the_tab_keeps_the_usual_order() {
    let mut app = loaded(1200.0);
    placed(&mut app);
    // Nothing to splice in, so the step matches focus_next.
    let mut laid = lay_out(app.view(), app.window_size);
    focus(&mut laid, FILTER_BUTTON_ID);
    drive(&mut laid, step(&app, false));
    let ours = laid.focused();
    focus(&mut laid, FILTER_BUTTON_ID);
    drive(&mut laid, iced::widget::operation::focus_next());
    assert_eq!(ours, laid.focused());
}

#[test]
fn clicks_on_the_tab_never_reach_the_grid() {
    let mut app = loaded(1600.0);
    let _ = filter(&mut app, Event::ToggleTag("game".into()));
    let _ = app.update(Message::Select("balatro".into()));
    app.panel_motion.snap(true);
    placed(&mut app);
    let probe = crate::layout_probe::probe(&mut app.view(), app.window_size);
    let tab = bounds_at(&probe.node, &[1, 0]);
    let mut laid = lay_out(app.view(), app.window_size);
    // On the chip's label, below the toolbar, where the grid is.
    let label = Point::new(tab.x + 12.0, tab.y + tab.height - 6.0);
    let messages = click(&mut laid, label);
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, Message::ClosePanel)),
        "{messages:?}"
    );
    // Just past the tab the grid takes the click and closes the panel.
    let beside = Point::new(tab.x + tab.width + 8.0, label.y);
    let messages = click(&mut laid, beside);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::ClosePanel)),
        "{messages:?}"
    );
}

#[test]
fn an_open_menu_keeps_the_tab_drawn_but_inert() {
    let mut app = loaded(1200.0);
    let _ = filter(&mut app, Event::ToggleStore("GOG"));
    let _ = filter(&mut app, Event::ToggleTag("game".into()));
    placed(&mut app);
    let mut laid = lay_out(app.view(), app.window_size);
    let x = bounds(&mut laid, Chip::Store("GOG").id()).unwrap();
    drop(laid);

    let menus: [fn(&mut App); 3] = [
        crate::sort_control::tests::open_sort,
        crate::filter_control::tests::open_filters,
        |app| {
            let _ = app.update(Message::Context("hades".into()));
        },
    ];
    for open in menus {
        open(&mut app);
        assert!(app.filter_tab_blocked());
        assert!(app.filter_tab().is_some());
        let probe = crate::layout_probe::probe(&mut app.view(), app.window_size);
        assert!(bounds_at(&probe.node, &[1, 0]).width > 0.0);
        let mut laid = lay_out(app.view(), app.window_size);
        assert!(tab_controls(&mut laid).is_empty());
        let messages = click(&mut laid, x.center());
        drop(laid);
        assert!(
            !messages.iter().any(|message| matches!(
                message,
                Message::Filter(Event::Remove(_) | Event::ClearChips)
            )),
            "{messages:?}"
        );
        // A stale press does nothing either.
        let _ = filter(&mut app, Event::Remove(Chip::Store("GOG")));
        let _ = filter(&mut app, Event::ClearChips);
        assert_eq!(app.filters.active(), 2);
        app.close_library_menus();
    }
}

/// Every filter on and the panel open, in a window `width` wide.
fn with_panel(width: f32) -> App {
    let mut app = loaded(width);
    every_filter(&mut app);
    let _ = app.update(Message::Select("balatro".into()));
    app.panel_motion.snap(true);
    placed(&mut app);
    app
}

#[test]
fn no_tab_when_even_more_and_its_x_dont_fit_before_the_panel() {
    // At the default size the panel starts left of the Filters button's
    // right edge.
    let app = with_panel(1200.0);
    assert!(app.filters.active() > 0);
    assert!(app.filter_tab().is_none());
    let probe = crate::layout_probe::probe(&mut app.view(), app.window_size);
    assert_eq!(bounds_at(&probe.node, &[1]).width, 0.0);
    assert_eq!(crate::filter_control::button_label(&app), "Filters (8)");
}

#[test]
fn the_tab_shows_just_more_and_its_x_when_only_they_fit() {
    let app = with_panel(1600.0);
    let button = app.filter_button.unwrap();
    let chips = app.filter_chips().len();
    // The narrowest window whose cap leaves room for +8 and the x.
    let width = button.x
        + 2.0 * TAB_BORDER
        + TAB_PADDING_LEFT
        + TAB_PADDING_RIGHT
        + CLEAR_SIZE
        + more_width(chips)
        + SPACING
        + MARGIN
        + crate::grid_motion::DOCK_WIDTH;
    let mut app = app;
    app.window_size.width = width;
    let tab = app.filter_tab().expect("+N and the x fit");
    assert_eq!((tab.visible, tab.hidden()), (0, chips));
    let (right, more) = tab_extent(&app);
    assert_eq!(more, Some(chips));
    assert!(
        right <= width - crate::grid_motion::DOCK_WIDTH - MARGIN,
        "{right}"
    );
    app.window_size.width = width - 1.0;
    assert!(app.filter_tab().is_none());
}

#[test]
fn rows_under_the_toolbar_keep_the_tab_away() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let mut app = App::boot(paths).0;
    let _ = app.update(Message::Loaded(Ok(library())));
    let _ = filter(&mut app, Event::ToggleTag("game".into()));
    placed(&mut app);
    assert!(app.filter_tab().is_some());

    let file = app.paths.library_file();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "[environments.alpha\nfavorite = true\n").unwrap();
    app.problem_read(wineo_core::library::load(&app.paths).problem);
    assert!(app.library_banner().is_some());
    assert!(app.filter_tab().is_none());
    let mut laid = lay_out(app.view(), app.window_size);
    assert!(tab_controls(&mut laid).is_empty());
    drop(laid);
    app.problem_read(None);
    assert!(app.filter_tab().is_some());

    app.error = Some("broken".into());
    assert!(app.filter_tab().is_none());
}

#[test]
fn the_wheel_over_the_tab_scrolls_the_grid_like_the_grid_itself() {
    let mut app = crate::tests::app();
    let cards = (0..40)
        .map(|index| {
            let mut card = crate::tests::card(
                &format!("game-{index:02}"),
                wineo_core::runtime::State::Inactive,
                0,
            );
            card.tags = vec!["game".into()];
            card
        })
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    let _ = filter(&mut app, Event::ToggleTag("game".into()));
    placed(&mut app);
    let probe = crate::layout_probe::probe(&mut app.view(), app.window_size);
    let tab = bounds_at(&probe.node, &[1, 0]);
    let grid_offset =
        |laid: &mut Laid<'_, Message>| laid.offsets().into_iter().fold(0.0_f32, f32::max);
    let below = Point::new(tab.center_x(), tab.y + tab.height + 100.0);
    let lines = |x, y| mouse::ScrollDelta::Lines { x, y };
    let cases = [
        (lines(0.0, -2.0), false),
        // Shift turns a line turn sideways, which the grid can't scroll.
        (lines(0.0, -2.0), true),
        (lines(-2.0, 0.0), true),
        (mouse::ScrollDelta::Pixels { x: 0.0, y: -90.0 }, false),
        (mouse::ScrollDelta::Pixels { x: 0.0, y: -90.0 }, true),
    ];
    for (delta, shift) in cases {
        let modifiers = if shift {
            keyboard::Modifiers::SHIFT
        } else {
            keyboard::Modifiers::empty()
        };
        let held = iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers));
        let wheel = iced::Event::Mouse(mouse::Event::WheelScrolled { delta });

        // On the grid itself.
        let mut laid = lay_out(app.view(), app.window_size);
        let _ = laid.event_at(&held, Cursor::Available(below));
        let _ = laid.event_at(&wheel, Cursor::Available(below));
        let direct = grid_offset(&mut laid);
        if !shift || matches!(delta, mouse::ScrollDelta::Pixels { .. }) {
            assert!(direct > 0.0, "{delta:?} {shift}");
        }
        drop(laid);

        // On the tab, which hands it on and nothing else.
        let _ =
            app.update(crate::event_message(held.clone(), iced::event::Status::Ignored).unwrap());
        let mut laid = lay_out(app.view(), app.window_size);
        let messages = laid.event_at(&wheel, Cursor::Available(tab.center()));
        assert_eq!(grid_offset(&mut laid), 0.0);
        let [Message::Filter(Event::Wheel(delta))] = messages.as_slice() else {
            panic!("{messages:?}");
        };
        drive(&mut laid, super::wheel(*delta, app.modifiers.shift()));
        assert_eq!(grid_offset(&mut laid), direct, "{delta:?} {shift}");
    }
}

#[test]
fn a_mouse_removal_leaves_no_ring_on_the_tab() {
    let mut app = loaded(1600.0);
    for event in [
        Event::ToggleStore("GOG"),
        Event::TogglePlatform("Windows"),
        Event::ToggleTag("Indie".into()),
        Event::ToggleTag("rpg".into()),
    ] {
        let _ = filter(&mut app, event);
    }
    placed(&mut app);
    for start in [Some(FILTER_BUTTON_ID), None] {
        let mut laid = lay_out(app.view(), app.window_size);
        if let Some(id) = start {
            // Where Esc from the popup leaves focus.
            focus(&mut laid, id);
        }
        let x = bounds(&mut laid, Chip::Store("GOG").id()).unwrap();
        let messages = click(&mut laid, x.center());
        // At most one control has focus at any point, and never the tab's.
        let focused = laid.focused();
        assert!(focused.len() <= 1, "{focused:?}");
        let tab: Vec<Id> = app
            .filter_tab()
            .unwrap()
            .ids()
            .into_iter()
            .map(Id::from)
            .collect();
        assert!(!focused.iter().any(|id| tab.contains(id)), "{focused:?}");
        let tree = laid.into_tree();
        let tasks: Vec<_> = messages
            .into_iter()
            .map(|message| app.update(message))
            .collect();
        assert!(!app.filters.stores.contains("GOG"));
        let mut laid = crate::layout_probe::relay_out(app.view(), tree, app.window_size);
        drive(&mut laid, Task::batch(tasks));
        let focused = laid.focused();
        assert!(focused.len() <= 1, "{focused:?}");
        let tab: Vec<Id> = app
            .filter_tab()
            .unwrap()
            .ids()
            .into_iter()
            .map(Id::from)
            .collect();
        assert!(!focused.iter().any(|id| tab.contains(id)), "{focused:?}");
        drop(laid);
        let _ = filter(&mut app, Event::ToggleStore("GOG"));
    }
}
