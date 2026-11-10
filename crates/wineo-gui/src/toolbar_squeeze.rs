//! The library toolbar's right part as one row that fits the window: search
//! shrinks first, then controls fold right to left into the More button.
//! The fold is decided at layout from the controls' laid out widths, so a
//! longer Filters label or a bigger interface size folds sooner, and every
//! child keeps its place in the tree whether it shows or not, so a fold
//! never drops focus or typing.

use iced::advanced::widget::{Operation, operation::Focusable};
use iced::advanced::{
    Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget::tree,
};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// A control that can fold into More, in toolbar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Filters,
    Dice,
    Layout,
    Zoom,
    Titles,
}

impl Control {
    pub(crate) const ALL: [Self; 5] = [
        Self::Filters,
        Self::Dice,
        Self::Layout,
        Self::Zoom,
        Self::Titles,
    ];

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// Which controls are folded into More.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Folds(u8);

impl Folds {
    pub(crate) fn contains(self, control: Control) -> bool {
        self.0 & control.bit() != 0
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn with(self, control: Control) -> Self {
        Self(self.0 | control.bit())
    }

    /// The folded controls, in toolbar order.
    pub(crate) fn iter(self) -> impl Iterator<Item = Control> {
        Control::ALL
            .into_iter()
            .filter(move |control| self.contains(*control))
    }
}

/// One child of the row after search, with its laid out width.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Item {
    /// None for a child that never folds.
    pub(crate) control: Option<Control>,
    pub(crate) width: f32,
}

/// How the row fits: what's folded and how wide search is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Squeeze {
    pub(crate) folds: Folds,
    pub(crate) search: f32,
}

/// Search's width range.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Search {
    pub(crate) min: f32,
    pub(crate) max: f32,
}

/// The fold candidates from roomiest to tightest: nothing folded, then
/// each foldable item from the right added in turn.
fn stages(items: &[Item]) -> Vec<Folds> {
    let mut stages = vec![Folds::default()];
    let mut folds = Folds::default();
    for control in items.iter().rev().filter_map(|item| item.control) {
        folds = folds.with(control);
        stages.push(folds);
    }
    stages
}

/// The width the items take beside search with `folds` folded, counting
/// the gap before each item and More's when anything is folded.
fn rest(items: &[Item], folds: Folds, more: f32, spacing: f32) -> f32 {
    let shown: f32 = items
        .iter()
        .filter(|item| !item.control.is_some_and(|control| folds.contains(control)))
        .map(|item| spacing + item.width)
        .sum();
    if folds.is_empty() {
        shown
    } else {
        shown + spacing + more
    }
}

/// How the row fits in `room`. Search shrinks to its minimum before
/// anything folds; then the first stage that fits wins, or the tightest
/// when none does and the right end clips. Search stays at its minimum
/// once anything is folded, so a fold moves only the controls after it.
///
/// Search is its own step here: a stage that collapses it to an icon would
/// go into `stages` as a field of `Squeeze`, tried either before the first
/// fold or after the last one.
pub(crate) fn plan(room: f32, search: Search, items: &[Item], more: f32, spacing: f32) -> Squeeze {
    let stages = stages(items);
    let folds = stages
        .iter()
        .copied()
        .find(|folds| search.min + rest(items, *folds, more, spacing) <= room)
        .unwrap_or_else(|| *stages.last().expect("at least the roomiest stage"));
    let search = if folds.is_empty() {
        (room - rest(items, folds, more, spacing)).clamp(search.min, search.max)
    } else {
        search.min
    };
    Squeeze { folds, search }
}

/// One child after search: what it is, or None for one that's absent, like
/// the zoom controls in the list, which then take no room or gap.
pub(crate) struct Child<'a, Message> {
    pub(crate) element: Element<'a, Message>,
    /// None for a child that never folds.
    pub(crate) control: Option<Control>,
    pub(crate) present: bool,
}

pub(crate) fn child<'a, Message>(
    element: Element<'a, Message>,
    control: Option<Control>,
    present: bool,
) -> Child<'a, Message> {
    Child {
        element,
        control,
        present,
    }
}

/// The row: search, then `children`, then `more`, shown only while
/// something is folded. `on_fold` reports the folds when they change.
pub(crate) fn squeeze<'a, Message: Clone + 'a>(
    search: Element<'a, Message>,
    range: Search,
    children: Vec<Child<'a, Message>>,
    more: Element<'a, Message>,
    spacing: f32,
    on_fold: fn(Folds) -> Message,
) -> Element<'a, Message> {
    let mut elements = vec![search];
    let mut slots = Vec::with_capacity(children.len());
    for child in children {
        elements.push(child.element);
        slots.push((child.control, child.present));
    }
    elements.push(more);
    Element::new(Row {
        elements,
        slots,
        range,
        spacing,
        on_fold,
        shown: Vec::new(),
    })
}

struct Row<'a, Message> {
    /// Search, the children, then More.
    elements: Vec<Element<'a, Message>>,
    slots: Vec<(Option<Control>, bool)>,
    range: Search,
    spacing: f32,
    on_fold: fn(Folds) -> Message,
    /// Whether each element shows, from the last layout.
    shown: Vec<bool>,
}

#[derive(Default)]
struct State {
    folds: Folds,
    /// The folds last reported, which the next redraw reports again when
    /// they changed.
    reported: Option<Folds>,
}

impl<Message: Clone> Row<'_, Message> {
    fn more_index(&self) -> usize {
        self.elements.len() - 1
    }
}

/// Unfocuses whatever it reaches and says whether anything had focus.
#[derive(Default)]
struct Unfocus(bool);

impl Operation for Unfocus {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&iced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if state.is_focused() {
            self.0 = true;
            state.unfocus();
        }
    }
}

/// Focuses the first focusable it reaches.
#[derive(Default)]
struct FocusFirst(bool);

impl Operation for FocusFirst {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&iced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if !self.0 {
            self.0 = true;
            state.focus();
        }
    }
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer> for Row<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<tree::Tree> {
        self.elements.iter().map(tree::Tree::new).collect()
    }

    fn diff(&self, tree: &mut tree::Tree) {
        tree.diff_children(&self.elements);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut tree::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let room = limits.max().width;
        let loose = layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, limits.max().height));
        let more = self.more_index();
        // Every child is laid out at its own width, folded or not, so the
        // fold can be found from what they really take.
        let mut nodes: Vec<layout::Node> = Vec::with_capacity(self.elements.len());
        nodes.push(layout::Node::new(Size::ZERO));
        for (index, element) in self.elements.iter_mut().enumerate().skip(1) {
            nodes.push(
                element
                    .as_widget_mut()
                    .layout(&mut tree.children[index], renderer, &loose),
            );
        }
        let items: Vec<Item> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, (_, present))| *present)
            .map(|(index, (control, _))| Item {
                control: *control,
                width: nodes[index + 1].size().width,
            })
            .collect();
        let squeeze = plan(
            room,
            self.range,
            &items,
            nodes[more].size().width,
            self.spacing,
        );
        tree.state.downcast_mut::<State>().folds = squeeze.folds;
        let fixed = layout::Limits::new(
            Size::new(squeeze.search, 0.0),
            Size::new(squeeze.search, limits.max().height),
        );
        nodes[0] = self.elements[0]
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, &fixed);
        self.shown = std::iter::once(true)
            .chain(self.slots.iter().map(|(control, present)| {
                *present && !control.is_some_and(|control| squeeze.folds.contains(control))
            }))
            .chain(std::iter::once(!squeeze.folds.is_empty()))
            .collect();
        let height = nodes
            .iter()
            .zip(&self.shown)
            .filter(|(_, shown)| **shown)
            .map(|(node, _)| node.size().height)
            .fold(0.0, f32::max);
        let mut x = 0.0;
        let mut first = true;
        for (node, shown) in nodes.iter_mut().zip(&self.shown) {
            let size = node.size();
            if !*shown {
                // Never drawn or reached, so where it sits doesn't matter.
                node.move_to_mut(iced::Point::new(x, (height - size.height) / 2.0));
                continue;
            }
            if !first {
                x += self.spacing;
            }
            first = false;
            node.move_to_mut(iced::Point::new(x, (height - size.height) / 2.0));
            x += size.width;
        }
        let width = limits.resolve(Length::Fill, Length::Shrink, Size::new(x, height));
        layout::Node::with_children(Size::new(width.width, height), nodes)
    }

    fn operate(
        &mut self,
        tree: &mut tree::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.traverse(&mut |operation| {
            for (((element, state), layout), shown) in self
                .elements
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .zip(&self.shown)
            {
                if *shown {
                    element
                        .as_widget_mut()
                        .operate(state, layout, renderer, operation);
                }
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            let folds = tree.state.downcast_ref::<State>().folds;
            // A control that folded with focus hands it to More, which
            // now holds it.
            let mut lost = false;
            for (((element, state), child), shown) in self
                .elements
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .zip(&self.shown)
            {
                if !*shown {
                    let mut unfocus = Unfocus::default();
                    element
                        .as_widget_mut()
                        .operate(state, child, renderer, &mut unfocus);
                    lost |= unfocus.0;
                }
            }
            let more = self.more_index();
            if lost && self.shown[more] {
                let child = layout.children().nth(more).expect("More's layout");
                self.elements[more].as_widget_mut().operate(
                    &mut tree.children[more],
                    child,
                    renderer,
                    &mut FocusFirst::default(),
                );
                shell.request_redraw();
            }
            let state = tree.state.downcast_mut::<State>();
            if state.reported != Some(folds) {
                state.reported = Some(folds);
                shell.publish((self.on_fold)(folds));
            }
        }
        for (((element, state), child), shown) in self
            .elements
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
            .zip(&self.shown)
        {
            if *shown {
                element.as_widget_mut().update(
                    state, event, child, cursor, renderer, clipboard, shell, viewport,
                );
            }
        }
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
        for (((element, state), child), shown) in self
            .elements
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .zip(&self.shown)
        {
            if *shown {
                element
                    .as_widget()
                    .draw(state, renderer, theme, style, child, cursor, viewport);
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.elements
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .zip(&self.shown)
            .filter(|(_, shown)| **shown)
            .map(|(((element, state), child), _)| {
                element
                    .as_widget()
                    .mouse_interaction(state, child, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut tree::Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let children = self
            .elements
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
            .zip(&self.shown)
            .filter(|(_, shown)| **shown)
            .filter_map(|(((element, state), child), _)| {
                element
                    .as_widget_mut()
                    .overlay(state, child, renderer, viewport, translation)
            })
            .collect::<Vec<_>>();
        (!children.is_empty()).then(|| overlay::Group::with_children(children).overlay())
    }
}

#[cfg(test)]
mod tests;
