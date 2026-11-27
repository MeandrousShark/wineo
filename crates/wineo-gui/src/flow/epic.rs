//! Import from Epic, like `wineo import epic --list`: every game the
//! account owns, on the shared poster grid. Games that aren't installed are
//! dimmed, games wineo can't install say why, and picking one opens its
//! install (see `epic_install`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use iced::futures::channel::mpsc::unbounded;
use iced::keyboard::{self, Key, key::Named};
use iced::widget::{column, image, row, text};
use iced::{Center, Element, Fill, Task};
use wineo_core::art::{self, ArtConfig, Kind};
use wineo_core::cancel::Token;
use wineo_core::download::{Https, Transport};
use wineo_core::epic::Error;
use wineo_core::epic::covers;
use wineo_core::epic::install::{self, HOST_ONLY, Local, LocalState};
use wineo_core::epic::owned::{self, Image, Owned};
use wineo_core::gog::covers::CoverError;
use wineo_core::paths::Paths;

use super::Action;
use super::epic_install::{self, Game as InstallGame, Install, Leave};
use super::frame;
use super::import_grid::{ImportGrid, Tile};
use super::posters;
use crate::epic_legendary::{self as legendary, Found};
use crate::task;
use crate::theme::Palette;
use crate::ui::secondary_button;

const GRID_ID: &str = "epic-import-grid";

/// What logged out says about the games you own.
pub(crate) const LOGGED_OUT: &str = "Log in to Epic in Settings to install the games you own.";

/// What wineo has of a game, like `import epic --list` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum State {
    NotInstalled,
    /// Its install stopped; installing again finishes it.
    Unfinished,
    /// Whole on disk, with no environment owning it.
    Downloaded,
    Installed(String),
    /// Why wineo can't install it.
    Unavailable(&'static str),
}

#[derive(Debug, Clone)]
pub(crate) struct Game {
    pub(crate) app: String,
    pub(crate) title: String,
    pub(crate) state: State,
    images: Vec<Image>,
}

impl Game {
    /// The tile's reason, which keeps it from being picked, and its note.
    pub(crate) fn tile_text(&self) -> (Option<String>, Option<String>) {
        match &self.state {
            State::NotInstalled => (None, Some("Not installed".to_owned())),
            State::Unfinished => (None, Some("Install didn't finish".to_owned())),
            State::Installed(name) => (None, Some(format!("Installed as {name}."))),
            State::Downloaded => (
                Some("Downloaded, but no environment owns it.".to_owned()),
                None,
            ),
            // The reason's color says it can't be installed, and the tile
            // has room for two short lines.
            State::Unavailable(why) => (Some(capitalized(why)), None),
        }
    }

    fn installed(&self) -> bool {
        matches!(self.state, State::Installed(_))
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Listing {
    pub(crate) games: Vec<Game>,
    /// Why the list is missing or not fresh, like being logged out.
    pub(crate) notes: Vec<String>,
    /// Records of wineo's that couldn't be read.
    pub(crate) issues: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Listed(u64, Result<Listing, String>),
    /// Opens the game at this index: its install, or its card once
    /// installed.
    Open(usize),
    /// An install's event, with the instance it belongs to, so a reply to
    /// a review that was left can't reach the next one.
    Installing(u64, Box<epic_install::Event>),
    Poster(u64, String, Result<image::Handle, String>),
    PostersEnded(u64),
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
    generation: u64,
    listing: Option<Listing>,
    status: String,
    error: Option<String>,
    action: Option<Action>,
    install: Option<Box<Install>>,
    /// Which opened review `install` is; each one gets a new number.
    install_instance: u64,
    grid: ImportGrid<Event>,
    /// Decoded posters by app. None when a poster failed, which keeps the
    /// text tile.
    posters: HashMap<String, Option<image::Handle>>,
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
            generation: 0,
            listing: None,
            status: String::new(),
            error: None,
            action: None,
            install: None,
            install_instance: 0,
            grid: ImportGrid::new(GRID_ID, Event::ProgressDue, next_poster_load()),
            posters: HashMap::new(),
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
        self.install.as_ref().is_some_and(|install| install.busy())
    }

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
        if self.waiting() {
            return Task::none();
        }
        self.open_focused()
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        if let Some(install) = &mut self.install {
            let task = install.back();
            return self.installing(task);
        }
        self.stop_posters();
        self.action = Some(Action::Choose);
        Task::none()
    }

    /// Wraps the install's task, and follows where it leaves to.
    fn installing(&mut self, task: Task<epic_install::Event>) -> Task<Event> {
        match self
            .install
            .as_mut()
            .and_then(|install| install.take_leave())
        {
            Some(Leave::Games) => {
                self.install = None;
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

    fn open_focused(&mut self) -> Task<Event> {
        match self.grid.focus {
            Some(index) => self.update(Event::Open(index)),
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
        if self.install.is_some() || self.waiting() {
            return None;
        }
        let count = self.listing.as_ref()?.games.len();
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

    fn list(&mut self) -> Task<Event> {
        self.listing = None;
        self.posters.clear();
        self.stop_posters();
        self.status = "Reading your Epic games...".to_owned();
        let generation = self.next_generation();
        let paths = self.paths.clone();
        task::blocking(
            move || load_listing(&paths, &Token::new()),
            move |result| Event::Listed(generation, result),
        )
    }

    fn stop_posters(&mut self) {
        if let Some(token) = self.poster_token.take() {
            token.cancel();
        }
        self.grid.restart_posters(next_poster_load());
    }

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
                run_posters(&paths, jobs, &token, open_transport, |id, poster| {
                    let _ = sender.unbounded_send(Event::Poster(generation, id, poster));
                });
            })
            .await;
            Event::PostersEnded(generation)
        });
        let shown = self.grid.progress.add(count, Instant::now());
        Task::batch([Task::run(receiver, |event| event), work, shown])
    }

    /// A poster for every tile that has none yet, in grid order: the art an
    /// installed game stored, else Epic's box art.
    fn poster_jobs(&self) -> Vec<Job> {
        let Some(listing) = &self.listing else {
            return Vec::new();
        };
        listing
            .games
            .iter()
            .filter(|game| !self.posters.contains_key(&game.app))
            .map(|game| Job {
                id: game.app.clone(),
                source: match &game.state {
                    State::Installed(name) => Source::Stored(name.clone()),
                    _ => Source::Cover(game.images.clone()),
                },
            })
            .collect()
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
            Event::Open(index) if self.install.is_none() && !self.waiting() => {
                let Some(game) = self
                    .listing
                    .as_ref()
                    .and_then(|listing| listing.games.get(index))
                else {
                    return Task::none();
                };
                self.grid.click(index);
                match &game.state {
                    State::Installed(name) => {
                        self.action = Some(Action::Library {
                            name: Some(name.clone()),
                            launch: false,
                        });
                        Task::none()
                    }
                    State::NotInstalled | State::Unfinished => {
                        let game = InstallGame {
                            app: game.app.clone(),
                            title: game.title.clone(),
                            unfinished: game.state == State::Unfinished,
                        };
                        self.error = None;
                        self.stop_posters();
                        let (install, task) = Install::new(self.paths.clone(), game);
                        self.install = Some(Box::new(install));
                        self.install_instance = self.install_instance.wrapping_add(1);
                        let instance = self.install_instance;
                        task.map(move |event| Event::Installing(instance, Box::new(event)))
                    }
                    State::Downloaded | State::Unavailable(_) => Task::none(),
                }
            }
            Event::Back => self.back(),
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
            _ => Task::none(),
        }
    }

    /// `width` is the dialog's, which sets how many tiles fit across.
    pub(crate) fn view_at(&self, palette: Palette, width: f32) -> Element<'_, Event> {
        let finished = self
            .install
            .as_ref()
            .is_some_and(|install| install.finished());
        let back = secondary_button("Back", palette)
            .on_press_maybe((!self.busy() && !finished).then_some(Event::Back));
        let mut page = column![]
            .spacing(12)
            .max_width(frame::PAGE_MAX_WIDTH)
            .height(Fill);
        if let Some(install) = &self.install {
            let (body, footer) = install.view(column![].spacing(14).width(Fill), palette);
            let instance = self.install_instance;
            let body =
                Element::from(body).map(move |event| Event::Installing(instance, Box::new(event)));
            page = page
                .push(row![back].align_y(Center))
                .push(frame::scroll_body(body, None, palette));
            if let Some(footer) = footer {
                page = page.push(frame::pinned(
                    footer.map(move |event| Event::Installing(instance, Box::new(event))),
                    palette,
                ));
            }
            return frame::page_frame(page);
        }
        let mut body = column![crate::ui::page_title("Choose an Epic game", palette)]
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
        page = page.push(row![back].align_y(Center));
        page = page.push(self.games_view(body, palette, width));
        if let Some(bar) = self.grid.progress_bar(palette) {
            page = page.push(frame::pinned(bar, palette));
        }
        frame::page_frame(page)
    }

    fn games_view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Event>,
        palette: Palette,
        width: f32,
    ) -> Element<'a, Event> {
        let Some(listing) = &self.listing else {
            return frame::scroll_body(body, None, palette);
        };
        for issue in &listing.issues {
            body = body.push(text(issue).size(13).color(palette.warning));
        }
        for note in &listing.notes {
            body = body.push(text(note).size(13).color(palette.muted));
        }
        if listing.games.is_empty() {
            if listing.notes.is_empty() {
                body = body.push(text("You don't own any Epic games.").color(palette.muted));
            }
            return frame::scroll_body(body, None, palette);
        }
        let tiles = listing.games.iter().enumerate().map(|(index, game)| {
            let (reason, note) = game.tile_text();
            (
                Tile {
                    title: &game.title,
                    poster: self.posters.get(&game.app).and_then(Option::as_ref),
                    reason,
                    note,
                    chosen: false,
                    focus_ring: self.grid.focus_ring(index),
                    // Like the library: a game that isn't installed is dimmed.
                    dimmed: !game.installed(),
                },
                Event::Open(index),
            )
        });
        self.grid.view(body.into(), tiles, width, palette)
    }

    #[cfg(test)]
    pub(super) fn listing(&self) -> Option<&Listing> {
        self.listing.as_ref()
    }

    #[cfg(test)]
    pub(super) fn installing_game(&self) -> bool {
        self.install.is_some()
    }
}

/// Every owned game with what wineo has of it, sorted by title, the way
/// `import epic --list` reads it: the saved list when no Legendary can run,
/// and a note saying why when there is no list at all.
pub(crate) fn load_listing(paths: &Paths, token: &Token) -> Result<Listing, String> {
    if paths.portable_root().is_some() {
        return Err(HOST_ONLY.to_owned());
    }
    let found = legendary::find(paths, token)?;
    listing_from(paths, found, token)
}

pub(crate) fn listing_from(paths: &Paths, found: Found, token: &Token) -> Result<Listing, String> {
    let mut listing = Listing::default();
    let owned = match found {
        Found::Usable(legendary) => match owned::load(&legendary, false, token) {
            Ok(owned) => Some(owned),
            Err(Error::LoggedOut) => {
                listing.notes.push(LOGGED_OUT.to_owned());
                None
            }
            Err(error) => {
                listing
                    .notes
                    .push(format!("Couldn't list the games you own: {error}"));
                None
            }
        },
        // Listing what is saved never asks to download Legendary.
        Found::Offer(_) | Found::Unusable(_) => {
            let why = match &found {
                Found::Unusable(why) => Some(why.clone()),
                _ => None,
            };
            match owned::cached(paths) {
                Some(owned) => {
                    listing.notes.push(match &why {
                        None => "Legendary isn't installed, so this is the saved list.".to_owned(),
                        Some(why) => format!("{why} This is the saved list."),
                    });
                    Some(owned)
                }
                None => {
                    listing
                        .notes
                        .push(why.unwrap_or_else(|| LOGGED_OUT.to_owned()));
                    None
                }
            }
        }
    };
    let Some(owned) = owned else {
        return Ok(listing);
    };
    let Owned { games, warning, .. } = owned;
    listing.notes.extend(warning);
    let (local, errors) = install::local(paths).map_err(|error| format!("{error:#}"))?;
    listing.issues = errors;
    let mut games: Vec<Game> = games
        .into_iter()
        .map(|game| {
            let state = state(&game, local.get(&game.app_name));
            Game {
                app: game.app_name,
                title: game.title,
                state,
                images: game.images,
            }
        })
        .collect();
    games.sort_by_cached_key(|game| (game.title.to_lowercase(), game.app.clone()));
    listing.games = games;
    Ok(listing)
}

/// A game's state: what wineo has of it, else what the owned list says.
fn state(game: &owned::Game, local: Option<&Local>) -> State {
    match local {
        Some(local) => match (local.state, &local.environment) {
            (LocalState::Unfinished, _) => State::Unfinished,
            (LocalState::Downloaded, _) => State::Downloaded,
            (LocalState::Installed, Some(name)) => State::Installed(name.clone()),
            (LocalState::Installed, None) => State::Downloaded,
        },
        None => match unavailable(game) {
            Some(why) => State::Unavailable(why),
            None => State::NotInstalled,
        },
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The reason a game can't be installed, as `import epic --list` words it.
fn unavailable(game: &owned::Game) -> Option<&'static str> {
    if game.third_party.is_some() {
        Some("another launcher manages it")
    } else if game.version.is_none() {
        Some("no Windows build")
    } else if game.partner_link.is_some() {
        Some("needs an account at another launcher")
    } else {
        None
    }
}

/// Where a tile's poster comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// The Grid art stored for this environment.
    Stored(String),
    /// Epic's box art, from the owned list's addresses.
    Cover(Vec<Image>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Job {
    pub(crate) id: String,
    pub(crate) source: Source,
}

/// How many covers in a row may fail because the network is down before
/// the rest are left alone for this load.
const OUTAGE_LIMIT: u32 = 3;

fn open_transport() -> Result<Box<dyn Transport>, String> {
    Ok(Box::new(Https::new().map_err(|error| error.to_string())?))
}

/// Loads each job's poster and hands it to `emit`, everything on disk
/// first, then each cover not yet cached, one at a time, like GOG's. A
/// failure leaves only that game's text tile.
pub(crate) fn run_posters(
    paths: &Paths,
    jobs: Vec<Job>,
    token: &Token,
    transport: impl FnOnce() -> Result<Box<dyn Transport>, String>,
    mut emit: impl FnMut(String, Result<image::Handle, String>),
) {
    let mut missing = Vec::new();
    for job in jobs {
        if token.is_canceled() {
            return;
        }
        match job.source {
            Source::Stored(environment) => match stored_grid(paths, &environment) {
                Ok(Some(path)) => match decode(paths, token, &path) {
                    Some(poster) => emit(job.id, poster),
                    None => return,
                },
                Ok(None) => {}
                Err(error) => emit(job.id, Err(error)),
            },
            Source::Cover(images) => match covers::cached(paths, &job.id) {
                Some(path) => match decode_cover(paths, token, &job.id, &path) {
                    Some(poster) => emit(job.id, poster),
                    None => return,
                },
                None => missing.push((job.id, images)),
            },
        }
    }
    if missing.is_empty() {
        return;
    }
    let transport = match transport() {
        Ok(transport) => transport,
        Err(error) => {
            for (id, _) in missing {
                emit(id, Err(error.clone()));
            }
            return;
        }
    };
    let mut outages = 0;
    for (id, images) in missing {
        if token.is_canceled() || outages >= OUTAGE_LIMIT {
            return;
        }
        let poster = match covers::cover(paths, &*transport, &id, &images, token) {
            Ok(path) => {
                outages = 0;
                match decode_cover(paths, token, &id, &path) {
                    Some(poster) => poster,
                    None => return,
                }
            }
            Err(error) => {
                if matches!(error, CoverError::Unavailable(_)) {
                    outages += 1;
                } else {
                    outages = 0;
                }
                Err(error.to_string())
            }
        };
        emit(id, poster);
    }
}

fn stored_grid(paths: &Paths, environment: &str) -> Result<Option<PathBuf>, String> {
    let dir = paths.art_dir(environment);
    let config = ArtConfig::load(&dir).map_err(|error| error.to_string())?;
    let resolved =
        art::resolve(&dir, &config, None, Kind::Grid).map_err(|error| error.to_string())?;
    Ok(resolved.map(|resolved| resolved.found.path))
}

fn decode(
    paths: &Paths,
    token: &Token,
    path: &std::path::Path,
) -> Option<Result<image::Handle, String>> {
    posters::decode_waiting(path, &paths.art_cache_dir(), Some(token))
        .map(|poster| poster.map(|(handle, _)| handle))
}

/// Decodes a cached cover, removing one that can't be decoded so the next
/// load fetches it again.
fn decode_cover(
    paths: &Paths,
    token: &Token,
    app: &str,
    path: &std::path::Path,
) -> Option<Result<image::Handle, String>> {
    let poster = decode(paths, token, path)?;
    if poster.is_err() {
        let _ = covers::discard(paths, app);
    }
    Some(poster)
}

#[cfg(test)]
mod tests;
