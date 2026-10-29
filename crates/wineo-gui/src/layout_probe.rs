//! Lays out real widget trees with the CPU renderer, so tests can check
//! geometry without opening a window.

use iced::advanced::Layout;
use iced::advanced::layout::{self, Node};
use iced::advanced::renderer::Headless;
use iced::advanced::widget::operation::Scrollable;
use iced::advanced::widget::{Id, Operation, Tree};
use iced::{Element, Font, Pixels, Rectangle, Size, Vector};

fn renderer() -> iced::Renderer {
    crate::fonts::install_once();
    iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        Font::DEFAULT,
        Pixels(14.0),
        Some("tiny-skia"),
    ))
    .expect("tiny-skia renderer")
}

/// Draws `element` laid out in `size` onto a transparent background with
/// the CPU renderer, and returns its RGBA pixels.
pub(crate) fn render<M>(element: &mut Element<'_, M>, size: Size) -> Vec<u8> {
    use iced::advanced::Renderer as _;
    let mut renderer = renderer();
    let mut tree = Tree::new(element.as_widget());
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    let viewport = Rectangle::with_size(size);
    renderer.with_layer(viewport, |renderer| {
        element.as_widget().draw(
            &tree,
            renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: iced::Color::WHITE,
            },
            Layout::new(&node),
            iced::mouse::Cursor::Unavailable,
            &viewport,
        );
    });
    renderer.screenshot(
        iced::Size::new(size.width as u32, size.height as u32),
        1.0,
        iced::Color::TRANSPARENT,
    )
}

pub(crate) fn layout<M>(element: &mut Element<'_, M>, size: Size) -> Node {
    probe(element, size).node
}

/// A laid out tree and the scrollable viewports in it.
pub(crate) struct Probe {
    pub(crate) node: Node,
    /// Each scrollable's viewport and the bounds of the content inside it.
    pub(crate) scrollables: Vec<(Rectangle, Rectangle)>,
}

pub(crate) fn probe<M>(element: &mut Element<'_, M>, size: Size) -> Probe {
    let renderer = renderer();
    let mut tree = Tree::new(element.as_widget());
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    let mut finder = Scrollables(Vec::new());
    element
        .as_widget_mut()
        .operate(&mut tree, Layout::new(&node), &renderer, &mut finder);
    Probe {
        node,
        scrollables: finder.0,
    }
}

/// A laid out tree that keeps its widget state, so operations that scroll it
/// show in what the next operation reads.
pub(crate) struct Laid<'a, M> {
    element: Element<'a, M>,
    tree: Tree,
    pub(crate) node: Node,
    renderer: iced::Renderer,
    size: Size,
}

pub(crate) fn lay_out<'a, M>(mut element: Element<'a, M>, size: Size) -> Laid<'a, M> {
    let renderer = renderer();
    let mut tree = Tree::new(element.as_widget());
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    Laid {
        element,
        tree,
        node,
        renderer,
        size,
    }
}

/// Lays out `element` over `tree` from an earlier layout, diffed the way
/// the runtime does after an update, so widget state carries over.
pub(crate) fn relay_out<'a, M>(element: Element<'a, M>, mut tree: Tree, size: Size) -> Laid<'a, M> {
    let renderer = renderer();
    tree.diff(element.as_widget());
    let mut element = element;
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    Laid {
        element,
        tree,
        node,
        renderer,
        size,
    }
}

impl<M> Laid<'_, M> {
    /// The widget state, for `relay_out` after the view is rebuilt.
    pub(crate) fn into_tree(self) -> Tree {
        self.tree
    }

    /// Runs `operation` and any operation it chains to.
    pub(crate) fn run(&mut self, mut operation: Box<dyn Operation>) {
        loop {
            self.element.as_widget_mut().operate(
                &mut self.tree,
                Layout::new(&self.node),
                &self.renderer,
                operation.as_mut(),
            );
            match operation.finish() {
                iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                _ => break,
            }
        }
    }

    /// The ids of the focused widgets.
    pub(crate) fn focused(&mut self) -> Vec<Id> {
        struct Focused(Vec<Id>);
        impl Operation for Focused {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }

            fn focusable(
                &mut self,
                id: Option<&Id>,
                _bounds: Rectangle,
                state: &mut dyn iced::advanced::widget::operation::Focusable,
            ) {
                if state.is_focused()
                    && let Some(id) = id
                {
                    self.0.push(id.clone());
                }
            }
        }
        let mut focused = Box::new(Focused(Vec::new()));
        self.element.as_widget_mut().operate(
            &mut self.tree,
            Layout::new(&self.node),
            &self.renderer,
            focused.as_mut(),
        );
        focused.0
    }

    /// Hands `event` to the tree, with no cursor, and returns what it
    /// published.
    pub(crate) fn event(&mut self, event: &iced::Event) -> Vec<M> {
        self.event_at(event, iced::mouse::Cursor::Unavailable)
    }

    /// Hands `event` to the tree with the cursor at `cursor`, and returns
    /// what it published.
    pub(crate) fn event_at(&mut self, event: &iced::Event, cursor: iced::mouse::Cursor) -> Vec<M> {
        self.event_with_status(event, cursor).0
    }

    /// Like `event_at`, with whether a widget captured the event.
    pub(crate) fn event_with_status(
        &mut self,
        event: &iced::Event,
        cursor: iced::mouse::Cursor,
    ) -> (Vec<M>, iced::event::Status) {
        let mut messages = Vec::new();
        let mut shell = iced::advanced::Shell::new(&mut messages);
        self.element.as_widget_mut().update(
            &mut self.tree,
            event,
            Layout::new(&self.node),
            cursor,
            &self.renderer,
            &mut iced::advanced::clipboard::Null,
            &mut shell,
            &Rectangle::with_size(self.size),
        );
        let status = shell.event_status();
        (messages, status)
    }

    /// The scroll offsets of every scrollable, top first.
    pub(crate) fn offsets(&mut self) -> Vec<f32> {
        struct Offsets(Vec<f32>);
        impl Operation for Offsets {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }

            fn scrollable(
                &mut self,
                _id: Option<&Id>,
                _bounds: Rectangle,
                _content_bounds: Rectangle,
                translation: Vector,
                _state: &mut dyn Scrollable,
            ) {
                self.0.push(translation.y);
            }
        }
        let mut offsets = Box::new(Offsets(Vec::new()));
        self.element.as_widget_mut().operate(
            &mut self.tree,
            Layout::new(&self.node),
            &self.renderer,
            offsets.as_mut(),
        );
        offsets.0
    }
}

/// Scrollable widgets report their viewport and content bounds only through
/// operations, which is how scrolled content is told apart from overflow.
struct Scrollables(Vec<(Rectangle, Rectangle)>);

impl Operation for Scrollables {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        _id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        _translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        self.0.push((bounds, content_bounds));
    }
}

impl Probe {
    /// Paths of descendants that spill out of their parent or `frame`, or
    /// were squeezed to no width, and scrollable viewports with no room, which
    /// carry an empty path. Only
    /// a scrollable's own content may run past its viewport, so from there
    /// down only the horizontal extent counts against `frame`.
    pub(crate) fn outside(&self, frame: Rectangle) -> Vec<(Vec<usize>, Rectangle)> {
        let mut found = Vec::new();
        for (viewport, _) in &self.scrollables {
            if viewport.width < 1.0 || viewport.height < 1.0 {
                found.push((Vec::new(), *viewport));
            }
        }
        self.walk(
            Layout::new(&self.node),
            &mut Vec::new(),
            frame,
            false,
            &mut found,
        );
        found
    }

    fn scrolled_content(&self, parent: Rectangle, child: Rectangle) -> bool {
        self.scrollables
            .iter()
            .any(|(viewport, content)| same(*viewport, parent) && same(*content, child))
    }

    fn walk(
        &self,
        layout: Layout<'_>,
        path: &mut Vec<usize>,
        frame: Rectangle,
        scrolled: bool,
        found: &mut Vec<(Vec<usize>, Rectangle)>,
    ) {
        let parent = layout.bounds();
        for (index, child) in layout.children().enumerate() {
            let bounds = child.bounds();
            let content = self.scrolled_content(parent, bounds);
            let scrolled = scrolled || content;
            // A row too narrow for its controls squeezes the last to nothing.
            // Empty text is a zero width leaf too, so only containers count.
            let crushed =
                bounds.width < 1.0 && bounds.height >= 1.0 && child.children().next().is_some();
            let in_parent = if content {
                bounds.x >= parent.x - 0.01
                    && bounds.x + bounds.width <= parent.x + parent.width + 0.01
            } else {
                within(bounds, parent)
            };
            let in_frame = if scrolled {
                bounds.x >= frame.x - 0.01
                    && bounds.x + bounds.width <= frame.x + frame.width + 0.01
            } else {
                within(bounds, frame)
            };
            path.push(index);
            if crushed || !in_parent || !in_frame {
                found.push((path.clone(), bounds));
            }
            self.walk(child, path, frame, scrolled, found);
            path.pop();
        }
    }
}

fn same(a: Rectangle, b: Rectangle) -> bool {
    (a.x - b.x).abs() < 0.01
        && (a.y - b.y).abs() < 0.01
        && (a.width - b.width).abs() < 0.01
        && (a.height - b.height).abs() < 0.01
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

/// What pressing the centre of `element`'s widget at `path` publishes, as
/// debug text. A disabled button publishes nothing.
pub(crate) fn click_at<M: std::fmt::Debug>(
    element: Element<'_, M>,
    size: Size,
    path: &[usize],
) -> Vec<String> {
    let mut laid = lay_out(element, size);
    let bounds = bounds_at(&laid.node, path);
    let at = iced::Point::new(
        bounds.x + bounds.width / 2.0,
        bounds.y + bounds.height / 2.0,
    );
    let cursor = iced::mouse::Cursor::Available(at);
    let left = iced::mouse::Button::Left;
    let mut messages = laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(left)),
        cursor,
    );
    messages.extend(laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonReleased(left)),
        cursor,
    ));
    messages
        .iter()
        .map(|message| format!("{message:?}"))
        .collect()
}

/// Asserts a `page_frame` page of Back, a scrolling body, and a pinned strip
/// (the root's first child, in that order) is laid out inside `inset`: Back
/// alone at the top left, the body under it, and the forward action at the
/// left of a strip that ends at the bottom.
pub(crate) fn assert_back_on_top_and_forward_pinned(probe: &Probe, inset: Rectangle, case: &str) {
    assert_eq!(probe.scrollables.len(), 1, "{case}: one scrolling region");
    let (viewport, _) = probe.scrollables[0];
    let back = bounds_at(&probe.node, &[0, 0, 0]);
    let strip = bounds_at(&probe.node, &[0, 2]);
    let forward = bounds_at(&probe.node, &[0, 2, 0]);
    assert!(within(back, inset), "{case}: {back:?}");
    assert!(
        (back.y - inset.y).abs() < 0.01 && (back.x - strip.x).abs() < 0.01,
        "{case}: Back should be at the top left, got {back:?}"
    );
    assert!(
        viewport.y >= back.y + back.height - 0.01,
        "{case}: {viewport:?} should start under {back:?}"
    );
    assert!(
        strip.y >= viewport.y + viewport.height - 0.01,
        "{case}: {strip:?} should sit under {viewport:?}"
    );
    assert!(
        (strip.y + strip.height - (inset.y + inset.height)).abs() < 0.01,
        "{case}: {strip:?} should end at the bottom of {inset:?}"
    );
    assert!(within(forward, strip), "{case}: {forward:?} in {strip:?}");
    assert!(
        forward.x - strip.x < 13.0,
        "{case}: {forward:?} should sit at the left of {strip:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::{Space, column, container, row, scrollable};

    fn frame() -> Rectangle {
        Rectangle::with_size(Size::new(200.0, 200.0))
    }

    #[test]
    fn scrolled_content_may_run_past_its_viewport() {
        let mut view: Element<'_, ()> = scrollable(Space::new().width(100).height(1000))
            .height(200)
            .into();
        let probe = probe(&mut view, frame().size());
        assert_eq!(probe.scrollables.len(), 1);
        assert!(probe.outside(frame()).is_empty());
    }

    #[test]
    fn overflow_outside_a_scrollable_is_reported() {
        // Most layouts clamp children to their limits, but a wrapping row
        // grows past them, as the import action rows did in a short dialog.
        let mut view: Element<'_, ()> = column![
            container(
                row![
                    Space::new().width(150).height(60),
                    Space::new().width(150).height(60)
                ]
                .wrap()
            )
            .height(80),
            Space::new().width(50).height(50),
        ]
        .into();
        let probe = probe(&mut view, frame().size());
        assert!(!probe.outside(frame()).is_empty());
    }

    #[test]
    fn a_collapsed_viewport_is_reported() {
        let mut view: Element<'_, ()> = column![
            Space::new().width(50).height(200),
            scrollable(Space::new().width(50).height(400)),
        ]
        .into();
        let probe = probe(&mut view, frame().size());
        assert_eq!(probe.scrollables.len(), 1);
        assert!(
            probe
                .outside(frame())
                .iter()
                .any(|(_, bounds)| bounds.height < 1.0)
        );
    }
}
