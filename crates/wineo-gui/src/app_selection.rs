//! Message handlers for selecting a card and opening its panel.

use std::time::Instant;

use iced::Task;

use crate::grid_motion::Places;
use crate::slot::{SectionKey, SlotId};
use crate::{App, Message};

/// Where the keyboard cursor is besides the selected game, which stays in
/// `App::selected` either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cursor {
    /// Which copy of the selected game it's on, set by clicks and arrow
    /// keys. Gone, or for another game, it's the first copy; see
    /// `App::selected_index`.
    Card(SlotId),
    /// The header it moved to when its section collapsed, which the arrows
    /// then start from (plan 0062, question 22).
    Header(SectionKey),
}

/// The slot in `places` the cursor is on: `active` while it's a shown copy
/// of the `selected` game, else that game's first copy.
pub(crate) fn cursor(
    places: &Places,
    selected: Option<&str>,
    active: Option<&SlotId>,
) -> Option<usize> {
    let selected = selected?;
    active
        .filter(|slot| slot.name() == selected)
        .and_then(|slot| places.index(slot))
        .or_else(|| places.first(selected))
}

impl App {
    /// The copy the cursor is on, unless it's on a header.
    pub(crate) fn card_cursor(&self) -> Option<&SlotId> {
        match &self.cursor {
            Some(Cursor::Card(slot)) => Some(slot),
            _ => None,
        }
    }

    /// The header the cursor is on, if it's on one.
    pub(crate) fn header_cursor(&self) -> Option<&SectionKey> {
        match &self.cursor {
            Some(Cursor::Header(key)) => Some(key),
            _ => None,
        }
    }

    /// Where the cursor is in `places`: the copy of the selected game it's
    /// on, else that game's first.
    pub(crate) fn selected_index(&self, places: &Places) -> Option<usize> {
        cursor(places, self.selected.as_deref(), self.card_cursor())
    }

    /// The slot the cursor is on, when the selected game is shown.
    pub(crate) fn selected_slot(&self) -> Option<SlotId> {
        let places = self.grid_places();
        self.selected_index(&places)
            .map(|index| places.ids[index].clone())
    }

    /// Puts the cursor on `slot`, selecting its game.
    pub(crate) fn set_cursor(&mut self, slot: SlotId) {
        self.selected = Some(slot.name().to_owned());
        self.cursor = Some(Cursor::Card(slot));
    }

    /// Forgets a copy of a game that isn't the selected one, however the
    /// selection moved, so coming back to that game later starts on its
    /// first copy. A copy that stops being drawn, like one in a section
    /// that just collapsed, falls back to the game's first through
    /// `cursor`; collapsing drops the cursor's own. A cursor on a header
    /// goes with its header, or with the selection.
    pub(crate) fn sync_cursor(&mut self) {
        let stale = match &self.cursor {
            None => false,
            Some(Cursor::Card(slot)) => Some(slot.name()) != self.selected.as_deref(),
            // Coverflow keeps a header with nothing selected, since its
            // collapsed card is a place of its own; `sync_strip` selects
            // the section's first game.
            Some(Cursor::Header(key)) => {
                (self.selected.is_none()
                    && self.layout != crate::grid_motion::ViewLayout::Coverflow)
                    || !self
                        .grid_places()
                        .headers
                        .iter()
                        .any(|place| &place.header.key == key)
            }
        };
        if stale {
            self.cursor = None;
        }
    }

    /// Selects the game `name` from outside the grid (a page, a menu, a
    /// flow), off any header, so a collapsed section's card never holds a
    /// selection it doesn't stand for.
    pub(crate) fn select_game(&mut self, name: String) {
        self.leave_header();
        self.selected = Some(name);
    }

    /// Takes the cursor off a header, leaving a card's copy alone.
    pub(crate) fn leave_header(&mut self) {
        if self.header_cursor().is_some() {
            self.cursor = None;
        }
    }

    pub(crate) fn select_card(&mut self, name: String) -> Task<Message> {
        if self.selected.as_deref() == Some(&name) && self.panel_open {
            self.card_click_target = Some((name, self.target.clone()));
            self.close_panel();
            Task::none()
        } else {
            let was_open = self.panel_open;
            self.leave_header();
            self.close_log();
            if self.selected.as_deref() != Some(&name) {
                self.close_checkup();
            }
            self.select_game(name.clone());
            self.panel_open = true;
            if !was_open {
                self.panel_motion.open(Instant::now());
            }
            self.target = self
                .cards
                .iter()
                .find(|card| card.name == name)
                .and_then(|card| card.default_target.clone());
            self.panel_error = None;
            self.panel_note = None;
            self.show_held_error(&name);
            self.load_panel_art()
        }
    }

    pub(crate) fn double_click_card(&mut self, name: String) -> Task<Message> {
        // Fast marking clicks land as double-clicks, which must never launch.
        if self.modifiers.control() || self.modifiers.shift() {
            return Task::none();
        }
        let prior_target = self
            .card_click_target
            .take()
            .and_then(|(clicked, target)| (clicked == name).then_some(target));
        let panel_art = if self.selected.as_deref() != Some(&name) || !self.panel_open {
            self.update(Message::Select(name))
        } else {
            Task::none()
        };
        if let Some(target) = prior_target {
            self.target = target;
        }
        Task::batch([panel_art, self.load_panel_art(), self.start_selected()])
    }
}

#[cfg(test)]
mod tests;
