use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use iced::futures::channel::mpsc::unbounded;
use iced::keyboard::{self, Key, key::Named};
use iced::widget::{Row, column, container, image, row, text, text_input};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::art::Kind;
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::heroic_import::default_name;
use wineo_core::paths::Paths;
use wineo_core::steam;
use wineo_core::steam::InstallState;
use wineo_core::steam_import::{
    self, ApplyOutcome, EntryState, NO_KEY_HINT, OwnedData, Plan, PlanOutcome,
};

use super::heroic;
use super::import_grid::{self, Tile};
use super::{Action, posters};
use crate::ends::{MIN_LEFT, ends};
use crate::progress::Delayed;
use crate::theme::Palette;
use crate::ui::{accent_base, secondary_button};
use std::time::Instant;

const GRID_ID: &str = "steam-import-grid";

/// A poster by app id and whether it is drawn dimmed, since the grayscale is
/// baked into the handle.
type PosterKey = (String, bool);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Games,
    Details,
    Review,
    Working,
    Finished,
}

#[derive(Debug, Clone)]
struct Candidate {
    app_id: String,
    title: String,
    size: u64,
    note: Option<&'static str>,
    /// False for an owned game Steam doesn't have on disk, which is dimmed.
    installed: bool,
    poster: Option<PathBuf>,
}

/// What the page says about the owned games, beside the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OwnedNote {
    NoKey,
    Problem(String),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Listing {
    games: Vec<Candidate>,
    issues: Vec<String>,
}

#[derive(Debug, Clone)]
struct Failure {
    app_id: String,
    name: String,
    reason: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Review {
    ready: Vec<Plan>,
    already: Vec<String>,
    failures: Vec<Failure>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Report {
    done: Vec<String>,
    total: usize,
    already: Vec<String>,
    failures: Vec<Failure>,
    warnings: Vec<String>,
    canceled: bool,
}

impl Report {
    fn lines(&self) -> Vec<String> {
        let noun = if self.total == 1 { "game" } else { "games" };
        let mut lines = vec![format!(
            "Imported {} of {} {noun}.",
            self.done.len(),
            self.total
        )];
        if !self.already.is_empty() {
            lines.push(format!(
                "{} already imported: {}",
                self.already.len(),
                self.already.join(", ")
            ));
        }
        if !self.failures.is_empty() {
            lines.push(format!(
                "{} failed: {}",
                self.failures.len(),
                self.failures
                    .iter()
                    .map(|failure| failure.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        lines
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Toggle(usize),
    SelectAll,
    SelectNone,
    ImportSelected,
    Name(String),
    Review,
    Import,
    Cancel,
    Force,
    BackToLibrary,
    Listed(u64, Result<Listing, String>),
    /// Fetches the owned games again instead of using the saved list.
    Refresh,
    OpenSteamKey,
    OwnedListed(u64, Result<(OwnedData, Listing), String>),
    Poster(u64, PosterKey, Result<image::Handle, String>),
    /// The delay before the art progress bar shows has passed.
    ProgressDue(u64),
    Planned(u64, Review),
    Progress(u64, String),
    Finished(u64, Result<Report, String>),
}

pub(crate) struct Page {
    paths: Paths,
    step: Step,
    generation: u64,
    poster_generation: u64,
    listing: Option<Listing>,
    /// The owned list once loaded, which planning and importing an
    /// uninstalled game need. It loads once per page, never in the library.
    owned: Option<OwnedData>,
    owned_note: Option<OwnedNote>,
    owned_loading: bool,
    owned_generation: u64,
    chosen: BTreeSet<String>,
    focus: Option<usize>,
    /// Whether `focus` came from the keyboard. A click moves focus too, but
    /// like CSS :focus-visible only keyboard focus draws the ring.
    focus_visible: bool,
    posters: HashMap<PosterKey, Option<image::Handle>>,
    progress: Delayed<Event>,
    name: String,
    review: Option<Review>,
    report: Option<Report>,
    token: Option<Token>,
    status: String,
    error: Option<String>,
    action: Option<Action>,
}

impl Page {
    pub(crate) fn new(paths: Paths) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            step: Step::Games,
            generation: 0,
            poster_generation: 0,
            listing: None,
            owned: None,
            owned_note: None,
            owned_loading: false,
            owned_generation: 0,
            chosen: BTreeSet::new(),
            focus: None,
            focus_visible: false,
            posters: HashMap::new(),
            progress: Delayed::new(Event::ProgressDue),
            name: String::new(),
            review: None,
            report: None,
            token: None,
            status: String::new(),
            error: None,
            action: None,
        };
        let task = page.list();
        (page, task)
    }

    pub(crate) fn busy(&self) -> bool {
        self.token.is_some()
    }

    pub(crate) fn waiting(&self) -> bool {
        !self.status.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn spinning(&self) -> bool {
        self.waiting() || self.owned_loading
    }

    fn owned_games(&self) -> OwnedData {
        self.owned.clone().unwrap_or(OwnedData::Unloaded)
    }

    pub(crate) fn cancel(&mut self, force: bool) {
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
        if self.busy() || self.waiting() {
            return Task::none();
        }
        match self.step {
            Step::Games => self.import_selected(),
            Step::Details => self.plan(),
            Step::Review => self.start_import(),
            Step::Finished => self.update(Event::BackToLibrary),
            Step::Working => Task::none(),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        if self.busy() {
            return Task::none();
        }
        self.generation = self.generation.wrapping_add(1);
        self.status.clear();
        self.error = None;
        match self.step {
            Step::Games => self.action = Some(Action::Choose),
            Step::Details => self.step = Step::Games,
            Step::Review if self.chosen.len() == 1 => self.step = Step::Details,
            Step::Review => self.step = Step::Games,
            Step::Working => {}
            Step::Finished => {
                self.action = Some(Action::Library {
                    name: None,
                    launch: false,
                })
            }
        }
        Task::none()
    }

    fn list(&mut self) -> Task<Event> {
        self.step = Step::Games;
        self.listing = None;
        self.chosen.clear();
        self.focus = None;
        self.focus_visible = false;
        self.posters.clear();
        self.poster_generation = self.poster_generation.wrapping_add(1);
        self.progress.reset();
        self.review = None;
        self.report = None;
        self.error = None;
        self.status = "Reading Steam games...".to_owned();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
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

    /// Loads the owned games off the UI thread, then lists them after the
    /// installed ones.
    fn load_owned(&mut self, refresh: bool) -> Task<Event> {
        self.owned_loading = true;
        self.owned_generation = self.owned_generation.wrapping_add(1);
        let generation = self.owned_generation;
        let paths = self.paths.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    load_owned(&paths, refresh, |paths, global, refresh| {
                        OwnedData::load(paths, global, refresh, true)
                    })
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| Event::OwnedListed(generation, result),
        )
    }

    fn refresh_available(&self) -> bool {
        self.step == Step::Games
            && self.listing.is_some()
            && !self.waiting()
            && !self.owned_loading
            && self.owned_note != Some(OwnedNote::NoKey)
    }

    /// Posters the listing shows that aren't loaded in the color treatment
    /// they need now. Failed loads are tried again.
    fn posters_to_load(&self, listing: &Listing) -> Vec<(PosterKey, PathBuf)> {
        listing
            .games
            .iter()
            .filter_map(|game| {
                let key = (game.app_id.clone(), !game.installed);
                let loaded = matches!(self.posters.get(&key), Some(Some(_)));
                (!loaded).then(|| Some((key, game.poster.clone()?)))?
            })
            .collect()
    }

    fn load_posters(&mut self, listing: &Listing) -> Task<Event> {
        let generation = self.poster_generation;
        let cache = self.paths.art_cache_dir();
        let wanted = self.posters_to_load(listing);
        let shown = self.progress.add(wanted.len(), Instant::now());
        Task::batch(
            wanted
                .into_iter()
                .map(|(key, path)| {
                    let dim = key.1;
                    let cache = cache.clone();
                    Task::perform(
                        async move {
                            posters::bounded(move || {
                                let (handle, _) = posters::decode(&path, &cache)?;
                                // Grayscale like an uninstalled card in the library.
                                Ok(if dim {
                                    crate::artwork::grayscale(&handle).unwrap_or(handle)
                                } else {
                                    handle
                                })
                            })
                            .await
                            .and_then(|result| result)
                        },
                        move |result| Event::Poster(generation, key, result),
                    )
                })
                .chain([shown]),
        )
    }

    fn import_selected(&mut self) -> Task<Event> {
        if self.waiting() || self.chosen.is_empty() {
            return Task::none();
        }
        if self.chosen.len() == 1 {
            let Some(game) = self.listing.as_ref().and_then(|listing| {
                listing
                    .games
                    .iter()
                    .find(|game| self.chosen.contains(&game.app_id))
            }) else {
                return Task::none();
            };
            self.name = default_name(&game.title, None);
            self.step = Step::Details;
            self.error = None;
            Task::none()
        } else {
            self.plan()
        }
    }

    fn plan(&mut self) -> Task<Event> {
        let Some(listing) = &self.listing else {
            return Task::none();
        };
        if self.chosen.is_empty() || self.waiting() {
            return Task::none();
        }
        let selected: Vec<_> = listing
            .games
            .iter()
            .filter(|game| self.chosen.contains(&game.app_id))
            .cloned()
            .collect();
        let name = (selected.len() == 1).then(|| self.name.clone());
        self.error = None;
        self.status = "Checking Steam games...".to_owned();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let owned = self.owned_games();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || plan_review(&paths, selected, name, &owned))
                    .await
                    .unwrap_or_else(|error| Review {
                        failures: vec![Failure {
                            app_id: String::new(),
                            name: "Steam import".to_owned(),
                            reason: error.to_string(),
                        }],
                        ..Review::default()
                    })
            },
            move |review| Event::Planned(generation, review),
        )
    }

    fn start_import(&mut self) -> Task<Event> {
        let Some(review) = self.review.clone() else {
            return Task::none();
        };
        if self.busy() || self.waiting() || (review.ready.is_empty() && review.failures.is_empty())
        {
            return Task::none();
        }
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Working;
        self.status = "Importing from Steam...".to_owned();
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let owned = self.owned_games();
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    run_import(&paths, review, &owned, &token, |message| {
                        let _ = sender.unbounded_send(Event::Progress(generation, message));
                    })
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
            Event::Back => {
                // The Back button is a click, so the grid it returns to shows no ring.
                self.focus_visible = false;
                self.back()
            }
            Event::Toggle(index) if self.step == Step::Games && !self.waiting() => {
                if let Some(game) = self
                    .listing
                    .as_ref()
                    .and_then(|listing| listing.games.get(index))
                {
                    if !self.chosen.insert(game.app_id.clone()) {
                        self.chosen.remove(&game.app_id);
                    }
                    self.focus = Some(index);
                    self.focus_visible = false;
                }
                Task::none()
            }
            Event::SelectAll if self.step == Step::Games && !self.waiting() => {
                self.chosen = self
                    .listing
                    .as_ref()
                    .map(|listing| {
                        listing
                            .games
                            .iter()
                            .map(|game| game.app_id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                Task::none()
            }
            Event::SelectNone if self.step == Step::Games && !self.waiting() => {
                self.chosen.clear();
                Task::none()
            }
            Event::ImportSelected if self.step == Step::Games => self.import_selected(),
            Event::Name(name) if self.step == Step::Details => {
                self.name = name;
                Task::none()
            }
            Event::Review if self.step == Step::Details => self.plan(),
            Event::Import if self.step == Step::Review => self.start_import(),
            Event::Cancel if self.step == Step::Working => {
                self.cancel(false);
                Task::none()
            }
            Event::Force if self.step == Step::Working => {
                self.cancel(true);
                Task::none()
            }
            Event::BackToLibrary if self.step == Step::Finished => {
                self.action = Some(Action::Library {
                    name: None,
                    launch: false,
                });
                Task::none()
            }
            Event::Listed(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(listing) => {
                        let task = self.load_posters(&listing);
                        self.listing = Some(listing);
                        Task::batch([task, self.load_owned(false)])
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Task::none()
                    }
                }
            }
            Event::Refresh if self.refresh_available() => self.load_owned(true),
            Event::OpenSteamKey if self.step == Step::Games && !self.busy() => {
                self.action = Some(Action::SteamKey);
                Task::none()
            }
            Event::OwnedListed(generation, result) if generation == self.owned_generation => {
                self.owned_loading = false;
                match result {
                    Ok((owned, listing)) => {
                        self.owned_note = match &owned {
                            OwnedData::NoKey => Some(OwnedNote::NoKey),
                            other => other.problem().map(OwnedNote::Problem),
                        };
                        self.owned = Some(owned);
                        self.chosen.retain(|app_id| {
                            listing.games.iter().any(|game| &game.app_id == app_id)
                        });
                        self.focus = self.focus.filter(|index| *index < listing.games.len());
                        let task = self.load_posters(&listing);
                        self.listing = Some(listing);
                        task
                    }
                    Err(error) => {
                        self.owned_note = Some(OwnedNote::Problem(error));
                        Task::none()
                    }
                }
            }
            Event::ProgressDue(generation) => {
                self.progress.reveal(generation);
                Task::none()
            }
            Event::Poster(generation, key, result) if generation == self.poster_generation => {
                self.progress.finish_one();
                self.posters.insert(key, result.ok());
                Task::none()
            }
            Event::Planned(generation, review) if generation == self.generation => {
                self.status.clear();
                self.review = Some(review);
                self.step = Step::Review;
                Task::none()
            }
            Event::Progress(generation, message)
                if generation == self.generation && self.step == Step::Working =>
            {
                if self.token.as_ref().is_some_and(Token::is_canceled) {
                    return Task::none();
                }
                self.status = message;
                Task::none()
            }
            Event::Finished(generation, result) if generation == self.generation => {
                self.token = None;
                self.status.clear();
                self.step = Step::Finished;
                match result {
                    Ok(report) => self.report = Some(report),
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    pub(crate) fn key(
        &mut self,
        key: &Key,
        modifiers: keyboard::Modifiers,
        width: f32,
    ) -> Option<Task<Event>> {
        if self.step != Step::Games || self.waiting() {
            return None;
        }
        let count = self.listing.as_ref()?.games.len();
        let columns = import_grid::grid_columns(width);
        match key.as_ref() {
            Key::Named(
                named @ (Named::ArrowLeft | Named::ArrowRight | Named::ArrowUp | Named::ArrowDown),
            ) => {
                self.focus = import_grid::move_focus(self.focus, named, columns, count);
                self.focus_visible = true;
                Some(self.reveal_focus(columns, count))
            }
            Key::Named(Named::Space) | Key::Character(" ") => {
                if let Some(index) = self.focus {
                    let _ = self.update(Event::Toggle(index));
                    self.focus_visible = true;
                }
                Some(Task::none())
            }
            Key::Character("a" | "A") if modifiers.control() => Some(self.update(Event::SelectAll)),
            Key::Named(Named::Enter) if !modifiers.control() => Some(self.import_selected()),
            _ => None,
        }
    }

    fn focus_ring(&self, index: usize) -> bool {
        self.focus_visible && self.focus == Some(index)
    }

    fn reveal_focus(&self, columns: usize, count: usize) -> Task<Event> {
        let Some(index) = self.focus else {
            return Task::none();
        };
        let columns = columns.max(1);
        import_grid::reveal_row(GRID_ID, index / columns, count.div_ceil(columns))
    }

    fn grid<'a>(
        &'a self,
        listing: &'a Listing,
        palette: Palette,
        width: f32,
        header: Element<'a, Event>,
    ) -> Element<'a, Event> {
        let columns = import_grid::grid_columns(width);
        let mut rows = column![].spacing(import_grid::TILE_GAP);
        let tiles: Vec<Element<'a, Event>> = listing
            .games
            .iter()
            .enumerate()
            .map(|(index, game)| {
                import_grid::tile(
                    Tile {
                        title: &game.title,
                        poster: self
                            .posters
                            .get(&(game.app_id.clone(), !game.installed))
                            .and_then(Option::as_ref),
                        reason: None,
                        note: game.note.map(str::to_owned),
                        chosen: self.chosen.contains(&game.app_id),
                        focus_ring: self.focus_ring(index),
                        dimmed: !game.installed,
                    },
                    palette,
                    Event::Toggle(index),
                )
            })
            .collect();
        let mut tiles = tiles.into_iter().peekable();
        while tiles.peek().is_some() {
            rows = rows.push(
                Row::with_children(tiles.by_ref().take(columns)).spacing(import_grid::TILE_GAP),
            );
        }
        import_grid::grid_scroll(GRID_ID, header, rows.into(), palette)
    }

    pub(crate) fn view(&self, palette: Palette, width: f32) -> Element<'_, Event> {
        let title = match self.step {
            Step::Games => "Choose Steam games",
            Step::Details => "Import details",
            Step::Review => "Review Steam import",
            Step::Working => "Importing from Steam",
            Step::Finished => "Steam import finished",
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
        // Pinned under the scrolling grid so it stays reachable in a short dialog.
        let mut games_actions: Option<Element<'_, Event>> = None;
        match self.step {
            Step::Games => {
                if let Some(listing) = &self.listing {
                    for issue in &listing.issues {
                        body = body.push(text(issue).size(12).color(palette.warning));
                    }
                    if self.owned_loading {
                        body = body.push(
                            row![
                                crate::spinner::spinner(palette, 16.0),
                                text("Getting owned games...").size(12).color(palette.muted)
                            ]
                            .spacing(8)
                            .align_y(Center),
                        );
                    }
                    match &self.owned_note {
                        Some(OwnedNote::NoKey) => {
                            body = body.push(
                                row![
                                    text(NO_KEY_HINT).size(12).color(palette.muted),
                                    secondary_button("Open Settings", palette).on_press_maybe(
                                        (!self.waiting()).then_some(Event::OpenSteamKey),
                                    ),
                                ]
                                .spacing(8)
                                .align_y(Center),
                            );
                        }
                        Some(OwnedNote::Problem(problem)) => {
                            body = body.push(text(problem).size(12).color(palette.warning));
                        }
                        None => {}
                    }
                    let refresh = (self.owned_note != Some(OwnedNote::NoKey)).then(|| {
                        secondary_button("Refresh", palette)
                            .on_press_maybe(self.refresh_available().then_some(Event::Refresh))
                    });
                    if listing.games.is_empty() {
                        if !self.owned_loading {
                            body = body.push(
                                text("No Steam games are ready to import.").color(palette.muted),
                            );
                        }
                        if let Some(refresh) = refresh {
                            body = body.push(refresh);
                        }
                    } else {
                        let count = self.chosen.len();
                        let mut others = row![
                            secondary_button("Select all", palette).on_press(Event::SelectAll),
                            secondary_button("Select none", palette)
                                .on_press_maybe((count > 0).then_some(Event::SelectNone)),
                            refresh,
                        ]
                        .spacing(12)
                        .align_y(Center);
                        if let Some(bar) = posters::progress_bar(&self.progress, palette) {
                            others = others.push(bar);
                        }
                        let import = accent_base(import_grid::import_label(count), palette.primary)
                            .on_press_maybe((count > 0).then_some(Event::ImportSelected));
                        // A narrow dialog stacks the import button under the rest.
                        games_actions = Some(ends(
                            others.wrap().vertical_spacing(8),
                            import,
                            12.0,
                            MIN_LEFT,
                        ));
                    }
                }
            }
            Step::Details => {
                if let Some(game) = self.listing.as_ref().and_then(|listing| {
                    listing
                        .games
                        .iter()
                        .find(|game| self.chosen.contains(&game.app_id))
                }) {
                    body = body.push(text(format!("{} ({})", game.title, game.app_id)));
                    body = body.push(text(if game.installed {
                        format!("Size: {}", crate::size::format_bytes(game.size))
                    } else {
                        "Not installed".to_owned()
                    }));
                    body = body.push(
                        column![
                            text("Environment name").size(13),
                            text_input("Environment name", &self.name).on_input(Event::Name)
                        ]
                        .spacing(4),
                    );
                }
            }
            Step::Review => {
                if let Some(review) = &self.review {
                    let mut summary =
                        column![crate::ui::section_heading("Games", palette)].spacing(8);
                    for plan in &review.ready {
                        let size = if plan.state() == InstallState::NotInstalled {
                            "not installed".to_owned()
                        } else {
                            crate::size::format_bytes(plan.size_on_disk())
                        };
                        summary = summary.push(text(format!(
                            "{}: {} ({}) - {size}",
                            plan.name(),
                            plan.title(),
                            plan.app_id(),
                        )));
                    }
                    if !review.ready.is_empty() {
                        summary = summary.push(text("Source: Steam"));
                        summary = summary.push(text("Tags: game"));
                        summary = summary.push(text("Desktop: off"));
                        summary = summary.push(text("Art: Steam cache"));
                    }
                    for name in &review.already {
                        summary = summary.push(text(format!("Already imported as {name}.")));
                    }
                    for failure in &review.failures {
                        summary = summary.push(
                            text(format!("Failed {}: {}", failure.app_id, failure.reason))
                                .color(palette.warning),
                        );
                    }
                    if review.ready.is_empty()
                        && review.failures.is_empty()
                        && review.already.is_empty()
                    {
                        summary = summary
                            .push(text("No games are ready to import.").color(palette.muted));
                    }
                    body = body.push(container(summary).padding(16).width(Fill).style(move |_| {
                        container::Style::default()
                            .background(palette.surface)
                            .border(border::rounded(12).width(1).color(palette.border))
                    }));
                }
            }
            Step::Working => {}
            Step::Finished => {
                if let Some(report) = &self.report {
                    if report.canceled {
                        body = body.push(text("Completed imports remain after canceling."));
                    }
                    for line in report.lines() {
                        body = body.push(text(line));
                    }
                    for failure in &report.failures {
                        body = body.push(
                            text(format!("Failed {}: {}", failure.app_id, failure.reason))
                                .size(12)
                                .color(palette.warning),
                        );
                    }
                    for warning in &report.warnings {
                        body = body.push(text(warning).size(12).color(palette.warning));
                    }
                }
            }
        }
        let back = secondary_button("Back", palette)
            .on_press_maybe((!self.busy() && self.step != Step::Finished).then_some(Event::Back));
        let content: Element<'_, Event> = match (&self.listing, self.step) {
            // The title and notes scroll with the grid, above the fixed action row.
            (Some(listing), Step::Games) if !listing.games.is_empty() => {
                let grid = self.grid(listing, palette, width, body.into());
                column![grid]
                    .push(games_actions)
                    .spacing(14)
                    .height(Fill)
                    .into()
            }
            _ => heroic::scroll_body(body, None, palette),
        };
        let mut page = column![row![back], content]
            .spacing(12)
            .max_width(heroic::PAGE_MAX_WIDTH)
            .height(Fill);
        let footer: Option<Element<'_, Event>> = match self.step {
            Step::Details => Some(
                accent_base("Review", palette.primary)
                    .on_press(Event::Review)
                    .into(),
            ),
            Step::Review => self.review.as_ref().map(|review| {
                accent_base("Import", palette.primary)
                    .on_press_maybe(
                        (!review.ready.is_empty() || !review.failures.is_empty())
                            .then_some(Event::Import),
                    )
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
            Step::Games => None,
        };
        if let Some(footer) = footer {
            page = page.push(container(footer).padding(12).width(Fill).style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(8).width(1).color(palette.border))
            }));
        }
        heroic::page_frame(page)
    }
}

fn load_listing(paths: &Paths) -> Result<Listing, String> {
    load_listing_with(paths, &OwnedData::Unloaded)
}

/// Installed games first, then owned games Steam doesn't have on disk, when
/// the owned list is loaded.
fn load_listing_with(paths: &Paths, owned: &OwnedData) -> Result<Listing, String> {
    if paths.portable_root().is_some() {
        return Err("Steam games can't be imported into a portable folder.".to_owned());
    }
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let entries =
        steam_import::listing(paths, &global, owned.games()).map_err(|error| error.to_string())?;
    let install = steam::active_for(paths, &global);
    let mut listing = Listing::default();
    for entry in entries {
        if entry.imported_as.is_some() {
            continue;
        }
        let (installed, note) = match &entry.state {
            EntryState::Installed => (true, None),
            EntryState::Updating => (true, Some("Updating in Steam")),
            EntryState::Installing => (true, Some("Installing in Steam")),
            EntryState::NotInstalled => (false, None),
            EntryState::InstallQueued => (false, Some("Install queued")),
            EntryState::Uninstalling => (false, Some("Uninstalling")),
            EntryState::Unreadable(reason) => {
                listing
                    .issues
                    .push(format!("Steam app {}: {reason}", entry.app_id));
                continue;
            }
        };
        // Steam caches art for owned games too, not just installed ones.
        let poster = install.as_ref().and_then(|install| {
            steam::art_files(install, &entry.app_id)
                .into_iter()
                .find_map(|(kind, path)| (kind == Kind::Grid).then_some(path))
        });
        listing.games.push(Candidate {
            title: entry.title.unwrap_or_else(|| entry.app_id.clone()),
            app_id: entry.app_id,
            size: entry.size_on_disk.unwrap_or(0),
            note,
            installed,
            poster,
        });
    }
    Ok(listing)
}

/// Loads the owned list with `load`, then the listing with it.
fn load_owned(
    paths: &Paths,
    refresh: bool,
    load: impl FnOnce(&Paths, &GlobalConfig, bool) -> anyhow::Result<OwnedData>,
) -> Result<(OwnedData, Listing), String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let owned = load(paths, &global, refresh).map_err(|error| error.to_string())?;
    let listing = load_listing_with(paths, &owned)?;
    Ok((owned, listing))
}

fn plan_review(
    paths: &Paths,
    selected: Vec<Candidate>,
    name: Option<String>,
    owned: &OwnedData,
) -> Review {
    let mut review = Review::default();
    let global = match GlobalConfig::load(paths) {
        Ok(global) => global,
        Err(error) => {
            review.failures.extend(selected.into_iter().map(|game| {
                Failure {
                    app_id: game.app_id,
                    name: name
                        .clone()
                        .unwrap_or_else(|| default_name(&game.title, None)),
                    reason: error.to_string(),
                }
            }));
            return review;
        }
    };
    let mut names = HashSet::new();
    for game in selected {
        let wanted_name = name.as_deref();
        match steam_import::plan(paths, &global, owned.games(), &game.app_id, wanted_name) {
            Ok(PlanOutcome::Ready(plan)) if names.insert(plan.name().to_owned()) => {
                review.ready.push(plan);
            }
            Ok(PlanOutcome::Ready(plan)) => review.failures.push(Failure {
                app_id: game.app_id,
                name: plan.name().to_owned(),
                reason: wineo_core::Error::EnvironmentExists(plan.name().to_owned()).to_string(),
            }),
            Ok(PlanOutcome::AlreadyImported(existing)) => review.already.push(existing),
            Err(error) => review.failures.push(Failure {
                app_id: game.app_id,
                name: wanted_name
                    .map(str::to_owned)
                    .unwrap_or_else(|| default_name(&game.title, None)),
                reason: format!("{error:#}"),
            }),
        }
    }
    review
}

fn run_import(
    paths: &Paths,
    review: Review,
    owned: &OwnedData,
    token: &Token,
    mut progress: impl FnMut(String),
) -> Result<Report, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let mut report = Report {
        total: review.ready.len(),
        failures: review.failures,
        ..Report::default()
    };
    for plan in review.ready {
        if token.is_canceled() {
            report.canceled = true;
            break;
        }
        progress(format!("Importing {}...", plan.title()));
        match steam_import::apply(paths, &global, owned.games(), &plan, token) {
            Ok(ApplyOutcome::Imported(applied)) => {
                report.done.push(applied.environment.name);
                report.warnings.extend(applied.warnings);
            }
            Ok(ApplyOutcome::AlreadyImported(name)) => {
                report.total -= 1;
                report.already.push(name);
            }
            Err(_) if token.is_canceled() => {
                report.canceled = true;
                break;
            }
            Err(error) => report.failures.push(Failure {
                app_id: plan.app_id().to_owned(),
                name: plan.name().to_owned(),
                reason: format!("{error:#}"),
            }),
        }
    }
    if token.is_canceled() {
        report.canceled = true;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::io::Cursor;
    use std::path::Path;

    use ::image::{ImageFormat, Rgba, RgbaImage};
    use tempfile::TempDir;

    use super::*;

    fn paths(root: &Path, portable: bool) -> Paths {
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(&home)),
            portable.then(|| root.join("portable")).as_deref(),
        )
        .unwrap()
    }

    fn manifest(root: &Path, app_id: &str, title: &str, flags: u32) {
        fs::write(
            root.join(format!("steamapps/appmanifest_{app_id}.acf")),
            format!(
                "\"AppState\" {{ \"appid\" \"{app_id}\" \"name\" \"{title}\" \"StateFlags\" \"{flags}\" \"installdir\" \"{title}\" \"SizeOnDisk\" \"1024\" }}"
            ),
        )
        .unwrap();
    }

    #[test]
    fn page_import_flow_drops_stale_results() {
        let temp = TempDir::new().unwrap();
        let paths = paths(temp.path(), false);
        let steam = temp.path().join("home/.local/share/Steam");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        manifest(&steam, "1", "Alpha", 4);
        let (mut page, _) = Page::new(paths.clone());
        let listed_generation = page.generation;

        let _ = page.update(Event::Listed(listed_generation - 1, Ok(Listing::default())));
        assert!(page.listing.is_none());
        assert!(page.waiting());
        let _ = page.update(Event::Listed(listed_generation, load_listing(&paths)));
        assert_eq!(page.step, Step::Games);
        assert!(!page.waiting());
        assert_eq!(page.listing.as_ref().unwrap().games.len(), 1);

        let _ = page.update(Event::Toggle(0));
        let _ = page.update(Event::ImportSelected);
        assert_eq!(page.step, Step::Details);
        assert_eq!(page.name, "alpha");
        let _ = page.update(Event::Review);
        let planned_generation = page.generation;
        assert!(page.waiting());
        let _ = page.update(Event::Planned(listed_generation, Review::default()));
        assert_eq!(page.step, Step::Details);
        assert!(page.review.is_none());
        assert!(page.waiting());
        let review = plan_review(
            &paths,
            page.listing.as_ref().unwrap().games.clone(),
            Some(page.name.clone()),
            &OwnedData::Unloaded,
        );
        let _ = page.update(Event::Planned(planned_generation, review));
        assert_eq!(page.step, Step::Review);
        assert!(!page.waiting());
        assert_eq!(page.review.as_ref().unwrap().ready.len(), 1);

        let _ = page.update(Event::Import);
        let finished_generation = page.generation;
        assert_eq!(page.step, Step::Working);
        assert!(page.busy());
        assert!(page.waiting());
        let _ = page.update(Event::Finished(planned_generation, Ok(Report::default())));
        assert_eq!(page.step, Step::Working);
        assert!(page.busy());
        assert!(page.report.is_none());
        let _ = page.update(Event::Progress(
            finished_generation,
            "Importing Alpha...".into(),
        ));
        assert_eq!(page.status, "Importing Alpha...");
        let report = run_import(
            &paths,
            page.review.clone().unwrap(),
            &OwnedData::Unloaded,
            page.token.as_ref().unwrap(),
            |_| {},
        );
        let _ = page.update(Event::Finished(finished_generation, report));
        assert_eq!(page.step, Step::Finished);
        assert!(!page.busy());
        assert!(!page.waiting());
        assert_eq!(
            page.report.as_ref().unwrap().lines(),
            ["Imported 1 of 1 game."]
        );
        assert!(paths.environment_config("alpha").is_file());
        let _ = page.update(Event::BackToLibrary);
        assert!(matches!(
            page.take_action(),
            Some(Action::Library {
                name: None,
                launch: false
            })
        ));
    }

    #[test]
    fn page_ignores_progress_after_finished() {
        let temp = TempDir::new().unwrap();
        let (mut page, _) = Page::new(paths(temp.path(), false));
        page.step = Step::Working;
        page.token = Some(Token::new());
        let generation = page.generation;
        let _ = page.update(Event::Finished(generation, Ok(Report::default())));
        let _ = page.update(Event::Progress(generation, "Importing Alpha...".into()));
        assert_eq!(page.step, Step::Finished);
        assert!(!page.busy());
        assert!(!page.waiting());
        assert!(page.status.is_empty());
        assert!(page.report.is_some());
    }

    #[test]
    fn page_does_not_import_an_empty_or_already_imported_review() {
        let temp = TempDir::new().unwrap();
        for already in [Vec::new(), vec!["alpha".to_owned()]] {
            let (mut page, _) = Page::new(paths(temp.path(), false));
            let _ = page.update(Event::Planned(
                page.generation,
                Review {
                    already: already.clone(),
                    ..Review::default()
                },
            ));
            let generation = page.generation;
            let _ = page.submit();
            let _ = page.update(Event::Import);
            assert_eq!(page.step, Step::Review);
            assert_eq!(page.generation, generation);
            assert!(!page.busy());
            assert!(!page.waiting());
            assert!(page.report.is_none());
            assert_eq!(page.review.as_ref().unwrap().already, already);
        }
    }

    #[test]
    fn lists_only_unimported_installed_games_and_keeps_healthy_manifests() {
        let temp = TempDir::new().unwrap();
        let paths = paths(temp.path(), false);
        let steam = temp.path().join("home/.local/share/Steam");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        manifest(&steam, "1", "Alpha", 4);
        manifest(&steam, "2", "Beta", 4);
        manifest(&steam, "3", "Gamma", 0);
        fs::write(steam.join("steamapps/appmanifest_4.acf"), "broken").unwrap();
        let global = GlobalConfig::load(&paths).unwrap();
        let PlanOutcome::Ready(plan) =
            steam_import::plan(&paths, &global, OwnedData::Unloaded.games(), "1", None).unwrap()
        else {
            panic!("expected ready plan")
        };
        steam_import::apply(
            &paths,
            &global,
            OwnedData::Unloaded.games(),
            &plan,
            &Token::new(),
        )
        .unwrap();

        let listing = load_listing(&paths).unwrap();
        assert_eq!(listing.games.len(), 1);
        assert_eq!(listing.games[0].app_id, "2");
        assert_eq!(listing.issues.len(), 1);
        assert!(listing.issues[0].contains("4"));
    }

    #[test]
    fn notes_tell_installing_from_updating() {
        let temp = TempDir::new().unwrap();
        let paths = paths(temp.path(), false);
        let steam = temp.path().join("home/.local/share/Steam");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        manifest(&steam, "1", "Alpha", 4);
        manifest(&steam, "2", "Beta", 1028);
        manifest(&steam, "3", "Gamma", 1026);
        let notes: Vec<_> = load_listing(&paths)
            .unwrap()
            .games
            .iter()
            .map(|game| game.note)
            .collect();
        assert_eq!(
            notes,
            [None, Some("Updating in Steam"), Some("Installing in Steam"),]
        );
    }

    #[test]
    fn plans_and_imports_selected_games_with_cli_summary() {
        let temp = TempDir::new().unwrap();
        let paths = paths(temp.path(), false);
        let steam = temp.path().join("home/.local/share/Steam");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        manifest(&steam, "1", "Alpha", 4);
        manifest(&steam, "2", "Beta", 4);
        let cache = steam.join("appcache/librarycache/1");
        fs::create_dir_all(&cache).unwrap();
        let mut art = Cursor::new(Vec::new());
        RgbaImage::from_pixel(2, 2, Rgba([17, 23, 42, 255]))
            .write_to(&mut art, ImageFormat::Png)
            .unwrap();
        fs::write(cache.join("library_600x900.jpg"), art.into_inner()).unwrap();
        let listing = load_listing(&paths).unwrap();
        assert_eq!(
            listing.games[0].poster.as_deref(),
            Some(cache.join("library_600x900.jpg").as_path())
        );
        let review = plan_review(&paths, listing.games, None, &OwnedData::Unloaded);
        assert_eq!(review.ready.len(), 2);
        let report =
            run_import(&paths, review, &OwnedData::Unloaded, &Token::new(), |_| {}).unwrap();
        assert_eq!(report.lines(), ["Imported 2 of 2 games."]);
        assert!(paths.environment_config("alpha").is_file());
        assert!(paths.environment_config("beta").is_file());
        assert!(paths.art_dir("alpha").join("grid.png").is_file());
    }

    fn listed(count: usize) -> Listing {
        Listing {
            games: (0..count)
                .map(|index| Candidate {
                    app_id: index.to_string(),
                    title: format!("Game {index}"),
                    size: 1,
                    note: None,
                    installed: true,
                    poster: None,
                })
                .collect(),
            ..Listing::default()
        }
    }

    #[test]
    fn every_step_sits_inside_the_dialog_padding() {
        use crate::layout_probe::{bounds_at, probe, within};
        let temp = TempDir::new().unwrap();
        let (mut page, _) = Page::new(paths(temp.path(), false));
        page.status.clear();
        let mut listing = listed(40);
        listing.games[0].title =
            "Grand Theft Auto IV: The Complete Edition and Episodes from Liberty City".to_owned();
        listing.games[3].installed = false;
        listing.games[3].note = Some("Updating in Steam");
        listing.issues = vec!["A Steam library folder couldn't be read.".to_owned()];
        page.listing = Some(listing);
        page.chosen = ["0", "3", "7"].map(str::to_owned).into();
        page.name = "grand-theft-auto-iv-the-complete-edition".to_owned();
        let failure = Failure {
            app_id: "12210".to_owned(),
            name: "grand-theft-auto-iv-the-complete-edition".to_owned(),
            reason:
                "Steam no longer lists the install folder for this game, so it can't be imported"
                    .to_owned(),
        };
        page.review = Some(Review {
            ready: Vec::new(),
            already: vec!["portal-2".to_owned()],
            failures: vec![failure.clone()],
        });
        page.report = Some(Report {
            done: vec!["half-life-2".to_owned()],
            total: 3,
            already: vec!["portal-2".to_owned()],
            failures: vec![failure],
            warnings: vec!["Desktop entry: the menu folder couldn't be written".to_owned()],
            canceled: true,
        });
        // The smallest dialog the window allows, a small one, and the widest.
        for (width, height) in [(280.0, 240.0), (360.0, 280.0), (900.0, 720.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            for step in [
                Step::Games,
                Step::Details,
                Step::Review,
                Step::Working,
                Step::Finished,
            ] {
                page.step = step;
                page.token = (step == Step::Working).then(Token::new);
                let mut view = page.view(Palette::DARK, width);
                let probe = probe(&mut view, iced::Size::new(width, height));
                let node = &probe.node;
                let spilled = probe.outside(inset);
                assert_eq!(probe.scrollables.len(), 1, "one scrolling region");
                assert!(
                    spilled.is_empty(),
                    "{step:?} at {width}x{height}: {spilled:?}"
                );
                let back = bounds_at(node, &[0, 0, 0]);
                assert!(within(back, inset), "{step:?}: {back:?}");
                if step != Step::Games {
                    let footer = bounds_at(node, &[0, 2, 0]);
                    assert!(within(footer, inset), "{step:?}: {footer:?}");
                }
            }
        }
    }

    #[test]
    fn a_short_dialog_keeps_the_actions_in_frame_and_the_grid_scrolling() {
        use crate::layout_probe::{bounds_at, probe, within};
        let temp = TempDir::new().unwrap();
        let (mut page, _) = Page::new(paths(temp.path(), false));
        page.status.clear();
        page.listing = Some(listed(40));
        page.step = Step::Games;
        for (width, height) in [(280.0, 240.0), (360.0, 200.0), (900.0, 240.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view(Palette::DARK, width);
            let probe = probe(&mut view, iced::Size::new(width, height));
            assert_eq!(probe.scrollables.len(), 1, "one scrolling region");
            let (viewport, content) = probe.scrollables[0];
            assert!(viewport.height > 0.0, "{width}x{height}: {viewport:?}");
            assert!(
                (content.y - viewport.y).abs() < 0.01 && content.height > viewport.height,
                "{width}x{height}: {viewport:?} {content:?}"
            );
            let actions = bounds_at(&probe.node, &[0, 1, 1]);
            assert!(within(actions, inset), "{width}x{height}: {actions:?}");
            assert!(
                actions.y >= viewport.y + viewport.height - 0.01,
                "{width}x{height}: {actions:?} under {viewport:?}"
            );
        }
    }

    #[test]
    fn only_keyboard_focus_draws_the_ring() {
        let temp = TempDir::new().unwrap();
        let (mut page, _) = Page::new(paths(temp.path(), false));
        page.status.clear();
        page.listing = Some(listed(3));
        let none = keyboard::Modifiers::empty();
        let _ = page.update(Event::Toggle(1));
        assert_eq!(page.focus, Some(1));
        assert!(!page.focus_ring(1));
        let _ = page.key(&Key::Named(Named::ArrowRight), none, 900.0);
        assert!(page.focus_ring(2));
        let _ = page.update(Event::Toggle(0));
        assert!(!page.focus_ring(0));
        let _ = page.key(&Key::Named(Named::Space), none, 900.0);
        assert!(page.focus_ring(0));
    }

    #[test]
    fn mouse_back_and_reopening_show_no_ring_until_a_key() {
        let temp = TempDir::new().unwrap();
        let paths = paths(temp.path(), false);
        let none = keyboard::Modifiers::empty();
        let (mut page, _) = Page::new(paths.clone());
        page.status.clear();
        page.listing = Some(listed(3));
        let _ = page.key(&Key::Named(Named::ArrowRight), none, 900.0);
        assert!(page.focus_ring(0));
        page.step = Step::Details;
        let _ = page.update(Event::Back);
        assert_eq!(page.step, Step::Games);
        assert!(!page.focus_ring(0));
        let _ = page.key(&Key::Named(Named::ArrowRight), none, 900.0);
        assert!(page.focus_ring(1));

        // Leaving from the grid closes the page, and Add opens a fresh one.
        let mut flow = super::super::Page::Choose(paths.clone());
        let _ = flow.update(super::super::Event::Steam);
        let super::super::Page::Steam(reopened) = &mut flow else {
            panic!("expected the Steam page");
        };
        reopened.listing = Some(listed(3));
        reopened.status.clear();
        assert!((0..3).all(|index| !reopened.focus_ring(index)));
        let _ = flow.key(&Key::Named(Named::ArrowDown), none, 900.0);
        let super::super::Page::Steam(reopened) = &flow else {
            unreachable!()
        };
        assert!(reopened.focus_ring(0));
    }

    #[test]
    fn grid_keys_toggle_and_select_every_listed_game() {
        let temp = TempDir::new().unwrap();
        let (mut page, _) = Page::new(paths(temp.path(), false));
        page.status.clear();
        page.listing = Some(Listing {
            games: vec![
                Candidate {
                    app_id: "1".to_owned(),
                    title: "Alpha".to_owned(),
                    size: 1,
                    note: None,
                    installed: true,
                    poster: None,
                },
                Candidate {
                    app_id: "2".to_owned(),
                    title: "Beta".to_owned(),
                    size: 2,
                    note: Some("Updating in Steam"),
                    installed: true,
                    poster: None,
                },
            ],
            ..Listing::default()
        });
        let none = keyboard::Modifiers::empty();
        let key = |page: &mut Page, key: Key, modifiers| page.key(&key, modifiers, 900.0);
        assert!(key(&mut page, Key::Named(Named::ArrowRight), none).is_some());
        assert_eq!(page.focus, Some(0));
        let _ = key(&mut page, Key::Named(Named::Space), none);
        assert_eq!(
            page.chosen.iter().map(String::as_str).collect::<Vec<_>>(),
            ["1"]
        );
        let _ = key(&mut page, Key::Named(Named::ArrowRight), none);
        let _ = key(&mut page, Key::Named(Named::Space), none);
        assert_eq!(
            page.chosen.iter().map(String::as_str).collect::<Vec<_>>(),
            ["1", "2"]
        );
        let _ = page.update(Event::SelectNone);
        let _ = key(
            &mut page,
            Key::Character("a".into()),
            keyboard::Modifiers::CTRL,
        );
        assert_eq!(
            page.chosen.iter().map(String::as_str).collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert!(key(&mut page, Key::Named(Named::Enter), none).is_some());
        assert_eq!(page.status, "Checking Steam games...");
    }

    #[test]
    fn planning_failure_is_listed_but_excluded_from_cli_total() {
        let temp = TempDir::new().unwrap();
        let paths = paths(temp.path(), false);
        let steam = temp.path().join("home/.local/share/Steam");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        manifest(&steam, "1", "Alpha", 4);
        manifest(&steam, "2", "Alpha", 4);
        let review = plan_review(
            &paths,
            load_listing(&paths).unwrap().games,
            None,
            &OwnedData::Unloaded,
        );
        assert_eq!(review.ready.len(), 1);
        assert_eq!(review.failures.len(), 1);
        let report =
            run_import(&paths, review, &OwnedData::Unloaded, &Token::new(), |_| {}).unwrap();
        assert_eq!(report.lines(), ["Imported 1 of 1 game.", "1 failed: alpha"]);
    }

    #[test]
    fn apply_time_imports_are_reported_without_counting_them_twice() {
        let temp = TempDir::new().unwrap();
        let paths = paths(temp.path(), false);
        let steam = temp.path().join("home/.local/share/Steam");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        manifest(&steam, "1", "Alpha", 4);
        manifest(&steam, "2", "Beta", 4);
        let review = plan_review(
            &paths,
            load_listing(&paths).unwrap().games,
            None,
            &OwnedData::Unloaded,
        );
        let global = GlobalConfig::load(&paths).unwrap();
        steam_import::apply(
            &paths,
            &global,
            wineo_core::steam_import::OwnedGames::Unloaded,
            &review.ready[0],
            &Token::new(),
        )
        .unwrap();

        let report =
            run_import(&paths, review, &OwnedData::Unloaded, &Token::new(), |_| {}).unwrap();
        assert_eq!(
            report.lines(),
            ["Imported 1 of 1 game.", "1 already imported: alpha"]
        );
    }

    #[test]
    fn portable_mode_uses_the_cli_refusal() {
        let temp = TempDir::new().unwrap();
        let paths = paths(temp.path(), true);
        assert_eq!(
            load_listing(&paths).unwrap_err(),
            "Steam games can't be imported into a portable folder."
        );
    }

    #[test]
    fn summary_keeps_apply_time_skips_out_of_the_total() {
        let report = Report {
            done: vec!["alpha".to_owned()],
            total: 2,
            already: vec!["beta".to_owned()],
            failures: vec![Failure {
                app_id: "3".to_owned(),
                name: "gamma".to_owned(),
                reason: "broken manifest".to_owned(),
            }],
            ..Report::default()
        };
        assert_eq!(
            report.lines(),
            [
                "Imported 1 of 2 games.",
                "1 already imported: beta",
                "1 failed: gamma"
            ]
        );
    }

    const STEAM_ID: u64 = 76_561_197_960_265_729;

    fn login(steam: &Path, users: &[(u64, bool)]) {
        let body: String = users
            .iter()
            .map(|(id, recent)| format!("\"{id}\" {{ \"MostRecent\" \"{}\" }}", u8::from(*recent)))
            .collect();
        fs::create_dir_all(steam.join("config")).unwrap();
        fs::write(
            steam.join("config/loginusers.vdf"),
            format!("\"users\" {{ {body} }}"),
        )
        .unwrap();
    }

    fn owned_cache(paths: &Paths, fetched_at: i64, games: &[(&str, &str)]) {
        let games: Vec<_> = games
            .iter()
            .map(|(app_id, name)| serde_json::json!({ "app_id": app_id, "name": name }))
            .collect();
        let path = paths.steam_owned_cache(STEAM_ID);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            serde_json::json!({ "fetched_at": fetched_at, "games": games }).to_string(),
        )
        .unwrap();
    }

    /// Steam's API at an address nothing listens on.
    fn unreachable() -> Result<wineo_core::steam::api::SteamApi, wineo_core::steam::api::ApiError> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        wineo_core::steam::api::SteamApi::with_base(&base)
    }

    fn owned_with_key(
        key: bool,
    ) -> impl FnOnce(&Paths, &GlobalConfig, bool) -> anyhow::Result<OwnedData> {
        move |paths, global, refresh| {
            OwnedData::load_with(
                paths,
                global,
                refresh,
                true,
                |name| {
                    (key && name == wineo_core::steam::key::KEY_VAR)
                        .then(|| OsString::from("SECRETKEY"))
                },
                &wineo_core::key_store::NoKeyring,
                unreachable,
            )
        }
    }

    /// A Steam root with Alpha installed and an account to look up.
    fn owned_fixture(temp: &TempDir) -> (Paths, PathBuf) {
        let paths = paths(temp.path(), false);
        let steam = temp.path().join("home/.local/share/Steam");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        manifest(&steam, "1", "Alpha", 4);
        login(&steam, &[(STEAM_ID, true)]);
        (paths, steam)
    }

    fn listed_page(paths: &Paths) -> Page {
        let (mut page, _) = Page::new(paths.clone());
        let generation = page.generation;
        let _ = page.update(Event::Listed(generation, load_listing(paths)));
        assert!(
            page.owned_loading,
            "the owned list loads once the page lists"
        );
        assert!(page.spinning());
        assert!(!page.waiting(), "installed games stay usable meanwhile");
        page
    }

    #[test]
    fn owned_games_are_listed_after_installed_ones_dimmed_and_import() {
        let temp = TempDir::new().unwrap();
        let (paths, steam) = owned_fixture(&temp);
        let now = jiff::Timestamp::now().as_second();
        owned_cache(
            &paths,
            now,
            &[
                ("3", "Gamma"),
                ("1", "Alpha"),
                ("2", "Beta"),
                ("1493710", "Proton Experimental"),
            ],
        );
        let cache = steam.join("appcache/librarycache/2");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("library_600x900.jpg"), b"art").unwrap();

        let mut page = listed_page(&paths);
        let _ = page.update(Event::Toggle(0));
        let result = load_owned(&paths, false, owned_with_key(true));
        let _ = page.update(Event::OwnedListed(page.owned_generation, result));
        assert!(!page.owned_loading);
        assert_eq!(page.owned_note, None);
        let games = page.listing.as_ref().unwrap().games.clone();
        let shown: Vec<_> = games
            .iter()
            .map(|game| (game.app_id.as_str(), game.installed))
            .collect();
        assert_eq!(shown, [("1", true), ("2", false), ("3", false)]);
        assert_eq!(
            games[1].poster.as_deref(),
            Some(cache.join("library_600x900.jpg").as_path())
        );
        assert_eq!(games[1].note, None);
        // The installed game picked before the list landed stays picked.
        assert!(page.chosen.contains("1"));

        let _ = page.update(Event::SelectAll);
        assert_eq!(page.chosen.len(), 3);
        let selected = games.clone();
        let review = plan_review(&paths, selected, None, page.owned.as_ref().unwrap());
        assert_eq!(review.ready.len(), 3, "{:?}", review.failures);
        assert_eq!(review.ready[1].state(), InstallState::NotInstalled);
        let report = run_import(
            &paths,
            review,
            page.owned.as_ref().unwrap(),
            &Token::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(report.lines(), ["Imported 3 of 3 games."]);
        let config = fs::read_to_string(paths.environment_config("beta")).unwrap();
        assert!(config.contains("app_id = \"2\""));
        assert!(config.contains("title = \"Beta\""));
    }

    #[test]
    fn without_a_key_installed_games_show_with_a_link_to_settings() {
        let temp = TempDir::new().unwrap();
        let (paths, _) = owned_fixture(&temp);
        let mut page = listed_page(&paths);
        let result = load_owned(&paths, false, owned_with_key(false));
        let _ = page.update(Event::OwnedListed(page.owned_generation, result));
        assert_eq!(page.owned_note, Some(OwnedNote::NoKey));
        assert_eq!(
            NO_KEY_HINT,
            "Add a Steam Web API key to see uninstalled games."
        );
        assert_eq!(page.listing.as_ref().unwrap().games.len(), 1);
        assert!(!page.refresh_available());
        let _ = page.update(Event::Refresh);
        assert!(!page.owned_loading);

        let _ = page.update(Event::OpenSteamKey);
        assert!(matches!(page.take_action(), Some(Action::SteamKey)));
    }

    #[test]
    fn a_failed_fetch_without_a_cache_lists_installed_games_and_says_why() {
        let temp = TempDir::new().unwrap();
        let (paths, _) = owned_fixture(&temp);
        let mut page = listed_page(&paths);
        let result = load_owned(&paths, false, owned_with_key(true));
        let _ = page.update(Event::OwnedListed(page.owned_generation, result));
        assert_eq!(
            page.owned_note,
            Some(OwnedNote::Problem(
                "Couldn't get owned games: can't reach Steam".to_owned()
            ))
        );
        assert_eq!(page.listing.as_ref().unwrap().games.len(), 1);
        assert!(page.refresh_available());
    }

    #[test]
    fn refresh_forces_a_fetch_and_falls_back_to_the_cache() {
        let temp = TempDir::new().unwrap();
        let (paths, _) = owned_fixture(&temp);
        let now = jiff::Timestamp::now().as_second();
        owned_cache(&paths, now, &[("2", "Beta")]);
        let mut page = listed_page(&paths);
        let first = page.owned_generation;
        let result = load_owned(&paths, false, owned_with_key(true));
        let _ = page.update(Event::OwnedListed(first, result));
        assert_eq!(page.owned_note, None);

        let _ = page.update(Event::Refresh);
        assert!(page.owned_loading);
        assert_ne!(page.owned_generation, first);
        // An answer from before the refresh can't land on top of it.
        let _ = page.update(Event::OwnedListed(first, Err("stale".to_owned())));
        assert!(page.owned_loading);
        let result = load_owned(&paths, true, owned_with_key(true));
        let _ = page.update(Event::OwnedListed(page.owned_generation, result));
        assert_eq!(
            page.owned_note,
            Some(OwnedNote::Problem(
                "Couldn't refresh owned games: can't reach Steam".to_owned()
            ))
        );
        let games = &page.listing.as_ref().unwrap().games;
        assert_eq!(games.len(), 2);
        assert!(!games[1].installed);
    }

    #[test]
    fn several_accounts_with_none_most_recent_show_that_error() {
        let temp = TempDir::new().unwrap();
        let (paths, steam) = owned_fixture(&temp);
        login(&steam, &[(STEAM_ID, false), (STEAM_ID + 1, false)]);
        let mut page = listed_page(&paths);
        let result = load_owned(&paths, false, owned_with_key(true));
        let _ = page.update(Event::OwnedListed(page.owned_generation, result));
        assert_eq!(
            page.owned_note,
            Some(OwnedNote::Problem(
                "Steam doesn't say which account was used last".to_owned()
            ))
        );
        assert_eq!(page.listing.as_ref().unwrap().games.len(), 1);
        assert!(page.owned.is_none());
    }

    #[test]
    fn art_progress_counts_posters_after_the_delay_and_ignores_stale_ones() {
        let temp = TempDir::new().unwrap();
        let (paths, steam) = owned_fixture(&temp);
        let cache = steam.join("appcache/librarycache/1");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("library_600x900.jpg"), b"art").unwrap();
        let mut page = listed_page(&paths);
        let wanted = page.posters_to_load(page.listing.as_ref().unwrap());
        let total = wanted.len();
        assert!(total >= 1);

        assert_eq!(page.progress.shown(), None, "not before the delay");
        assert!(page.progress.pending_wakeup().is_some());
        let _ = page.update(Event::ProgressDue(page.progress.wakeup_generation()));
        assert_eq!(page.progress.shown(), Some((0, total)));
        crate::layout_probe::layout(
            &mut page.view(Palette::DARK, 680.0),
            iced::Size::new(680.0, 560.0),
        );

        let generation = page.poster_generation;
        let _ = page.update(Event::Poster(
            generation.wrapping_sub(1),
            wanted[0].0.clone(),
            Err("late".into()),
        ));
        assert_eq!(page.progress.shown(), Some((0, total)));
        for (key, _) in wanted {
            let _ = page.update(Event::Poster(generation, key, Err("bad".into())));
        }
        assert_eq!(page.progress.shown(), None, "hidden at completion");
        assert_eq!(page.progress.pending_wakeup(), None);
    }

    #[test]
    fn art_progress_never_shows_when_the_posters_finish_before_the_delay() {
        let temp = TempDir::new().unwrap();
        let (paths, steam) = owned_fixture(&temp);
        let cache = steam.join("appcache/librarycache/1");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("library_600x900.jpg"), b"art").unwrap();
        let mut page = listed_page(&paths);
        let wanted = page.posters_to_load(page.listing.as_ref().unwrap());
        let late = page.progress.wakeup_generation();
        let generation = page.poster_generation;
        for (key, _) in wanted {
            let _ = page.update(Event::Poster(generation, key, Err("bad".into())));
        }
        assert_eq!(page.progress.pending_wakeup(), None);
        let _ = page.update(Event::ProgressDue(late));
        assert_eq!(page.progress.shown(), None);
    }

    #[test]
    fn refresh_reloads_posters_whose_install_state_flipped_and_retries_failures() {
        let temp = TempDir::new().unwrap();
        let (paths, steam) = owned_fixture(&temp);
        let now = jiff::Timestamp::now().as_second();
        owned_cache(&paths, now, &[("1", "Alpha"), ("2", "Beta")]);
        for app_id in ["1", "2"] {
            let cache = steam.join(format!("appcache/librarycache/{app_id}"));
            fs::create_dir_all(&cache).unwrap();
            fs::write(cache.join("library_600x900.jpg"), b"art").unwrap();
        }
        let keys = |page: &Page| {
            let mut keys: Vec<_> = page
                .posters_to_load(page.listing.as_ref().unwrap())
                .into_iter()
                .map(|(key, _)| key)
                .collect();
            keys.sort();
            keys
        };
        let handle = || image::Handle::from_rgba(1, 1, vec![10, 80, 240, 255]);

        let mut page = listed_page(&paths);
        let result = load_owned(&paths, false, owned_with_key(true));
        let _ = page.update(Event::OwnedListed(page.owned_generation, result));
        assert_eq!(
            keys(&page),
            [("1".to_owned(), false), ("2".to_owned(), true)]
        );
        let generation = page.poster_generation;
        let _ = page.update(Event::Poster(
            generation,
            ("1".into(), false),
            Err("bad".into()),
        ));
        let _ = page.update(Event::Poster(generation, ("2".into(), true), Ok(handle())));
        assert_eq!(keys(&page), [("1".to_owned(), false)]);

        // Steam installs Beta behind wineo's back.
        manifest(&steam, "2", "Beta", 4);
        let _ = page.update(Event::Refresh);
        let result = load_owned(&paths, true, owned_with_key(true));
        let _ = page.update(Event::OwnedListed(page.owned_generation, result));
        let games = &page.listing.as_ref().unwrap().games;
        assert!(games.iter().all(|game| game.installed));
        // The gray Beta poster isn't reused, and the failed Alpha one is retried.
        assert_eq!(
            keys(&page),
            [("1".to_owned(), false), ("2".to_owned(), false)]
        );
        assert!(!page.posters.contains_key(&("2".to_owned(), false)));
    }
}
