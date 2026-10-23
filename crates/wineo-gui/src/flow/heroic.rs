mod edit;
mod finish;
mod update;
mod view;
mod work;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use iced::futures::channel::mpsc::unbounded;
use iced::keyboard::{self, Key};
use iced::widget::operation::RelativeOffset;
use iced::widget::{image, row, text};
use iced::{Center, Element, Task};
use wineo_core::art::{Format, Kind};
use wineo_core::cancel::Token;
use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::download::Lazy;
use wineo_core::environment::{self, Environment};
use wineo_core::gamescope::{Gamescope, GamescopeSetting};
use wineo_core::heroic::{HeroicCandidate, HeroicGameKind, HeroicListing, HeroicSource};
use wineo_core::heroic_import::{self, Game, LibraryPlan, Registered, RunnerSuggestion};
use wineo_core::paths::Paths;

use super::import_grid::{GridKey, ImportGrid};
use super::{Action, artwork, posters};
use crate::focus_button::ListRows;
use crate::runner_label::{self, Choice};
use crate::task;
use crate::theme::Palette;
use work::{
    BatchPoster, CopyProgress, apply_game, batch_options, heroic_id, hook_paths, import_status,
    plan_review, prepare_game, quote_args, refresh, store_heroic_poster,
};
pub(super) use work::{
    Prerequisite, install_prerequisite, playtime_note, review_warnings, runner_choices, suggest,
};

const SOURCE_LIST: &str = "heroic-sources";

/// A source row's id, by its config folder.
fn source_id(source: &HeroicSource) -> String {
    format!("heroic-source:{}", source.config_dir().display())
}

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

    /// The first step's source list, which fills in once discovery answers.
    pub(crate) fn rows(&self) -> Option<ListRows> {
        (self.step == Step::Sources).then(|| ListRows {
            list: SOURCE_LIST,
            ids: self.sources.iter().map(source_id).collect(),
        })
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
                                notes.extend(registered.warnings.iter().cloned());
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
}

const GRID_ID: &str = "heroic-grid";
const BODY_ID: &str = "heroic-body";

#[cfg(test)]
mod tests;
