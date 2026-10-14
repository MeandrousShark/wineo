//! The Heroic page's event handling.

use iced::Task;
use wineo_core::cancel::Token;
use wineo_core::components::Component;
use wineo_core::download::Provider;
use wineo_core::gamescope::{Filter, GamescopeSetting, Scaler, Window};
use wineo_core::heroic::HeroicSource;
use wineo_core::heroic_import::{self, RunnerSuggestion};

use crate::flow::{Action, artwork};
use crate::task;

use super::{Draft, Event, Page, Prepared, Prerequisite, Review, ScopeDraft, Step, playtime_note};

impl Page {
    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        self.update_with_key_availability(event, false)
    }

    pub(crate) fn update_with_key_availability(
        &mut self,
        event: Event,
        key_available: bool,
    ) -> Task<Event> {
        match event {
            Event::Back => {
                // The Back button is a click, so the grid it returns to shows no ring.
                self.grid.focus_visible = false;
                self.back()
            }
            Event::Discard => {
                self.leave_prompt = false;
                self.action = Some(Action::Choose);
                Task::none()
            }
            Event::KeepEditing => {
                self.leave_prompt = false;
                Task::none()
            }
            Event::Source(index) => {
                let Some(source) = self.sources.get(index).cloned() else {
                    return Task::none();
                };
                self.source = Some(index);
                self.list(source)
            }
            Event::SourcePath(value) => {
                self.next_picker();
                self.source_path = value;
                Task::none()
            }
            Event::OpenSourcePath => {
                if self.source_path.trim().is_empty() {
                    self.error = Some("Enter a Heroic config folder".to_owned());
                    return Task::none();
                }
                match self.paths.absolute(&self.source_path) {
                    Ok(path) => {
                        let source = HeroicSource::from_config_dir(path);
                        self.sources.push(source.clone());
                        self.source = Some(self.sources.len() - 1);
                        self.list(source)
                    }
                    Err(error) => {
                        self.error = Some(error.to_string());
                        Task::none()
                    }
                }
            }
            Event::OpenExisting(name) => {
                self.action = Some(Action::Settings {
                    name,
                    launch: false,
                });
                Task::none()
            }
            Event::BrowseSource => {
                let sequence = self.next_picker();
                Task::perform(
                    crate::picker::pick("Heroic configuration folder".to_owned(), true),
                    move |result| Event::PickedSource(sequence, result),
                )
            }
            Event::PickedSource(sequence, Ok(Some(path)))
                if sequence == self.picker_sequence && self.step == Step::Sources =>
            {
                let source = HeroicSource::from_config_dir(path);
                self.sources.push(source.clone());
                self.source = Some(self.sources.len() - 1);
                self.list(source)
            }
            Event::PickedSource(sequence, Err(error))
                if sequence == self.picker_sequence && self.step == Step::Sources =>
            {
                self.error = Some(error);
                Task::none()
            }
            Event::PickedSource(_, Ok(None)) => Task::none(),
            Event::SourcesLoaded(generation, result) if generation == self.generation => {
                match result {
                    Ok(sources) => {
                        self.sources = sources;
                        self.status.clear();
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Listed(generation, result) if generation == self.generation => {
                match result {
                    Ok((listing, already)) => {
                        let task = self.load_posters(&listing);
                        self.listing = Some(listing);
                        self.already = already;
                        self.status.clear();
                        return task;
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::ProgressDue(generation) => {
                self.grid.progress.reveal(generation);
                Task::none()
            }
            Event::Poster(generation, id, result) if generation == self.grid.poster_generation => {
                self.grid.progress.finish_one();
                self.posters.insert(id, result.ok());
                // The game's details may have opened before its poster decoded.
                self.prefill_poster();
                Task::none()
            }
            Event::Toggle(index) => {
                self.grid.click(index);
                self.toggle(index);
                Task::none()
            }
            Event::SelectAll => {
                self.grid.chosen = self.eligible().collect();
                self.selection_changed();
                Task::none()
            }
            Event::SelectNone => {
                self.grid.chosen.clear();
                self.selection_changed();
                Task::none()
            }
            Event::ImportSelected => self.import_chosen(),
            Event::BatchPrepared(generation, result) if generation == self.generation => {
                match result {
                    Ok(options) => {
                        self.draft.runner = options.runner.clone();
                        self.batch_options = Some(options);
                        self.status.clear();
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Prepared(generation, result) if generation == self.generation => {
                match result {
                    Ok(prepared) => {
                        let prepared = *prepared;
                        self.draft = Draft {
                            name: heroic_import::default_name(
                                &prepared.candidate.title,
                                prepared.candidate.executable.as_deref(),
                            ),
                            executable: prepared
                                .candidate
                                .executable
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_default(),
                            arguments: prepared.arguments.clone(),
                            unparsed_arguments: prepared.unparsed_arguments.clone(),
                            working_directory: prepared.working_directory.clone(),
                            runner: prepared.runner.clone(),
                            environment: prepared.environment.clone(),
                            gamemode: prepared.gamemode.unwrap_or(false),
                            wayland: prepared.wayland.unwrap_or(false),
                            mangohud: prepared.mangohud.unwrap_or(false),
                            gamescope: ScopeDraft::from_setting(prepared.gamescope.clone()),
                            hdr: prepared.hdr,
                            ..Draft::default()
                        };
                        self.status.clear();
                        self.step = Step::Details;
                        self.art = artwork::Draft::new();
                        self.art_generation = self.art_generation.wrapping_add(1);
                        let art_generation = self.art_generation;
                        let task = self
                            .art
                            .configure(self.paths.clone(), prepared.candidate.title.clone())
                            .map(move |event| Event::Art(art_generation, event));
                        self.prepared = Some(prepared);
                        self.prefill_poster();
                        return task;
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Name(value) => {
                self.draft.name = value;
                self.review = None;
                Task::none()
            }
            Event::Executable(value) => {
                self.next_picker();
                self.draft.executable = value;
                self.review = None;
                Task::none()
            }
            Event::BrowseExecutable => {
                let sequence = self.next_picker();
                Task::perform(
                    crate::picker::pick("Game executable".to_owned(), false),
                    move |result| Event::PickedExecutable(sequence, result),
                )
            }
            Event::PickedExecutable(sequence, Ok(Some(path)))
                if sequence == self.picker_sequence
                    && self.step == Step::Details
                    && !self.draft.batch =>
            {
                self.draft.executable = path.display().to_string();
                self.review = None;
                Task::none()
            }
            Event::PickedExecutable(sequence, Err(error))
                if sequence == self.picker_sequence
                    && self.step == Step::Details
                    && !self.draft.batch =>
            {
                self.error = Some(error);
                Task::none()
            }
            Event::PickedExecutable(_, Ok(None)) => Task::none(),
            Event::ArgumentInput(value) => {
                self.draft.argument_input = value;
                self.review = None;
                Task::none()
            }
            Event::ArgumentAdd => {
                if !self.draft.argument_input.trim().is_empty() {
                    self.draft
                        .arguments
                        .push(std::mem::take(&mut self.draft.argument_input));
                    self.draft.unparsed_arguments = None;
                }
                self.review = None;
                Task::none()
            }
            Event::ArgumentRemove(index) => {
                if index < self.draft.arguments.len() {
                    self.draft.arguments.remove(index);
                }
                self.review = None;
                Task::none()
            }
            Event::ArgumentsClear => {
                self.draft.arguments.clear();
                self.draft.argument_input.clear();
                self.draft.unparsed_arguments = None;
                self.review = None;
                Task::none()
            }
            Event::EnvironmentInput(value) => {
                self.draft.environment_input = value;
                self.review = None;
                Task::none()
            }
            Event::EnvironmentAdd => {
                if !self.draft.environment_input.trim().is_empty() {
                    self.draft
                        .environment
                        .push(std::mem::take(&mut self.draft.environment_input));
                    self.draft.environment_dirty = true;
                }
                self.review = None;
                Task::none()
            }
            Event::EnvironmentRemove(index) => {
                if index < self.draft.environment.len() {
                    self.draft.environment.remove(index);
                    self.draft.environment_dirty = true;
                }
                self.review = None;
                Task::none()
            }
            Event::EnvironmentClear => {
                self.draft.environment.clear();
                self.draft.environment_input.clear();
                self.draft.environment_dirty = true;
                self.review = None;
                Task::none()
            }
            Event::Gamemode(value) => {
                self.draft.gamemode = value;
                self.draft.gamemode_dirty = true;
                self.review = None;
                Task::none()
            }
            Event::Wayland(value) => {
                self.draft.wayland = value;
                self.draft.wayland_dirty = true;
                if !value {
                    self.draft.hdr = false;
                }
                self.review = None;
                Task::none()
            }
            Event::Mangohud(value) => {
                self.draft.mangohud = value;
                self.draft.mangohud_dirty = true;
                self.review = None;
                Task::none()
            }
            Event::Hdr(value) => {
                self.draft.hdr = value;
                self.draft.hdr_dirty = true;
                self.review = None;
                Task::none()
            }
            Event::GamescopeEnabled(value) => {
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
            Event::GamescopeNumber(index, value) => {
                if let Some(number) = self.draft.gamescope.numbers.get_mut(index) {
                    *number = value;
                    self.draft.gamescope.dirty = true;
                }
                self.review = None;
                Task::none()
            }
            Event::GamescopeChoice(index, value) => {
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
            Event::GamescopeGrab(value) => {
                let mut scope = self.draft.gamescope.scope();
                scope.grab_cursor = value;
                self.draft.gamescope.setting = Some(GamescopeSetting::On(scope));
                self.draft.gamescope.dirty = true;
                self.review = None;
                Task::none()
            }
            Event::GamescopeArgInput(value) => {
                self.draft.gamescope.args_input = value;
                self.review = None;
                Task::none()
            }
            Event::GamescopeArgAdd => {
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
            Event::GamescopeArgRemove(index) => {
                if index < self.draft.gamescope.args.len() {
                    self.draft.gamescope.args.remove(index);
                    self.draft.gamescope.dirty = true;
                }
                self.review = None;
                Task::none()
            }
            Event::WorkingDirectory(value) => {
                self.next_picker();
                self.draft.working_directory = value;
                self.review = None;
                Task::none()
            }
            Event::BrowseWorkingDirectory => {
                let sequence = self.next_picker();
                Task::perform(
                    crate::picker::pick("Working directory".to_owned(), true),
                    move |result| Event::PickedWorkingDirectory(sequence, result),
                )
            }
            Event::PickedWorkingDirectory(sequence, Ok(Some(path)))
                if sequence == self.picker_sequence
                    && self.step == Step::Details
                    && !self.draft.batch =>
            {
                self.draft.working_directory = path.display().to_string();
                self.review = None;
                Task::none()
            }
            Event::PickedWorkingDirectory(sequence, Err(error))
                if sequence == self.picker_sequence
                    && self.step == Step::Details
                    && !self.draft.batch =>
            {
                self.error = Some(error);
                Task::none()
            }
            Event::PickedWorkingDirectory(_, Ok(None)) => Task::none(),
            Event::Runner(value) => {
                self.draft.runner = value;
                self.draft.runner_chosen = true;
                self.review = None;
                Task::none()
            }
            Event::Offer(on) if self.offer_shown() && !self.answering => {
                self.answering = true;
                self.relist(Some(on))
            }
            Event::Offered(sequence, session, result) if sequence == self.offer_sequence => {
                self.answering = false;
                match result {
                    Ok(refreshed) if session == self.details_session => self.refreshed(refreshed),
                    // The answer is saved, but these choices were for a game
                    // or batch that's no longer open, so read the open one's
                    // when it has a runner to pick.
                    Ok(_) if self.step == Step::Details && self.uses_runner() => self.relist(None),
                    Ok(_) => {
                        self.offer_answered();
                        Task::none()
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Task::none()
                    }
                }
            }
            Event::Adopt(value) => {
                self.draft.adopt = value;
                self.review = None;
                Task::none()
            }
            Event::Desktop(value) => {
                self.draft.desktop = value;
                self.review = None;
                Task::none()
            }
            Event::FetchBatchArt(value) => {
                let enabled = self
                    .batch_options
                    .as_ref()
                    .is_some_and(|options| options.art_fetch_enabled);
                if value && (!enabled || !key_available) {
                    return Task::none();
                }
                self.draft.fetch_batch_art = value;
                Task::none()
            }
            Event::Plan => self.plan(),
            Event::Planned(generation, result) if generation == self.generation => match result {
                Ok(review) => {
                    self.review = Some(*review);
                    self.status.clear();
                    self.enter_review()
                }
                Err(error) => {
                    self.error = Some(error);
                    Task::none()
                }
            },
            Event::CopyRunner => {
                let Some(Prepared {
                    suggested: Some(RunnerSuggestion::Copy(copy)),
                    ..
                }) = &self.prepared
                else {
                    return Task::none();
                };
                self.prerequisite(Prerequisite::CopyRunner(copy.clone()))
            }
            Event::DownloadRunner => {
                self.prerequisite(Prerequisite::Download(Provider::GeProton, None))
            }
            Event::DownloadBuild(component, version) => {
                let provider = match component {
                    Component::Dxvk => Provider::Dxvk,
                    Component::Vkd3d => Provider::Vkd3d,
                };
                self.prerequisite(Prerequisite::Download(provider, version))
            }
            Event::PrerequisiteFinished(generation, result) if generation == self.generation => {
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
            Event::Import => self.start_import(),
            Event::Cancel => {
                self.cancel(false);
                Task::none()
            }
            Event::Force => {
                self.cancel(true);
                Task::none()
            }
            Event::Progress(generation, status) if generation == self.generation => {
                self.status = status;
                Task::none()
            }
            Event::GameResult(generation, name, result) if generation == self.generation => {
                self.results.push((name, result));
                Task::none()
            }
            Event::SingleRegistered(generation, result) if generation == self.generation => {
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
                            self.notes.push("Heroic settings changed. Review the refreshed plan before importing.".to_owned());
                            self.step = Step::Details;
                            self.plan()
                        } else {
                            self.error = Some(error.message);
                            self.enter_review()
                        }
                    }
                }
            }
            Event::BatchFinished(generation, result) if generation == self.generation => {
                self.token = None;
                let (changed, canceled) = match result {
                    Ok(value) => value,
                    Err(error) => {
                        self.error = Some(error);
                        return self.enter_review();
                    }
                };
                if changed {
                    self.notes.push(
                        "Heroic settings changed. Review the remaining games again.".to_owned(),
                    );
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
            Event::ArtFinished(generation, result) if generation == self.generation => match result
            {
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
            },
            Event::RetryArt => {
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
            Event::SkipArt => {
                self.error = None;
                self.step = Step::Finished;
                self.status = "Import finished".to_owned();
                Task::none()
            }
            Event::FinishedRegistered(generation, result) if generation == self.generation => {
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
            Event::Art(generation, event) if generation == self.art_generation => self
                .art
                .update_with_key_availability(event, key_available)
                .map(move |event| Event::Art(generation, event)),
            Event::OpenSettings => {
                if let Some(registered) = &self.registered {
                    self.action = Some(Action::Settings {
                        name: registered.environment.name.clone(),
                        launch: false,
                    });
                }
                Task::none()
            }
            Event::BackToLibrary => {
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
            _ => Task::none(),
        }
    }
}
