//! A control with a popup that opens under it, for dropdowns a pick list
//! can't draw, such as one with a search field or rows in their own fonts.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::keyboard::{self, key::Named};
use iced::{Color, Element, Event, Length, Point, Rectangle, Size, Vector};

/// Room kept between the popup and the window's edges.
const MARGIN: f32 = 8.0;
/// Between the control and the popup.
const GAP: f32 = 4.0;

/// `underlay`, with `popup` under it while that's Some. A press outside the
/// popup or Escape publishes `on_dismiss`.
pub(crate) fn dropdown<'a, Message: Clone + 'a>(
    underlay: impl Into<Element<'a, Message>>,
    popup: Option<Element<'a, Message>>,
    on_dismiss: Message,
) -> Element<'a, Message> {
    Element::new(Dropdown {
        underlay: underlay.into(),
        popup,
        on_dismiss,
    })
}

struct Dropdown<'a, Message> {
    underlay: Element<'a, Message>,
    popup: Option<Element<'a, Message>>,
    on_dismiss: Message,
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer> for Dropdown<'_, Message> {
    fn children(&self) -> Vec<Tree> {
        let mut children = vec![Tree::new(&self.underlay)];
        children.extend(self.popup.as_ref().map(Tree::new));
        children
    }

    fn diff(&self, tree: &mut Tree) {
        match &self.popup {
            Some(popup) => tree.diff_children(&[&self.underlay, popup]),
            None => tree.diff_children(std::slice::from_ref(&self.underlay)),
        }
    }

    fn size(&self) -> Size<Length> {
        self.underlay.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.underlay.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.underlay
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.underlay
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.underlay.as_widget_mut().update(
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
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.underlay.as_widget().draw(
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
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.underlay.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        _renderer: &iced::Renderer,
        _viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let popup = self.popup.as_mut()?;
        Some(overlay::Element::new(Box::new(Popup {
            popup,
            tree: &mut tree.children[1],
            anchor: layout.bounds() + translation,
            on_dismiss: self.on_dismiss.clone(),
        })))
    }
}

struct Popup<'a, 'b, Message> {
    popup: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
    /// The control's bounds in the window.
    anchor: Rectangle,
    on_dismiss: Message,
}

impl<Message: Clone> overlay::Overlay<Message, iced::Theme, iced::Renderer>
    for Popup<'_, '_, Message>
{
    /// As wide as the control, under it, or over it when there's more room
    /// above.
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        let below = bounds.height - (self.anchor.y + self.anchor.height + GAP) - MARGIN;
        let above = self.anchor.y - GAP - MARGIN;
        let room = below.max(above).max(0.0);
        let width = self.anchor.width;
        let node = self.popup.as_widget_mut().layout(
            self.tree,
            renderer,
            &layout::Limits::new(Size::new(width, 0.0), Size::new(width, room)),
        );
        let height = node.size().height;
        let y = if height <= below || below >= above {
            self.anchor.y + self.anchor.height + GAP
        } else {
            self.anchor.y - GAP - height
        };
        let x = self
            .anchor
            .x
            .min(bounds.width - width - MARGIN)
            .max(MARGIN.min(self.anchor.x));
        node.move_to(Point::new(x, y))
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        let bounds = layout.bounds();
        self.popup
            .as_widget()
            .draw(self.tree, renderer, theme, style, layout, cursor, &bounds);
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.popup
            .as_widget_mut()
            .operate(self.tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = layout.bounds();
        match event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(Named::Escape),
                ..
            }) => {
                shell.publish(self.on_dismiss.clone());
                shell.capture_event();
                return;
            }
            Event::Mouse(mouse::Event::ButtonPressed(_)) if !cursor.is_over(bounds) => {
                shell.publish(self.on_dismiss.clone());
                // The control's own press would open it again on release.
                if cursor.is_over(self.anchor) {
                    shell.capture_event();
                }
                return;
            }
            _ => {}
        }
        self.popup.as_widget_mut().update(
            self.tree, event, layout, cursor, renderer, clipboard, shell, &bounds,
        );
        // Nothing under the popup takes input meant for it.
        if cursor.is_over(bounds) && matches!(event, Event::Mouse(_) | Event::Touch(_)) {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let interaction = self.popup.as_widget().mouse_interaction(
            self.tree,
            layout,
            cursor,
            &layout.bounds(),
            renderer,
        );
        // Anything but None keeps the cursor from the widgets under it.
        if interaction == mouse::Interaction::None && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Idle
        } else {
            interaction
        }
    }
}

/// A pick list's arrow, drawn over a control the way iced's pick list draws
/// its own: right-aligned inside `right` padding and centered on the
/// control's middle, so the two line up to the pixel.
pub(crate) fn arrow<'a, Message: 'a>(color: Color, right: f32) -> Element<'a, Message> {
    Element::new(Arrow { color, right })
}

struct Arrow {
    color: Color,
    right: f32,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Arrow {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fill, Size::ZERO))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::text::{self, Renderer as _};
        let bounds = layout.bounds();
        let size = renderer.default_size();
        let line_height = text::LineHeight::default();
        renderer.fill_text(
            text::Text {
                content: iced::Renderer::ARROW_DOWN_ICON.to_string(),
                bounds: Size::new(bounds.width, f32::from(line_height.to_absolute(size))),
                size,
                line_height,
                font: iced::Renderer::ICON_FONT,
                align_x: text::Alignment::Right,
                align_y: iced::alignment::Vertical::Center,
                shaping: text::Shaping::Basic,
                wrapping: text::Wrapping::default(),
            },
            Point::new(bounds.x + bounds.width - self.right, bounds.center_y()),
            self.color,
            *viewport,
        );
    }
}

#[cfg(test)]
mod tests;
