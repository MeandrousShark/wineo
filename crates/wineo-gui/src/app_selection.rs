//! Message handlers for selecting a card and opening its panel.

use std::time::Instant;

use iced::Task;

use crate::grid_motion::Places;
use crate::slot::SlotId;
use crate::{App, Message};

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
    /// Where the cursor is in `places`: the copy of the selected game it's
    /// on, else that game's first.
    pub(crate) fn selected_index(&self, places: &Places) -> Option<usize> {
        cursor(places, self.selected.as_deref(), self.active_slot.as_ref())
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
        self.active_slot = Some(slot);
    }

    /// Forgets a copy of a game that isn't the selected one, however the
    /// selection moved, so coming back to that game later starts on its
    /// first copy.
    pub(crate) fn sync_active_slot(&mut self) {
        if self.active_slot.as_ref().map(SlotId::name) != self.selected.as_deref() {
            self.active_slot = None;
        }
    }

    pub(crate) fn select_card(&mut self, name: String) -> Task<Message> {
        if self.selected.as_deref() == Some(&name) && self.panel_open {
            self.card_click_target = Some((name, self.target.clone()));
            self.close_panel();
            Task::none()
        } else {
            let was_open = self.panel_open;
            self.close_log();
            if self.selected.as_deref() != Some(&name) {
                self.close_checkup();
            }
            self.selected = Some(name.clone());
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
