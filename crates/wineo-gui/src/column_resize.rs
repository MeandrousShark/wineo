//! Resizing the list view's columns: the handles on the header's dividers,
//! the drag and double-click reset they send, and saving the width.

use iced::advanced::{
    Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget::Operation,
    widget::tree,
};
use iced::widget::button;
use iced::{
    Background, Color, Element, Length, Point, Rectangle, Renderer, Size, Task, Theme, Vector,
};
use wineo_core::library::ListColumn;

use crate::list_view::{Column, HEADER_INSET, bounded, header_style, widest};
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
/// How much of the palette's muted color the idle lines keep, so they sit
/// back from the header's labels.
const MUTED_STRENGTH: f32 = 0.5;
/// How much of the header's height each end of a line fades over, from
/// transparent to full strength.
const FADE: f32 = 0.25;
/// The list header's id.
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
    /// The width the drag starts from, as drawn.
    from: f32,
    /// The saved width, which differs from `from` for a State squeezed to
    /// fit, so a drag that never moves saves nothing.
    saved: f32,
    /// State as drawn when it started, which a drag of another column holds.
    pub(crate) state: f32,
    start_x: f32,
    /// The columns showing when it started, which stay until it ends so
    /// nothing jumps under the pointer.
    pub(crate) held: &'static [Column],
    /// A write of this column landed during the drag and its follow-up
    /// waited, so the end writes even at the width it started from.
    owed: bool,
}

impl App {
    /// A drag of `column`'s handle is under way.
    pub(crate) fn column_dragged(&self, column: ListColumn) -> bool {
        self.column_drag
            .as_ref()
            .is_some_and(|drag| drag.column == column)
    }

    /// A write of `column` landed during its drag, so the drag's end writes
    /// whatever it lets go at.
    pub(crate) fn owe_column_write(&mut self, column: ListColumn) {
        if let Some(drag) = self
            .column_drag
            .as_mut()
            .filter(|drag| drag.column == column)
        {
            drag.owed = true;
        }
    }

    /// The widest `column` can go now, never under `from`.
    fn column_widest(&self, column: ListColumn, from: f32) -> f32 {
        let (columns, width) = (self.list_columns(), self.slot_width());
        widest(
            &self.drawn_widths(width, columns),
            column,
            columns,
            width,
            from,
        )
    }

    fn column_pressed(&mut self, column: ListColumn, x: f32) -> Task<Message> {
        // A press that arrives with a drag still going ends that one first.
        let ended = self.end_column_drag();
        let drawn = self.drawn_widths(self.slot_width(), self.list_columns());
        self.column_drag = Some(Drag {
            column,
            from: drawn.get(column),
            saved: self.list_widths.get(column),
            state: drawn.get(ListColumn::State),
            start_x: x,
            held: self.list_columns(),
            owed: false,
        });
        ended
    }

    fn column_moved(&mut self, x: f32) {
        let Some(drag) = &self.column_drag else {
            return;
        };
        let (column, from, saved) = (drag.column, drag.from, drag.saved);
        // The handle is on the column's left edge, so moving right narrows it.
        let wanted = from - (x - drag.start_x);
        let widest = self.column_widest(column, from);
        let width = bounded(wanted, widest);
        // Motion that leaves the drawn width alone keeps the saved one, so a
        // State squeezed to fit isn't saved narrower by a drag that changed
        // nothing visible.
        self.list_widths
            .set(column, if width == from { saved } else { width });
    }

    /// Ends a drag, saving where it let go. Closing or hiding the window,
    /// switching layouts, and anything covering the Library end one too.
    pub(crate) fn end_column_drag(&mut self) -> Task<Message> {
        let Some(drag) = self.column_drag.take() else {
            return Task::none();
        };
        if self.list_widths.get(drag.column) == drag.saved && !drag.owed {
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
    /// A handle took the press, so it takes the release too, a
    /// double-click's included.
    pressed: bool,
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

/// Where the header button of `column` shows its hover look, the cell's
/// button or, for State's plain label, the room a button would take.
pub(crate) fn tint_area(
    layout: Layout<'_>,
    columns: &[ListColumn],
    column: ListColumn,
) -> Option<Rectangle> {
    // The cells start with Title's, which has no handle of its own.
    let index = columns.iter().position(|shown| *shown == column)? + 1;
    let cell = layout.children().next()?.children().nth(index)?;
    let content = cell.children().next()?.bounds();
    let area = if column == ListColumn::State {
        Rectangle::new(
            Point::new(content.x - HEADER_INSET, content.y - 2.0),
            Size::new(content.width + 2.0 * HEADER_INSET, content.height + 4.0),
        )
    } else {
        content
    };
    // Cut to the cell sideways only: State's label has no vertical padding
    // of its own, so a vertical cut would strip back the button's 2 px.
    let cell = cell.bounds();
    let left = area.x.max(cell.x);
    let right = (area.x + area.width).min(cell.x + cell.width);
    (right > left).then(|| {
        Rectangle::new(
            Point::new(left, area.y),
            Size::new(right - left, area.height),
        )
    })
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

/// `color` as a vertical gradient, transparent at both ends and solid
/// between the fades, so a line doesn't end hard at the header's edges.
fn fading(color: Color) -> Background {
    let line = iced::gradient::Linear::new(iced::Radians::PI)
        .add_stop(0.0, color.scale_alpha(0.0))
        .add_stop(FADE, color)
        .add_stop(1.0 - FADE, color)
        .add_stop(1.0, color.scale_alpha(0.0));
    Background::Gradient(iced::Gradient::Linear(line))
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

    /// A drag is going, by the app's word or this widget's own. The app's
    /// counts when the header's state was made anew mid-drag, as when a
    /// row above it in the page comes or goes.
    fn dragging(&self, state: &State) -> bool {
        self.dragging.is_some() || state.dragging.is_some()
    }

    /// The pointer as the sort buttons see it: gone while a handle has it.
    fn under(&self, state: &State, layout: Layout<'_>, cursor: mouse::Cursor) -> mouse::Cursor {
        if self.dragging(state) || self.hovered(layout, cursor).is_some() {
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
            // The app ended a drag this press started without the release
            // reaching here, so that release isn't owed to the handle any
            // more. A double-click's second press starts no drag, so its
            // release stays taken.
            if state.dragging.take().is_some() {
                state.pressed = false;
            }
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
        let dragging = self.dragging(tree.state.downcast_ref::<State>());
        let state = tree.state.downcast_mut::<State>();
        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let (Some((column, _)), Some(position)) = (hovered, cursor.position()) {
                    let click = mouse::Click::new(position, mouse::Button::Left, state.last_click);
                    state.last_click = Some(click);
                    state.pressed = true;
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
                // A press elsewhere means no handle's release is coming.
                state.pressed = false;
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) if dragging => {
                shell.publish(Message::Columns(Event::Move(position.x)));
                shell.capture_event();
                return;
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if dragging || state.pressed =>
            {
                state.dragging = None;
                state.pressed = false;
                if dragging {
                    shell.publish(Message::Columns(Event::Release));
                }
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
        // The app's word on a drag is the one draw honors: it survives a
        // rebuild of the header mid-drag, which makes the widget's own state
        // anew.
        let primary = self.dragging;
        // Under the label, so the tint reads as the button's own hover.
        let resized = primary.or_else(|| self.hovered(layout, cursor).map(|(column, _)| column));
        if let Some(area) = resized.and_then(|column| tint_area(layout, &self.columns, column)) {
            let hover = header_style(self.palette, button::Status::Hovered, false);
            if let Some(background) = hover.background {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: area,
                        border: hover.border,
                        ..renderer::Quad::default()
                    },
                    background,
                );
            }
        }
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
        if primary.is_none() && !cursor.is_over(bounds) {
            return;
        }
        for (column, gap) in gaps(layout, &self.columns) {
            let color = if primary == Some(column) {
                self.palette.primary
            } else {
                self.palette.muted.scale_alpha(MUTED_STRENGTH)
            };
            renderer.fill_quad(
                renderer::Quad {
                    bounds: line(bounds, gap),
                    ..renderer::Quad::default()
                },
                fading(color),
            );
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut tree::Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
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
        if self.dragging(state) || self.hovered(layout, cursor).is_some() {
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
