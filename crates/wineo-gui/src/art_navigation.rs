use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};

use crate::{App, Message, art_page, sidebar};

impl App {
    pub(crate) fn art_busy(&self) -> bool {
        !self.pending_art_writes.is_empty() || self.art_pages.values().any(art_page::Page::busy)
    }

    pub(crate) fn open_art(&mut self, name: String) -> Task<Message> {
        if self.settings.is_some()
            || self.flow.is_some()
            || self.removal.is_some()
            || self.trash_confirm_open()
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
                art_page::Page::new(name.clone(), self.art_generation, identity),
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
        generation: u64,
        event: art_page::Event,
    ) -> Task<Message> {
        let completion = write_completion(&event);
        let Some(page) = self.art_pages.get(&name) else {
            return self.drop_stale_art_event(&name, generation, completion);
        };
        if page.generation != generation || self.art_page.as_deref() != Some(&name) {
            return self.drop_stale_art_event(&name, generation, completion);
        }
        if art_page::EnvironmentIdentity::read(&self.paths, &name) != Some(page.identity) {
            self.close_missing_art(&name);
            return self.drop_stale_art_event(&name, generation, completion);
        }
        match event {
            art_page::Event::Back if self.art_page.as_ref() == Some(&name) => {
                self.leave_art();
                Task::batch([self.load_visible_art(), self.load_panel_art()])
            }
            art_page::Event::Settings if self.art_page.as_ref() == Some(&name) => {
                self.navigate_section(sidebar::Section::Settings)
            }
            art_page::Event::Back | art_page::Event::Settings => Task::none(),
            event => {
                if let Some((sequence, _, _)) = completion {
                    self.pending_art_writes.remove(&(
                        name.clone(),
                        generation,
                        sequence,
                        page.identity,
                    ));
                }
                let decodes = self.decodes.clone();
                let (step, identity, write_sequence, started_write) = {
                    let page = self.art_pages.get_mut(&name).unwrap();
                    let was_writing = page.writing;
                    let step = page.update(&self.paths, event, decodes);
                    (
                        step,
                        page.identity,
                        page.write_sequence,
                        !was_writing && page.writing,
                    )
                };
                if started_write {
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
        art_page::Event::Wrote(sequence, _, result) => Some((*sequence, result.changed(), true)),
        art_page::Event::GameSaved(sequence, _, result) => Some((*sequence, result.is_ok(), false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::event::Status;
    use std::fs;
    use wineo_core::art::{self, Kind};
    use wineo_core::config::GlobalConfig;
    use wineo_core::environment::Environment;
    use wineo_core::paths::Paths;

    fn fixture() -> (tempfile::TempDir, App) {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        for name in ["game", "other"] {
            Environment::create_native(&paths, &GlobalConfig::default(), name, None, false)
                .unwrap();
        }
        let mut app = App::boot(paths.clone()).0;
        app.merge(crate::library::load_library(&paths).unwrap());
        (root, app)
    }

    #[test]
    fn panel_and_both_context_menus_open_the_selected_environment_art() {
        for from_menu in [false, true] {
            for panel_open in [false, true] {
                let (_root, mut app) = fixture();
                app.selected = Some("other".into());
                app.panel_open = panel_open;
                if from_menu {
                    let _ = app.update(Message::Context("game".into()));
                    let _ = app.update(Message::ContextArt("game".into()));
                } else {
                    app.selected = Some("game".into());
                    let _ = app.update(Message::OpenArt);
                }
                assert_eq!(app.art_page.as_deref(), Some("game"));
                assert_eq!(app.selected.as_deref(), Some("game"));
                assert!(app.action_menu.is_none());
                assert!(app.settings.is_none());
                assert!(app.section_selected(sidebar::Section::Library));
                assert!(!app.section_selected(sidebar::Section::Settings));
            }
        }
    }

    #[test]
    fn back_and_escape_return_to_the_library_and_preserve_the_panel() {
        for escape in [false, true] {
            let (_root, mut app) = fixture();
            app.selected = Some("game".into());
            app.panel_open = true;
            let _ = app.update(Message::OpenArt);
            let generation = app.art_pages["game"].generation;
            if escape {
                let _ = app.update(Message::Key(
                    Key::Named(Named::Escape),
                    Modifiers::empty(),
                    Status::Ignored,
                ));
            } else {
                let _ = app.update(Message::ArtPage(
                    "game".into(),
                    generation,
                    art_page::Event::Back,
                ));
            }
            assert!(app.art_page.is_none());
            assert!(app.panel_open);
            assert_eq!(app.selected.as_deref(), Some("game"));
            assert_eq!(app.section, sidebar::Section::Library);
            assert!(app.art_pages.contains_key("game"));
        }
    }

    #[test]
    fn sidebar_navigation_leaves_art_and_opens_its_destination() {
        for section in [
            sidebar::Section::Library,
            sidebar::Section::Trash,
            sidebar::Section::Checkup,
            sidebar::Section::Settings,
        ] {
            let (_root, mut app) = fixture();
            let _ = app.update(Message::ContextArt("game".into()));
            let _ = app.update(Message::Section(section));
            assert!(app.art_page.is_none());
            assert!(app.section_selected(section));
            assert!(app.art_pages.contains_key("game"));
        }
    }

    #[test]
    fn settings_hint_opens_global_settings() {
        let (_root, mut app) = fixture();
        let _ = app.update(Message::ContextArt("game".into()));
        let generation = app.art_pages["game"].generation;
        let _ = app.update(Message::ArtPage(
            "game".into(),
            generation,
            art_page::Event::Settings,
        ));
        assert!(app.art_page.is_none());
        assert!(app.global_settings_open());
    }

    #[test]
    fn stale_navigation_cannot_close_another_environment_art_page() {
        let (_root, mut app) = fixture();
        let _ = app.update(Message::ContextArt("game".into()));
        let generation = app.art_pages["game"].generation;
        let _ = app.update(Message::Section(sidebar::Section::Library));
        let _ = app.update(Message::ContextArt("other".into()));
        let _ = app.update(Message::ArtPage(
            "game".into(),
            generation,
            art_page::Event::Back,
        ));
        assert_eq!(app.art_page.as_deref(), Some("other"));
        let _ = app.update(Message::ArtPage(
            "other".into(),
            generation,
            art_page::Event::Back,
        ));
        assert_eq!(app.art_page.as_deref(), Some("other"));
    }

    #[test]
    fn art_page_does_not_accept_hidden_library_actions_or_launch_add() {
        let (_root, mut app) = fixture();
        app.search = "original".into();
        let _ = app.update(Message::ContextArt("game".into()));
        let _ = app.update(Message::SearchChanged("hidden".into()));
        let _ = app.update(Message::Select("other".into()));
        let _ = app.update(Message::Add);
        let _ = app.update(Message::CloneEnvironment("game".into()));
        let _ = app.update(Message::Play);
        assert_eq!(app.search, "original");
        assert_eq!(app.selected.as_deref(), Some("game"));
        assert!(app.flow.is_none());
        assert!(app.operation.is_none());
    }

    fn browse(app: &mut App, path: std::path::PathBuf) -> (String, u64) {
        let _ = app.update(Message::ContextArt("game".into()));
        let generation = app.art_pages["game"].generation;
        let _ = app.update(Message::ArtPage(
            "game".into(),
            generation,
            art_page::Event::Browse(wineo_core::art::Kind::Grid),
        ));
        let _ = app.update(Message::ArtPage(
            "game".into(),
            generation,
            art_page::Event::Picked(1, (None, wineo_core::art::Kind::Grid), Ok(Some(path))),
        ));
        assert!(app.art_busy());
        ("game".into(), generation)
    }

    #[test]
    fn browse_completion_refreshes_the_card_and_evicts_old_art_cache() {
        use wineo_core::art::{self, Kind};
        let (root, mut app) = fixture();
        let source = root.path().join("grid.png");
        ::image::RgbImage::new(2, 3).save(&source).unwrap();
        art::set_manual(&app.paths, "game", None, Kind::Grid, &source).unwrap();
        app.merge(crate::library::load_library(&app.paths).unwrap());
        let old = app
            .cards
            .iter()
            .find(|card| card.name == "game")
            .unwrap()
            .art
            .clone()
            .unwrap();
        let old_key = (old.clone(), crate::artwork::Kind::Grid);
        app.images.insert(
            old_key.clone(),
            iced::widget::image::Handle::from_rgba(1, 1, vec![0; 4]),
        );
        ::image::RgbImage::new(10, 15).save(&source).unwrap();
        let (name, generation) = browse(&mut app, source.clone());
        let previous = app.library_generation;
        let outcome = art_page::write_art(&app.paths, "game", &(None, Kind::Grid), Some(&source));
        let _ = app.update(Message::ArtPage(
            name,
            generation,
            art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
        ));
        assert!(!app.art_busy());
        assert_ne!(app.library_generation, previous);
        let cards = crate::library::load_library(&app.paths).unwrap();
        let _ = app.update(Message::LibraryReloaded(app.library_generation, Ok(cards)));
        let new = app
            .cards
            .iter()
            .find(|card| card.name == "game")
            .unwrap()
            .art
            .clone()
            .unwrap();
        assert_ne!(new.identity, old.identity);
        assert!(!app.images.contains_key(&old_key));
        assert!(app.loading.contains(&(new, crate::artwork::Kind::Grid)));
    }

    #[test]
    fn closing_after_leaving_art_waits_for_the_write_to_complete() {
        let (root, mut app) = fixture();
        let source = root.path().join("grid.png");
        ::image::RgbImage::new(2, 3).save(&source).unwrap();
        let (name, generation) = browse(&mut app, source.clone());
        let _ = app.update(Message::Section(sidebar::Section::Trash));
        assert!(app.art_page.is_none());
        assert!(app.foreground_busy());
        let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
        assert!(app.close_prompt && !app.closing);
        let _ = app.update(Message::CancelAndClose);
        assert!(app.closing && app.foreground_busy());
        let outcome = art_page::write_art(&app.paths, "game", &(None, Kind::Grid), Some(&source));
        let _ = app.update(Message::ArtPage(
            name,
            generation,
            art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
        ));
        assert!(!app.foreground_busy());
        assert!(app.paths.art_dir("game").join("grid.png").exists());
    }

    #[test]
    fn refresh_closes_and_drops_art_after_external_removal() {
        let (_root, mut app) = fixture();
        let _ = app.update(Message::ContextArt("game".into()));
        fs::remove_file(app.paths.environment_config("game")).unwrap();

        let cards = crate::library::load_library(&app.paths).unwrap();
        let _ = app.update(Message::Loaded(Ok(cards)));

        assert!(app.art_page.is_none());
        assert!(!app.art_pages.contains_key("game"));
        assert!(app.selected.is_none());
        assert!(!app.panel_open);
    }

    #[test]
    fn refresh_closes_art_after_external_rename() {
        let (_root, mut app) = fixture();
        let _ = app.update(Message::ContextArt("game".into()));
        fs::rename(
            app.paths.environment_config("game"),
            app.paths.environment_config("renamed"),
        )
        .unwrap();

        let cards = crate::library::load_library(&app.paths).unwrap();
        let _ = app.update(Message::Loaded(Ok(cards)));

        assert!(app.art_page.is_none());
        assert!(!app.art_pages.contains_key("game"));
        assert!(app.selected.is_none());
        assert!(!app.panel_open);
    }

    #[test]
    fn same_name_recreation_gets_a_fresh_art_page() {
        let (root, mut app) = fixture();
        let _ = app.update(Message::ContextArt("game".into()));
        let old_generation = app.art_pages["game"].generation;
        let old_identity = app.art_pages["game"].identity;
        let retained = root.path().join("retained-game.toml");
        fs::rename(app.paths.environment_config("game"), retained).unwrap();
        Environment::create_native(&app.paths, &GlobalConfig::default(), "game", None, false)
            .unwrap();

        let cards = crate::library::load_library(&app.paths).unwrap();
        let _ = app.update(Message::Loaded(Ok(cards)));
        assert!(app.art_page.is_none());
        assert!(!app.art_pages.contains_key("game"));

        let _ = app.update(Message::ContextArt("game".into()));

        let page = &app.art_pages["game"];
        assert_ne!(page.generation, old_generation);
        assert_ne!(page.identity, old_identity);
        assert_eq!(page.game_label(), "Not picked");
    }

    #[test]
    fn refresh_reloads_open_art_after_an_external_art_change() {
        let (root, mut app) = fixture();
        let _ = app.update(Message::ContextArt("game".into()));
        let generation = app.art_pages["game"].generation;
        let source = root.path().join("grid.png");
        ::image::RgbImage::new(10, 15).save(&source).unwrap();
        art::set_manual(&app.paths, "game", None, Kind::Grid, &source).unwrap();
        fs::write(
            app.paths.environment_config("game"),
            "schema_version = 1\nkind = 'native'\n[targets.fresh]\nexe = 'notepad'\n",
        )
        .unwrap();
        let load_sequence = app.art_pages["game"].load_sequence;

        let cards = crate::library::load_library(&app.paths).unwrap();
        let _ = app.update(Message::Loaded(Ok(cards)));

        let page = &app.art_pages["game"];
        assert!(page.loading);
        assert!(page.load_sequence > load_sequence);
        let sequence = page.load_sequence;
        let snapshot = art_page::load_snapshot(&app.paths, "game").unwrap();
        let _ = app.update(Message::ArtPage(
            "game".into(),
            generation,
            art_page::Event::Loaded(sequence, Ok(snapshot)),
        ));
        assert!(app.art_pages["game"].has_override(Kind::Grid));
        assert!(app.art_pages["game"].has_target("fresh"));
    }

    #[test]
    fn late_write_result_after_close_does_not_update_cached_page() {
        let (root, mut app) = fixture();
        let source = root.path().join("grid.png");
        ::image::RgbImage::new(2, 3).save(&source).unwrap();
        let (name, generation) = browse(&mut app, source.clone());
        let _ = app.update(Message::ArtPage(
            name.clone(),
            generation,
            art_page::Event::Back,
        ));
        assert!(
            app.pending_art_writes
                .iter()
                .any(|(pending_name, _, _, _)| pending_name == &name)
        );

        let outcome = art_page::write_art(&app.paths, "game", &(None, Kind::Grid), Some(&source));
        let _ = app.update(Message::ArtPage(
            name,
            generation,
            art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
        ));

        assert!(!app.art_busy());
        assert!(!app.art_pages["game"].has_override(Kind::Grid));
    }

    #[test]
    fn late_lookup_and_preview_results_after_close_are_dropped() {
        let (root, mut app) = fixture();
        let source = root.path().join("grid.png");
        ::image::RgbImage::new(10, 15).save(&source).unwrap();
        art::set_manual(&app.paths, "game", None, Kind::Grid, &source).unwrap();
        art::save_game(&app.paths, "game", 77).unwrap();

        let _ = app.update(Message::ContextArt("game".into()));
        let generation = app.art_pages["game"].generation;
        let load_sequence = app.art_pages["game"].load_sequence;
        let mut snapshot = art_page::load_snapshot(&app.paths, "game").unwrap();
        snapshot.key_available = true;
        let _ = app.update(Message::ArtPage(
            "game".into(),
            generation,
            art_page::Event::Loaded(load_sequence, Ok(snapshot)),
        ));
        let lookup_sequence = app.art_pages["game"].lookup_sequence;
        let preview_sequence = app.art_pages["game"].load_sequence;

        let _ = app.update(Message::ArtPage(
            "game".into(),
            generation,
            art_page::Event::Back,
        ));
        let _ = app.update(Message::ArtPage(
            "game".into(),
            generation,
            art_page::Event::GameLoaded(
                lookup_sequence,
                77,
                Ok(Some(wineo_core::steamgriddb::Game {
                    id: 77,
                    name: "Stale game name".into(),
                    year: None,
                })),
            ),
        ));
        let _ = app.update(Message::ArtPage(
            "game".into(),
            generation,
            art_page::Event::Previewed(
                preview_sequence,
                (None, Kind::Grid),
                Ok((
                    iced::widget::image::Handle::from_rgba(1, 1, vec![0; 4]),
                    Some((10, 15)),
                )),
            ),
        ));

        assert_eq!(app.art_pages["game"].game_label(), "77");
        assert!(!app.art_pages["game"].has_preview(&(None, Kind::Grid)));
    }
}
