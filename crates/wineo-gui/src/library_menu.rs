//! What the library toolbar's Sort menu, Filters popup, and dice menu share,
//! along with the action bar's tag popup: one open at a time, opening at
//! their button, and their rows.

use iced::widget::{Space, button, container, mouse_area, svg};
use iced::{Color, Element, Fill, Point, Rectangle, Task, border};

use crate::grid::context_menu_style;
use crate::theme::Palette;
use crate::ui;
use crate::{App, Message, focus_button};

pub(crate) const CHECK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M3.5 8.5l3 3 6-6.5" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
/// The dash in a check box whose choice holds for only some of the games.
const DASH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M4 8h8" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round"/></svg>"##;
pub(crate) const CHEVRON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M4 6l4 4 4-4" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;

/// A request to open a menu while it looks up its button.
#[derive(Debug, Default)]
pub(crate) struct Opening {
    last: u64,
    pub(crate) pending: Option<u64>,
}

impl Opening {
    /// Drops the request in flight, so its reply is ignored.
    pub(crate) fn cancel(&mut self) {
        self.pending = None;
    }

    /// Starts a new request and returns its number.
    pub(crate) fn start(&mut self) -> u64 {
        self.last += 1;
        self.pending = Some(self.last);
        self.last
    }

    /// Whether `request` is the one waiting, which it then stops being.
    pub(crate) fn answer(&mut self, request: u64) -> bool {
        let current = self.pending == Some(request);
        if current {
            self.pending = None;
        }
        current
    }
}

/// Asks for the bounds of the button `id`, answered as `reply(request, bounds)`.
pub(crate) fn request_bounds(
    opening: &mut Opening,
    id: &'static str,
    reply: fn(u64, Option<Rectangle>) -> Message,
) -> Task<Message> {
    let request = opening.start();
    iced::advanced::widget::operate(bounds_of(id)).map(move |bounds| reply(request, bounds))
}

impl App {
    /// Any toolbar menu is open: Sort, Filters, the dice's, More, or the
    /// action bar's tag popup.
    pub(crate) fn library_menu_open(&self) -> bool {
        self.sort_menu
            || self.filter_menu
            || self.random_menu
            || self.more_menu
            || self.search_open
            || self.tag_menu
    }

    /// Any toolbar menu is showing over the Library.
    pub(crate) fn library_menu_shown(&self) -> bool {
        self.sort_menu_shown()
            || self.filter_menu_shown()
            || self.random_menu_shown()
            || self.more_menu_shown()
            || self.search_dropdown_shown()
            || self.tag_menu_shown()
    }

    /// Whether a menu can open: nothing took the window since it was asked
    /// for, which an open's late reply checks on arrival.
    pub(crate) fn library_menu_may_open(&self) -> bool {
        !(self.exiting
            || self.closing
            || self.sort_menu
            || self.filter_menu
            || self.random_menu
            || self.more_menu
            || self.search_open
            || self.tag_menu
            || self.action_menu.is_some()
            || self.hook_stop_confirm.is_some()
            || self.section != crate::sidebar::Section::Library
            || self.panel_hidden())
    }

    /// Closes every menu and drops any open still waiting, so only
    /// one menu is ever open.
    pub(crate) fn close_library_menus(&mut self) {
        self.sort_menu = false;
        self.filter_menu = false;
        self.random_menu = false;
        self.more_menu = false;
        self.search_open = false;
        self.tag_menu = false;
        self.sort_opening.cancel();
        self.filter_opening.cancel();
        self.random_opening.cancel();
        self.more_opening.cancel();
        self.tag_opening.cancel();
        self.action_menu = None;
    }
}

/// The pointer position that puts a menu a small gap under the button's
/// left edge. `menu_position` offsets the point by 4 px each way.
pub(crate) fn anchor_under(button: Rectangle) -> Point {
    Point::new(button.x - 4.0, button.y + button.height)
}

/// Finds the bounds of the focus control with `id`, if it is on screen.
pub(crate) fn bounds_of(
    id: &'static str,
) -> impl iced::advanced::widget::Operation<Option<Rectangle>> {
    struct Bounds {
        id: iced::widget::Id,
        found: Option<Rectangle>,
    }

    impl iced::advanced::widget::Operation<Option<Rectangle>> for Bounds {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<Option<Rectangle>>),
        ) {
            operate(self);
        }

        fn focusable(
            &mut self,
            id: Option<&iced::widget::Id>,
            bounds: Rectangle,
            _state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            if id == Some(&self.id) {
                self.found = Some(bounds);
            }
        }

        fn finish(&self) -> iced::advanced::widget::operation::Outcome<Option<Rectangle>> {
            iced::advanced::widget::operation::Outcome::Some(self.found)
        }
    }

    Bounds {
        id: iced::widget::Id::new(id),
        found: None,
    }
}

/// Focuses `id`, then scrolls `scroll` so it's in view.
pub(crate) fn focus(id: impl Into<iced::widget::Id>, scroll: &'static str) -> Task<Message> {
    iced::widget::operation::focus(id.into())
        .chain(iced::advanced::widget::operate(reveal_focused(scroll)).discard())
}

/// Moves keyboard focus through `ids` like `focus_button::cycle`, then
/// scrolls the menu's `scroll` so the focused row is in view, since a short
/// window or a long list leaves rows below the visible part.
pub(crate) fn cycle<S: AsRef<str> + 'static>(
    ids: &[S],
    reverse: bool,
    scroll: &'static str,
) -> Task<Message> {
    iced::advanced::widget::operate(focus_button::cycle(ids, reverse))
        .discard()
        .chain(iced::advanced::widget::operate(reveal_focused(scroll)).discard())
}

/// Scrolls the scrollable `scroll` just enough to show the focused control
/// inside it.
pub(crate) fn reveal_focused(scroll: &'static str) -> impl iced::advanced::widget::Operation {
    Reveal {
        id: iced::widget::Id::new(scroll),
        row: None,
        view: None,
        entering: false,
        depth: 0,
        focused: None,
        first: None,
    }
}

/// `reveal_focused` for the container `row` instead of the focused control,
/// for a list whose keyboard highlight isn't focus.
pub(crate) fn reveal_row(
    scroll: &'static str,
    row: iced::widget::Id,
) -> impl iced::advanced::widget::Operation {
    Reveal {
        id: iced::widget::Id::new(scroll),
        row: Some(row),
        view: None,
        entering: false,
        depth: 0,
        focused: None,
        first: None,
    }
}

struct Reveal {
    id: iced::widget::Id,
    /// The container to show, or None for the focused control.
    row: Option<iced::widget::Id>,
    /// The viewport, the content, and the current offset.
    view: Option<(Rectangle, Rectangle, f32)>,
    /// The next traversal is into the target scrollable.
    entering: bool,
    /// How deep inside the target scrollable the traversal is.
    depth: usize,
    /// The focused control inside it.
    focused: Option<Rectangle>,
    /// The top of the first focusable control inside it.
    first: Option<f32>,
}

impl iced::advanced::widget::Operation for Reveal {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation)) {
        // A scrollable reports itself, then traverses its content.
        let entering = std::mem::take(&mut self.entering);
        let inside = entering || self.depth > 0;
        if inside {
            self.depth += 1;
        }
        operate(self);
        if inside {
            self.depth -= 1;
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: iced::Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if self.depth == 0 && id == Some(&self.id) {
            self.view = Some((bounds, content_bounds, translation.y));
            self.entering = true;
        }
    }

    fn container(&mut self, id: Option<&iced::widget::Id>, bounds: Rectangle) {
        let Some(row) = &self.row else {
            return;
        };
        if self.depth == 0 || id.is_none() {
            return;
        }
        if self.first.is_none_or(|first| bounds.y < first) {
            self.first = Some(bounds.y);
        }
        if id == Some(row) {
            self.focused = Some(bounds);
        }
    }

    fn focusable(
        &mut self,
        _id: Option<&iced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if self.depth == 0 || self.row.is_some() {
            return;
        }
        if self.first.is_none_or(|first| bounds.y < first) {
            self.first = Some(bounds.y);
        }
        if state.is_focused() {
            self.focused = Some(bounds);
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<()> {
        use iced::advanced::widget::operation::{Outcome, scrollable};
        let (Some((viewport, content, offset)), Some(row)) = (self.view, self.focused) else {
            return Outcome::None;
        };
        let target = reveal_offset(
            row.y - content.y,
            row.height,
            viewport.height,
            offset,
            self.first.is_some_and(|first| row.y <= first),
        );
        match target {
            Some(target) if (target - offset).abs() > 0.25 => {
                Outcome::Chain(Box::new(scrollable::scroll_to(
                    self.id.clone(),
                    iced::widget::scrollable::AbsoluteOffset {
                        x: None,
                        y: Some(target),
                    },
                )))
            }
            _ => Outcome::None,
        }
    }
}

/// Where to scroll so a row starting `start` into the content and `height`
/// tall shows in a viewport `viewport` tall now scrolled to `offset`. The
/// first row goes back to the top, header and all, when it fits there. A
/// row taller than the viewport shows its top, so repeated reveals agree.
pub(crate) fn reveal_offset(
    start: f32,
    height: f32,
    viewport: f32,
    offset: f32,
    first: bool,
) -> Option<f32> {
    let end = start + height;
    if first && end <= viewport {
        return Some(0.0);
    }
    if height >= viewport {
        return Some(start);
    }
    if start < offset {
        Some(start)
    } else if end > offset + viewport {
        Some(end - viewport)
    } else {
        None
    }
}

pub(crate) fn icon(bytes: &'static [u8], color: Color, size: f32) -> svg::Svg<'static> {
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
}

/// A quiet full-width menu row, tinted when `chosen`.
pub(crate) fn menu_row<'a>(
    content: Element<'a, Message>,
    palette: Palette,
    chosen: bool,
    id: impl Into<iced::widget::Id>,
    message: Message,
) -> Element<'a, Message> {
    menu_row_with(content, palette, chosen, id, message, None)
}

/// A menu row that also sends `exclude` on a right-click or X while it has
/// focus, when there is one.
pub(crate) fn menu_row_with<'a>(
    content: Element<'a, Message>,
    palette: Palette,
    chosen: bool,
    id: impl Into<iced::widget::Id>,
    message: Message,
    exclude: Option<Message>,
) -> Element<'a, Message> {
    let row = button(container(content).height(Fill).center_y(Fill))
        .width(Fill)
        .height(ui::BUTTON_HEIGHT)
        .padding([0, 10])
        .on_press(message.clone())
        .style(move |_, status| {
            let mut style = context_menu_style(palette, false, status);
            if chosen && status == button::Status::Active {
                style.background = Some(palette.primary.scale_alpha(0.12).into());
            }
            style
        });
    let Some(exclude) = exclude else {
        return focus_button::control(row.into(), true, id, message);
    };
    let control = focus_button::excludable(row.into(), id, message, exclude.clone());
    mouse_area(control).on_right_press(exclude).into()
}

/// A box that shows whether a toggle is on. Noto Sans has no check mark, so
/// it's drawn.
pub(crate) fn check_box(on: bool, palette: Palette) -> Element<'static, Message> {
    let mark: Element<'static, Message> = if on {
        icon(CHECK, ui::accent_ink(palette.primary), 12.0).into()
    } else {
        Space::new().into()
    };
    container(mark)
        .width(16)
        .height(16)
        .center(16)
        .style(move |_| {
            let style =
                container::Style::default().border(border::rounded(4).width(1).color(if on {
                    palette.primary
                } else {
                    palette.muted
                }));
            if on {
                style.background(palette.primary)
            } else {
                style
            }
        })
        .into()
}

/// `check_box` for a choice that holds for some of several games but not
/// all: filled like a checked box, with a dash in place of the check.
pub(crate) fn mixed_box(palette: Palette) -> Element<'static, Message> {
    container(icon(DASH, ui::accent_ink(palette.primary), 12.0))
        .width(16)
        .height(16)
        .center(16)
        .style(move |_| {
            container::Style::default()
                .background(palette.primary)
                .border(border::rounded(4).width(1).color(palette.primary))
        })
        .into()
}

/// The menu's surface.
pub(crate) fn surface_style(palette: Palette) -> container::Style {
    container::Style::default()
        .background(palette.surface)
        .border(
            border::rounded(ui::SECTION_RADIUS)
                .width(1)
                .color(palette.border),
        )
        .shadow(iced::Shadow {
            color: Color::BLACK.scale_alpha(0.2),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 14.0,
        })
}

/// A click anywhere outside the menu sends `close`; the menu sits at
/// `position` above it.
pub(crate) fn layers<'a>(
    menu: Element<'a, Message>,
    position: Point,
    close: Message,
) -> [Element<'a, Message>; 2] {
    [
        mouse_area(Space::new().width(Fill).height(Fill))
            .on_press(close.clone())
            .on_right_press(close)
            .into(),
        container(menu)
            .padding(iced::Padding {
                top: position.y,
                left: position.x,
                right: 0.0,
                bottom: 0.0,
            })
            .width(Fill)
            .height(Fill)
            .into(),
    ]
}

#[cfg(test)]
mod tests;
