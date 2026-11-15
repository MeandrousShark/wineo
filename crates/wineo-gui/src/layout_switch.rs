//! The toolbar's layout segment: a list, a grid, and a coverflow side under
//! one thumb that slides to the chosen one, Ctrl+L to cycle through them,
//! and saving the choice to `library.toml`.

use std::time::Instant;

use iced::widget::{Space, button, container, row, scrollable, stack};
use iced::{Element, Fill, Padding, Task, border};

use crate::grid_motion::ViewLayout;
use crate::library_menu::icon;
use crate::slot::SlotId;
use crate::sort_control::Field;
use crate::{App, Message, SCROLL_ID, focus_button, ui, zoom_control};

pub(crate) const ID: &str = "library-layout";
pub(crate) const WIDTH: f32 = 96.0;
const SIDE: f32 = WIDTH / 3.0;

const LIST: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M6.5 3.5h7.5M6.5 8h7.5M6.5 12.5h7.5" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round"/><circle cx="2.75" cy="3.5" r="1.1" fill="#000"/><circle cx="2.75" cy="8" r="1.1" fill="#000"/><circle cx="2.75" cy="12.5" r="1.1" fill="#000"/></svg>"##;
const COVERFLOW: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect x="5" y="2.5" width="6" height="11" rx="1.2" fill="none" stroke="#000" stroke-width="1.6"/><path d="M2.25 5v6M13.75 5v6" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round"/></svg>"##;
const GRID: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect x="2" y="2" width="5" height="5" rx="1.2" fill="none" stroke="#000" stroke-width="1.6"/><rect x="9" y="2" width="5" height="5" rx="1.2" fill="none" stroke="#000" stroke-width="1.6"/><rect x="2" y="9" width="5" height="5" rx="1.2" fill="none" stroke="#000" stroke-width="1.6"/><rect x="9" y="9" width="5" height="5" rx="1.2" fill="none" stroke="#000" stroke-width="1.6"/></svg>"##;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A click on one side.
    Set(ViewLayout),
    /// Ctrl+L: the next side, and the first again after the last.
    Toggle,
    /// Left on the focused segment: the side before, none before the first.
    Back,
    /// Right, Enter, or Space on the focused segment: the side after, none
    /// after the last.
    Forward,
    /// A frame of the thumb's slide.
    Tick(u64, Instant),
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        !matches!(self, Self::Tick(..))
    }
}

/// What pressing a side does, for its tooltip.
pub(crate) fn label(layout: ViewLayout) -> &'static str {
    match layout {
        ViewLayout::List => "Show as list",
        ViewLayout::Grid => "Show as grid",
        ViewLayout::Coverflow => "Show as coverflow",
    }
}

/// The sides left to right.
const ORDER: [ViewLayout; 3] = [ViewLayout::List, ViewLayout::Grid, ViewLayout::Coverflow];

/// Where the thumb rests for `layout`, in sides from the left.
pub(crate) fn stop(layout: ViewLayout) -> f32 {
    ORDER.iter().position(|side| *side == layout).unwrap_or(0) as f32
}

/// The side after `layout`, wrapping to the first: Ctrl+L's way round.
pub(crate) fn next(layout: ViewLayout) -> ViewLayout {
    ORDER[(stop(layout) as usize + 1) % ORDER.len()]
}

/// The side `step` places from `layout`, or `layout` itself at an end.
fn stepped(layout: ViewLayout, step: isize) -> ViewLayout {
    let at = stop(layout) as isize + step;
    usize::try_from(at)
        .ok()
        .and_then(|at| ORDER.get(at))
        .copied()
        .unwrap_or(layout)
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    // The More menu holds the segment while the layout switch is folded.
    if event.is_input() && !(app.library_menu_may_open() || app.more_menu_input()) {
        return Task::none();
    }
    match event {
        Event::Set(layout) => app.set_layout(layout),
        Event::Toggle => app.set_layout(next(app.layout)),
        Event::Back => app.set_layout(stepped(app.layout, -1)),
        Event::Forward => app.set_layout(stepped(app.layout, 1)),
        Event::Tick(generation, now) => {
            app.layout_thumb.tick(generation, now);
            Task::none()
        }
    }
}

impl App {
    /// Shows the library as `layout` and saves it.
    pub(crate) fn set_layout(&mut self, layout: ViewLayout) -> Task<Message> {
        if layout == self.layout {
            return Task::none();
        }
        let switched = self.switch_layout(layout);
        Task::batch([switched, self.save_field(Field::Layout(layout))])
    }

    /// Shows the library as `layout` without saving it, keeping the marks,
    /// the sort, and the selection, with the card that held the place still
    /// on screen.
    pub(crate) fn switch_layout(&mut self, layout: ViewLayout) -> Task<Message> {
        if layout == self.layout {
            return Task::none();
        }
        // Found where the cards are drawn now, before a zoom in flight
        // jumps them to where it was heading.
        let anchor = self.find_anchor();
        self.switch_layout_from(layout, anchor, false)
    }

    /// `switch_layout` keeping the place `find_anchor` gave, for a caller
    /// that also changes the order and so has to find it first. When
    /// `lands_itself`, the caller lands the selection afterward, as a
    /// regroup does, so a held card that's gone from the new order keeps
    /// the scroll where it was instead of going back to the top.
    pub(crate) fn switch_layout_from(
        &mut self,
        layout: ViewLayout,
        (anchor, reveal): (Option<(SlotId, f32)>, bool),
        lands_itself: bool,
    ) -> Task<Message> {
        if layout == self.layout {
            return Task::none();
        }
        // The zoom stays saved for the grid, so one in flight ends where it
        // was heading. Its own anchor and pending scroll would fight the
        // place kept here.
        self.interrupt_zoom();
        self.grid_scroll_after_settle = None;
        let stopped = self.stop_zoom();
        self.collapse.reset();
        let header = self.header_room();
        let from = self.layout;
        self.layout = layout;
        self.viewport_height = (self.viewport_height + header - self.header_room()).max(0.0);
        if self.motion.reduced() {
            self.layout_thumb.snap_to(stop(layout));
        } else {
            self.layout_thumb.slide_to(stop(layout), Instant::now());
        }
        // Posters and rows don't morph into each other, so the new layout
        // shows at once, and the pointer's hover is found again.
        self.hovered = None;
        self.star_hovered = None;
        self.snap_grid();
        let _ = self.motion_ended(false);
        // The strip doesn't scroll: the kept selection goes to its middle,
        // or with none shown, the first stop does.
        if layout == ViewLayout::Coverflow {
            let centered = self.sync_strip();
            // The strip shows at once on its stop, as the other layouts do.
            self.strip.snap();
            let fullscreen = self.fullscreen_after_layout(from);
            return Task::batch([stopped, centered, self.load_visible_art(), fullscreen]);
        }
        let places = self.grid_places();
        let height = places.height + self.bar_room();
        let end = (height - self.viewport_height).max(0.0);
        if let Some((slot, offset)) = anchor
            && let Some(index) = places.index(&slot)
        {
            // A card clipped by the top edge would otherwise keep its clipped
            // offset, and a shorter row would then land above the viewport.
            let offset = offset.max(0.0);
            let top = places.slots[index].y - offset;
            self.scroll_offset = top.clamp(0.0, end);
        } else if lands_itself {
            self.scroll_offset = self.scroll_offset.clamp(0.0, end);
        } else {
            self.scroll_offset = 0.0;
        }
        let scroll = iced::widget::operation::scroll_to(
            iced::widget::Id::new(SCROLL_ID),
            scrollable::AbsoluteOffset {
                x: 0.0,
                y: self.scroll_offset,
            },
        );
        // A selection off screen before stays where the place is kept;
        // one on screen is the anchor itself.
        let landing = if reveal {
            Task::none()
        } else {
            self.scroll_to_landing()
        };
        let fullscreen = self.fullscreen_after_layout(from);
        Task::batch([
            stopped,
            scroll,
            landing,
            self.load_visible_art(),
            fullscreen,
        ])
    }
}

/// The segment for the toolbar, left of the zoom slider.
pub(crate) fn toolbar(app: &App) -> Element<'_, Message> {
    segment(app, ID)
}

/// The segment as focus control `id`, for the toolbar or the More menu.
pub(crate) fn segment<'a>(app: &'a App, id: &'static str) -> Element<'a, Message> {
    let palette = app.palette;
    let track = container(Space::new())
        .width(WIDTH)
        .height(ui::BUTTON_HEIGHT)
        .style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(
                    border::rounded(ui::CONTROL_RADIUS)
                        .width(1)
                        .color(palette.border),
                )
        });
    // The poster-only toggle's selected look, sliding between the sides.
    let thumb = container(
        container(Space::new())
            .width(SIDE)
            .height(ui::BUTTON_HEIGHT)
            .style(move |_| {
                container::Style::default()
                    .background(palette.primary.scale_alpha(0.15))
                    .border(
                        border::rounded(ui::CONTROL_RADIUS)
                            .width(1)
                            .color(palette.primary),
                    )
            }),
    )
    .padding(Padding {
        left: app.layout_thumb.progress().clamp(0.0, 2.0) * SIDE,
        ..Padding::ZERO
    })
    .width(WIDTH)
    .height(ui::BUTTON_HEIGHT);
    let side = |layout: ViewLayout, bytes: &'static [u8]| {
        let chosen = app.layout == layout;
        let ink = if chosen {
            palette.primary
        } else {
            palette.foreground
        };
        let side = button(container(icon(bytes, ink, 16.0)).center(Fill))
            .width(SIDE)
            .height(ui::BUTTON_HEIGHT)
            .padding(0)
            .on_press(Message::Layout(Event::Set(layout)))
            .style(move |_, status| button::Style {
                background: matches!(status, button::Status::Hovered | button::Status::Pressed)
                    .then(|| palette.foreground.scale_alpha(0.08).into()),
                border: border::rounded(ui::CONTROL_RADIUS),
                ..button::Style::default()
            });
        zoom_control::with_tooltip(side.into(), label(layout), palette)
    };
    // One Tab stop and one ring for all the sides: Left and Right step
    // through them, and Enter and Space step forward.
    focus_button::ordered_segment(
        stack![
            track,
            thumb,
            row![
                side(ViewLayout::List, LIST),
                side(ViewLayout::Grid, GRID),
                side(ViewLayout::Coverflow, COVERFLOW),
            ],
        ]
        .width(WIDTH)
        .height(ui::BUTTON_HEIGHT)
        .into(),
        id,
        Message::Layout(Event::Back),
        Message::Layout(Event::Forward),
    )
}

#[cfg(test)]
mod tests;
