//! Import from GOG, like `wineo import gog`: every game you own, to install
//! (see `gog_install`), and the GOG games Heroic installed, to move into
//! wineo. A migration is reviewed, applied, then adds GOG's playtime and
//! art, which never fails the migration.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use iced::futures::channel::mpsc::unbounded;
use iced::keyboard::{self, Key, key::Named};
use iced::widget::{column, container, image, pick_list, row, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::art::Kind;
use wineo_core::cancel::Token;
use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Https, Provider};
use wineo_core::gog::account::Session;
use wineo_core::gog::api::Client;
use wineo_core::gog::extras::{self, Request};
use wineo_core::gog::folder::{self, FolderState, JournalKind};
use wineo_core::gog::migrate::{
    self, MigrateError, Migration, Outcome, Plan, ReviewLabel, StateChange, Target,
};
use wineo_core::heroic::{HeroicGogGame, HeroicSource, gog_games};
use wineo_core::heroic_import::{self, Policy, RunnerSuggestion};
use wineo_core::key_store::Keyring;
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb;

use super::Action;
use super::gog_install::{self, Install, Leave, Offer};
use super::gog_posters::{self, Job, Network, Source as PosterSource};
use super::heroic::{self, Prerequisite};
use super::import_grid::{ImportGrid, Tile};
use crate::runner_label::Choice;
use crate::task;
use crate::theme::Palette;
use crate::ui::{PickListStyle, accent_base, secondary_button};

const GRID_ID: &str = "gog-import-grid";

pub(crate) const HEROIC_NOTE: &str =
    "Remove it from Heroic's library, without deleting its files, so both don't update it.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Games,
    Review,
    Working,
    Finished,
    /// Installing an owned game, which `install` runs.
    Install,
}

#[derive(Debug, Clone)]
enum Standing {
    Ready(Box<Migration>),
    Already { environment: String },
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    source: HeroicSource,
    game: HeroicGogGame,
    standing: Standing,
    /// Which Heroic the game is read from, when another config that lists
    /// it was set aside.
    heroic: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Listing {
    games: Vec<Candidate>,
    /// Heroic records that couldn't be used, as sentences.
    issues: Vec<String>,
    /// The owned games Heroic doesn't have, by what wineo has of them.
    owned: Vec<Owned>,
    /// Why the owned games aren't all listed, like being logged out.
    owned_note: Option<String>,
}

/// An owned game, and what wineo has of it, like `import gog --list` says.
#[derive(Debug, Clone)]
pub(crate) struct Owned {
    offer: Offer,
    state: OwnedState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OwnedState {
    NotInstalled,
    /// Its install stopped; installing again finishes it.
    Unfinished,
    /// Installed by wineo, as this environment.
    Installed(String),
    /// Migrated from Heroic, as this environment.
    Imported(String),
}

/// One tile of the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Heroic(usize),
    Owned(usize),
}

/// What logged out says about the games you own.
const OWNED_LOGGED_OUT: &str = "Log in to GOG in Settings to install the games you own.";

/// One game ready to review: its migration, with a runner planned for a new
/// Windows environment when one could be.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    source: HeroicSource,
    heroic: Option<String>,
    migration: Box<Migration>,
    runners: Vec<Choice>,
    runner: String,
    suggestion: Option<RunnerSuggestion>,
    missing_build: Option<(Component, Option<String>)>,
    /// Why it can't be migrated as it stands.
    blocking: Option<String>,
}

impl Prepared {
    fn ready(&self) -> bool {
        self.blocking.is_none()
            && self.missing_build.is_none()
            && self.migration.environment().is_some()
    }
}

/// What a migration did, for the finish page.
#[derive(Debug, Clone, Default)]
pub(crate) struct Report {
    lines: Vec<String>,
    warnings: Vec<String>,
    environment: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Discard,
    KeepEditing,
    Listed(u64, Result<Listing, String>),
    Pick(usize),
    Prepared(u64, Result<Box<Prepared>, String>),
    Runner(String),
    CopyRunner,
    DownloadRunner,
    DownloadBuild(Component, Option<String>),
    PrerequisiteFinished(u64, Result<String, String>),
    Migrate,
    Cancel,
    Force,
    Progress(u64, String),
    Finished(u64, Result<Report, String>),
    BackToLibrary,
    /// Opens the install of the owned game at this index.
    Install(usize),
    /// An install's event, with the instance it belongs to, so a reply to
    /// a review that was left can't reach the next one.
    Installing(u64, Box<gog_install::Event>),
    /// A decoded poster, with the load it belongs to and the id of its tile.
    Poster(u64, String, Result<image::Handle, String>),
    /// The poster worker of this load has stopped, with or without a poster
    /// for every game it was given.
    PostersEnded(u64),
    /// The delay before the art progress bar shows has passed.
    ProgressDue(u64),
}

/// Numbers each poster load across pages, so a late reply from a page that
/// was left can't match the load of the one opened next.
static POSTER_LOADS: AtomicU64 = AtomicU64::new(0);

fn next_poster_load() -> u64 {
    POSTER_LOADS.fetch_add(1, Ordering::Relaxed)
}

pub(crate) struct Page {
    paths: Paths,
    step: Step,
    generation: u64,
    listing: Option<Listing>,
    picked: Option<usize>,
    prepared: Option<Prepared>,
    report: Option<Report>,
    token: Option<Token>,
    status: String,
    error: Option<String>,
    notes: Vec<String>,
    leave_prompt: bool,
    action: Option<Action>,
    install: Option<Box<Install>>,
    /// Which opened review `install` is; each one gets a new number.
    install_instance: u64,
    /// The environment whose interrupted install this page was opened to
    /// finish from the library, which it returns to instead of the grid.
    finishing: Option<String>,
    /// Its focus is the tile the keyboard is on, in grid order. Nothing is
    /// picked here, since a tile opens its game.
    grid: ImportGrid<Event>,
    /// Decoded posters by tile id. None when a game's poster failed to load,
    /// which keeps its text tile.
    posters: HashMap<String, Option<image::Handle>>,
    /// Stops the worker loading posters for a listing that is gone.
    poster_token: Option<Token>,
}

impl Drop for Page {
    fn drop(&mut self) {
        if let Some(token) = &self.poster_token {
            token.cancel();
        }
    }
}

impl Page {
    pub(crate) fn new(paths: Paths) -> (Self, Task<Event>) {
        let mut page = Self::blank(paths);
        let task = page.list();
        (page, task)
    }

    /// Opens the install page for `offer`, whose environment's install
    /// stopped, without the grid of games: the library's Finish install
    /// does.
    pub(crate) fn finish_install(paths: Paths, offer: Offer) -> (Self, Task<Event>) {
        let mut page = Self::blank(paths);
        page.finishing = offer.unfinished.clone();
        let (install, task) = Install::new(page.paths.clone(), offer);
        page.install = Some(Box::new(install));
        page.install_instance = 1;
        page.step = Step::Install;
        let instance = page.install_instance;
        let task = task.map(move |event| Event::Installing(instance, Box::new(event)));
        (page, task)
    }

    /// The environment this page finishes the install of, when opened from
    /// the library for that.
    #[cfg(test)]
    pub(super) fn finishing_name(&self) -> Option<&str> {
        self.finishing.as_deref()
    }

    fn blank(paths: Paths) -> Self {
        Self {
            paths,
            step: Step::Games,
            generation: 0,
            listing: None,
            picked: None,
            prepared: None,
            report: None,
            token: None,
            status: String::new(),
            error: None,
            notes: Vec::new(),
            leave_prompt: false,
            action: None,
            install: None,
            install_instance: 0,
            finishing: None,
            grid: ImportGrid::new(GRID_ID, Event::ProgressDue, next_poster_load()),
            posters: HashMap::new(),
            poster_token: None,
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub(crate) fn busy(&self) -> bool {
        self.token.is_some() || self.install.as_ref().is_some_and(|install| install.busy())
    }

    /// A listing, preparation, migration, or install is running.
    pub(crate) fn waiting(&self) -> bool {
        !self.status.is_empty()
            || self
                .install
                .as_ref()
                .is_some_and(|install| install.waiting())
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        if let Some(install) = &mut self.install {
            install.cancel(force);
            return;
        }
        if let Some(token) = &self.token {
            if force {
                token.force();
                self.status = "Force stop requested...".to_owned();
            } else {
                token.cancel();
                self.status = "Canceling...".to_owned();
            }
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        if let Some(install) = &mut self.install {
            let task = install.submit();
            return self.installing(task);
        }
        if self.busy() || self.waiting() {
            return Task::none();
        }
        match self.step {
            Step::Review => self.start(),
            Step::Finished => self.update(Event::BackToLibrary),
            Step::Games => self.open_focused(),
            Step::Working | Step::Install => Task::none(),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        if let Some(install) = &mut self.install {
            let task = install.back();
            return self.installing(task);
        }
        if self.busy() {
            return Task::none();
        }
        // A reply to what this step started must not reopen it.
        self.next_generation();
        self.status.clear();
        self.error = None;
        match self.step {
            // A game was picked, so there is a draft to lose.
            Step::Games if self.picked.is_some() => self.leave_prompt = true,
            Step::Games => self.action = Some(Action::Choose),
            Step::Review => {
                self.notes.clear();
                self.step = Step::Games;
            }
            Step::Working | Step::Install => {}
            Step::Finished => self.exit(),
        }
        if self.step == Step::Games && self.action.is_none() && self.listing.is_some() {
            // The posters still loading belong to a load that is now stale,
            // so pick up the ones not yet decoded under a new one.
            self.load_posters()
        } else {
            self.stop_posters();
            Task::none()
        }
    }

    /// Wraps the install's task, and follows where it leaves to.
    fn installing(&mut self, task: Task<gog_install::Event>) -> Task<Event> {
        let leave = self
            .install
            .as_mut()
            .and_then(|install| install.take_leave());
        match leave {
            Some(Leave::Games) if self.finishing.is_some() => {
                self.action = Some(Action::Library {
                    name: self.finishing.clone(),
                    launch: false,
                });
            }
            Some(Leave::Games) => {
                self.install = None;
                self.step = Step::Games;
                self.error = None;
                // What it may have changed shows in the grid.
                return self.list();
            }
            Some(Leave::Library(name)) => {
                self.action = Some(Action::Library {
                    name,
                    launch: false,
                });
            }
            None => {}
        }
        let instance = self.install_instance;
        task.map(move |event| Event::Installing(instance, Box::new(event)))
    }

    /// The grid's tiles in order: every game, by title.
    fn entries(listing: &Listing) -> Vec<Entry> {
        let mut entries: Vec<(String, Entry)> = listing
            .games
            .iter()
            .enumerate()
            .map(|(index, candidate)| (candidate.game.title.to_lowercase(), Entry::Heroic(index)))
            .chain(
                listing
                    .owned
                    .iter()
                    .enumerate()
                    .map(|(index, owned)| (owned.offer.title.to_lowercase(), Entry::Owned(index))),
            )
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.into_iter().map(|(_, entry)| entry).collect()
    }

    fn open(&mut self, entry: Entry) -> Task<Event> {
        match entry {
            Entry::Heroic(index) => self.update(Event::Pick(index)),
            Entry::Owned(index) => self.update(Event::Install(index)),
        }
    }

    fn open_focused(&mut self) -> Task<Event> {
        let entry = self
            .listing
            .as_ref()
            .zip(self.grid.focus)
            .and_then(|(listing, focus)| Self::entries(listing).get(focus).copied());
        match entry {
            Some(entry) => self.open(entry),
            None => Task::none(),
        }
    }

    /// Arrow keys move between tiles, and Enter or Space opens one.
    pub(crate) fn key(
        &mut self,
        key: &Key,
        _modifiers: keyboard::Modifiers,
        width: f32,
    ) -> Option<Task<Event>> {
        if self.step != Step::Games || self.waiting() || self.leave_prompt {
            return None;
        }
        let count = Self::entries(self.listing.as_ref()?).len();
        if let Some(task) = self.grid.arrow(key, width, count) {
            return Some(task);
        }
        match key.as_ref() {
            Key::Named(Named::Space | Named::Enter) | Key::Character(" ") => {
                Some(self.open_focused())
            }
            _ => None,
        }
    }

    fn exit(&mut self) {
        self.action = Some(Action::Library {
            name: self
                .report
                .as_ref()
                .and_then(|report| report.environment.clone()),
            launch: false,
        });
    }

    fn list(&mut self) -> Task<Event> {
        self.step = Step::Games;
        self.listing = None;
        self.posters.clear();
        self.stop_posters();
        self.status = "Reading your GOG games...".to_owned();
        let generation = self.next_generation();
        let paths = self.paths.clone();
        task::blocking(
            move || load_listing(&paths),
            move |result| Event::Listed(generation, result),
        )
    }

    /// Invalidates the poster load in flight, so its replies are dropped and
    /// its worker stops at the next game.
    fn stop_posters(&mut self) {
        if let Some(token) = self.poster_token.take() {
            token.cancel();
        }
        self.grid.restart_posters(next_poster_load());
    }

    /// Loads the listing's posters that aren't decoded yet on one worker,
    /// which sends each as it finishes.
    fn load_posters(&mut self) -> Task<Event> {
        self.stop_posters();
        let jobs = self.poster_jobs();
        if jobs.is_empty() {
            return Task::none();
        }
        let token = Token::new();
        self.poster_token = Some(token.clone());
        let generation = self.grid.poster_generation;
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let count = jobs.len();
        let work = Task::future(async move {
            let _ = tokio::task::spawn_blocking(move || {
                gog_posters::run(&paths, jobs, &token, Network::open, |id, poster| {
                    let _ = sender.unbounded_send(Event::Poster(generation, id, poster));
                });
            })
            .await;
            Event::PostersEnded(generation)
        });
        let shown = self.grid.progress.add(count, Instant::now());
        Task::batch([Task::run(receiver, |event| event), work, shown])
    }

    /// A poster to load for every tile that has none yet, in grid order. A
    /// game wineo has uses the art it stored; the others use GOG's cover.
    fn poster_jobs(&self) -> Vec<Job> {
        let Some(listing) = &self.listing else {
            return Vec::new();
        };
        Self::entries(listing)
            .into_iter()
            .filter_map(|entry| {
                let (id, source) = match entry {
                    Entry::Heroic(index) => {
                        let candidate = &listing.games[index];
                        let id = candidate.game.id.clone();
                        let source = match &candidate.standing {
                            Standing::Already { environment } => {
                                PosterSource::Stored(environment.clone())
                            }
                            Standing::Ready(_) | Standing::Failed(_) => {
                                PosterSource::Cover(id.parse().ok()?)
                            }
                        };
                        (id, source)
                    }
                    Entry::Owned(index) => {
                        let owned = &listing.owned[index];
                        let source = match &owned.state {
                            OwnedState::Installed(name) | OwnedState::Imported(name) => {
                                PosterSource::Stored(name.clone())
                            }
                            OwnedState::NotInstalled | OwnedState::Unfinished => {
                                PosterSource::Cover(owned.offer.gog_id)
                            }
                        };
                        (owned.offer.gog_id.to_string(), source)
                    }
                };
                (!self.posters.contains_key(&id)).then_some(Job { id, source })
            })
            .collect()
    }

    fn poster(&self, id: &str) -> Option<&image::Handle> {
        self.posters.get(id).and_then(Option::as_ref)
    }

    fn prepare(&mut self, runner: Option<String>) -> Task<Event> {
        let Some(candidate) = self
            .picked
            .and_then(|index| self.listing.as_ref()?.games.get(index))
        else {
            return Task::none();
        };
        let Standing::Ready(migration) = &candidate.standing else {
            return Task::none();
        };
        let source = candidate.source.clone();
        let heroic = candidate.heroic.clone();
        let migration = migration.clone();
        self.error = None;
        self.status = format!("Checking {}...", candidate.game.title);
        let generation = self.next_generation();
        let paths = self.paths.clone();
        task::blocking(
            move || prepare(&paths, source, heroic, migration, runner).map(Box::new),
            move |result| Event::Prepared(generation, result),
        )
    }

    fn prerequisite(&mut self, operation: Prerequisite) -> Task<Event> {
        let token = Token::new();
        self.token = Some(token.clone());
        self.status = operation.label();
        self.error = None;
        self.step = Step::Working;
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = task::blocking(
            move || {
                heroic::install_prerequisite(&paths, operation, &token, &|line| {
                    let _ = sender.unbounded_send(Event::Progress(generation, line));
                })
            },
            move |result| Event::PrerequisiteFinished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn start(&mut self) -> Task<Event> {
        let Some(prepared) = self.prepared.clone() else {
            return Task::none();
        };
        if self.busy() || self.waiting() || !prepared.ready() {
            return Task::none();
        }
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Working;
        self.error = None;
        self.status = format!("Migrating {}...", prepared.migration.game().title);
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = task::blocking(
            move || {
                run(
                    &paths,
                    &prepared,
                    &token,
                    |line| {
                        let _ = sender.unbounded_send(Event::Progress(generation, line));
                    },
                    import_extras,
                )
            },
            move |result| Event::Finished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Installing(instance, event) => match &mut self.install {
                Some(install) if instance == self.install_instance => {
                    let task = install.update(*event);
                    self.installing(task)
                }
                _ => Task::none(),
            },
            Event::Install(index) if self.step == Step::Games && !self.waiting() => {
                let Some(owned) = self
                    .listing
                    .as_ref()
                    .and_then(|listing| listing.owned.get(index))
                else {
                    return Task::none();
                };
                if !matches!(
                    owned.state,
                    OwnedState::NotInstalled | OwnedState::Unfinished
                ) {
                    return Task::none();
                }
                self.leave_prompt = false;
                self.error = None;
                self.notes.clear();
                let (install, task) = Install::new(self.paths.clone(), owned.offer.clone());
                self.install = Some(Box::new(install));
                self.install_instance = self.install_instance.wrapping_add(1);
                self.step = Step::Install;
                let instance = self.install_instance;
                task.map(move |event| Event::Installing(instance, Box::new(event)))
            }
            Event::Back => self.back(),
            Event::Discard => {
                self.leave_prompt = false;
                self.action = Some(Action::Choose);
                Task::none()
            }
            Event::KeepEditing => {
                self.leave_prompt = false;
                Task::none()
            }
            Event::Listed(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(listing) => {
                        self.listing = Some(listing);
                        return self.load_posters();
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Poster(generation, id, result) if generation == self.grid.poster_generation => {
                self.grid.progress.finish_one();
                self.posters.insert(id, result.ok());
                Task::none()
            }
            Event::PostersEnded(generation) if generation == self.grid.poster_generation => {
                self.grid.progress.finish_all();
                Task::none()
            }
            Event::ProgressDue(generation) => {
                self.grid.progress.reveal(generation);
                Task::none()
            }
            Event::Pick(index) if self.step == Step::Games && !self.waiting() => {
                let ready = self
                    .listing
                    .as_ref()
                    .and_then(|listing| listing.games.get(index))
                    .is_some_and(|game| matches!(game.standing, Standing::Ready(_)));
                if !ready {
                    return Task::none();
                }
                self.leave_prompt = false;
                self.picked = Some(index);
                self.grid.focus_visible = false;
                self.grid.focus = self.listing.as_ref().and_then(|listing| {
                    Self::entries(listing)
                        .iter()
                        .position(|entry| *entry == Entry::Heroic(index))
                });
                self.prepared = None;
                self.notes.clear();
                self.prepare(None)
            }
            Event::Prepared(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(prepared) => {
                        self.prepared = Some(*prepared);
                        self.step = Step::Review;
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Runner(name) if self.step == Step::Review && !self.waiting() => {
                self.prepare(Some(name))
            }
            Event::CopyRunner if self.step == Step::Review && !self.waiting() => {
                let Some(Prepared {
                    suggestion: Some(RunnerSuggestion::Copy(copy)),
                    ..
                }) = &self.prepared
                else {
                    return Task::none();
                };
                self.prerequisite(Prerequisite::CopyRunner(copy.clone()))
            }
            Event::DownloadRunner if self.step == Step::Review && !self.waiting() => {
                self.prerequisite(Prerequisite::Download(Provider::GeProton, None))
            }
            Event::DownloadBuild(component, version)
                if self.step == Step::Review && !self.waiting() =>
            {
                let provider = match component {
                    Component::Dxvk => Provider::Dxvk,
                    Component::Vkd3d => Provider::Vkd3d,
                };
                self.prerequisite(Prerequisite::Download(provider, version))
            }
            Event::PrerequisiteFinished(generation, result) if generation == self.generation => {
                self.token = None;
                self.status.clear();
                self.step = Step::Review;
                match result {
                    Ok(name) => {
                        self.notes.push(format!(
                            "Installed {name}. It stays installed if this import is canceled."
                        ));
                        // A copied or first runner becomes the pick; a build
                        // leaves the runner as it was.
                        let runner = self.prepared.as_ref().and_then(|prepared| {
                            (prepared.runner.is_empty()
                                || matches!(prepared.suggestion, Some(RunnerSuggestion::Copy(_))))
                            .then(|| name.clone())
                            .or_else(|| {
                                (!prepared.runner.is_empty()).then(|| prepared.runner.clone())
                            })
                        });
                        self.prepare(runner)
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Task::none()
                    }
                }
            }
            Event::Migrate if self.step == Step::Review => self.start(),
            Event::Cancel if self.step == Step::Working => {
                self.cancel(false);
                Task::none()
            }
            Event::Force if self.step == Step::Working => {
                self.cancel(true);
                Task::none()
            }
            Event::Progress(generation, line)
                if generation == self.generation && self.step == Step::Working =>
            {
                if !self.token.as_ref().is_some_and(Token::is_canceled) {
                    self.status = line;
                }
                Task::none()
            }
            Event::Finished(generation, result) if generation == self.generation => {
                self.token = None;
                self.status.clear();
                match result {
                    Ok(report) => {
                        self.report = Some(report);
                        self.step = Step::Finished;
                    }
                    Err(error) => {
                        // Nothing changed, so the review is still there to
                        // go back to.
                        self.error = Some(error);
                        self.step = Step::Review;
                    }
                }
                Task::none()
            }
            Event::BackToLibrary if self.step == Step::Finished => {
                self.exit();
                Task::none()
            }
            _ => Task::none(),
        }
    }

    #[cfg(test)]
    pub(crate) fn view(&self, palette: Palette) -> Element<'_, Event> {
        self.view_at(palette, 900.0)
    }

    /// `width` is the dialog's, which sets how many tiles fit across.
    pub(crate) fn view_at(&self, palette: Palette, width: f32) -> Element<'_, Event> {
        let finished = self.step == Step::Finished
            || self
                .install
                .as_ref()
                .is_some_and(|install| install.finished());
        let back = secondary_button("Back", palette)
            .on_press_maybe((!self.busy() && !finished).then_some(Event::Back));
        let mut page = column![]
            .spacing(12)
            .max_width(heroic::PAGE_MAX_WIDTH)
            .height(Fill);
        if let (Step::Install, Some(install)) = (self.step, &self.install) {
            let (body, footer) = install.view(column![].spacing(14).width(Fill), palette);
            let instance = self.install_instance;
            let body =
                Element::from(body).map(move |event| Event::Installing(instance, Box::new(event)));
            page = page
                .push(row![back].align_y(Center))
                .push(heroic::scroll_body(body, None, palette));
            if let Some(footer) = footer {
                page = page.push(heroic::pinned(
                    footer.map(move |event| Event::Installing(instance, Box::new(event))),
                    palette,
                ));
            }
            return heroic::page_frame(page);
        }
        let title = match self.step {
            Step::Games => "Choose a GOG game",
            Step::Review => "Review GOG migration",
            Step::Working => "Migrating from GOG",
            Step::Finished => "GOG migration finished",
            Step::Install => "Review GOG install",
        };
        let mut body = column![crate::ui::page_title(title, palette)]
            .spacing(14)
            .width(Fill);
        if !self.status.is_empty() {
            body = body.push(
                row![
                    crate::spinner::spinner(palette, 16.0),
                    text(&self.status).color(palette.muted)
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(palette.danger));
        }
        for note in &self.notes {
            body = body.push(text(note).size(13).color(palette.muted));
        }
        match self.step {
            Step::Games => {}
            Step::Review => body = self.review_view(body, palette),
            Step::Working | Step::Install => {}
            Step::Finished => {
                if let Some(report) = &self.report {
                    for line in &report.lines {
                        body = body.push(text(line));
                    }
                    for warning in &report.warnings {
                        body = body.push(text(warning).size(13).color(palette.warning));
                    }
                }
            }
        }
        if self.leave_prompt {
            page = page.push(text("Discard this GOG import draft?")).push(
                row![
                    secondary_button("Keep editing", palette).on_press(Event::KeepEditing),
                    secondary_button("Discard", palette).on_press(Event::Discard)
                ]
                .spacing(12),
            );
        }
        page = page.push(row![back].align_y(Center));
        // The title and notes scroll with the tiles, like the other import
        // grids.
        page = if self.step == Step::Games {
            page.push(self.games_view(body, palette, width))
        } else {
            page.push(heroic::scroll_body(body, None, palette))
        };
        let footer: Option<Element<'_, Event>> = match self.step {
            Step::Review => self.prepared.as_ref().map(|prepared| {
                accent_base("Migrate", palette.primary)
                    .on_press_maybe((prepared.ready() && !self.waiting()).then_some(Event::Migrate))
                    .into()
            }),
            Step::Working => self.token.as_ref().map(|token| {
                let (label, event) = if token.is_canceled() {
                    ("Force stop", Event::Force)
                } else {
                    ("Cancel", Event::Cancel)
                };
                secondary_button(label, palette).on_press(event).into()
            }),
            Step::Finished => Some(
                secondary_button("Back to library", palette)
                    .on_press(Event::BackToLibrary)
                    .into(),
            ),
            // Only while the art is still being prepared, so the grid keeps
            // its room otherwise.
            Step::Games => self.grid.progress_bar(palette),
            Step::Install => None,
        };
        if let Some(footer) = footer {
            page = page.push(heroic::pinned(footer, palette));
        }
        heroic::page_frame(page)
    }

    fn games_view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Event>,
        palette: Palette,
        width: f32,
    ) -> Element<'a, Event> {
        let Some(listing) = &self.listing else {
            return heroic::scroll_body(body, None, palette);
        };
        for issue in &listing.issues {
            body = body.push(text(issue).size(13).color(palette.warning));
        }
        if let Some(note) = &listing.owned_note {
            body = body.push(text(note).size(13).color(palette.muted));
        }
        let entries = Self::entries(listing);
        if entries.is_empty() {
            let empty = if listing.owned_note.is_some() {
                "Heroic has no GOG games."
            } else {
                "You don't own any GOG games."
            };
            return heroic::scroll_body(body.push(text(empty).color(palette.muted)), None, palette);
        }
        let tiles = entries.iter().enumerate().map(|(position, entry)| {
            let (id, title, reason, note, event) = match *entry {
                Entry::Heroic(index) => {
                    let candidate = &listing.games[index];
                    let (reason, note) = match &candidate.standing {
                        Standing::Ready(_) => (None, Some("In Heroic".to_owned())),
                        Standing::Already { environment } => {
                            (Some(format!("Already migrated as {environment}.")), None)
                        }
                        Standing::Failed(reason) => (Some(reason.clone()), None),
                    };
                    (
                        candidate.game.id.clone(),
                        candidate.game.title.as_str(),
                        reason,
                        note,
                        Event::Pick(index),
                    )
                }
                Entry::Owned(index) => {
                    let owned = &listing.owned[index];
                    let (reason, note) = match &owned.state {
                        OwnedState::NotInstalled => (None, Some("Not installed".to_owned())),
                        OwnedState::Unfinished => (None, Some("Install didn't finish".to_owned())),
                        OwnedState::Installed(name) => {
                            (Some(format!("Installed as {name}.")), None)
                        }
                        OwnedState::Imported(name) => {
                            (Some(format!("Already migrated as {name}.")), None)
                        }
                    };
                    (
                        owned.offer.gog_id.to_string(),
                        owned.offer.title.as_str(),
                        reason,
                        note,
                        Event::Install(index),
                    )
                }
            };
            (
                Tile {
                    title,
                    poster: self.poster(&id),
                    reason,
                    note,
                    chosen: false,
                    focus_ring: self.grid.focus_ring(position),
                    // Nearly every game here isn't installed, so
                    // dimming those like the library would wash out
                    // the whole grid.
                    dimmed: false,
                },
                event,
            )
        });
        self.grid.view(body.into(), tiles, width, palette)
    }

    fn review_view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Event>,
        palette: Palette,
    ) -> iced::widget::Column<'a, Event> {
        let Some(prepared) = &self.prepared else {
            return body;
        };
        let mut summary = column![crate::ui::section_heading(
            &prepared.migration.game().title,
            palette
        )]
        .spacing(8);
        for (label, value) in review_rows(&prepared.migration, prepared.heroic.as_deref()) {
            summary = summary.push(
                row![
                    text(label).color(palette.muted).width(120),
                    text(value).width(Fill)
                ]
                .spacing(12),
            );
        }
        if matches!(prepared.migration.target(), Target::Windows { .. }) {
            summary = summary.push(
                row![
                    text("Runner").color(palette.muted).width(120),
                    pick_list(
                        prepared.runners.clone(),
                        (!prepared.runner.is_empty())
                            .then(|| Choice::find(&prepared.runners, &prepared.runner)),
                        |choice| Event::Runner(choice.name),
                    )
                    .placeholder("Choose a runner")
                    .wineo_style(palette),
                ]
                .spacing(12)
                .align_y(Center),
            );
        }
        body = body.push(container(summary).padding(16).width(Fill).style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(12).width(1).color(palette.border))
        }));
        match &prepared.suggestion {
            Some(RunnerSuggestion::Copy(copy)) => {
                body = body.push(
                    text(format!(
                        "Heroic has runner {} at {}",
                        copy.name,
                        copy.source.display()
                    ))
                    .color(palette.muted),
                );
                body = body.push(
                    secondary_button("Copy Heroic runner", palette)
                        .on_press_maybe((!self.waiting()).then_some(Event::CopyRunner)),
                );
            }
            Some(RunnerSuggestion::Choose {
                proton,
                copy_blocked,
            }) if prepared.runner.is_empty() => {
                body = body.push(
                    text(format!(
                        "Choose a compatible {} runner",
                        if *proton { "Proton" } else { "Wine" }
                    ))
                    .color(palette.muted),
                );
                if let Some(reason) = copy_blocked {
                    body = body.push(
                        text(format!("Heroic runner cannot be copied: {reason}"))
                            .color(palette.warning),
                    );
                }
            }
            _ => {}
        }
        if matches!(prepared.migration.target(), Target::Windows { .. })
            && prepared.runner.is_empty()
        {
            body = body.push(
                secondary_button("Download latest GE-Proton", palette)
                    .on_press_maybe((!self.waiting()).then_some(Event::DownloadRunner)),
            );
        }
        if let Some(plan) = windows_plan(&prepared.migration) {
            for warning in heroic::review_warnings(&plan.warnings()) {
                body = body.push(text(warning).size(13).color(palette.warning));
            }
        }
        if let Some((component, version)) = &prepared.missing_build {
            body = body.push(
                text(format!("{} isn't installed yet.", component.label()))
                    .size(13)
                    .color(palette.warning),
            );
            body = body.push(
                secondary_button(format!("Download {} build", component.label()), palette)
                    .on_press_maybe(
                        (!self.waiting())
                            .then(|| Event::DownloadBuild(*component, version.clone())),
                    ),
            );
        }
        if let Some(blocking) = &prepared.blocking {
            body = body.push(text(blocking).color(palette.danger));
        }
        body
    }
}

fn windows_plan(migration: &Migration) -> Option<&heroic_import::GamePlan> {
    match migration.target() {
        Target::Windows { plan, .. } => plan.as_ref(),
        _ => None,
    }
}

/// The review's rows, the same ones `wineo import gog` prints, except the
/// runner, which the review picks below them.
fn review_rows(migration: &Migration, heroic: Option<&str>) -> Vec<(&'static str, String)> {
    let show = |path: &Path| path.display().to_string();
    migration
        .review_rows(heroic.map(str::to_owned), show)
        .into_iter()
        .filter_map(|(label, value)| {
            let label = match label {
                ReviewLabel::Environment => "Environment",
                ReviewLabel::Folder => "Folder",
                ReviewLabel::Heroic => "Heroic",
                ReviewLabel::Track => "Track",
                ReviewLabel::Version => "Version",
                ReviewLabel::Language => "Language",
                ReviewLabel::Dlc => "DLC",
                // One DLC per row; the rest have no label, so they sit under
                // the first.
                ReviewLabel::MoreDlc => "",
                ReviewLabel::Runner => return None,
                ReviewLabel::Prefix => "Prefix",
                ReviewLabel::FolderState => "Folder state",
            };
            Some((label, value))
        })
        .collect()
}

fn load_listing(paths: &Paths) -> Result<Listing, String> {
    if paths.portable_root().is_some() {
        return Err(MigrateError::Portable.to_string());
    }
    let sources = heroic_import::sources(paths, None).map_err(|error| error.to_string())?;
    let mut listing = listing_from(paths, &sources);
    let keyring = SecretServiceKeyring::gog();
    match wineo_core::gog::client_from_env() {
        Ok(client) => add_owned(paths, &keyring, client, &mut listing),
        Err(error) => listing.owned_note = Some(error.to_string()),
    }
    Ok(listing)
}

/// Adds the owned games Heroic doesn't have to `listing`, with what wineo
/// has of each, the way `import gog --list` tells them apart. Logged out, or
/// when GOG can't be asked, only the note says so: Heroic's games still
/// migrate without a login.
pub(crate) fn add_owned(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    listing: &mut Listing,
) {
    let mut session = match Session::open(paths, keyring, client) {
        Ok(Some(session)) => session,
        Ok(None) => {
            listing.owned_note = Some(OWNED_LOGGED_OUT.to_owned());
            return;
        }
        Err(error) => {
            listing.owned_note = Some(format!("Couldn't read the GOG login: {error}"));
            return;
        }
    };
    let owned = match wineo_core::gog::owned::load(&mut session, false) {
        Ok(owned) => owned,
        Err(error) => {
            listing.owned_note = Some(format!("Couldn't list the games you own: {error}"));
            return;
        }
    };
    listing.owned_note = owned.warning;
    let in_heroic: BTreeSet<&str> = listing
        .games
        .iter()
        .map(|candidate| candidate.game.id.as_str())
        .collect();
    let recorded = wineo_core::gog::update::recorded_games(paths);
    let configless = configless_installs(paths);
    let mut games: Vec<Owned> = owned
        .games
        .into_iter()
        .filter(|game| !in_heroic.contains(game.id.to_string().as_str()))
        .map(|game| {
            let state = match recorded.get(&game.id) {
                Some(name) => recorded_state(paths, name, game.id),
                None if configless.contains_key(&game.id) => OwnedState::Unfinished,
                None => OwnedState::NotInstalled,
            };
            let unfinished = match (&state, recorded.get(&game.id)) {
                (OwnedState::Unfinished, Some(name)) => Some(name.clone()),
                (OwnedState::Unfinished, None) => configless.get(&game.id).cloned(),
                _ => None,
            };
            Owned {
                offer: Offer {
                    gog_id: game.id,
                    title: game.title,
                    linux: game.works_on.linux,
                    windows: game.works_on.windows,
                    unfinished,
                },
                state,
            }
        })
        .collect();
    games.sort_by_cached_key(|owned| (owned.offer.title.to_lowercase(), owned.offer.gog_id));
    listing.owned = games;
}

/// What `name`'s folder for game `id` says: an install to finish, one wineo
/// made, or a migrated game. A state that can't be read reads as migrated.
fn recorded_state(paths: &Paths, name: &str, id: u64) -> OwnedState {
    let state = folder::find(paths, name, &id.to_string()).ok().flatten();
    let installing = state
        .as_ref()
        .and_then(|state| state.journal.as_ref())
        .is_some_and(|journal| journal.kind == JournalKind::Install);
    let installed = state.as_ref().is_some_and(FolderState::installed_by_wineo);
    if installing {
        OwnedState::Unfinished
    } else if installed {
        OwnedState::Installed(name.to_owned())
    } else {
        OwnedState::Imported(name.to_owned())
    }
}

/// Installs that stopped before their environment's config was written, by
/// game id, with the name they'll get.
fn configless_installs(paths: &Paths) -> BTreeMap<u64, String> {
    let mut found = BTreeMap::new();
    for (_, state) in folder::list(paths).unwrap_or_default() {
        let Ok(state) = state else { continue };
        let unfinished = state.committed.is_none()
            && state
                .journal
                .as_ref()
                .is_some_and(|journal| journal.kind == JournalKind::Install);
        if let (Some(owner), Some(id)) = (&state.owner, state.gog_id())
            && unfinished
            && !paths.environment_config(owner).exists()
        {
            found.entry(id).or_insert_with(|| owner.clone());
        }
    }
    found
}

/// Every GOG game the Heroic configurations installed, planned so the page
/// can mark the ones already migrated. Planning only reads.
pub(crate) fn listing_from(paths: &Paths, sources: &[HeroicSource]) -> Listing {
    let found = gog_games(sources);
    let mut listing = Listing::default();
    listing
        .issues
        .extend(found.problems.into_iter().map(|problem| match problem.id {
            Some(id) => format!("GOG game {id}: {}", problem.message),
            None => format!("Heroic: {}", problem.message),
        }));
    // A game the configs put in different folders is never dropped quietly.
    listing
        .issues
        .extend(found.conflicts.iter().map(|conflict| {
            let count = match conflict.places.len() {
                2 => "two".to_owned(),
                count => count.to_string(),
            };
            format!(
                "GOG game {} is installed in {count} places: {}.",
                conflict.id,
                conflict.places_text(|folder| folder.display().to_string())
            )
        }));
    for entry in found.games {
        let source = entry.source;
        let heroic = migrate::heroic_note(&entry, |path| path.display().to_string());
        let standing = match migrate::plan(paths, source, &entry.game.id) {
            Ok(Plan::Migrate(migration)) => Standing::Ready(Box::new(migration)),
            Ok(Plan::AlreadyMigrated { environment, .. }) => Standing::Already { environment },
            Ok(Plan::NotInstalled { .. }) => continue,
            Err(error) => Standing::Failed(format!("{error:#}")),
        };
        listing.games.push(Candidate {
            source: source.clone(),
            game: entry.game,
            standing,
            heroic,
        });
    }
    listing
        .games
        .sort_by_cached_key(|candidate| candidate.game.title.to_lowercase());
    listing
}

/// Settles what the review shows: the runner for a new Windows environment,
/// as Heroic import picks it, and whatever would stop `apply`.
fn prepare(
    paths: &Paths,
    source: HeroicSource,
    heroic: Option<String>,
    mut migration: Box<Migration>,
    runner: Option<String>,
) -> Result<Prepared, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let mut prepared = Prepared {
        source,
        heroic,
        migration: Box::new(Migration::clone(&migration)),
        runners: Vec::new(),
        runner: String::new(),
        suggestion: None,
        missing_build: None,
        blocking: None,
    };
    if let Target::Windows { imported, .. } = migration.target() {
        let imported = imported.clone();
        let (runners, choices) = heroic::runner_choices(paths, &global, true)?;
        let (suggestion, suggested) =
            heroic::suggest(paths, &global, &runners, &choices, &imported)?;
        prepared.runners = choices;
        prepared.suggestion = Some(suggestion);
        prepared.runner = runner.unwrap_or(suggested);
        if !prepared.runner.is_empty()
            && let Err(error) = migration.plan_runner(paths, &global, &prepared.runner, None, None)
        {
            prepared.blocking = Some(format!("{error:#}"));
        }
    }
    // New environments get desktop entries like a Heroic import's. GOG games
    // only migrate into the host root, which is where entries are written.
    let desktop = paths.portable_root().is_none();
    if let Some(plan) = migration.native_plan_mut() {
        plan.set_desktop_enabled(desktop);
    }
    if let Some(plan) = migration.game_plan_mut() {
        plan.set_desktop_enabled(desktop);
        plan.resolve(Policy::Defaults);
        if prepared.blocking.is_none()
            && let Some(finding) = plan.blocking().next()
        {
            prepared.blocking = Some(finding.issue.message.clone());
        }
        match plan.missing_build(paths, &global) {
            Ok(missing) => {
                prepared.missing_build = missing.map(|missing| (missing.component, missing.version))
            }
            Err(error) => {
                prepared.blocking.get_or_insert_with(|| error.to_string());
            }
        }
    }
    if prepared.blocking.is_none() && migration.name_taken(paths) {
        let name = migration.environment().unwrap_or_default();
        prepared.blocking = Some(wineo_core::Error::EnvironmentExists(name.to_owned()).to_string());
    }
    prepared.migration = migration;
    Ok(prepared)
}

/// Migrates, then imports GOG's playtime and art and writes desktop entries.
/// A migration that finished stays done even when canceled at the end; the
/// extras after it stop at the cancel and say what they skipped.
fn run(
    paths: &Paths,
    prepared: &Prepared,
    token: &Token,
    mut progress: impl FnMut(String),
    extras: impl FnOnce(&Paths, &GlobalConfig, &str, u64, &str, &Token) -> Extras,
) -> Result<Report, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let migration = &prepared.migration;
    let title = migration.game().title.clone();
    let outcome = migrate::apply(paths, &global, &prepared.source, migration, token, |_| {})
        .map_err(|error| {
            if token.is_canceled() {
                "Import canceled.".to_owned()
            } else {
                format!("{error:#}")
            }
        })?;
    let mut report = Report::default();
    let migrated = match outcome {
        Outcome::AlreadyMigrated {
            environment,
            folder,
        } => {
            report.lines.push(format!(
                "Already migrated. {environment} owns {}.",
                folder.display()
            ));
            report.environment = Some(environment);
            return Ok(report);
        }
        Outcome::Migrated(migrated) => migrated,
    };
    report.environment = Some(migrated.environment.clone());
    if let Some(registered) = &migrated.registered {
        report
            .warnings
            .extend(heroic::playtime_note(&registered.playtime));
    }
    let icon = if token.is_canceled() {
        report
            .warnings
            .push("Skipped GOG playtime and art: canceled.".to_owned());
        false
    } else {
        progress(format!("Adding GOG playtime and art for {title}..."));
        match migration.game().id.parse::<u64>() {
            Ok(gog_id) => {
                let extras = extras(paths, &global, &migrated.environment, gog_id, &title, token);
                report.lines.extend(extras.lines);
                report.warnings.extend(extras.notes);
                extras.icon
            }
            Err(_) => false,
        }
    };
    // Last, so a new icon is in the entries.
    match &migrated.registered {
        Some(registered) => match registered.finish(paths, &global) {
            Ok(changes) => report
                .lines
                .extend(changes.iter().map(crate::settings::desktop_line)),
            Err(error) => report.warnings.push(format!("Desktop entry: {error}")),
        },
        None if icon && wineo_core::desktop::supported(paths).is_ok() => {
            if let Err(error) = wineo_core::desktop::apply(paths, &migrated.environment, false) {
                report
                    .warnings
                    .push(format!("Couldn't update desktop entries: {error:#}"));
            }
        }
        None => {}
    }
    report
        .lines
        .insert(0, format!("Migrated {title} as {}.", migrated.environment));
    if matches!(migrated.state, StateChange::Reclaim { journal: true }) {
        report.lines.push(format!(
            "Its last update was interrupted. Use Finish update on {} in the library.",
            migrated.environment
        ));
    }
    report.lines.push(HEROIC_NOTE.to_owned());
    Ok(report)
}

/// What importing GOG's extras said.
#[derive(Debug, Default)]
pub(super) struct Extras {
    pub(super) lines: Vec<String>,
    pub(super) notes: Vec<String>,
    icon: bool,
}

/// Imports GOG's playtime and art, like `import gog` does after migrating.
/// Every problem is a note; nothing here fails.
pub(super) fn import_extras(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &str,
    gog_id: u64,
    title: &str,
    token: &Token,
) -> Extras {
    let mut out = Extras::default();
    let keyring = SecretServiceKeyring::gog();
    // Playtime needs the login and art doesn't, so a login that can't be read
    // only costs the playtime.
    let mut session = match wineo_core::gog::client_from_env() {
        Ok(client) => match Session::open(paths, &keyring, client) {
            Ok(session) => session,
            Err(error) => {
                out.notes.push(format!("Skipped GOG playtime: {error}"));
                None
            }
        },
        Err(error) => {
            out.notes.push(format!("Skipped GOG playtime: {error}"));
            None
        }
    };
    let transport = match Https::new() {
        Ok(transport) => transport,
        Err(error) => {
            out.notes
                .push(format!("Skipped GOG playtime and art: {error:#}"));
            return out;
        }
    };
    let key = match steamgriddb::find_key(
        paths,
        |name| std::env::var_os(name),
        &SecretServiceKeyring::steamgriddb(),
    ) {
        Ok(key) => key.map(|key| key.value),
        Err(error) => {
            out.notes.push(format!("Skipped SteamGridDB art: {error}"));
            None
        }
    };
    let language = steamgriddb::language(|name| std::env::var_os(name));
    let kinds = global.art.fetch.kinds();
    let request = Request {
        name: environment,
        gog_id,
        title,
        kinds: &kinds,
        transport: &transport,
        steamgriddb_key: key.as_deref(),
        language: &language,
    };
    match extras::import(paths, session.as_mut(), &request, token) {
        Ok(report) => {
            out.lines = report.lines();
            out.notes.extend(report.notes.iter().cloned());
            out.icon = report.art.iter().any(|filled| filled.kind == Kind::Icon);
        }
        Err(error) => out
            .notes
            .push(format!("Couldn't import GOG playtime and art: {error:#}")),
    }
    out
}

#[cfg(test)]
mod tests;
