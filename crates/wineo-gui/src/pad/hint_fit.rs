//! The hint bar as one widget that keeps as many hints from the start as
//! fit its room, decided at layout from each hint's laid out width, so the
//! Font setting and the interface size count. A hint that doesn't fit goes
//! whole, with every one after it, so the bar never clips or wraps, and
//! with none that fits it draws nothing.

use iced::advanced::widget::{Id, Operation, tree};
use iced::advanced::{Layout, Widget, layout, mouse, renderer};
use iced::{Element, Length, Point, Rectangle, Size};

use super::glyph::HINT_HEIGHT;
use crate::theme::Palette;

/// The bar's id, which tests find it by.
pub(crate) const BAR_ID: &str = "pad-hint-bar";
/// The gap between hints.
const SPACING: f32 = 16.0;
/// The bar's padding on its left and right.
const PADDING: f32 = 12.0;

/// How many hints from the start fit in `room`, with the bar's padding and
/// the gaps between them.
pub(crate) fn fitting(room: f32, widths: &[f32]) -> usize {
    let mut used = 2.0 * PADDING - SPACING;
    widths
        .iter()
        .take_while(|width| {
            used += SPACING + **width;
            used <= room
        })
        .count()
}

/// The bar holding `items`, at most `room` wide.
pub(crate) fn fit<'a, M: 'a>(
    items: Vec<Element<'a, M>>,
    room: f32,
    palette: Palette,
) -> Element<'a, M> {
    Element::new(Fit {
        items,
        room,
        palette,
        shown: 0,
    })
}

struct Fit<'a, M> {
    items: Vec<Element<'a, M>>,
    /// The width the window leaves the bar, which a column can't tell it.
    room: f32,
    palette: Palette,
    /// How many items show, from the last layout.
    shown: usize,
}

impl<M> Widget<M, iced::Theme, iced::Renderer> for Fit<'_, M> {
    fn children(&self) -> Vec<tree::Tree> {
        self.items.iter().map(tree::Tree::new).collect()
    }

    fn diff(&self, tree: &mut tree::Tree) {
        tree.diff_children(&self.items);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut tree::Tree,
        renderer: &iced::Renderer,
        _limits: &layout::Limits,
    ) -> layout::Node {
        let loose = layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, HINT_HEIGHT));
        let mut nodes: Vec<layout::Node> = self
            .items
            .iter_mut()
            .zip(&mut tree.children)
            .map(|(item, state)| item.as_widget_mut().layout(state, renderer, &loose))
            .collect();
        let widths: Vec<f32> = nodes.iter().map(|node| node.size().width).collect();
        self.shown = fitting(self.room, &widths);
        if self.shown == 0 {
            return layout::Node::with_children(Size::ZERO, nodes);
        }
        let mut x = PADDING;
        for node in nodes.iter_mut().take(self.shown) {
            let size = node.size();
            node.move_to_mut(Point::new(x, (HINT_HEIGHT - size.height) / 2.0));
            x += size.width + SPACING;
        }
        let width = x - SPACING + PADDING;
        layout::Node::with_children(Size::new(width, HINT_HEIGHT), nodes)
    }

    fn operate(
        &mut self,
        tree: &mut tree::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.shown == 0 {
            return;
        }
        operation.container(Some(&Id::new(BAR_ID)), layout.bounds());
        let shown = self.shown;
        operation.traverse(&mut |operation| {
            for ((item, state), child) in self
                .items
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .take(shown)
            {
                item.as_widget_mut()
                    .operate(state, child, renderer, operation);
            }
        });
    }

    fn draw(
        &self,
        tree: &tree::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        if self.shown == 0 {
            return;
        }
        let chrome = crate::library_menu::surface_style(self.palette);
        renderer.fill_quad(
            renderer::Quad {
                bounds: layout.bounds(),
                border: chrome.border,
                shadow: chrome.shadow,
                snap: false,
            },
            self.palette.surface,
        );
        for ((item, state), child) in self
            .items
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .take(self.shown)
        {
            item.as_widget()
                .draw(state, renderer, theme, style, child, cursor, viewport);
        }
    }
}
