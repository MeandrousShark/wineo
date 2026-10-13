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

fn opens_menu(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(
            mouse::Event::ButtonPressed(mouse::Button::Right)
                | mouse::Event::ButtonReleased(mouse::Button::Left)
        ) | Event::Keyboard(keyboard::Event::KeyPressed {
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
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<tree::Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut tree::Tree) {
        self.content.as_widget().diff(tree);
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
        self.content.as_widget_mut().layout(tree, renderer, limits)
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
            .operate(tree, layout, renderer, operation);
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
        if opens_menu(event)
            && let Some(position) = cursor.position()
        {
            shell.publish((self.on_anchor)(position));
        }
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
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
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut tree::Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use iced::widget::mouse_area;

    fn messages(event: Event, cursor: mouse::Cursor) -> Vec<Message> {
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
        let mut shell = Shell::new(&mut messages);
        widget.as_widget_mut().update(
            &mut tree,
            &event,
            Layout::new(&node),
            cursor,
            &(),
            &mut iced::advanced::clipboard::Null,
            &mut shell,
            &Rectangle::with_size(bounds),
        );
        messages
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
            let key = keyboard::Key::Named(Named::Enter);
            let press = Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat,
            });
            let anchored = messages(press, mouse::Cursor::Available(at))
                .iter()
                .any(|message| matches!(message, Message::MenuAnchor(_)));
            assert_eq!(anchored, !repeat);
        }
    }
}
