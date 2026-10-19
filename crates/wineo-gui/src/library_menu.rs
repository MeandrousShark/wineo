//! What the library toolbar's Sort menu and Filters popup share: one open at
//! a time, opening under their button from the keyboard, and their rows.

use iced::widget::{Space, button, container, mouse_area, svg};
use iced::{Color, Element, Fill, Point, Rectangle, Task, border};

use crate::grid::context_menu_style;
use crate::theme::Palette;
use crate::ui;
use crate::{App, Message, focus_button};

pub(crate) const CHECK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M3.5 8.5l3 3 6-6.5" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
pub(crate) const CHEVRON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M4 6l4 4 4-4" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;

/// A keyboard request to open a menu while it looks up its button.
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
    /// Either toolbar menu is open.
    pub(crate) fn library_menu_open(&self) -> bool {
        self.sort_menu || self.filter_menu
    }

    /// Either toolbar menu is showing over the Library.
    pub(crate) fn library_menu_shown(&self) -> bool {
        self.sort_menu_shown() || self.filter_menu_shown()
    }

    /// Whether a menu can open: nothing took the window since it was asked
    /// for, which a keyboard open's late reply checks on arrival.
    pub(crate) fn library_menu_may_open(&self) -> bool {
        !(self.exiting
            || self.closing
            || self.sort_menu
            || self.filter_menu
            || self.action_menu.is_some()
            || self.hook_stop_confirm.is_some()
            || self.section != crate::sidebar::Section::Library
            || self.panel_hidden())
    }

    /// Closes every menu and drops any keyboard open still waiting, so only
    /// one menu is ever open.
    pub(crate) fn close_library_menus(&mut self) {
        self.sort_menu = false;
        self.filter_menu = false;
        self.sort_opening.cancel();
        self.filter_opening.cancel();
        self.action_menu = None;
    }
}

/// The pointer position that puts a menu a small gap under the button's
/// left edge. `menu_position` offsets a pointer by 4 px each way.
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
    use iced::advanced::widget::operation::{Focusable, Outcome, Scrollable, scrollable};

    struct Reveal {
        id: iced::widget::Id,
        /// The viewport, the content, and the current offset.
        view: Option<(Rectangle, Rectangle, f32)>,
        focused: Option<Rectangle>,
    }

    impl iced::advanced::widget::Operation for Reveal {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }

        fn scrollable(
            &mut self,
            id: Option<&iced::widget::Id>,
            bounds: Rectangle,
            content_bounds: Rectangle,
            translation: iced::Vector,
            _state: &mut dyn Scrollable,
        ) {
            if id == Some(&self.id) {
                self.view = Some((bounds, content_bounds, translation.y));
            }
        }

        fn focusable(
            &mut self,
            _id: Option<&iced::widget::Id>,
            bounds: Rectangle,
            state: &mut dyn Focusable,
        ) {
            if state.is_focused() {
                self.focused = Some(bounds);
            }
        }

        fn finish(&self) -> Outcome<()> {
            let (Some((viewport, content, offset)), Some(row)) = (self.view, self.focused) else {
                return Outcome::None;
            };
            // Focus outside the menu isn't the menu's to reveal.
            let start = row.y - content.y;
            let end = start + row.height;
            if start < 0.0 || end > content.height + 0.5 {
                return Outcome::None;
            }
            let target = if start < offset {
                start
            } else if end > offset + viewport.height {
                end - viewport.height
            } else {
                return Outcome::None;
            };
            Outcome::Chain(Box::new(scrollable::scroll_to(
                self.id.clone(),
                iced::widget::scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(target),
                },
            )))
        }
    }

    Reveal {
        id: iced::widget::Id::new(scroll),
        view: None,
        focused: None,
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
    focus_button::control(row.into(), true, id, message)
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
