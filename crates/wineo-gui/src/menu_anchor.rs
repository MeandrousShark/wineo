use iced::advanced::{
    Clipboard, Layout, Renderer as AdvancedRenderer, Shell, Widget, layout, mouse, overlay,
    renderer, widget::tree,
};
use iced::keyboard::{self, key::Named};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// Publishes the cursor's window position before any child sees an input
/// that can open the action menu: a right press on a card, a left release on
/// the panel's Actions button, or Enter/Space on it. Tracking every cursor
/// move instead would rebuild the whole view on each one.
pub(crate) fn track<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_anchor: impl Fn(Point) -> Message + 'a,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: AdvancedRenderer + 'a,
{
    Element::new(MenuAnchor {
        content: content.into(),
        on_anchor: Box::new(on_anchor),
    })
}

struct MenuAnchor<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_anchor: Box<dyn Fn(Point) -> Message + 'a>,
}

/// The last cursor position seen inside the window. iced forgets the
/// cursor when it leaves, but the keyboard can still open the menu then.
#[derive(Default)]
struct State {
    last: Option<Point>,
}

fn clicks_menu(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(
            mouse::Event::ButtonPressed(mouse::Button::Right)
                | mouse::Event::ButtonReleased(mouse::Button::Left)
        )
    )
}

fn keys_menu(event: &Event) -> bool {
    matches!(
        event,
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(Named::Enter | Named::Space),
            repeat: false,
            ..
        })
    )
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for MenuAnchor<'_, Message, Theme, Renderer>
where
    Renderer: AdvancedRenderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<tree::Tree> {
        vec![tree::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut tree::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut tree::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut tree::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        if let Event::Mouse(mouse::Event::CursorMoved { position }) = event {
            state.last = Some(*position);
        }
        let anchor = if clicks_menu(event) {
            cursor.position()
        } else if keys_menu(event) {
            cursor.position().or(state.last)
        } else {
            None
        };
        if let Some(position) = anchor {
            shell.publish((self.on_anchor)(position));
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &tree::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut tree::Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use iced::widget::mouse_area;

    fn messages(event: Event, cursor: mouse::Cursor) -> Vec<Message> {
        sequence(&[(event, cursor)])
    }

    /// Feeds events through one widget tree and returns every message.
    fn sequence(events: &[(Event, mouse::Cursor)]) -> Vec<Message> {
        let card = mouse_area(iced::widget::Space::new().width(100).height(100))
            .on_right_press(Message::Context("game".into()));
        let mut widget: Element<'_, Message, iced::Theme, ()> = track(card, Message::MenuAnchor);
        let mut tree = tree::Tree::new(widget.as_widget());
        let bounds = Size::new(100.0, 100.0);
        let node =
            widget
                .as_widget_mut()
                .layout(&mut tree, &(), &layout::Limits::new(Size::ZERO, bounds));
        let mut messages = Vec::new();
        for (event, cursor) in events {
            let mut shell = Shell::new(&mut messages);
            widget.as_widget_mut().update(
                &mut tree,
                event,
                Layout::new(&node),
                *cursor,
                &(),
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
        }
        messages
    }

    fn enter(repeat: bool) -> Event {
        let key = keyboard::Key::Named(Named::Enter);
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat,
        })
    }

    #[test]
    fn right_press_anchors_before_the_card_opens_its_menu() {
        let at = Point::new(30.0, 40.0);
        let messages = messages(
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
            mouse::Cursor::Available(at),
        );
        assert!(matches!(
            messages.as_slice(),
            [Message::MenuAnchor(point), Message::Context(name)] if *point == at && name == "game"
        ));
    }

    #[test]
    fn moves_unknown_cursors_and_key_repeats_publish_nothing() {
        let at = Point::new(30.0, 40.0);
        assert!(
            messages(
                Event::Mouse(mouse::Event::CursorMoved { position: at }),
                mouse::Cursor::Available(at),
            )
            .is_empty()
        );
        assert!(
            messages(
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
                mouse::Cursor::Unavailable,
            )
            .is_empty()
        );
        for repeat in [false, true] {
            let anchored = messages(enter(repeat), mouse::Cursor::Available(at))
                .iter()
                .any(|message| matches!(message, Message::MenuAnchor(_)));
            assert_eq!(anchored, !repeat);
        }
    }

    #[test]
    fn keys_outside_the_window_anchor_at_the_last_cursor_position() {
        let last = Point::new(70.0, 20.0);
        let messages = sequence(&[
            (
                Event::Mouse(mouse::Event::CursorMoved { position: last }),
                mouse::Cursor::Available(last),
            ),
            (
                Event::Mouse(mouse::Event::CursorLeft),
                mouse::Cursor::Unavailable,
            ),
            (enter(false), mouse::Cursor::Unavailable),
            (
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
                mouse::Cursor::Unavailable,
            ),
        ]);
        assert!(matches!(
            messages.as_slice(),
            [Message::MenuAnchor(point)] if *point == last
        ));
    }
}
