//! The library toolbar's right part as one row that fits the window: search
//! shrinks first, then controls fold right to left into the More button,
//! and last search collapses to an icon that opens it in a drop-down.
//! The fold is decided at layout from the controls' laid out widths, so a
//! longer Filters label or a bigger interface size folds sooner, and every
//! child keeps its place in the tree whether it shows or not, so a fold
//! never drops focus or typing.

use iced::advanced::widget::{Operation, operation::Focusable};
use iced::advanced::{
    Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget::tree,
};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

use crate::theme::Palette;

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

/// How the row fits: what's folded, how wide the search field is, and
/// whether search is down to its icon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Squeeze {
    pub(crate) folds: Folds,
    pub(crate) search: f32,
    pub(crate) search_icon: bool,
}

/// Search's width range, and its icon's width.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Search {
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) icon: f32,
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
/// anything folds; then the first fold stage that fits wins. Search stays
/// at its minimum once anything is folded, so a fold moves only the
/// controls after it. When even everything folded doesn't fit, search
/// collapses to its icon, and past that the right end clips.
pub(crate) fn plan(room: f32, search: Search, items: &[Item], more: f32, spacing: f32) -> Squeeze {
    let stages = stages(items);
    let fits = stages
        .iter()
        .copied()
        .find(|folds| search.min + rest(items, *folds, more, spacing) <= room);
    match fits {
        Some(folds) if folds.is_empty() => Squeeze {
            folds,
            search: (room - rest(items, folds, more, spacing)).clamp(search.min, search.max),
            search_icon: false,
        },
        Some(folds) => Squeeze {
            folds,
            search: search.min,
            search_icon: false,
        },
        None => Squeeze {
            folds: *stages.last().expect("at least the roomiest stage"),
            search: search.icon,
            search_icon: true,
        },
    }
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

/// Search's two forms: the field, and the icon that stands in for it once
/// the toolbar collapses search. The field is the same widget either way,
/// so its text, caret, and focus carry over; collapsed and `open`, it shows
/// in a drop-down under the icon.
pub(crate) struct SearchParts<'a, Message> {
    pub(crate) field: Element<'a, Message>,
    pub(crate) icon: Element<'a, Message>,
    pub(crate) range: Search,
    pub(crate) open: bool,
    /// Sent when search collapses while the field has focus, which opens
    /// the drop-down so typing goes on there.
    pub(crate) on_collapse_focused: Message,
    /// Sent for a press outside the open drop-down.
    pub(crate) on_close: Message,
    pub(crate) palette: Palette,
}

/// What the toolbar reports from a layout: the folds and whether search is
/// an icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Fit {
    pub(crate) folds: Folds,
    pub(crate) search_icon: bool,
}

/// The row: search, then `children`, then `more`, shown only while
/// something is folded. `on_fit` reports the fit when it changes.
pub(crate) fn squeeze<'a, Message: Clone + 'a>(
    search: SearchParts<'a, Message>,
    children: Vec<Child<'a, Message>>,
    more: Element<'a, Message>,
    spacing: f32,
    on_fit: fn(Fit) -> Message,
) -> Element<'a, Message> {
    let mut elements = vec![search.field, search.icon];
    let mut slots = Vec::with_capacity(children.len());
    for child in children {
        elements.push(child.element);
        slots.push((child.control, child.present));
    }
    elements.push(more);
    Element::new(Row {
        elements,
        slots,
        range: search.range,
        open: search.open,
        on_collapse_focused: search.on_collapse_focused,
        on_close: search.on_close,
        palette: search.palette,
        spacing,
        on_fit,
        shown: Vec::new(),
        dropdown: None,
    })
}

const FIELD: usize = 0;
const ICON: usize = 1;
/// Where the children after search start among the elements.
const FIRST: usize = 2;

/// The drop-down's width and padding, the Sort menu's chrome.
const DROPDOWN_WIDTH: f32 = crate::grid::ACTION_MENU_WIDTH;
const DROPDOWN_PADDING: f32 = crate::grid::ACTION_MENU_PADDING;
/// Its gap under the icon, as a menu sits under its button.
const DROPDOWN_GAP: f32 = 4.0;

struct Row<'a, Message> {
    /// The search field, its icon, the children, then More.
    elements: Vec<Element<'a, Message>>,
    slots: Vec<(Option<Control>, bool)>,
    range: Search,
    open: bool,
    on_collapse_focused: Message,
    on_close: Message,
    palette: Palette,
    spacing: f32,
    on_fit: fn(Fit) -> Message,
    /// Whether each element shows in the row, from the last layout.
    shown: Vec<bool>,
    /// The open drop-down's bounds in the row, from the last layout.
    dropdown: Option<Rectangle>,
}

#[derive(Default)]
struct State {
    fit: Fit,
    /// The fit last reported, which the next redraw reports again when it
    /// changed.
    reported: Option<Fit>,
    /// The field was in the row at the last redraw, which tells a collapse
    /// from the drop-down closing.
    field_in_row: bool,
    /// Which elements showed in the row at the last redraw, which tells
    /// what just came back.
    shown: Vec<bool>,
}

impl<Message: Clone> Row<'_, Message> {
    fn more_index(&self) -> usize {
        self.elements.len() - 1
    }

    /// Whether the element at `index` takes operations: shown in the row,
    /// or the field in its open drop-down.
    fn reached(&self, index: usize) -> bool {
        self.shown.get(index).copied().unwrap_or(false)
            || (index == FIELD && self.dropdown.is_some())
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

/// Says whether anything it reaches has focus.
#[derive(Default)]
struct HasFocus(bool);

impl Operation for HasFocus {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&iced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        self.0 |= state.is_focused();
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
        for (index, element) in self.elements.iter_mut().enumerate().skip(ICON) {
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
                width: nodes[index + FIRST].size().width,
            })
            .collect();
        let range = Search {
            icon: nodes[ICON].size().width,
            ..self.range
        };
        let squeeze = plan(room, range, &items, nodes[more].size().width, self.spacing);
        tree.state.downcast_mut::<State>().fit = Fit {
            folds: squeeze.folds,
            search_icon: squeeze.search_icon,
        };
        let field_width = if squeeze.search_icon {
            DROPDOWN_WIDTH - 2.0 * DROPDOWN_PADDING
        } else {
            squeeze.search
        };
        let fixed = layout::Limits::new(
            Size::new(field_width, 0.0),
            Size::new(field_width, limits.max().height),
        );
        nodes[FIELD] = self.elements[FIELD].as_widget_mut().layout(
            &mut tree.children[FIELD],
            renderer,
            &fixed,
        );
        self.shown = [!squeeze.search_icon, squeeze.search_icon]
            .into_iter()
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
                node.move_to_mut(Point::new(x, (height - size.height) / 2.0));
                continue;
            }
            if !first {
                x += self.spacing;
            }
            first = false;
            node.move_to_mut(Point::new(x, (height - size.height) / 2.0));
            x += size.width;
        }
        // The open drop-down hangs under the icon, left edges aligned, and
        // the field sits in it, so operations find the field where it shows.
        self.dropdown = (squeeze.search_icon && self.open).then(|| {
            let icon = nodes[ICON].bounds();
            let field = nodes[FIELD].size();
            Rectangle::new(
                Point::new(icon.x, icon.y + icon.height + DROPDOWN_GAP),
                Size::new(DROPDOWN_WIDTH, field.height + 2.0 * DROPDOWN_PADDING),
            )
        });
        if let Some(dropdown) = self.dropdown {
            nodes[FIELD].move_to_mut(Point::new(
                dropdown.x + DROPDOWN_PADDING,
                dropdown.y + DROPDOWN_PADDING,
            ));
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
        let reached: Vec<bool> = (0..self.elements.len())
            .map(|index| self.reached(index))
            .collect();
        operation.traverse(&mut |operation| {
            for (((element, state), layout), reached) in self
                .elements
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .zip(&reached)
            {
                if *reached {
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
            let fit = tree.state.downcast_ref::<State>().fit;
            let state = tree.state.downcast_mut::<State>();
            // Reported first, so the app knows search is an icon before it
            // hears the field collapsed with focus.
            if state.reported != Some(fit) {
                state.reported = Some(fit);
                shell.publish((self.on_fit)(fit));
            }
            let was_in_row = std::mem::replace(&mut state.field_in_row, self.shown[FIELD]);
            let before = std::mem::replace(&mut state.shown, self.shown.clone());
            let more = self.more_index();
            let mut lost = false;
            let mut icon_lost = false;
            let mut more_lost = false;
            for (index, (element, state)) in
                self.elements.iter_mut().zip(&mut tree.children).enumerate()
            {
                let child = layout.children().nth(index).expect("a child's layout");
                if self.shown[index] || (index == FIELD && self.dropdown.is_some()) {
                    continue;
                }
                if index == FIELD && was_in_row {
                    // Collapsing while typing keeps the field's focus and
                    // opens the drop-down, so the typing goes on there.
                    let mut focus = HasFocus::default();
                    element
                        .as_widget_mut()
                        .operate(state, child, renderer, &mut focus);
                    if focus.0 {
                        shell.publish(self.on_collapse_focused.clone());
                    }
                    continue;
                }
                if index == FIELD {
                    // The drop-down closed, and the icon takes the focus.
                    element.as_widget_mut().operate(
                        state,
                        child,
                        renderer,
                        &mut Unfocus::default(),
                    );
                    continue;
                }
                let mut unfocus = Unfocus::default();
                element
                    .as_widget_mut()
                    .operate(state, child, renderer, &mut unfocus);
                match index {
                    ICON => icon_lost |= unfocus.0,
                    _ if index == more => more_lost |= unfocus.0,
                    _ => lost |= unfocus.0,
                }
            }
            // Focus follows what it was on: a control that folded hands it
            // to More, which now holds it; More going away hands it to the
            // first control that came back; the icon going away hands it to
            // the field.
            let target = if icon_lost {
                Some(FIELD)
            } else if more_lost {
                (FIRST..more)
                    .find(|index| {
                        self.shown[*index] && !before.get(*index).copied().unwrap_or(false)
                    })
                    .or_else(|| (FIRST..more).rev().find(|index| self.shown[*index]))
            } else if lost && self.shown[more] {
                Some(more)
            } else {
                None
            };
            if let Some(target) = target {
                let child = layout.children().nth(target).expect("a child's layout");
                self.elements[target].as_widget_mut().operate(
                    &mut tree.children[target],
                    child,
                    renderer,
                    &mut FocusFirst::default(),
                );
                shell.request_redraw();
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
        let origin = layout.bounds().position() + translation;
        let dropdown = self.dropdown;
        let on_close = self.on_close.clone();
        let palette = self.palette;
        let shown = self.shown.clone();
        let mut elements = self.elements.iter_mut();
        let mut trees = tree.children.iter_mut();
        let mut children = Vec::new();
        let (Some(field), Some(field_tree)) = (elements.next(), trees.next()) else {
            return None;
        };
        if let Some(dropdown) = dropdown {
            children.push(overlay::Element::new(Box::new(Dropdown {
                field,
                tree: field_tree,
                bounds: dropdown + Vector::new(origin.x, origin.y),
                on_close,
                palette,
            })));
        }
        for (((element, state), child), shown) in elements
            .zip(trees)
            .zip(layout.children().skip(1))
            .zip(shown.into_iter().skip(1))
        {
            if shown
                && let Some(overlay) =
                    element
                        .as_widget_mut()
                        .overlay(state, child, renderer, viewport, translation)
            {
                children.push(overlay);
            }
        }
        (!children.is_empty()).then(|| overlay::Group::with_children(children).overlay())
    }
}

/// The open search drop-down: the field in the Sort menu's chrome.
struct Dropdown<'a, 'b, Message> {
    field: &'b mut Element<'a, Message>,
    tree: &'b mut tree::Tree,
    /// Its bounds in the window, as the row laid it out.
    bounds: Rectangle,
    on_close: Message,
    palette: Palette,
}

impl<Message: Clone> overlay::Overlay<Message, iced::Theme, iced::Renderer>
    for Dropdown<'_, '_, Message>
{
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        let width = self.bounds.width - 2.0 * DROPDOWN_PADDING;
        let field = self
            .field
            .as_widget_mut()
            .layout(
                self.tree,
                renderer,
                &layout::Limits::new(Size::new(width, 0.0), Size::new(width, f32::INFINITY)),
            )
            .move_to(Point::new(DROPDOWN_PADDING, DROPDOWN_PADDING));
        let size = Size::new(
            self.bounds.width,
            field.size().height + 2.0 * DROPDOWN_PADDING,
        );
        let margin = crate::grid::ACTION_MENU_MARGIN;
        let x = self
            .bounds
            .x
            .min(bounds.width - size.width - margin)
            .max(margin);
        layout::Node::with_children(size, vec![field]).move_to(Point::new(x, self.bounds.y))
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let chrome = crate::library_menu::surface_style(self.palette);
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: chrome.border,
                shadow: chrome.shadow,
                snap: false,
            },
            self.palette.surface,
        );
        if let Some(field) = layout.children().next() {
            self.field
                .as_widget()
                .draw(self.tree, renderer, theme, style, field, cursor, &bounds);
        }
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
        if let Event::Mouse(mouse::Event::ButtonPressed(_)) = event
            && !cursor.is_over(bounds)
        {
            // Like a menu's backdrop, the press only closes it. The icon's
            // own press closes it too.
            shell.publish(self.on_close.clone());
            shell.capture_event();
            return;
        }
        if let Some(field) = layout.children().next() {
            self.field.as_widget_mut().update(
                self.tree, event, field, cursor, renderer, clipboard, shell, &bounds,
            );
        }
        // Nothing under the drop-down takes input meant for it.
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
        let Some(field) = layout.children().next() else {
            return mouse::Interaction::default();
        };
        let interaction = self.field.as_widget().mouse_interaction(
            self.tree,
            field,
            cursor,
            &layout.bounds(),
            renderer,
        );
        if interaction == mouse::Interaction::None && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Idle
        } else {
            interaction
        }
    }
}

#[cfg(test)]
mod tests;
