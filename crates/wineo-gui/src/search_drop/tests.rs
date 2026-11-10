use iced::Point;
use iced::keyboard::{Key, Modifiers, key::Named};

use super::*;
use crate::more_menu::tests::{Window, ids, library, right, settle, shown};

fn names(app: &App) -> Vec<String> {
    app.filtered()
        .iter()
        .map(|card| card.name.clone())
        .collect()
}

/// A typed character, as the window delivers it to the focused field.
fn typed(text: &str) -> iced::Event {
    let key = Key::Character(text.into());
    iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: iced::keyboard::key::Physical::Unidentified(
            iced::keyboard::key::NativeCode::Unidentified,
        ),
        location: iced::keyboard::Location::Standard,
        modifiers: Modifiers::empty(),
        text: Some(text.into()),
        repeat: false,
    })
}

/// A shortcut the library takes, as the app hears it with search
/// unfocused.
fn shortcut(app: &mut App, window: &mut Window, key: &str, modifiers: Modifiers) {
    window.send(
        app,
        vec![Message::KeyWithFocus(
            Key::Character(key.into()),
            modifiers,
            iced::event::Status::Ignored,
            false,
        )],
    );
    window.frame(app);
}

/// A left press; where it lands is the cursor's.
fn left_press() -> iced::Event {
    iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left))
}

fn center(bounds: iced::Rectangle) -> Point {
    Point::new(bounds.center_x(), bounds.center_y())
}

#[test]
fn search_is_an_icon_only_at_200_percent() {
    for size in [100, 150] {
        let (_root, mut app) = library(size);
        settle(&mut app);
        assert!(!app.search_icon, "{size}%");
        assert!(shown(&app, SEARCH_ID).is_some());
        assert_eq!(shown(&app, ICON_ID), None);
    }
    let (_root, mut app) = library(200);
    settle(&mut app);
    assert!(app.search_icon);
    assert_eq!(shown(&app, SEARCH_ID), None, "closed, the field is away");
    assert!(shown(&app, ICON_ID).is_some());
}

#[test]
fn a_click_opens_the_field_under_the_icon_and_typing_filters_live() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    let icon = window.bounds(&app, ICON_ID).unwrap();
    window.click(&mut app, center(icon));
    assert!(app.search_dropdown_shown());
    assert_eq!(window.focused(&app), ids(SEARCH_ID));
    // In the menu chrome under the icon, left edges aligned, and nothing
    // on the toolbar moved.
    let dropdown = *window
        .overlays(&app)
        .iter()
        .find(|bounds| bounds.width == crate::grid::ACTION_MENU_WIDTH)
        .expect("the drop-down");
    assert_eq!(dropdown.x, icon.x);
    assert_eq!(dropdown.y, icon.y + icon.height + 4.0);
    assert_eq!(dropdown.width, crate::grid::ACTION_MENU_WIDTH);
    assert_eq!(window.bounds(&app, ICON_ID), Some(icon));
    let field = window.bounds(&app, SEARCH_ID).unwrap();
    assert!(field.x >= dropdown.x && right(field) <= right(dropdown));

    for text in ["0", "0", "1"] {
        window.overlay_event(&mut app, typed(text), None);
    }
    assert_eq!(app.search, "001");
    assert_eq!(names(&app), ["game-001"]);
}

#[test]
fn slash_and_ctrl_f_open_it_too() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    shortcut(&mut app, &mut window, "/", Modifiers::empty());
    assert!(app.search_dropdown_shown());
    assert_eq!(window.focused(&app), ids(SEARCH_ID));
    window.press(&mut app, Named::Escape);
    assert!(!app.search_open);
    shortcut(&mut app, &mut window, "f", Modifiers::CTRL);
    assert!(app.search_dropdown_shown());
    assert_eq!(window.focused(&app), ids(SEARCH_ID));
}

#[test]
fn esc_and_a_click_outside_close_it_and_focus_the_icon() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    let icon = window.bounds(&app, ICON_ID).unwrap();
    window.click(&mut app, center(icon));
    window.overlay_event(&mut app, typed("g"), None);
    window.press(&mut app, Named::Escape);
    assert!(!app.search_open);
    assert_eq!(window.focused(&app), ids(ICON_ID));
    assert_eq!(app.search, "g", "closing keeps the text");

    window.click(&mut app, center(icon));
    assert!(app.search_open);
    let away = Point::new(app.window_size.width - 40.0, app.window_size.height - 40.0);
    window.overlay_event(&mut app, left_press(), Some(away));
    assert!(!app.search_open);
    assert_eq!(window.focused(&app), ids(ICON_ID));
    assert_eq!(app.selected, None, "the press only closed it");

    // A press on the icon while it's open closes it too, and stays closed.
    window.click(&mut app, center(icon));
    assert!(app.search_open);
    window.overlay_event(&mut app, left_press(), Some(center(icon)));
    assert!(!app.search_open);
}

#[test]
fn with_text_the_closed_icon_looks_selected_and_its_tooltip_shows_the_text() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    assert_eq!(tooltip_label(&app), "Search");
    let icon = window.bounds(&app, ICON_ID).unwrap();
    let pixel = |app: &App| {
        let size = app.window_size;
        let pixels = crate::layout_probe::render(&mut app.view(), size);
        let (x, y) = ((icon.x + 0.5) as usize, icon.center_y() as usize);
        let at = (y * size.width as usize + x) * 4;
        pixels[at..at + 3].to_vec()
    };
    let plain = pixel(&app);
    window.click(&mut app, center(icon));
    window.overlay_event(&mut app, typed("x"), None);
    window.press(&mut app, Named::Escape);
    assert!(!app.search_open);
    assert_eq!(tooltip_label(&app), "x");
    let selected = pixel(&app);
    assert_ne!(plain, selected);
    let primary = app.palette.primary.into_rgba8();
    let distance = |pixel: &[u8]| -> i32 {
        (0..3)
            .map(|channel| (i32::from(pixel[channel]) - i32::from(primary[channel])).abs())
            .sum()
    };
    assert!(
        distance(&selected) < distance(&plain),
        "{selected:?} reads nearer {primary:?} than {plain:?}"
    );
}

#[test]
fn reopening_keeps_the_text_and_adds_to_it() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    let icon = window.bounds(&app, ICON_ID).unwrap();
    window.click(&mut app, center(icon));
    window.overlay_event(&mut app, typed("0"), None);
    window.press(&mut app, Named::Escape);
    window.click(&mut app, center(icon));
    assert!(app.search_open);
    assert_eq!(app.search, "0");
    window.overlay_event(&mut app, typed("3"), None);
    assert_eq!(app.search, "03");
    assert_eq!(names(&app), ["game-003"]);
}

#[test]
fn resizing_between_the_field_and_the_icon_keeps_text_and_focus() {
    let (_root, mut app) = library(100);
    let mut window = Window::new(&mut app);
    window.focus(&app, SEARCH_ID);
    window.send(&mut app, vec![Message::SearchChanged("game".into())]);
    // Narrowing while typing opens the drop-down with the field focused.
    crate::more_menu::tests::at_size(&mut app, 200);
    window.frame(&mut app);
    window.frame(&mut app);
    assert!(app.search_icon);
    assert!(app.search_dropdown_shown());
    assert_eq!(window.focused(&app), ids(SEARCH_ID));
    window.overlay_event(&mut app, typed("-"), None);
    assert_eq!(app.search, "game-");
    // Widening brings the field back to the toolbar, still focused.
    crate::more_menu::tests::at_size(&mut app, 100);
    window.frame(&mut app);
    assert!(!app.search_icon);
    assert!(!app.search_open);
    assert_eq!(window.focused(&app), ids(SEARCH_ID));
    assert!(window.bounds(&app, SEARCH_ID).unwrap().width > crate::grid::SEARCH_MIN_WIDTH);
    assert_eq!(app.search, "game-");
}

#[test]
fn only_one_toolbar_menu_or_drop_down_is_open() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    let icon = window.bounds(&app, ICON_ID).unwrap();
    window.click(&mut app, center(icon));
    assert!(app.search_open);
    window.send(
        &mut app,
        vec![Message::More(crate::more_menu::Event::ToggleMenu)],
    );
    assert!(!app.more_menu);
    window.send(
        &mut app,
        vec![Message::Sort(crate::sort_control::Event::ToggleMenu)],
    );
    assert!(!app.sort_menu);
    assert!(app.search_open);
    window.press(&mut app, Named::Escape);
    window.send(
        &mut app,
        vec![Message::More(crate::more_menu::Event::ToggleMenu)],
    );
    assert!(app.more_menu);
    window.send(&mut app, vec![Message::Search(Event::Toggle)]);
    assert!(!app.search_open, "a menu is open");
    assert!(app.more_menu);
}

#[test]
fn a_focused_icon_hands_its_focus_to_the_field_when_search_comes_back() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    window.focus(&app, ICON_ID);
    crate::more_menu::tests::at_size(&mut app, 100);
    window.frame(&mut app);
    assert!(!app.search_icon);
    assert_eq!(window.focused(&app), ids(SEARCH_ID));
}

/// Opens the drop-down from its icon and types the character `text`.
fn opened_with(app: &mut App, window: &mut Window, text: &str) {
    let icon = window.bounds(app, ICON_ID).unwrap();
    window.click(app, center(icon));
    window.overlay_event(app, typed(text), None);
    assert!(app.search_dropdown_shown());
}

#[test]
fn tab_and_shift_tab_out_of_the_field_close_it_and_move_on_from_the_icon() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    opened_with(&mut app, &mut window, "g");
    window.press(&mut app, Named::Tab);
    assert!(!app.search_open);
    assert_eq!(
        app.search, "g",
        "the text stays, so the icon looks selected"
    );
    assert_eq!(
        window.focused(&app),
        ids(crate::sort_control::SORT_BUTTON_ID)
    );
    // The toolbar's menus open again.
    window.send(
        &mut app,
        vec![Message::More(crate::more_menu::Event::ToggleMenu)],
    );
    assert!(app.more_menu);
    window.press(&mut app, Named::Escape);

    opened_with(&mut app, &mut window, "a");
    window.key(&mut app, Named::Tab, Modifiers::SHIFT);
    assert!(!app.search_open);
    assert_eq!(window.focused(&app), ids(crate::grid::ADD_ID));
}

#[test]
fn down_out_of_the_field_closes_it_and_goes_to_the_grid() {
    let (_root, mut app) = library(200);
    let mut window = Window::new(&mut app);
    opened_with(&mut app, &mut window, "g");
    window.press(&mut app, Named::ArrowDown);
    assert!(!app.search_open);
    assert_eq!(app.selected.as_deref(), Some("game-000"));
    assert!(window.focused(&app).is_empty());
    assert_eq!(app.search, "g");
}

#[test]
fn collapsing_under_a_context_menu_leaves_the_drop_down_closed() {
    let (_root, mut app) = library(100);
    let mut window = Window::new(&mut app);
    window.focus(&app, SEARCH_ID);
    app.action_menu = Some("game-000".to_owned());
    crate::more_menu::tests::at_size(&mut app, 200);
    window.frame(&mut app);
    window.frame(&mut app);
    assert!(app.search_icon);
    assert!(!app.search_open);
}
