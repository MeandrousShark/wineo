//! Lays out real widget trees with the CPU renderer, so tests can check
//! geometry without opening a window.

use iced::advanced::Layout;
use iced::advanced::layout::{self, Node};
use iced::advanced::renderer::Headless;
use iced::advanced::widget::Tree;
use iced::{Element, Font, Pixels, Rectangle, Size};

pub(crate) fn layout<M>(element: &mut Element<'_, M>, size: Size) -> Node {
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        Font::DEFAULT,
        Pixels(14.0),
        Some("tiny-skia"),
    ))
    .expect("tiny-skia renderer");
    let mut tree = Tree::new(element.as_widget());
    element
        .as_widget_mut()
        .layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, size))
}

/// The absolute bounds of `node`'s child at each index of `path` in turn.
pub(crate) fn bounds_at(node: &Node, path: &[usize]) -> Rectangle {
    let mut layout = Layout::new(node);
    for index in path {
        layout = layout
            .children()
            .nth(*index)
            .unwrap_or_else(|| panic!("no child {index} on the path {path:?}"));
    }
    layout.bounds()
}

/// Paths of descendants that spill out of `frame` or were squeezed to no
/// width. Content taller than its parent is scrolled, so from there down only
/// the horizontal extent counts.
pub(crate) fn outside(node: &Node, frame: Rectangle) -> Vec<(Vec<usize>, Rectangle)> {
    fn walk(
        layout: Layout<'_>,
        path: &mut Vec<usize>,
        frame: Rectangle,
        scrolled: bool,
        found: &mut Vec<(Vec<usize>, Rectangle)>,
    ) {
        let parent = layout.bounds();
        for (index, child) in layout.children().enumerate() {
            let bounds = child.bounds();
            let scrolled = scrolled
                || bounds.y < parent.y - 0.01
                || bounds.y + bounds.height > parent.y + parent.height + 0.01;
            // A row too narrow for its controls squeezes the last to nothing.
            // Empty text is a zero width leaf too, so only containers count.
            let crushed =
                bounds.width < 1.0 && bounds.height >= 1.0 && child.children().next().is_some();
            let fits = !crushed
                && if scrolled {
                    bounds.x >= frame.x - 0.01
                        && bounds.x + bounds.width <= frame.x + frame.width + 0.01
                } else {
                    within(bounds, frame)
                };
            path.push(index);
            if !fits {
                found.push((path.clone(), bounds));
            }
            walk(child, path, frame, scrolled, found);
            path.pop();
        }
    }
    let mut found = Vec::new();
    walk(Layout::new(node), &mut Vec::new(), frame, false, &mut found);
    found
}

/// Whether `inner` lies within `outer`, allowing for float rounding.
pub(crate) fn within(inner: Rectangle, outer: Rectangle) -> bool {
    inner.x >= outer.x - 0.01
        && inner.y >= outer.y - 0.01
        && inner.x + inner.width <= outer.x + outer.width + 0.01
        && inner.y + inner.height <= outer.y + outer.height + 0.01
}
