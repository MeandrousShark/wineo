use iced::advanced::widget::operation::Focusable;
use iced::advanced::{
    Clipboard, Layout, Renderer as AdvancedRenderer, Shell, Widget, layout, mouse, overlay,
    renderer, widget::tree,
};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

pub(crate) const UNDO_ID: &str = "trash-undo";

pub(crate) fn undo<'a>(
    content: Element<'a, crate::Message>,
    enabled: bool,
) -> Element<'a, crate::Message> {
    Element::new(UndoButton { content, enabled })
}

struct UndoButton<'a, R: AdvancedRenderer = iced::Renderer> {
    content: Element<'a, crate::Message, iced::Theme, R>,
    enabled: bool,
}

#[derive(Default)]
struct State {
    focused: bool,
}

impl iced::advanced::widget::operation::Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }

    fn focus(&mut self) {
        self.focused = true;
    }

    fn unfocus(&mut self) {
        self.focused = false;
    }
}

impl<R> Widget<crate::Message, iced::Theme, R> for UndoButton<'_, R>
where
    R: AdvancedRenderer,
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
        if !self.enabled {
            tree.state.downcast_mut::<State>().unfocus();
        }
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
        renderer: &R,
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
        renderer: &R,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        if self.enabled {
            operation.focusable(
                Some(&iced::widget::Id::new(UNDO_ID)),
                layout.bounds(),
                tree.state.downcast_mut::<State>(),
            );
        } else {
            tree.state.downcast_mut::<State>().unfocus();
        }

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
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, crate::Message>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(iced::touch::Event::FingerPressed { .. }) => {
                let focused = self.enabled && cursor.is_over(layout.bounds());
                if state.focused != focused {
                    state.focused = focused;
                    shell.request_redraw();
                }
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key:
                    iced::keyboard::Key::Named(
                        iced::keyboard::key::Named::Enter | iced::keyboard::key::Named::Space,
                    ),
                repeat: false,
                ..
            }) if self.enabled && state.focused => {
                shell.publish(crate::Message::Undo);
                shell.capture_event();
                return;
            }
            _ => {}
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
        renderer: &mut R,
        theme: &iced::Theme,
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

        if self.enabled && tree.state.downcast_ref::<State>().focused {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: layout.bounds().expand(2.0),
                    border: iced::border::rounded(6)
                        .width(2)
                        .color(theme.palette().primary),
                    shadow: iced::Shadow::default(),
                    snap: false,
                },
                iced::Color::TRANSPARENT,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
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
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, crate::Message, iced::Theme, R>> {
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
    use iced::keyboard::{self, Key, key::Named};

    #[test]
    fn focus_operation_registers_the_undo_id_and_enter_activates_it() {
        let button = iced::widget::button::Button::<crate::Message, iced::Theme, ()>::new("Undo")
            .on_press(crate::Message::Undo);
        let mut widget: Element<'_, crate::Message, iced::Theme, ()> = Element::new(UndoButton {
            content: button.into(),
            enabled: true,
        });
        let mut tree = tree::Tree::new(widget.as_widget());
        let bounds = Size::new(100.0, 32.0);
        let node =
            widget
                .as_widget_mut()
                .layout(&mut tree, &(), &layout::Limits::new(Size::ZERO, bounds));
        let mut operation =
            iced::advanced::widget::operation::focusable::focus(iced::widget::Id::new(UNDO_ID));
        widget
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &(), &mut operation);

        let event = key_event(Named::Enter);
        let mut messages = Vec::new();
        let status = {
            let mut shell = Shell::new(&mut messages);
            widget.as_widget_mut().update(
                &mut tree,
                &event,
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &(),
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
            shell.event_status()
        };

        assert!(matches!(messages.as_slice(), [crate::Message::Undo]));
        assert_eq!(status, iced::event::Status::Captured);
    }

    #[test]
    fn focused_space_activates_but_unfocused_space_does_not() {
        let button = iced::widget::button::Button::<crate::Message, iced::Theme, ()>::new("Undo")
            .on_press(crate::Message::Undo);
        let mut widget: Element<'_, crate::Message, iced::Theme, ()> = Element::new(UndoButton {
            content: button.into(),
            enabled: true,
        });
        let mut tree = tree::Tree::new(widget.as_widget());
        let bounds = Size::new(100.0, 32.0);
        let node =
            widget
                .as_widget_mut()
                .layout(&mut tree, &(), &layout::Limits::new(Size::ZERO, bounds));
        let mut messages = Vec::new();
        {
            let mut shell = Shell::new(&mut messages);
            widget.as_widget_mut().update(
                &mut tree,
                &key_event(Named::Space),
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &(),
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
        }
        assert!(messages.is_empty());

        tree.state.downcast_mut::<State>().focus();
        {
            let mut shell = Shell::new(&mut messages);
            widget.as_widget_mut().update(
                &mut tree,
                &key_event(Named::Space),
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &(),
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
        }

        assert!(matches!(messages.as_slice(), [crate::Message::Undo]));
    }

    fn key_event(key: Named) -> Event {
        let key = Key::Named(key);
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        })
    }
}
