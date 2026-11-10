use iced::widget::{Space, button, column};
use iced::{Point, Size, mouse};
use iced_runtime::user_interface::{Cache, UserInterface};

use super::*;

#[derive(Debug, Clone, PartialEq)]
enum Pressed {
    Control,
    Row,
    Dismiss,
}

/// A control 100 px down, with its popup open when `open`.
fn view(open: bool) -> Element<'static, Pressed> {
    let popup = open.then(|| button("Row").on_press(Pressed::Row).into());
    column![
        Space::new().height(100),
        dropdown(
            button("Pick").on_press(Pressed::Control),
            popup,
            Pressed::Dismiss
        ),
    ]
    .into()
}

/// What `events` with the cursor at `at` publish, and whether the first was
/// captured.
fn run(open: bool, at: Point, events: &[Event]) -> (Vec<Pressed>, bool) {
    let mut renderer = crate::layout_probe::renderer();
    let mut ui = UserInterface::build(
        view(open),
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
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(Named::Escape),
        modified_key: keyboard::Key::Named(Named::Escape),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::empty(),
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
