//! Message handlers for selecting a card and opening its panel.

use std::time::Instant;

use iced::Task;

use crate::grid_motion::Places;
use crate::slot::SlotId;
use crate::{App, Message};

impl App {
    /// Where the selected card is in `places`: the selected game's first
    /// slot.
    pub(crate) fn selected_index(&self, places: &Places) -> Option<usize> {
        places.first(self.selected.as_deref()?)
    }

    /// The selected card's slot, when it's shown.
    pub(crate) fn selected_slot(&self) -> Option<SlotId> {
        let places = self.grid_places();
        self.selected_index(&places)
            .map(|index| places.ids[index].clone())
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
