use iced::mouse::{Cursor, ScrollDelta};
use iced::{Point, Size};

use super::*;
use crate::layout_probe::lay_out;
use crate::theme::Palette;

fn element(value: f32, dragging: bool) -> Element<'static, Message> {
    Element::new(Slider {
        value,
        dragging,
        modifiers: keyboard::Modifiers::empty(),
        palette: Palette::DARK,
    })
}

fn slider(value: f32) -> crate::layout_probe::Laid<'static, Message> {
    lay_out(element(value, false), Size::new(400.0, 100.0))
}

fn mouse(event: mouse::Event) -> Event {
    Event::Mouse(event)
}

#[test]
fn the_track_spans_the_range_with_the_tick_at_one_hundred() {
    let bounds = Rectangle::new(Point::new(10.0, 0.0), Size::new(WIDTH, 32.0));
    assert_eq!(x_of(bounds, 60.0), 17.0);
    assert_eq!(x_of(bounds, 200.0), 99.0);
    assert_eq!(zoom_at(bounds, 17.0), 60.0);
    assert_eq!(zoom_at(bounds, 99.0), 200.0);
    // Past either end still means the end.
    assert_eq!(zoom_at(bounds, 0.0), 60.0);
    assert_eq!(zoom_at(bounds, 500.0), 200.0);
    // 100% sits two sevenths along.
    let tick = x_of(bounds, 100.0);
    assert!((tick - (17.0 + 82.0 * 2.0 / 7.0)).abs() < 1e-4, "{tick}");
    assert!((zoom_at(bounds, tick) - 100.0).abs() < 1e-3);
}

#[test]
fn a_drag_reports_where_the_thumb_is_and_ends_once() {
    let mut laid = slider(100.0);
    let bounds = laid.node.bounds();
    assert_eq!(bounds.size(), Size::new(WIDTH, 32.0));
    let at = |x: f32| Point::new(x, 16.0);
    let start = at(x_of(bounds, 100.0));
    let messages = laid.event_at(
        &mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Cursor::Available(start),
    );
    assert!(matches!(
        messages.as_slice(),
        [Message::Zoom(ZoomEvent::Drag(zoom))] if (zoom - 100.0).abs() < 1e-3
    ));
    // The press focused it, like the toolbar's buttons.
    assert_eq!(laid.focused(), [iced::widget::Id::new(SLIDER_ID)]);
    // It keeps following past its own edge.
    let messages = laid.event_at(
        &mouse(mouse::Event::CursorMoved {
            position: at(300.0),
        }),
        Cursor::Available(at(300.0)),
    );
    assert!(matches!(
        messages.as_slice(),
        [Message::Zoom(ZoomEvent::Drag(200.0))]
    ));
    let release = mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
    let messages = laid.event_at(&release, Cursor::Available(at(300.0)));
    assert!(matches!(
        messages.as_slice(),
        [Message::Zoom(ZoomEvent::DragEnded)]
    ));
    assert!(
        laid.event_at(&release, Cursor::Available(at(300.0)))
            .is_empty()
    );
    let moved = mouse(mouse::Event::CursorMoved { position: at(20.0) });
    assert!(
        laid.event_at(&moved, Cursor::Available(at(20.0)))
            .is_empty()
    );
    // A press elsewhere lets focus go.
    let elsewhere = at(300.0);
    let messages = laid.event_at(
        &mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Cursor::Available(elsewhere),
    );
    assert!(messages.is_empty());
    assert!(laid.focused().is_empty());
}

#[test]
fn only_ctrl_wheel_over_the_slider_zooms() {
    let mut laid = slider(100.0);
    let center = laid.node.bounds().center();
    let notch = mouse(mouse::Event::WheelScrolled {
        delta: ScrollDelta::Lines { x: 0.0, y: 1.0 },
    });
    assert!(laid.event_at(&notch, Cursor::Available(center)).is_empty());
    let ctrl = Event::Keyboard(keyboard::Event::ModifiersChanged(keyboard::Modifiers::CTRL));
    let _ = laid.event(&ctrl);
    assert!(matches!(
        laid.event_at(&notch, Cursor::Available(center)).as_slice(),
        [Message::Zoom(ZoomEvent::Wheel(_))]
    ));
    assert!(
        laid.event_at(&notch, Cursor::Available(Point::new(300.0, 50.0)))
            .is_empty()
    );
}

#[test]
fn the_toggle_names_what_a_press_does() {
    assert_eq!(toggle_label(false), "Hide titles");
    assert_eq!(toggle_label(true), "Show titles");
}

#[test]
fn a_drag_the_app_ended_without_a_release_stays_ended() {
    let mut laid = slider(100.0);
    let center = laid.node.bounds().center();
    let _ = laid.event_at(
        &mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Cursor::Available(center),
    );
    let moved = mouse(mouse::Event::CursorMoved {
        position: Point::new(300.0, 16.0),
    });
    // While the app follows the drag, a rebuild keeps it going.
    let mut laid = crate::layout_probe::relay_out(
        element(100.0, true),
        laid.into_tree(),
        Size::new(400.0, 100.0),
    );
    assert!(matches!(
        laid.event_at(&moved, Cursor::Available(Point::new(300.0, 16.0)))
            .as_slice(),
        [Message::Zoom(ZoomEvent::Drag(_))]
    ));
    // A dialog took the release and the app ended the drag.
    let mut laid = crate::layout_probe::relay_out(
        element(100.0, false),
        laid.into_tree(),
        Size::new(400.0, 100.0),
    );
    assert!(
        laid.event_at(&moved, Cursor::Available(Point::new(300.0, 16.0)))
            .is_empty()
    );
}

#[test]
fn the_slider_starts_from_and_follows_the_apps_modifiers() {
    let element = |modifiers| {
        Element::new(Slider {
            value: 100.0,
            dragging: false,
            modifiers,
            palette: Palette::DARK,
        })
    };
    let notch = mouse(mouse::Event::WheelScrolled {
        delta: ScrollDelta::Lines { x: 0.0, y: 1.0 },
    });
    // Ctrl went down before the slider was built.
    let mut laid = lay_out(element(keyboard::Modifiers::CTRL), Size::new(400.0, 100.0));
    let center = laid.node.bounds().center();
    assert!(matches!(
        laid.event_at(&notch, Cursor::Available(center)).as_slice(),
        [Message::Zoom(ZoomEvent::Wheel(_))]
    ));
    // Losing the window's focus lets Ctrl go.
    let _ = laid.event(&Event::Window(iced::window::Event::Unfocused));
    assert!(laid.event_at(&notch, Cursor::Available(center)).is_empty());
    // A rebuild takes the app's view of the keys.
    let mut laid = crate::layout_probe::relay_out(
        element(keyboard::Modifiers::CTRL),
        laid.into_tree(),
        Size::new(400.0, 100.0),
    );
    assert!(matches!(
        laid.event_at(&notch, Cursor::Available(center)).as_slice(),
        [Message::Zoom(ZoomEvent::Wheel(_))]
    ));
}
