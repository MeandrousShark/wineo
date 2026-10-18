//! Message handlers for the card grid's scroll position and motion.

use std::ops::ControlFlow;
use std::time::Instant;

use iced::Task;
use iced::widget::scrollable;

use crate::{App, Early, SCROLL_ID, grid_motion, sidebar};

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
}
