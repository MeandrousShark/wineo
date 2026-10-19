use iced::advanced::{
    Clipboard, Layout, Renderer as AdvancedRenderer, Shell, Widget, layout, mouse, overlay,
    renderer, widget::tree,
};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

pub fn scale<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    factor: f32,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: AdvancedRenderer + 'a,
{
    Element::new(ScaledWheel {
        content: content.into(),
        factor,
    })
}

struct ScaledWheel<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    factor: f32,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ScaledWheel<'_, Message, Theme, Renderer>
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
        let scaled_event = match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                Some(Event::Mouse(mouse::Event::WheelScrolled {
                    delta: scale_delta(*delta, self.factor),
                }))
            }
            _ => None,
        };

        self.content.as_widget_mut().update(
            tree,
            scaled_event.as_ref().unwrap_or(event),
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

pub(crate) fn scale_delta(delta: mouse::ScrollDelta, factor: f32) -> mouse::ScrollDelta {
    match delta {
        mouse::ScrollDelta::Lines { x, y } => mouse::ScrollDelta::Lines {
            x: x * factor,
            y: y * factor,
        },
        mouse::ScrollDelta::Pixels { x, y } => mouse::ScrollDelta::Pixels {
            x: x * factor,
            y: y * factor,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::scale_delta;
    use iced::advanced::mouse::ScrollDelta;

    #[test]
    fn scales_line_wheel_deltas() {
        assert_eq!(
            scale_delta(ScrollDelta::Lines { x: 2.0, y: -4.0 }, 0.5),
            ScrollDelta::Lines { x: 1.0, y: -2.0 }
        );
    }

    #[test]
    fn scales_pixel_wheel_deltas() {
        assert_eq!(
            scale_delta(ScrollDelta::Pixels { x: 8.0, y: -20.0 }, 0.25),
            ScrollDelta::Pixels { x: 2.0, y: -5.0 }
        );
    }
}
