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
use wineo_core::create::{self, AfterInstall, CreateEvent, CreateRequest, Stepped, TargetOutcome};
use wineo_core::desktop;
use wineo_core::download::{Client, Provider};
use wineo_core::environment::{Environment, NewEnvironment};
use wineo_core::error::Error as CoreError;
use wineo_core::install::{self, Wanted};
use wineo_core::installer::{self, Outcome, Output};
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
use crate::focus_button::ListRows;
use crate::logview::{self, LogView};
use crate::picker;
use crate::runner_label::{self, Choice};
use crate::task;
use crate::theme::Palette;
use crate::ui::{self, PickListStyle, ScrollStyle};

const LOG_ID: &str = "install-log";

const PROGRAM_LIST: &str = "new-programs";

/// A program row's id on the target step.
fn program_id(exe: &str) -> String {
    format!("new-program:{exe}")
}
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
    /// Whether to ask which installed program to use.
    offer_programs: bool,
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
            offer_programs: false,
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
        task::blocking(move || read_options(&paths), Event::Options)
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

    /// The target step's program list.
    pub(crate) fn rows(&self) -> Option<ListRows> {
        (self.stage == Stage::Target).then(|| ListRows {
            list: PROGRAM_LIST,
            ids: self
                .report
                .iter()
                .flat_map(|report| &report.programs)
                .map(|(_, exe)| program_id(exe))
                .collect(),
        })
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
        task::blocking(
            move || prepare(&paths, &draft),
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
        let work = task::blocking(
            move || {
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
        task::blocking(
            move || {
                let environment = Environment::load(&paths, &name).map_err(|e| e.to_string())?;
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
                        return Err(
                            "Targets changed on disk. Open Settings and review them.".to_owned()
                        );
                    }
                }
                let environment = Environment::load(&paths, &name).map_err(|e| e.to_string())?;
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
                let can_select = report.registered && report.offer_programs;
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
                return task::blocking(
                    move || {
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
                            Some(installer::Cleaned::Unresolved | installer::Cleaned::Untracked)
                        ))
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
                return task::blocking(
                    move || {
                        let result = art.apply(&paths, &name);
                        let desktop =
                            desktop::apply(&paths, &name, false).map_err(|error| error.to_string());
                        match (result, desktop) {
                            (Ok(notes), Ok(_)) => Ok(notes),
                            (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
                            (Err(art_error), Err(desktop_error)) => {
                                Err(format!("{art_error}; desktop entry: {desktop_error}"))
                            }
                        }
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
                return task::blocking(
                    move || answer_offer(&paths, on),
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
                if let Some(report) = &self.report
                    && !report.programs.is_empty()
                {
                    body = body.push(ui::list_group(
                        PROGRAM_LIST,
                        report.programs.iter().map(|(label, exe)| {
                            ui::ListRow::new(
                                program_id(exe),
                                ui::list_row(
                                    ui::list_row_text(format!("{label} - {exe}")),
                                    palette,
                                ),
                                Event::Program(exe.clone()),
                            )
                        }),
                        palette,
                    ));
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
        let scroll = scrollable(container(body).max_width(max_width).width(Fill).padding(24))
            .wineo_edge_style(palette)
            .height(Fill);
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
                gog_id: None,
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
        gog_id: None,
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
            |warning| report.notes.push(warning),
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
    let log = Runtime::new(paths, name).install_log_path();
    let step = (draft.source == Source::Installer).then(|| create::Step::Installer {
        program: &draft.path,
        output: Output::Log(&log),
        token: &token,
    });
    let mut hooks = Hooks {
        paths,
        name,
        art: &art,
        report: &mut report,
        progress: &mut progress,
        log_output: &mut log_output,
        log: &log,
        log_offset: 0,
    };
    let created = create::create_and_install(paths, request, step, &token, &mut hooks);
    let mut log_offset = hooks.log_offset;
    let created = match created {
        Ok(created) => created,
        Err(error) => {
            report.registered = paths.environment_config(name).is_file();
            report.notes.push(error.to_string());
            return report;
        }
    };
    let (outcome, resolved) = match created.step {
        Stepped::None => return report,
        Stepped::Stopped(note) => {
            report.notes.push(note);
            return report;
        }
        Stepped::NotPrepared(error) => {
            report
                .notes
                .push(format!("Installer could not start: {error}"));
            return report;
        }
        Stepped::Ran { outcome, resolved } => (outcome, resolved),
    };
    pump_log(&log, &mut log_offset, &mut log_output);
    let next = create::after_install(&outcome, resolved);
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
        Ok(Outcome::Canceled { .. }) => report
            .notes
            .push("Installer canceled. Add a target in Settings.".to_owned()),
        Ok(Outcome::Unresolved { error, .. }) => report.notes.push(format!(
            "Programs the installer started may still be running: {error}"
        )),
        Err(error) => report.notes.push(format!("Installer failed: {error}")),
    }
    if next == AfterInstall::Unresolved {
        report.cleanup_resolved = false;
    }
    if next == AfterInstall::Programs {
        report.offer_programs = true;
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

/// The desktop app's side of creating a Windows environment, and of its
/// installer.
struct Hooks<'a, P, L> {
    paths: &'a Paths,
    name: &'a str,
    art: &'a artwork::Draft,
    report: &'a mut Report,
    progress: &'a mut P,
    log_output: &'a mut L,
    log: &'a std::path::Path,
    log_offset: usize,
}

impl<P: FnMut(String), L: FnMut(String)> create::Hooks for Hooks<'_, P, L> {
    /// A note for the report.
    type Stop = String;

    fn event(&mut self, event: CreateEvent) {
        let progress = &mut self.progress;
        match event {
            CreateEvent::Warning(warning) => self.report.notes.push(warning),
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
        }
    }

    fn created(&mut self, created: &create::Created) -> Result<(), String> {
        let report = &mut *self.report;
        report.registered = true;
        report.playable = created.target == TargetOutcome::Added;
        if created.target == TargetOutcome::ExeNotFound {
            report
                .notes
                .push("The executable was not found. Add a target in Settings.".to_owned());
        }
        apply_art(self.paths, self.name, self.art, report);
        if let Err(error) = desktop::reconcile(
            self.paths,
            &created.global,
            &created.environment,
            Some(&created.runner),
            &created.prefix,
        ) {
            report.notes.push(format!("Desktop entry: {error}"));
        }
        Ok(())
    }

    fn ready(&mut self, _launch: &launch::Launch) -> Result<(), String> {
        if let Some(parent) = self.log.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        (self.progress)("Running installer...".to_owned());
        Ok(())
    }

    fn watched(&mut self, watched: Watched) {
        pump_log(self.log, &mut self.log_offset, &mut self.log_output);
        let progress = &mut self.progress;
        match watched {
            Watched::Notice(Notice::Stopping) => progress("Canceling installer...".to_owned()),
            Watched::Notice(Notice::Killing) => progress("Force stopping installer...".to_owned()),
            Watched::Notice(Notice::Warning(error)) => {
                progress(format!("Installer warning: {error}"))
            }
            Watched::Notice(Notice::Heartbeat) | Watched::Exited(_) => {}
        }
    }
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
mod tests;
