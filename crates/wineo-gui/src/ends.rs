//! A header layout that pins one group left and another right on a single
//! line, and stacks them when the line is too narrow for both. A plain row
//! with a fill spacer lays its children out in order, so a long left side
//! squeezes the right side's buttons to nothing instead.

use iced::advanced::{
    Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
    widget::{Operation, tree::Tree},
};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// `left` and `right` share a line, right aligned, when both fit. Otherwise
/// `right` sits under `left`. Each side must wrap its own content, since it
/// is given the full width.
pub fn ends<'a, Message, Theme, Renderer>(
    left: impl Into<Element<'a, Message, Theme, Renderer>>,
    right: impl Into<Element<'a, Message, Theme, Renderer>>,
    spacing: f32,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(Ends {
        children: [left.into(), right.into()],
        spacing,
    })
}

struct Ends<'a, Message, Theme, Renderer> {
    children: [Element<'a, Message, Theme, Renderer>; 2],
    spacing: f32,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Ends<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let limits = limits.width(Length::Fill).height(Length::Shrink);
        let child_limits = limits.loose();
        let [left, right] = &mut self.children;
        let [left_tree, right_tree] = &mut tree.children[..] else {
            unreachable!("two children")
        };
        let left = left
            .as_widget_mut()
            .layout(left_tree, renderer, &child_limits);
        let right = right
            .as_widget_mut()
            .layout(right_tree, renderer, &child_limits);
        let (left_size, right_size) = (left.size(), right.size());
        let room = limits.max().width;
        let one_line = left_size.width + self.spacing + right_size.width <= room;
        let (content, left, right) = if one_line {
            let height = left_size.height.max(right_size.height);
            (
                Size::new(room, height),
                left.move_to((0.0, (height - left_size.height) / 2.0)),
                right.move_to((room - right_size.width, (height - right_size.height) / 2.0)),
            )
        } else {
            (
                Size::new(
                    left_size.width.max(right_size.width),
                    left_size.height + self.spacing + right_size.height,
                ),
                left.move_to((0.0, 0.0)),
                right.move_to((0.0, left_size.height + self.spacing)),
            )
        };
        let size = limits.resolve(Length::Fill, Length::Shrink, content);
        layout::Node::with_children(size, vec![left, right])
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            self.children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .for_each(|((child, state), layout)| {
                    child
                        .as_widget_mut()
                        .operate(state, layout, renderer, operation);
                });
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for ((child, tree), layout) in self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                tree, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, tree), layout)| {
                child
                    .as_widget()
                    .mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for ((child, tree), layout) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
        {
            child
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}
