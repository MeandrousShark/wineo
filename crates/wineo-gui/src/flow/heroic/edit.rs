//! The Heroic page's details form edits that take more than a field write.

use iced::Task;
use wineo_core::gamescope::{Filter, GamescopeSetting, Scaler, Window};

use super::{Event, Page};

impl Page {
    pub(super) fn add_argument(&mut self) -> Task<Event> {
        if !self.draft.argument_input.trim().is_empty() {
            self.draft
                .arguments
                .push(std::mem::take(&mut self.draft.argument_input));
            self.draft.unparsed_arguments = None;
        }
        self.review = None;
        Task::none()
    }

    pub(super) fn add_environment(&mut self) -> Task<Event> {
        if !self.draft.environment_input.trim().is_empty() {
            self.draft
                .environment
                .push(std::mem::take(&mut self.draft.environment_input));
            self.draft.environment_dirty = true;
        }
        self.review = None;
        Task::none()
    }

    pub(super) fn set_gamescope_enabled(&mut self, value: bool) -> Task<Event> {
        if !value {
            self.draft.gamescope.last_on = self.draft.gamescope.scope();
        }
        self.draft.gamescope.setting = Some(if value {
            GamescopeSetting::On(self.draft.gamescope.scope())
        } else {
            GamescopeSetting::Off
        });
        self.draft.gamescope.dirty = true;
        self.review = None;
        Task::none()
    }

    pub(super) fn set_gamescope_choice(&mut self, index: usize, value: String) -> Task<Event> {
        let mut scope = self.draft.gamescope.scope();
        match index {
            0 => {
                scope.filter = Filter::ALL
                    .into_iter()
                    .find(|choice| choice.as_str() == value)
            }
            1 => {
                scope.scaler = Scaler::ALL
                    .into_iter()
                    .find(|choice| choice.as_str() == value)
            }
            2 => {
                scope.window = Window::ALL
                    .into_iter()
                    .find(|choice| choice.as_str() == value)
            }
            _ => return Task::none(),
        }
        self.draft.gamescope.setting = Some(GamescopeSetting::On(scope));
        self.draft.gamescope.dirty = true;
        self.review = None;
        Task::none()
    }

    pub(super) fn add_gamescope_arg(&mut self) -> Task<Event> {
        if !self.draft.gamescope.args_input.trim().is_empty() {
            self.draft
                .gamescope
                .args
                .push(std::mem::take(&mut self.draft.gamescope.args_input));
            self.draft.gamescope.dirty = true;
        }
        self.review = None;
        Task::none()
    }
}
