//! Message handlers for the card grid's scroll position and motion.

use std::ops::ControlFlow;
use std::rc::Rc;
use std::time::{Duration, Instant};

use iced::Task;
use iced::widget::scrollable;

use crate::{App, Early, Message, SCROLL_ID, grid, grid_motion, sidebar};

impl App {
    pub(crate) fn grid_card_located(
        &mut self,
        name: String,
        generation: u64,
        bounds: grid_motion::CardBounds,
    ) -> Early {
        if self.selected.as_deref() != Some(&name)
            || self.panel_hidden()
            || generation != self.grid_revision()
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
        if self.section == sidebar::Section::Library && self.grid_motion.active() {
            self.grid_motion.tick(generation, now);
            let hold = self.hold_anchor();
            if !self.grid_motion.active() {
                self.grid_motion.anchor = None;
                // Drops the protection the moving cards' art needed.
                let art = self.load_visible_art();
                return ControlFlow::Break(Task::batch([hold, art, self.motion_ended(true)]));
            }
            return ControlFlow::Continue(hold);
        }
        ControlFlow::Continue(Task::none())
    }

    /// Where the shown cards belong at the shown zoom and dock. It's
    /// measured again only when the order or its cards (a rebuilt or kept
    /// order, or new cards), the zoom frame, the column count, or the
    /// cards' text changed, so frames that only read it allocate
    /// nothing.
    pub(crate) fn grid_places(&self) -> Rc<grid_motion::Places> {
        let layout = self.grid_layout();
        let key = self
            .order
            .revision(self.order_inputs())
            .map(|order| grid_motion::PlacesKey {
                order,
                layout,
                columns: self.columns(),
            });
        if let Some(key) = &key
            && let Some((cached, places)) = &*self.places.borrow()
            && cached == key
        {
            return places.clone();
        }
        let filtered = self.filtered();
        let heights: Vec<f32> = filtered
            .iter()
            .map(|card| grid::card_height(card, layout))
            .collect();
        let names = filtered.iter().map(|card| card.name.clone()).collect();
        let places = Rc::new(grid_motion::Places::new(
            names,
            &heights,
            self.columns(),
            layout,
        ));
        *self.places.borrow_mut() = key.map(|key| (key, places.clone()));
        places
    }

    /// Springs the cards to their slots from where they're drawn, or puts
    /// them there at once while the grid is covered. Returns the scroll a
    /// motion this ended was waiting for.
    pub(crate) fn move_grid(&mut self) -> Task<Message> {
        let was_active = self.grid_motion.active();
        let places = self.grid_places();
        if self.panel_hidden() {
            self.grid_motion.snap_to(&places);
        } else {
            self.grid_motion.spring_to(&places, Instant::now());
        }
        // A card that left takes no input, so it never sends its exit.
        if self
            .hovered
            .as_ref()
            .is_some_and(|name| places.index(name).is_none())
        {
            self.hovered = None;
        }
        // New slots that match where every card is drawn end the motion.
        if was_active && !self.grid_motion.active() {
            self.motion_ended(true)
        } else {
            Task::none()
        }
    }

    /// Puts every card in its slot at once, ending any motion.
    pub(crate) fn snap_grid(&mut self) {
        let places = self.grid_places();
        self.grid_motion.snap_to(&places);
    }

    /// A selection scroll waits only while the grid moves: the dock's
    /// motion or the zoom. When the last of them ends, by whatever path,
    /// it scrolls once if `scroll` and its card is still the shown
    /// selection, and is dropped otherwise. Without `scroll` it's dropped
    /// at once, even while something still moves, since the caller has
    /// put the grid somewhere new.
    pub(crate) fn motion_ended(&mut self, scroll: bool) -> Task<Message> {
        if scroll && (self.grid_motion.active() || self.zooming.moving()) {
            return Task::none();
        }
        let Some(name) = self.grid_scroll_after_settle.take() else {
            return Task::none();
        };
        if scroll
            && !self.panel_hidden()
            && self.selected.as_deref() == Some(&name)
            && self.selected_shown()
        {
            self.scroll_selection()
        } else {
            Task::none()
        }
    }

    pub(crate) fn search_changed(&mut self, search: String) -> Task<Message> {
        self.search = search;
        self.reorder();
        self.reflow_grid();
        self.scroll_to_top()
    }

    /// Applies a new sort or filter. Like a search it moves the cards to
    /// their new slots, but the selection stays in view when it's shown.
    pub(crate) fn resorted(&mut self) -> Task<Message> {
        self.reorder();
        self.reflow_grid();
        if self.selected_shown() {
            Task::batch([self.scroll_to_landing(), self.load_visible_art()])
        } else {
            self.scroll_to_top()
        }
    }

    /// Brings the selection just into view where a new order is moving it,
    /// once, rather than where it's drawn now and again when it lands. A
    /// zoom in progress holds the scroll itself, so then it waits for the
    /// zoom like any selection scroll, without asking where the card is
    /// drawn now.
    pub(crate) fn scroll_to_landing(&mut self) -> Task<Message> {
        if self.zooming.moving() {
            self.grid_scroll_after_settle = self.selected.clone();
            return Task::none();
        }
        let places = self.grid_places();
        let Some(slot) = self
            .selected
            .as_ref()
            .and_then(|name| places.index(name))
            .map(|index| places.slots[index])
        else {
            return Task::none();
        };
        let offset = if slot.y < self.scroll_offset {
            slot.y
        } else if slot.y + slot.height > self.scroll_offset + self.viewport_height {
            slot.y + slot.height - self.viewport_height
        } else {
            return Task::none();
        };
        self.scroll_offset = offset.max(0.0);
        iced::widget::operation::scroll_to(
            iced::widget::Id::new(SCROLL_ID),
            scrollable::AbsoluteOffset {
                x: 0.0,
                y: self.scroll_offset,
            },
        )
    }

    /// Moves every card to its slot in a new order.
    pub(crate) fn reflow_grid(&mut self) {
        self.collapse.reset();
        // The caller scrolls for the new order itself, which a zoom's
        // anchor or its reveal at the end would fight, as a manual scroll
        // would.
        let _ = self.motion_ended(false);
        self.interrupt_zoom();
        let _ = self.move_grid();
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
