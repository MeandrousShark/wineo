//! Reloading the library and merging what a reload or rescan found.

use iced::Task;

use crate::library::{Card, load_library};
use crate::{App, Message, order, rescan, sidebar, task, trash_page};

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
        // Explicit reloads don't read the problem, so the rescan's always applies.
        if let Some(problem) = scan.problem {
            self.shown.show_problem(problem.clone());
            self.library_problem = problem;
        }
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

    /// `merge_checked` for tests, which don't run the checks it schedules.
    #[cfg(test)]
    pub(crate) fn merge(&mut self, cards: Vec<Card>) {
        let _ = self.merge_checked(cards);
    }

    /// Shows `cards`, and checks again each GOG game whose own folder now
    /// says something new about it: that is newer than anything asked of GOG
    /// before, so the app forgets it, and the check puts the Update row back.
    pub(crate) fn merge_checked(&mut self, cards: Vec<Card>) -> Task<Message> {
        let changed = crate::gog_library::local_changes(&self.cards, &cards);
        self.merge_cards(cards);
        Task::batch(changed.into_iter().map(|(name, check)| {
            self.gog_updates.changed(&name);
            if check {
                crate::gog_library::recheck(self, name)
            } else {
                Task::none()
            }
        }))
    }

    fn merge_cards(&mut self, cards: Vec<Card>) {
        // A rescan that changed nothing the order reads keeps the order.
        let new_order = (self.cards.len() != cards.len()
            || !self
                .cards
                .iter()
                .zip(&cards)
                .all(|(old, new)| order::same_inputs(old, new)))
        .then(|| order::Order::sorted(&cards, &self.search, self.query));
        let layout_changed = new_order.as_ref().is_some_and(|indices| {
            let old = self.filtered();
            old.len() != indices.len()
                || old.iter().zip(indices).any(|(old, &index)| {
                    old.name != cards[index].name || old.display_title != cards[index].display_title
                })
        });
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
        match new_order {
            Some(indices) => self
                .order
                .replace(indices, &self.cards, &self.search, self.query),
            None => self.order.keep(&self.cards, &self.search, self.query),
        }
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

    pub(crate) fn refresh(&mut self) -> Task<Message> {
        let library = self.reload_library_once();
        let badge = if self.trash_refreshing {
            Task::none()
        } else {
            self.trash_refreshing = true;
            if self.section == sidebar::Section::Trash {
                self.reload_trash_page()
            } else {
                self.reload_badge()
            }
        };
        Task::batch([library, badge, self.settings_tick()])
    }

    pub(crate) fn cards_loaded(&mut self, cards: Vec<Card>) -> Task<Message> {
        self.refreshing = false;
        self.loaded = true;
        if self.hook_stop_confirm.as_ref().is_some_and(|name| {
            !cards.iter().any(|card| {
                &card.name == name
                    && card.busy_holder
                        == Some(wineo_core::operation::Operation::Hook(
                            wineo_core::hooks::Phase::After,
                        ))
            })
        }) {
            self.hook_stop_confirm = None;
            self.hook_stop_identity = None;
        }
        let art = self.refresh_art(&cards);
        let checks = self.merge_checked(cards);
        Task::batch([art, checks, self.load_visible_art(), self.load_panel_art()])
    }
}
