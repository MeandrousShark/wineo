use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc::unbounded;
use iced::keyboard::{self, Key};
use iced::widget::operation::RelativeOffset;
use iced::widget::{
    checkbox, column, container, image, pick_list, row, scrollable, text, text_input,
};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::art::{Format, Kind};
use wineo_core::cancel::Token;
use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Lazy, Provider};
use wineo_core::environment::{self, Environment};
use wineo_core::gamescope::{Filter, Gamescope, GamescopeSetting, Scaler, Window};
use wineo_core::heroic::{
    HeroicCandidate, HeroicGameKind, HeroicListing, HeroicOrigin, HeroicSharedPrefix, HeroicSource,
    SettingsReview,
};
use wineo_core::heroic_import::{
    self, Entry, Game, LibraryOptions, LibraryPlan, Options, Playtime, Policy, Registered,
    Resolution, RunnerCopy, RunnerSuggestion, SkipReason, Warning,
};
use wineo_core::import;
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;

use super::import_grid::{GridKey, ImportGrid, Tile, import_label};
use super::{Action, artwork, posters};
use crate::chips::Chips;
use crate::runner_label::{self, Choice};
use crate::task;
use crate::theme::Palette;
use crate::ui::{
    PickListStyle, ScrollStyle, accent_base, list_group, list_row, list_row_text, secondary_button,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Sources,
    Games,
    Details,
    Review,
    Working,
    Finished,
}

#[derive(Debug, Clone)]
pub(crate) enum Review {
    Single {
        game: Box<Game>,
        missing_build: Option<(Component, Option<String>)>,
        /// Worked out on the worker, since naming the launcher reads the
        /// disk.
        runner_line: Option<String>,
    },
    Batch(LibraryPlan),
}

#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    candidate: HeroicCandidate,
    runners: Vec<Choice>,
    suggested: Option<RunnerSuggestion>,
    runner: String,
    /// Whether to ask about other launchers' builds.
    offer: bool,
    arguments: Vec<String>,
    unparsed_arguments: Option<String>,
    working_directory: String,
    environment: Vec<String>,
    gamemode: Option<bool>,
    wayland: Option<bool>,
    mangohud: Option<bool>,
    gamescope: Option<GamescopeSetting>,
    hook_paths: Vec<(&'static str, String)>,
    hdr: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct BatchOptions {
    runner: String,
    runners: Vec<Choice>,
    art_fetch_enabled: bool,
    offer: bool,
}

/// The runner choices again after the offer is answered, which can add
/// builds and change what Heroic's runner suggests.
#[derive(Debug, Clone)]
pub(crate) enum Refreshed {
    Single {
        runners: Vec<Choice>,
        suggested: Option<RunnerSuggestion>,
        runner: String,
    },
    Batch(BatchOptions),
}

#[derive(Debug, Clone, Default)]
struct Draft {
    name: String,
    executable: String,
    arguments: Vec<String>,
    argument_input: String,
    unparsed_arguments: Option<String>,
    working_directory: String,
    runner: String,
    /// Set once the person picks, types, copies, or downloads a runner, so
    /// a refreshed suggestion leaves their choice alone.
    runner_chosen: bool,
    adopt: bool,
    desktop: bool,
    batch: bool,
    /// The Heroic ids a batch imports.
    games: BTreeSet<String>,
    fetch_batch_art: bool,
    environment: Vec<String>,
    environment_input: String,
    environment_dirty: bool,
    gamemode: bool,
    gamemode_dirty: bool,
    wayland: bool,
    wayland_dirty: bool,
    mangohud: bool,
    mangohud_dirty: bool,
    gamescope: ScopeDraft,
    hdr: bool,
    hdr_dirty: bool,
}

#[derive(Debug, Clone, Default)]
struct ScopeDraft {
    setting: Option<GamescopeSetting>,
    last_on: Gamescope,
    numbers: [String; 6],
    args: Vec<String>,
    args_input: String,
    dirty: bool,
}

impl ScopeDraft {
    fn from_setting(setting: Option<GamescopeSetting>) -> Self {
        let scope = setting.as_ref().and_then(GamescopeSetting::on);
        let number = |value: Option<u32>| value.map(|value| value.to_string()).unwrap_or_default();
        Self {
            numbers: scope
                .map(|scope| {
                    [
                        number(scope.width),
                        number(scope.height),
                        number(scope.output_width),
                        number(scope.output_height),
                        number(scope.fps_limit),
                        number(scope.fps_limit_unfocused),
                    ]
                })
                .unwrap_or_default(),
            args: scope.map(|scope| scope.args.clone()).unwrap_or_default(),
            last_on: scope.cloned().unwrap_or_default(),
            setting,
            ..Self::default()
        }
    }

    fn scope(&self) -> Gamescope {
        match &self.setting {
            Some(GamescopeSetting::On(scope)) => scope.clone(),
            _ => self.last_on.clone(),
        }
    }

    fn resolved(&self) -> Result<Option<GamescopeSetting>, String> {
        if !self.dirty {
            return Ok(self.setting.clone());
        }
        if !matches!(self.setting, Some(GamescopeSetting::On(_))) {
            return Ok(self.setting.clone());
        }
        let mut scope = self.scope();
        let mut numbers = [None; 6];
        for (index, raw) in self.numbers.iter().enumerate() {
            numbers[index] = if raw.trim().is_empty() {
                None
            } else {
                Some(
                    raw.parse::<u32>()
                        .map_err(|_| format!("Invalid gamescope {}", SCOPE_NUMBER_LABELS[index]))?,
                )
            };
        }
        [
            scope.width,
            scope.height,
            scope.output_width,
            scope.output_height,
            scope.fps_limit,
            scope.fps_limit_unfocused,
        ] = numbers;
        scope.args = self.args.clone();
        Ok(Some(GamescopeSetting::On(scope)))
    }
}

const SCOPE_NUMBER_LABELS: [&str; 6] = [
    "Game width",
    "Game height",
    "Output width",
    "Output height",
    "FPS limit",
    "Unfocused FPS limit",
];

#[derive(Debug, Clone)]
pub(crate) struct ImportFailure {
    message: String,
    source_changed: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Discard,
    KeepEditing,
    Source(usize),
    SourcePath(String),
    OpenSourcePath,
    OpenExisting(String),
    BrowseSource,
    PickedSource(u64, Result<Option<PathBuf>, String>),
    SourcesLoaded(u64, Result<Vec<HeroicSource>, String>),
    Listed(
        u64,
        Result<(HeroicListing, HashMap<String, String>), String>,
    ),
    Toggle(usize),
    SelectAll,
    SelectNone,
    ImportSelected,
    Poster(u64, String, Result<(image::Handle, Format), String>),
    /// The delay before the art progress bar shows has passed.
    ProgressDue(u64),
    Prepared(u64, Result<Box<Prepared>, String>),
    BatchPrepared(u64, Result<BatchOptions, String>),
    Name(String),
    Executable(String),
    BrowseExecutable,
    PickedExecutable(u64, Result<Option<PathBuf>, String>),
    ArgumentInput(String),
    ArgumentAdd,
    ArgumentRemove(usize),
    ArgumentsClear,
    EnvironmentInput(String),
    EnvironmentAdd,
    EnvironmentRemove(usize),
    EnvironmentClear,
    Gamemode(bool),
    Wayland(bool),
    Mangohud(bool),
    Hdr(bool),
    GamescopeEnabled(bool),
    GamescopeNumber(usize, String),
    GamescopeChoice(usize, String),
    GamescopeGrab(bool),
    GamescopeArgInput(String),
    GamescopeArgAdd,
    GamescopeArgRemove(usize),
    WorkingDirectory(String),
    BrowseWorkingDirectory,
    PickedWorkingDirectory(u64, Result<Option<PathBuf>, String>),
    Runner(String),
    /// The answer to using other launchers' builds.
    Offer(bool),
    /// The reply's sequence, the details session it was asked for, and
    /// the runner choices read after saving the answer.
    Offered(u64, u64, Result<Refreshed, String>),
    Adopt(bool),
    Desktop(bool),
    FetchBatchArt(bool),
    Plan,
    Planned(u64, Result<Box<Review>, String>),
    CopyRunner,
    DownloadRunner,
    DownloadBuild(Component, Option<String>),
    PrerequisiteFinished(u64, Result<String, String>),
    Import,
    Cancel,
    Force,
    Progress(u64, String),
    GameResult(u64, String, Result<Vec<String>, String>),
    SingleRegistered(u64, Result<Arc<Registered>, ImportFailure>),
    BatchFinished(u64, Result<(bool, bool), String>),
    RetryArt,
    SkipArt,
    ArtFinished(u64, Result<Vec<String>, String>),
    FinishedRegistered(u64, Result<Vec<String>, String>),
    Art(u64, artwork::Event),
    OpenSettings,
    BackToLibrary,
}

pub(crate) struct Page {
    paths: Paths,
    step: Step,
    generation: u64,
    picker_sequence: u64,
    sources: Vec<HeroicSource>,
    source_path: String,
    source: Option<usize>,
    listing: Option<HeroicListing>,
    already: HashMap<String, String>,
    /// Picks Heroic ids.
    grid: ImportGrid<Event>,
    /// Decoded posters by Heroic id, with their format. None when the file
    /// couldn't be decoded, so it's never stored as art either.
    posters: HashMap<String, Option<(image::Handle, Format)>>,
    selected: Option<usize>,
    prepared: Option<Prepared>,
    batch_options: Option<BatchOptions>,
    draft: Draft,
    review: Option<Review>,
    art: artwork::Draft,
    art_generation: u64,
    registered: Option<Arc<Registered>>,
    token: Option<Token>,
    status: String,
    results: Vec<(String, Result<Vec<String>, String>)>,
    error: Option<String>,
    notes: Vec<String>,
    leave_prompt: bool,
    action: Option<Action>,
    /// Set while an answer to the offer is being saved.
    answering: bool,
    offer_sequence: u64,
    /// Bumped each time details open for a game or a batch, so choices read
    /// for one never land on another.
    details_session: u64,
}

impl Page {
    pub(crate) fn new(paths: Paths) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            step: Step::Sources,
            generation: 0,
            picker_sequence: 0,
            sources: Vec::new(),
            source_path: String::new(),
            source: None,
            listing: None,
            already: HashMap::new(),
            grid: ImportGrid::new(GRID_ID, Event::ProgressDue, 0),
            posters: HashMap::new(),
            selected: None,
            prepared: None,
            batch_options: None,
            draft: Draft::default(),
            review: None,
            art: artwork::Draft::new(),
            art_generation: 0,
            registered: None,
            token: None,
            status: "Finding Heroic configuration...".to_owned(),
            results: Vec::new(),
            error: None,
            notes: Vec::new(),
            leave_prompt: false,
            action: None,
            answering: false,
            offer_sequence: 0,
            details_session: 0,
        };
        let task = page.discover();
        (page, task)
    }

    pub(crate) fn busy(&self) -> bool {
        self.token.is_some()
    }

    /// Background work is running: an import, or a listing, inspection, or
    /// plan whose status stays set until its reply clears it.
    pub(crate) fn waiting(&self) -> bool {
        self.busy()
            || (!self.status.is_empty()
                && matches!(
                    self.step,
                    Step::Sources | Step::Games | Step::Details | Step::Review
                ))
    }

    /// The status sits by the button that started the work, where the
    /// person is looking, rather than at the top of the page.
    fn status_by_action(&self) -> bool {
        self.waiting() && matches!(self.step, Step::Games | Step::Details | Step::Review)
    }

    fn working<'a>(&'a self, palette: Palette) -> Element<'a, Event> {
        row![
            crate::spinner::spinner(palette, 16.0),
            text(&self.status).color(palette.muted)
        ]
        .spacing(8)
        .align_y(Center)
        .into()
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        if let Some(token) = &self.token {
            if force {
                token.force()
            } else {
                token.cancel()
            }
            self.status = if force {
                "Force stop requested..."
            } else {
                "Canceling..."
            }
            .to_owned();
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        if self.busy() {
            return Task::none();
        }
        match self.step {
            Step::Sources if !self.source_path.trim().is_empty() => {
                self.update(Event::OpenSourcePath)
            }
            Step::Sources if self.sources.len() == 1 => self.update(Event::Source(0)),
            Step::Games => self.import_chosen(),
            Step::Details => self.plan(),
            Step::Review => self.start_import(),
            Step::Finished => self.update(Event::BackToLibrary),
            _ => Task::none(),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        if self.busy() {
            return Task::none();
        }
        self.error = None;
        self.next_picker();
        if matches!(self.step, Step::Games | Step::Details | Step::Review) {
            // A reply to what this step started must not reopen it.
            self.next_generation();
            self.status.clear();
        }
        match self.step {
            Step::Sources => {
                if self.source.is_some() {
                    self.leave_prompt = true
                } else {
                    self.action = Some(Action::Choose)
                }
            }
            Step::Games => self.step = Step::Sources,
            Step::Details => {
                self.review = None;
                self.step = Step::Games;
            }
            Step::Review => self.step = Step::Details,
            Step::Working => {}
            Step::Finished => self.exit(),
        }
        Task::none()
    }

    /// Saves the answer, if there is one, then reads the runner choices
    /// again for the open game or batch.
    fn relist(&mut self, answer: Option<bool>) -> Task<Event> {
        self.offer_sequence = self.offer_sequence.wrapping_add(1);
        let sequence = self.offer_sequence;
        let session = self.details_session;
        let paths = self.paths.clone();
        let single = (!self.draft.batch)
            .then(|| {
                let listing = self.listing.as_ref()?;
                let candidate = self.prepared.as_ref()?.candidate.clone();
                Some((listing.source.clone(), candidate))
            })
            .flatten();
        task::blocking(
            move || {
                if let Some(on) = answer {
                    runner_label::answer(&paths, on)?;
                }
                refresh(&paths, single)
            },
            move |result| Event::Offered(sequence, session, result),
        )
    }

    /// Whether the open details pick a runner: a Windows game, or a batch
    /// with one. Reading a native game's choices would inspect Wine
    /// settings it doesn't have.
    fn uses_runner(&self) -> bool {
        if self.draft.batch {
            self.listing.as_ref().is_some_and(|listing| {
                listing.candidates.iter().any(|candidate| {
                    self.draft.games.contains(&candidate.id)
                        && candidate.kind == HeroicGameKind::Windows
                })
            })
        } else {
            self.prepared
                .as_ref()
                .is_some_and(|prepared| prepared.candidate.kind == HeroicGameKind::Windows)
        }
    }

    /// The setting is saved, so the open details stop asking.
    fn offer_answered(&mut self) {
        if let Some(prepared) = &mut self.prepared {
            prepared.offer = false;
        }
        if let Some(options) = &mut self.batch_options {
            options.offer = false;
        }
    }

    /// The question shows on the runner step until it's answered, for a
    /// batch or a Windows game.
    fn offer_shown(&self) -> bool {
        self.step == Step::Details
            && if self.draft.batch {
                self.batch_options
                    .as_ref()
                    .is_some_and(|options| options.offer)
            } else {
                self.prepared.as_ref().is_some_and(|prepared| {
                    prepared.offer && prepared.candidate.kind == HeroicGameKind::Windows
                })
            }
    }

    /// Takes the new runner choices, and the new suggestion too while the
    /// person hasn't chosen a runner and is still on the runner step. An
    /// open review shows what Import does and stays.
    fn refreshed(&mut self, refreshed: Refreshed) -> Task<Event> {
        let suggested = match refreshed {
            Refreshed::Single {
                runners,
                suggested,
                runner,
            } => {
                let Some(prepared) = &mut self.prepared else {
                    return Task::none();
                };
                *prepared = Prepared {
                    runners,
                    suggested,
                    runner: runner.clone(),
                    offer: false,
                    ..prepared.clone()
                };
                runner
            }
            Refreshed::Batch(options) => {
                let runner = options.runner.clone();
                self.batch_options = Some(options);
                runner
            }
        };
        if self.step != Step::Details {
            return Task::none();
        }
        // A plan started before the answer was saved may have read the old
        // setting.
        let planning = !self.status.is_empty() && !self.busy();
        if !self.draft.runner_chosen && self.draft.runner != suggested {
            self.draft.runner = suggested;
            self.review = None;
        }
        if planning {
            return self.plan();
        }
        Task::none()
    }

    fn exit(&mut self) {
        self.action = Some(Action::Library {
            name: None,
            launch: false,
        });
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    fn next_picker(&mut self) -> u64 {
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
        self.picker_sequence
    }

    fn discover(&mut self) -> Task<Event> {
        let generation = self.next_generation();
        let paths = self.paths.clone();
        task::blocking(
            move || heroic_import::sources(&paths, None).map_err(|e| e.to_string()),
            move |result| Event::SourcesLoaded(generation, result),
        )
    }

    fn list(&mut self, source: HeroicSource) -> Task<Event> {
        self.next_picker();
        // Choosing a source answers an open discard prompt with keep editing.
        self.leave_prompt = false;
        self.step = Step::Games;
        self.status = "Reading Heroic games...".to_owned();
        self.error = None;
        self.listing = None;
        self.already.clear();
        self.grid.clear();
        self.posters.clear();
        self.prepared = None;
        self.batch_options = None;
        self.review = None;
        let generation = self.next_generation();
        let paths = self.paths.clone();
        task::blocking(
            move || {
                let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                let listing =
                    heroic_import::listing(&paths, &global, &source).map_err(|e| e.to_string())?;
                let mut already = HashMap::new();
                let mut existing = Vec::new();
                for name in environment::names(&paths).map_err(|e| e.to_string())? {
                    if let Ok((environment, _)) = Environment::load_for_heroic_match(&paths, &name)
                    {
                        existing.push(environment);
                    }
                }
                for candidate in &listing.candidates {
                    let default = heroic_import::default_name(
                        &candidate.title,
                        candidate.executable.as_deref(),
                    );
                    if let Some(environment) = existing.iter().find(|environment| {
                        environment.config.heroic_id.as_deref() == Some(&candidate.id)
                            || (environment.config.heroic_id.is_none()
                                && environment.name == default)
                    }) {
                        already.insert(candidate.id.clone(), environment.name.clone());
                    }
                }
                Ok((listing, already))
            },
            move |result| Event::Listed(generation, result),
        )
    }

    fn prepare(&mut self, index: usize) -> Task<Event> {
        self.next_picker();
        let Some(listing) = self.listing.as_ref() else {
            return Task::none();
        };
        let Some(candidate) = listing.candidates.get(index).cloned() else {
            return Task::none();
        };
        if let Some(issue) = heroic_import::blocker(&candidate) {
            self.error = Some(issue.message.clone());
            return Task::none();
        }
        self.selected = Some(index);
        self.details_session = self.details_session.wrapping_add(1);
        self.status = format!("Inspecting {}...", candidate.title);
        self.error = None;
        let paths = self.paths.clone();
        let source = listing.source.clone();
        let generation = self.next_generation();
        task::blocking(
            move || prepare_game(&paths, &source, candidate).map(Box::new),
            move |result| Event::Prepared(generation, result),
        )
    }

    fn prepare_batch(&mut self) -> Task<Event> {
        self.next_picker();
        self.selected = None;
        self.draft = Draft {
            batch: true,
            games: self.grid.chosen.clone(),
            ..Draft::default()
        };
        self.prepared = None;
        self.details_session = self.details_session.wrapping_add(1);
        self.step = Step::Details;
        self.status = "Checking installed runners...".to_owned();
        let paths = self.paths.clone();
        let generation = self.next_generation();
        task::blocking(
            move || batch_options(&paths),
            move |result| Event::BatchPrepared(generation, result),
        )
    }

    /// Decodes each cached poster on a worker. A game without one keeps its
    /// title placeholder.
    fn load_posters(&mut self, listing: &HeroicListing) -> Task<Event> {
        let jobs = listing
            .candidates
            .iter()
            .filter_map(|candidate| Some((candidate.id.clone(), candidate.poster.clone()?)))
            .collect();
        self.grid.decode_posters(
            self.paths.art_cache_dir(),
            jobs,
            |_, path, cache| posters::decode(path, cache),
            Event::Poster,
        )
    }

    fn poster(&self, id: &str) -> Option<&image::Handle> {
        self.posters
            .get(id)
            .and_then(Option::as_ref)
            .map(|(handle, _)| handle)
    }

    /// Offers the single game's poster as its Grid art once the poster has
    /// decoded. The art draft ignores it if the user already chose or removed
    /// a Grid file.
    fn prefill_poster(&mut self) {
        if self.draft.batch || !matches!(self.step, Step::Details | Step::Review) {
            return;
        }
        let Some(candidate) = self.prepared.as_ref().map(|prepared| &prepared.candidate) else {
            return;
        };
        if let Some(Some((handle, format))) = self.posters.get(&candidate.id)
            && let Some(path) = &candidate.poster
        {
            self.art.prefill(
                Kind::Grid,
                path,
                heroic_import::poster_name(&candidate.id, *format),
                "From Heroic",
                handle.clone(),
            );
        }
    }

    /// The listed posters a batch may store. One that failed to decode in the
    /// grid is left out, since a file can look like an image and still be
    /// unusable; one still decoding is checked again when it's stored.
    fn batch_posters(&self) -> HashMap<String, BatchPoster> {
        self.listing
            .iter()
            .flat_map(|listing| &listing.candidates)
            .filter_map(|candidate| {
                let decoded = match self.posters.get(&candidate.id) {
                    Some(Some(_)) => true,
                    Some(None) => return None,
                    None => false,
                };
                Some((
                    candidate.id.clone(),
                    BatchPoster {
                        path: candidate.poster.clone()?,
                        decoded,
                    },
                ))
            })
            .collect()
    }

    /// Why the game at `index` can't be picked, or None when it can.
    fn ineligible(&self, index: usize) -> Option<String> {
        let candidate = self.listing.as_ref()?.candidates.get(index)?;
        if let Some(name) = self.already.get(&candidate.id) {
            return Some(format!("Already imported as {name}"));
        }
        heroic_import::blocker(candidate).map(|issue| issue.message.clone())
    }

    /// The ids of every game that can be picked.
    fn eligible(&self) -> impl Iterator<Item = String> + '_ {
        self.listing
            .iter()
            .flat_map(|listing| listing.candidates.iter().enumerate())
            .filter(|(index, _)| self.ineligible(*index).is_none())
            .map(|(_, candidate)| candidate.id.clone())
    }

    fn toggle(&mut self, index: usize) {
        if self.ineligible(index).is_some() {
            return;
        }
        let Some(candidate) = self
            .listing
            .as_ref()
            .and_then(|listing| listing.candidates.get(index))
        else {
            return;
        };
        self.grid.toggle(&candidate.id);
        self.selection_changed();
    }

    /// Drops a pending inspection of the old selection, so its reply can't
    /// open details for a game no longer chosen.
    fn selection_changed(&mut self) {
        self.next_generation();
        self.status.clear();
    }

    /// One game opens the editable single review; several share the batch
    /// options.
    fn import_chosen(&mut self) -> Task<Event> {
        if self.busy() || self.step != Step::Games {
            return Task::none();
        }
        let Some(listing) = &self.listing else {
            return Task::none();
        };
        let chosen: Vec<usize> = listing
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| self.grid.chosen.contains(&candidate.id))
            .map(|(index, _)| index)
            .collect();
        match chosen[..] {
            [] => Task::none(),
            [index] => self.prepare(index),
            _ => self.prepare_batch(),
        }
    }

    /// Grid keys: arrows move, Space toggles, Ctrl+A selects all, and Enter
    /// imports. None leaves the key to the flow's shared handling.
    pub(crate) fn key(
        &mut self,
        key: &Key,
        modifiers: keyboard::Modifiers,
        width: f32,
    ) -> Option<Task<Event>> {
        if self.step != Step::Games || self.busy() {
            return None;
        }
        let count = self.listing.as_ref()?.candidates.len();
        Some(match self.grid.key(key, modifiers, width, count)? {
            GridKey::Moved(task) => task,
            GridKey::Toggle(focus) => {
                if let Some(index) = focus {
                    self.toggle(index);
                    self.grid.focus_visible = true;
                }
                Task::none()
            }
            GridKey::SelectAll => self.update(Event::SelectAll),
            GridKey::Import => self.import_chosen(),
        })
    }

    /// The body scrollable keeps its offset across steps, so without the snap
    /// Review opens wherever Details was scrolled to.
    fn enter_review(&mut self) -> Task<Event> {
        self.step = Step::Review;
        iced::widget::operation::snap_to(
            iced::widget::Id::new(BODY_ID),
            RelativeOffset {
                x: None,
                y: Some(0.0),
            },
        )
    }

    fn plan(&mut self) -> Task<Event> {
        if !self.draft.batch && !self.draft.argument_input.trim().is_empty() {
            self.error = Some("Add the typed argument before reviewing".to_owned());
            return Task::none();
        }
        if !self.draft.batch && !self.draft.environment_input.trim().is_empty() {
            self.error = Some("Add the typed environment variable before reviewing".to_owned());
            return Task::none();
        }
        if !self.draft.batch && !self.draft.gamescope.args_input.trim().is_empty() {
            self.error = Some("Add the typed gamescope argument before reviewing".to_owned());
            return Task::none();
        }
        if !self.draft.batch && self.draft.unparsed_arguments.is_some() {
            self.error = Some("Heroic launcher arguments could not be parsed. Confirm corrected argument chips or choose No arguments.".to_owned());
            return Task::none();
        }
        if !self.draft.batch
            && let Err(error) = self.art.validate()
        {
            self.error = Some(error);
            return Task::none();
        }
        let Some(listing) = self.listing.clone() else {
            return Task::none();
        };
        let draft = self.draft.clone();
        self.error = None;
        self.status = "Checking import choices...".to_owned();
        let paths = self.paths.clone();
        let selected = self.selected;
        let generation = self.next_generation();
        task::blocking(
            move || plan_review(&paths, &listing, selected, &draft).map(Box::new),
            move |result| Event::Planned(generation, result),
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
                install_prerequisite(&paths, operation, &token, &|line| {
                    let _ = sender.unbounded_send(Event::Progress(generation, line));
                })
            },
            move |result| Event::PrerequisiteFinished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn start_import(&mut self) -> Task<Event> {
        let Some(review) = self.review.clone() else {
            return Task::none();
        };
        if self.token.is_some() {
            return Task::none();
        }
        let Some(listing) = self.listing.as_ref() else {
            return Task::none();
        };
        let source = listing.source.clone();
        let posters = self.batch_posters();
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Working;
        self.error = None;
        self.results.clear();
        self.status = match &review {
            Review::Single { game, .. } => import_status(game.title(), None, None),
            Review::Batch(_) => "Importing...".to_owned(),
        };
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let fetch_batch_art = self.draft.fetch_batch_art;
        let batch_desktop = self.draft.desktop;
        match review {
            Review::Single { game, .. } => {
                let (sender, receiver) = unbounded();
                let work = Task::perform(
                    task::join(
                        move || {
                            let global =
                                GlobalConfig::load(&paths).map_err(|error| ImportFailure {
                                    message: error.to_string(),
                                    source_changed: false,
                                })?;
                            let title = game.title().to_owned();
                            let mut copy = CopyProgress::default();
                            apply_game(&paths, &global, &source, &game, &token, |progress| {
                                if let Some(phase) = copy.record(&progress, Instant::now()) {
                                    let _ = sender.unbounded_send(Event::Progress(
                                        generation,
                                        import_status(&title, None, Some(&phase)),
                                    ));
                                }
                            })
                            .map(Arc::new)
                            .map_err(|error| ImportFailure {
                                source_changed: error.is::<heroic_import::ApplyError>(),
                                message: format!("{error:#}"),
                            })
                        },
                        |error| ImportFailure {
                            message: error.to_string(),
                            source_changed: false,
                        },
                    ),
                    move |result| Event::SingleRegistered(generation, result),
                );
                Task::batch([Task::run(receiver, |event| event), work])
            }
            Review::Batch(library) => {
                let (sender, receiver) = unbounded();
                let work = task::blocking(
                    move || {
                        let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                        if let Err(error) = library.record_backfills(&paths) {
                            let _ = sender.unbounded_send(Event::Progress(
                                generation,
                                format!("Heroic ID backfill: {error}"),
                            ));
                        }
                        let mut source_changed = false;
                        // One client for every game's art, not one per game.
                        let https = Lazy::new();
                        let ready = library.into_ready();
                        let count = ready.len();
                        for (index, mut game) in ready.into_iter().enumerate() {
                            if token.is_canceled() {
                                break;
                            }
                            match &mut game {
                                Game::Windows(plan) => plan.set_desktop_enabled(batch_desktop),
                                Game::Native(plan) => plan.set_desktop_enabled(batch_desktop),
                            }
                            let name = game.name().to_owned();
                            let title = game.title().to_owned();
                            let position = Some((index + 1, count));
                            let status = |phase: Option<&str>| {
                                Event::Progress(generation, import_status(&title, position, phase))
                            };
                            let _ = sender.unbounded_send(status(None));
                            let mut copy = CopyProgress::default();
                            let result = (|| {
                                let registered = apply_game(
                                    &paths,
                                    &global,
                                    &source,
                                    &game,
                                    &token,
                                    |progress| {
                                        if let Some(phase) = copy.record(&progress, Instant::now())
                                        {
                                            let _ = sender.unbounded_send(status(Some(&phase)));
                                        }
                                    },
                                )?;
                                let mut notes = Vec::new();
                                if let Some(note) = playtime_note(&registered.playtime) {
                                    notes.push(note);
                                }
                                let _ = sender.unbounded_send(status(Some("adding artwork")));
                                let app = heroic_id(&game);
                                notes.extend(store_heroic_poster(
                                    &paths,
                                    &name,
                                    app,
                                    posters.get(app),
                                ));
                                if fetch_batch_art {
                                    notes.extend(
                                        artwork::fetch_default(&paths, &name, &title, &https)
                                            .unwrap_or_else(|error| {
                                                vec![format!("Artwork: {error}")]
                                            }),
                                    );
                                }
                                let _ = sender
                                    .unbounded_send(status(Some("finalizing desktop entries")));
                                if let Err(error) = registered.finish(&paths, &global) {
                                    notes.push(format!("Desktop entry: {error}"));
                                }
                                Ok::<_, anyhow::Error>(notes)
                            })()
                            .map_err(|error| {
                                if error.is::<heroic_import::ApplyError>() {
                                    source_changed = true;
                                }
                                format!("{error:#}")
                            });
                            let _ =
                                sender.unbounded_send(Event::GameResult(generation, name, result));
                            if source_changed {
                                break;
                            }
                        }
                        Ok::<_, String>((source_changed, token.is_canceled()))
                    },
                    move |result| Event::BatchFinished(generation, result),
                );
                Task::batch([Task::run(receiver, |event| event), work])
            }
        }
    }

    fn finish_art(&mut self) -> Task<Event> {
        let Some(registered) = self.registered.clone() else {
            return Task::none();
        };
        let paths = self.paths.clone();
        let generation = self.next_generation();
        self.status = "Finalizing desktop entries...".to_owned();
        task::blocking(
            move || {
                let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                registered
                    .finish(&paths, &global)
                    .map(|changes| {
                        changes
                            .into_iter()
                            .map(|change| crate::settings::desktop_line(&change))
                            .collect()
                    })
                    .map_err(|e| e.to_string())
            },
            move |result| Event::FinishedRegistered(generation, result),
        )
    }

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

    fn grid<'a>(
        &'a self,
        listing: &'a HeroicListing,
        palette: Palette,
        width: f32,
        header: Element<'a, Event>,
    ) -> Element<'a, Event> {
        let tiles = listing
            .candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                (
                    Tile {
                        title: &candidate.title,
                        poster: self.poster(&candidate.id),
                        reason: self.ineligible(index),
                        note: None,
                        chosen: self.grid.chosen.contains(&candidate.id),
                        focus_ring: self.grid.focus_ring(index),
                        dimmed: false,
                    },
                    Event::Toggle(index),
                )
            });
        self.grid.view(header, tiles, width, palette)
    }

    pub(crate) fn view_with_key_availability(
        &self,
        palette: Palette,
        width: f32,
        key_available: bool,
    ) -> Element<'_, Event> {
        let mut body = column![].spacing(14).width(Fill);
        // Pinned under the scrolling body so it stays reachable in a short dialog.
        let mut actions: Option<Element<'_, Event>> = None;
        let title = match self.step {
            Step::Sources => "Import from Heroic".to_owned(),
            Step::Games => "Choose Heroic games".to_owned(),
            Step::Details if self.draft.batch => import_label(self.draft.games.len()),
            Step::Details => "Import details".to_owned(),
            Step::Review => "Review Heroic import".to_owned(),
            Step::Working => "Importing from Heroic".to_owned(),
            Step::Finished => "Heroic import finished".to_owned(),
        };
        body = body.push(text(title).size(26).color(palette.foreground));
        if self.waiting() && !self.status_by_action() {
            body = body.push(self.working(palette));
        } else if !self.status.is_empty() && !self.status_by_action() {
            body = body.push(text(&self.status).color(palette.muted));
        }
        for note in &self.notes {
            body = body.push(text(note).size(13).color(palette.muted));
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).size(14).color(palette.danger));
        }
        match self.step {
            Step::Sources => {
                if self.sources.is_empty() {
                    body = body.push(text("No Heroic configuration was found automatically. Choose its config folder.").color(palette.muted));
                }
                if !self.sources.is_empty() {
                    body = body.push(list_group(
                        self.sources.iter().enumerate().map(|(index, source)| {
                            list_row(
                                list_row_text(format!(
                                    "{}: {}",
                                    origin_label(source.origin()),
                                    source.config_dir().display()
                                )),
                                palette,
                            )
                            .on_press(Event::Source(index))
                        }),
                        palette,
                    ));
                }
                body = body.push(
                    secondary_button("Browse for Heroic config folder...", palette)
                        .on_press(Event::BrowseSource),
                );
                body = body.push(field(
                    "Config folder path",
                    &self.source_path,
                    Event::SourcePath,
                ));
                body = body
                    .push(secondary_button("Open folder", palette).on_press(Event::OpenSourcePath));
            }
            Step::Games => {
                if let Some(listing) = &self.listing {
                    for issue in &listing.issues {
                        body = body.push(text(&issue.message).size(12).color(palette.warning));
                    }
                    let count = self.grid.chosen.len();
                    let idle = !self.busy();
                    actions = Some(self.grid.footer(
                        idle.then_some(Event::SelectAll),
                        (idle && count > 0).then_some(Event::SelectNone),
                        self.status_by_action().then(|| self.working(palette)),
                        (!self.waiting() && count > 0).then_some(Event::ImportSelected),
                        palette,
                    ));
                }
            }
            Step::Details => {
                if self.offer_shown() {
                    body = body.push(runner_label::offer_card(
                        palette,
                        !self.answering,
                        Event::Offer,
                    ));
                }
                if self.draft.batch {
                    body = body.push(
                        text("Windows games use the selected runner. Native games need no runner.")
                            .color(palette.muted),
                    );
                    body = body.push(field("Windows runner", &self.draft.runner, Event::Runner));
                    if let Some(options) = &self.batch_options {
                        body = body.push(
                            pick_list(
                                options.runners.clone(),
                                Some(Choice::find(&options.runners, &self.draft.runner)),
                                |choice| Event::Runner(choice.name),
                            )
                            .wineo_style(palette),
                        );
                    }
                    if self.draft.runner.is_empty() {
                        body = body.push(
                            secondary_button("Download latest GE-Proton", palette)
                                .on_press(Event::DownloadRunner),
                        );
                    }
                    body = body.push(
                        checkbox(self.draft.adopt)
                            .label("Use Heroic prefixes in place (otherwise copy)")
                            .on_toggle(Event::Adopt),
                    );
                    body = body.push(
                        checkbox(self.draft.desktop)
                            .label("Create desktop entries")
                            .on_toggle(Event::Desktop),
                    );
                    if let Some(options) = &self.batch_options {
                        if options.art_fetch_enabled && key_available {
                            body = body.push(
                                checkbox(self.draft.fetch_batch_art)
                                    .label("Fetch art using the top SteamGridDB match")
                                    .on_toggle(Event::FetchBatchArt),
                            );
                        } else if !options.art_fetch_enabled {
                            body = body.push(
                                text("Automatic artwork is disabled in Global Settings")
                                    .color(palette.muted),
                            );
                        } else {
                            body = body.push(
                                text("SteamGridDB needs a configured key. Manual artwork is available in single-game import.")
                                    .color(palette.muted),
                            );
                        }
                    }
                } else {
                    body = body.push(field("Environment name", &self.draft.name, Event::Name));
                    if self.paths.environment_config(&self.draft.name).exists() {
                        body = body.push(
                            text("An environment with this name already exists.")
                                .color(palette.warning),
                        );
                        body = body.push(
                            secondary_button("Open its settings", palette)
                                .on_press(Event::OpenExisting(self.draft.name.clone())),
                        );
                    }
                    body = body.push(field(
                        "Executable",
                        &self.draft.executable,
                        Event::Executable,
                    ));
                    body = body.push(
                        secondary_button("Browse executable...", palette)
                            .on_press(Event::BrowseExecutable),
                    );
                    body = body.push(text("Arguments").size(13));
                    if let Some(raw) = &self.draft.unparsed_arguments {
                        body = body.push(
                            text(format!("Heroic arguments need correction: {raw}"))
                                .color(palette.warning),
                        );
                    }
                    body = body.push(
                        Chips {
                            items: &self.draft.arguments,
                            input: &self.draft.argument_input,
                            placeholder: "One argument",
                            error: None,
                            on_input: Box::new(Event::ArgumentInput),
                            on_add: Event::ArgumentAdd,
                            on_remove: Box::new(Event::ArgumentRemove),
                            enabled: true,
                        }
                        .view(palette),
                    );
                    body = body.push(
                        secondary_button("No arguments", palette).on_press(Event::ArgumentsClear),
                    );
                    body = body.push(field(
                        "Working directory",
                        &self.draft.working_directory,
                        Event::WorkingDirectory,
                    ));
                    body = body.push(
                        secondary_button("Browse working directory...", palette)
                            .on_press(Event::BrowseWorkingDirectory),
                    );
                    if let Some(prepared) = &self.prepared {
                        for (label, path) in &prepared.hook_paths {
                            body = body.push(text(format!("{label}: {path}")));
                        }
                    }
                    let windows = self
                        .prepared
                        .as_ref()
                        .is_some_and(|prepared| prepared.candidate.kind == HeroicGameKind::Windows);
                    if windows {
                        body = body.push(field("Runner", &self.draft.runner, Event::Runner));
                        if let Some(prepared) = &self.prepared {
                            body = body.push(
                                pick_list(
                                    prepared.runners.clone(),
                                    Some(Choice::find(&prepared.runners, &self.draft.runner)),
                                    |choice| Event::Runner(choice.name),
                                )
                                .wineo_style(palette),
                            );
                            if let Some(RunnerSuggestion::Copy(copy)) = &prepared.suggested {
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
                                        .on_press(Event::CopyRunner),
                                );
                            } else if let Some(RunnerSuggestion::Choose {
                                proton,
                                copy_blocked,
                            }) = &prepared.suggested
                            {
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
                        }
                        if self.draft.runner.is_empty() {
                            body = body.push(
                                secondary_button("Download latest GE-Proton", palette)
                                    .on_press(Event::DownloadRunner),
                            );
                        }
                        body = body.push(
                            checkbox(self.draft.adopt)
                                .label("Use Heroic prefix in place (otherwise copy)")
                                .on_toggle(Event::Adopt),
                        );
                    }
                    body = body.push(
                        checkbox(self.draft.desktop)
                            .label("Create desktop entry")
                            .on_toggle(Event::Desktop),
                    );
                    body = body.push(launch_settings(&self.draft, windows, palette));
                    let art_generation = self.art_generation;
                    body = body.push(
                        self.art
                            .view(palette, key_available)
                            .map(move |event| Event::Art(art_generation, event)),
                    );
                }
                let mut next = row![
                    accent_base("Next: Review", palette.primary)
                        .on_press_maybe((!self.waiting()).then_some(Event::Plan))
                ]
                .spacing(12)
                .align_y(Center);
                if self.status_by_action() {
                    next = next.push(self.working(palette));
                }
                actions = Some(next.wrap().vertical_spacing(8).into());
            }
            Step::Review => {
                if let Some(review) = &self.review {
                    match review {
                        Review::Single {
                            game,
                            missing_build,
                            runner_line,
                        } => {
                            body = body.push(text(format!("{} -> {}", game.title(), game.name())));
                            match game.as_ref() {
                                Game::Windows(plan) => {
                                    body = body.push(runner_line.as_deref().map(text));
                                    body = body.push(text(format!(
                                        "Executable: {}",
                                        plan.resolved().exe.display()
                                    )));
                                    body = body.push(text(format!(
                                        "Arguments: {}",
                                        plan.resolved().args.join(" ")
                                    )));
                                    if let Some(cwd) = &plan.resolved().cwd {
                                        body = body.push(text(format!(
                                            "Working directory: {}",
                                            cwd.display()
                                        )));
                                    }
                                    body = body.push(text(format!(
                                        "Desktop entry: {}",
                                        if plan.desktop().enabled { "yes" } else { "no" }
                                    )));
                                    body = body.push(text(format!(
                                        "Prefix: {} -> {}",
                                        plan.inspection().source_root.display(),
                                        plan.destination().display()
                                    )));
                                    body = body.push(text(if plan.is_adopt() {
                                        "Use in place"
                                    } else {
                                        "Copy prefix"
                                    }));
                                    // Only what stops the import; the rest reads like
                                    // debug output here.
                                    for finding in plan.findings().iter().filter(|finding| {
                                        matches!(
                                            finding.resolution,
                                            Resolution::Blocking | Resolution::NeedsInput(_)
                                        )
                                    }) {
                                        body = body.push(
                                            text(&finding.issue.message)
                                                .size(13)
                                                .color(palette.warning),
                                        );
                                    }
                                    if let Some(note) =
                                        shared_prefix_note(plan.is_adopt(), plan.shared_prefix())
                                    {
                                        body =
                                            body.push(text(note).size(13).color(palette.warning));
                                    }
                                }
                                Game::Native(plan) => {
                                    body = body.push(text(format!(
                                        "Native executable: {}",
                                        plan.resolved().exe.display()
                                    )));
                                    body = body.push(text(format!(
                                        "Arguments: {}",
                                        plan.resolved().args.join(" ")
                                    )));
                                    if let Some(cwd) = plan.working_directory() {
                                        body = body.push(text(format!(
                                            "Working directory: {}",
                                            cwd.display()
                                        )));
                                    }
                                    body = body.push(text(format!(
                                        "Desktop entry: {}",
                                        if plan.desktop().enabled { "yes" } else { "no" }
                                    )));
                                }
                            }
                            let settings = match game.as_ref() {
                                Game::Windows(plan) => plan.settings(),
                                Game::Native(plan) => plan.settings(),
                            };
                            let hook_review = match game.as_ref() {
                                Game::Windows(plan) => &plan.heroic().settings_review,
                                Game::Native(plan) => &plan.heroic().settings_review,
                            };
                            for (label, path) in hook_paths(hook_review) {
                                body = body.push(text(format!("{label}: {path}")));
                            }
                            body = body.push(text(format!(
                                "GameMode: {}, MangoHud: {}",
                                toggle_label(settings.gamemode),
                                toggle_label(settings.mangohud)
                            )));
                            if matches!(game.as_ref(), Game::Windows(_)) {
                                body = body.push(text(format!(
                                    "Wine Wayland: {}",
                                    toggle_label(settings.wayland)
                                )));
                            }
                            body = body.push(text(format!(
                                "Gamescope: {}",
                                match &settings.gamescope {
                                    None => "Use global default".to_owned(),
                                    Some(GamescopeSetting::Off) => "Off".to_owned(),
                                    Some(GamescopeSetting::On(scope)) =>
                                        format!("On {}", quote_args(&scope.args(false))),
                                }
                            )));
                            for (key, value) in settings.env {
                                body = body.push(text(format!("Environment: {key}={value}")));
                            }
                            for warning in review_warnings(&game.warnings()) {
                                body = body.push(text(warning).size(13).color(palette.warning));
                            }
                            if let Some((component, version)) = missing_build {
                                body = body.push(
                                    secondary_button(
                                        format!("Download {} build", component.label()),
                                        palette,
                                    )
                                    .on_press(Event::DownloadBuild(*component, version.clone())),
                                );
                            }
                        }
                        Review::Batch(library) => {
                            body = body.push(text(format!(
                                "{} ready, {} already imported",
                                library.ready_count(),
                                library.already().len()
                            )));
                            for name in library.already() {
                                body = body.push(
                                    text(format!("Already imported: {name}"))
                                        .size(13)
                                        .color(palette.muted),
                                );
                            }
                            for entry in library.entries() {
                                let line = match entry {
                                    Entry::Ready(game) => {
                                        format!(
                                            "Ready: {} -> {} ({})",
                                            game.title(),
                                            game.name(),
                                            if matches!(game, Game::Native(_)) {
                                                "native"
                                            } else if self.draft.adopt {
                                                "use in place"
                                            } else {
                                                "copy"
                                            }
                                        )
                                    }
                                    Entry::Pending(game) => format!(
                                        "Waiting: {} - {}",
                                        game.plan().heroic().title,
                                        game.reason()
                                    ),
                                    Entry::Skipped(game) => {
                                        format!(
                                            "Skipped {}: {}",
                                            game.title,
                                            skip_reason(&game.reason)
                                        )
                                    }
                                };
                                body = body.push(text(line).size(13));
                                if let Entry::Ready(game) = entry {
                                    let hook_review = match game {
                                        Game::Windows(plan) => &plan.heroic().settings_review,
                                        Game::Native(plan) => &plan.heroic().settings_review,
                                    };
                                    if let Some(marker) = hook_marker(hook_review) {
                                        body = body.push(
                                            text(format!("  {marker}"))
                                                .size(12)
                                                .color(palette.muted),
                                        );
                                    }
                                    for warning in review_warnings(&game.warnings()) {
                                        body = body.push(
                                            text(format!("  {warning}"))
                                                .size(12)
                                                .color(palette.warning),
                                        );
                                    }
                                    if let Game::Windows(plan) = game
                                        && let Some(note) = shared_prefix_note(
                                            plan.is_adopt(),
                                            plan.shared_prefix(),
                                        )
                                    {
                                        body = body.push(
                                            text(format!("  {note}"))
                                                .size(12)
                                                .color(palette.warning),
                                        );
                                    }
                                }
                            }
                            for offer in library.pending_builds() {
                                let version =
                                    offer.versions.iter().find_map(|version| version.clone());
                                body = body.push(
                                    secondary_button(
                                        format!(
                                            "Download {} for {} game(s)",
                                            offer.component.label(),
                                            offer.games
                                        ),
                                        palette,
                                    )
                                    .on_press(Event::DownloadBuild(offer.component, version)),
                                );
                            }
                        }
                    }
                    let can_import = match review {
                        Review::Single { missing_build, .. } => missing_build.is_none(),
                        Review::Batch(library) => {
                            library.ready_count() > 0 || !library.backfills().is_empty()
                        }
                    };
                    let mut import =
                        row![accent_base("Import", palette.primary).on_press_maybe(
                            (can_import && !self.waiting()).then_some(Event::Import)
                        )]
                        .spacing(12)
                        .align_y(Center);
                    if self.status_by_action() {
                        import = import.push(self.working(palette));
                    }
                    actions = Some(import.wrap().vertical_spacing(8).into());
                }
            }
            Step::Working => {
                for (name, result) in &self.results {
                    let line = match result {
                        Ok(notes) => format!(
                            "Imported {name}{}",
                            if notes.is_empty() {
                                ""
                            } else {
                                " (with notes)"
                            }
                        ),
                        Err(error) => format!("Failed {name}: {error}"),
                    };
                    body = body.push(text(line).size(13));
                }
                if self.registered.is_some() && self.error.is_some() && self.token.is_none() {
                    actions = Some(
                        row![
                            secondary_button("Retry artwork", palette).on_press(Event::RetryArt),
                            secondary_button("Skip artwork", palette).on_press(Event::SkipArt),
                        ]
                        .spacing(12)
                        .wrap()
                        .vertical_spacing(8)
                        .into(),
                    );
                } else if let Some(token) = &self.token {
                    let (label, event) = if token.is_canceled() {
                        ("Force stop", Event::Force)
                    } else {
                        ("Cancel", Event::Cancel)
                    };
                    actions = Some(secondary_button(label, palette).on_press(event).into());
                }
            }
            Step::Finished => {
                for (name, result) in &self.results {
                    body = body.push(
                        text(match result {
                            Ok(_) => format!("Imported {name}"),
                            Err(error) => format!("Failed {name}: {error}"),
                        })
                        .size(13),
                    );
                    if let Ok(notes) = result {
                        for note in notes {
                            body = body
                                .push(text(format!("  {note}")).size(12).color(palette.warning));
                        }
                    }
                }
                let mut done = row![].spacing(12).align_y(Center);
                if self.registered.is_some() {
                    done = done.push(
                        secondary_button("Open settings", palette).on_press(Event::OpenSettings),
                    );
                }
                done = done.push(
                    secondary_button("Back to library", palette).on_press(Event::BackToLibrary),
                );
                actions = Some(done.wrap().vertical_spacing(8).into());
            }
        }
        let back = secondary_button("Back", palette)
            .on_press_maybe((!self.busy() && self.step != Step::Finished).then_some(Event::Back));
        let scrolling: Element<'_, Event> = match (&self.listing, self.step) {
            // The title and notes scroll with the grid, above fixed controls, so
            // a short dialog still shows games. Keyboard focus scrolls the
            // grid to a known row.
            (Some(listing), Step::Games) => self.grid(listing, palette, width, body.into()),
            _ => scroll_body(body, Some(BODY_ID), palette),
        };
        let content: Element<'_, Event> = match actions {
            Some(actions) => column![scrolling]
                .push(actions)
                .spacing(14)
                .height(Fill)
                .into(),
            None => scrolling,
        };
        let mut page = column![].spacing(12).max_width(PAGE_MAX_WIDTH).height(Fill);
        if self.leave_prompt {
            page = page.push(text("Discard this Heroic import draft?")).push(
                row![
                    secondary_button("Keep editing", palette).on_press(Event::KeepEditing),
                    secondary_button("Discard", palette).on_press(Event::Discard)
                ]
                .spacing(12),
            );
        }
        page_frame(page.push(row![back].align_y(Center)).push(content))
    }
}

/// The widest an import page's content grows in a wide dialog.
pub(super) const PAGE_MAX_WIDTH: f32 = 780.0;
pub(super) const PAGE_PADDING: f32 = 24.0;
/// Keeps scrolled text clear of the scrollbar, which draws over its content.
pub(super) const SCROLL_GUTTER: f32 = crate::ui::SCROLLBAR_WIDTH + 6.0;

/// Insets an import page from the dialog's rounded border on every side, so
/// Back sits inside the corner and scrolled content ends above the bottom.
pub(super) fn page_frame<'a, Message: 'a>(
    page: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(page)
        .padding(PAGE_PADDING)
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .into()
}

/// A step's action row, pinned under the scrolling content.
pub(super) fn pinned<'a, Message: 'a>(
    footer: impl Into<Element<'a, Message>>,
    palette: Palette,
) -> Element<'a, Message> {
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

pub(super) fn scroll_body<'a, Message: 'a>(
    body: impl Into<Element<'a, Message>>,
    id: Option<&'static str>,
    palette: Palette,
) -> Element<'a, Message> {
    let mut body = scrollable(container(body).padding(iced::Padding::ZERO.right(SCROLL_GUTTER)))
        .wineo_style(palette)
        .width(Fill)
        .height(Fill);
    if let Some(id) = id {
        body = body.id(iced::widget::Id::new(id));
    }
    body.into()
}

fn field<'a>(
    label: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> Event + 'a,
) -> Element<'a, Event> {
    column![
        text(label).size(13),
        text_input(label, value).on_input(on_input).padding(8)
    ]
    .spacing(4)
    .into()
}

fn launch_settings(draft: &Draft, windows: bool, palette: Palette) -> Element<'_, Event> {
    let mut body = column![text("Launch settings").size(19)].spacing(9);
    body = body.push(text("Environment variables (KEY=VALUE)").size(13));
    body = body.push(
        Chips {
            items: &draft.environment,
            input: &draft.environment_input,
            placeholder: "KEY=VALUE",
            error: None,
            on_input: Box::new(Event::EnvironmentInput),
            on_add: Event::EnvironmentAdd,
            on_remove: Box::new(Event::EnvironmentRemove),
            enabled: true,
        }
        .view(palette),
    );
    body = body.push(
        secondary_button("Clear environment variables", palette).on_press(Event::EnvironmentClear),
    );
    body = body.push(
        checkbox(draft.gamemode)
            .label("GameMode")
            .on_toggle(Event::Gamemode),
    );
    body = body.push(
        checkbox(draft.mangohud)
            .label("MangoHud")
            .on_toggle(Event::Mangohud),
    );
    if windows {
        body = body.push(
            checkbox(draft.wayland)
                .label("Wine Wayland")
                .on_toggle(Event::Wayland),
        );
        body = body.push(
            checkbox(draft.hdr)
                .label("HDR (requires Wine Wayland)")
                .on_toggle(Event::Hdr),
        );
    }
    let scope_on = matches!(draft.gamescope.setting, Some(GamescopeSetting::On(_)));
    body = body.push(
        checkbox(scope_on)
            .label("Gamescope")
            .on_toggle(Event::GamescopeEnabled),
    );
    if scope_on {
        let scope = draft.gamescope.scope();
        for (index, label) in SCOPE_NUMBER_LABELS.into_iter().enumerate() {
            body = body.push(field(
                label,
                &draft.gamescope.numbers[index],
                move |value| Event::GamescopeNumber(index, value),
            ));
        }
        for (index, (label, selected, choices)) in [
            (
                "Filter",
                scope.filter.map(Filter::as_str),
                Filter::ALL
                    .iter()
                    .map(|value| value.as_str().to_owned())
                    .collect::<Vec<_>>(),
            ),
            (
                "Scaler",
                scope.scaler.map(Scaler::as_str),
                Scaler::ALL
                    .iter()
                    .map(|value| value.as_str().to_owned())
                    .collect::<Vec<_>>(),
            ),
            (
                "Window",
                scope.window.map(Window::as_str),
                Window::ALL
                    .iter()
                    .map(|value| value.as_str().to_owned())
                    .collect::<Vec<_>>(),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let mut options = vec!["Default".to_owned()];
            options.extend(choices);
            body = body.push(text(label).size(13));
            body = body.push(
                pick_list(
                    options,
                    Some(selected.unwrap_or("Default").to_owned()),
                    move |value| Event::GamescopeChoice(index, value),
                )
                .wineo_style(palette),
            );
        }
        body = body.push(
            checkbox(scope.grab_cursor)
                .label("Grab cursor")
                .on_toggle(Event::GamescopeGrab),
        );
        body = body.push(text("Extra gamescope arguments").size(13));
        body = body.push(
            Chips {
                items: &draft.gamescope.args,
                input: &draft.gamescope.args_input,
                placeholder: "One argument",
                error: None,
                on_input: Box::new(Event::GamescopeArgInput),
                on_add: Event::GamescopeArgAdd,
                on_remove: Box::new(Event::GamescopeArgRemove),
                enabled: true,
            }
            .view(palette),
        );
    }
    body.into()
}

const GRID_ID: &str = "heroic-grid";
const BODY_ID: &str = "heroic-body";

fn prepare_game(
    paths: &Paths,
    source: &HeroicSource,
    candidate: HeroicCandidate,
) -> Result<Prepared, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    let (runners, names) = runner_choices(paths, &global, true)?;
    let (
        suggested,
        runner,
        raw_arguments,
        working_directory,
        environment,
        gamemode,
        wayland,
        mangohud,
        gamescope,
        hook_paths,
        hdr,
    ) = if candidate.kind == HeroicGameKind::Windows {
        let imported = source.inspect(&candidate.id).map_err(|e| e.to_string())?;
        let (suggestion, runner) = suggest(paths, &global, &runners, &names, &imported)?;
        let hook_paths = hook_paths(&imported.settings_review);
        (
            Some(suggestion),
            runner,
            imported.launcher_args,
            imported
                .cwd
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            imported
                .env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
            imported.gamemode,
            Some(imported.enable_wine_wayland),
            imported.mangohud,
            imported.gamescope,
            hook_paths,
            imported.enable_hdr,
        )
    } else {
        let imported = source
            .inspect_native(&candidate.id)
            .map_err(|e| e.to_string())?;
        let hook_paths = hook_paths(&imported.settings_review);
        (
            None,
            String::new(),
            imported.launcher_args,
            String::new(),
            imported
                .env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
            imported.gamemode,
            None,
            imported.mangohud,
            imported.gamescope,
            hook_paths,
            false,
        )
    };
    let (arguments, unparsed_arguments) =
        match wineo_core::heroic::parse_launcher_args(&raw_arguments) {
            Ok(arguments) => (arguments, None),
            Err(_) => (Vec::new(), Some(raw_arguments)),
        };
    Ok(Prepared {
        candidate,
        runners: names,
        suggested,
        runner,
        offer: runner_label::offer(paths, &global),
        arguments,
        unparsed_arguments,
        working_directory,
        environment,
        gamemode,
        wayland,
        mangohud,
        gamescope,
        hook_paths,
        hdr,
    })
}

/// Installed runners as the pickers show them, with the system wine first
/// when it's wanted and there is one.
pub(super) fn runner_choices(
    paths: &Paths,
    global: &GlobalConfig,
    system: bool,
) -> Result<(Runners, Vec<Choice>), String> {
    let runners = Runners::new(paths, global).map_err(|e| e.to_string())?;
    let mut choices: Vec<Choice> = runners
        .list()
        .map_err(|e| e.to_string())?
        .runners
        .iter()
        .map(|runner| Choice::of(paths, runner))
        .collect();
    if system && runners.resolve("wine").is_ok() {
        choices.insert(0, Choice::plain("wine"));
    }
    Ok((runners, choices))
}

/// What to do about a Windows game's runner, and the runner that starts out
/// picked for it.
pub(super) fn suggest(
    paths: &Paths,
    global: &GlobalConfig,
    runners: &Runners,
    choices: &[Choice],
    imported: &wineo_core::heroic::HeroicImport,
) -> Result<(RunnerSuggestion, String), String> {
    let suggestion =
        heroic_import::suggest_runner(paths, global, imported, None).map_err(|e| e.to_string())?;
    let runner = match &suggestion {
        RunnerSuggestion::Use(name) => name.clone(),
        RunnerSuggestion::Copy(_) => String::new(),
        RunnerSuggestion::Choose { proton, .. } => choices
            .iter()
            .find(|choice| {
                runners
                    .resolve(&choice.name)
                    .is_ok_and(|runner| runner.is_proton() == *proton)
            })
            .map(|choice| choice.name.clone())
            .unwrap_or_default(),
    };
    Ok((suggestion, runner))
}

fn batch_options(paths: &Paths) -> Result<BatchOptions, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    let (runners, choices) = runner_choices(paths, &global, false)?;
    let runner = runners
        .newest_proton()
        .map_err(|e| e.to_string())?
        .map(|runner| runner.selection().to_owned())
        .unwrap_or_default();
    Ok(BatchOptions {
        runner,
        runners: choices,
        art_fetch_enabled: !global.art.fetch.kinds().is_empty(),
        offer: runner_label::offer(paths, &global),
    })
}

fn refresh(
    paths: &Paths,
    single: Option<(HeroicSource, HeroicCandidate)>,
) -> Result<Refreshed, String> {
    let Some((source, candidate)) = single else {
        return batch_options(paths).map(Refreshed::Batch);
    };
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    let (runners, choices) = runner_choices(paths, &global, true)?;
    let imported = source.inspect(&candidate.id).map_err(|e| e.to_string())?;
    let (suggested, runner) = suggest(paths, &global, &runners, &choices, &imported)?;
    Ok(Refreshed::Single {
        runners: choices,
        suggested: Some(suggested),
        runner,
    })
}

/// The review's runner line, naming the launcher when the build isn't
/// wineo's, like `Runner: GE-Proton-latest (Heroic)`.
pub(super) fn runner_line(paths: &Paths, runner: &wineo_core::runner::Runner) -> String {
    format!(
        "Runner: {}",
        runner_label::labeled(runner.selection(), runner_label::launcher(paths, runner))
    )
}

fn toggle_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "On",
        Some(false) => "Off",
        None => "Use global default",
    }
}

fn quote_args(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| format!("'{}'", argument.replace('\'', "'\"'\"'")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn plan_review(
    paths: &Paths,
    listing: &HeroicListing,
    selected: Option<usize>,
    draft: &Draft,
) -> Result<Review, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    if draft.desktop {
        wineo_core::desktop::supported(paths).map_err(|e| e.to_string())?;
    }
    if draft.batch {
        let has_windows = listing.candidates.iter().any(|candidate| {
            draft.games.contains(&candidate.id) && candidate.kind == HeroicGameKind::Windows
        });
        let runner = if has_windows {
            if draft.runner.trim().is_empty() {
                return Err("Choose or download a runner for Windows games".to_owned());
            }
            Some(draft.runner.as_str())
        } else {
            None
        };
        let mut library = heroic_import::plan_selection(
            paths,
            &global,
            listing,
            &draft.games,
            runner,
            &LibraryOptions {
                adopt: draft.adopt,
                arch: None,
            },
        )
        .map_err(|e| format!("{e:#}"))?;
        library.recheck(paths, &global);
        return Ok(Review::Batch(library));
    }
    let candidate = selected
        .and_then(|index| listing.candidates.get(index))
        .ok_or("Choose a Heroic game")?;
    if draft.name.trim().is_empty() {
        return Err("Enter an environment name".to_owned());
    }
    if let Some(problem) = wineo_core::name::problem(&draft.name) {
        return Err(format!("Invalid environment name: {problem}"));
    }
    if paths.environment_config(&draft.name).exists() {
        return Err(format!("{} already exists", draft.name));
    }
    let source = &listing.source;
    let mut missing_build = None;
    let game = if candidate.kind == HeroicGameKind::Native {
        let mut plan = heroic_import::plan_native(source, &candidate.id, Some(draft.name.clone()))
            .map_err(|e| e.to_string())?;
        plan.set_executable(PathBuf::from(&draft.executable))
            .map_err(|e| e.to_string())?;
        plan.set_launcher_args(&quote_args(&draft.arguments))
            .map_err(|e| e.to_string())?;
        plan.set_working_directory(
            (!draft.working_directory.trim().is_empty())
                .then(|| PathBuf::from(&draft.working_directory)),
        );
        if draft.environment_dirty {
            plan.set_environment(&draft.environment)
                .map_err(|e| e.to_string())?;
        }
        if draft.gamemode_dirty {
            plan.set_gamemode(draft.gamemode);
        }
        if draft.mangohud_dirty {
            plan.set_mangohud(draft.mangohud);
        }
        if draft.gamescope.dirty {
            plan.set_gamescope(draft.gamescope.resolved()?);
        }
        plan.set_desktop_enabled(draft.desktop);
        plan.reject_invalid().map_err(|e| e.to_string())?;
        plan.check_exe(paths, &global).map_err(|e| e.to_string())?;
        Game::Native(plan)
    } else {
        if draft.runner.trim().is_empty() {
            return Err("Choose or download a runner".to_owned());
        }
        let mut plan = heroic_import::plan_game(
            paths,
            &global,
            source,
            &candidate.id,
            &draft.runner,
            Options {
                name: Some(draft.name.clone()),
                adopt: draft.adopt,
                ..Options::default()
            },
        )
        .map_err(|e| format!("{e:#}"))?;
        let exe = plan
            .resolve_path(paths, &draft.executable)
            .map_err(|e| e.to_string())?;
        plan.set_executable(exe).map_err(|e| e.to_string())?;
        let cwd = if draft.working_directory.trim().is_empty() {
            None
        } else {
            Some(
                plan.resolve_path(paths, &draft.working_directory)
                    .map_err(|e| e.to_string())?,
            )
        };
        plan.set_working_directory(cwd);
        plan.set_launcher_args(&quote_args(&draft.arguments))
            .map_err(|e| e.to_string())?;
        if draft.environment_dirty {
            let mut entries = draft.environment.clone();
            let hdr_key = if plan.runner().is_proton() {
                "PROTON_ENABLE_HDR"
            } else {
                "DXVK_HDR"
            };
            if plan.resolved().enable_hdr
                && !entries
                    .iter()
                    .any(|entry| entry.starts_with(&format!("{hdr_key}=")))
                && let Some(value) = plan.resolved().env.get(hdr_key)
            {
                entries.push(format!("{hdr_key}={value}"));
            }
            plan.set_environment(&entries).map_err(|e| e.to_string())?;
        }
        if draft.gamemode_dirty {
            plan.set_gamemode(draft.gamemode);
        }
        if draft.mangohud_dirty {
            plan.set_mangohud(draft.mangohud);
        }
        if draft.gamescope.dirty {
            plan.set_gamescope(draft.gamescope.resolved()?);
        }
        if draft.wayland_dirty {
            plan.set_wayland(draft.wayland).map_err(|e| e.to_string())?;
        }
        if draft.hdr_dirty {
            plan.set_hdr(draft.hdr).map_err(|e| e.to_string())?;
        }
        plan.set_desktop_enabled(draft.desktop);
        plan.resolve(Policy::Defaults);
        if let Some(finding) = plan.blocking().next() {
            return Err(finding.issue.message.clone());
        }
        missing_build = plan
            .missing_build(paths, &global)
            .map_err(|e| e.to_string())?
            .map(|missing| (missing.component, missing.version));
        Game::Windows(plan)
    };
    let runner_line = match &game {
        Game::Windows(plan) => Some(runner_line(paths, plan.runner())),
        Game::Native(_) => None,
    };
    Ok(Review::Single {
        game: Box::new(game),
        missing_build,
        runner_line,
    })
}

fn apply_game(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    game: &Game,
    token: &Token,
    progress: impl FnMut(import::ImportProgress),
) -> anyhow::Result<Registered> {
    if token.is_canceled() {
        anyhow::bail!("Import canceled");
    }
    match game {
        Game::Windows(plan) => heroic_import::apply(paths, global, source, plan, token, progress),
        Game::Native(plan) => heroic_import::apply_native(paths, source, plan),
    }
}

fn heroic_id(game: &Game) -> &str {
    match game {
        Game::Windows(plan) => &plan.heroic().id,
        Game::Native(plan) => &plan.heroic().id,
    }
}

/// A Heroic poster a batch may store, and whether the grid decoded it.
#[derive(Debug, Clone)]
struct BatchPoster {
    path: PathBuf,
    decoded: bool,
}

/// Stores Heroic's cached poster as a batch-imported environment's Grid art.
/// It runs before SteamGridDB, which then only fills what's still missing. Art
/// never undoes an import, so a failure is only a note. Runs on a worker,
/// since a poster the grid hadn't decoded yet is decoded here.
fn store_heroic_poster(
    paths: &Paths,
    name: &str,
    app: &str,
    poster: Option<&BatchPoster>,
) -> Option<String> {
    let poster = poster?;
    // Core only checks the signature, so the image has to decode first.
    if !poster.decoded
        && !posters::decode_waiting(&poster.path, &paths.art_cache_dir(), None)
            .is_some_and(|decoded| decoded.is_ok())
    {
        return None;
    }
    heroic_import::store_poster(paths, name, app, &poster.path)
        .err()
        .map(|error| format!("Heroic poster: {error}"))
}

/// Why a batch left a game out, as the end of "Skipped Title: ...".
fn skip_reason(reason: &SkipReason) -> String {
    match reason {
        SkipReason::LauncherArgs => "Heroic's launch arguments couldn't be parsed".to_owned(),
        SkipReason::NeedsRunner { proton: true } => "needs a Proton runner".to_owned(),
        SkipReason::NeedsRunner { proton: false } => "needs a Wine runner".to_owned(),
        SkipReason::NameTaken(name) => format!("the name {name} is taken"),
        SkipReason::Blocked(message)
        | SkipReason::Unreadable(message)
        | SkipReason::Unavailable(message) => message.clone(),
    }
}

/// Only matters when the prefix is used in place, since then the other games
/// keep changing it; a copy is wineo's own.
fn shared_prefix_note(adopt: bool, shared: &[HeroicSharedPrefix]) -> Option<String> {
    if !adopt || shared.is_empty() {
        return None;
    }
    let titles: Vec<&str> = shared.iter().map(|game| game.title.as_str()).collect();
    Some(format!("Shares prefix with: {}", titles.join(", ")))
}

/// How often copy progress reaches the UI. Core reports every copied file,
/// and a message per file floods the channel on a large prefix.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Turns core's import progress into a readable phase, throttled.
#[derive(Debug, Default)]
struct CopyProgress {
    total: u64,
    copied: u64,
    sent: Option<Instant>,
}

impl CopyProgress {
    /// The phase to show for `progress`, or None when it's too soon after
    /// the last one.
    fn record(&mut self, progress: &import::ImportProgress, now: Instant) -> Option<String> {
        self.due(progress, now).then(|| self.phase(progress))
    }

    /// Counts `progress` and says whether it should show. The last copied
    /// file always shows.
    fn due(&mut self, progress: &import::ImportProgress, now: Instant) -> bool {
        if let import::ImportProgress::Started { entries } = progress {
            self.total = *entries;
            self.copied = 0;
        }
        if let import::ImportProgress::Copied { .. } = progress {
            self.copied += 1;
            let due = self.copied >= self.total
                || self
                    .sent
                    .is_none_or(|sent| now.duration_since(sent) >= PROGRESS_INTERVAL);
            if !due {
                return false;
            }
        }
        self.sent = Some(now);
        true
    }

    fn counts(&self) -> String {
        format!("{} of {} files", grouped(self.copied), grouped(self.total))
    }

    fn phase(&self, progress: &import::ImportProgress) -> String {
        match progress {
            import::ImportProgress::Started { .. } | import::ImportProgress::Copied { .. } => {
                format!("copying prefix ({})", self.counts())
            }
            import::ImportProgress::Checking { entries } => {
                format!("checking prefix ({} files)", grouped(*entries))
            }
            import::ImportProgress::Finished { .. } => "finishing".to_owned(),
        }
    }
}

/// "(2 of 5) Importing Title: phase", without the position for a single game
/// and with an ellipsis before the first phase.
fn import_status(title: &str, position: Option<(usize, usize)>, phase: Option<&str>) -> String {
    let position = position
        .map(|(index, count)| format!("({index} of {count}) "))
        .unwrap_or_default();
    match phase {
        Some(phase) => format!("{position}Importing {title}: {phase}"),
        None => format!("{position}Importing {title}..."),
    }
}

/// `n` with comma thousands separators.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// How a Heroic config was found, as the Sources page names it.
fn origin_label(origin: HeroicOrigin) -> &'static str {
    match origin {
        HeroicOrigin::Native => "Native",
        HeroicOrigin::Flatpak => "Flatpak",
        HeroicOrigin::Explicit => "Folder",
    }
}

fn hook_paths(review: &SettingsReview) -> Vec<(&'static str, String)> {
    review
        .hooks()
        .into_iter()
        .map(|(field, path)| {
            let label = if field == "before hook" {
                "Before hook"
            } else {
                "After hook"
            };
            (label, path)
        })
        .collect()
}

fn hook_marker(review: &SettingsReview) -> Option<&'static str> {
    (!review.hooks().is_empty()).then_some("runs launch scripts")
}

/// The review only calls out anti-cheat, once even when both EAC and
/// BattlEye are on, since the game may not run without it.
pub(super) fn review_warnings(warnings: &[Warning]) -> Vec<&'static str> {
    if warnings.contains(&Warning::AntiCheatNotImported) {
        vec!["Heroic-managed anti-cheat runtime is not imported"]
    } else {
        Vec::new()
    }
}

pub(super) fn playtime_note(playtime: &Playtime) -> Option<String> {
    match playtime {
        Playtime::None | Playtime::Imported => None,
        Playtime::Unreadable(reason) => {
            Some(format!("Heroic playtime could not be read: {reason}"))
        }
        Playtime::NotRecorded(reason) => {
            Some(format!("Heroic playtime could not be recorded: {reason}"))
        }
    }
}

#[derive(Clone)]
pub(super) enum Prerequisite {
    CopyRunner(RunnerCopy),
    Download(Provider, Option<String>),
}

impl Prerequisite {
    pub(super) fn label(&self) -> String {
        match self {
            Self::CopyRunner(copy) => format!("Copying Heroic runner {}...", copy.name),
            Self::Download(provider, _) => format!("Downloading {provider}..."),
        }
    }
}

pub(super) fn install_prerequisite(
    paths: &Paths,
    operation: Prerequisite,
    token: &Token,
    report: &dyn Fn(String),
) -> Result<String, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    match operation {
        Prerequisite::CopyRunner(copy) => {
            let root = global.runner_root(paths).map_err(|e| e.to_string())?;
            let _source_guard = copy
                .lock_source(paths, &global)
                .map_err(|e| e.to_string())?;
            let mut files = CopyProgress::default();
            let receipt = import::copy_runner_reviewed(
                &copy.inspection,
                root,
                &copy.name,
                token,
                |progress| {
                    if files.due(&progress, Instant::now()) {
                        let line = match progress {
                            import::ImportProgress::Started { .. }
                            | import::ImportProgress::Copied { .. } => {
                                format!("Copying {} ({})", copy.name, files.counts())
                            }
                            import::ImportProgress::Checking { entries } => {
                                format!("Checking {} ({} files)", copy.name, grouped(entries))
                            }
                            import::ImportProgress::Finished { .. } => {
                                format!("Finishing copy of {}", copy.name)
                            }
                        };
                        report(line);
                    }
                },
            )
            .map_err(|e| format!("{e:#}"))?;
            Ok(receipt.name)
        }
        Prerequisite::Download(provider, version) => {
            let client = Client::new().map_err(|e| e.to_string())?;
            let release = match version {
                Some(name) => client.select_pinned(provider, &name),
                None => client.select(provider, None),
            }
            .map_err(|e| format!("{e:#}"))?;
            crate::download_progress::install(
                &client,
                paths,
                &global,
                &release,
                token,
                PROGRESS_INTERVAL,
                report,
            )
            .map_err(|e| format!("{e:#}"))?;
            Ok(release.installed_name)
        }
    }
}

#[cfg(test)]
mod tests;
