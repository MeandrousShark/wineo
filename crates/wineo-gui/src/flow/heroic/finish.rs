//! The Heroic page's handling of prerequisites, imports, and artwork once
//! their work reports back.

use std::sync::Arc;

use iced::Task;
use wineo_core::cancel::Token;
use wineo_core::heroic_import::{Registered, RunnerSuggestion};

use crate::flow::Action;
use crate::task;

use super::{Event, ImportFailure, Page, Review, Step, playtime_note};

impl Page {
    pub(super) fn prerequisite_finished(&mut self, result: Result<String, String>) -> Task<Event> {
        self.token = None;
        self.step = Step::Details;
        match result {
            Ok(name) => {
                self.notes.push(format!(
                    "Installed {name}. It stays installed if this import is canceled."
                ));
                if self.draft.runner.is_empty()
                    || self.prepared.as_ref().is_some_and(|prepared| {
                        matches!(prepared.suggested, Some(RunnerSuggestion::Copy(_)))
                    })
                {
                    self.draft.runner = name;
                    self.draft.runner_chosen = true;
                }
                self.plan()
            }
            Err(error) => {
                self.error = Some(error);
                Task::none()
            }
        }
    }

    pub(super) fn single_registered(
        &mut self,
        generation: u64,
        result: Result<Arc<Registered>, ImportFailure>,
    ) -> Task<Event> {
        match result {
            Ok(registered) => {
                if let Some(note) = playtime_note(&registered.playtime) {
                    self.notes.push(note);
                }
                let title = match &self.review {
                    Some(Review::Single { game, .. }) => game.title(),
                    _ => &registered.environment.name,
                };
                self.status = format!("Imported {title}. Adding artwork...");
                let name = registered.environment.name.clone();
                self.registered = Some(registered);
                let art = self.art.clone();
                let paths = self.paths.clone();
                task::blocking(
                    move || art.apply(&paths, &name),
                    move |result| Event::ArtFinished(generation, result),
                )
            }
            Err(error) => {
                self.token = None;
                if error.source_changed {
                    self.notes.push(
                        "Heroic settings changed. Review the refreshed plan before importing."
                            .to_owned(),
                    );
                    self.step = Step::Details;
                    self.plan()
                } else {
                    self.error = Some(error.message);
                    self.enter_review()
                }
            }
        }
    }

    pub(super) fn batch_finished(&mut self, result: Result<(bool, bool), String>) -> Task<Event> {
        self.token = None;
        let (changed, canceled) = match result {
            Ok(value) => value,
            Err(error) => {
                self.error = Some(error);
                return self.enter_review();
            }
        };
        if changed {
            self.notes
                .push("Heroic settings changed. Review the remaining games again.".to_owned());
            self.step = Step::Details;
            self.plan()
        } else {
            self.step = Step::Finished;
            self.status = if canceled {
                "Batch import canceled. Completed imports remain."
            } else {
                "Batch import finished"
            }
            .to_owned();
            Task::none()
        }
    }

    pub(super) fn art_finished(&mut self, result: Result<Vec<String>, String>) -> Task<Event> {
        match result {
            Ok(notes) => {
                self.notes.extend(notes);
                self.finish_art()
            }
            Err(error) => {
                self.error = Some(format!(
                    "Artwork failed: {error}. The environment is registered. Retry or skip artwork."
                ));
                self.finish_art()
            }
        }
    }

    pub(super) fn retry_art(&mut self) -> Task<Event> {
        let Some(registered) = &self.registered else {
            return Task::none();
        };
        let name = registered.environment.name.clone();
        let art = self.art.clone();
        let paths = self.paths.clone();
        let generation = self.generation;
        self.token = Some(Token::new());
        self.error = None;
        task::blocking(
            move || art.apply(&paths, &name),
            move |result| Event::ArtFinished(generation, result),
        )
    }

    pub(super) fn finished_registered(
        &mut self,
        result: Result<Vec<String>, String>,
    ) -> Task<Event> {
        self.token = None;
        match result {
            Ok(notes) => self.notes.extend(notes),
            Err(error) => self.notes.push(format!("Desktop entry: {error}")),
        }
        if self.error.is_some() {
            self.step = Step::Working;
            self.status = "Imported with an artwork issue".to_owned();
        } else {
            self.step = Step::Finished;
            self.status = "Import finished".to_owned();
        }
        Task::none()
    }

    pub(super) fn open_settings(&mut self) -> Task<Event> {
        if let Some(registered) = &self.registered {
            self.action = Some(Action::Settings {
                name: registered.environment.name.clone(),
                launch: false,
            });
        }
        Task::none()
    }

    pub(super) fn back_to_library(&mut self) -> Task<Event> {
        let name = self
            .registered
            .as_ref()
            .map(|registered| registered.environment.name.clone());
        self.action = Some(Action::Library {
            name,
            launch: false,
        });
        Task::none()
    }
}
