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

/// Whether `inner` lies within `outer`, allowing for float rounding.
pub(crate) fn within(inner: Rectangle, outer: Rectangle) -> bool {
    inner.x >= outer.x - 0.01
        && inner.y >= outer.y - 0.01
        && inner.x + inner.width <= outer.x + outer.width + 0.01
        && inner.y + inner.height <= outer.y + outer.height + 0.01
}
