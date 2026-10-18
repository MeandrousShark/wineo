use std::collections::HashMap;
use std::path::PathBuf;

use iced::futures::channel::mpsc::unbounded;
use iced::keyboard::{self, Key};
use iced::widget::{column, container, image, row, text, text_input};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::art::Kind;
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::heroic_import::default_name;
use wineo_core::paths::Paths;
use wineo_core::steam;
use wineo_core::steam::InstallState;
use wineo_core::steam_import::{
    self, Applying, EntryState, Failure, NO_KEY_HINT, OwnedData, Wanted,
};

use super::frame;
use super::import_grid::{GridKey, ImportGrid, Tile};
use super::{Action, posters};
use crate::task;
use crate::theme::Palette;
use crate::ui::{accent_base, secondary_button};

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

pub(crate) type Review = steam_import::Batch;
pub(crate) type Report = steam_import::BatchReport;

fn report_lines(report: &Report) -> Vec<String> {
    let noun = if report.total == 1 { "game" } else { "games" };
    let mut lines = vec![format!(
        "Imported {} of {} {noun}.",
        report.done.len(),
        report.total
    )];
    if !report.already.is_empty() {
        lines.push(format!(
            "{} already imported: {}",
            report.already.len(),
            report.already.join(", ")
        ));
    }
    if !report.failures.is_empty() {
        lines.push(format!(
            "{} failed: {}",
            report.failures.len(),
            report
                .failures
                .iter()
                .map(|failure| failure.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines
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
    listing: Option<Listing>,
    /// The owned list once loaded, which planning and importing an
    /// uninstalled game need. It loads once per page, never in the library.
    owned: Option<OwnedData>,
    owned_note: Option<OwnedNote>,
    owned_loading: bool,
    owned_generation: u64,
    /// Picks app ids.
    grid: ImportGrid<Event>,
    posters: HashMap<PosterKey, Option<image::Handle>>,
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
            listing: None,
            owned: None,
            owned_note: None,
            owned_loading: false,
            owned_generation: 0,
            grid: ImportGrid::new(GRID_ID, Event::ProgressDue, 0),
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
            Step::Review if self.grid.chosen.len() == 1 => self.step = Step::Details,
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
        self.grid.clear();
        self.posters.clear();
        self.review = None;
        self.report = None;
        self.error = None;
        self.status = "Reading Steam games...".to_owned();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        task::blocking(
            move || load_listing(&paths),
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
        task::blocking(
            move || {
                load_owned(&paths, refresh, |paths, global, refresh| {
                    OwnedData::load(paths, global, refresh, true)
                })
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
        let jobs = self.posters_to_load(listing);
        self.grid.decode_posters(
            self.paths.art_cache_dir(),
            jobs,
            |(_, dim), path, cache| {
                let (handle, _) = posters::decode(path, cache)?;
                // Grayscale like an uninstalled card in the library.
                Ok(if *dim {
                    crate::artwork::grayscale(&handle).unwrap_or(handle)
                } else {
                    handle
                })
            },
            Event::Poster,
        )
    }

    fn import_selected(&mut self) -> Task<Event> {
        if self.waiting() || self.grid.chosen.is_empty() {
            return Task::none();
        }
        if self.grid.chosen.len() == 1 {
            let Some(game) = self.listing.as_ref().and_then(|listing| {
                listing
                    .games
                    .iter()
                    .find(|game| self.grid.chosen.contains(&game.app_id))
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
        if self.grid.chosen.is_empty() || self.waiting() {
            return Task::none();
        }
        let selected: Vec<_> = listing
            .games
            .iter()
            .filter(|game| self.grid.chosen.contains(&game.app_id))
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
        let work = task::blocking(
            move || {
                run_import(&paths, review, &owned, &token, |message| {
                    let _ = sender.unbounded_send(Event::Progress(generation, message));
                })
            },
            move |result| Event::Finished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Back => {
                // The Back button is a click, so the grid it returns to shows no ring.
                self.grid.focus_visible = false;
                self.back()
            }
            Event::Toggle(index) if self.step == Step::Games && !self.waiting() => {
                if let Some(game) = self
                    .listing
                    .as_ref()
                    .and_then(|listing| listing.games.get(index))
                {
                    self.grid.toggle(&game.app_id);
                    self.grid.click(index);
                }
                Task::none()
            }
            Event::SelectAll if self.step == Step::Games && !self.waiting() => {
                self.grid.chosen = self
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
                self.grid.chosen.clear();
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
                        self.grid.chosen.retain(|app_id| {
                            listing.games.iter().any(|game| &game.app_id == app_id)
                        });
                        self.grid.focus =
                            self.grid.focus.filter(|index| *index < listing.games.len());
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
                self.grid.progress.reveal(generation);
                Task::none()
            }
            Event::Poster(generation, key, result) if generation == self.grid.poster_generation => {
                self.grid.progress.finish_one();
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
        Some(match self.grid.key(key, modifiers, width, count)? {
            GridKey::Moved(task) => task,
            GridKey::Toggle(focus) => {
                if let Some(index) = focus {
                    let _ = self.update(Event::Toggle(index));
                    self.grid.focus_visible = true;
                }
                Task::none()
            }
            GridKey::SelectAll => self.update(Event::SelectAll),
            GridKey::Import => self.import_selected(),
        })
    }

    fn grid<'a>(
        &'a self,
        listing: &'a Listing,
        palette: Palette,
        width: f32,
        header: Element<'a, Event>,
    ) -> Element<'a, Event> {
        let tiles = listing.games.iter().enumerate().map(|(index, game)| {
            (
                Tile {
                    title: &game.title,
                    poster: self
                        .posters
                        .get(&(game.app_id.clone(), !game.installed))
                        .and_then(Option::as_ref),
                    reason: None,
                    note: game.note.map(str::to_owned),
                    chosen: self.grid.chosen.contains(&game.app_id),
                    focus_ring: self.grid.focus_ring(index),
                    dimmed: !game.installed,
                },
                Event::Toggle(index),
            )
        });
        self.grid.view(header, tiles, width, palette)
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
                        let count = self.grid.chosen.len();
                        games_actions = Some(self.grid.footer(
                            Some(Event::SelectAll),
                            (count > 0).then_some(Event::SelectNone),
                            refresh.map(Element::from),
                            (count > 0).then_some(Event::ImportSelected),
                            palette,
                        ));
                    }
                }
            }
            Step::Details => {
                if let Some(game) = self.listing.as_ref().and_then(|listing| {
                    listing
                        .games
                        .iter()
                        .find(|game| self.grid.chosen.contains(&game.app_id))
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
                    for line in report_lines(report) {
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
            _ => frame::scroll_body(body, None, palette),
        };
        let mut page = column![row![back], content]
            .spacing(12)
            .max_width(frame::PAGE_MAX_WIDTH)
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
            page = page.push(frame::pinned(footer, palette));
        }
        frame::page_frame(page)
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
    let games = selected.into_iter().map(|game| Wanted {
        app_id: game.app_id,
        title: Some(game.title),
        imported_as: None,
    });
    match steam_import::plan_batch(
        paths,
        &global,
        owned.games(),
        games,
        name.as_deref(),
        |_| Ok(()),
    ) {
        Ok(review) => review,
        Err(error) => Review {
            failures: vec![Failure {
                app_id: String::new(),
                name: "Steam import".to_owned(),
                reason: error.to_string(),
            }],
            ..Review::default()
        },
    }
}

fn run_import(
    paths: &Paths,
    review: Review,
    owned: &OwnedData,
    token: &Token,
    mut progress: impl FnMut(String),
) -> Result<Report, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    steam_import::apply_batch(paths, &global, owned.games(), review, token, |applying| {
        if let Applying::Starting(plan) = applying {
            progress(format!("Importing {}...", plan.title()));
        }
        Ok(())
    })
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
