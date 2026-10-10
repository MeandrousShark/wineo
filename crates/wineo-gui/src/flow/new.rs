//! Guided creation of Windows and native environments.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{
    Space, button, checkbox, column, container, pick_list, radio, responsive, row, scrollable,
    text, text_input,
};
use iced::{Center, Element, Fill, Shrink, Task};
use wineo_core::cancel::Token;
use wineo_core::config::{Arch, GamescopeSetting, GlobalConfig};
use wineo_core::create::{self, CreateEvent, CreateRequest, TargetOutcome};
use wineo_core::desktop;
use wineo_core::download::{Client, Provider};
use wineo_core::environment::{Environment, NewEnvironment};
use wineo_core::error::Error as CoreError;
use wineo_core::install::{self, Wanted};
use wineo_core::installer::{self, Installer, Outcome, Output};
use wineo_core::launch;
use wineo_core::name;
use wineo_core::paths::Paths;
use wineo_core::programs;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;
use wineo_core::runtime::Runtime;
use wineo_core::supervisor::{Notice, Watched};
use wineo_core::targets::{self, Batch, Spec, Written};
use wineo_core::templates::{Template, Templates};

use super::{Action, artwork};
use crate::logview::{self, LogView};
use crate::picker;
use crate::runner_label::{self, Choice};
use crate::theme::Palette;
use crate::ui::{self, PickListStyle};

const LOG_ID: &str = "install-log";
const LOG_CAP: usize = 128 * 1024;
const LOG_KEEP: usize = 64 * 1024;

/// Poster plus a form wide enough for the Name, Icon, and Type row. Below
/// this the poster and icon move under the form.
const SIDE_POSTER: f32 = artwork::POSTER_WIDTH as f32 + 16.0 + 400.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Windows,
    Native,
}

impl Kind {
    const ALL: [Self; 2] = [Self::Windows, Self::Native];
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Windows => "Windows",
            Self::Native => "Native Linux",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Empty,
    Executable,
    Installer,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Empty => "Empty registration",
            Self::Executable => "Existing install",
            Self::Installer => "Run an installer",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Details,
    Review,
    Working,
    Target,
    Finished,
}

#[derive(Debug, Clone)]
pub(crate) struct Options {
    runner: String,
    templates: Vec<String>,
    runners: Vec<Choice>,
    /// Whether to ask about other launchers' builds for a game.
    offer: bool,
    arch: Arch,
    prefix_root: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct Review {
    runner: Option<Runner>,
    /// The runner as the review shows it, worked out on the worker since
    /// naming its launcher reads the disk.
    runner_row: Option<String>,
    template: Option<Template>,
    arch: Arch,
    prefix: Option<PathBuf>,
    adopted: bool,
    omitted_components: bool,
    prerequisite: Option<Prerequisite>,
}

#[derive(Debug, Clone)]
enum Prerequisite {
    Runner(String),
    Component {
        provider: Provider,
        version: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Report {
    registered: bool,
    playable: bool,
    cleanup_resolved: bool,
    notes: Vec<String>,
    programs: Vec<(String, String)>,
    art_error: Option<String>,
}

impl Report {
    fn new() -> Self {
        Self {
            registered: false,
            playable: false,
            cleanup_resolved: true,
            notes: Vec::new(),
            programs: Vec::new(),
            art_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Draft {
    /// The free-text game title. The environment name derives from it.
    title: String,
    /// The user's own environment name. None follows the title, and clearing
    /// the text resumes following it.
    environment: Option<String>,
    kind: Kind,
    source: Source,
    path: String,
    prefix: String,
    runner: String,
    template: String,
    arch: Option<Arch>,
    desktop: bool,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            title: String::new(),
            environment: None,
            kind: Kind::Windows,
            source: Source::Empty,
            path: String::new(),
            prefix: String::new(),
            runner: String::new(),
            template: String::new(),
            arch: None,
            desktop: false,
        }
    }
}

impl Draft {
    fn derived_environment(&self) -> String {
        name::from_title(&self.title)
    }

    /// The name used for the environment, its files, and its config.
    fn environment_name(&self) -> String {
        match self.environment.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => self.derived_environment(),
        }
    }

    /// What the desktop entry is called, when the title says anything.
    fn target_title(&self) -> Option<String> {
        let title = self.title.trim();
        (!title.is_empty()).then(|| title.to_owned())
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    OpenExisting,
    Discard,
    KeepEditing,
    Kind(Kind),
    Source(Source),
    Name(String),
    EditEnvironment,
    EnvironmentName(String),
    Path(String),
    Prefix(String),
    Runner(String),
    Template(String),
    ToggleAdditionalWineOptions,
    Arch(Arch),
    Desktop(bool),
    Browse,
    BrowsePrefix,
    PrefixPicked(u64, Result<Option<PathBuf>, String>),
    Picked(u64, Result<Option<PathBuf>, String>),
    Next,
    Create,
    Download(Provider),
    Downloaded(u64, Result<String, String>),
    Cancel,
    Force,
    OpenSettings,
    ToLibrary,
    Launch(bool),
    TargetPath(String),
    TargetDesktop(bool),
    Program(String),
    AddTarget,
    SkipTarget,
    CleanUp,
    Cleaned(u64, Result<bool, String>),
    RetryArt,
    RetryCreation,
    SkipArt,
    ArtApplied(u64, Result<Vec<String>, String>),
    TargetAdded(u64, Result<Vec<String>, String>),
    Options(Result<Options, String>),
    /// The answer to using other launchers' builds.
    Offer(bool),
    Offered(u64, Result<Options, String>),
    Reviewed(u64, Result<Box<Review>, String>),
    Progress(u64, String),
    LogChunk(u64, String),
    LogAction(iced::widget::text_editor::Action),
    LogScrolled(iced::widget::scrollable::Viewport),
    LogFollow,
    CopyLog,
    Done(u64, Report),
    Art(artwork::Event),
}

pub(crate) struct Page {
    paths: Paths,
    draft: Draft,
    art: artwork::Draft,
    stage: Stage,
    options: Option<Options>,
    template_preview: Option<Template>,
    additional_wine_options: bool,
    editing_environment: bool,
    review: Option<Review>,
    report: Option<Report>,
    error: Option<String>,
    status: String,
    log: String,
    log_view: LogView,
    target_path: String,
    target_desktop: bool,
    launch: bool,
    leave_prompt: bool,
    action: Option<Action>,
    generation: u64,
    token: Option<Token>,
    downloading: Option<Provider>,
    picker_sequence: u64,
    /// The generation of the review being prepared, if one is.
    reviewing: Option<u64>,
    /// Set while an answer to the offer is being saved.
    answering: bool,
    offer_sequence: u64,
}

impl Page {
    pub(crate) fn new(paths: Paths) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            draft: Draft::default(),
            art: artwork::Draft::new(),
            stage: Stage::Details,
            options: None,
            template_preview: None,
            additional_wine_options: false,
            editing_environment: false,
            review: None,
            report: None,
            error: None,
            status: String::new(),
            log: String::new(),
            log_view: LogView::default(),
            target_path: String::new(),
            target_desktop: false,
            launch: false,
            leave_prompt: false,
            action: None,
            generation: 0,
            token: None,
            downloading: None,
            picker_sequence: 0,
            reviewing: None,
            answering: false,
            offer_sequence: 0,
        };
        let task = Task::batch([
            page.load_options(),
            page.art
                .configure(page.paths.clone(), String::new())
                .map(Event::Art),
        ]);
        (page, task)
    }

    fn load_options(&mut self) -> Task<Event> {
        self.generation = self.generation.wrapping_add(1);
        let paths = self.paths.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || read_options(&paths))
                    .await
                    .map_err(|error| error.to_string())?
            },
            Event::Options,
        )
    }

    /// The question shows for a game, which is where Steam, Heroic, and
    /// Lutris builds matter, until it's answered.
    fn offer_shown(&self) -> bool {
        self.stage == Stage::Details
            && self.draft.kind == Kind::Windows
            && self.options.as_ref().is_some_and(|options| options.offer)
            && self
                .template_preview
                .as_ref()
                .is_some_and(|template| template.tags.iter().any(|tag| tag == "game"))
    }

    #[must_use]
    fn append_log(&mut self, text: &str) -> bool {
        let text = logview::normalize(text);
        self.log.push_str(&text);
        let mut dropped = String::new();
        if self.log.len() > LOG_CAP {
            let mut start = self.log.len() - LOG_KEEP;
            while !self.log.is_char_boundary(start) {
                start += 1;
            }
            dropped = self.log.drain(..start).collect();
        }
        let log = &self.log;
        self.log_view.push(&text, &dropped, || log.clone())
    }

    pub(crate) fn busy(&self) -> bool {
        self.stage == Stage::Working
    }

    #[cfg(test)]
    pub(crate) fn art_fetching(&self) -> bool {
        self.art.is_fetching()
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        if let Some(token) = &self.token {
            if force {
                token.force();
                self.status = "Force stopping...".to_owned();
            } else {
                token.cancel();
                self.status = "Canceling and waiting for cleanup...".to_owned();
            }
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
        match self.stage {
            Stage::Details => {
                if self.draft == Draft::default() && !self.art.is_dirty() {
                    self.action = Some(Action::Choose);
                } else {
                    self.leave_prompt = true;
                }
            }
            Stage::Review => self.stage = Stage::Details,
            Stage::Working => self.cancel(false),
            Stage::Target => self.finish(),
            Stage::Finished => self.leave(false),
        }
        Task::none()
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        if self.busy() || self.leave_prompt {
            return Task::none();
        }
        match self.stage {
            Stage::Details => self.update(Event::Next),
            Stage::Review => self.update(Event::Create),
            Stage::Target => self.update(Event::AddTarget),
            Stage::Finished => self.update(Event::ToLibrary),
            Stage::Working => Task::none(),
        }
    }

    fn edited(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
        self.error = None;
        self.review = None;
    }

    fn additional_wine_options_visible(&self) -> bool {
        self.draft.kind == Kind::Windows && self.additional_wine_options
    }

    fn validate(&self) -> Result<(), String> {
        let name = self.draft.environment_name();
        if name.is_empty() {
            return Err("Name needs letters or digits.".to_owned());
        }
        if let Some(problem) = name::problem(&name) {
            return Err(problem);
        }
        if name.len() > name::TITLE_MAX {
            return Err("Name is too long.".to_owned());
        }
        if self.paths.environment_config(&name).exists() {
            return Err(format!(
                "An environment named {name} already exists. Open its Settings from the library."
            ));
        }
        if self.draft.source != Source::Empty && self.draft.path.trim().is_empty() {
            return Err("Choose an executable or installer".to_owned());
        }
        if self.draft.kind == Kind::Native && self.draft.source == Source::Installer {
            return Err("Native environments cannot run a Windows installer".to_owned());
        }
        self.art.validate()
    }

    fn review_task(&mut self) -> Task<Event> {
        if let Err(error) = self.validate() {
            self.error = Some(error);
            return Task::none();
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let draft = self.draft.clone();
        self.reviewing = Some(generation);
        self.status = "Checking creation choices...".to_owned();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || prepare(&paths, &draft))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |result| Event::Reviewed(generation, result.map(Box::new)),
        )
    }

    fn start(&mut self) -> Task<Event> {
        if let Err(error) = self.validate() {
            self.error = Some(error);
            return Task::none();
        }
        let Some(review) = self.review.clone() else {
            return Task::none();
        };
        if review.prerequisite.is_some() {
            return Task::none();
        }
        if self.busy() {
            return Task::none();
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let draft = self.draft.clone();
        let art = self.art.clone();
        let token = Token::new();
        self.token = Some(token.clone());
        self.stage = Stage::Working;
        self.status = "Creating environment...".to_owned();
        self.log.clear();
        self.log_view = LogView::default();
        let (sender, receiver) = unbounded();
        let log_sender = sender.clone();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    perform(
                        &paths,
                        draft,
                        review,
                        art,
                        token,
                        |line| {
                            let _ = sender.unbounded_send(Event::Progress(generation, line));
                        },
                        |chunk| {
                            let _ = log_sender.unbounded_send(Event::LogChunk(generation, chunk));
                        },
                    )
                })
                .await
                .unwrap_or_else(|error| {
                    let mut report = Report::new();
                    report.notes.push(error.to_string());
                    report
                })
            },
            move |report| Event::Done(generation, report),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn download(&mut self, provider: Provider) -> Task<Event> {
        let Some(prerequisite) = self
            .review
            .as_ref()
            .and_then(|review| review.prerequisite.clone())
        else {
            return Task::none();
        };
        let valid = match &prerequisite {
            Prerequisite::Runner(name) => match name.as_str() {
                "GE-Proton-Latest" => provider == Provider::GeProton,
                "Proton-CachyOS-Latest" => provider == Provider::ProtonCachyos,
                _ => matches!(provider, Provider::GeProton | Provider::ProtonCachyos),
            },
            Prerequisite::Component {
                provider: expected, ..
            } => *expected == provider,
        };
        if !valid {
            return Task::none();
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let token = Token::new();
        self.token = Some(token.clone());
        self.downloading = Some(provider);
        self.stage = Stage::Working;
        self.status = format!("Finding {provider} release...");
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let client = Client::new().map_err(|e| e.to_string())?;
                    let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                    let release = match prerequisite {
                        Prerequisite::Component {
                            version: Some(version),
                            ..
                        } => client.select_pinned(provider, &version),
                        _ => client.select(provider, None),
                    }
                    .map_err(|e| e.to_string())?;
                    let name = release.installed_name.clone();
                    let _ = sender.unbounded_send(Event::Progress(
                        generation,
                        format!("Downloading {} ({})", release.tag, release.verification()),
                    ));
                    crate::download_progress::install(
                        &client,
                        &paths,
                        &global,
                        &release,
                        &token,
                        Duration::from_millis(250),
                        |line| {
                            let _ = sender.unbounded_send(Event::Progress(generation, line));
                        },
                    )
                    .map_err(|e| e.to_string())?;
                    Ok::<_, String>(name)
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |result| Event::Downloaded(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn finish(&mut self) {
        self.stage = Stage::Finished;
        self.token = None;
    }

    fn leave(&mut self, settings: bool) {
        let name = self.draft.environment_name();
        let registered = self.report.as_ref().is_some_and(|report| report.registered);
        let launch = self.launch
            && self
                .report
                .as_ref()
                .is_some_and(|report| report.playable && report.cleanup_resolved);
        self.action = Some(if settings && registered {
            Action::Settings { name, launch }
        } else {
            Action::Library {
                name: registered.then_some(name),
                launch,
            }
        });
    }

    fn add_target(&mut self) -> Task<Event> {
        let exe = self.target_path.trim().to_owned();
        if exe.is_empty() {
            self.error = Some("Choose a game executable".to_owned());
            return Task::none();
        }
        let paths = self.paths.clone();
        let name = self.draft.environment_name();
        let title = self.draft.target_title();
        let desktop = self.target_desktop;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.stage = Stage::Working;
        self.status = "Adding game target...".to_owned();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let environment =
                        Environment::load(&paths, &name).map_err(|e| e.to_string())?;
                    let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                    if let Some(wine) = environment.config.wine() {
                        let runner = Runners::new(&paths, &global)
                            .and_then(|runners| runners.resolve(&wine.runner))
                            .map_err(|e| e.to_string())?;
                        let prefix = environment.prefix(&paths).map_err(|e| e.to_string())?;
                        desktop::check(&paths, &runner.wine_prefix(&prefix), &exe)
                            .map_err(|e| e.to_string())?;
                    } else {
                        launch::check_native_exe(&paths, &global, &name, &Default::default(), &exe)
                            .map_err(|e| e.to_string())?;
                    }
                    let mut batch = Batch::new(environment.config.targets);
                    batch
                        .add(&name, target_spec(exe, title, desktop))
                        .map_err(|e| e.to_string())?;
                    batch.set_default(Some(&name));
                    match targets::write(&paths, &name, &batch).map_err(|e| e.to_string())? {
                        Written::Saved => {}
                        Written::Changed(_) => {
                            return Err("Targets changed on disk. Open Settings and review them."
                                .to_owned());
                        }
                    }
                    let environment =
                        Environment::load(&paths, &name).map_err(|e| e.to_string())?;
                    let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                    let note = if let Some(wine) = environment.config.wine() {
                        let runner = Runners::new(&paths, &global)
                            .and_then(|runners| runners.resolve(&wine.runner))
                            .map_err(|e| e.to_string())?;
                        let prefix = environment.prefix(&paths).map_err(|e| e.to_string())?;
                        desktop::reconcile(&paths, &global, &environment, Some(&runner), &prefix)
                            .err()
                            .map(|error| format!("Desktop entry: {error}"))
                    } else {
                        desktop::reconcile_native(&paths, &global, &environment)
                            .err()
                            .map(|error| format!("Desktop entry: {error}"))
                    };
                    Ok::<_, String>(note.into_iter().collect())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |result| Event::TargetAdded(generation, result),
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
        if self.leave_prompt && !matches!(event, Event::KeepEditing | Event::Discard) {
            return Task::none();
        }
        match event {
            Event::Back => return self.back(),
            Event::OpenExisting if self.stage == Stage::Details => {
                let name = self.draft.environment_name();
                if self.paths.environment_config(&name).exists() {
                    self.action = Some(Action::Settings {
                        name,
                        launch: false,
                    });
                }
            }
            Event::Discard => {
                self.leave_prompt = false;
                self.action = Some(Action::Choose);
            }
            Event::KeepEditing => self.leave_prompt = false,
            Event::Kind(kind) if self.stage == Stage::Details => {
                self.draft.kind = kind;
                if kind == Kind::Native && self.draft.source == Source::Installer {
                    self.draft.source = Source::Empty;
                }
                self.edited();
            }
            Event::Source(value) if self.stage == Stage::Details => {
                self.draft.source = match value {
                    Source::Installer if self.draft.kind != Kind::Windows => Source::Empty,
                    value => value,
                };
                self.edited();
            }
            Event::Name(value) if self.stage == Stage::Details => {
                self.draft.title = value;
                self.edited();
                return self.art.set_title(&self.draft.title).map(Event::Art);
            }
            Event::EditEnvironment if self.stage == Stage::Details => {
                self.editing_environment = true;
            }
            Event::EnvironmentName(value) if self.stage == Stage::Details => {
                self.draft.environment = Some(value);
                self.edited();
            }
            Event::Path(value) if self.stage == Stage::Details => {
                self.picker_sequence = self.picker_sequence.wrapping_add(1);
                self.draft.path = value;
                self.edited();
            }
            Event::Prefix(value) if self.stage == Stage::Details => {
                self.draft.prefix = value;
                self.edited();
            }
            Event::BrowsePrefix if self.stage == Stage::Details => {
                self.picker_sequence = self.picker_sequence.wrapping_add(1);
                let sequence = self.picker_sequence;
                return Task::perform(
                    picker::pick("Select prefix folder".to_owned(), true),
                    move |result| Event::PrefixPicked(sequence, result),
                );
            }
            Event::PrefixPicked(sequence, result)
                if sequence == self.picker_sequence && self.stage == Stage::Details =>
            {
                match result {
                    Ok(Some(path)) => {
                        self.draft.prefix = path.to_string_lossy().into_owned();
                        self.edited();
                    }
                    Ok(None) => {}
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Runner(value) if self.stage == Stage::Details => {
                self.draft.runner = value;
                self.edited();
            }
            Event::Template(value) if self.stage == Stage::Details => {
                self.draft.template = value;
                self.edited();
                self.template_preview = if self.draft.template.is_empty() {
                    None
                } else {
                    match Templates::new(&self.paths).resolve(&self.draft.template) {
                        Ok(template) => Some(template),
                        Err(error) => {
                            self.error = Some(error.to_string());
                            None
                        }
                    }
                };
            }
            Event::ToggleAdditionalWineOptions if self.stage == Stage::Details => {
                self.additional_wine_options = !self.additional_wine_options;
            }
            Event::Arch(value) if self.stage == Stage::Details => {
                self.draft.arch = Some(value);
                self.edited();
            }
            Event::Desktop(value) if self.stage == Stage::Details => {
                self.draft.desktop = value;
                self.edited();
            }
            Event::Browse if matches!(self.stage, Stage::Details | Stage::Target) => {
                self.picker_sequence = self.picker_sequence.wrapping_add(1);
                let sequence = self.picker_sequence;
                return Task::perform(
                    picker::pick("Choose program".to_owned(), false),
                    move |result| Event::Picked(sequence, result),
                );
            }
            Event::Picked(sequence, result) if sequence == self.picker_sequence => match result {
                Ok(Some(path)) => {
                    if self.stage == Stage::Target {
                        self.target_path = path.display().to_string();
                    } else if self.stage == Stage::Details {
                        self.draft.path = path.display().to_string();
                        self.edited();
                    }
                }
                Ok(None) => {}
                Err(error) => self.error = Some(error),
            },
            Event::Next if self.stage == Stage::Details => return self.review_task(),
            Event::RetryCreation
                if self.stage == Stage::Finished
                    && self
                        .report
                        .as_ref()
                        .is_some_and(|report| !report.registered) =>
            {
                self.stage = Stage::Details;
                self.report = None;
                self.edited();
                return self.review_task();
            }
            Event::Reviewed(generation, result)
                if generation == self.generation && self.stage == Stage::Details =>
            {
                self.reviewing = None;
                self.status.clear();
                match result {
                    Ok(review) => {
                        self.error = None;
                        self.review = Some(*review);
                        self.stage = Stage::Review;
                        return self
                            .art
                            .configure(self.paths.clone(), self.draft.title.clone())
                            .map(Event::Art);
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Create if self.stage == Stage::Review => return self.start(),
            Event::Download(provider) if self.stage == Stage::Review => {
                return self.download(provider);
            }
            Event::Downloaded(generation, result)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                self.token = None;
                self.downloading = None;
                match result {
                    Ok(name) => {
                        if self.review.as_ref().is_some_and(|review| {
                            matches!(&review.prerequisite, Some(Prerequisite::Runner(runner))
                                if runner != "GE-Proton-Latest" && runner != "Proton-CachyOS-Latest")
                        }) {
                            self.draft.runner = name.clone();
                        }
                        self.status = format!("Installed {name}. Checking choices again...");
                        self.stage = Stage::Details;
                        return self.review_task();
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.stage = Stage::Review;
                    }
                }
            }
            Event::Progress(generation, line)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                self.status.clone_from(&line);
                if self.append_log(&format!("{line}\n")) {
                    return snap_log();
                }
            }
            Event::LogChunk(generation, chunk)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                if self.append_log(&chunk) {
                    return snap_log();
                }
            }
            Event::LogAction(action) => {
                let log = &self.log;
                if self.log_view.perform(action, || log.clone()) {
                    return snap_log();
                }
            }
            Event::LogScrolled(viewport) => {
                let log = &self.log;
                let snap = self.log_view.scrolled(
                    viewport.absolute_offset().y,
                    viewport.bounds().height,
                    viewport.content_bounds().height,
                    || log.clone(),
                );
                if snap {
                    return snap_log();
                }
            }
            Event::LogFollow => {
                let log = &self.log;
                if self.log_view.follow(|| log.clone()) {
                    return snap_log();
                }
            }
            Event::CopyLog => {
                self.log_view.mark_copied();
                return iced::clipboard::write(self.log.clone());
            }
            Event::Done(generation, report)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                let can_select = report.registered
                    && report.cleanup_resolved
                    && self.draft.source == Source::Installer;
                self.report = Some(report);
                self.token = None;
                if can_select {
                    self.stage = Stage::Target;
                } else {
                    self.finish();
                }
            }
            Event::Cancel if self.busy() => self.cancel(false),
            Event::Force if self.busy() => self.cancel(true),
            Event::TargetPath(value) if self.stage == Stage::Target => {
                self.picker_sequence = self.picker_sequence.wrapping_add(1);
                self.target_path = value;
            }
            Event::TargetDesktop(value) if self.stage == Stage::Target => {
                self.target_desktop = value
            }
            Event::Program(exe) if self.stage == Stage::Target => self.target_path = exe,
            Event::AddTarget if self.stage == Stage::Target => return self.add_target(),
            Event::SkipTarget if self.stage == Stage::Target => self.finish(),
            Event::TargetAdded(generation, result)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                match result {
                    Ok(notes) => {
                        if let Some(report) = &mut self.report {
                            report.playable = true;
                            report.notes.extend(notes);
                        }
                        self.finish();
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.stage = Stage::Target;
                    }
                }
            }
            Event::ArtApplied(generation, result)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                match result {
                    Ok(notes) => {
                        if let Some(report) = &mut self.report {
                            report.notes.extend(notes);
                            report.art_error = None;
                        }
                    }
                    Err(error) => {
                        if let Some(report) = &mut self.report {
                            report.art_error = Some(error);
                        }
                    }
                }
                self.finish();
            }
            Event::CleanUp
                if self.stage == Stage::Finished
                    && self.report.as_ref().is_some_and(|r| !r.cleanup_resolved) =>
            {
                self.generation = self.generation.wrapping_add(1);
                let generation = self.generation;
                let paths = self.paths.clone();
                let name = self.draft.environment_name();
                self.stage = Stage::Working;
                self.status = "Cleaning up installer...".to_owned();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                            let outcome = installer::clean_up(
                                &paths,
                                &global,
                                &name,
                                installer::clean_up_timeout(&global),
                            )
                            .map_err(|e| e.to_string())?;
                            Ok::<_, String>(!matches!(
                                outcome,
                                Some(
                                    installer::Cleaned::Unresolved | installer::Cleaned::Untracked
                                )
                            ))
                        })
                        .await
                        .map_err(|e| e.to_string())?
                    },
                    move |result| Event::Cleaned(generation, result),
                );
            }
            Event::Cleaned(generation, result)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                match result {
                    Ok(true) => {
                        if let Some(report) = &mut self.report {
                            report.cleanup_resolved = true;
                            report.notes.push("Installer cleaned up".to_owned());
                        }
                        if self.draft.source == Source::Installer {
                            self.stage = Stage::Target;
                        } else {
                            self.finish();
                        }
                    }
                    Ok(false) => {
                        self.error = Some("Installer cleanup is still unresolved".to_owned());
                        self.finish();
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.finish();
                    }
                }
            }
            Event::SkipArt if self.stage == Stage::Finished => {
                if let Some(report) = &mut self.report
                    && report.art_error.take().is_some()
                {
                    report.notes.push("Artwork skipped".to_owned());
                }
            }
            Event::RetryArt
                if self.stage == Stage::Finished
                    && self.report.as_ref().is_some_and(|r| r.art_error.is_some()) =>
            {
                self.generation = self.generation.wrapping_add(1);
                let generation = self.generation;
                let paths = self.paths.clone();
                let name = self.draft.environment_name();
                let art = self.art.clone();
                self.stage = Stage::Working;
                self.status = "Applying artwork...".to_owned();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let result = art.apply(&paths, &name);
                            let desktop = desktop::apply(&paths, &name, false)
                                .map_err(|error| error.to_string());
                            match (result, desktop) {
                                (Ok(notes), Ok(_)) => Ok(notes),
                                (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
                                (Err(art_error), Err(desktop_error)) => {
                                    Err(format!("{art_error}; desktop entry: {desktop_error}"))
                                }
                            }
                        })
                        .await
                        .map_err(|e| e.to_string())?
                    },
                    move |result| Event::ArtApplied(generation, result),
                );
            }
            Event::Launch(value) if self.stage == Stage::Finished => self.launch = value,
            Event::OpenSettings if self.stage == Stage::Finished => self.leave(true),
            Event::ToLibrary if self.stage == Stage::Finished => self.leave(false),
            Event::Art(event) if matches!(self.stage, Stage::Details | Stage::Review) => {
                return self
                    .art
                    .update_with_key_availability(event, key_available)
                    .map(Event::Art);
            }
            Event::Options(result) => match result {
                Ok(options) => {
                    self.options = Some(options);
                }
                Err(error) => self.error = Some(error),
            },
            Event::Offer(on) if self.offer_shown() && !self.answering => {
                self.answering = true;
                self.offer_sequence = self.offer_sequence.wrapping_add(1);
                let sequence = self.offer_sequence;
                let paths = self.paths.clone();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || answer_offer(&paths, on))
                            .await
                            .map_err(|error| error.to_string())?
                    },
                    move |result| Event::Offered(sequence, result),
                );
            }
            Event::Offered(sequence, result) if sequence == self.offer_sequence => {
                self.answering = false;
                match result {
                    Ok(options) => {
                        self.options = Some(options);
                        // A review started before the answer was saved may
                        // have read the old setting, so check it again. An
                        // open review shows what Create does and stays.
                        if self.stage == Stage::Details && self.reviewing == Some(self.generation) {
                            return self.review_task();
                        }
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn details_form<'a>(
        &'a self,
        icon: Option<Element<'a, Event>>,
        palette: Palette,
    ) -> Element<'a, Event> {
        let mut form = column![].spacing(10).width(Fill);
        let environment = self.draft.environment_name();
        let mut name_field = column![
            text("Name"),
            text_input("Game name", &self.draft.title).on_input(Event::Name)
        ]
        .spacing(4)
        .width(Fill);
        let mut derived = row![
            text(format!(
                "Environment: {}",
                if environment.is_empty() {
                    "(none yet)"
                } else {
                    &environment
                }
            ))
            .size(12)
            .color(palette.muted)
        ]
        .spacing(8);
        if !self.editing_environment {
            derived = derived.push(
                button(text("Edit").size(12))
                    .padding(0)
                    .on_press(Event::EditEnvironment)
                    .style(move |_, status| button::Style {
                        text_color: if matches!(status, button::Status::Hovered) {
                            palette.foreground
                        } else {
                            palette.primary
                        },
                        ..button::Style::default()
                    }),
            );
        }
        // A long environment name in a narrow dialog would squeeze Edit away.
        name_field = name_field.push(derived.wrap());
        if self.editing_environment {
            name_field = name_field.push(text("Environment name")).push(
                text_input(
                    "Environment name",
                    self.draft
                        .environment
                        .as_deref()
                        .unwrap_or(&self.draft.derived_environment()),
                )
                .on_input(Event::EnvironmentName),
            );
        }
        form = form.push(
            row![name_field]
                .push(icon)
                .push(
                    column![
                        text("Type"),
                        pick_list(Kind::ALL, Some(self.draft.kind), Event::Kind)
                            .wineo_style(palette)
                    ]
                    .spacing(4)
                    .width(180),
                )
                .spacing(12),
        );
        if !environment.is_empty() && self.paths.environment_config(&environment).exists() {
            form = form.push(
                ui::secondary_button("Open existing environment settings", palette)
                    .on_press(Event::OpenExisting),
            );
        }
        let sources = if self.draft.kind == Kind::Windows {
            vec![Source::Installer, Source::Executable, Source::Empty]
        } else {
            vec![Source::Executable, Source::Empty]
        };
        let mut choices = column![text("Starting point")].spacing(4);
        for source in sources {
            choices = choices.push(radio(
                source.to_string(),
                source,
                Some(self.draft.source),
                Event::Source,
            ));
        }
        form = form.push(choices);
        if self.draft.source != Source::Empty {
            let hint = if self.draft.source == Source::Installer {
                "Installer path"
            } else {
                "Executable path"
            };
            form = form.push(
                row![
                    text_input(hint, &self.draft.path).on_input(Event::Path),
                    ui::secondary_button("Browse...", palette).on_press(Event::Browse),
                ]
                .spacing(8),
            );
            if self.draft.source == Source::Executable {
                form = form.push(
                    checkbox(self.draft.desktop)
                        .label("Create desktop entry")
                        .on_toggle(Event::Desktop),
                );
            }
        }
        if self.draft.kind == Kind::Windows {
            form = form.push(
                column![
                    text("Template"),
                    pick_list(
                        std::iter::once("None".to_owned())
                            .chain(
                                self.options
                                    .as_ref()
                                    .into_iter()
                                    .flat_map(|o| o.templates.iter().cloned()),
                            )
                            .collect::<Vec<_>>(),
                        Some(if self.draft.template.is_empty() {
                            "None".to_owned()
                        } else {
                            self.draft.template.clone()
                        }),
                        |value| {
                            Event::Template(if value == "None" {
                                String::new()
                            } else {
                                value
                            })
                        },
                    )
                    .wineo_style(palette)
                ]
                .spacing(4),
            );
            if self.offer_shown() {
                form = form.push(runner_label::offer_card(
                    palette,
                    !self.answering,
                    Event::Offer,
                ));
            }
            if let Some(template) = &self.template_preview {
                let effects = template_effects(template, &self.draft);
                let mut summary = column![text("This template sets:").size(12)].spacing(2);
                for effect in effects {
                    summary = summary.push(text(effect).size(12));
                }
                form = form.push(summary);
            }
            form = form.push(
                ui::secondary_button(
                    if self.additional_wine_options_visible() {
                        "Additional wine options (hide)"
                    } else {
                        "Additional wine options"
                    },
                    palette,
                )
                .on_press(Event::ToggleAdditionalWineOptions),
            );
            if self.additional_wine_options_visible() {
                let mut options_body = column![
                    text("Runner override"),
                    text_input("Use template or global default", &self.draft.runner)
                        .on_input(Event::Runner)
                ]
                .spacing(6);
                if let Some(options) = &self.options {
                    options_body = options_body.push(
                        pick_list(
                            std::iter::once(Pick::Default)
                                .chain(options.runners.iter().cloned().map(Pick::Runner))
                                .collect::<Vec<_>>(),
                            Some(if self.draft.runner.is_empty() {
                                Pick::Default
                            } else {
                                Pick::Runner(Choice::find(&options.runners, &self.draft.runner))
                            }),
                            |pick| {
                                Event::Runner(match pick {
                                    Pick::Default => String::new(),
                                    Pick::Runner(choice) => choice.name,
                                })
                            },
                        )
                        .wineo_style(palette)
                        .width(Fill),
                    );
                    options_body = options_body.push(
                        text(format!(
                            "Global default: {} ({})",
                            options.runner, options.arch
                        ))
                        .size(12)
                        .color(palette.muted),
                    );
                }
                options_body = options_body.push(text("Architecture override")).push(
                    pick_list([Arch::Win64, Arch::Win32], self.draft.arch, Event::Arch)
                        .placeholder("Use template or global default")
                        .wineo_style(palette),
                );
                options_body = options_body.push(text("Prefix folder override")).push(
                    row![
                        text_input("Use default path", &self.draft.prefix).on_input(Event::Prefix),
                        ui::secondary_button("Browse...", palette).on_press(Event::BrowsePrefix)
                    ]
                    .spacing(8),
                );
                if self.draft.prefix.is_empty()
                    && let Some(options) = &self.options
                {
                    options_body = options_body.push(
                        text(format!(
                            "Default: {}",
                            options.prefix_root.join(&environment).display()
                        ))
                        .size(12)
                        .color(palette.muted),
                    );
                }
                form = form.push(container(options_body).padding(10));
            }
        }
        form.into()
    }

    pub(crate) fn view_with_key_availability(
        &self,
        palette: Palette,
        key_available: bool,
    ) -> Element<'_, Event> {
        let mut review_footer: Option<Element<'_, Event>> = None;
        let heading = ui::page_title(
            match self.stage {
                Stage::Details => "New environment",
                Stage::Review => "Review new environment",
                Stage::Working if self.downloading.is_some() => "Downloading prerequisite",
                Stage::Working => "Creating environment",
                Stage::Target => "Choose game target",
                Stage::Finished => "Environment created",
            },
            palette,
        );
        let mut body = column![heading].spacing(10).width(Fill);
        match self.stage {
            Stage::Details => {
                body = body.push(self.art.banner(palette).map(Event::Art));
                if let Some(status) = self.art.status_line(palette) {
                    body = body.push(status.map(Event::Art));
                }
                body = body.push(
                    responsive(move |size| {
                        let poster = self.art.poster(palette).map(Event::Art);
                        let icon = self.art.icon(palette).map(Event::Art);
                        let controls = self.art.controls(palette, key_available).map(Event::Art);
                        let navigation = row![
                            ui::secondary_button("Back", palette).on_press(Event::Back),
                            ui::accent_base("Next", palette.primary).on_press(Event::Next)
                        ]
                        .spacing(8);
                        if size.width >= SIDE_POSTER {
                            // Keeps everything below the fields aligned with them.
                            row![
                                poster,
                                column![
                                    self.details_form(Some(icon), palette),
                                    controls,
                                    navigation
                                ]
                                .spacing(10)
                            ]
                            .spacing(16)
                            .width(Fill)
                            .into()
                        } else {
                            column![
                                self.details_form(None, palette),
                                row![poster, icon].spacing(12),
                                controls,
                                navigation
                            ]
                            .spacing(16)
                            .into()
                        }
                    })
                    .height(Shrink),
                );
            }
            Stage::Review => {
                if let Some(review) = &self.review {
                    let mut application = vec![("Environment", self.draft.environment_name())];
                    if let Some(title) = self.draft.target_title() {
                        application.insert(0, ("Name", title));
                    }
                    let mut source = vec![
                        ("Type", self.draft.kind.to_string()),
                        ("Starting point", self.draft.source.to_string()),
                    ];
                    if !self.draft.path.is_empty() {
                        source.push(("Program", self.draft.path.clone()));
                    }
                    let mut runtime = Vec::new();
                    if let Some(row) = &review.runner_row {
                        runtime.push(("Runner", row.clone()));
                    } else if let Some(Prerequisite::Runner(name)) = &review.prerequisite {
                        runtime.push(("Runner", format!("{name} (not installed)")));
                    }
                    if self.draft.kind == Kind::Windows {
                        runtime.push(("Architecture", review.arch.to_string()));
                    }
                    if let Some(prefix) = &review.prefix {
                        runtime.push(("Prefix", prefix.display().to_string()));
                    }
                    let mut groups = vec![("Application", application), ("Source", source)];
                    if self.draft.kind == Kind::Windows {
                        groups.push(("Runtime", runtime));
                    }
                    body = body.push(review_summary(groups, palette));

                    let mut notes = column![ui::section_heading("Review notes", palette)]
                        .spacing(8)
                        .width(Fill);
                    let mut has_notes = false;
                    if review.adopted {
                        notes = notes.push(
                            text("This compatible existing prefix will be registered in place.")
                                .color(palette.warning),
                        );
                        has_notes = true;
                    }
                    if review.omitted_components {
                        if has_notes {
                            notes = notes.push(ui::separator(palette));
                        }
                        notes = notes.push(text("This Proton runner bundles DXVK and vkd3d-proton, so the template's component picks will be omitted.").color(palette.warning));
                        has_notes = true;
                    }
                    if let Some(prerequisite) = &review.prerequisite {
                        if has_notes {
                            notes = notes.push(ui::separator(palette));
                        }
                        match prerequisite {
                            Prerequisite::Runner(name) => {
                                notes = notes.push(text(format!("Runner {name} is unavailable. Choose a provider to download its latest release:")).color(palette.warning));
                                let mut providers = column![].spacing(8).width(Fill);
                                if name != "Proton-CachyOS-Latest" {
                                    providers = providers.push(
                                        ui::secondary_button("Download GE-Proton", palette)
                                            .on_press(Event::Download(Provider::GeProton)),
                                    );
                                }
                                if name != "GE-Proton-Latest" {
                                    providers = providers.push(
                                        ui::secondary_button("Download Proton-CachyOS", palette)
                                            .on_press(Event::Download(Provider::ProtonCachyos)),
                                    );
                                }
                                notes = notes.push(providers);
                            }
                            Prerequisite::Component { provider, version } => {
                                let description = version.as_ref().map_or_else(
                                    || "latest release".to_owned(),
                                    |name| format!("exact build {name}"),
                                );
                                notes = notes.push(text(format!("Missing {provider} {description}. Download it before creating this environment?")).color(palette.warning));
                                let label = match provider {
                                    Provider::Dxvk => "Download DXVK",
                                    Provider::Vkd3d => "Download vkd3d",
                                    Provider::GeProton => "Download GE-Proton",
                                    Provider::ProtonCachyos => "Download Proton-CachyOS",
                                };
                                notes = notes.push(
                                    ui::secondary_button(label, palette)
                                        .on_press(Event::Download(*provider)),
                                );
                            }
                        }
                        has_notes = true;
                    }
                    if has_notes {
                        body = body.push(ui::section(notes, palette));
                    }
                }
                let can_create = self
                    .review
                    .as_ref()
                    .is_some_and(|review| review.prerequisite.is_none());
                review_footer = Some(
                    row![
                        Space::new().width(Fill),
                        ui::secondary_button("Back", palette).on_press(Event::Back),
                        ui::accent_button("Create", palette.primary, can_create, Event::Create),
                    ]
                    .spacing(8)
                    .into(),
                );
            }
            Stage::Working => {
                body = body.push(text(&self.status));
                // Force stop only makes sense after a cancel, like the other flows.
                body = body.push(if self.token.as_ref().is_some_and(Token::is_canceled) {
                    ui::secondary_button("Force stop", palette).on_press(Event::Force)
                } else {
                    ui::secondary_button("Cancel", palette).on_press(Event::Cancel)
                });
                if !self.log.is_empty() {
                    body = body.push(logview::controls(
                        &self.log_view,
                        true,
                        Event::CopyLog,
                        Event::LogFollow,
                        palette,
                    ));
                    body = body.push(logview::editor(
                        &self.log_view,
                        palette,
                        220,
                        LOG_ID,
                        Event::LogAction,
                        Event::LogScrolled,
                    ));
                }
            }
            Stage::Target => {
                body = body.push(text(
                    "Choose the game executable. The installer itself is not a target.",
                ));
                if let Some(report) = &self.report {
                    for (label, exe) in &report.programs {
                        body = body.push(
                            button(text(format!("{label} - {exe}")))
                                .on_press(Event::Program(exe.clone())),
                        );
                    }
                }
                body = body.push(
                    row![
                        text_input("Executable path", &self.target_path)
                            .on_input(Event::TargetPath),
                        ui::secondary_button("Browse...", palette).on_press(Event::Browse)
                    ]
                    .spacing(8),
                );
                body = body.push(
                    checkbox(self.target_desktop)
                        .label("Create desktop entry")
                        .on_toggle(Event::TargetDesktop),
                );
                body = body.push(
                    row![
                        ui::accent_base("Confirm", palette.primary).on_press(Event::AddTarget),
                        button("Skip for now")
                            .style(move |_, status| crate::chips::link(palette, status))
                            .on_press(Event::SkipTarget),
                    ]
                    .spacing(16)
                    .align_y(Center),
                );
            }
            Stage::Finished => {
                if let Some(report) = &self.report {
                    if report.registered {
                        body = body.push(text(format!(
                            "{} is in your library.",
                            self.draft
                                .target_title()
                                .unwrap_or_else(|| self.draft.environment_name())
                        )));
                    } else {
                        body = body.push(
                            text("Creation did not register an environment.").color(palette.danger),
                        );
                    }
                    for note in &report.notes {
                        body = body.push(text(note));
                    }
                    if !report.cleanup_resolved {
                        body = body.push(
                            ui::secondary_button("Clean up installer", palette)
                                .on_press(Event::CleanUp),
                        );
                    }
                    if let Some(error) = &report.art_error {
                        body = body.push(text(format!("Artwork: {error}")).color(palette.warning));
                        body = body.push(
                            ui::secondary_button("Retry artwork", palette)
                                .on_press(Event::RetryArt),
                        );
                        body = body.push(
                            ui::secondary_button("Skip artwork", palette).on_press(Event::SkipArt),
                        );
                    }
                    if report.registered && report.playable && report.cleanup_resolved {
                        body = body.push(
                            checkbox(self.launch)
                                .label("Launch game when this page closes")
                                .on_toggle(Event::Launch),
                        );
                    }
                    if report.registered {
                        body = body.push(
                            row![
                                ui::secondary_button("Open settings", palette)
                                    .on_press(Event::OpenSettings),
                                ui::secondary_button("Back to library", palette)
                                    .on_press(Event::ToLibrary)
                            ]
                            .spacing(8),
                        );
                    } else {
                        body = body.push(
                            ui::secondary_button("Review again", palette)
                                .on_press(Event::RetryCreation),
                        );
                        body = body.push(
                            ui::secondary_button("Back to library", palette)
                                .on_press(Event::ToLibrary),
                        );
                    }
                }
            }
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(palette.danger));
        }
        if self.leave_prompt {
            body = body.push(
                column![
                    text("Discard this new environment draft?"),
                    row![
                        ui::secondary_button("Keep editing", palette).on_press(Event::KeepEditing),
                        ui::secondary_button("Discard", palette).on_press(Event::Discard)
                    ]
                    .spacing(8)
                ]
                .spacing(8),
            );
        }
        let max_width = if self.stage == Stage::Details {
            900
        } else {
            760
        };
        let scroll =
            scrollable(container(body).max_width(max_width).width(Fill).padding(24)).height(Fill);
        let content: Element<'_, Event> = if let Some(footer) = review_footer {
            column![
                scroll,
                container(footer)
                    .max_width(max_width)
                    .width(Fill)
                    .padding([8, 24])
            ]
            .spacing(0)
            .width(Fill)
            .height(Fill)
            .into()
        } else {
            scroll.into()
        };
        container(content)
            .width(Fill)
            .height(Fill)
            .align_x(Center)
            .into()
    }
}

fn review_summary(
    groups: Vec<(&'static str, Vec<(&'static str, String)>)>,
    palette: Palette,
) -> Element<'static, Event> {
    let mut content = column![].spacing(8).width(Fill);
    for (group_index, (title, fields)) in groups.into_iter().enumerate() {
        if group_index > 0 {
            content = content.push(ui::separator(palette));
        }
        content = content.push(ui::section_heading(title, palette));
        for (field_index, (label, value)) in fields.into_iter().enumerate() {
            if field_index > 0 {
                content = content.push(ui::separator(palette));
            }
            content = content.push(review_field(label, value, palette));
        }
    }
    ui::section(content, palette).into()
}

fn review_field(label: &'static str, value: String, palette: Palette) -> Element<'static, Event> {
    column![
        text(label).size(12).color(palette.muted),
        text(value)
            .size(14)
            .color(palette.foreground)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
    ]
    .spacing(2)
    .width(Fill)
    .into()
}

/// A picker entry: the template or global default, or a named runner.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pick {
    Default,
    Runner(Choice),
}

impl std::fmt::Display for Pick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => f.write_str("Use template or global default"),
            Self::Runner(choice) => choice.fmt(f),
        }
    }
}

fn read_options(paths: &Paths) -> Result<Options, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    let available = Runners::new(paths, &global).map_err(|e| e.to_string())?;
    let mut runners = vec![
        Choice::plain(global.defaults.runner.clone()),
        Choice::plain(Runner::SYSTEM),
    ];
    runners.extend(
        available
            .list()
            .map_err(|e| e.to_string())?
            .runners
            .iter()
            .map(|runner| Choice::of(paths, runner)),
    );
    // A listed build knows its launcher, so it wins over the plain default.
    runners.sort_by(|a, b| a.name.cmp(&b.name).then(b.launcher.cmp(&a.launcher)));
    runners.dedup_by(|a, b| a.name == b.name);
    let templates = Templates::new(paths)
        .list()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    Ok(Options {
        prefix_root: global.prefix_root(paths).map_err(|e| e.to_string())?,
        offer: runner_label::offer(paths, &global),
        runner: global.defaults.runner,
        templates,
        runners,
        arch: global.defaults.arch,
    })
}

fn answer_offer(paths: &Paths, on: bool) -> Result<Options, String> {
    runner_label::answer(paths, on)?;
    read_options(paths)
}

fn template_effects(template: &Template, draft: &Draft) -> Vec<String> {
    let mut effects = Vec::new();
    if !draft.runner.is_empty() {
        effects.push(format!("Runner: {} (your override)", draft.runner));
    } else if let Some(runner) = &template.runner {
        let label = if runner == "proton" {
            "newest installed Proton, or the global default".to_owned()
        } else {
            runner.clone()
        };
        effects.push(format!("Runner: {label}"));
    }
    if draft.arch.is_none()
        && let Some(arch) = template.arch
    {
        effects.push(format!("Architecture: {arch}"));
    }
    if template.dxvk.is_some() || template.vkd3d.is_some() {
        let mut components = Vec::new();
        if let Some(dxvk) = &template.dxvk {
            components.push(format!("DXVK {dxvk}"));
        }
        if let Some(vkd3d) = &template.vkd3d {
            components.push(format!("vkd3d {vkd3d}"));
        }
        effects.push(format!(
            "Graphics: {} (Proton uses its bundled versions)",
            components.join(" + ")
        ));
    }
    for (label, enabled) in [
        ("GameMode", template.gamemode),
        ("Wayland", template.wayland),
        ("MangoHud", template.mangohud),
    ] {
        if let Some(enabled) = enabled {
            effects.push(format!("{label}: {}", if enabled { "on" } else { "off" }));
        }
    }
    if let Some(gamescope) = &template.gamescope {
        effects.push(format!(
            "Gamescope: {}",
            if matches!(gamescope, GamescopeSetting::Off) {
                "off"
            } else {
                "on"
            }
        ));
    }
    if let Some(hooks) = template
        .hooks
        .as_ref()
        .and_then(wineo_core::config::HooksSetting::on)
    {
        let phases: Vec<_> = [("before", &hooks.before), ("after", &hooks.after)]
            .into_iter()
            .filter_map(|(phase, command)| command.as_ref().map(|_| phase))
            .collect();
        if !phases.is_empty() {
            effects.push(format!("Hooks: {}", phases.join(", ")));
        }
    }
    for (label, value) in [("Game ID", &template.gameid), ("Store", &template.store)] {
        if let Some(value) = value {
            effects.push(format!("{label}: {value}"));
        }
    }
    if !template.winetricks.is_empty() {
        effects.push(format!("Winetricks: {}", template.winetricks.join(", ")));
    }
    if !template.tags.is_empty() {
        effects.push(format!("Tags: {}", template.tags.join(", ")));
    }
    if !template.env.is_empty() {
        effects.push(format!(
            "Environment variables: {}",
            template.env.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    effects
}

fn snap_log() -> Task<Event> {
    logview::snap_to_end(LOG_ID)
}

fn prepare(paths: &Paths, draft: &Draft) -> Result<Review, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let name = draft.environment_name();
    let name = name.as_str();
    if paths.environment_config(name).exists() {
        return Err(format!(
            "An environment named {name} already exists. Open its Settings from the library."
        ));
    }
    if draft.kind == Kind::Native {
        if draft.source == Source::Executable {
            launch::check_native_exe(paths, &global, name, &Default::default(), &draft.path)
                .map_err(|error| error.to_string())?;
        }
        return Ok(Review {
            runner: None,
            runner_row: None,
            template: None,
            arch: global.defaults.arch,
            prefix: None,
            adopted: false,
            omitted_components: false,
            prerequisite: None,
        });
    }
    if draft.source == Source::Installer {
        launch::check_program_path(paths, &draft.path).map_err(|error| error.to_string())?;
        let lower = draft.path.to_ascii_lowercase();
        if !lower.ends_with(".exe") && !lower.ends_with(".msi") {
            return Err("Choose an .exe or .msi installer".to_owned());
        }
    }
    let template = if draft.template.is_empty() {
        None
    } else {
        Some(
            Templates::new(paths)
                .resolve(&draft.template)
                .map_err(|error| error.to_string())?,
        )
    };
    let runners = Runners::new(paths, &global).map_err(|error| error.to_string())?;
    let runner_name = if !draft.runner.is_empty() {
        draft.runner.clone()
    } else if let Some(template) = &template {
        template
            .runner_name(&runners, &global.defaults.runner)
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| global.defaults.runner.clone())
    } else {
        global.defaults.runner.clone()
    };
    let runner = match runners.resolve(&runner_name) {
        Ok(runner) => runner,
        Err(error)
            if matches!(
                &error,
                CoreError::UnknownRunner { .. } | CoreError::ProgramNotFound("wine" | "wineserver")
            ) || matches!(&error, CoreError::RunnerAlias { reason, .. } if reason.starts_with("no compatible stable release")) =>
        {
            let prefix = NewEnvironment {
                name: name.to_owned(),
                dir: (!draft.prefix.is_empty()).then(|| draft.prefix.clone()),
                arch: draft.arch,
                template: template.clone(),
            }
            .prefix_dir(paths, &global)
            .map_err(|error| error.to_string())?;
            return Ok(Review {
                runner: None,
                runner_row: None,
                template,
                arch: draft.arch.unwrap_or(global.defaults.arch),
                prefix: Some(prefix),
                adopted: false,
                omitted_components: false,
                prerequisite: Some(Prerequisite::Runner(runner_name)),
            });
        }
        Err(error) => return Err(error.to_string()),
    };
    let arch = if runner.is_proton() {
        Arch::Win64
    } else {
        draft
            .arch
            .or_else(|| template.as_ref().and_then(|template| template.arch))
            .unwrap_or(global.defaults.arch)
    };
    create::check_arch(&runner, Some(arch)).map_err(|error| error.to_string())?;
    let prefix = NewEnvironment {
        name: name.to_owned(),
        dir: (!draft.prefix.is_empty()).then(|| draft.prefix.clone()),
        arch: Some(arch),
        template: template.clone(),
    }
    .prefix_dir(paths, &global)
    .map_err(|error| error.to_string())?;
    let actual = runner.wine_prefix(&prefix);
    if draft.source == Source::Installer {
        let installer = if wineo_core::winpath::is_windows_path(&draft.path) {
            wineo_core::winpath::to_unix(&actual, &draft.path)
        } else {
            Some(
                paths
                    .absolute(&draft.path)
                    .map_err(|error| error.to_string())?,
            )
        };
        if !installer.as_ref().is_some_and(|path| path.is_file()) {
            return Err(format!("Installer {} was not found", draft.path));
        }
    }
    let adopted = wineo_core::prefix::is_prefix(&actual);
    if prefix.exists() && !adopted {
        let occupied = fs::read_dir(&prefix)
            .map_err(|error| error.to_string())?
            .next()
            .is_some();
        if occupied {
            return Err(format!(
                "{} is occupied and is not a compatible prefix",
                prefix.display()
            ));
        }
    }
    let omitted_components = template
        .as_ref()
        .is_some_and(|template| template.for_runner(&runner).1);
    let prerequisite = if let Some(template) = &template {
        let (template, _) = template.for_runner(&runner);
        let wanted = Wanted {
            dxvk: template.dxvk,
            vkd3d: template.vkd3d,
        };
        match install::check_available(paths, &global, name, &runner, &wanted) {
            Ok(()) => None,
            Err(CoreError::NoBuilds { component, .. }) => Some(Prerequisite::Component {
                provider: component_provider(component)?,
                version: None,
            }),
            Err(CoreError::UnknownBuild {
                component, name, ..
            }) => Some(Prerequisite::Component {
                provider: component_provider(component)?,
                version: Some(name),
            }),
            Err(error) => return Err(error.to_string()),
        }
    } else {
        None
    };
    Ok(Review {
        runner_row: Some(runner_label::resolved(paths, &runner)),
        runner: Some(runner),
        template,
        arch,
        prefix: Some(prefix),
        adopted,
        omitted_components,
        prerequisite,
    })
}

fn component_provider(component: &str) -> Result<Provider, String> {
    match component {
        "dxvk" => Ok(Provider::Dxvk),
        "vkd3d" => Ok(Provider::Vkd3d),
        _ => Err(format!("No download provider for {component}")),
    }
}

fn create_request(draft: &Draft, review: Review, runner: Runner) -> CreateRequest {
    CreateRequest {
        name: draft.environment_name(),
        dir: review
            .prefix
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        resolved_runner: Some(runner),
        arch: Some(review.arch),
        template: review.template,
        exe: (draft.source == Source::Executable).then(|| draft.path.clone()),
        target_name: draft.target_title(),
        desktop: draft.desktop,
        expected_adopted: Some(review.adopted),
        ..CreateRequest::default()
    }
}

/// The picked installer program is the environment's only target, so it takes
/// the game title for its desktop entry.
fn target_spec(exe: String, title: Option<String>, desktop: bool) -> Spec {
    Spec {
        exe,
        name: title,
        desktop,
        ..Spec::default()
    }
}

fn perform(
    paths: &Paths,
    draft: Draft,
    review: Review,
    art: artwork::Draft,
    token: Token,
    mut progress: impl FnMut(String),
    mut log_output: impl FnMut(String),
) -> Report {
    let mut report = Report::new();
    let name = draft.environment_name();
    let name = name.as_str();
    if draft.kind == Kind::Native {
        let global = match GlobalConfig::load(paths) {
            Ok(global) => global,
            Err(error) => {
                report.notes.push(error.to_string());
                return report;
            }
        };
        let exe = (draft.source == Source::Executable).then_some(draft.path.as_str());
        match Environment::create_native_titled(
            paths,
            &global,
            name,
            exe,
            draft.target_title().as_deref(),
            draft.desktop,
        ) {
            Ok(environment) => {
                report.registered = true;
                report.playable = exe.is_some();
                progress("Native environment registered".to_owned());
                apply_art(paths, name, &art, &mut report);
                if let Err(error) = desktop::reconcile_native(paths, &global, &environment) {
                    report.notes.push(error.to_string());
                }
            }
            Err(error) => report.notes.push(error.to_string()),
        }
        return report;
    }
    let Some(runner) = review.runner.clone() else {
        report.notes.push("Missing resolved runner".to_owned());
        return report;
    };
    let request = create_request(&draft, review, runner.clone());
    let created = match create::run(paths, request, &token, |event| match event {
        CreateEvent::BuildingPrefix { dir, .. } => {
            progress(format!("Building prefix in {}", dir.display()))
        }
        CreateEvent::Saved => progress("Environment registered".to_owned()),
        CreateEvent::TemplateComponentsOmitted { .. } => {
            progress("Template components bundled by Proton were omitted".to_owned())
        }
        CreateEvent::Components(changes) => {
            progress(format!("Applied {} component changes", changes.len()))
        }
        CreateEvent::Desktop(changes) => {
            progress(format!("Applied {} desktop changes", changes.len()))
        }
        CreateEvent::Registered { .. } => progress("Default target checked".to_owned()),
    }) {
        Ok(created) => created,
        Err(error) => {
            report.registered = paths.environment_config(name).is_file();
            report.notes.push(error.to_string());
            return report;
        }
    };
    report.registered = true;
    report.playable = created.target == TargetOutcome::Added;
    if created.target == TargetOutcome::ExeNotFound {
        report
            .notes
            .push("The executable was not found. Add a target in Settings.".to_owned());
    }
    apply_art(paths, name, &art, &mut report);
    if let Err(error) = desktop::reconcile(
        paths,
        &created.global,
        &created.environment,
        Some(&created.runner),
        &created.prefix,
    ) {
        report.notes.push(format!("Desktop entry: {error}"));
    }
    if draft.source != Source::Installer {
        return report;
    }
    let installer = match Installer::prepare_with_runner(
        created.guard,
        paths,
        &created.global,
        name,
        &draft.path,
        &created.runner,
    ) {
        Ok(installer) => installer,
        Err(error) => {
            report
                .notes
                .push(format!("Installer could not start: {error}"));
            return report;
        }
    };
    let log = Runtime::new(paths, name).install_log_path();
    if let Some(parent) = log.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        report.notes.push(error.to_string());
        return report;
    }
    progress("Running installer...".to_owned());
    let mut log_offset = 0;
    let outcome = installer.run(
        &token,
        Duration::from_secs(created.global.stop_timeout.into()),
        Output::Log(&log),
        |event| {
            pump_log(&log, &mut log_offset, &mut log_output);
            match event {
                Watched::Notice(Notice::Stopping) => progress("Canceling installer...".to_owned()),
                Watched::Notice(Notice::Killing) => {
                    progress("Force stopping installer...".to_owned())
                }
                Watched::Notice(Notice::Warning(error)) => {
                    progress(format!("Installer warning: {error}"))
                }
                Watched::Notice(Notice::Heartbeat) | Watched::Exited(_) => {}
            }
        },
    );
    pump_log(&log, &mut log_offset, &mut log_output);
    match outcome {
        Ok(Outcome::Finished { exit_code: Some(0) }) => {
            report.notes.push("Installed successfully".to_owned());
        }
        Ok(Outcome::Finished {
            exit_code: Some(code),
        }) => report
            .notes
            .push(format!("Installer reported an error (code {code})")),
        Ok(Outcome::Finished { exit_code: None }) => report
            .notes
            .push("Installer finished, but its result is unknown".to_owned()),
        // After a cancel the code only reflects how it was stopped.
        Ok(Outcome::Canceled { .. }) => report.notes.push("Installer canceled".to_owned()),
        Ok(Outcome::Unresolved { error, .. }) => {
            report.cleanup_resolved = false;
            report.notes.push(format!(
                "Programs the installer started may still be running: {error}"
            ));
        }
        Err(error) => report.notes.push(format!("Installer failed: {error}")),
    }
    if Runtime::new(paths, name).check_resolved(name).is_err() {
        report.cleanup_resolved = false;
    }
    if report.cleanup_resolved {
        match programs::scan(&runner.wine_prefix(&created.prefix)) {
            Ok(listing) => {
                report.programs = listing
                    .programs
                    .into_iter()
                    .map(|program| (program.name, program.exe))
                    .collect()
            }
            Err(error) => report.notes.push(format!("Program scan: {error}")),
        }
    }
    report
}

fn pump_log(path: &std::path::Path, offset: &mut usize, output: &mut impl FnMut(String)) {
    if let Ok(bytes) = fs::read(path) {
        if bytes.len() < *offset {
            *offset = 0;
        }
        if bytes.len() > *offset {
            output(String::from_utf8_lossy(&bytes[*offset..]).into_owned());
            *offset = bytes.len();
        }
    }
}

fn apply_art(paths: &Paths, name: &str, art: &artwork::Draft, report: &mut Report) {
    match art.apply(paths, name) {
        Ok(notes) => report.notes.extend(notes),
        Err(error) => report.art_error = Some(error),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn page() -> Page {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        Page::new(paths).0
    }

    #[test]
    fn installer_log_stays_capped_and_mirrored_in_the_view() {
        let mut page = page();
        for index in 0..4000 {
            let _ = page.append_log(&format!("{index} {}\r\n", "x".repeat(60)));
        }
        assert!(page.log.len() <= LOG_CAP);
        assert!(page.log.len() > LOG_KEEP);
        assert!(!page.log.contains('\r'));
        assert_eq!(page.log_view.content().text(), page.log);
    }

    #[test]
    fn installer_log_pauses_while_selected_and_copies_everything() {
        let mut page = page();
        let _ = page.append_log("first\n");
        let _ = page.update(Event::LogAction(
            iced::widget::text_editor::Action::SelectAll,
        ));
        let _ = page.append_log("second\n");
        assert!(page.log_view.paused());
        assert_eq!(page.log_view.content().text(), "first\n");
        let _ = page.update(Event::CopyLog);
        assert!(page.log_view.copied());
        assert_eq!(page.log, "first\nsecond\n");
        let _ = page.update(Event::LogFollow);
        assert_eq!(page.log_view.content().text(), page.log);
    }

    fn fake_runner(paths: &Paths) {
        let bin = GlobalConfig::default()
            .runner_root(paths)
            .unwrap()
            .join("fake")
            .join("bin");
        fs::create_dir_all(&bin).unwrap();
        let wine = bin.join("wine");
        fs::write(&wine, "#!/bin/sh\nif [ \"$1\" = wineboot ]; then\n  mkdir -p \"$WINEPREFIX/drive_c\"\n  printf 'WINE REGISTRY Version 2\\n' > \"$WINEPREFIX/system.reg\"\nfi\nexit 0\n").unwrap();
        let wineserver = bin.join("wineserver");
        fs::write(&wineserver, "#!/bin/sh\nexit 0\n").unwrap();
        for path in [&wine, &wineserver] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn details_and_artwork_fit_narrow_and_wide_dialogs() {
        use crate::layout_probe::probe;
        let page = page();
        assert_eq!(page.stage, Stage::Details);
        for (width, height) in [(280.0, 240.0), (900.0, 720.0)] {
            let mut view = page.view_with_key_availability(Palette::DARK, true);
            let probe = probe(&mut view, iced::Size::new(width, height));
            assert!(!probe.scrollables.is_empty());
            let frame = iced::Rectangle::with_size(iced::Size::new(width, height));
            let spilled = probe.outside(frame);
            assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
        }
    }

    #[test]
    fn back_from_review_preserves_details() {
        let mut page = page();
        page.draft.title = "test-game".to_owned();
        page.draft.path = "/games/test.exe".to_owned();
        page.stage = Stage::Review;
        let _ = page.back();
        assert_eq!(page.stage, Stage::Details);
        assert_eq!(page.draft.path, "/games/test.exe");
    }

    #[test]
    fn details_back_returns_to_add_choices() {
        let mut page = page();
        let _ = page.back();
        assert!(matches!(page.take_action(), Some(Action::Choose)));

        page.draft.title = "draft".to_owned();
        let _ = page.back();
        assert!(page.leave_prompt);
        let _ = page.update(Event::Discard);
        assert!(matches!(page.take_action(), Some(Action::Choose)));
    }

    #[test]
    fn typing_a_title_derives_the_environment_name_and_seeds_art_search() {
        let mut page = page();
        let _ = page.update(Event::Name("The Sims 4".to_owned()));
        assert_eq!(page.draft.environment_name(), "the-sims-4");
        assert_eq!(page.art.search_text(), "The Sims 4");
        assert!(page.draft != Draft::default(), "a title counts as an edit");
    }

    #[test]
    fn edited_environment_name_stops_following_the_title_until_cleared() {
        let mut page = page();
        let _ = page.update(Event::Name("The Sims 4".to_owned()));
        let _ = page.update(Event::EditEnvironment);
        let _ = page.update(Event::EnvironmentName("sims".to_owned()));
        let _ = page.update(Event::Name("The Sims 5".to_owned()));
        assert_eq!(page.draft.environment_name(), "sims");

        let _ = page.update(Event::EnvironmentName(String::new()));
        assert_eq!(page.draft.environment_name(), "the-sims-5");
        let _ = page.update(Event::Name("Hades".to_owned()));
        assert_eq!(page.draft.environment_name(), "hades");

        let _ = page.update(Event::EnvironmentName("bad name".to_owned()));
        assert_eq!(page.validate().unwrap_err(), "spaces not allowed");
    }

    #[test]
    fn a_long_override_is_refused_but_a_long_title_is_cut() {
        let mut page = page();
        let _ = page.update(Event::Name("word ".repeat(100)));
        assert!(page.draft.environment_name().len() <= name::TITLE_MAX);
        assert!(page.validate().is_ok());

        let _ = page.update(Event::EnvironmentName("a".repeat(name::TITLE_MAX + 1)));
        assert_eq!(page.validate().unwrap_err(), "Name is too long.");
        let _ = page.update(Event::EnvironmentName("a".repeat(name::TITLE_MAX)));
        assert!(page.validate().is_ok());
    }

    #[test]
    fn a_title_without_letters_or_digits_needs_an_override() {
        let mut page = page();
        let _ = page.update(Event::Name("\u{30cf}\u{30fc}\u{30c7}\u{30b9}".to_owned()));
        assert_eq!(
            page.validate().unwrap_err(),
            "Name needs letters or digits."
        );
        let _ = page.update(Event::EnvironmentName("hades".to_owned()));
        assert!(page.validate().is_ok());
    }

    #[test]
    fn executable_creation_uses_the_slug_for_the_environment_and_the_title_for_the_target() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        fake_runner(&paths);
        let exe = root.path().join("game.exe");
        fs::write(&exe, "").unwrap();
        let draft = Draft {
            title: "The Sims 4".to_owned(),
            source: Source::Executable,
            path: exe.display().to_string(),
            runner: "fake".to_owned(),
            ..Draft::default()
        };
        let review = prepare(&paths, &draft).unwrap();
        let runner = review.runner.clone().unwrap();
        let request = create_request(&draft, review.clone(), runner);
        assert_eq!(request.name, "the-sims-4");
        assert_eq!(request.target_name.as_deref(), Some("The Sims 4"));

        let report = perform(
            &paths,
            draft,
            review,
            artwork::Draft::new(),
            Token::new(),
            |_| {},
            |_| {},
        );
        assert!(report.registered, "{:?}", report.notes);
        let environment = Environment::load(&paths, "the-sims-4").unwrap();
        assert_eq!(
            environment.config.targets["the-sims-4"].name.as_deref(),
            Some("The Sims 4")
        );
    }

    #[test]
    fn native_executable_titles_its_target() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        let exe = root.path().join("game.sh");
        fs::write(&exe, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let draft = Draft {
            title: "Half-Life 2".to_owned(),
            kind: Kind::Native,
            source: Source::Executable,
            path: exe.display().to_string(),
            ..Draft::default()
        };
        let review = prepare(&paths, &draft).unwrap();
        let report = perform(
            &paths,
            draft,
            review,
            artwork::Draft::new(),
            Token::new(),
            |_| {},
            |_| {},
        );
        assert!(report.registered, "{:?}", report.notes);
        let environment = Environment::load(&paths, "half-life-2").unwrap();
        assert_eq!(
            environment.config.targets["half-life-2"].name.as_deref(),
            Some("Half-Life 2")
        );
    }

    #[test]
    fn picked_installer_program_takes_the_title() {
        let spec = target_spec(
            "C:\\Game\\game.exe".to_owned(),
            Some("Hades".to_owned()),
            true,
        );
        assert_eq!(spec.name.as_deref(), Some("Hades"));
        assert!(spec.desktop);
        assert_eq!(target_spec(String::new(), None, false).name, None);
    }

    #[test]
    fn template_preview_names_only_configured_hook_phases() {
        let draft = page().draft;
        for (before, after, expected) in [
            (true, true, Some("Hooks: before, after")),
            (true, false, Some("Hooks: before")),
            (false, true, Some("Hooks: after")),
            (false, false, None),
        ] {
            let template = Template {
                hooks: Some(wineo_core::config::HooksSetting::On(
                    wineo_core::config::Hooks {
                        before: before.then(|| "private command".into()),
                        after: after.then(|| "private command".into()),
                    },
                )),
                ..Template::default()
            };
            let effects = template_effects(&template, &draft);
            assert_eq!(
                effects
                    .iter()
                    .find(|line| line.starts_with("Hooks:"))
                    .map(String::as_str),
                expected
            );
            assert!(effects.iter().all(|line| !line.contains("private command")));
        }
    }

    #[test]
    fn gaming_template_shows_effects_and_extra_options_only_for_windows() {
        let mut page = page();
        let _ = page.update(Event::Template("gaming".to_owned()));
        let effects = template_effects(page.template_preview.as_ref().unwrap(), &page.draft);
        assert_eq!(
            effects,
            [
                "Runner: newest installed Proton, or the global default",
                "Graphics: DXVK latest + vkd3d latest (Proton uses its bundled versions)",
                "GameMode: on",
                "Tags: game"
            ]
        );
        assert!(!page.additional_wine_options_visible());
        let _ = page.update(Event::ToggleAdditionalWineOptions);
        assert!(page.additional_wine_options_visible());
        let _ = page.update(Event::Kind(Kind::Native));
        assert!(!page.additional_wine_options_visible());
    }

    #[test]
    fn finished_launch_needs_opt_in_and_playable_target() {
        let mut page = page();
        page.draft.title = "test-game".to_owned();
        page.report = Some(Report {
            registered: true,
            playable: true,
            cleanup_resolved: true,
            ..Report::new()
        });
        page.finish();
        let _ = page.update(Event::ToLibrary);
        assert!(matches!(
            page.take_action(),
            Some(Action::Library { launch: false, .. })
        ));

        page.launch = true;
        page.finish();
        assert!(page.launch, "artwork retry preserves the launch choice");
        if let Some(report) = &mut page.report {
            report.cleanup_resolved = false;
        }
        let _ = page.update(Event::ToLibrary);
        assert!(matches!(
            page.take_action(),
            Some(Action::Library { launch: false, .. })
        ));

        if let Some(report) = &mut page.report {
            report.cleanup_resolved = true;
        }
        let _ = page.update(Event::ToLibrary);
        assert!(matches!(
            page.take_action(),
            Some(Action::Library { launch: true, .. })
        ));
    }

    #[test]
    fn installer_creates_environment_without_registering_it_as_target() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        fake_runner(&paths);
        let installer = root.path().join("setup.msi");
        fs::write(&installer, "fixture").unwrap();
        let draft = Draft {
            title: "installed-game".to_owned(),
            source: Source::Installer,
            path: installer.display().to_string(),
            runner: "fake".to_owned(),
            ..Draft::default()
        };
        let review = prepare(&paths, &draft).unwrap();
        let report = perform(
            &paths,
            draft,
            review,
            artwork::Draft::new(),
            Token::new(),
            |_| {},
            |_| {},
        );
        assert!(report.registered, "{:?}", report.notes);
        assert!(report.cleanup_resolved, "{:?}", report.notes);
        assert!(!report.playable);
        assert!(
            Environment::load(&paths, "installed-game")
                .unwrap()
                .config
                .targets
                .is_empty()
        );
    }

    #[test]
    fn cancellation_before_creation_leaves_no_environment() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        fake_runner(&paths);
        let draft = Draft {
            title: "canceled-game".to_owned(),
            runner: "fake".to_owned(),
            ..Draft::default()
        };
        let review = prepare(&paths, &draft).unwrap();
        let token = Token::new();
        token.cancel();
        let report = perform(
            &paths,
            draft,
            review,
            artwork::Draft::new(),
            token,
            |_| {},
            |_| {},
        );
        assert!(!report.registered);
        assert!(!paths.environment_config("canceled-game").exists());
    }

    fn offer_page(paths: &Paths) -> Page {
        let mut page = Page::new(paths.clone()).0;
        let _ = page.update(Event::Options(read_options(paths)));
        page
    }

    fn write_global(paths: &Paths, text: &str) {
        let file = paths.global_config();
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }

    #[test]
    fn the_offer_shows_for_a_game_only_while_unanswered_and_not_portable() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(&home))).unwrap();
        let mut page = offer_page(&paths);
        assert!(!page.offer_shown());
        let _ = page.update(Event::Template("gaming".to_owned()));
        assert!(page.offer_shown());
        let _ = page.update(Event::Kind(Kind::Native));
        assert!(!page.offer_shown());

        write_global(&paths, "external_runners = false\n");
        let mut page = offer_page(&paths);
        let _ = page.update(Event::Template("gaming".to_owned()));
        assert!(!page.offer_shown());

        let portable = root.path().join("portable");
        let paths = Paths::select(
            |key| (key == "HOME").then(|| OsString::from(&home)),
            Some(&portable),
        )
        .unwrap();
        let mut page = offer_page(&paths);
        let _ = page.update(Event::Template("gaming".to_owned()));
        assert!(!page.offer_shown());
    }

    #[test]
    fn answering_the_offer_saves_it_hides_it_and_relists_runners() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
        let mut page = offer_page(&paths);
        let _ = page.update(Event::Template("gaming".to_owned()));
        let steam = |page: &Page| {
            page.options
                .as_ref()
                .unwrap()
                .runners
                .iter()
                .any(|choice| choice.to_string() == "GE-Proton11-3 (Steam)")
        };
        assert!(!steam(&page));

        let _ = page.update(Event::Offer(true));
        assert!(page.answering);
        let _ = page.update(Event::Offered(
            page.offer_sequence,
            answer_offer(&paths, true),
        ));
        assert!(!page.answering);
        assert_eq!(
            GlobalConfig::load(&paths).unwrap().external_runners,
            Some(true)
        );
        assert!(!page.offer_shown());
        assert!(steam(&page));
        // Answered, it never comes back.
        let _ = page.update(Event::Offer(false));
        assert_eq!(
            GlobalConfig::load(&paths).unwrap().external_runners,
            Some(true)
        );
    }

    #[test]
    fn a_late_answer_keeps_the_review_and_rechecks_a_pending_one() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
        let mut page = offer_page(&paths);
        let _ = page.update(Event::Template("gaming".to_owned()));
        let _ = page.update(Event::Name("Hades".to_owned()));

        let _ = page.update(Event::Offer(true));
        let sequence = page.offer_sequence;
        // The buttons are off while it saves, so another click does nothing.
        let _ = page.update(Event::Offer(false));
        assert_eq!(page.offer_sequence, sequence);

        // Next is clicked and its review lands before the answer does.
        let _ = page.update(Event::Next);
        let _ = page.update(Event::Reviewed(
            page.generation,
            prepare(&paths, &page.draft).map(Box::new),
        ));
        assert_eq!(page.stage, Stage::Review);
        let _ = page.update(Event::Offered(sequence, answer_offer(&paths, true)));
        assert_eq!(page.stage, Stage::Review);
        assert!(page.review.is_some());
        assert!(!page.answering);

        // A review still being prepared when the answer lands is redone, so
        // it can't read the old setting.
        fs::remove_file(paths.global_config()).unwrap();
        let mut page = offer_page(&paths);
        let _ = page.update(Event::Template("gaming".to_owned()));
        let _ = page.update(Event::Name("Hades".to_owned()));
        let _ = page.update(Event::Offer(true));
        let _ = page.update(Event::Next);
        let stale = page.generation;
        let _ = page.update(Event::Offered(
            page.offer_sequence,
            answer_offer(&paths, true),
        ));
        assert_ne!(page.generation, stale);
        assert_eq!(page.reviewing, Some(page.generation));
        let old = prepare(&paths, &page.draft).map(Box::new);
        let _ = page.update(Event::Reviewed(stale, old));
        assert_eq!(page.stage, Stage::Details);
        // A reply from an earlier answer is ignored.
        let _ = page.update(Event::Offered(
            page.offer_sequence.wrapping_sub(1),
            Err("old".to_owned()),
        ));
        assert!(page.error.is_none());
    }

    #[test]
    fn the_review_runner_row_names_another_launchers_build() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
        write_global(&paths, "external_runners = true\n");
        let global = GlobalConfig::load(&paths).unwrap();
        let runners = Runners::new(&paths, &global).unwrap();
        let steam = runners.resolve("GE-Proton11-3").unwrap();
        assert_eq!(
            runner_label::resolved(&paths, &steam),
            "GE-Proton11-3 (Steam)"
        );
        fake_runner(&paths);
        let own = runners.resolve("fake").unwrap();
        assert_eq!(runner_label::resolved(&paths, &own), "fake (fake)");

        let draft = Draft {
            title: "steam-game".to_owned(),
            runner: "GE-Proton11-3".to_owned(),
            ..Draft::default()
        };
        let review = prepare(&paths, &draft).unwrap();
        assert_eq!(review.runner_row.as_deref(), Some("GE-Proton11-3 (Steam)"));
    }
}
