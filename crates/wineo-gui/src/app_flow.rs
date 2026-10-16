//! Hosting the Add flow: routing its events and landing in the library when it finishes.

use std::time::Instant;

use iced::Task;

use crate::library::{Card, load_library};
use crate::{App, Message, flow, task};

impl App {
    /// Opens the update page for `name`'s GOG game: an update, or the end
    /// of an interrupted one, or with `options` what the Version picker
    /// chose. Callers decide whether it may start.
    pub(crate) fn open_gog_update(
        &mut self,
        name: String,
        options: Option<wineo_core::gog::update::Options>,
    ) -> Task<Message> {
        if self.settings.is_some() || self.art_page.is_some() || self.flow.is_some() {
            return Task::none();
        }
        self.close_log();
        self.action_menu = None;
        self.flow_generation = self.flow_generation.wrapping_add(1);
        let generation = self.flow_generation;
        // An interrupted Galaxy install is finished by installing the
        // game again, which the update page can't do.
        let galaxy_install = self
            .cards
            .iter()
            .find(|card| card.name == name)
            .filter(|card| options.is_none() && card.finish_update())
            .filter(|_| crate::gog_library::galaxy_install(&self.paths, &name))
            .and_then(|card| {
                Some(flow::Offer {
                    gog_id: card.gog.as_ref()?.gog_id.as_deref()?.parse().ok()?,
                    title: card.display_title.clone(),
                    linux: false,
                    windows: true,
                    unfinished: Some(name.clone()),
                })
            });
        let (page, task) = match (galaxy_install, options) {
            (Some(offer), _) => flow::Page::gog_finish_install(self.paths.clone(), offer),
            (None, Some(options)) => flow::Page::gog_update_with(self.paths.clone(), name, options),
            (None, None) => flow::Page::gog_update(self.paths.clone(), name),
        };
        self.flow = Some(page);
        Task::batch([
            task.map(move |event| Message::Flow(generation, event)),
            iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus()),
        ])
    }

    pub(crate) fn flow_update(&mut self, generation: u64, event: flow::Event) -> Task<Message> {
        if generation != self.flow_generation {
            return Task::none();
        }
        let Some(page) = &mut self.flow else {
            return Task::none();
        };
        let choose_back =
            matches!(page, flow::Page::Choose(_)) && matches!(event, flow::Event::Back);
        let task = page
            .update(event, self.steamgriddb_available)
            .map(move |event| Message::Flow(generation, event));
        // What changed is checked again, so the panel's Update and Version
        // rows say what the game has now.
        let task = match page.take_updated() {
            Some(name) => {
                self.gog_updates.changed(&name);
                Task::batch([task, crate::gog_library::recheck(self, name)])
            }
            None => task,
        };
        let Some(page) = &mut self.flow else {
            return task;
        };
        if self.closing && !page.busy() {
            return task;
        }
        if choose_back {
            self.flow = None;
            return task;
        }
        if let Some(action) = page.take_action() {
            if matches!(action, flow::Action::Choose) {
                self.flow_generation = self.flow_generation.wrapping_add(1);
                self.flow = Some(flow::Page::choose(self.paths.clone()));
                return Task::none();
            }
            self.flow = None;
            self.flow_generation = self.flow_generation.wrapping_add(1);
            if matches!(action, flow::Action::SteamKey) {
                return self.open_steam_key_settings();
            }
            let generation = self.flow_generation;
            let paths = self.paths.clone();
            // An explicit library reload like `reload_library`.
            self.shown.reloading_library();
            return task::blocking(
                move || load_library(&paths).map_err(|e| e.to_string()),
                move |cards| Message::FlowReturned(generation, action.clone(), cards),
            );
        }
        task
    }

    pub(crate) fn flow_returned(
        &mut self,
        action: flow::Action,
        result: Result<Vec<Card>, String>,
    ) -> Task<Message> {
        if self.closing {
            return if self.foreground_busy() {
                Task::none()
            } else {
                iced::exit()
            };
        }
        match result {
            Ok(cards) => self.merge(cards),
            Err(error) => {
                self.shown.show_library(Err(error.clone()));
                self.error = Some(error);
                return Task::none();
            }
        }
        let (name, launch, settings) = match action {
            flow::Action::Choose | flow::Action::SteamKey => return Task::none(),
            flow::Action::Library { name, launch } => (name, launch, false),
            flow::Action::Settings { name, launch } => (Some(name), launch, true),
        };
        let Some(name) = name else {
            return Task::none();
        };
        self.search.clear();
        if self.selected.as_deref() != Some(&name) {
            self.close_checkup();
        }
        self.selected = Some(name.clone());
        let was_open = self.panel_open;
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
        let mut tasks = vec![self.load_visible_art(), self.load_panel_art()];
        if launch {
            tasks.push(self.start_selected());
        }
        if settings {
            tasks.push(self.open_settings());
            if self
                .cards
                .iter()
                .find(|card| card.name == name)
                .is_some_and(|card| card.targets.is_empty())
                && let Some(page) = &mut self.settings
            {
                page.open_add_after_load = true;
            }
        }
        Task::batch(tasks)
    }
}
