use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use iced::futures::channel::mpsc::unbounded;
use iced::keyboard::{self, Key, key::Named};
use iced::widget::operation::RelativeOffset;
use iced::widget::{Row, button, column, container, image, row, scrollable, text, text_input};
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

use super::heroic::{self, Tile};
use super::{Action, artwork};
use crate::theme::Palette;

const GRID_ID: &str = "steam-import-grid";

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
    Poster(u64, String, Result<image::Handle, String>),
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
    posters: HashMap<String, Option<image::Handle>>,
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
            posters: HashMap::new(),
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
        self.posters.clear();
        self.poster_generation = self.poster_generation.wrapping_add(1);
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

    fn load_posters(&self, listing: &Listing) -> Task<Event> {
        let generation = self.poster_generation;
        Task::batch(listing.games.iter().filter_map(|game| {
            if self.posters.contains_key(&game.app_id) {
                return None;
            }
            let path = game.poster.clone()?;
            let app_id = game.app_id.clone();
            let dim = !game.installed;
            Some(Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        let handle = artwork::preview_with_format(&path, Kind::Grid)
                            .map(|(handle, _)| handle)?;
                        // Grayscale like an uninstalled card in the library.
                        Ok(if dim {
                            crate::artwork::grayscale(&handle).unwrap_or(handle)
                        } else {
                            handle
                        })
                    })
                    .await
                    .map_err(|error| error.to_string())?
                },
                move |result| Event::Poster(generation, app_id, result),
            ))
        }))
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
            Event::Back => self.back(),
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
            Event::Poster(generation, app_id, result) if generation == self.poster_generation => {
                self.posters.insert(app_id, result.ok());
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
        let columns = heroic::grid_columns(width);
        match key.as_ref() {
            Key::Named(
                named @ (Named::ArrowLeft | Named::ArrowRight | Named::ArrowUp | Named::ArrowDown),
            ) => {
                self.focus = heroic::move_focus(self.focus, named, columns, count);
                Some(self.reveal_focus(columns, count))
            }
            Key::Named(Named::Space) | Key::Character(" ") => {
                if let Some(index) = self.focus {
                    let _ = self.update(Event::Toggle(index));
                }
                Some(Task::none())
            }
            Key::Character("a" | "A") if modifiers.control() => Some(self.update(Event::SelectAll)),
            Key::Named(Named::Enter) if !modifiers.control() => Some(self.import_selected()),
            _ => None,
        }
    }

    fn reveal_focus(&self, columns: usize, count: usize) -> Task<Event> {
        let Some(index) = self.focus else {
            return Task::none();
        };
        let rows = count.div_ceil(columns.max(1));
        let y = if rows > 1 {
            (index / columns.max(1)) as f32 / (rows - 1) as f32
        } else {
            0.0
        };
        iced::widget::operation::snap_to(
            iced::widget::Id::new(GRID_ID),
            RelativeOffset {
                x: None,
                y: Some(y),
            },
        )
    }

    fn grid<'a>(
        &'a self,
        listing: &'a Listing,
        palette: Palette,
        width: f32,
    ) -> Element<'a, Event> {
        let columns = heroic::grid_columns(width);
        let mut rows = column![].spacing(heroic::TILE_GAP);
        let tiles: Vec<Element<'a, Event>> = listing
            .games
            .iter()
            .enumerate()
            .map(|(index, game)| {
                heroic::tile(
                    Tile {
                        title: &game.title,
                        poster: self.posters.get(&game.app_id).and_then(Option::as_ref),
                        reason: None,
                        note: game.note.map(str::to_owned),
                        chosen: self.chosen.contains(&game.app_id),
                        focused: self.focus == Some(index),
                        dimmed: !game.installed,
                    },
                    palette,
                    Event::Toggle(index),
                )
            })
            .collect();
        let mut tiles = tiles.into_iter().peekable();
        while tiles.peek().is_some() {
            rows = rows
                .push(Row::with_children(tiles.by_ref().take(columns)).spacing(heroic::TILE_GAP));
        }
        scrollable(rows)
            .id(iced::widget::Id::new(GRID_ID))
            .width(Fill)
            .height(Fill)
            .into()
    }

    pub(crate) fn view(&self, palette: Palette, width: f32, spin: f32) -> Element<'_, Event> {
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
                    crate::spinner::spinner(palette, spin, 16.0),
                    text(&self.status).color(palette.muted)
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(palette.danger));
        }
        match self.step {
            Step::Games => {
                if let Some(listing) = &self.listing {
                    for issue in &listing.issues {
                        body = body.push(text(issue).size(12).color(palette.warning));
                    }
                    if self.owned_loading {
                        body = body.push(
                            row![
                                crate::spinner::spinner(palette, spin, 16.0),
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
                                    button(text("Open Settings").size(12))
                                        .on_press_maybe(
                                            (!self.waiting()).then_some(Event::OpenSteamKey),
                                        )
                                        .padding([4, 8]),
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
                        button("Refresh")
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
                        body = body.push(
                            row![
                                button("Select all").on_press(Event::SelectAll),
                                button("Select none")
                                    .on_press_maybe((count > 0).then_some(Event::SelectNone)),
                                button(text(heroic::import_label(count)))
                                    .on_press_maybe((count > 0).then_some(Event::ImportSelected)),
                                refresh,
                            ]
                            .spacing(12)
                            .align_y(Center),
                        );
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
        let back = button("< Back")
            .on_press_maybe((!self.busy() && self.step != Step::Finished).then_some(Event::Back));
        let content: Element<'_, Event> = match (&self.listing, self.step) {
            (Some(listing), Step::Games) if !listing.games.is_empty() => container(
                column![body, self.grid(listing, palette, width)]
                    .spacing(14)
                    .height(Fill),
            )
            .max_width(780)
            .width(Fill)
            .height(Fill)
            .padding(24)
            .center_x(Fill)
            .into(),
            _ => scrollable(
                container(body)
                    .max_width(780)
                    .width(Fill)
                    .padding(24)
                    .center_x(Fill),
            )
            .height(Fill)
            .into(),
        };
        let mut page = column![row![back], content].spacing(12);
        let footer: Option<Element<'_, Event>> = match self.step {
            Step::Details => Some(button("Review").on_press(Event::Review).into()),
            Step::Review => self.review.as_ref().map(|review| {
                button("Import")
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
                button(label).on_press(event).into()
            }),
            Step::Finished => Some(
                button("Back to library")
                    .on_press(Event::BackToLibrary)
                    .into(),
            ),
            Step::Games => None,
        };
        if let Some(footer) = footer {
            page = page.push(container(footer).padding(12).width(Fill).style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(1).width(1).color(palette.border))
            }));
        }
        container(page).width(Fill).height(Fill).into()
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
            EntryState::Updating | EntryState::Installing => (true, Some("Updating in Steam")),
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
}
