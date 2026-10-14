//! Import from GOG, like `wineo import gog`: every game you own, to install
//! (see `gog_install`), and the GOG games Heroic installed, to move into
//! wineo. A migration is reviewed, applied, then adds GOG's playtime and
//! art, which never fails the migration.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

use iced::futures::channel::mpsc::unbounded;
use iced::keyboard::{self, Key, key::Named};
use iced::widget::{Row, column, container, image, pick_list, row, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::art::{Kind, Source};
use wineo_core::cancel::Token;
use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Https, Provider};
use wineo_core::gog::account::Session;
use wineo_core::gog::api::Client;
use wineo_core::gog::extras::{self, Request};
use wineo_core::gog::folder::{self, JournalKind, Source as FolderSource, Track};
use wineo_core::gog::migrate::{self, MigrateError, Migration, Outcome, Plan, StateChange, Target};
use wineo_core::heroic::{HeroicGogEntry, HeroicGogGame, HeroicOrigin, HeroicSource, gog_games};
use wineo_core::heroic_import::{self, Policy, RunnerSuggestion};
use wineo_core::key_store::Keyring;
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb;

use super::Action;
use super::gog_install::{self, Install, Leave, Offer};
use super::gog_posters::{self, Job, Network, Source as PosterSource};
use super::heroic::{self, Prerequisite, Tile};
use crate::runner_label::Choice;
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
    /// The tile the keyboard is on, in grid order.
    focus: Option<usize>,
    /// Whether `focus` came from the keyboard, which draws the ring.
    focus_visible: bool,
    /// Decoded posters by tile id. None when a game's poster failed to load,
    /// which keeps its text tile.
    posters: HashMap<String, Option<image::Handle>>,
    poster_generation: u64,
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
        let mut page = Self {
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
            focus: None,
            focus_visible: false,
            posters: HashMap::new(),
            poster_generation: next_poster_load(),
            poster_token: None,
        };
        let task = page.list();
        (page, task)
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
            .zip(self.focus)
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
        let columns = heroic::grid_columns(width);
        match key.as_ref() {
            Key::Named(
                named @ (Named::ArrowLeft | Named::ArrowRight | Named::ArrowUp | Named::ArrowDown),
            ) => {
                self.focus = heroic::move_focus(self.focus, named, columns, count);
                self.focus_visible = true;
                let index = self.focus?;
                let columns = columns.max(1);
                Some(heroic::reveal_row(
                    GRID_ID,
                    index / columns,
                    count.div_ceil(columns),
                ))
            }
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
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || load_listing(&paths))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |result| Event::Listed(generation, result),
        )
    }

    /// Invalidates the poster load in flight, so its replies are dropped and
    /// its worker stops at the next game.
    fn stop_posters(&mut self) {
        if let Some(token) = self.poster_token.take() {
            token.cancel();
        }
        self.poster_generation = next_poster_load();
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
        let generation = self.poster_generation;
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = Task::future(async move {
            let _ = tokio::task::spawn_blocking(move || {
                gog_posters::run(&paths, jobs, &token, Network::open, |id, poster| {
                    let _ = sender.unbounded_send(Event::Poster(generation, id, poster));
                });
            })
            .await;
        });
        Task::batch([Task::run(receiver, |event| event), work.discard()])
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
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    prepare(&paths, source, heroic, migration, runner).map(Box::new)
                })
                .await
                .map_err(|error| error.to_string())?
            },
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
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    heroic::install_prerequisite(&paths, operation, &token, &|line| {
                        let _ = sender.unbounded_send(Event::Progress(generation, line));
                    })
                })
                .await
                .map_err(|error| error.to_string())?
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
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    run(
                        &paths,
                        &prepared,
                        &token,
                        |line| {
                            let _ = sender.unbounded_send(Event::Progress(generation, line));
                        },
                        import_extras,
                    )
                })
                .await
                .map_err(|error| error.to_string())?
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
            Event::Poster(generation, id, result) if generation == self.poster_generation => {
                self.posters.insert(id, result.ok());
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
                self.focus_visible = false;
                self.focus = self.listing.as_ref().and_then(|listing| {
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
                page = page.push(pinned(
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
            Step::Games | Step::Install => None,
        };
        if let Some(footer) = footer {
            page = page.push(pinned(footer, palette));
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
        let columns = heroic::grid_columns(width);
        let tiles: Vec<Element<'a, Event>> = entries
            .iter()
            .enumerate()
            .map(|(position, entry)| {
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
                            OwnedState::Unfinished => {
                                (None, Some("Install didn't finish".to_owned()))
                            }
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
                heroic::tile(
                    Tile {
                        title,
                        poster: self.poster(&id),
                        reason,
                        note,
                        chosen: false,
                        focus_ring: self.focus_visible && self.focus == Some(position),
                        // Nearly every game here isn't installed, so
                        // dimming those like the library would wash out
                        // the whole grid.
                        dimmed: false,
                    },
                    palette,
                    event,
                )
            })
            .collect();
        let mut rows = column![].spacing(heroic::TILE_GAP);
        let mut tiles = tiles.into_iter().peekable();
        while tiles.peek().is_some() {
            rows = rows
                .push(Row::with_children(tiles.by_ref().take(columns)).spacing(heroic::TILE_GAP));
        }
        heroic::grid_scroll(GRID_ID, body.into(), rows.into(), palette)
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

/// A step's action row, pinned under the scrolling content.
fn pinned<'a>(footer: Element<'a, Event>, palette: Palette) -> Element<'a, Event> {
    container(footer)
        .padding(12)
        .width(Fill)
        .style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(8).width(1).color(palette.border))
        })
        .into()
}

fn windows_plan(migration: &Migration) -> Option<&heroic_import::GamePlan> {
    match migration.target() {
        Target::Windows { plan, .. } => plan.as_ref(),
        _ => None,
    }
}

/// The review's rows, the same ones `wineo import gog` prints.
fn review_rows(migration: &Migration, heroic: Option<&str>) -> Vec<(&'static str, String)> {
    let committed = migration.committed();
    let kind = match migration.target() {
        Target::Existing { .. } => "existing",
        Target::Windows { .. } => "new Windows environment",
        Target::Native(_) => "new native environment",
    };
    let environment = migration.environment().unwrap_or("?");
    let version = committed
        .products
        .iter()
        .find(|product| product.id == committed.gog_id)
        .and_then(|product| match committed.track {
            Track::Installer => product.version.clone(),
            Track::Galaxy => product.build.clone().map(|build| format!("build {build}")),
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let mut rows = vec![
        ("Environment", format!("{environment} ({kind})")),
        ("Folder", migration.folder().display().to_string()),
    ];
    if let Some(heroic) = heroic {
        rows.push(("Heroic", heroic.to_owned()));
    }
    rows.extend([
        (
            "Track",
            match committed.track {
                Track::Installer => "installer",
                Track::Galaxy => "galaxy",
            }
            .to_owned(),
        ),
        ("Version", version),
        ("Language", committed.language.clone()),
    ]);
    // One DLC per row; the rest have no label, so they sit under the first.
    let mut dlc = migration.dlc_labels().into_iter();
    if let Some(first) = dlc.next() {
        rows.push(("DLC", first));
        rows.extend(dlc.map(|label| ("", label)));
    }
    if let Some(plan) = windows_plan(migration) {
        rows.push((
            "Prefix",
            format!(
                "{} (Heroic's, kept where it is)",
                plan.destination().display()
            ),
        ));
    }
    if let StateChange::Reclaim { journal } = migration.state() {
        rows.push((
            "Folder state",
            if journal {
                "kept from before, with an interrupted update"
            } else {
                "kept from before"
            }
            .to_owned(),
        ));
    }
    rows
}

fn load_listing(paths: &Paths) -> Result<Listing, String> {
    if paths.portable_root().is_some() {
        return Err(MigrateError::Portable.to_string());
    }
    let sources = heroic_import::sources(paths, None).map_err(|error| error.to_string())?;
    let mut listing = listing_from(paths, &sources);
    let keyring = SecretServiceKeyring::for_spec(&wineo_core::gog::account::KEY_SPEC);
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
    let recorded = recorded_games(paths);
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

/// The environment that records each GOG game, by game id. A config that
/// can't be read doesn't say.
fn recorded_games(paths: &Paths) -> BTreeMap<u64, String> {
    let mut recorded = BTreeMap::new();
    for name in wineo_core::environment::names(paths).unwrap_or_default() {
        if let Ok(Some(id)) = wineo_core::gog::update::recorded_gog_id(paths, &name)
            && let Ok(id) = id.parse()
        {
            recorded.entry(id).or_insert(name);
        }
    }
    recorded
}

/// What `name`'s folder for game `id` says: an install to finish, one wineo
/// made, or a migrated game. A state that can't be read reads as migrated.
fn recorded_state(paths: &Paths, name: &str, id: u64) -> OwnedState {
    let state = folder::find(paths, name, &id.to_string()).ok().flatten();
    let installing = state
        .as_ref()
        .and_then(|state| state.journal.as_ref())
        .is_some_and(|journal| journal.kind == JournalKind::Install);
    let installed = state
        .and_then(|state| state.committed)
        .is_some_and(|committed| committed.source == Some(FolderSource::Install));
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
        let heroic = heroic_row(&entry);
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

/// Which Heroic a migration reads from, in `import gog`'s words, when
/// another config lists the same game and was set aside.
fn heroic_row(entry: &HeroicGogEntry) -> Option<String> {
    if entry.also_listed_by.is_empty() {
        return None;
    }
    let used = entry.source.origin();
    let others: Vec<String> = entry
        .also_listed_by
        .iter()
        .map(|other| {
            // Two configs of one kind need the folder to tell them apart.
            if other.origin() == used {
                format!(
                    "{} ({})",
                    other.origin().label(),
                    other.config_dir().display()
                )
            } else {
                other.origin().label().to_owned()
            }
        })
        .collect();
    let name = match used {
        HeroicOrigin::Native => "native",
        HeroicOrigin::Flatpak => "Flatpak",
        HeroicOrigin::Explicit => "Heroic",
    };
    Some(format!(
        "{name} ({}), also listed by {}",
        entry.source.config_dir().display(),
        others.join(" and ")
    ))
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
    let keyring = SecretServiceKeyring::for_spec(&wineo_core::gog::account::KEY_SPEC);
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
            out.lines = extras_lines(&report);
            out.notes.extend(report.notes.iter().cloned());
            out.icon = report.art.iter().any(|filled| filled.kind == Kind::Icon);
        }
        Err(error) => out
            .notes
            .push(format!("Couldn't import GOG playtime and art: {error:#}")),
    }
    out
}

/// What the extras report says happened, in `import gog`'s words.
fn extras_lines(report: &extras::Report) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(playtime) = report.playtime.filter(|playtime| playtime.raised()) {
        lines.push(format!(
            "Imported {} of GOG playtime.",
            duration(playtime.gog_seconds)
        ));
    }
    let gog: fn(&Source) -> bool = |source| matches!(source, Source::Gog(_));
    let grid: fn(&Source) -> bool = |source| matches!(source, Source::SteamGridDb(_));
    for (name, wanted) in [("GOG", gog), ("SteamGridDB", grid)] {
        let kinds: Vec<&str> = [Kind::Grid, Kind::Hero, Kind::Logo, Kind::Icon]
            .into_iter()
            .filter(|kind| {
                report
                    .art
                    .iter()
                    .any(|filled| filled.kind == *kind && wanted(&filled.source))
            })
            .map(|kind| match kind {
                Kind::Grid => "poster",
                other => other.as_str(),
            })
            .collect();
        if !kinds.is_empty() {
            lines.push(format!("Added {name} art: {}.", kinds.join(", ")));
        }
    }
    lines
}

/// Whole hours, or minutes for under an hour.
fn duration(seconds: i64) -> String {
    if seconds >= 3600 {
        format!("{} h", (seconds + 1800) / 3600)
    } else {
        format!("{} min", seconds / 60)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_script::write_script;
    use std::ffi::OsString;
    use std::fs;

    use std::path::PathBuf;

    use wineo_core::gog::extras::{Filled, Playtime};

    use super::*;

    const RIMWORLD: &str = "1094900565";
    const STARDEW: &str = "1453375253";
    const DISCO: &str = "1771589310";

    struct Fixture {
        root: tempfile::TempDir,
        paths: Paths,
        source: HeroicSource,
        installed: Vec<serde_json::Value>,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().to_owned();
            let paths =
                Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
                    .unwrap();
            let heroic = root.path().join("heroic");
            for dir in ["GamesConfig", "gog_store", "store_cache"] {
                fs::create_dir_all(heroic.join(dir)).unwrap();
            }
            fs::write(
                heroic.join("config.json"),
                r#"{"defaultSettings":{"wineVersion":{"type":"wine","name":"System","bin":"wine"}}}"#,
            )
            .unwrap();
            fs::write(
                heroic.join("store_cache/gog_library.json"),
                format!(
                    r#"{{"games":[{{"runner":"gog","app_name":"{RIMWORLD}","title":"RimWorld"}},{{"runner":"gog","app_name":"{STARDEW}","title":"Stardew Valley"}},{{"runner":"gog","app_name":"{DISCO}","title":"Disco Elysium"}}],"__timestamp":1}}"#
                ),
            )
            .unwrap();
            let source = HeroicSource::from_config_dir_with_home(heroic, root.path().to_owned());
            let fixture = Self {
                root,
                paths,
                source,
                installed: Vec::new(),
            };
            fixture.write_installed();
            fixture
        }

        fn write_installed(&self) {
            fs::write(
                self.source.config_dir().join("gog_store/installed.json"),
                serde_json::json!({ "installed": self.installed }).to_string(),
            )
            .unwrap();
        }

        fn install(&mut self, entry: serde_json::Value) {
            self.installed.push(entry);
            self.write_installed();
        }

        /// A native game, its `gameinfo` naming `id`.
        fn native(&mut self, id: &str, title: &str, dlc: &[u64]) -> PathBuf {
            let dir = self.root.path().join("Games").join(title);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("gameinfo"),
                format!("{title}\n1.6.4871 rev588\n93132\nen-US\n{id}\n{id}\n59819264299049740\n"),
            )
            .unwrap();
            let start = dir.join("start.sh");
            write_script(&start, "#!/bin/sh\n");
            self.install(serde_json::json!({
                "platform": "linux", "executable": "", "install_path": dir,
                "install_size": "5.03 GiB", "is_dlc": false, "version": "1.6.4871 rev588",
                "appName": id, "installedDLCs": dlc,
                "language": "en", "versionEtag": "", "buildId": "", "pinnedVersion": false,
            }));
            dir
        }

        fn disco(&mut self) {
            let dir = self.root.path().join("Games/Disco Elysium");
            fs::create_dir_all(dir.join("bin")).unwrap();
            fs::write(dir.join("Disco Elysium.exe"), "game").unwrap();
            fs::write(
                dir.join(format!("goggame-{DISCO}.info")),
                serde_json::json!({
                    "gameId": DISCO, "rootGameId": DISCO, "buildId": "59781086834418085",
                    "playTasks": [{
                        "isPrimary": true, "type": "FileTask", "path": "Disco Elysium.exe",
                        "workingDir": "bin", "category": "game",
                    }],
                })
                .to_string(),
            )
            .unwrap();
            let prefix = self.root.path().join("Prefixes/Disco");
            fs::create_dir_all(prefix.join("drive_c")).unwrap();
            fs::write(
                prefix.join("system.reg"),
                "WINE REGISTRY Version 2\n#arch=win64\n",
            )
            .unwrap();
            fs::write(
                self.source
                    .config_dir()
                    .join(format!("GamesConfig/{DISCO}.json")),
                serde_json::json!({
                    DISCO: {
                        "winePrefix": prefix,
                        "wineVersion": {"type": "wine", "name": "System", "bin": "wine"},
                    }
                })
                .to_string(),
            )
            .unwrap();
            self.install(serde_json::json!({
                "platform": "windows", "executable": "", "install_path": dir,
                "install_size": "9.57 GiB", "is_dlc": false, "version": "",
                "appName": DISCO, "installedDLCs": [], "language": "en-US",
                "versionEtag": "", "buildId": "59781086834418085", "pinnedVersion": false,
            }));
        }

        fn listing(&self) -> Listing {
            listing_from(&self.paths, std::slice::from_ref(&self.source))
        }

        fn prepared(&self, id: &str) -> Prepared {
            let candidate = self
                .listing()
                .games
                .into_iter()
                .find(|candidate| candidate.game.id == id)
                .unwrap();
            let Standing::Ready(migration) = candidate.standing else {
                panic!("{id} isn't ready");
            };
            prepare(
                &self.paths,
                candidate.source,
                candidate.heroic,
                migration,
                None,
            )
            .unwrap()
        }

        fn page(&self) -> Page {
            let (mut page, _) = Page::new(self.paths.clone());
            let generation = page.generation;
            let _ = page.update(Event::Listed(generation, Ok(self.listing())));
            page
        }
    }

    fn no_extras(_: &Paths, _: &GlobalConfig, _: &str, _: u64, _: &str, _: &Token) -> Extras {
        Extras::default()
    }

    fn titles(listing: &Listing) -> Vec<&str> {
        listing
            .games
            .iter()
            .map(|candidate| candidate.game.title.as_str())
            .collect()
    }

    #[test]
    fn lists_heroic_games_with_migrated_ones_marked_and_bad_entries_as_issues() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[1233017772]);
        fx.native(STARDEW, "Stardew Valley", &[]);
        fx.install(serde_json::json!({"platform": "osx", "install_path": "/y", "appName": "5"}));
        let stardew = fx.prepared(STARDEW);
        let report = run(&fx.paths, &stardew, &Token::new(), |_| {}, no_extras).unwrap();
        assert_eq!(report.environment.as_deref(), Some("stardew-valley"));

        let listing = fx.listing();
        assert_eq!(titles(&listing), ["RimWorld", "Stardew Valley"]);
        assert!(matches!(listing.games[0].standing, Standing::Ready(_)));
        assert!(matches!(
            &listing.games[1].standing,
            Standing::Already { environment } if environment == "stardew-valley"
        ));
        assert_eq!(listing.issues, ["GOG game 5: osx games can't be migrated"]);

        let mut page = fx.page();
        // A migrated game can't be picked again.
        let _ = page.update(Event::Pick(1));
        assert_eq!(page.picked, None);
        let _ = page.update(Event::Pick(0));
        assert_eq!(page.picked, Some(0));
        assert_eq!(page.status, "Checking RimWorld...");
        crate::layout_probe::layout(&mut page.view(Palette::DARK), iced::Size::new(680.0, 560.0));
    }

    /// Copies the fixture's Heroic config to where native and Flatpak
    /// Heroic keep theirs, with `flatpak` edited first, and finds both.
    fn two_heroics(fx: &Fixture, flatpak: impl FnOnce(&std::path::Path)) -> Vec<HeroicSource> {
        let home = fx.root.path();
        let native = home.join(".config/heroic");
        let other = home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic");
        for dir in [&native, &other] {
            for sub in ["gog_store", "store_cache", "GamesConfig"] {
                fs::create_dir_all(dir.join(sub)).unwrap();
            }
            for file in [
                "config.json",
                "gog_store/installed.json",
                "store_cache/gog_library.json",
            ] {
                fs::copy(fx.source.config_dir().join(file), dir.join(file)).unwrap();
            }
        }
        flatpak(&other);
        HeroicSource::discover(home, None, None).unwrap()
    }

    #[test]
    fn a_game_both_heroics_list_at_one_folder_shows_once_and_says_which() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let sources = two_heroics(&fx, |_| {});
        assert_eq!(sources.len(), 2);
        let listing = listing_from(&fx.paths, &sources);
        assert_eq!(titles(&listing), ["RimWorld"]);
        assert!(listing.issues.is_empty(), "{:?}", listing.issues);
        let candidate = listing.games.into_iter().next().unwrap();
        assert_eq!(candidate.source.origin(), HeroicOrigin::Native);
        let Standing::Ready(migration) = candidate.standing else {
            panic!("RimWorld isn't ready");
        };
        let prepared = prepare(
            &fx.paths,
            candidate.source,
            candidate.heroic,
            migration,
            None,
        )
        .unwrap();
        let rows = review_rows(&prepared.migration, prepared.heroic.as_deref());
        let native = fx.root.path().join(".config/heroic");
        assert_eq!(
            rows[2],
            (
                "Heroic",
                format!(
                    "native ({}), also listed by Flatpak Heroic",
                    native.display()
                )
            )
        );
        // One config, as before: no Heroic row.
        let single = fx.prepared(RIMWORLD);
        assert!(
            !review_rows(&single.migration, single.heroic.as_deref())
                .iter()
                .any(|(label, _)| *label == "Heroic")
        );
    }

    #[test]
    fn a_game_the_heroics_put_in_two_folders_is_an_issue() {
        let mut fx = Fixture::new();
        let folder = fx.native(RIMWORLD, "RimWorld", &[]);
        let elsewhere = fx.root.path().join("Elsewhere/RimWorld");
        let sources = two_heroics(&fx, |flatpak| {
            let installed = flatpak.join("gog_store/installed.json");
            let text = fs::read_to_string(&installed).unwrap();
            let moved = text.replace(
                &serde_json::to_string(&folder).unwrap(),
                &serde_json::to_string(&elsewhere).unwrap(),
            );
            assert_ne!(text, moved);
            fs::write(installed, moved).unwrap();
        });
        let listing = listing_from(&fx.paths, &sources);
        assert!(listing.games.is_empty());
        assert_eq!(
            listing.issues,
            [format!(
                "GOG game {RIMWORLD} is installed in two places: {} (native Heroic) and {} (Flatpak Heroic).",
                folder.display(),
                elsewhere.display()
            )]
        );
        let mut page = fx.page();
        let generation = page.generation;
        let _ = page.update(Event::Listed(generation, Ok(listing)));
        crate::layout_probe::layout(&mut page.view(Palette::DARK), iced::Size::new(680.0, 560.0));
    }

    #[test]
    fn the_review_says_what_migrating_does() {
        let mut fx = Fixture::new();
        let folder = fx.native(RIMWORLD, "RimWorld", &[1233017772, 1827857764]);
        let prepared = fx.prepared(RIMWORLD);
        assert!(prepared.ready(), "{:?}", prepared.blocking);
        let rows = review_rows(&prepared.migration, prepared.heroic.as_deref());
        let canonical = folder.canonicalize().unwrap();
        assert_eq!(
            rows,
            [
                (
                    "Environment",
                    "rimworld (new native environment)".to_owned()
                ),
                ("Folder", canonical.display().to_string()),
                ("Track", "installer".to_owned()),
                ("Version", "1.6.4871 rev588".to_owned()),
                ("Language", "en".to_owned()),
                ("DLC", "DLC (1233017772)".to_owned()),
                ("", "DLC (1827857764)".to_owned()),
            ]
        );
    }

    #[test]
    fn the_review_lists_each_dlc_by_name_and_id() {
        let mut fx = Fixture::new();
        let folder = fx.native(RIMWORLD, "RimWorld", &[1233017772, 1827857764, 1865538500]);
        let game = folder.join("game");
        fs::create_dir_all(&game).unwrap();
        for (id, name) in [
            (1233017772_u64, "RimWorld - Royalty"),
            (1827857764, "RimWorld - Ideology"),
        ] {
            fs::write(
                game.join(format!("goggame-{id}.info")),
                serde_json::json!({"gameId": id.to_string(), "name": name}).to_string(),
            )
            .unwrap();
        }
        let prepared = fx.prepared(RIMWORLD);
        let rows = review_rows(&prepared.migration, prepared.heroic.as_deref());
        let dlc: Vec<_> = rows
            .iter()
            .skip_while(|(label, _)| *label != "DLC")
            .cloned()
            .collect();
        assert_eq!(
            dlc,
            [
                ("DLC", "Royalty (1233017772)".to_owned()),
                ("", "Ideology (1827857764)".to_owned()),
                ("", "DLC (1865538500)".to_owned()),
            ]
        );
        // The rows draw, the unlabeled ones included.
        let mut page = fx.page();
        page.picked = Some(0);
        page.prepared = Some(prepared);
        page.step = Step::Review;
        let mut view = page.view(Palette::DARK);
        let probe = crate::layout_probe::probe(&mut view, iced::Size::new(680.0, 560.0));
        let inset =
            iced::Rectangle::new(iced::Point::new(24.0, 24.0), iced::Size::new(632.0, 512.0));
        assert!(probe.outside(inset).is_empty());
    }

    #[test]
    fn a_windows_game_reviews_with_a_runner_choice() {
        let mut fx = Fixture::new();
        fx.disco();
        let prepared = fx.prepared(DISCO);
        assert!(matches!(
            prepared.migration.target(),
            Target::Windows { .. }
        ));
        assert!(prepared.suggestion.is_some());
        let rows = review_rows(&prepared.migration, prepared.heroic.as_deref());
        assert_eq!(rows[2], ("Track", "galaxy".to_owned()));
        assert_eq!(rows[3], ("Version", "build 59781086834418085".to_owned()));
        // Without a runner the game can't be migrated yet.
        if prepared.runner.is_empty() {
            assert!(!prepared.ready());
        }
        let mut page = fx.page();
        page.picked = Some(0);
        page.prepared = Some(prepared);
        page.step = Step::Review;
        for (width, height) in [(280.0, 240.0), (680.0, 560.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view(Palette::DARK);
            let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
            let spilled = probe.outside(inset);
            assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
        }
    }

    #[test]
    fn migrating_reports_like_the_cli_and_adds_the_extras() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let prepared = fx.prepared(RIMWORLD);
        let mut said = Vec::new();
        let report = run(
            &fx.paths,
            &prepared,
            &Token::new(),
            |line| said.push(line),
            |_, _, name, id, title, _| {
                assert_eq!((name, id, title), ("rimworld", 1094900565, "RimWorld"));
                Extras {
                    lines: vec!["Imported 41 h of GOG playtime.".to_owned()],
                    notes: vec!["Skipped SteamGridDB art: no key".to_owned()],
                    icon: false,
                }
            },
        )
        .unwrap();
        assert_eq!(said, ["Adding GOG playtime and art for RimWorld..."]);
        assert_eq!(report.lines[0], "Migrated RimWorld as rimworld.");
        assert!(
            report
                .lines
                .contains(&"Imported 41 h of GOG playtime.".to_owned())
        );
        assert_eq!(report.lines.last().unwrap(), HEROIC_NOTE);
        assert_eq!(report.warnings, ["Skipped SteamGridDB art: no key"]);
        let config = fs::read_to_string(fx.paths.environment_config("rimworld")).unwrap();
        assert!(config.contains(RIMWORLD), "{config}");
    }

    /// A cancel that comes after the migration committed, while GOG's
    /// playtime and art are being fetched, reaches that work through the
    /// page's token: it stops, and the migration stays done.
    #[test]
    fn a_cancel_during_the_extras_stops_them_and_keeps_the_migration() {
        use std::sync::mpsc;
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let prepared = fx.prepared(RIMWORLD);
        let token = Token::new();
        let (started, extras_started) = mpsc::channel();
        let report = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                run(
                    &fx.paths,
                    &prepared,
                    &token,
                    |_| {},
                    |paths, _, name, _, _, extras_token| {
                        // The migration is committed before the extras start.
                        assert!(paths.environment_config(name).exists());
                        started.send(()).unwrap();
                        // Stands in for a slow GOG request that checks the
                        // token, as `extras::import` does between steps.
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(10);
                        while !extras_token.is_canceled() {
                            assert!(std::time::Instant::now() < deadline, "never canceled");
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Extras {
                            notes: vec!["Skipped GOG artwork: canceled.".to_owned()],
                            ..Extras::default()
                        }
                    },
                )
            });
            extras_started.recv().unwrap();
            token.cancel();
            worker.join().unwrap()
        })
        .unwrap();
        assert_eq!(report.lines[0], "Migrated RimWorld as rimworld.");
        assert_eq!(report.warnings, ["Skipped GOG artwork: canceled."]);
        assert!(fx.paths.environment_config("rimworld").exists());
    }

    /// Core decides whether a cancel before the migration comes in time. A
    /// native game has nothing to copy, so it finishes and stays done, and
    /// the extras are skipped with a note.
    #[test]
    fn a_cancel_before_a_native_migration_keeps_it_and_skips_the_extras() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let prepared = fx.prepared(RIMWORLD);
        let token = Token::new();
        token.cancel();
        let report = run(
            &fx.paths,
            &prepared,
            &token,
            |_| {},
            |_, _, _, _, _, _| panic!("no extras after a cancel"),
        )
        .unwrap();
        assert_eq!(report.lines[0], "Migrated RimWorld as rimworld.");
        assert_eq!(report.warnings, ["Skipped GOG playtime and art: canceled."]);
        assert!(fx.paths.environment_config("rimworld").exists());
    }

    #[test]
    fn working_cancels_and_a_failed_migration_returns_to_the_review() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let mut page = fx.page();
        let generation = page.generation;
        let _ = page.update(Event::Pick(0));
        let generation = page.generation.max(generation);
        let _ = page.update(Event::Prepared(
            generation,
            Ok(Box::new(fx.prepared(RIMWORLD))),
        ));
        assert_eq!(page.step, Step::Review);
        let _ = page.update(Event::Migrate);
        assert_eq!(page.step, Step::Working);
        assert!(page.busy());
        let _ = page.update(Event::Cancel);
        assert!(page.token.as_ref().unwrap().is_canceled());
        assert_eq!(page.status, "Canceling...");
        // Back does nothing while it works.
        let _ = page.update(Event::Back);
        assert_eq!(page.step, Step::Working);
        let generation = page.generation;
        let _ = page.update(Event::Finished(
            generation,
            Err("Import canceled.".to_owned()),
        ));
        assert!(!page.busy());
        assert_eq!(page.step, Step::Review);
        assert_eq!(page.error.as_deref(), Some("Import canceled."));
    }

    #[test]
    fn closing_the_window_while_migrating_asks_first_then_cancels() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let mut page = fx.page();
        let _ = page.update(Event::Pick(0));
        let generation = page.generation;
        let _ = page.update(Event::Prepared(
            generation,
            Ok(Box::new(fx.prepared(RIMWORLD))),
        ));
        let _ = page.update(Event::Migrate);
        let mut app = crate::App::boot(fx.paths.clone()).0;
        app.flow = Some(super::super::Page::Gog(Box::new(page)));
        let _ = app.update(crate::Message::Window(iced::window::Event::CloseRequested));
        assert!(app.close_prompt);
        let _ = app.update(crate::Message::CancelAndClose);
        assert!(app.closing);
        let Some(super::super::Page::Gog(page)) = &app.flow else {
            panic!("the page stays until its work ends");
        };
        assert!(page.token.as_ref().unwrap().is_canceled());
    }

    /// Like Heroic import: reading and checking games is dropped, not
    /// waited on, when the page moves on; copying a runner or downloading a
    /// build is work, which Cancel stops and closing the window asks about.
    #[test]
    fn checking_is_dropped_on_back_and_prerequisites_cancel_like_heroic() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let mut page = fx.page();
        let _ = page.update(Event::Pick(0));
        assert!(
            page.waiting() && !page.busy(),
            "checking isn't work to wait for"
        );
        let generation = page.generation;
        let _ = page.update(Event::Back);
        assert!(page.status.is_empty());
        let _ = page.update(Event::Prepared(
            generation,
            Ok(Box::new(fx.prepared(RIMWORLD))),
        ));
        assert_eq!(page.step, Step::Games, "a dropped check doesn't reopen");
        assert!(page.prepared.is_none());

        let _ = page.update(Event::KeepEditing);
        let _ = page.update(Event::Pick(0));
        let generation = page.generation;
        let _ = page.update(Event::Prepared(
            generation,
            Ok(Box::new(fx.prepared(RIMWORLD))),
        ));
        let _ = page.update(Event::DownloadRunner);
        assert_eq!(page.step, Step::Working);
        assert!(page.busy());
        let mut app = crate::App::boot(fx.paths.clone()).0;
        app.flow = Some(super::super::Page::Gog(Box::new(page)));
        let _ = app.update(crate::Message::Window(iced::window::Event::CloseRequested));
        assert!(app.close_prompt);
        let _ = app.update(crate::Message::CancelAndClose);
        let Some(super::super::Page::Gog(page)) = &mut app.flow else {
            panic!("the page stays until the download stops");
        };
        assert!(page.token.as_ref().unwrap().is_canceled());
        let generation = page.generation;
        let _ = page.update(Event::PrerequisiteFinished(
            generation,
            Err("Canceled".to_owned()),
        ));
        assert!(!page.busy());
        assert_eq!(page.step, Step::Review);
    }

    #[test]
    fn leaving_after_picking_a_game_asks_first() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let mut page = fx.page();
        let _ = page.update(Event::Back);
        assert!(matches!(page.take_action(), Some(Action::Choose)));

        let mut page = fx.page();
        let _ = page.update(Event::Pick(0));
        let generation = page.generation;
        let _ = page.update(Event::Prepared(
            generation,
            Ok(Box::new(fx.prepared(RIMWORLD))),
        ));
        let _ = page.update(Event::Back);
        assert_eq!(page.step, Step::Games);
        assert!(page.take_action().is_none());
        let _ = page.update(Event::Back);
        assert!(page.leave_prompt);
        assert!(page.take_action().is_none());
        let _ = page.update(Event::KeepEditing);
        assert!(!page.leave_prompt);
        let _ = page.update(Event::Back);
        let _ = page.update(Event::Discard);
        assert!(matches!(page.take_action(), Some(Action::Choose)));
    }

    #[test]
    fn a_portable_session_says_where_gog_games_live() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(&home)),
            Some(&root.path().join("portable")),
        )
        .unwrap();
        let error = load_listing(&paths).unwrap_err();
        assert_eq!(
            error,
            "GOG games live in the host library. Run this without --portable."
        );
        let (mut page, _) = Page::new(paths);
        let generation = page.generation;
        let _ = page.update(Event::Listed(generation, Err(error.clone())));
        assert_eq!(page.error.as_deref(), Some(error.as_str()));
        assert!(page.listing.is_none());
    }

    #[test]
    fn extras_read_like_import_gog() {
        let report = extras::Report {
            playtime: Some(Playtime {
                gog_seconds: 41 * 3600 + 600,
                before_seconds: 0,
                after_seconds: 41 * 3600 + 600,
            }),
            art: vec![
                Filled {
                    kind: Kind::Icon,
                    source: Source::Gog(1),
                },
                Filled {
                    kind: Kind::Grid,
                    source: Source::Gog(1),
                },
                Filled {
                    kind: Kind::Logo,
                    source: Source::SteamGridDb(9),
                },
            ],
            notes: Vec::new(),
        };
        assert_eq!(
            extras_lines(&report),
            [
                "Imported 41 h of GOG playtime.",
                "Added GOG art: poster, icon.",
                "Added SteamGridDB art: logo.",
            ]
        );
        assert_eq!(duration(25 * 60), "25 min");
    }

    const CELESTE: u64 = 1452425640;
    const HADES: u64 = 1477079530;

    /// Writes an install of `id` that stopped before `owner`'s config was
    /// written.
    fn configless(paths: &Paths, folder: &std::path::Path, owner: &str, id: u64) {
        use wineo_core::gog::folder::{FolderLock, FolderState, Journal, Platform, Target};
        fs::create_dir_all(folder).unwrap();
        let folder = fs::canonicalize(folder).unwrap();
        let state = FolderState {
            journal: Some(Journal {
                operation: "op".to_owned(),
                kind: JournalKind::Install,
                target: Target {
                    gog_id: id,
                    platform: Platform::Linux,
                    language: "en".to_owned(),
                    track: Track::Installer,
                    products: Vec::new(),
                },
                packages: Vec::new(),
                attempted: Vec::new(),
            }),
            ..FolderState::new(folder.clone(), Some(owner.to_owned()))
        };
        FolderLock::exclusive(paths, &folder, &|| false)
            .unwrap()
            .write(&state)
            .unwrap();
    }

    #[test]
    fn the_grid_lists_every_owned_game_by_what_wineo_has_of_it() {
        use super::super::gog_install::tests::{
            DISCO, RIMWORLD as RIMWORLD_ID, World, disco, inno, rimworld,
        };
        use crate::gog_fake::{native_game, owned_game};
        use wineo_core::gog::update::windows::set_graphical_session;

        let world = World::new();
        set_graphical_session(Some(true));
        world.installed(&rimworld(), &Default::default()).unwrap();
        // Disco's installer stops partway, which leaves its install to finish.
        world.disco("1", "exit 4", true);
        let failure = world.installed(&disco(), &inno()).unwrap_err();
        assert!(
            format!("{failure:?}").contains("stopped partway"),
            "{failure:?}"
        );
        set_graphical_session(None);
        native_game(
            &world.paths,
            world.root.path(),
            "stardew",
            STARDEW.parse().unwrap(),
            "1.0",
            false,
        );
        configless(
            &world.paths,
            &world.root.path().join("Games/Hades"),
            "hades",
            HADES,
        );
        world.library.lock().unwrap().owned.extend([
            owned_game(STARDEW.parse().unwrap(), "Stardew Valley", true, true),
            owned_game(CELESTE, "Celeste", true, false),
            owned_game(HADES, "Hades", true, true),
        ]);

        let mut listing = Listing::default();
        add_owned(
            &world.paths,
            &world.keyring,
            world.server.client(),
            &mut listing,
        );
        assert_eq!(listing.owned_note, None);
        let states: Vec<(&str, &OwnedState, Option<&str>)> = listing
            .owned
            .iter()
            .map(|owned| {
                (
                    owned.offer.title.as_str(),
                    &owned.state,
                    owned.offer.unfinished.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            states,
            [
                ("Celeste", &OwnedState::NotInstalled, None),
                (
                    "Disco Elysium",
                    &OwnedState::Unfinished,
                    Some("disco-elysium")
                ),
                ("Hades", &OwnedState::Unfinished, Some("hades")),
                (
                    "RimWorld",
                    &OwnedState::Installed("rimworld".to_owned()),
                    None
                ),
                (
                    "Stardew Valley",
                    &OwnedState::Imported("stardew".to_owned()),
                    None
                ),
            ]
        );
        assert_eq!(listing.owned[0].offer.gog_id, CELESTE);
        assert!(listing.owned[0].offer.linux && !listing.owned[0].offer.windows);
        assert_eq!(listing.owned[3].offer.gog_id, RIMWORLD_ID);
        assert_eq!(listing.owned[1].offer.gog_id, DISCO);

        let (mut page, _) = Page::new(world.paths.clone());
        let generation = page.generation;
        let _ = page.update(Event::Listed(generation, Ok(listing)));
        // Narrow and short dialogs keep the grid scrolling inside them.
        for (width, height) in [(280.0, 240.0), (360.0, 280.0), (680.0, 560.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view_at(Palette::DARK, width);
            let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
            assert!(probe.outside(inset).is_empty(), "{width}x{height}");
        }
        // Installed and migrated games can't be opened.
        let _ = page.update(Event::Install(3));
        let _ = page.update(Event::Install(4));
        assert_eq!(page.step, Step::Games);
        // The keyboard moves to Celeste first, and Enter opens its install.
        let task = page.key(
            &Key::Named(Named::ArrowRight),
            keyboard::Modifiers::empty(),
            680.0,
        );
        assert!(task.is_some());
        assert_eq!(page.focus, Some(0));
        let _ = page.key(
            &Key::Named(Named::Enter),
            keyboard::Modifiers::empty(),
            680.0,
        );
        assert_eq!(page.step, Step::Install);
        assert!(page.waiting(), "it plans the install");
        {
            let mut view = page.view_at(Palette::DARK, 680.0);
            let _ = crate::layout_probe::layout(&mut view, iced::Size::new(680.0, 560.0));
        }
        // Back from the review returns to the grid and reads it again.
        let _ = page.back();
        assert_eq!(page.step, Step::Games);
        assert!(page.install.is_none());
    }

    #[test]
    fn heroic_games_show_once_and_logged_out_still_migrates() {
        use crate::gog_fake::{Library, library_server, owned_game, save_login};
        use crate::settings::steam_key::tests::TestKeyring;

        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let library = std::sync::Arc::new(std::sync::Mutex::new(Library {
            owned: vec![
                owned_game(RIMWORLD.parse().unwrap(), "RimWorld", true, true),
                owned_game(CELESTE, "Celeste", true, false),
            ],
            ..Library::default()
        }));
        let server = library_server(library);
        let keyring = TestKeyring(std::cell::RefCell::new(None));

        let mut listing = fx.listing();
        add_owned(&fx.paths, &keyring, server.client(), &mut listing);
        assert_eq!(
            listing.owned_note.as_deref(),
            Some("Log in to GOG in Settings to install the games you own.")
        );
        assert!(listing.owned.is_empty());
        assert_eq!(titles(&listing), ["RimWorld"]);

        save_login(&fx.paths, &keyring);
        let mut listing = fx.listing();
        add_owned(&fx.paths, &keyring, server.client(), &mut listing);
        let owned: Vec<&str> = listing
            .owned
            .iter()
            .map(|owned| owned.offer.title.as_str())
            .collect();
        assert_eq!(owned, ["Celeste"], "Heroic's RimWorld shows once");
        assert_eq!(Page::entries(&listing), [Entry::Owned(0), Entry::Heroic(0)]);
    }

    #[test]
    fn the_install_pages_stay_inside_narrow_and_short_dialogs() {
        use super::super::gog_install::tests::{World, inno, rimworld};
        use wineo_core::gog::update::windows::set_graphical_session;

        let world = World::new();
        set_graphical_session(Some(true));
        let native = world.prepare(&rimworld(), &Default::default(), false);
        let windows = world.prepare(
            &rimworld(),
            &super::super::gog_install::Choices {
                platform: Some(wineo_core::gog::folder::Platform::Windows),
                ..inno()
            },
            false,
        );
        set_graphical_session(None);
        for prepared in [native, windows] {
            check_install_layout(&world, prepared);
        }
    }

    fn check_install_layout(
        world: &super::super::gog_install::tests::World,
        prepared: super::super::gog_install::Prepared,
    ) {
        let (mut page, _) = Page::new(world.paths.clone());
        let generation = page.generation;
        let mut listing = Listing::default();
        add_owned(
            &world.paths,
            &world.keyring,
            world.server.client(),
            &mut listing,
        );
        let _ = page.update(Event::Listed(generation, Ok(listing)));
        let index = page
            .listing
            .as_ref()
            .unwrap()
            .owned
            .iter()
            .position(|owned| owned.offer.title == "RimWorld")
            .unwrap();
        let _ = page.update(Event::Install(index));
        let generation = page.install.as_ref().unwrap().generation();
        let instance = page.install_instance;
        let _ = page.update(Event::Installing(
            instance,
            Box::new(gog_install::Event::Prepared(
                generation,
                Ok(Box::new(prepared)),
            )),
        ));
        for (width, height) in [(280.0, 240.0), (360.0, 280.0), (900.0, 720.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view_at(Palette::DARK, width);
            let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
            let spilled = probe.outside(inset);
            assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
        }
    }

    #[test]
    fn a_reply_to_a_review_that_was_left_never_reaches_the_next_one() {
        use super::super::gog_install::tests::{World, rimworld};

        let world = World::new();
        let stale = world.prepare(&rimworld(), &Default::default(), false);
        let mut listing = Listing::default();
        add_owned(
            &world.paths,
            &world.keyring,
            world.server.client(),
            &mut listing,
        );
        let titles: Vec<String> = listing
            .owned
            .iter()
            .map(|owned| owned.offer.title.clone())
            .collect();
        let rimworld_at = titles.iter().position(|title| title == "RimWorld").unwrap();
        let disco_at = titles
            .iter()
            .position(|title| title == "Disco Elysium")
            .unwrap();
        let (mut page, _) = Page::new(world.paths.clone());
        let generation = page.generation;
        let _ = page.update(Event::Listed(generation, Ok(listing.clone())));
        // Open RimWorld, leave while it plans, then open Disco Elysium.
        let _ = page.update(Event::Install(rimworld_at));
        let first = page.install_instance;
        let first_generation = page.install.as_ref().unwrap().generation();
        let _ = page.back();
        assert!(page.install.is_none());
        let generation = page.generation;
        let _ = page.update(Event::Listed(generation, Ok(listing)));
        let _ = page.update(Event::Install(disco_at));
        assert_eq!(
            page.install.as_ref().unwrap().generation(),
            first_generation,
            "both start at the same generation"
        );
        // RimWorld's plan arrives late and is dropped.
        let _ = page.update(Event::Installing(
            first,
            Box::new(gog_install::Event::Prepared(
                first_generation,
                Ok(Box::new(stale)),
            )),
        ));
        let install = page.install.as_ref().unwrap();
        assert_eq!(install.planned(), None);
        assert!(install.waiting(), "Disco Elysium is still being planned");
    }

    // -- posters --

    fn png_bytes() -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        ::image::RgbaImage::from_pixel(8, 12, ::image::Rgba([200, 30, 30, 255]))
            .write_to(&mut bytes, ::image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn handle() -> image::Handle {
        image::Handle::from_rgba(1, 1, vec![0; 4])
    }

    /// Serves one body for every cover URL, and records the requests.
    struct Covers {
        status: u16,
        body: Vec<u8>,
        seen: std::sync::Mutex<Vec<String>>,
    }

    impl Covers {
        fn new(status: u16, body: Vec<u8>) -> Self {
            Self {
                status,
                body,
                seen: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn count(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    impl wineo_core::download::Transport for Covers {
        fn get(&self, url: &str) -> anyhow::Result<Box<dyn std::io::Read>> {
            anyhow::bail!("unexpected plain request to {url}")
        }

        fn request(
            &self,
            url: &str,
            _bearer: Option<&str>,
        ) -> anyhow::Result<wineo_core::download::Response> {
            self.seen.lock().unwrap().push(url.to_owned());
            Ok(wineo_core::download::Response {
                status: self.status,
                body: Box::new(std::io::Cursor::new(self.body.clone())),
            })
        }
    }

    /// A GOG whose art answer names a cover for whichever game is asked
    /// about, or fails with 500.
    fn art_server(works: bool) -> crate::gog_fake::Server {
        crate::gog_fake::Server::start(move |line| {
            let path = line.split(' ').nth(1).unwrap_or_default();
            let path = path.split('?').next().unwrap_or_default();
            match path.strip_prefix("/v2/games/") {
                Some(id) if works => crate::gog_fake::reply(
                    200,
                    &serde_json::json!({
                        "_links": {"boxArtImage": {"href": format!("https://images.example/{id}.png")}},
                        "_embedded": {"product": {"id": id.parse::<u64>().unwrap(), "title": "X"}},
                    })
                    .to_string(),
                ),
                _ => crate::gog_fake::reply(500, "{}"),
            }
        })
    }

    fn owned_game_in(id: u64, title: &str, state: OwnedState) -> Owned {
        Owned {
            offer: Offer {
                gog_id: id,
                title: title.to_owned(),
                linux: true,
                windows: false,
                unfinished: None,
            },
            state,
        }
    }

    fn owned_listing(owned: Vec<Owned>) -> Listing {
        Listing {
            owned,
            ..Listing::default()
        }
    }

    fn store_grid(paths: &Paths, environment: &str) {
        let dir = paths.art_dir(environment);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("grid.png"), png_bytes()).unwrap();
    }

    /// Runs the worker on `jobs` and collects what it sent.
    fn load(
        paths: &Paths,
        jobs: Vec<Job>,
        token: &Token,
        server: Option<&crate::gog_fake::Server>,
        covers: &'static Covers,
    ) -> Vec<(String, Result<image::Handle, String>)> {
        let mut sent = Vec::new();
        gog_posters::run(
            paths,
            jobs,
            token,
            || {
                let server = server.ok_or_else(|| "no network in this test".to_owned())?;
                Ok(Network {
                    client: server.client(),
                    transport: Box::new(CoversRef(covers)),
                })
            },
            |id, poster| sent.push((id, poster)),
        );
        sent
    }

    struct CoversRef(&'static Covers);

    impl wineo_core::download::Transport for CoversRef {
        fn get(&self, url: &str) -> anyhow::Result<Box<dyn std::io::Read>> {
            self.0.get(url)
        }

        fn request(
            &self,
            url: &str,
            bearer: Option<&str>,
        ) -> anyhow::Result<wineo_core::download::Response> {
            self.0.request(url, bearer)
        }
    }

    fn leaked(covers: Covers) -> &'static Covers {
        Box::leak(Box::new(covers))
    }

    fn covers_dir_is_empty(paths: &Paths) -> bool {
        fs::read_dir(paths.gog_covers_dir()).map_or(true, |mut entries| entries.next().is_none())
    }

    fn page_with(paths: &Paths, listing: Listing) -> Page {
        let (mut page, _) = Page::new(paths.clone());
        let generation = page.generation;
        let _ = page.update(Event::Listed(generation, Ok(listing)));
        page
    }

    #[test]
    fn a_stale_poster_event_is_ignored() {
        let fx = Fixture::new();
        let mut page = fx.page();
        let generation = page.poster_generation;
        let _ = page.update(Event::Poster(
            generation.wrapping_sub(1),
            RIMWORLD.to_owned(),
            Ok(handle()),
        ));
        assert!(page.poster(RIMWORLD).is_none());
        let _ = page.update(Event::Poster(generation, RIMWORLD.to_owned(), Ok(handle())));
        assert!(page.poster(RIMWORLD).is_some());

        // A relist starts over: the old load's reply lands nowhere.
        let _ = page.list();
        assert_ne!(page.poster_generation, generation);
        let _ = page.update(Event::Poster(generation, STARDEW.to_owned(), Ok(handle())));
        assert!(page.posters.is_empty());

        // A page opened later never shares a load number with this one.
        let (next, _) = Page::new(fx.paths.clone());
        assert_ne!(next.poster_generation, page.poster_generation);
        assert_ne!(next.poster_generation, generation);
    }

    #[test]
    fn leaving_the_grid_stops_and_invalidates_the_poster_load() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let mut page = fx.page();
        let token = page.poster_token.clone().expect("a load started");
        let generation = page.poster_generation;
        let _ = page.back();
        assert!(matches!(page.take_action(), Some(Action::Choose)));
        assert!(token.is_canceled());
        assert_ne!(page.poster_generation, generation);
        let _ = page.update(Event::Poster(generation, RIMWORLD.to_owned(), Ok(handle())));
        assert!(page.poster(RIMWORLD).is_none());

        // Dropping the page stops the worker too.
        let mut page = fx.page();
        let token = page.poster_token.take().unwrap();
        page.poster_token = Some(token.clone());
        drop(page);
        assert!(token.is_canceled());
    }

    #[test]
    fn going_back_from_a_review_restarts_the_unfinished_posters_under_a_new_load() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let mut page = fx.page();
        let old = page.poster_token.clone().unwrap();
        let generation = page.poster_generation;
        page.step = Step::Review;
        let _ = page.back();
        assert_eq!(page.step, Step::Games);
        assert!(old.is_canceled());
        let token = page.poster_token.as_ref().expect("loading again");
        assert!(!token.is_canceled());
        assert_ne!(page.poster_generation, generation);
        // The old load's reply is dropped; the new one's is kept.
        let _ = page.update(Event::Poster(generation, RIMWORLD.to_owned(), Ok(handle())));
        assert!(page.poster(RIMWORLD).is_none());
        let current = page.poster_generation;
        let _ = page.update(Event::Poster(current, RIMWORLD.to_owned(), Ok(handle())));
        assert!(page.poster(RIMWORLD).is_some());
    }

    #[test]
    fn a_failed_listing_loads_no_posters() {
        let fx = Fixture::new();
        let (mut page, _) = Page::new(fx.paths.clone());
        let generation = page.generation;
        let _ = page.update(Event::Listed(generation, Err("nope".to_owned())));
        assert!(page.poster_token.is_none());
        assert!(page.poster_jobs().is_empty());
    }

    #[test]
    fn games_wineo_has_use_their_stored_grid_art_with_no_network() {
        let mut fx = Fixture::new();
        fx.native(STARDEW, "Stardew Valley", &[]);
        let stardew = fx.prepared(STARDEW);
        run(&fx.paths, &stardew, &Token::new(), |_| {}, no_extras).unwrap();
        store_grid(&fx.paths, "stardew-valley");
        store_grid(&fx.paths, "rimworld");
        store_grid(&fx.paths, "stardew");
        let mut listing = fx.listing();
        assert!(matches!(
            &listing.games[0].standing,
            Standing::Already { environment } if environment == "stardew-valley"
        ));
        listing.owned = vec![
            owned_game_in(1, "Installed", OwnedState::Installed("rimworld".to_owned())),
            owned_game_in(2, "Imported", OwnedState::Imported("stardew".to_owned())),
            owned_game_in(3, "Bare", OwnedState::Installed("absent".to_owned())),
        ];
        let mut page = page_with(&fx.paths, listing);
        let jobs: Vec<(String, PosterSource)> = page
            .poster_jobs()
            .into_iter()
            .map(|job| (job.id, job.source))
            .collect();
        // In grid order, which sorts by title.
        assert_eq!(
            jobs,
            [
                ("3".to_owned(), PosterSource::Stored("absent".to_owned())),
                ("2".to_owned(), PosterSource::Stored("stardew".to_owned())),
                ("1".to_owned(), PosterSource::Stored("rimworld".to_owned())),
                (
                    STARDEW.to_owned(),
                    PosterSource::Stored("stardew-valley".to_owned())
                ),
            ]
        );

        // The worker never asks for a connection, and a game without stored
        // art simply has no poster.
        let covers = leaked(Covers::new(500, Vec::new()));
        let generation = page.poster_generation;
        let sent = load(&fx.paths, page.poster_jobs(), &Token::new(), None, covers);
        assert_eq!(covers.count(), 0);
        assert!(covers_dir_is_empty(&fx.paths));
        assert_eq!(
            sent.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            ["2", "1", STARDEW]
        );
        for (id, poster) in sent {
            let _ = page.update(Event::Poster(generation, id, poster));
        }
        assert!(page.poster("1").is_some());
        assert!(page.poster("2").is_some());
        assert!(page.poster(STARDEW).is_some());
        assert!(page.poster("3").is_none());
        // What has a poster isn't loaded again.
        assert_eq!(page.poster_jobs().len(), 1);
        crate::layout_probe::layout(&mut page.view(Palette::DARK), iced::Size::new(680.0, 560.0));
    }

    #[test]
    fn a_game_wineo_lacks_loads_its_cover_once_and_then_from_the_cache() {
        let fx = Fixture::new();
        let listing = owned_listing(vec![
            owned_game_in(7, "Celeste", OwnedState::NotInstalled),
            owned_game_in(8, "Hades", OwnedState::Unfinished),
        ]);
        let mut page = page_with(&fx.paths, listing);
        let server = art_server(true);
        let covers = leaked(Covers::new(200, png_bytes()));

        let sent = load(
            &fx.paths,
            page.poster_jobs(),
            &Token::new(),
            Some(&server),
            covers,
        );
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|(_, poster)| poster.is_ok()));
        assert_eq!(server.requests().len(), 2);
        assert_eq!(covers.count(), 2);
        assert!(fx.paths.gog_covers_dir().join("7.png").is_file());
        assert!(!fx.paths.art_cache_dir().exists());

        // A second page finds both in the cache and never builds a client.
        let again = load(&fx.paths, page.poster_jobs(), &Token::new(), None, covers);
        assert_eq!(again.len(), 2);
        assert!(again.iter().all(|(_, poster)| poster.is_ok()));
        assert_eq!(server.requests().len(), 2);
        assert_eq!(covers.count(), 2);

        let generation = page.poster_generation;
        for (id, poster) in again {
            let _ = page.update(Event::Poster(generation, id, poster));
        }
        assert!(page.poster("7").is_some() && page.poster("8").is_some());
    }

    #[test]
    fn a_cover_that_cant_be_fetched_or_isnt_an_image_leaves_no_file_and_no_poster() {
        for (server, covers) in [
            (art_server(false), Covers::new(200, png_bytes())),
            (art_server(true), Covers::new(404, png_bytes())),
            (
                art_server(true),
                Covers::new(200, b"<html>no</html>".to_vec()),
            ),
        ] {
            let fx = Fixture::new();
            let listing =
                owned_listing(vec![owned_game_in(7, "Celeste", OwnedState::NotInstalled)]);
            let mut page = page_with(&fx.paths, listing);
            let covers = leaked(covers);
            let sent = load(
                &fx.paths,
                page.poster_jobs(),
                &Token::new(),
                Some(&server),
                covers,
            );
            assert_eq!(sent.len(), 1);
            assert!(sent[0].1.is_err());
            assert!(covers_dir_is_empty(&fx.paths));
            let generation = page.poster_generation;
            for (id, poster) in sent {
                let _ = page.update(Event::Poster(generation, id, poster));
            }
            assert!(page.poster("7").is_none());
            // The tile keeps its title and still lays out.
            crate::layout_probe::layout(
                &mut page.view(Palette::DARK),
                iced::Size::new(680.0, 560.0),
            );
        }
    }

    #[test]
    fn a_canceled_load_fetches_nothing_more() {
        let fx = Fixture::new();
        let listing = owned_listing(vec![owned_game_in(7, "Celeste", OwnedState::NotInstalled)]);
        let page = page_with(&fx.paths, listing);
        let server = art_server(true);
        let covers = leaked(Covers::new(200, png_bytes()));
        let token = Token::new();
        token.cancel();
        let sent = load(&fx.paths, page.poster_jobs(), &token, Some(&server), covers);
        assert!(sent.is_empty());
        assert!(server.requests().is_empty());
    }

    #[test]
    fn a_cached_cover_that_wont_decode_is_removed_and_fetched_again_next_load() {
        let fx = Fixture::new();
        let listing = owned_listing(vec![owned_game_in(7, "Celeste", OwnedState::NotInstalled)]);
        let page = page_with(&fx.paths, listing);
        // A truncated download that passed the signature check.
        fs::create_dir_all(fx.paths.gog_covers_dir()).unwrap();
        let cached = fx.paths.gog_covers_dir().join("7.png");
        fs::write(&cached, &png_bytes()[..12]).unwrap();
        let server = art_server(true);
        let covers = leaked(Covers::new(200, png_bytes()));

        let first = load(&fx.paths, page.poster_jobs(), &Token::new(), None, covers);
        assert_eq!(first.len(), 1);
        assert!(first[0].1.is_err());
        assert!(!cached.exists(), "it can't poison the cache");
        assert_eq!(server.requests().len(), 0);

        let second = load(
            &fx.paths,
            page.poster_jobs(),
            &Token::new(),
            Some(&server),
            covers,
        );
        assert!(second[0].1.is_ok());
        assert_eq!(server.requests().len(), 1);
        assert!(cached.is_file());
    }

    #[test]
    fn the_worker_decodes_only_when_a_shared_permit_is_free_and_stops_when_canceled() {
        use std::sync::mpsc::channel;
        use std::time::Duration;

        let fx = Fixture::new();
        let listing = owned_listing(vec![owned_game_in(7, "Celeste", OwnedState::NotInstalled)]);
        let page = page_with(&fx.paths, listing);
        fs::create_dir_all(fx.paths.gog_covers_dir()).unwrap();
        fs::write(fx.paths.gog_covers_dir().join("7.png"), png_bytes()).unwrap();
        let covers = leaked(Covers::new(200, png_bytes()));
        let slots = crate::flow::posters::import_decode_slots();
        let all = crate::flow::posters::import_limit() as u32;
        let held =
            iced::futures::executor::block_on(slots.clone().acquire_many_owned(all)).unwrap();
        let token = Token::new();

        std::thread::scope(|scope| {
            let (sender, receiver) = channel();
            let (done, paths, jobs) = (&token, &fx.paths, page.poster_jobs());
            scope.spawn(move || {
                let _ = sender.send(load(paths, jobs, done, None, covers));
            });
            // Both permits are out, so the cover waits instead of decoding.
            assert!(receiver.recv_timeout(Duration::from_millis(200)).is_err());
            token.cancel();
            let sent = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(sent.is_empty());
        });

        std::thread::scope(|scope| {
            let (sender, receiver) = channel();
            let (paths, jobs) = (&fx.paths, page.poster_jobs());
            scope.spawn(move || {
                let _ = sender.send(load(paths, jobs, &Token::new(), None, covers));
            });
            assert!(receiver.recv_timeout(Duration::from_millis(200)).is_err());
            drop(held);
            let sent = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(sent.len(), 1);
            assert!(sent[0].1.is_ok());
        });
    }

    #[test]
    fn stored_art_that_wont_decode_is_never_removed() {
        let fx = Fixture::new();
        let dir = fx.paths.art_dir("rimworld");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("grid.png"), &png_bytes()[..12]).unwrap();
        let listing = owned_listing(vec![owned_game_in(
            1,
            "RimWorld",
            OwnedState::Installed("rimworld".to_owned()),
        )]);
        let page = page_with(&fx.paths, listing);
        let covers = leaked(Covers::new(200, Vec::new()));
        let sent = load(&fx.paths, page.poster_jobs(), &Token::new(), None, covers);
        assert!(sent[0].1.is_err());
        assert!(dir.join("grid.png").exists());
    }

    #[test]
    fn an_outage_stops_the_fetching_after_three_games_but_cached_covers_still_load() {
        let fx = Fixture::new();
        let listing = owned_listing(
            (1..=6)
                .map(|id| owned_game_in(id, &format!("Game {id}"), OwnedState::NotInstalled))
                .collect(),
        );
        let mut page = page_with(&fx.paths, listing);
        // The last game in the grid was cached by an earlier load.
        fs::create_dir_all(fx.paths.gog_covers_dir()).unwrap();
        fs::write(fx.paths.gog_covers_dir().join("6.png"), png_bytes()).unwrap();
        let server = crate::gog_fake::Server::start(|_| crate::gog_fake::reply(503, "{}"));
        let covers = leaked(Covers::new(200, png_bytes()));
        let sent = load(
            &fx.paths,
            page.poster_jobs(),
            &Token::new(),
            Some(&server),
            covers,
        );
        let art_requests = server
            .requests()
            .iter()
            .filter(|line| line.contains("/v2/games/"))
            .count();
        assert_eq!(art_requests, 3);
        assert_eq!(covers.count(), 0);
        let loaded: Vec<&str> = sent
            .iter()
            .filter(|(_, poster)| poster.is_ok())
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(loaded, ["6"]);
        let generation = page.poster_generation;
        for (id, poster) in sent {
            let _ = page.update(Event::Poster(generation, id, poster));
        }
        assert!(page.poster("6").is_some());
        assert!(page.poster("1").is_none());
        assert!(cached_only(&fx.paths, "6.png"), "nothing new was cached");
    }

    fn cached_only(paths: &Paths, name: &str) -> bool {
        fs::read_dir(paths.gog_covers_dir())
            .unwrap()
            .all(|entry| entry.unwrap().file_name() == name)
    }

    #[test]
    fn a_game_without_a_cover_does_not_count_as_an_outage() {
        let fx = Fixture::new();
        let listing = owned_listing(
            (1..=6)
                .map(|id| owned_game_in(id, &format!("Game {id}"), OwnedState::NotInstalled))
                .collect(),
        );
        let page = page_with(&fx.paths, listing);
        let server = crate::gog_fake::Server::start(|_| crate::gog_fake::reply(404, "{}"));
        let covers = leaked(Covers::new(200, png_bytes()));
        let sent = load(
            &fx.paths,
            page.poster_jobs(),
            &Token::new(),
            Some(&server),
            covers,
        );
        assert_eq!(sent.len(), 6);
        assert_eq!(server.requests().len(), 6);
    }

    #[test]
    fn a_cover_that_isnt_an_outage_resets_the_count() {
        let fx = Fixture::new();
        let listing = owned_listing(
            (1..=7)
                .map(|id| owned_game_in(id, &format!("Game {id}"), OwnedState::NotInstalled))
                .collect(),
        );
        let page = page_with(&fx.paths, listing);
        // Two outages, a game with no cover, then outages until the stop.
        let server = crate::gog_fake::Server::start(|line| {
            let status = if line.contains("/v2/games/3?") {
                404
            } else {
                503
            };
            crate::gog_fake::reply(status, "{}")
        });
        let covers = leaked(Covers::new(200, png_bytes()));
        load(
            &fx.paths,
            page.poster_jobs(),
            &Token::new(),
            Some(&server),
            covers,
        );
        let art_requests = server
            .requests()
            .iter()
            .filter(|line| line.contains("/v2/games/"))
            .count();
        assert_eq!(art_requests, 6);
    }

    #[test]
    fn canceling_after_the_first_cover_arrives_requests_nothing_more() {
        let fx = Fixture::new();
        let listing = owned_listing(
            (1..=4)
                .map(|id| owned_game_in(id, &format!("Game {id}"), OwnedState::NotInstalled))
                .collect(),
        );
        let page = page_with(&fx.paths, listing);
        let server = art_server(true);
        let covers = leaked(Covers::new(200, png_bytes()));
        let token = Token::new();
        let mut sent = Vec::new();
        gog_posters::run(
            &fx.paths,
            page.poster_jobs(),
            &token,
            || {
                Ok(Network {
                    client: server.client(),
                    transport: Box::new(CoversRef(covers)),
                })
            },
            |id, poster| {
                sent.push((id, poster));
                token.cancel();
            },
        );
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1.is_ok());
        assert_eq!(server.requests().len(), 1);
        assert_eq!(covers.count(), 1);
    }

    #[test]
    fn heroic_games_not_yet_migrated_take_the_cover_by_their_numeric_id() {
        let mut fx = Fixture::new();
        fx.native(RIMWORLD, "RimWorld", &[]);
        let page = fx.page();
        let jobs = page.poster_jobs();
        assert_eq!(
            jobs,
            [Job {
                id: RIMWORLD.to_owned(),
                source: PosterSource::Cover(RIMWORLD.parse().unwrap()),
            }]
        );
    }
}
