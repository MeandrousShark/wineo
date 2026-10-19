//! Message handlers for the card grid's scroll position and motion.

use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use iced::Task;
use iced::widget::scrollable;

use crate::{App, Early, Message, SCROLL_ID, grid_motion, sidebar};

impl App {
    pub(crate) fn grid_card_located(
        &mut self,
        name: String,
        generation: u64,
        bounds: grid_motion::CardBounds,
    ) -> Early {
        if self.selected.as_deref() != Some(&name)
            || self.panel_hidden()
            || generation != self.grid_motion.generation()
        {
            return ControlFlow::Break(Task::none());
        }
        let top = bounds.card.y - bounds.viewport.y;
        let bottom = top + bounds.card.height;
        let offset = if top < bounds.offset.y {
            top
        } else if bottom > bounds.offset.y + bounds.viewport.height {
            bottom - bounds.viewport.height
        } else {
            return ControlFlow::Break(Task::none());
        };
        self.scroll_offset = offset.max(0.0);
        ControlFlow::Continue(iced::widget::operation::scroll_to(
            iced::widget::Id::new(SCROLL_ID),
            scrollable::AbsoluteOffset {
                x: 0.0,
                y: self.scroll_offset,
            },
        ))
    }

    pub(crate) fn grid_tick(&mut self, generation: u64, now: Instant) -> Early {
        if self.section == sidebar::Section::Library {
            let was_active = self.grid_motion.active();
            self.grid_motion.tick(generation, now);
            if was_active && !self.grid_motion.active() {
                // Drops the protection the motion's sweep needed.
                let art = self.load_visible_art();
                if let Some(name) = self.grid_scroll_after_settle.take()
                    && self.selected.as_deref() == Some(&name)
                {
                    return ControlFlow::Break(Task::batch([art, self.scroll_selection()]));
                }
                return ControlFlow::Break(art);
            }
        }
        ControlFlow::Continue(Task::none())
    }

    pub(crate) fn search_changed(&mut self, search: String) -> Task<Message> {
        self.search = search;
        self.reorder();
        self.snap_grid();
        self.scroll_to_top()
    }

    /// Applies a new sort or filter. Like a search it snaps the cards to
    /// their new slots, but the selection stays in view when it's shown.
    pub(crate) fn resorted(&mut self) -> Task<Message> {
        self.reorder();
        self.snap_grid();
        if self.selected_shown() {
            Task::batch([self.scroll_selection(), self.load_visible_art()])
        } else {
            self.scroll_to_top()
        }
    }

    /// Puts every card in its new slot at once, with nothing in motion that
    /// could report a stale position.
    pub(crate) fn snap_grid(&mut self) {
        self.collapse.reset();
        self.grid_motion
            .snap(self.panel_open, self.filtered().len());
        self.grid_scroll_after_settle = None;
    }

    fn scroll_to_top(&mut self) -> Task<Message> {
        self.scroll_offset = 0.0;
        Task::batch([
            iced::widget::operation::scroll_to(
                iced::widget::Id::new(SCROLL_ID),
                scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
            ),
            self.load_visible_art(),
        ])
    }

    pub(crate) fn grid_scrolled(&mut self, viewport: scrollable::Viewport) -> Task<Message> {
        self.viewport_width = viewport.bounds().width;
        self.viewport_height = viewport.bounds().height;
        self.on_scroll_offset(viewport.absolute_offset().y);
        self.scroll_visible_until = Some(Instant::now() + Duration::from_millis(850));
        self.load_visible_art()
    }
}
