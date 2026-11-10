use iced::widget::{Space, button, column};
use iced::{Point, Size, mouse};
use iced_runtime::user_interface::{Cache, UserInterface};

use super::*;

#[derive(Debug, Clone, PartialEq)]
enum Pressed {
    Control,
    Row,
    Dismiss,
    Up,
    Down,
    Enter,
}

/// A control 100 px down, with its popup open when `open`, and the arrow
/// and Enter messages when `keys`.
fn view(open: bool, keys: bool) -> Element<'static, Pressed> {
    let popup = open.then(|| button("Row").on_press(Pressed::Row).into());
    let mut dropdown = dropdown(
        button("Pick").on_press(Pressed::Control),
        popup,
        Pressed::Dismiss,
    );
    if keys {
        dropdown = dropdown.on_keys(Pressed::Up, Pressed::Down, Pressed::Enter);
    }
    column![Space::new().height(100), dropdown].into()
}

/// What `events` with the cursor at `at` publish, and whether the first was
/// captured.
fn run(open: bool, at: Point, events: &[Event]) -> (Vec<Pressed>, bool) {
    run_with(open, false, at, events)
}

fn run_with(open: bool, keys: bool, at: Point, events: &[Event]) -> (Vec<Pressed>, bool) {
    let mut renderer = crate::layout_probe::renderer();
    let mut ui = UserInterface::build(
        view(open, keys),
        Size::new(400.0, 600.0),
        Cache::default(),
        &mut renderer,
    );
    let mut messages = Vec::new();
    let (_, statuses) = ui.update(
        events,
        mouse::Cursor::Available(at),
        &mut renderer,
        &mut iced::advanced::clipboard::Null,
        &mut messages,
    );
    (messages, statuses[0] == iced::event::Status::Captured)
}

fn click() -> [Event; 2] {
    let left = mouse::Button::Left;
    [
        Event::Mouse(mouse::Event::ButtonPressed(left)),
        Event::Mouse(mouse::Event::ButtonReleased(left)),
    ]
}

fn escape() -> Event {
    press(keyboard::Key::Named(Named::Escape))
}

fn press(key: keyboard::Key) -> Event {
    press_with(key, keyboard::Modifiers::empty())
}

fn press_with(key: keyboard::Key, modifiers: keyboard::Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
        text: None,
        repeat: false,
    })
}

const CONTROL: Point = Point::new(20.0, 110.0);
/// Just under the control, where the popup's row opens.
const ROW: Point = Point::new(20.0, 140.0);
const ELSEWHERE: Point = Point::new(300.0, 400.0);

#[test]
fn a_closed_dropdown_is_its_control() {
    assert_eq!(run(false, CONTROL, &click()).0, [Pressed::Control]);
    assert_eq!(run(false, ROW, &click()).0, []);
}

#[test]
fn the_popup_takes_its_own_presses() {
    assert_eq!(run(true, ROW, &click()), (vec![Pressed::Row], true));
}

#[test]
fn a_press_outside_or_escape_dismisses() {
    assert_eq!(
        run(true, ELSEWHERE, &click()),
        (vec![Pressed::Dismiss], false)
    );
    assert_eq!(
        run(true, ELSEWHERE, &[escape()]),
        (vec![Pressed::Dismiss], true)
    );
}

#[test]
fn pressing_the_control_while_open_only_dismisses() {
    // Its release would otherwise open the popup again.
    assert_eq!(run(true, CONTROL, &click()), (vec![Pressed::Dismiss], true));
}

#[test]
fn the_arrows_and_enter_publish_their_messages_while_open() {
    for (key, sent) in [
        (Named::ArrowUp, Pressed::Up),
        (Named::ArrowDown, Pressed::Down),
        (Named::Enter, Pressed::Enter),
    ] {
        let event = press(keyboard::Key::Named(key));
        assert_eq!(
            run_with(true, true, ELSEWHERE, std::slice::from_ref(&event)),
            (vec![sent], true),
            "{key:?}"
        );
        assert_eq!(
            run_with(false, true, ELSEWHERE, &[event]),
            (vec![], false),
            "{key:?} while closed"
        );
    }
    // Escape still dismisses, and other keys, like typing, pass to the popup.
    assert_eq!(
        run_with(true, true, ELSEWHERE, &[escape()]),
        (vec![Pressed::Dismiss], true)
    );
    let typed = press(keyboard::Key::Character("a".into()));
    assert_eq!(run_with(true, true, ELSEWHERE, &[typed]), (vec![], false));
}

#[test]
fn without_keys_the_arrows_pass_through() {
    let event = press(keyboard::Key::Named(Named::ArrowDown));
    assert_eq!(run(true, ELSEWHERE, &[event]), (vec![], false));
}

#[test]
fn tab_and_shift_tab_move_like_the_arrows_while_open() {
    let tab = keyboard::Key::Named(Named::Tab);
    let down = press(tab.clone());
    let up = press_with(tab, keyboard::Modifiers::SHIFT);
    assert_eq!(
        run_with(true, true, ELSEWHERE, std::slice::from_ref(&down)),
        (vec![Pressed::Down], true)
    );
    assert_eq!(
        run_with(true, true, ELSEWHERE, &[up]),
        (vec![Pressed::Up], true)
    );
    // Closed, Tab is left to move focus.
    assert_eq!(run_with(false, true, ELSEWHERE, &[down]), (vec![], false));
}
