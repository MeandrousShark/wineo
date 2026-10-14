use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};

use crate::{App, Message, art_page, art_picker, keys::Direction, sidebar};

impl App {
    pub(crate) fn art_busy(&self) -> bool {
        !self.pending_art_writes.is_empty() || self.art_pages.values().any(art_page::Page::busy)
    }

    pub(crate) fn open_art(&mut self, name: String) -> Task<Message> {
        if self.settings.is_some()
            || self.flow.is_some()
            || self.removal.is_some()
            || self.trash_confirm_open()
            || self.downloads_confirm_open()
            || self.hook_stop_confirm.is_some()
            || self.close_prompt
            || self.closing
            || !self.cards.iter().any(|card| card.name == name)
        {
            return Task::none();
        }
        let Some(identity) = art_page::EnvironmentIdentity::read(&self.paths, &name) else {
            self.close_missing_art(&name);
            return Task::none();
        };
        self.tool_page = None;
        self.portable_page = None;
        if self
            .art_pages
            .get(&name)
            .is_some_and(|page| page.identity != identity)
        {
            self.close_missing_art(&name);
        }
        self.leave_art();
        self.close_log();
        self.trash_page.cancel_review();
        self.downloads_page.cancel_review();
        self.action_menu = None;
        self.hovered = None;
        self.search_focused = false;
        self.section = sidebar::Section::Library;
        if self.selected.as_ref() != Some(&name) {
            self.close_checkup();
            self.target = self
                .cards
                .iter()
                .find(|card| card.name == name)
                .and_then(|card| card.default_target.clone());
        }
        self.selected = Some(name.clone());
        if !self.art_pages.contains_key(&name) {
            self.art_generation = self.art_generation.wrapping_add(1);
            self.art_pages.insert(
                name.clone(),
                art_page::Page::with_thumbnail_cache(
                    name.clone(),
                    self.art_generation,
                    identity,
                    self.thumbnails.clone(),
                ),
            );
        }
        let page = self.art_pages.get_mut(&name).unwrap();
        let generation = page.generation;
        let load = if page.busy() {
            Task::none()
        } else {
            page.load(&self.paths, self.decodes.clone())
        };
        self.art_page = Some(name.clone());
        Task::batch([
            load.map(move |event| Message::ArtPage(name.clone(), generation, event)),
            iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus()),
        ])
    }

    pub(crate) fn leave_art(&mut self) {
        if let Some(name) = self.art_page.take() {
            self.shown.forget_art();
            self.art_generation = self.art_generation.wrapping_add(1);
            if let Some(page) = self.art_pages.get_mut(&name) {
                page.leave();
                page.generation = self.art_generation;
            }
        }
    }

    pub(crate) fn refresh_art(&mut self, cards: &[crate::library::Card]) -> Task<Message> {
        let Some(name) = self.art_page.clone() else {
            return Task::none();
        };
        let Some(page) = self.art_pages.get(&name) else {
            self.close_missing_art(&name);
            return Task::none();
        };
        if !cards.iter().any(|card| card.name == name)
            || art_page::EnvironmentIdentity::read(&self.paths, &name) != Some(page.identity)
        {
            self.close_missing_art(&name);
            return Task::none();
        }
        let page = self.art_pages.get_mut(&name).unwrap();
        let generation = page.generation;
        page.load(&self.paths, self.decodes.clone())
            .map(move |event| Message::ArtPage(name.clone(), generation, event))
    }

    fn close_missing_art(&mut self, name: &str) {
        self.art_pages.remove(name);
        if self.art_page.as_deref() == Some(name) {
            self.art_page = None;
            self.shown.forget_art();
        }
        if self.selected.as_deref() == Some(name) {
            self.selected = None;
            self.close_panel();
        }
        self.art_generation = self.art_generation.wrapping_add(1);
    }

    pub(crate) fn art_update(
        &mut self,
        name: String,
        mut generation: u64,
        event: art_page::Event,
    ) -> Task<Message> {
        if (self.management_busy() || self.tool_prefix_busy(&name))
            && matches!(
                event,
                art_page::Event::UsePath
                    | art_page::Event::Remove(_)
                    | art_page::Event::SaveGame
                    | art_page::Event::Picker(
                        _,
                        crate::art_picker::Event::Use | crate::art_picker::Event::UseSelected(_)
                    )
            )
        {
            return Task::none();
        }
        let completion = write_completion(&event);
        let use_sequence = match &event {
            art_page::Event::Used(sequence, ..) | art_page::Event::UseProgress(sequence, ..) => {
                Some(*sequence)
            }
            _ => None,
        };
        let pending_use = use_sequence.and_then(|sequence| {
            self.pending_art_cancels
                .keys()
                .find(|(pending_name, pending_generation, pending_sequence, _)| {
                    pending_name == &name
                        && *pending_generation == generation
                        && *pending_sequence == sequence
                })
                .cloned()
        });
        if let Some(pending) = &pending_use
            && let Some(page) = self.art_pages.get(&name)
            && page.identity == pending.3
            && page.write_sequence == pending.2
        {
            generation = page.generation;
        }
        if completion.is_some()
            && let Some(pending) = &pending_use
        {
            self.pending_art_cancels.remove(pending);
        }
        let Some(page) = self.art_pages.get(&name) else {
            return self.drop_stale_art_event(
                &name,
                pending_use.as_ref().map_or(generation, |pending| pending.1),
                completion,
            );
        };
        if page.generation != generation
            || (self.art_page.as_deref() != Some(&name) && pending_use.is_none())
        {
            return self.drop_stale_art_event(
                &name,
                pending_use.as_ref().map_or(generation, |pending| pending.1),
                completion,
            );
        }
        if art_page::EnvironmentIdentity::read(&self.paths, &name) != Some(page.identity) {
            self.close_missing_art(&name);
            return self.drop_stale_art_event(
                &name,
                pending_use.as_ref().map_or(generation, |pending| pending.1),
                completion,
            );
        }
        match event {
            art_page::Event::Back
                if self.art_page.as_ref() == Some(&name) && !page.has_overlay() =>
            {
                self.leave_art();
                Task::batch([self.load_visible_art(), self.load_panel_art()])
            }
            art_page::Event::Settings if self.art_page.as_ref() == Some(&name) => {
                self.navigate_section(sidebar::Section::Settings)
            }
            art_page::Event::Settings => Task::none(),
            event => {
                if let Some(pending) = &pending_use
                    && completion.is_some()
                {
                    self.pending_art_writes.remove(pending);
                }
                if let Some((sequence, _, _)) = completion {
                    self.pending_art_writes.remove(&(
                        name.clone(),
                        generation,
                        sequence,
                        page.identity,
                    ));
                }
                let decodes = self.decodes.clone();
                let key_available = self.steamgriddb_available;
                let (step, identity, write_sequence, started_write, token) = {
                    let page = self.art_pages.get_mut(&name).unwrap();
                    if let art_page::Event::Loaded(sequence, snapshot) = &event
                        && *sequence == page.load_sequence
                        && self.art_page.as_deref() == Some(&name)
                    {
                        self.shown
                            .show_art(name.clone(), page.identity, snapshot.clone());
                    }
                    let was_writing = page.writing;
                    let step = page.update_with_key_availability(
                        &self.paths,
                        event,
                        decodes,
                        key_available,
                    );
                    (
                        step,
                        page.identity,
                        page.write_sequence,
                        !was_writing && page.writing,
                        page.use_token(),
                    )
                };
                if started_write {
                    if let Some(token) = token {
                        self.pending_art_cancels
                            .insert((name.clone(), generation, write_sequence, identity), token);
                    }
                    self.pending_art_writes.insert((
                        name.clone(),
                        generation,
                        write_sequence,
                        identity,
                    ));
                }
                let task = step
                    .task
                    .map(move |event| Message::ArtPage(name.clone(), generation, event));
                if step.changed {
                    Task::batch([task, self.reload_library()])
                } else {
                    task
                }
            }
        }
    }

    fn drop_stale_art_event(
        &mut self,
        name: &str,
        generation: u64,
        completion: Option<(u64, bool, bool)>,
    ) -> Task<Message> {
        let Some((sequence, changed, is_art_write)) = completion else {
            return Task::none();
        };
        let Some(pending) = self
            .pending_art_writes
            .iter()
            .find(|(pending_name, pending_generation, pending_sequence, _)| {
                pending_name == name
                    && *pending_generation == generation
                    && *pending_sequence == sequence
            })
            .cloned()
        else {
            return Task::none();
        };
        self.pending_art_writes.remove(&pending);
        let page = self
            .art_pages
            .get_mut(name)
            .filter(|page| page.identity == pending.3 && page.write_sequence == sequence);
        let active_reload = if self.art_page.as_deref() == Some(name) {
            let page_name = name.to_owned();
            page.map(|page| {
                page.finish_abandoned_write(sequence);
                let generation = page.generation;
                page.load(&self.paths, self.decodes.clone())
                    .map(move |event| Message::ArtPage(page_name.clone(), generation, event))
            })
        } else {
            if let Some(page) = page {
                page.finish_abandoned_write(sequence);
            }
            None
        };
        let library_reload = if is_art_write && changed {
            self.reload_library()
        } else {
            Task::none()
        };
        Task::batch([active_reload.unwrap_or_else(Task::none), library_reload])
    }

    pub(crate) fn art_key(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        captured: bool,
    ) -> Task<Message> {
        if !captured && modifiers.is_empty() {
            let picker_event = match key.as_ref() {
                Key::Named(Named::ArrowLeft) => Some(art_picker::Event::Move(Direction::Left)),
                Key::Named(Named::ArrowRight) => Some(art_picker::Event::Move(Direction::Right)),
                Key::Named(Named::ArrowUp) => Some(art_picker::Event::Move(Direction::Up)),
                Key::Named(Named::ArrowDown) => Some(art_picker::Event::Move(Direction::Down)),
                Key::Named(Named::Enter) => Some(art_picker::Event::Use),
                _ => None,
            };
            if let Some(name) = self.art_page.clone()
                && let Some(event) = picker_event
            {
                let page = &self.art_pages[&name];
                if let Some(event) = page.picker_event(event) {
                    return self.art_update(name, page.generation, event);
                }
            }
        }
        match key.as_ref() {
            Key::Named(Named::Escape) if !captured => {
                let Some(name) = self.art_page.clone() else {
                    return Task::none();
                };
                let generation = self.art_pages[&name].generation;
                self.art_update(name, generation, art_page::Event::Back)
            }
            Key::Named(Named::Tab) if modifiers.shift() => {
                iced::widget::operation::focus_previous()
            }
            Key::Named(Named::Tab) => iced::widget::operation::focus_next(),
            _ => Task::none(),
        }
    }
}

fn write_completion(event: &art_page::Event) -> Option<(u64, bool, bool)> {
    match event {
        art_page::Event::Wrote(sequence, _, result)
        | art_page::Event::Used(sequence, _, _, result) => {
            Some((*sequence, result.changed(), true))
        }
        art_page::Event::GameSaved(sequence, _, result) => Some((*sequence, result.is_ok(), false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
