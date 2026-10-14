//! Reloading the library and merging what a reload or rescan found.

use iced::Task;

use crate::library::{Card, load_library};
use crate::{App, Message, rescan, sidebar, task, trash_page};

impl App {
    pub(crate) fn reload_library(&mut self) -> Task<Message> {
        self.library_generation = self.library_generation.wrapping_add(1);
        self.shown.reloading_library();
        let generation = self.library_generation;
        let paths = self.paths.clone();
        task::blocking(
            move || load_library(&paths).map_err(|e| e.to_string()),
            move |result| Message::LibraryReloaded(generation, result),
        )
    }

    /// A library reload unless one is already in flight.
    pub(crate) fn reload_library_once(&mut self) -> Task<Message> {
        if self.refreshing {
            Task::none()
        } else {
            self.refreshing = true;
            self.reload_library()
        }
    }

    pub(crate) fn reload_badge(&mut self) -> Task<Message> {
        self.trash_generation = self.trash_generation.wrapping_add(1);
        self.shown.reloading_trash();
        let generation = self.trash_generation;
        Self::load_trash_badge(self.paths.clone(), move |badge| {
            Message::TrashReloaded(generation, badge)
        })
    }

    /// Applies what a periodic rescan found changed. An explicit reload in
    /// flight wins, and the next rescan compares against its result.
    pub(crate) fn rescanned(&mut self, scan: rescan::Scan) -> Task<Message> {
        let scan = self.shown.current(scan);
        // Loaded already reloads the open Art page.
        let art = match (&scan.library, &scan.art) {
            (None, Some(art)) if self.art_page.as_ref() == Some(&art.name) => {
                let cards = self.cards.clone();
                self.refresh_art(&cards)
            }
            _ => Task::none(),
        };
        let library = match scan.library {
            Some(result) if !self.refreshing => self.update(Message::Loaded(result)),
            _ => Task::none(),
        };
        let trash = match scan.trash {
            _ if self.trash_refreshing => Task::none(),
            Some(rescan::Trash::Page(snapshot)) if self.section == sidebar::Section::Trash => {
                trash_page::update(
                    self,
                    trash_page::Event::Loaded(self.trash_generation, snapshot),
                )
            }
            Some(rescan::Trash::Badge(badge)) if self.section != sidebar::Section::Trash => {
                self.update(Message::TrashBadgeLoaded(badge))
            }
            _ => Task::none(),
        };
        Task::batch([library, trash, art])
    }

    pub(crate) fn merge(&mut self, cards: Vec<Card>) {
        let needle = self.search.to_lowercase();
        let old_layout: Vec<_> = self
            .filtered()
            .iter()
            .map(|card| (&card.name, &card.display_title))
            .collect();
        let new_layout: Vec<_> = cards
            .iter()
            .filter(|card| {
                card.display_title.to_lowercase().contains(&needle)
                    || card.name.to_lowercase().contains(&needle)
            })
            .map(|card| (&card.name, &card.display_title))
            .collect();
        let layout_changed = old_layout != new_layout;
        if self
            .selected
            .as_ref()
            .is_some_and(|name| !cards.iter().any(|card| &card.name == name))
        {
            self.selected = None;
            self.close_panel();
        }
        if let Some(card) = self
            .selected
            .as_ref()
            .and_then(|name| cards.iter().find(|card| &card.name == name))
            && self
                .target
                .as_ref()
                .is_some_and(|target| !card.targets.contains(target))
        {
            self.target = card.default_target.clone();
        }
        self.shown.show_library(Ok(cards.clone()));
        self.cards = cards;
        self.refresh_facts();
        if layout_changed {
            self.grid_motion
                .snap(self.panel_open, self.filtered().len());
            self.grid_scroll_after_settle = None;
            self.collapse.reset();
        }
        self.current_art = self
            .cards
            .iter()
            .flat_map(|card| {
                card.art.iter().chain(
                    card.panel_art
                        .values()
                        .flat_map(|art| art.hero.iter().chain(art.logo.iter())),
                )
            })
            .map(|art| (art.path.clone(), art.identity))
            .collect();
        self.images
            .retain(|(art, _)| self.current_art.get(&art.path) == Some(&art.identity));
        self.loading
            .retain(|((art, _), _)| self.current_art.get(&art.path) == Some(&art.identity));
        self.failed_images
            .retain(|(art, _)| self.current_art.get(&art.path) == Some(&art.identity));
        self.error = None;
    }
}
