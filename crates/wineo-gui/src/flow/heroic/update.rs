//! The Heroic page's event handling.

use std::collections::HashMap;

use iced::Task;
use wineo_core::components::Component;
use wineo_core::download::Provider;
use wineo_core::gamescope::GamescopeSetting;
use wineo_core::heroic::{HeroicListing, HeroicSource};
use wineo_core::heroic_import::{self, RunnerSuggestion};

use crate::flow::{Action, artwork};

use super::{
    BatchOptions, Draft, Event, Page, Prepared, Prerequisite, Refreshed, Review, ScopeDraft, Step,
};

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
            Event::OpenSourcePath => self.open_source_path(),
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
                self.listed(result)
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
                self.batch_prepared(result)
            }
            Event::Prepared(generation, result) if generation == self.generation => {
                self.prepared_game(result)
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
            Event::ArgumentAdd => self.add_argument(),
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
            Event::EnvironmentAdd => self.add_environment(),
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
            Event::GamescopeEnabled(value) => self.set_gamescope_enabled(value),
            Event::GamescopeNumber(index, value) => {
                if let Some(number) = self.draft.gamescope.numbers.get_mut(index) {
                    *number = value;
                    self.draft.gamescope.dirty = true;
                }
                self.review = None;
                Task::none()
            }
            Event::GamescopeChoice(index, value) => self.set_gamescope_choice(index, value),
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
            Event::GamescopeArgAdd => self.add_gamescope_arg(),
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
                self.offered(session, result)
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
            Event::FetchBatchArt(value) => self.set_fetch_batch_art(value, key_available),
            Event::Plan => self.plan(),
            Event::Planned(generation, result) if generation == self.generation => {
                self.planned(result)
            }
            Event::CopyRunner => self.copy_runner(),
            Event::DownloadRunner => {
                self.prerequisite(Prerequisite::Download(Provider::GeProton, None))
            }
            Event::DownloadBuild(component, version) => self.download_build(component, version),
            Event::PrerequisiteFinished(generation, result) if generation == self.generation => {
                self.prerequisite_finished(result)
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
                self.single_registered(generation, result)
            }
            Event::BatchFinished(generation, result) if generation == self.generation => {
                self.batch_finished(result)
            }
            Event::ArtFinished(generation, result) if generation == self.generation => {
                self.art_finished(result)
            }
            Event::RetryArt => self.retry_art(),
            Event::SkipArt => {
                self.error = None;
                self.step = Step::Finished;
                self.status = "Import finished".to_owned();
                Task::none()
            }
            Event::FinishedRegistered(generation, result) if generation == self.generation => {
                self.finished_registered(result)
            }
            Event::Art(generation, event) if generation == self.art_generation => self
                .art
                .update_with_key_availability(event, key_available)
                .map(move |event| Event::Art(generation, event)),
            Event::OpenSettings => self.open_settings(),
            Event::BackToLibrary => self.back_to_library(),
            _ => Task::none(),
        }
    }

    fn open_source_path(&mut self) -> Task<Event> {
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

    fn listed(
        &mut self,
        result: Result<(HeroicListing, HashMap<String, String>), String>,
    ) -> Task<Event> {
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

    fn batch_prepared(&mut self, result: Result<BatchOptions, String>) -> Task<Event> {
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

    fn prepared_game(&mut self, result: Result<Box<Prepared>, String>) -> Task<Event> {
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

    fn offered(&mut self, session: u64, result: Result<Refreshed, String>) -> Task<Event> {
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

    fn set_fetch_batch_art(&mut self, value: bool, key_available: bool) -> Task<Event> {
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

    fn planned(&mut self, result: Result<Box<Review>, String>) -> Task<Event> {
        match result {
            Ok(review) => {
                self.review = Some(*review);
                self.status.clear();
                self.enter_review()
            }
            Err(error) => {
                self.error = Some(error);
                Task::none()
            }
        }
    }

    fn copy_runner(&mut self) -> Task<Event> {
        let Some(Prepared {
            suggested: Some(RunnerSuggestion::Copy(copy)),
            ..
        }) = &self.prepared
        else {
            return Task::none();
        };
        self.prerequisite(Prerequisite::CopyRunner(copy.clone()))
    }

    fn download_build(&mut self, component: Component, version: Option<String>) -> Task<Event> {
        let provider = match component {
            Component::Dxvk => Provider::Dxvk,
            Component::Vkd3d => Provider::Vkd3d,
        };
        self.prerequisite(Prerequisite::Download(provider, version))
    }
}
