//! Message handlers for the card grid's scroll position and motion.

use std::ops::ControlFlow;
use std::rc::Rc;
use std::time::{Duration, Instant};

use iced::Task;
use iced::widget::scrollable;

use crate::slot::SlotId;
use crate::{App, Early, Message, SCROLL_ID, grid, grid_motion, sidebar};

impl App {
    pub(crate) fn grid_card_located(
        &mut self,
        slot: SlotId,
        generation: u64,
        bounds: grid_motion::CardBounds,
    ) -> Early {
        // A reply for a copy the cursor has left is stale too.
        if self.selected_slot().as_ref() != Some(&slot)
            || self.panel_hidden()
            || generation != self.grid_revision()
        {
            return ControlFlow::Break(Task::none());
        }
        let top = bounds.card.y - bounds.viewport.y;
        let bottom = top + bounds.card.height;
        // The action bar covers the viewport's bottom while it shows.
        let visible = (bounds.viewport.height - self.bar_room()).max(0.0);
        let offset = if top < bounds.offset.y {
            top
        } else if bottom > bounds.offset.y + visible {
            (bottom - visible).min(top)
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
            self.grid_motion.set_reduced(self.motion.reduced());
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

    /// Whether the cards show their details, from the zoom the grid is
    /// heading to, as the cards' text is.
    pub(crate) fn details_full(&self) -> bool {
        grid_motion::CardText::of(self.poster_only, self.zoom) == grid_motion::CardText::Full
    }

    pub(crate) fn details_progress(&self) -> f32 {
        self.details_fade.progress(self.details_full())
    }

    /// How far below their place the details draw at `progress`. Reduced
    /// motion keeps their fade but not the drift.
    pub(crate) fn details_drift(&self, progress: f32) -> f32 {
        if self.motion.reduced() {
            0.0
        } else {
            crate::details_fade::DetailsFade::drift(progress)
        }
    }

    /// The card whose poster star lights up: the pointer is on it, its card
    /// is hovered, so it takes presses, and nothing covers the grid.
    pub(crate) fn glowing_star(&self) -> Option<&crate::slot::SlotId> {
        self.star_hovered
            .as_ref()
            .filter(|slot| self.hovered.as_ref() == Some(slot))
            .filter(|_| !self.input_covered() && self.collapse.idle())
    }

    /// Retargets the hover fades to the hover and selection the update left,
    /// and drops those of cards no longer shown.
    pub(crate) fn sync_hover_fades(&mut self) {
        // A hover that moved off the star's card by any path, not just the
        // pointer, leaves the star too. The star's own enter can come just
        // before its card's, so it's kept while the hover holds still.
        if self.hovered.as_ref() != self.hover_fades.hovered() && self.star_hovered != self.hovered
        {
            self.star_hovered = None;
        }
        // Whatever covers the grid takes the pointer, and the star's exit
        // under it is never heard, so only a fresh enter lights it again.
        if self.input_covered() || !self.collapse.idle() {
            self.star_hovered = None;
        }
        let glowing = self.glowing_star().cloned();
        // Measured only when the selection's slots need retargeting.
        let shown = self
            .hover_fades
            .selection_moves(self.selected.as_deref())
            .then(|| self.grid_places());
        self.hover_fades.sync(
            self.hovered.as_ref(),
            self.selected.as_deref(),
            glowing.as_ref(),
            shown.as_ref().map_or(&[][..], |places| &places.ids),
        );
        if !self.hover_fades.active() {
            return;
        }
        if let Some(slot) = &self.hovered
            && self.hover_fades.fading(slot)
            && self
                .cards
                .iter()
                .any(|card| card.name == slot.name() && card.favorite)
        {
            self.hover_fades.fill_star(slot);
        }
        let places = self.grid_places();
        self.hover_fades.retain(|slot| places.index(slot).is_some());
    }

    /// Drops the pops of cards no longer shown or no longer favorites.
    pub(crate) fn sync_star_pops(&mut self) {
        if !self.star_pops.active() {
            return;
        }
        let places = self.grid_places();
        let cards = &self.cards;
        self.star_pops.retain(|slot| {
            places.index(slot).is_some()
                && cards
                    .iter()
                    .any(|card| card.name == slot.name() && card.favorite)
        });
    }

    /// Where the shown cards belong at the shown zoom and dock. It's
    /// measured again only when the order or its cards (a rebuilt or kept
    /// order, or new cards), the zoom frame, the column count, the
    /// centering shift, or the cards' text changed, so frames that only
    /// read it allocate nothing.
    pub(crate) fn grid_places(&self) -> Rc<grid_motion::Places> {
        let layout = self.grid_layout();
        let key = self
            .order
            .revision(self.order_inputs())
            .map(|order| grid_motion::PlacesKey {
                order,
                layout,
                columns: self.columns(),
                shift: self.grid_shift(),
                mode: self.layout,
                width: self.slot_width(),
            });
        if let Some(key) = &key
            && let Some((cached, places)) = &*self.places.borrow()
            && cached == key
        {
            return places.clone();
        }
        let sections = self.order.sections(self.order_inputs());
        // The flat library is one section with no header. Group by Recent
        // has an All with one, so the key alone can't say.
        let grouped = self.group != wineo_core::library::GroupBy::None;
        let mut cards: Vec<usize> = Vec::new();
        let mut ids: Vec<SlotId> = Vec::new();
        let mut headers = Vec::new();
        for section in sections.iter() {
            if grouped {
                headers.push(grid_motion::Header {
                    key: section.key.clone(),
                    count: section.cards.len(),
                    collapsed: section.collapsed,
                    at: ids.len(),
                });
            }
            if section.collapsed {
                continue;
            }
            for &index in &section.cards {
                if let Some(card) = self.cards.get(index) {
                    ids.push(SlotId::new(section.key.clone(), card.name.as_str()));
                    cards.push(index);
                }
            }
        }
        let heights: Vec<f32> = match self.layout {
            grid_motion::ViewLayout::Grid => cards
                .iter()
                .map(|&index| grid::card_height(&self.cards[index], layout))
                .collect(),
            grid_motion::ViewLayout::List => vec![crate::list_view::ROW_HEIGHT; cards.len()],
        };
        let mut places = grid_motion::Places::sectioned(
            ids,
            &heights,
            headers,
            self.slot_width(),
            self.columns(),
            self.grid_shift(),
            layout,
            self.layout,
        );
        places.cards = cards;
        let places = Rc::new(places);
        *self.places.borrow_mut() = key.map(|key| (key, places.clone()));
        places
    }

    /// Each slot's card in `places`, which must be this app's
    /// `grid_places`.
    pub(crate) fn slot_cards(&self, places: &grid_motion::Places) -> Vec<&crate::library::Card> {
        places
            .cards
            .iter()
            .filter_map(|&index| self.cards.get(index))
            .collect()
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
            .is_some_and(|slot| places.index(slot).is_none())
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
        self.reflowed()
    }

    /// `resorted` for an order already rebuilt.
    pub(crate) fn reflowed(&mut self) -> Task<Message> {
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
            .selected_index(&places)
            .map(|index| places.slots[index])
        else {
            return Task::none();
        };
        let visible = (self.viewport_height - self.bar_room()).max(0.0);
        let offset = if slot.y < self.scroll_offset {
            slot.y
        } else if slot.y + slot.height > self.scroll_offset + visible {
            (slot.y + slot.height - visible).min(slot.y)
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
