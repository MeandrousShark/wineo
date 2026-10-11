//! A header layout that pins one group left and another right on a single
//! line, and stacks them when the line is too narrow for both. A plain row
//! with a fill spacer lays its children out in order, so a long left side
//! squeezes the right side's buttons to nothing instead.

use iced::advanced::{
    Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
    widget::{Operation, tree::Tree},
};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// The narrowest left side worth keeping beside the right: a button and a
/// few words of text.
pub const MIN_LEFT: f32 = 240.0;

/// `left` and `right` share a line, right aligned, when both fit. When the
/// left side is too wide, it gets the room the right side leaves, as long as
/// that is at least `min_left`. Below that, `right` sits under `left`.
///
/// Each side must wrap its own content, since it is offered the full width.
/// A fill-width left side takes whatever the right side leaves, like a
/// spacer. A fill-width right side takes the whole line, so it always stacks.
/// With no width limit the sides sit on one line at their natural widths, and
/// a fill-width child counts as zero wide.
pub fn ends<'a, Message, Theme, Renderer>(
    left: impl Into<Element<'a, Message, Theme, Renderer>>,
    right: impl Into<Element<'a, Message, Theme, Renderer>>,
    spacing: f32,
    min_left: f32,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(Ends {
        children: [left.into(), right.into()],
        spacing,
        min_left,
    })
}

struct Ends<'a, Message, Theme, Renderer> {
    children: [Element<'a, Message, Theme, Renderer>; 2],
    spacing: f32,
    min_left: f32,
}

/// An unbounded child asks for an infinite size, which would poison the sums.
fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
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
        let child_limits = limits.loose();
        let room = limits.max().width;
        let bounded = room.is_finite();
        let [left, right] = &mut self.children;
        let [left_tree, right_tree] = &mut tree.children[..] else {
            unreachable!("two children")
        };
        // The right side goes first so the left side can take what it leaves.
        let right = right
            .as_widget_mut()
            .layout(right_tree, renderer, &child_limits);
        let right_width = finite(right.size().width);
        let right_height = finite(right.size().height);
        let mut left_node = left
            .as_widget_mut()
            .layout(left_tree, renderer, &child_limits);
        if bounded && finite(left_node.size().width) + self.spacing + right_width > room {
            let remainder = room - self.spacing - right_width;
            if remainder >= self.min_left {
                left_node = left.as_widget_mut().layout(
                    left_tree,
                    renderer,
                    &layout::Limits::new(
                        Size::ZERO,
                        Size::new(remainder, child_limits.max().height),
                    ),
                );
            }
        }
        let left_width = finite(left_node.size().width);
        let left_height = finite(left_node.size().height);
        let one_line = !bounded || left_width + self.spacing + right_width <= room + 0.01;
        let (content, left_node, right) = if one_line {
            let height = left_height.max(right_height);
            let width = if bounded {
                room
            } else {
                left_width + self.spacing + right_width
            };
            (
                Size::new(width, height),
                left_node.move_to((0.0, (height - left_height) / 2.0)),
                right.move_to((width - right_width, (height - right_height) / 2.0)),
            )
        } else {
            (
                Size::new(
                    left_width.max(right_width),
                    left_height + self.spacing + right_height,
                ),
                left_node.move_to((0.0, 0.0)),
                right.move_to((0.0, left_height + self.spacing)),
            )
        };
        let width = if bounded {
            Length::Fill
        } else {
            Length::Shrink
        };
        let size = limits.resolve(width, Length::Shrink, content);
        layout::Node::with_children(size, vec![left_node, right])
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
        if layout.bounds().intersection(viewport).is_none() {
            return;
        }
        for ((child, tree), layout) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|(_, layout)| layout.bounds().intersects(viewport))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_probe::layout;
    use iced::Fill;
    use iced::widget::Space;

    fn block(width: f32, height: f32) -> Element<'static, ()> {
        Space::new().width(width).height(height).into()
    }

    /// The size of the widget and the bounds of both children inside it.
    fn place(
        left: Element<'static, ()>,
        right: Element<'static, ()>,
        min_left: f32,
        room: f32,
    ) -> (Size, Rectangle, Rectangle) {
        let mut view = ends(left, right, 10.0, min_left);
        let node = layout(&mut view, Size::new(room, 400.0));
        let children = node.children();
        (node.size(), children[0].bounds(), children[1].bounds())
    }

    #[test]
    fn unbounded_width_puts_both_sides_on_one_line_at_natural_widths() {
        let (size, left, right) = place(block(100.0, 20.0), block(50.0, 30.0), 0.0, f32::INFINITY);
        assert_eq!(size, Size::new(160.0, 30.0));
        assert_eq!((left.x, left.y), (0.0, 5.0));
        assert_eq!((right.x, right.y), (110.0, 0.0));
    }

    #[test]
    fn unbounded_width_treats_a_fill_child_as_zero_wide() {
        let (size, left, right) = place(
            Space::new().width(Fill).height(20).into(),
            block(50.0, 30.0),
            0.0,
            f32::INFINITY,
        );
        assert!(
            size.width.is_finite() && size.height.is_finite(),
            "{size:?}"
        );
        assert_eq!(size.width, 60.0);
        assert!(left.x.is_finite() && left.y.is_finite());
        assert_eq!(right.x, size.width - 50.0);
    }

    #[test]
    fn a_fill_left_side_takes_what_the_right_leaves() {
        let (size, left, right) = place(
            Space::new().width(Fill).height(20).into(),
            block(50.0, 30.0),
            100.0,
            400.0,
        );
        assert_eq!(size.width, 400.0);
        assert_eq!(left.width, 340.0);
        assert_eq!(right.x, 350.0);
    }

    #[test]
    fn a_fill_right_side_takes_the_line_so_it_stacks() {
        let (_, left, right) = place(
            block(100.0, 20.0),
            Space::new().width(Fill).height(30).into(),
            0.0,
            400.0,
        );
        assert_eq!((right.x, right.y), (0.0, 30.0));
        assert_eq!(left.y, 0.0);
    }

    #[test]
    fn an_exact_fit_stays_on_one_line_and_a_hair_less_stacks() {
        let (size, left, right) = place(block(100.0, 20.0), block(50.0, 20.0), 200.0, 160.0);
        assert_eq!(size.width, 160.0);
        assert_eq!((left.y, right.y, right.x), (0.0, 0.0, 110.0));
        let (_, _, right) = place(block(100.0, 20.0), block(50.0, 20.0), 200.0, 159.0);
        assert_eq!((right.x, right.y), (0.0, 30.0));
    }

    #[test]
    fn stacking_adds_the_spacing_to_the_height() {
        let (size, left, right) = place(block(300.0, 20.0), block(50.0, 30.0), 400.0, 200.0);
        // The block is clamped to the room, and the right side sits below.
        assert_eq!(left.width, 200.0);
        assert_eq!((right.x, right.y), (0.0, 30.0));
        assert_eq!(size.height, 20.0 + 10.0 + 30.0);
    }

    #[test]
    fn a_wide_left_side_shrinks_beside_the_right_when_enough_remains() {
        let (_, left, right) = place(block(600.0, 20.0), block(50.0, 20.0), 100.0, 400.0);
        assert_eq!(left.width, 340.0);
        assert_eq!((right.x, right.y), (350.0, 0.0));
        let (_, left, right) = place(block(600.0, 20.0), block(50.0, 20.0), 350.0, 400.0);
        assert_eq!(left.width, 400.0);
        assert_eq!(right.y, 30.0);
    }
}
