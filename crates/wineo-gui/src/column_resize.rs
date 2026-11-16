//! Resizing the list view's columns: the handles on the header's dividers,
//! the drag and double-click reset they send, and saving the width.

use iced::advanced::{
    Clipboard, Layout, Shell, Widget, layout, mouse, renderer, widget::Operation, widget::tree,
};
use iced::{Element, Length, Point, Rectangle, Renderer, Size, Task, Theme};
use wineo_core::library::ListColumn;

use crate::list_view::{Column, bounded, widest};
use crate::sort_control::Field;
use crate::theme::Palette;
use crate::{App, Message};

/// A handle's hit area, centered on the gap between two header buttons.
pub(crate) const HIT_WIDTH: f32 = 8.0;
/// The gap between two header buttons the handle sits over.
const GAP: f32 = crate::list_view::HEADER_GAP;
/// The hover and drag line's place in the gap: its third pixel, the one
/// nearer the column being resized.
pub(crate) const LINE_OFFSET: f32 = 2.0;
/// The header's id, which tests find its place by.
#[cfg(test)]
pub(crate) const HEADER_ID: &str = "library-list-header";

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A handle was pressed with the pointer at this x.
    Press(ListColumn, f32),
    /// The pointer moved to this x during a drag.
    Move(f32),
    Release,
    /// A double-click on a handle, which puts its column back to its default.
    Reset(ListColumn),
}

impl Event {
    /// Input from the header, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        // A drag must always be able to end, or it would never save.
        matches!(self, Self::Press(..) | Self::Reset(_))
    }
}

/// A handle being dragged.
#[derive(Debug, Clone)]
pub(crate) struct Drag {
    pub(crate) column: ListColumn,
    from: f32,
    start_x: f32,
    /// The columns showing when it started, which stay until it ends so
    /// nothing jumps under the pointer.
    pub(crate) held: &'static [Column],
}

impl App {
    /// A drag of `column`'s handle is under way.
    pub(crate) fn column_dragged(&self, column: ListColumn) -> bool {
        self.column_drag
            .as_ref()
            .is_some_and(|drag| drag.column == column)
    }

    /// The widest `column` can go now, never under `from`.
    fn column_widest(&self, column: ListColumn, from: f32) -> f32 {
        widest(
            &self.list_widths,
            column,
            self.list_columns(),
            self.slot_width(),
            from,
        )
    }

    fn column_pressed(&mut self, column: ListColumn, x: f32) -> Task<Message> {
        // A press that arrives with a drag still going ends that one first.
        let ended = self.end_column_drag();
        self.column_drag = Some(Drag {
            column,
            from: self.list_widths.get(column),
            start_x: x,
            held: self.list_columns(),
        });
        ended
    }

    fn column_moved(&mut self, x: f32) {
        let Some(drag) = &self.column_drag else {
            return;
        };
        let column = drag.column;
        // The handle is on the column's left edge, so moving right narrows it.
        let wanted = drag.from - (x - drag.start_x);
        let widest = self.column_widest(column, drag.from);
        self.list_widths.set(column, bounded(wanted, widest));
    }

    /// Ends a drag, saving where it let go. Closing or hiding the window,
    /// switching layouts, and anything covering the Library end one too.
    pub(crate) fn end_column_drag(&mut self) -> Task<Message> {
        let Some(drag) = self.column_drag.take() else {
            return Task::none();
        };
        if self.list_widths.get(drag.column) == drag.from {
            return Task::none();
        }
        self.save_field(Field::Column(
            drag.column,
            self.list_widths.saved_width(drag.column),
        ))
    }

    /// Puts `column` back to its default width, or as near as Title's
    /// minimum allows when that's wider than it is now.
    fn reset_column(&mut self, column: ListColumn) -> Task<Message> {
        let ended = self.end_column_drag();
        let now = self.list_widths.get(column);
        let default = f32::from(column.default_width());
        let width = bounded(default, self.column_widest(column, now));
        if width == now {
            return ended;
        }
        self.list_widths.set(column, width);
        let save = self.save_field(Field::Column(column, self.list_widths.saved_width(column)));
        Task::batch([ended, save])
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::Press(column, x) => app.column_pressed(column, x),
        Event::Move(x) => {
            app.column_moved(x);
            Task::none()
        }
        Event::Release => app.end_column_drag(),
        Event::Reset(column) => app.reset_column(column),
    }
}

/// The header's cells with a handle on the left edge of each column but
/// Title, as one widget: its layout is the cells' own, so the header's
/// layout paths stay as they were. The handles win over the sort buttons
/// under them, which see no pointer while one is hovered or dragged.
pub(crate) struct Handles<'a> {
    content: Element<'a, Message>,
    /// The column each divider resizes, left to right: State's, then each
    /// column's after it.
    columns: Vec<ListColumn>,
    dragging: Option<ListColumn>,
    palette: Palette,
}

impl<'a> Handles<'a> {
    pub(crate) fn new(
        content: Element<'a, Message>,
        columns: Vec<ListColumn>,
        dragging: Option<ListColumn>,
        palette: Palette,
    ) -> Self {
        Self {
            content,
            columns,
            dragging,
            palette,
        }
    }
}

impl<'a> From<Handles<'a>> for Element<'a, Message> {
    fn from(handles: Handles<'a>) -> Self {
        Element::new(handles)
    }
}

#[derive(Default)]
struct State {
    dragging: Option<ListColumn>,
    last_click: Option<mouse::Click>,
}

/// The gap left of each column a handle resizes, as `(column, gap's left
/// edge)`, from the cells laid out in `layout`: the header's inner
/// container, whose one child is the row of cells.
fn gaps(layout: Layout<'_>, columns: &[ListColumn]) -> Vec<(ListColumn, f32)> {
    let Some(cells) = layout.children().next() else {
        return Vec::new();
    };
    cells
        .children()
        .zip(columns)
        .map(|(left, column)| {
            let bounds = left.bounds();
            (*column, bounds.x + bounds.width)
        })
        .collect()
}

/// A handle's hit area for the gap starting at `gap`, the header's height.
pub(crate) fn hit_area(bounds: Rectangle, gap: f32) -> Rectangle {
    Rectangle::new(
        Point::new(gap + GAP / 2.0 - HIT_WIDTH / 2.0, bounds.y),
        Size::new(HIT_WIDTH, bounds.height),
    )
}

/// The 1 px line drawn in the gap starting at `gap`.
pub(crate) fn line(bounds: Rectangle, gap: f32) -> Rectangle {
    Rectangle::new(
        Point::new(gap + LINE_OFFSET, bounds.y),
        Size::new(1.0, bounds.height),
    )
}

impl Handles<'_> {
    /// The handle the pointer is on, if any.
    fn hovered(&self, layout: Layout<'_>, cursor: mouse::Cursor) -> Option<(ListColumn, f32)> {
        let position = cursor.position()?;
        let bounds = layout.bounds();
        gaps(layout, &self.columns)
            .into_iter()
            .find(|(_, gap)| hit_area(bounds, *gap).contains(position))
    }

    /// The pointer as the sort buttons see it: gone while a handle has it.
    fn under(&self, state: &State, layout: Layout<'_>, cursor: mouse::Cursor) -> mouse::Cursor {
        if state.dragging.is_some() || self.hovered(layout, cursor).is_some() {
            cursor.levitate()
        } else {
            cursor
        }
    }
}

impl Widget<Message, Theme, Renderer> for Handles<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<tree::Tree> {
        vec![tree::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut tree::Tree) {
        // A rebuild comes after the press's own message, so a drag the app
        // has let go of, as an interruption does, is over here too.
        let state = tree.state.downcast_mut::<State>();
        if self.dragging.is_none() {
            state.dragging = None;
        }
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut tree::Tree,
        renderer: &Renderer,
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
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        #[cfg(test)]
        operation.container(Some(&iced::widget::Id::new(HEADER_ID)), layout.bounds());
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let hovered = self.hovered(layout, cursor);
        let state = tree.state.downcast_mut::<State>();
        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let (Some((column, _)), Some(position)) = (hovered, cursor.position()) {
                    let click = mouse::Click::new(position, mouse::Button::Left, state.last_click);
                    state.last_click = Some(click);
                    if click.kind() == mouse::click::Kind::Double {
                        state.dragging = None;
                        shell.publish(Message::Columns(Event::Reset(column)));
                    } else {
                        state.dragging = Some(column);
                        shell.publish(Message::Columns(Event::Press(column, position.x)));
                    }
                    // Taken here, so the sort button under it and the
                    // Library's press that closes the panel never see it.
                    shell.capture_event();
                    return;
                }
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { position })
                if state.dragging.is_some() =>
            {
                shell.publish(Message::Columns(Event::Move(position.x)));
                shell.capture_event();
                return;
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.dragging.is_some() =>
            {
                state.dragging = None;
                shell.publish(Message::Columns(Event::Release));
                shell.capture_event();
                return;
            }
            _ => {}
        }
        let under = self.under(tree.state.downcast_ref::<State>(), layout, cursor);
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            under,
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
        use iced::advanced::Renderer as _;
        let state = tree.state.downcast_ref::<State>();
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            self.under(state, layout, cursor),
            viewport,
        );
        let bounds = layout.bounds();
        let gaps = gaps(layout, &self.columns);
        let shown = match self.dragging {
            Some(dragged) => gaps
                .iter()
                .find(|(column, _)| *column == dragged)
                .map(|(_, gap)| (*gap, self.palette.primary)),
            None => self
                .hovered(layout, cursor)
                .map(|(_, gap)| (gap, self.palette.muted)),
        };
        if let Some((gap, color)) = shown {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: line(bounds, gap),
                    ..renderer::Quad::default()
                },
                color,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        if state.dragging.is_some() || self.hovered(layout, cursor).is_some() {
            mouse::Interaction::ResizingHorizontally
        } else {
            self.content.as_widget().mouse_interaction(
                &tree.children[0],
                layout,
                cursor,
                viewport,
                renderer,
            )
        }
    }
}

#[cfg(test)]
mod tests;
