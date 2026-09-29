use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use iced::futures::channel::mpsc::{UnboundedSender, unbounded};
use iced::widget::{
    button, checkbox, column, container, pick_list, row, scrollable, text, text_input,
};
use iced::{Center, Element, Fill, Task};
use wineo_core::cancel::Token;
use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Provider};
use wineo_core::environment::{self, Environment};
use wineo_core::gamescope::{Filter, Gamescope, GamescopeSetting, Scaler, Window};
use wineo_core::heroic::{HeroicCandidate, HeroicGameKind, HeroicListing, HeroicSource};
use wineo_core::heroic_import::{
    self, Entry, Game, LibraryOptions, LibraryPlan, Options, Playtime, Policy, Registered,
    RunnerCopy, RunnerSuggestion,
};
use wineo_core::import;
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb;

use super::{Action, artwork};
use crate::chips::Chips;
use crate::theme::Palette;

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
    },
    Batch(LibraryPlan),
}

#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    candidate: HeroicCandidate,
    runners: Vec<String>,
    suggested: Option<RunnerSuggestion>,
    runner: String,
    arguments: Vec<String>,
    unparsed_arguments: Option<String>,
    working_directory: String,
    environment: Vec<String>,
    gamemode: Option<bool>,
    wayland: Option<bool>,
    mangohud: Option<bool>,
    gamescope: Option<GamescopeSetting>,
    hdr: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct BatchOptions {
    runner: String,
    runners: Vec<String>,
    art_available: bool,
    art_note: String,
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
    adopt: bool,
    desktop: bool,
    batch: bool,
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
    Game(usize),
    Batch,
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
        };
        let task = page.discover();
        (page, task)
    }

    pub(crate) fn busy(&self) -> bool {
        self.token.is_some()
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
            Step::Games => {
                let eligible: Vec<usize> = self
                    .listing
                    .as_ref()
                    .map(|listing| {
                        listing
                            .candidates
                            .iter()
                            .enumerate()
                            .filter_map(|(index, candidate)| {
                                (heroic_import::blocker(candidate).is_none()
                                    && !self.already.contains_key(&candidate.id))
                                .then_some(index)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if eligible.len() == 1 {
                    self.update(Event::Game(eligible[0]))
                } else {
                    Task::none()
                }
            }
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
        match self.step {
            Step::Sources => {
                if self.source.is_some() {
                    self.leave_prompt = true
                } else {
                    self.exit()
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
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    heroic_import::sources(&paths, None).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |result| Event::SourcesLoaded(generation, result),
        )
    }

    fn list(&mut self, source: HeroicSource) -> Task<Event> {
        self.next_picker();
        self.step = Step::Games;
        self.status = "Reading Heroic games...".to_owned();
        self.error = None;
        self.listing = None;
        self.already.clear();
        self.prepared = None;
        self.batch_options = None;
        self.review = None;
        let generation = self.next_generation();
        let paths = self.paths.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                    let listing = heroic_import::listing(&paths, &global, &source)
                        .map_err(|e| e.to_string())?;
                    let mut already = HashMap::new();
                    let mut existing = Vec::new();
                    for name in environment::names(&paths).map_err(|e| e.to_string())? {
                        if let Ok((environment, _)) =
                            Environment::load_for_heroic_match(&paths, &name)
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
                })
                .await
                .map_err(|e| e.to_string())?
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
        self.status = format!("Inspecting {}...", candidate.title);
        self.error = None;
        let paths = self.paths.clone();
        let source = listing.source.clone();
        let generation = self.next_generation();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    prepare_game(&paths, &source, candidate).map(Box::new)
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |result| Event::Prepared(generation, result),
        )
    }

    fn prepare_batch(&mut self) -> Task<Event> {
        self.next_picker();
        self.selected = None;
        self.draft = Draft {
            batch: true,
            ..Draft::default()
        };
        self.prepared = None;
        self.step = Step::Details;
        self.status = "Checking installed runners...".to_owned();
        let paths = self.paths.clone();
        let generation = self.next_generation();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                    let runners = Runners::new(&paths, &global).map_err(|e| e.to_string())?;
                    let runner = runners.newest_proton().map_err(|e| e.to_string())?
                        .map(|runner| runner.selection().to_owned()).unwrap_or_default();
                    let names = runners.list().map_err(|e| e.to_string())?.runners.into_iter()
                        .map(|runner| runner.selection().to_owned()).collect();
                    let (art_available, art_note) = if global.art.fetch.kinds().is_empty() {
                        (false, "Automatic artwork is disabled in Global Settings".to_owned())
                    } else {
                        match steamgriddb::find_key(&paths, |key| std::env::var_os(key), &SecretServiceKeyring::steamgriddb()) {
                            Ok(Some(_)) => (true, String::new()),
                            Ok(None) => (false, "SteamGridDB needs a configured key. Manual artwork is available in single-game import.".to_owned()),
                            Err(error) => (false, format!("SteamGridDB key unavailable: {error}")),
                        }
                    };
                    Ok(BatchOptions { runner, runners: names, art_available, art_note })
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |result| Event::BatchPrepared(generation, result),
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
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    plan_review(&paths, &listing, selected, &draft).map(Box::new)
                })
                .await
                .map_err(|e| e.to_string())?
            },
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
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    install_prerequisite(&paths, operation, &token, &sender, generation)
                })
                .await
                .map_err(|e| e.to_string())?
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
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Working;
        self.error = None;
        self.results.clear();
        self.status = "Importing...".to_owned();
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let fetch_batch_art = self.draft.fetch_batch_art;
        let batch_desktop = self.draft.desktop;
        match review {
            Review::Single { game, .. } => {
                let (sender, receiver) = unbounded();
                let work = Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let global =
                                GlobalConfig::load(&paths).map_err(|error| ImportFailure {
                                    message: error.to_string(),
                                    source_changed: false,
                                })?;
                            apply_game(&paths, &global, &source, &game, &token, |progress| {
                                let _ = sender.unbounded_send(Event::Progress(
                                    generation,
                                    format!("Import: {progress:?}"),
                                ));
                            })
                            .map(Arc::new)
                            .map_err(|error| ImportFailure {
                                source_changed: error.is::<heroic_import::ApplyError>(),
                                message: format!("{error:#}"),
                            })
                        })
                        .await
                        .map_err(|error| ImportFailure {
                            message: error.to_string(),
                            source_changed: false,
                        })?
                    },
                    move |result| Event::SingleRegistered(generation, result),
                );
                Task::batch([Task::run(receiver, |event| event), work])
            }
            Review::Batch(library) => {
                let (sender, receiver) = unbounded();
                let work = Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                            if let Err(error) = library.record_backfills(&paths) {
                                let _ = sender.unbounded_send(Event::Progress(
                                    generation,
                                    format!("Heroic ID backfill: {error}"),
                                ));
                            }
                            let mut source_changed = false;
                            for mut game in library.into_ready() {
                                if token.is_canceled() {
                                    break;
                                }
                                match &mut game {
                                    Game::Windows(plan) => plan.set_desktop_enabled(batch_desktop),
                                    Game::Native(plan) => plan.set_desktop_enabled(batch_desktop),
                                }
                                let name = game.name().to_owned();
                                let title = game.title().to_owned();
                                let _ = sender.unbounded_send(Event::Progress(
                                    generation,
                                    format!("Importing {name}..."),
                                ));
                                let result = (|| {
                                    let registered = apply_game(
                                        &paths,
                                        &global,
                                        &source,
                                        &game,
                                        &token,
                                        |progress| {
                                            let _ = sender.unbounded_send(Event::Progress(
                                                generation,
                                                format!("{name}: {progress:?}"),
                                            ));
                                        },
                                    )?;
                                    let mut notes = Vec::new();
                                    if let Some(note) = playtime_note(&registered.playtime) {
                                        notes.push(note);
                                    }
                                    if fetch_batch_art {
                                        notes.extend(
                                            artwork::fetch_default(&paths, &name, &title)
                                                .unwrap_or_else(|error| {
                                                    vec![format!("Artwork: {error}")]
                                                }),
                                        );
                                    }
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
                                let _ = sender
                                    .unbounded_send(Event::GameResult(generation, name, result));
                                if source_changed {
                                    break;
                                }
                            }
                            Ok::<_, String>((source_changed, token.is_canceled()))
                        })
                        .await
                        .map_err(|e| e.to_string())?
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
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                    registered
                        .finish(&paths, &global)
                        .map(|changes| {
                            changes
                                .into_iter()
                                .map(|change| format!("{change:?}"))
                                .collect()
                        })
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |result| Event::FinishedRegistered(generation, result),
        )
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Back => self.back(),
            Event::Discard => {
                self.leave_prompt = false;
                self.exit();
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
                        self.listing = Some(listing);
                        self.already = already;
                        self.status.clear();
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Game(index) => self.prepare(index),
            Event::Batch => self.prepare_batch(),
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
                self.review = None;
                Task::none()
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
                self.draft.fetch_batch_art = value;
                Task::none()
            }
            Event::Plan => self.plan(),
            Event::Planned(generation, result) if generation == self.generation => {
                match result {
                    Ok(review) => {
                        self.review = Some(*review);
                        self.step = Step::Review;
                        self.status.clear();
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
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
                        self.status = format!(
                            "Imported {}. Adding artwork...",
                            registered.environment.name
                        );
                        let name = registered.environment.name.clone();
                        self.registered = Some(registered);
                        let art = self.art.clone();
                        let paths = self.paths.clone();
                        Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || art.apply(&paths, &name))
                                    .await
                                    .map_err(|e| e.to_string())?
                            },
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
                            self.step = Step::Review;
                            Task::none()
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
                        self.step = Step::Review;
                        return Task::none();
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
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || art.apply(&paths, &name))
                            .await
                            .map_err(|e| e.to_string())?
                    },
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
                .update(event)
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

    pub(crate) fn view(&self, palette: Palette) -> Element<'_, Event> {
        let mut body = column![].spacing(14).width(Fill);
        let title = match self.step {
            Step::Sources => "Import from Heroic",
            Step::Games => "Choose a Heroic game",
            Step::Details => {
                if self.draft.batch {
                    "Import all eligible games"
                } else {
                    "Import details"
                }
            }
            Step::Review => "Review Heroic import",
            Step::Working => "Importing from Heroic",
            Step::Finished => "Heroic import finished",
        };
        body = body.push(text(title).size(26).color(palette.foreground));
        if !self.status.is_empty() {
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
                for (index, source) in self.sources.iter().enumerate() {
                    body = body.push(
                        button(text(format!(
                            "{:?}: {}",
                            source.origin(),
                            source.config_dir().display()
                        )))
                        .on_press(Event::Source(index)),
                    );
                }
                body = body.push(
                    button("Browse for Heroic config folder...").on_press(Event::BrowseSource),
                );
                body = body.push(field(
                    "Config folder path",
                    &self.source_path,
                    Event::SourcePath,
                ));
                body = body.push(button("Open folder").on_press(Event::OpenSourcePath));
            }
            Step::Games => {
                if let Some(listing) = &self.listing {
                    for (index, candidate) in listing.candidates.iter().enumerate() {
                        let label = format!("{} ({})", candidate.title, kind_label(candidate.kind));
                        let issue = heroic_import::blocker(candidate);
                        let row = if let Some(name) = self.already.get(&candidate.id) {
                            button(text(format!("{label} - already imported as {name}")))
                                .on_press(Event::OpenExisting(name.clone()))
                        } else {
                            button(text(label))
                                .on_press_maybe(issue.is_none().then_some(Event::Game(index)))
                        };
                        body = body.push(row);
                        if let Some(issue) = issue {
                            body = body.push(text(&issue.message).size(12).color(palette.warning));
                        }
                    }
                    for issue in &listing.issues {
                        body = body.push(text(&issue.message).size(12).color(palette.warning));
                    }
                    body = body.push(button("Import all eligible games").on_press(Event::Batch));
                }
            }
            Step::Details => {
                if self.draft.batch {
                    body = body.push(
                        text("Windows games use the selected runner. Native games need no runner.")
                            .color(palette.muted),
                    );
                    body = body.push(field("Windows runner", &self.draft.runner, Event::Runner));
                    if let Some(options) = &self.batch_options {
                        body = body.push(pick_list(
                            options.runners.clone(),
                            Some(self.draft.runner.clone()),
                            Event::Runner,
                        ));
                    }
                    if self.draft.runner.is_empty() {
                        body = body.push(
                            button("Download latest GE-Proton").on_press(Event::DownloadRunner),
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
                        if options.art_available {
                            body = body.push(
                                checkbox(self.draft.fetch_batch_art)
                                    .label("Fetch art using the top SteamGridDB match")
                                    .on_toggle(Event::FetchBatchArt),
                            );
                        } else {
                            body = body.push(text(&options.art_note).color(palette.muted));
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
                            button("Open its settings")
                                .on_press(Event::OpenExisting(self.draft.name.clone())),
                        );
                    }
                    body = body.push(field(
                        "Executable",
                        &self.draft.executable,
                        Event::Executable,
                    ));
                    body =
                        body.push(button("Browse executable...").on_press(Event::BrowseExecutable));
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
                    body = body.push(button("No arguments").on_press(Event::ArgumentsClear));
                    body = body.push(field(
                        "Working directory",
                        &self.draft.working_directory,
                        Event::WorkingDirectory,
                    ));
                    body = body.push(
                        button("Browse working directory...")
                            .on_press(Event::BrowseWorkingDirectory),
                    );
                    let windows = self
                        .prepared
                        .as_ref()
                        .is_some_and(|prepared| prepared.candidate.kind == HeroicGameKind::Windows);
                    if windows {
                        body = body.push(field("Runner", &self.draft.runner, Event::Runner));
                        if let Some(prepared) = &self.prepared {
                            body = body.push(pick_list(
                                prepared.runners.clone(),
                                Some(self.draft.runner.clone()),
                                Event::Runner,
                            ));
                            if let Some(RunnerSuggestion::Copy(copy)) = &prepared.suggested {
                                body = body.push(
                                    text(format!(
                                        "Heroic has runner {} at {}",
                                        copy.name,
                                        copy.source.display()
                                    ))
                                    .color(palette.muted),
                                );
                                body = body
                                    .push(button("Copy Heroic runner").on_press(Event::CopyRunner));
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
                                button("Download latest GE-Proton").on_press(Event::DownloadRunner),
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
                            .view(palette)
                            .map(move |event| Event::Art(art_generation, event)),
                    );
                }
                body = body.push(button("Next: Review").on_press(Event::Plan));
            }
            Step::Review => {
                if let Some(review) = &self.review {
                    match review {
                        Review::Single {
                            game,
                            missing_build,
                        } => {
                            body = body.push(text(format!("{} -> {}", game.title(), game.name())));
                            match game.as_ref() {
                                Game::Windows(plan) => {
                                    body = body.push(text(format!(
                                        "Runner: {}",
                                        plan.runner().selection()
                                    )));
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
                                    for finding in
                                        plan.findings().iter().filter(|finding| finding.is_active())
                                    {
                                        body = body.push(
                                            text(format!(
                                                "{:?}: {}",
                                                finding.resolution, finding.issue.message
                                            ))
                                            .size(13)
                                            .color(palette.warning),
                                        );
                                    }
                                    for line in heroic_import::runner_dependency_lines(
                                        plan.runner_dependencies(),
                                        true,
                                    ) {
                                        body =
                                            body.push(text(line).size(13).color(palette.warning));
                                    }
                                    for dependency in &plan.inspection().external_dependencies {
                                        body = body.push(
                                            text(format!(
                                                "External link: {} -> {}",
                                                dependency.source.display(),
                                                dependency.target.display()
                                            ))
                                            .size(13)
                                            .color(palette.warning),
                                        );
                                    }
                                    for path in &plan.resolved().external_dependencies {
                                        body = body.push(
                                            text(format!("External game path: {}", path.display()))
                                                .size(13)
                                                .color(palette.warning),
                                        );
                                    }
                                    for shared in plan.shared_prefix() {
                                        body = body.push(
                                            text(format!("Shares prefix with: {}", shared.title))
                                                .size(13)
                                                .color(palette.warning),
                                        );
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
                            for warning in game.warnings() {
                                body = body.push(
                                    text(format!("{warning:?}")).size(13).color(palette.warning),
                                );
                            }
                            if let Some((component, version)) = missing_build {
                                body = body.push(
                                    button(text(format!("Download {} build", component.label())))
                                        .on_press(Event::DownloadBuild(
                                            *component,
                                            version.clone(),
                                        )),
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
                                        format!("Skipped: {} - {:?}", game.title, game.reason)
                                    }
                                };
                                body = body.push(text(line).size(13));
                                if let Entry::Ready(game) = entry {
                                    for warning in game.warnings() {
                                        body = body.push(
                                            text(format!("  {warning:?}"))
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
                                    button(text(format!(
                                        "Download {} for {} game(s)",
                                        offer.component.label(),
                                        offer.games
                                    )))
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
                    body = body
                        .push(button("Import").on_press_maybe(can_import.then_some(Event::Import)));
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
                    body = body.push(button("Retry artwork").on_press(Event::RetryArt));
                    body = body.push(button("Skip artwork").on_press(Event::SkipArt));
                } else if self.token.is_some() {
                    body = body.push(button("Cancel").on_press(Event::Cancel));
                    body = body.push(button("Force stop").on_press(Event::Force));
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
                if self.registered.is_some() {
                    body = body.push(button("Open settings").on_press(Event::OpenSettings));
                }
                body = body.push(button("Back to library").on_press(Event::BackToLibrary));
            }
        }
        let back = button("< Back")
            .on_press_maybe((!self.busy() && self.step != Step::Finished).then_some(Event::Back));
        let page = column![
            row![back].align_y(Center),
            scrollable(
                container(body)
                    .max_width(780)
                    .width(Fill)
                    .padding(24)
                    .center_x(Fill)
            )
            .height(Fill)
        ]
        .spacing(12)
        .width(Fill)
        .height(Fill);
        let mut page: Element<'_, Event> = container(page).width(Fill).height(Fill).into();
        if self.leave_prompt {
            page = container(
                column![
                    text("Discard this Heroic import draft?"),
                    row![
                        button("Keep editing").on_press(Event::KeepEditing),
                        button("Discard").on_press(Event::Discard)
                    ]
                    .spacing(12),
                    page,
                ]
                .spacing(12)
                .padding(24),
            )
            .width(Fill)
            .height(Fill)
            .into();
        }
        page
    }
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
    body = body.push(button("Clear environment variables").on_press(Event::EnvironmentClear));
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
            body = body.push(pick_list(
                options,
                Some(selected.unwrap_or("Default").to_owned()),
                move |value| Event::GamescopeChoice(index, value),
            ));
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

fn kind_label(kind: HeroicGameKind) -> &'static str {
    match kind {
        HeroicGameKind::Windows => "Windows",
        HeroicGameKind::Native => "Native Linux",
        HeroicGameKind::Browser => "browser - unsupported",
        HeroicGameKind::Unknown => "unsupported",
    }
}

fn prepare_game(
    paths: &Paths,
    source: &HeroicSource,
    candidate: HeroicCandidate,
) -> Result<Prepared, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    let runners = Runners::new(paths, &global).map_err(|e| e.to_string())?;
    let installed = runners.list().map_err(|e| e.to_string())?;
    let mut names: Vec<String> = installed
        .runners
        .iter()
        .map(|runner| runner.selection().to_owned())
        .collect();
    if runners.resolve("wine").is_ok() {
        names.insert(0, "wine".to_owned());
    }
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
        hdr,
    ) = if candidate.kind == HeroicGameKind::Windows {
        let imported = source.inspect(&candidate.id).map_err(|e| e.to_string())?;
        let suggestion = heroic_import::suggest_runner(paths, &global, &imported, None)
            .map_err(|e| e.to_string())?;
        let runner = match &suggestion {
            RunnerSuggestion::Use(name) => name.clone(),
            RunnerSuggestion::Copy(_) => String::new(),
            RunnerSuggestion::Choose { proton, .. } => names
                .iter()
                .find(|name| {
                    runners
                        .resolve(name)
                        .is_ok_and(|runner| runner.is_proton() == *proton)
                })
                .cloned()
                .unwrap_or_default(),
        };
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
            imported.enable_hdr,
        )
    } else {
        let imported = source
            .inspect_native(&candidate.id)
            .map_err(|e| e.to_string())?;
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
        arguments,
        unparsed_arguments,
        working_directory,
        environment,
        gamemode,
        wayland,
        mangohud,
        gamescope,
        hdr,
    })
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
        let has_windows = listing
            .candidates
            .iter()
            .any(|candidate| candidate.kind == HeroicGameKind::Windows);
        let runner = if has_windows {
            if draft.runner.trim().is_empty() {
                return Err("Choose or download a runner for Windows games".to_owned());
            }
            Some(draft.runner.as_str())
        } else {
            None
        };
        let mut library = heroic_import::plan_library(
            paths,
            &global,
            listing,
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
    Ok(Review::Single {
        game: Box::new(game),
        missing_build,
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

fn playtime_note(playtime: &Playtime) -> Option<String> {
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
enum Prerequisite {
    CopyRunner(RunnerCopy),
    Download(Provider, Option<String>),
}

impl Prerequisite {
    fn label(&self) -> String {
        match self {
            Self::CopyRunner(copy) => format!("Copying Heroic runner {}...", copy.name),
            Self::Download(provider, _) => format!("Downloading {provider}..."),
        }
    }
}

fn install_prerequisite(
    paths: &Paths,
    operation: Prerequisite,
    token: &Token,
    sender: &UnboundedSender<Event>,
    generation: u64,
) -> Result<String, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    match operation {
        Prerequisite::CopyRunner(copy) => {
            let root = global.runner_root(paths).map_err(|e| e.to_string())?;
            let receipt = import::copy_runner_reviewed(
                &copy.inspection,
                root,
                &copy.name,
                token,
                |progress| {
                    let _ = sender.unbounded_send(Event::Progress(
                        generation,
                        format!("Copying runner: {progress:?}"),
                    ));
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
            client
                .install(paths, &global, &release, token, |progress| {
                    let _ = sender.unbounded_send(Event::Progress(
                        generation,
                        format!("{provider}: {progress:?}"),
                    ));
                })
                .map_err(|e| format!("{e:#}"))?;
            Ok(release.installed_name)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    fn paths(root: &std::path::Path) -> Paths {
        let home = root.to_owned();
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap()
    }

    fn native_fixture(root: &std::path::Path) -> (HeroicSource, PathBuf) {
        let heroic = root.join("heroic");
        fs::create_dir_all(heroic.join("sideload_apps")).unwrap();
        fs::create_dir_all(heroic.join("GamesConfig")).unwrap();
        fs::create_dir_all(heroic.join("store")).unwrap();
        let exe = root.join("games/original");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            heroic.join("sideload_apps/library.json"),
            format!(
                r#"{{"games":[{{"app_name":"dusk","title":"Dusklight","runner":"sideload","install":{{"platform":"linux","executable":"{}"}}}}]}}"#,
                exe.display()
            ),
        )
        .unwrap();
        fs::write(
            heroic.join("GamesConfig/dusk.json"),
            r#"{"dusk":{"launcherArgs":"--old"}}"#,
        )
        .unwrap();
        (
            HeroicSource::from_config_dir_with_home(heroic, root.to_owned()),
            exe,
        )
    }

    #[test]
    fn native_review_edits_are_read_only_until_import() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let (source, _) = native_fixture(root.path());
        let global = GlobalConfig::load(&paths).unwrap();
        let listing = heroic_import::listing(&paths, &global, &source).unwrap();
        let new_exe = root.path().join("games/changed");
        fs::write(&new_exe, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&new_exe, fs::Permissions::from_mode(0o755)).unwrap();
        let before = fs::read(source.config_dir().join("GamesConfig/dusk.json")).unwrap();
        let review = plan_review(
            &paths,
            &listing,
            Some(0),
            &Draft {
                name: "changed".to_owned(),
                executable: new_exe.display().to_string(),
                arguments: vec!["--name".to_owned(), "two words".to_owned()],
                working_directory: root.path().join("games").display().to_string(),
                ..Draft::default()
            },
        )
        .unwrap();
        let Review::Single { game, .. } = review else {
            panic!("expected native plan")
        };
        let Game::Native(plan) = game.as_ref() else {
            panic!("expected native game")
        };
        let target = &plan.settings().targets["game"];
        assert_eq!(target.exe, new_exe.display().to_string());
        assert_eq!(target.args, ["--name", "two words"]);
        assert_eq!(target.cwd.as_deref(), root.path().join("games").to_str());
        assert_eq!(
            fs::read(source.config_dir().join("GamesConfig/dusk.json")).unwrap(),
            before
        );
        assert!(!paths.environment_config("changed").exists());
    }

    #[test]
    fn native_launch_controls_map_to_the_reviewed_plan() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let (source, exe) = native_fixture(root.path());
        let global = GlobalConfig::load(&paths).unwrap();
        let listing = heroic_import::listing(&paths, &global, &source).unwrap();
        let before = fs::read(source.config_dir().join("GamesConfig/dusk.json")).unwrap();
        let mut draft = Draft {
            name: "changed".to_owned(),
            executable: exe.display().to_string(),
            arguments: vec!["--old".to_owned()],
            environment: vec!["NEW=value".to_owned()],
            environment_dirty: true,
            gamemode: true,
            gamemode_dirty: true,
            mangohud: true,
            mangohud_dirty: true,
            gamescope: ScopeDraft::from_setting(Some(GamescopeSetting::On(Gamescope::default()))),
            ..Draft::default()
        };
        draft.gamescope.numbers[0] = "1280".to_owned();
        draft.gamescope.dirty = true;
        let Review::Single { game, .. } = plan_review(&paths, &listing, Some(0), &draft).unwrap()
        else {
            panic!("expected single review");
        };
        let Game::Native(plan) = game.as_ref() else {
            panic!("expected native plan");
        };
        let settings = plan.settings();
        assert_eq!(settings.env["NEW"], "value");
        assert_eq!(settings.gamemode, Some(true));
        assert_eq!(settings.mangohud, Some(true));
        assert_eq!(
            settings
                .gamescope
                .and_then(|setting| setting.on().cloned())
                .unwrap()
                .width,
            Some(1280)
        );
        assert_eq!(
            fs::read(source.config_dir().join("GamesConfig/dusk.json")).unwrap(),
            before
        );
    }

    #[test]
    fn gamescope_toggle_preserves_edited_options() {
        let root = tempdir().unwrap();
        let (mut page, _) = Page::new(paths(root.path()));
        page.draft.gamescope = ScopeDraft::from_setting(Some(GamescopeSetting::On(Gamescope {
            filter: Some(Filter::Fsr),
            grab_cursor: true,
            ..Gamescope::default()
        })));
        let _ = page.update(Event::GamescopeEnabled(false));
        let _ = page.update(Event::GamescopeEnabled(true));
        let scope = page.draft.gamescope.resolved().unwrap().unwrap();
        let scope = scope.on().unwrap();
        assert_eq!(scope.filter, Some(Filter::Fsr));
        assert!(scope.grab_cursor);
    }

    #[test]
    fn stale_discovery_reply_does_not_replace_selected_source() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let (mut page, _) = Page::new(paths);
        let stale = page.generation;
        page.next_generation();
        page.sources
            .push(HeroicSource::from_config_dir(root.path().join("chosen")));
        let _ = page.update(Event::SourcesLoaded(stale, Ok(Vec::new())));
        assert_eq!(page.sources.len(), 1);
    }

    #[test]
    fn cancel_and_back_preserve_review_draft() {
        let root = tempdir().unwrap();
        let (mut page, _) = Page::new(paths(root.path()));
        page.step = Step::Review;
        page.draft.name = "my-game".to_owned();
        let _ = page.back();
        assert_eq!(page.step, Step::Details);
        assert_eq!(page.draft.name, "my-game");
        page.step = Step::Working;
        let token = Token::new();
        page.token = Some(token.clone());
        let _ = page.update(Event::Cancel);
        assert!(token.is_canceled());
        assert!(page.busy());
        let _ = page.update(Event::Force);
        assert!(token.is_forced());
    }

    #[test]
    fn stale_picker_replies_cannot_change_source_or_executable() {
        let root = tempdir().unwrap();
        let (mut page, _) = Page::new(paths(root.path()));
        page.picker_sequence = 4;
        let _ = page.update(Event::PickedSource(3, Ok(Some(root.path().join("stale")))));
        assert!(page.sources.is_empty());
        page.step = Step::Details;
        page.draft.executable = "chosen.exe".to_owned();
        let _ = page.update(Event::PickedExecutable(
            3,
            Ok(Some(root.path().join("stale.exe"))),
        ));
        assert_eq!(page.draft.executable, "chosen.exe");
        let _ = page.update(Event::PickedWorkingDirectory(
            3,
            Ok(Some(root.path().join("stale-dir"))),
        ));
        assert!(page.draft.working_directory.is_empty());
    }

    #[test]
    fn argument_chips_preserve_spaces_quotes_and_empty_values() {
        let arguments = vec![
            "two words".to_owned(),
            "it's ready".to_owned(),
            String::new(),
        ];
        assert_eq!(
            wineo_core::heroic::parse_launcher_args(&quote_args(&arguments)).unwrap(),
            arguments
        );
    }

    #[test]
    fn changed_heroic_settings_refuse_registration() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let (source, exe) = native_fixture(root.path());
        let global = GlobalConfig::load(&paths).unwrap();
        let listing = heroic_import::listing(&paths, &global, &source).unwrap();
        let review = plan_review(
            &paths,
            &listing,
            Some(0),
            &Draft {
                name: "changed".to_owned(),
                executable: exe.display().to_string(),
                arguments: vec!["--old".to_owned()],
                ..Draft::default()
            },
        )
        .unwrap();
        let Review::Single { game, .. } = review else {
            panic!("expected single plan")
        };
        let settings = source.config_dir().join("GamesConfig/dusk.json");
        fs::write(&settings, r#"{"dusk":{"launcherArgs":"--new"}}"#).unwrap();
        let error = apply_game(&paths, &global, &source, &game, &Token::new(), |_| {}).unwrap_err();
        assert!(error.is::<heroic_import::ApplyError>());
        assert!(!paths.environment_config("changed").exists());
        assert_eq!(
            fs::read_to_string(settings).unwrap(),
            r#"{"dusk":{"launcherArgs":"--new"}}"#
        );
    }
}
