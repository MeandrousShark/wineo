//! Choice lists whose rows take keyboard focus. Up and Down move it between
//! the rows and stop at the ends, and when the list changes under it, focus
//! follows its row by id.

use iced::advanced::widget::operation::{Focusable, Outcome, focusable};
use iced::advanced::widget::{Operation, tree};
use iced::advanced::{
    Clipboard, Layout, Renderer as AdvancedRenderer, Shell, Widget, layout, mouse, overlay,
    renderer,
};
use iced::keyboard::{self, Key, key::Named};
use iced::widget::Id;
use iced::{Element, Event, Length, Rectangle, Size, Task, Vector};

use super::State;

/// A page's choice list as its update sees it: which list it is, and the ids
/// of the rows that can take focus, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListRows {
    pub(crate) list: &'static str,
    pub(crate) ids: Vec<String>,
}

/// Wraps `content`, which holds one `focus_button::row` for each of `rows`
/// in the same order, with whether each can take focus. `list` names the
/// list, so a page that puts another list in its place doesn't hand it
/// the old one's focus.
pub(crate) fn rows<'a, M: 'a>(
    list: &'static str,
    content: Element<'a, M>,
    rows: Vec<(String, bool)>,
) -> Element<'a, M> {
    Element::new(Rows {
        list,
        content,
        rows: rows
            .into_iter()
            .map(|(id, enabled)| (Id::from(id), enabled))
            .collect(),
    })
}

/// Focus for a list that `before` an update looked one way and `after` it
/// another. A list that opens focuses its first row. One that fills in late
/// focuses it only when nothing else has focus, so it doesn't take the keys
/// from a field being typed in. Either way, and when the list changed under
/// a focused row, the focused row is scrolled into view.
pub(crate) fn land<M: Send + 'static>(
    before: Option<&ListRows>,
    after: Option<&ListRows>,
) -> Option<Task<M>> {
    let ids = after.map(|after| after.ids.clone()).unwrap_or_default();
    match landing(before, after) {
        Landing::Stay => {
            let changed = before.zip(after).is_some_and(|(before, after)| {
                before.list == after.list && before.ids != after.ids
            });
            (changed && !ids.is_empty()).then(|| reveal(ids))
        }
        Landing::Focus(id) => Some(iced::widget::operation::focus(Id::from(id)).chain(reveal(ids))),
        Landing::FocusIfIdle(id) => Some(
            iced::advanced::widget::operate(FocusIfIdle {
                id: Id::from(id),
                busy: false,
            })
            .discard()
            .chain(reveal(ids)),
        ),
    }
}

/// Scrolls the scrollable holding the focused one of the rows `ids` just
/// enough to show it, and the first row back to the top. Nothing happens
/// when no row has focus or it already shows.
pub(crate) fn reveal<M: Send + 'static>(ids: Vec<String>) -> Task<M> {
    iced::advanced::widget::operate(Reveal {
        first: ids.first().map(|id| Id::from(id.clone())),
        ids: ids.into_iter().map(Id::from).collect(),
        seen: 0,
        entering: None,
        open: Vec::new(),
        target: None,
    })
    .discard()
}

/// A scrollable as an operation sees it: its place in the walk, its
/// viewport, its content, and how far it's scrolled.
#[derive(Clone, Copy)]
struct Scroller {
    index: usize,
    viewport: Rectangle,
    content: Rectangle,
    offset: f32,
}

struct Reveal {
    ids: Vec<Id>,
    first: Option<Id>,
    /// Scrollables walked so far.
    seen: usize,
    /// A scrollable whose content the next traversal enters.
    entering: Option<Scroller>,
    /// The scrollables the walk is inside, innermost last.
    open: Vec<Scroller>,
    /// The innermost scrollable around the focused row, and where to scroll it.
    target: Option<(usize, f32)>,
}

impl Operation for Reveal {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        let entered = self.entering.take();
        if let Some(scroller) = entered {
            self.open.push(scroller);
        }
        operate(self);
        if entered.is_some() {
            self.open.pop();
        }
    }

    fn scrollable(
        &mut self,
        _id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        self.entering = Some(Scroller {
            index: self.seen,
            viewport: bounds,
            content: content_bounds,
            offset: translation.y,
        });
        self.seen += 1;
    }

    fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
        let Some(id) = id.filter(|id| state.is_focused() && self.ids.contains(id)) else {
            return;
        };
        let Some(scroller) = self.open.last() else {
            return;
        };
        let target = crate::library_menu::reveal_offset(
            bounds.y - scroller.content.y,
            bounds.height,
            scroller.viewport.height,
            scroller.offset,
            self.first.as_ref() == Some(id),
        )
        .filter(|target| (target - scroller.offset).abs() > 0.25);
        self.target = target.map(|target| (scroller.index, target));
    }

    fn finish(&self) -> Outcome<()> {
        match self.target {
            Some((index, offset)) => Outcome::Chain(Box::new(ScrollTo {
                index,
                offset,
                seen: 0,
            })),
            None => Outcome::None,
        }
    }
}

/// Scrolls the `index`th scrollable of the walk to `offset`. Most of the
/// app's scrollables have no id to find them by.
struct ScrollTo {
    index: usize,
    offset: f32,
    seen: usize,
}

impl Operation for ScrollTo {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        _id: Option<&Id>,
        _bounds: Rectangle,
        _content_bounds: Rectangle,
        _translation: Vector,
        state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if self.seen == self.index {
            state.scroll_to(iced::widget::scrollable::AbsoluteOffset {
                x: None,
                y: Some(self.offset),
            });
        }
        self.seen += 1;
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Landing {
    Stay,
    Focus(String),
    FocusIfIdle(String),
}

fn landing(before: Option<&ListRows>, after: Option<&ListRows>) -> Landing {
    let Some(after) = after else {
        return Landing::Stay;
    };
    let Some(first) = after.ids.first() else {
        return Landing::Stay;
    };
    match before {
        Some(before) if before.list == after.list && before.ids.is_empty() => {
            Landing::FocusIfIdle(first.clone())
        }
        // The same list with rows already shown keeps whatever focus it has.
        Some(before) if before.list == after.list => Landing::Stay,
        _ => Landing::Focus(first.clone()),
    }
}

struct FocusIfIdle {
    id: Id,
    busy: bool,
}

impl Operation for FocusIfIdle {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        self.busy |= state.is_focused();
    }

    fn finish(&self) -> Outcome<()> {
        if self.busy {
            Outcome::None
        } else {
            Outcome::Chain(Box::new(focusable::focus(self.id.clone())))
        }
    }
}

struct Rows<'a, M, R: AdvancedRenderer = iced::Renderer> {
    list: &'static str,
    content: Element<'a, M, iced::Theme, R>,
    rows: Vec<(Id, bool)>,
}

/// The list and its ids at the last diff.
struct Shown {
    list: &'static str,
    ids: Vec<Id>,
}

/// Calls `visit` with each row's focus state in order. A row holds no rows.
fn each_row(tree: &mut tree::Tree, visit: &mut dyn FnMut(&mut State)) {
    if tree.tag == tree::Tag::of::<State>() {
        visit(tree.state.downcast_mut::<State>());
        return;
    }
    for child in &mut tree.children {
        each_row(child, visit);
    }
}

fn focused_row(tree: &mut tree::Tree) -> Option<usize> {
    let mut index = 0;
    let mut found = None;
    each_row(tree, &mut |state| {
        if state.focused && found.is_none() {
            found = Some(index);
        }
        index += 1;
    });
    found
}

fn focus_row(tree: &mut tree::Tree, target: Option<usize>) {
    let mut index = 0;
    each_row(tree, &mut |state| {
        state.focused = target == Some(index);
        index += 1;
    });
}

/// Where focus goes when the list changes while row `focused` of `old` has
/// it: the same row if it's still there, else the row now nearest that
/// place that can take it.
fn follow(old: &[Id], focused: usize, rows: &[(Id, bool)]) -> Option<usize> {
    if let Some(id) = old.get(focused)
        && let Some(index) = rows.iter().position(|(row, _)| row == id)
        && rows[index].1
    {
        return Some(index);
    }
    let start = focused.min(rows.len().checked_sub(1)?);
    (0..rows.len())
        .flat_map(|distance| [start.checked_add(distance), start.checked_sub(distance)])
        .flatten()
        .find(|index| rows.get(*index).is_some_and(|(_, enabled)| *enabled))
}

/// The next row from `from` that can take focus, None at the end.
fn step(from: usize, up: bool, rows: &[(Id, bool)]) -> Option<usize> {
    if up {
        (0..from).rev().find(|index| rows[*index].1)
    } else {
        (from + 1..rows.len()).find(|index| rows[*index].1)
    }
}

impl<M, R> Widget<M, iced::Theme, R> for Rows<'_, M, R>
where
    R: AdvancedRenderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Shown>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Shown {
            list: self.list,
            ids: self.rows.iter().map(|(id, _)| id.clone()).collect(),
        })
    }

    fn children(&self) -> Vec<tree::Tree> {
        vec![tree::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut tree::Tree) {
        let ids: Vec<Id> = self.rows.iter().map(|(id, _)| id.clone()).collect();
        let shown = tree.state.downcast_mut::<Shown>();
        let same_list = shown.list == self.list;
        let old = std::mem::take(&mut shown.ids);
        // Row state is kept by position, so a focused row's flag would stay
        // on whichever row takes its place.
        let focused = if same_list && old == ids {
            None
        } else {
            focused_row(&mut tree.children[0])
        };
        tree.diff_children(std::slice::from_ref(&self.content));
        if let Some(focused) = focused {
            let target = same_list
                .then(|| follow(&old, focused, &self.rows))
                .flatten();
            focus_row(&mut tree.children[0], target);
        }
        *tree.state.downcast_mut::<Shown>() = Shown {
            list: self.list,
            ids,
        };
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
        renderer: &R,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut tree::Tree,
        layout: Layout<'_>,
        renderer: &R,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if let Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(named @ (Named::ArrowUp | Named::ArrowDown)),
            modifiers,
            ..
        }) = event
            && !(modifiers.control() || modifiers.alt() || modifiers.logo())
            && let Some(focused) = focused_row(&mut tree.children[0])
        {
            if let Some(next) = step(focused, *named == Named::ArrowUp, &self.rows) {
                focus_row(&mut tree.children[0], Some(next));
                shell.request_redraw();
            }
            // Kept at the ends too, so the arrow doesn't reach the page
            // behind, like the library grid.
            shell.capture_event();
            return;
        }
        self.content.as_widget_mut().update(
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
        tree: &tree::Tree,
        renderer: &mut R,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
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
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut tree::Tree,
        layout: Layout<'a>,
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, M, iced::Theme, R>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
pub(super) mod tests;
