//! Installing a GOG game wineo doesn't have yet, or finishing one whose
//! install stopped, like `wineo import gog --game`: a review offering the
//! platform, language, runner, and folder, then download and install
//! progress, then GOG's playtime and art and the desktop entries. A Windows
//! game that names no program to start offers the programs it installed,
//! its own folder's first.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{column, container, pick_list, row, text, text_input};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::cancel::Token;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::create::CreateEvent;
use wineo_core::desktop;
use wineo_core::environment::Environment;
use wineo_core::gog::account::Session;
use wineo_core::gog::api::Client;
use wineo_core::gog::folder::{Platform, ProductStatus, Track};
use wineo_core::gog::install_dir::install_folder;
use wineo_core::gog::update::galaxy::present::{
    Label as GalaxyLabel, Reporter as GalaxyReporter, Wording, restart_lines,
};
use wineo_core::gog::update::galaxy::{
    self as core_galaxy, GalaxyError, install as galaxy_install,
};
use wineo_core::gog::update::install::{self, Installed, Request};
use wineo_core::gog::update::windows::{Event as WindowsEvent, SetupError};
use wineo_core::gog::update::{self, Reporter, UpdateError};
use wineo_core::gog::{language, product};
use wineo_core::key_store::Keyring;
use wineo_core::paths::Paths;
use wineo_core::programs;
use wineo_core::runners::Runners;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::supervisor::{Notice, Watched};
use wineo_core::targets::{self, Batch, Spec, Written};

use super::gog::{Extras, import_extras};
use super::gog_update::{
    LOGGED_OUT, PROGRESS_INTERVAL, setup_window_buttons, setup_window_question,
};
use super::heroic::{self, Prerequisite};
use crate::runner_label::Choice;
use crate::size::format_bytes;
use crate::task;
use crate::theme::Palette;
use crate::ui::{
    PickListStyle, accent_base, list_group, list_row, list_row_text, secondary_button,
};

const CANCELED: &str = "Install canceled.";
const LABEL_WIDTH: f32 = 120.0;

/// A game from the owned list, as the import grid knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Offer {
    pub(crate) gog_id: u64,
    pub(crate) title: String,
    /// The platforms GOG says it runs on.
    pub(crate) linux: bool,
    pub(crate) windows: bool,
    /// The environment whose install of it stopped, which installing again
    /// finishes.
    pub(crate) unfinished: Option<String>,
}

/// What the review lets you change. Empty text keeps the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Choices {
    pub(crate) platform: Option<Platform>,
    /// None takes Galaxy's builds when GOG has them for a Windows game.
    pub(crate) track: Option<Track>,
    pub(crate) language: String,
    pub(crate) runner: Option<String>,
    pub(crate) arch: Option<Arch>,
    pub(crate) dir: String,
}

/// A planned install on either track.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Plan {
    Installer(install::Plan),
    Galaxy(galaxy_install::Plan),
}

impl Plan {
    fn track(&self) -> Track {
        match self {
            Self::Installer(_) => Track::Installer,
            Self::Galaxy(_) => Track::Galaxy,
        }
    }

    fn environment(&self) -> &str {
        match self {
            Self::Installer(plan) => plan.environment(),
            Self::Galaxy(plan) => plan.environment(),
        }
    }

    fn title(&self) -> &str {
        match self {
            Self::Installer(plan) => plan.title(),
            Self::Galaxy(plan) => plan.title(),
        }
    }

    fn gog_id(&self) -> u64 {
        match self {
            Self::Installer(plan) => plan.gog_id(),
            Self::Galaxy(plan) => plan.gog_id(),
        }
    }

    fn folder(&self) -> &Path {
        match self {
            Self::Installer(plan) => plan.folder(),
            Self::Galaxy(plan) => plan.folder(),
        }
    }

    /// Galaxy builds are only for Windows games.
    fn platform(&self) -> Platform {
        match self {
            Self::Installer(plan) => plan.platform(),
            Self::Galaxy(_) => Platform::Windows,
        }
    }

    fn resumes(&self) -> bool {
        match self {
            Self::Installer(plan) => plan.resumes(),
            Self::Galaxy(plan) => plan.resumes(),
        }
    }

    fn has_config(&self) -> bool {
        match self {
            Self::Installer(plan) => plan.has_config(),
            Self::Galaxy(plan) => plan.has_config(),
        }
    }

    fn creation(&self) -> Option<&wineo_core::gog::update::install::Creation> {
        match self {
            Self::Installer(plan) => plan.creation(),
            Self::Galaxy(plan) => plan.creation(),
        }
    }

    fn requested_language(&self) -> &str {
        match self {
            Self::Installer(plan) => plan.requested_language(),
            Self::Galaxy(plan) => plan.requested_language(),
        }
    }

    /// GOG's name for the language an installer installs in. A Galaxy
    /// build has depot languages, which its review doesn't show.
    #[cfg(test)]
    fn language_name(&self) -> &str {
        match self {
            Self::Installer(plan) => plan.language_name(),
            Self::Galaxy(plan) => plan.language(),
        }
    }

    fn warnings(&self) -> &[String] {
        match self {
            Self::Installer(plan) => plan.warnings(),
            Self::Galaxy(plan) => plan.warnings(),
        }
    }

    fn product_labels(&self) -> HashMap<u64, String> {
        match self {
            Self::Installer(plan) => plan.product_labels(),
            Self::Galaxy(plan) => plan.product_labels(),
        }
    }

    /// Whether the game's installer runs as its own setup window.
    fn wizard(&self) -> bool {
        matches!(self, Self::Installer(plan) if plan.wizard())
    }
}

/// A track in the review's picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrackShown(Track);

impl fmt::Display for TrackShown {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(track_name(self.0))
    }
}

fn track_name(track: Track) -> &'static str {
    match track {
        Track::Galaxy => "Galaxy",
        Track::Installer => "Offline installer",
    }
}

/// A planned install and what its review offers.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    name: String,
    /// The plan, or why the choices can't be installed.
    plan: Result<Box<Plan>, String>,
    runners: Vec<Choice>,
    /// Whether GOG offers both tracks for this game, so the review lets
    /// you pick. This is learned for the platform it was planned with
    /// (`choices.platform`) and means nothing for another, so the review only
    /// acts on it while the platform chosen now is that one.
    both_tracks: bool,
    /// The `gog.language` setting, which an empty language uses.
    default_language: String,
    /// `<games_root>/<title>`, which an empty folder uses.
    default_folder: PathBuf,
    /// What it was planned with.
    choices: Choices,
}

/// What a finished install says, and the programs to choose from when a
/// Windows game names none.
#[derive(Debug, Clone, Default)]
pub(crate) struct Done {
    environment: String,
    lines: Vec<String>,
    lists: Vec<(&'static str, Vec<String>)>,
    warnings: Vec<String>,
    /// Labels and paths, the game folder's first. Some only when the game
    /// needs one picked.
    programs: Option<Vec<(String, String)>>,
}

/// Why an install didn't finish, as the page says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    message: String,
    /// The installer that never started, when running it as its own setup
    /// window may get further.
    setup_window: Option<String>,
}

impl Failure {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            setup_window: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Preparing,
    Review,
    Working,
    /// Asking whether to run an installer that never started as its own
    /// setup window.
    Question,
    /// Choosing the program a Windows game starts.
    Target,
    Finished,
}

/// A platform in the review's picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shown(Platform);

impl fmt::Display for Shown {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(platform_name(self.0))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Prepared(u64, Result<Box<Prepared>, String>),
    Platform(Platform),
    Track(Track),
    Language(String),
    /// Plans again with the language typed so far.
    LanguageDone,
    Runner(String),
    Arch(Arch),
    Folder(String),
    FolderDone,
    Browse,
    Picked(u64, String, Result<Option<PathBuf>, String>),
    DownloadRunner,
    PrerequisiteFinished(u64, Result<String, String>),
    Install,
    Cancel,
    Force,
    Progress(u64, String),
    Finished(u64, Result<Box<Done>, Failure>),
    SetupWindow(bool),
    Program(String),
    TargetPath(String),
    BrowseTarget,
    PickedTarget(u64, Result<Option<PathBuf>, String>),
    AddTarget,
    SkipTarget,
    TargetAdded(u64, Result<Vec<String>, String>),
    BackToLibrary,
}

/// What the import page does after an event.
pub(crate) enum Leave {
    /// Back to the games.
    Games,
    /// Out to the library, at the environment when there is one.
    Library(Option<String>),
}

pub(crate) struct Install {
    paths: Paths,
    offer: Offer,
    stage: Stage,
    generation: u64,
    choices: Choices,
    prepared: Option<Prepared>,
    /// Whether GOG offers both tracks, as the last plan found. An explicit
    /// track doesn't ask GOG again, so it carries this on.
    both_tracks: Option<bool>,
    /// The first plan, which names products for a resumed one that only
    /// knows them by id.
    names: Option<Box<Plan>>,
    /// Set while planning again to run the setup window, which then starts
    /// without another review.
    wizard: bool,
    question: Option<String>,
    done: Option<Done>,
    target_path: String,
    token: Option<Token>,
    status: String,
    error: Option<String>,
    notes: Vec<String>,
    leave: Option<Leave>,
}

impl Install {
    pub(crate) fn new(paths: Paths, offer: Offer) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            offer,
            stage: Stage::Preparing,
            generation: 0,
            choices: Choices::default(),
            prepared: None,
            both_tracks: None,
            names: None,
            wizard: false,
            question: None,
            done: None,
            target_path: String::new(),
            token: None,
            status: String::new(),
            error: None,
            notes: Vec::new(),
            leave: None,
        };
        let task = page.prepare();
        (page, task)
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub(crate) fn busy(&self) -> bool {
        self.token.is_some()
    }

    pub(crate) fn waiting(&self) -> bool {
        !self.status.is_empty()
    }

    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn finished(&self) -> bool {
        self.stage == Stage::Finished
    }

    pub(crate) fn take_leave(&mut self) -> Option<Leave> {
        self.leave.take()
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

    fn plan(&self) -> Option<&Plan> {
        self.prepared.as_ref()?.plan.as_deref().ok()
    }

    /// Whether the review shows what installing would do now: a typed
    /// language or folder only counts once it's planned.
    fn current(&self) -> bool {
        self.prepared
            .as_ref()
            .is_some_and(|prepared| prepared.choices == self.choices)
    }

    /// The platform the game installs for without a plan to say: the one
    /// picked, or Windows for a game GOG lists only for Windows.
    fn platform(&self) -> Option<Platform> {
        self.choices
            .platform
            .or((self.offer.windows && !self.offer.linux).then_some(Platform::Windows))
    }

    /// Whether the review can still change things, which a resumed install
    /// can't.
    fn editable(&self) -> bool {
        self.stage == Stage::Review && !self.plan().is_some_and(Plan::resumes)
    }

    /// Whether the tracks can be picked: GOG offers both for the platform
    /// chosen now, not just for one that was.
    fn tracks_offered(&self) -> bool {
        self.prepared.as_ref().is_some_and(|prepared| {
            prepared.both_tracks && prepared.choices.platform == self.choices.platform
        })
    }

    fn windows(&self) -> bool {
        self.plan().map(Plan::platform).or(self.platform()) == Some(Platform::Windows)
    }

    /// A Windows plan that failed, perhaps on its runner or arch, keeps both
    /// pickers so they can be fixed here.
    fn corrects_runner(&self) -> bool {
        self.editable() && self.windows() && self.plan().is_none()
    }

    fn offers_runner_download(&self) -> bool {
        self.editable()
            && self.windows()
            && (self.plan().is_none()
                || self
                    .prepared
                    .as_ref()
                    .is_some_and(|prepared| prepared.runners.is_empty()))
    }

    #[cfg(test)]
    pub(super) fn planned(&self) -> Option<&str> {
        self.plan().map(Plan::title)
    }

    fn ready(&self) -> bool {
        self.plan().is_some() && !self.waiting() && self.current()
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        match self.stage {
            Stage::Review => self.update(Event::Install),
            Stage::Question => self.update(Event::SetupWindow(true)),
            Stage::Target => self.update(Event::AddTarget),
            Stage::Finished => {
                self.exit();
                Task::none()
            }
            Stage::Preparing | Stage::Working => Task::none(),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        if self.busy() {
            return Task::none();
        }
        match self.stage {
            Stage::Preparing | Stage::Review => {
                // A reply to the plan must not reopen it.
                self.next_generation();
                self.status.clear();
                self.leave = Some(Leave::Games);
                Task::none()
            }
            Stage::Question => self.update(Event::SetupWindow(false)),
            Stage::Target => self.update(Event::SkipTarget),
            Stage::Working => Task::none(),
            Stage::Finished => {
                self.exit();
                Task::none()
            }
        }
    }

    fn exit(&mut self) {
        let name = self
            .done
            .as_ref()
            .map(|done| done.environment.clone())
            .or_else(|| self.prepared.as_ref().map(|prepared| prepared.name.clone()))
            .filter(|name| self.paths.environment_config(name).exists());
        self.leave = Some(Leave::Library(name));
    }

    fn prepare(&mut self) -> Task<Event> {
        self.status = format!("Checking {} with GOG...", self.offer.title);
        self.error = None;
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let offer = self.offer.clone();
        let choices = self.choices.clone();
        let wizard = self.wizard;
        let known = self.both_tracks;
        // GOG's answers can't be interrupted, so the token only drops a plan
        // that arrives after Back.
        let token = Token::new();
        task::blocking(
            move || {
                let client =
                    wineo_core::gog::client_from_env().map_err(|error| error.to_string())?;
                prepare_known(
                    &paths,
                    &keyring(),
                    client,
                    &offer,
                    &choices,
                    wizard,
                    known,
                    &token,
                )
                .map(Box::new)
            },
            move |result| Event::Prepared(generation, result),
        )
    }

    fn start(&mut self) -> Task<Event> {
        let Some(plan) = self.plan().cloned() else {
            return Task::none();
        };
        if self.busy() {
            return Task::none();
        }
        let names = self
            .names
            .as_deref()
            .cloned()
            .unwrap_or_else(|| plan.clone());
        let token = Token::new();
        self.token = Some(token.clone());
        self.stage = Stage::Working;
        self.error = None;
        self.status = if plan.resumes() {
            format!("Finishing the install of {}...", names.title())
        } else {
            format!("Installing {}...", names.title())
        };
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            task::join(
                move || {
                    let client = wineo_core::gog::client_from_env()
                        .map_err(|error| Failure::new(error.to_string()))?;
                    install_with(
                        &paths,
                        &keyring(),
                        client,
                        &plan,
                        &names,
                        &token,
                        |line| {
                            let _ = sender.unbounded_send(Event::Progress(generation, line));
                        },
                        import_extras,
                    )
                    .map(Box::new)
                },
                |error| Failure::new(error.to_string()),
            ),
            move |result| Event::Finished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn prerequisite(&mut self, operation: Prerequisite) -> Task<Event> {
        let token = Token::new();
        self.token = Some(token.clone());
        self.status = operation.label();
        self.error = None;
        self.stage = Stage::Working;
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = task::blocking(
            move || {
                heroic::install_prerequisite(&paths, operation, &token, &|line| {
                    let _ = sender.unbounded_send(Event::Progress(generation, line));
                })
            },
            move |result| Event::PrerequisiteFinished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    /// Plans again after a choice changed, unless the review is busy.
    fn replan(&mut self) -> Task<Event> {
        if self.stage != Stage::Review || self.busy() {
            return Task::none();
        }
        self.prepare()
    }

    fn add_target(&mut self) -> Task<Event> {
        let exe = self.target_path.trim().to_owned();
        if exe.is_empty() {
            self.error = Some("Choose a program first.".to_owned());
            return Task::none();
        }
        let Some(done) = &self.done else {
            return Task::none();
        };
        let paths = self.paths.clone();
        let name = done.environment.clone();
        let title = self.offer.title.clone();
        self.error = None;
        self.status = "Adding the program...".to_owned();
        let generation = self.next_generation();
        task::blocking(
            move || add_program(&paths, &name, &title, &exe),
            move |result| Event::TargetAdded(generation, result),
        )
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Prepared(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(prepared) => {
                        let run = std::mem::take(&mut self.wizard);
                        if let Err(error) = &prepared.plan
                            && run
                        {
                            self.error = Some(error.clone());
                            self.stage = Stage::Finished;
                            return Task::none();
                        }
                        self.both_tracks = Some(prepared.both_tracks);
                        self.prepared = Some(*prepared);
                        self.stage = Stage::Review;
                        // The setup window was asked for, so it runs now.
                        if run {
                            return self.start();
                        }
                    }
                    Err(error) => {
                        self.error = Some(error);
                        if std::mem::take(&mut self.wizard) {
                            self.stage = Stage::Finished;
                        }
                    }
                }
                Task::none()
            }
            Event::Platform(platform) if self.stage == Stage::Review => {
                self.choices.platform = Some(platform);
                // Another platform may have other tracks, which planning
                // finds out again.
                self.choices.track = None;
                self.both_tracks = None;
                self.replan()
            }
            Event::Track(track) if self.stage == Stage::Review && self.tracks_offered() => {
                self.choices.track = Some(track);
                self.replan()
            }
            Event::Language(value) if self.stage == Stage::Review => {
                self.choices.language = value;
                Task::none()
            }
            Event::LanguageDone | Event::FolderDone => self.replan(),
            Event::Runner(name) if self.stage == Stage::Review => {
                self.choices.runner = Some(name);
                self.replan()
            }
            Event::Arch(arch) if self.stage == Stage::Review => {
                self.choices.arch = Some(arch);
                self.replan()
            }
            Event::Folder(value) if self.stage == Stage::Review => {
                self.choices.dir = value;
                Task::none()
            }
            Event::Browse if self.stage == Stage::Review && !self.busy() => {
                let previous = self.choices.dir.clone();
                let generation = self.generation;
                Task::perform(
                    crate::picker::pick("Select install folder".to_owned(), true),
                    move |result| Event::Picked(generation, previous.clone(), result),
                )
            }
            Event::Picked(generation, previous, result)
                if generation == self.generation
                    && self.stage == Stage::Review
                    && self.choices.dir == previous =>
            {
                match result {
                    Ok(Some(path)) => {
                        self.choices.dir = path.to_string_lossy().into_owned();
                        self.replan()
                    }
                    Ok(None) => Task::none(),
                    Err(error) => {
                        self.error = Some(error);
                        Task::none()
                    }
                }
            }
            Event::DownloadRunner if self.stage == Stage::Review && !self.waiting() => self
                .prerequisite(Prerequisite::Download(
                    wineo_core::download::Provider::GeProton,
                    None,
                )),
            Event::PrerequisiteFinished(generation, result) if generation == self.generation => {
                self.token = None;
                self.status.clear();
                self.stage = Stage::Review;
                match result {
                    Ok(name) => {
                        self.notes.push(format!(
                            "Installed {name}. It stays installed if this install is canceled."
                        ));
                        self.choices.runner = Some(name);
                        self.prepare()
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Task::none()
                    }
                }
            }
            Event::Install if self.stage == Stage::Review && self.ready() => self.start(),
            Event::Cancel if self.stage == Stage::Working => {
                self.cancel(false);
                Task::none()
            }
            Event::Force if self.stage == Stage::Working => {
                self.cancel(true);
                Task::none()
            }
            Event::Progress(generation, line)
                if generation == self.generation && self.stage == Stage::Working =>
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
                    Ok(done) => {
                        let pick = done.programs.is_some();
                        self.done = Some(*done);
                        self.stage = if pick { Stage::Target } else { Stage::Finished };
                    }
                    // Asked once: a setup window that also never starts is
                    // only reported.
                    Err(Failure {
                        setup_window: Some(label),
                        ..
                    }) if !self.plan().is_some_and(Plan::wizard) => {
                        if self.names.is_none() {
                            self.names = self.plan().cloned().map(Box::new);
                        }
                        self.question = Some(label);
                        self.stage = Stage::Question;
                    }
                    Err(failure) => {
                        self.error = Some(failure.message);
                        self.stage = Stage::Finished;
                    }
                }
                Task::none()
            }
            Event::SetupWindow(run) if self.stage == Stage::Question => {
                let Some(label) = self.question.take() else {
                    return Task::none();
                };
                if run {
                    self.wizard = true;
                    self.stage = Stage::Preparing;
                    return self.prepare();
                }
                let name = self
                    .plan()
                    .map(|plan| plan.environment().to_owned())
                    .unwrap_or_default();
                self.error = Some(format!(
                    "{label}'s installer stopped before installing anything. {}",
                    finish_hint(&self.paths, &name)
                ));
                self.stage = Stage::Finished;
                Task::none()
            }
            Event::Program(exe) if self.stage == Stage::Target => {
                self.target_path = exe;
                Task::none()
            }
            Event::TargetPath(value) if self.stage == Stage::Target => {
                self.target_path = value;
                Task::none()
            }
            Event::BrowseTarget if self.stage == Stage::Target && !self.waiting() => {
                let generation = self.generation;
                Task::perform(
                    crate::picker::pick("Select program".to_owned(), false),
                    move |result| Event::PickedTarget(generation, result),
                )
            }
            Event::PickedTarget(generation, result)
                if generation == self.generation && self.stage == Stage::Target =>
            {
                match result {
                    Ok(Some(path)) => self.target_path = path.to_string_lossy().into_owned(),
                    Ok(None) => {}
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::AddTarget if self.stage == Stage::Target && !self.waiting() => self.add_target(),
            Event::SkipTarget if self.stage == Stage::Target && !self.waiting() => {
                if let Some(done) = &mut self.done {
                    done.lines.push(format!(
                        "Add a program in {}'s settings to play it.",
                        done.environment
                    ));
                }
                self.stage = Stage::Finished;
                Task::none()
            }
            Event::BackToLibrary if self.stage == Stage::Finished => {
                self.exit();
                Task::none()
            }
            Event::TargetAdded(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(notes) => {
                        if let Some(done) = &mut self.done {
                            done.warnings.extend(notes);
                        }
                        self.stage = Stage::Finished;
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn title(&self) -> &'static str {
        match self.stage {
            Stage::Preparing | Stage::Review => "Review GOG install",
            Stage::Working | Stage::Question => "Installing from GOG",
            Stage::Target => "Choose a program",
            Stage::Finished => "GOG install finished",
        }
    }

    /// The scrolling body, and the pinned action row.
    pub(crate) fn view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Event>,
        palette: Palette,
    ) -> (iced::widget::Column<'a, Event>, Option<Element<'a, Event>>) {
        body = body.push(crate::ui::page_title(self.title(), palette));
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
        match self.stage {
            Stage::Preparing => {}
            Stage::Review | Stage::Working => body = self.review_view(body, palette),
            Stage::Question => {
                if let Some(label) = &self.question {
                    body = body.push(text(setup_window_question(label)));
                }
            }
            Stage::Target => body = self.target_view(body, palette),
            Stage::Finished => body = self.result_view(body, palette),
        }
        let footer: Option<Element<'a, Event>> = match self.stage {
            Stage::Review => {
                let label = if self.plan().is_some_and(Plan::resumes) {
                    "Finish"
                } else {
                    "Install"
                };
                Some(
                    accent_base(label, palette.primary)
                        .on_press_maybe(self.ready().then_some(Event::Install))
                        .into(),
                )
            }
            Stage::Working => self.token.as_ref().map(|token| {
                let (label, event) = if token.is_canceled() {
                    ("Force stop", Event::Force)
                } else {
                    ("Cancel", Event::Cancel)
                };
                secondary_button(label, palette).on_press(event).into()
            }),
            Stage::Question => Some(setup_window_buttons(palette, Event::SetupWindow)),
            Stage::Target => Some(
                row![
                    secondary_button("Skip for now", palette)
                        .on_press_maybe((!self.waiting()).then_some(Event::SkipTarget)),
                    accent_base("Use program", palette.primary).on_press_maybe(
                        (!self.waiting() && !self.target_path.trim().is_empty())
                            .then_some(Event::AddTarget)
                    ),
                ]
                .spacing(12)
                .align_y(Center)
                .wrap()
                .vertical_spacing(8)
                .into(),
            ),
            Stage::Finished => Some(
                secondary_button("Back to library", palette)
                    .on_press(Event::BackToLibrary)
                    .into(),
            ),
            Stage::Preparing => None,
        };
        (body, footer)
    }

    fn review_view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Event>,
        palette: Palette,
    ) -> iced::widget::Column<'a, Event> {
        let Some(prepared) = &self.prepared else {
            return body;
        };
        let plan = prepared.plan.as_deref().ok();
        let editable = self.stage == Stage::Review && !plan.is_some_and(Plan::resumes);
        let idle = editable && !self.waiting();
        let shown_row = |label: &'a str, value: String| -> Element<'a, Event> {
            row![
                text(label).color(palette.muted).width(LABEL_WIDTH),
                text(value).width(Fill)
            ]
            .spacing(12)
            .into()
        };
        let control_row = |label: &'a str, control: Element<'a, Event>| -> Element<'a, Event> {
            row![text(label).color(palette.muted).width(LABEL_WIDTH), control]
                .spacing(12)
                .align_y(Center)
                .into()
        };
        let title = plan.map_or(self.offer.title.as_str(), Plan::title);
        let mut summary = column![crate::ui::section_heading(title, palette)].spacing(8);
        let kind = match plan {
            Some(plan) if !plan.has_config() => match plan.platform() {
                Platform::Linux => " (new native environment)",
                Platform::Windows => " (new Windows environment)",
            },
            _ => "",
        };
        summary = summary.push(shown_row("Environment", format!("{}{kind}", prepared.name)));
        if editable {
            let input = text_input(
                &prepared.default_folder.display().to_string(),
                &self.choices.dir,
            )
            .on_input_maybe(idle.then_some(Event::Folder))
            .on_submit_maybe(idle.then_some(Event::FolderDone))
            .padding([6, 10])
            .width(Fill)
            .style(move |_, status| crate::chips::input_style(palette, status));
            summary = summary.push(control_row("Folder", input.into()));
            // Its own line, so a narrow window doesn't squeeze the field away.
            summary = summary.push(control_row(
                "",
                secondary_button("Browse...", palette)
                    .on_press_maybe(idle.then_some(Event::Browse))
                    .into(),
            ));
            if let Some(plan) = plan
                && !self.choices.dir.trim().is_empty()
            {
                summary = summary.push(shown_row("", plan.folder().display().to_string()));
            }
        } else if let Some(plan) = plan {
            summary = summary.push(shown_row("Folder", plan.folder().display().to_string()));
        }
        let both = self.offer.linux && self.offer.windows;
        let platform = plan.map(Plan::platform).or(self.platform());
        if editable && both {
            summary = summary.push(control_row(
                "Platform",
                pick_list(
                    [Shown(Platform::Linux), Shown(Platform::Windows)],
                    platform.map(Shown),
                    |shown| Event::Platform(shown.0),
                )
                .placeholder("Choose a platform")
                .wineo_style(palette)
                .into(),
            ));
        } else if let Some(platform) = platform {
            summary = summary.push(shown_row("Platform", platform_name(platform).to_owned()));
        }
        if editable && self.tracks_offered() {
            summary = summary.push(control_row(
                "Track",
                pick_list(
                    [TrackShown(Track::Galaxy), TrackShown(Track::Installer)],
                    plan.map(Plan::track).or(self.choices.track).map(TrackShown),
                    |shown| Event::Track(shown.0),
                )
                .placeholder("Choose a track")
                .wineo_style(palette)
                .into(),
            ));
        } else if let Some(plan) = plan.filter(|plan| {
            plan.platform() == Platform::Windows && (plan.resumes() || self.tracks_offered())
        }) {
            summary = summary.push(shown_row("Track", track_name(plan.track()).to_owned()));
        }
        if editable {
            summary = summary.push(control_row(
                "Language",
                text_input(&prepared.default_language, &self.choices.language)
                    .on_input_maybe(idle.then_some(Event::Language))
                    .on_submit_maybe(idle.then_some(Event::LanguageDone))
                    .padding([6, 10])
                    .width(Fill)
                    .style(move |_, status| crate::chips::input_style(palette, status))
                    .into(),
            ));
        } else if let Some(plan) = plan {
            summary = summary.push(shown_row("Language", plan.requested_language().to_owned()));
        }
        if let Some(Plan::Installer(plan)) = plan {
            summary = summary.push(shown_row("Installs in", plan.language_name().to_owned()));
        }
        if let Some(creation) = plan.and_then(Plan::creation) {
            if editable {
                let runner = creation.runner().selection();
                summary = summary.push(control_row(
                    "Runner",
                    pick_list(
                        prepared.runners.clone(),
                        Some(Choice::find(&prepared.runners, runner)),
                        |choice| Event::Runner(choice.name),
                    )
                    .placeholder("Choose a runner")
                    .wineo_style(palette)
                    .into(),
                ));
                summary = summary.push(control_row(
                    "Arch",
                    pick_list(
                        [Arch::Win64, Arch::Win32],
                        Some(creation.resolved_arch()),
                        Event::Arch,
                    )
                    .wineo_style(palette)
                    .into(),
                ));
            } else {
                summary = summary.push(shown_row("Runner", creation.runner().name().to_owned()));
                summary = summary.push(shown_row("Arch", creation.resolved_arch().to_string()));
            }
        } else if self.corrects_runner() {
            // The plan failed, perhaps on the runner or arch, so either can
            // be picked again.
            summary = summary.push(control_row(
                "Runner",
                pick_list(
                    prepared.runners.clone(),
                    self.choices
                        .runner
                        .as_deref()
                        .map(|name| Choice::find(&prepared.runners, name)),
                    |choice| Event::Runner(choice.name),
                )
                .placeholder("Choose a runner")
                .wineo_style(palette)
                .into(),
            ));
            summary = summary.push(control_row(
                "Arch",
                pick_list([Arch::Win64, Arch::Win32], self.choices.arch, Event::Arch)
                    .placeholder("Choose an arch")
                    .wineo_style(palette)
                    .into(),
            ));
        }
        match plan {
            Some(Plan::Installer(plan)) => {
                for (label, value) in installer_product_rows(plan) {
                    summary = summary.push(shown_row(label, value));
                }
                summary = summary.push(shown_row(
                    "Download",
                    if plan.is_offline() {
                        "none, uses the packages already downloaded".to_owned()
                    } else {
                        format_bytes(plan.download_size())
                    },
                ));
                summary = summary.push(shown_row(
                    "Installed",
                    format!(
                        "about {} (an estimate)",
                        format_bytes(plan.installed_estimate())
                    ),
                ));
            }
            // Galaxy's sizes are exact, so there is no estimate.
            Some(Plan::Galaxy(plan)) => {
                let mut dlc = false;
                for (label, value) in plan.review_rows(&self.paths, Wording::App) {
                    let label = match label {
                        // One DLC per row; the rest have no label, so they
                        // sit under the first.
                        GalaxyLabel::Dlc if std::mem::replace(&mut dlc, true) => "",
                        label => install_label(&label),
                    };
                    summary = summary.push(shown_row(label, value));
                }
            }
            None => {}
        }
        body = body.push(container(summary).padding(16).width(Fill).style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(12).width(1).color(palette.border))
        }));
        if editable && !self.current() && !self.waiting() {
            body = body.push(
                text("Press Enter to check the new language or folder.")
                    .size(13)
                    .color(palette.muted),
            );
        } else if let Err(error) = &prepared.plan {
            body = body.push(text(error).color(palette.danger));
        }
        // A failed plan may be missing its runner, so one can be fetched.
        if self.offers_runner_download() {
            body = body.push(
                secondary_button("Download latest GE-Proton", palette)
                    .on_press_maybe(idle.then_some(Event::DownloadRunner)),
            );
        }
        if let Some(plan) = plan {
            for warning in review_notes(plan) {
                body = body.push(text(warning).size(13).color(palette.warning));
            }
        }
        body
    }

    fn target_view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Event>,
        palette: Palette,
    ) -> iced::widget::Column<'a, Event> {
        let Some(done) = &self.done else {
            return body;
        };
        body = body.push(text(format!(
            "{} names no program to start. Choose one.",
            self.offer.title
        )));
        let programs = done.programs.as_deref().unwrap_or_default();
        if programs.is_empty() {
            body = body.push(text("Nothing installed was found.").color(palette.muted));
        } else {
            body = body.push(list_group(
                programs.iter().map(|(label, exe)| {
                    let chosen = *exe == self.target_path;
                    let mut line = column![list_row_text(label.as_str())].spacing(2);
                    line = line.push(text(exe).size(12).color(if chosen {
                        palette.primary
                    } else {
                        palette.muted
                    }));
                    list_row(line, palette).on_press(Event::Program(exe.clone()))
                }),
                palette,
            ));
        }
        body.push(
            row![
                text_input("Program path", &self.target_path)
                    .on_input(Event::TargetPath)
                    .padding([6, 10])
                    .width(Fill)
                    .style(move |_, status| crate::chips::input_style(palette, status)),
                secondary_button("Browse...", palette)
                    .on_press_maybe((!self.waiting()).then_some(Event::BrowseTarget)),
            ]
            .spacing(8)
            .align_y(Center),
        )
    }

    fn result_view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Event>,
        palette: Palette,
    ) -> iced::widget::Column<'a, Event> {
        let Some(done) = &self.done else {
            return body;
        };
        for line in &done.lines {
            body = body.push(text(line));
        }
        for (heading, items) in &done.lists {
            let mut list = column![crate::ui::section_heading(heading, palette)].spacing(4);
            for item in items {
                list = list.push(text(item).size(13));
            }
            body = body.push(list);
        }
        for warning in &done.warnings {
            body = body.push(text(warning).size(13).color(palette.warning));
        }
        body
    }
}

fn keyring() -> SecretServiceKeyring {
    SecretServiceKeyring::gog()
}

fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Linux => "Linux",
        Platform::Windows => "Windows",
    }
}

/// The version row and one row per DLC, `NAME (ID)` with its version, like
/// `import gog` prints them.
#[cfg(test)]
fn product_rows(plan: &Plan) -> Vec<(&'static str, String)> {
    // Galaxy's rows are core's.
    match plan {
        Plan::Installer(plan) => installer_product_rows(plan),
        Plan::Galaxy(_) => Vec::new(),
    }
}

fn installer_product_rows(plan: &install::Plan) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    let mut dlc = Vec::new();
    for product in plan.products() {
        let version = match (product.status, &product.version) {
            (ProductStatus::Installed, Some(version)) => version.clone(),
            (ProductStatus::Installed, None) => "unknown version".to_owned(),
            _ => "unavailable".to_owned(),
        };
        if product.id == plan.gog_id() {
            rows.push(("Version", version));
        } else {
            dlc.push(format!("{} {version}", plan.dlc_label(product.id)));
        }
    }
    // One DLC per row; the rest have no label, so they sit under the first.
    for (index, line) in dlc.into_iter().enumerate() {
        rows.push((if index == 0 { "DLC" } else { "" }, line));
    }
    rows
}

/// The label of a row of a Galaxy install's review. The rows an update
/// reviews have none here.
fn install_label(label: &GalaxyLabel) -> &'static str {
    match label {
        GalaxyLabel::Branch => "Branch",
        GalaxyLabel::Pin => "Pin",
        GalaxyLabel::Version => "Version",
        GalaxyLabel::Dlc => "DLC",
        GalaxyLabel::Support => "Support",
        GalaxyLabel::Setup => "Setup",
        GalaxyLabel::Download => "Download",
        _ => "",
    }
}

/// The plan's warnings, then why each product falls back or is unavailable.
fn review_notes(plan: &Plan) -> Vec<&str> {
    let notes: Vec<&Option<String>> = match plan {
        Plan::Installer(plan) => plan
            .products()
            .iter()
            .map(|product| &product.note)
            .collect(),
        Plan::Galaxy(plan) => plan
            .products()
            .iter()
            .map(|product| &product.note)
            .collect(),
    };
    plan.warnings()
        .iter()
        .map(String::as_str)
        .chain(notes.into_iter().filter_map(|note| note.as_deref()))
        .collect()
}

/// How to finish an install that stopped: from the library once its
/// environment exists, else by installing it again.
fn finish_hint(paths: &Paths, name: &str) -> String {
    if !name.is_empty() && paths.environment_config(name).exists() {
        format!("Use Finish install on {name} in the library.")
    } else {
        "Install it again here to finish it.".to_owned()
    }
}

/// The new environment's name, from the game's title on GOG.
fn new_name(paths: &Paths, client: &Client, id: u64) -> Result<String, String> {
    let details =
        product::load_with(paths, client, id, false, true).map_err(|error| format!("{error:#}"))?;
    let title = details.product.title.trim();
    let title = if title.is_empty() {
        format!("GOG game {id}")
    } else {
        title.to_owned()
    };
    Ok(wineo_core::name::from_title(&title))
}

/// Plans installing `offer` with `choices`. A plan that fails is part of
/// what's prepared, so the review can still offer other choices.
#[cfg(test)]
pub(crate) fn prepare_with(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    offer: &Offer,
    choices: &Choices,
    wizard: bool,
    token: &Token,
) -> Result<Prepared, String> {
    prepare_known(paths, keyring, client, offer, choices, wizard, None, token)
}

/// `prepare_with`, given whether GOG offered both tracks when the last plan
/// found out. An explicit track doesn't ask GOG for its builds, and carries
/// that on; so does a plan that never got as far as the question.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_known(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    offer: &Offer,
    choices: &Choices,
    wizard: bool,
    known: Option<bool>,
    token: &Token,
) -> Result<Prepared, String> {
    let mut session = Session::open(paths, keyring, client)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| LOGGED_OUT.to_owned())?;
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let name = match &offer.unfinished {
        Some(name) => name.clone(),
        None => new_name(paths, session.client(), offer.gog_id)?,
    };
    let default_folder = global
        .games_root(paths)
        .ok()
        .and_then(|root| install_folder(&root, &offer.title, None).ok())
        .unwrap_or_default();
    let runners = if offer.windows {
        heroic::runner_choices(paths, &global, true)
            .map(|(_, choices)| choices)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let language = choices.language.trim();
    let dir = choices.dir.trim();
    // The track is chosen before planning on it, so a plan that fails
    // still knows whether there was another to fall back to.
    let mut chosen = None;
    let mut choosing_failed = false;
    let plan = (|| {
        if !language.is_empty() {
            language::validate(language)?;
        }
        let dir = (!dir.is_empty())
            .then(|| paths.absolute(dir))
            .transpose()
            .map_err(|error| format!("{error:#}"))?;
        let request = Request {
            name: name.clone(),
            gog_id: offer.gog_id,
            dir,
            language: (!language.is_empty()).then(|| language.to_owned()),
            // New environments get desktop entries, as a migration's do.
            desktop: true,
            refresh: false,
            read_only: false,
            platform: choices.platform,
            runner: choices.runner.clone(),
            arch: choices.arch,
            wizard,
            // The app installs the default branch's latest build.
            builds: core_galaxy::Choice::Keep,
            // A private branch's password is step 13's panel.
            password: None,
        };
        let track = match choices.track {
            Some(track) => track,
            None => {
                let track = core_galaxy::track_for(paths, &mut session, &request, None).map_err(
                    |error| {
                        choosing_failed = true;
                        format!("{error:#}")
                    },
                )?;
                chosen = Some(track);
                track
            }
        };
        core_galaxy::plan(paths, &mut session, &request, Some(track), token)
            .map(|planned| {
                Box::new(match planned {
                    core_galaxy::Planned::Installer(plan) => Plan::Installer(plan),
                    core_galaxy::Planned::Galaxy(plan) => Plan::Galaxy(plan),
                })
            })
            .map_err(|error| format!("{error:#}"))
    })();
    let journaled = journal_for(paths, &name, offer.gog_id).is_some();
    if token.is_canceled() {
        return Err(if journaled {
            format!("Install interrupted. {}", finish_hint(paths, &name))
        } else {
            CANCELED.to_owned()
        });
    }
    // The failure of a plan that left an unfinished install ends with how
    // to finish it.
    let plan = plan.map_err(|message| {
        let hint = finish_hint(paths, &name);
        if !journaled || message.contains(&hint) {
            return message;
        }
        let sep = if message.ends_with('.') { "" } else { "." };
        format!("{message}{sep} {hint}")
    });
    // With no track asked for, Galaxy is only chosen when GOG has a build
    // and the game can run on Windows, which is when both are possible. A
    // finished install's track was never a choice.
    //
    // When GOG couldn't say, a Windows install may still go either way, so
    // both are offered and the error shows as the plan's.
    let both_tracks = match choices.track {
        None if choosing_failed => {
            offer.windows && choices.platform != Some(Platform::Linux) && offer.unfinished.is_none()
        }
        None if chosen.is_some() => chosen == Some(Track::Galaxy) && offer.unfinished.is_none(),
        // The track wasn't asked about, as when a typed language or folder
        // didn't pass, or an explicit one was: what was known stays.
        None | Some(_) => known.unwrap_or(false),
    };
    Ok(Prepared {
        name,
        plan,
        both_tracks,
        runners,
        default_language: global.gog_language().to_owned(),
        default_folder,
        choices: choices.clone(),
    })
}

/// Runs the install with a status line for each step, then adds GOG's
/// playtime and art and writes the desktop entries, as after a migration.
/// Products are named as `names` names them, since a plan resumed from saved
/// packages may only know them by id.
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_with(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    plan: &Plan,
    names: &Plan,
    token: &Token,
    mut say: impl FnMut(String),
    extras: impl FnOnce(&Paths, &GlobalConfig, &str, u64, &str, &Token) -> Extras,
) -> Result<Done, Failure> {
    let mut session = Session::open(paths, keyring, client)
        .map_err(|error| Failure::new(error.to_string()))?
        .ok_or_else(|| Failure::new(LOGGED_OUT))?;
    let labels = names.product_labels();
    let say = std::cell::RefCell::new(&mut say);
    let installed = match plan {
        Plan::Installer(offline) => {
            let mut reporter = Reporter::new(labels.clone(), PROGRESS_INTERVAL);
            install::apply_with_events(
                paths,
                &mut session,
                offline,
                token,
                |step| {
                    if let Some(line) = reporter.line(&step) {
                        (say.borrow_mut())(line);
                    }
                },
                |event| {
                    if let Some(line) = event_line(event, &labels) {
                        (say.borrow_mut())(line);
                    }
                },
            )
        }
        Plan::Galaxy(galaxy) => {
            let mut reporter = GalaxyReporter::new(PROGRESS_INTERVAL);
            galaxy_install::apply_with_events(
                paths,
                &mut session,
                galaxy,
                token,
                |step| {
                    if let Some(line) = reporter.line(&step) {
                        (say.borrow_mut())(line);
                    }
                },
                |event| {
                    if let Some(line) = event_line(event, &labels) {
                        (say.borrow_mut())(line);
                    }
                },
            )
        }
    }
    .map_err(|error| failure(paths, &error, plan, &labels))?;
    let say = say.into_inner();
    Ok(finish(paths, plan, names, &installed, token, say, extras))
}

/// The status line for something an install's Windows side reports.
fn event_line(event: WindowsEvent, labels: &HashMap<u64, String>) -> Option<String> {
    match event {
        WindowsEvent::Installing {
            product,
            interactive,
        } => {
            let label = labels
                .get(&product)
                .cloned()
                .unwrap_or_else(|| format!("GOG product {product}"));
            Some(if interactive {
                format!("Opening {label}'s setup window")
            } else {
                format!("Running {label}'s installer")
            })
        }
        WindowsEvent::Create(CreateEvent::BuildingPrefix { dir, .. }) => {
            Some(format!("Building prefix in {}", dir.display()))
        }
        WindowsEvent::Watched(Watched::Notice(Notice::Stopping)) => {
            Some("Canceling installer...".to_owned())
        }
        WindowsEvent::Watched(Watched::Notice(Notice::Killing)) => {
            Some("Force stopping installer...".to_owned())
        }
        WindowsEvent::Watched(Watched::Notice(Notice::Warning(error))) => {
            Some(format!("Installer warning: {error}"))
        }
        _ => None,
    }
}

/// How an install that didn't finish left things.
fn failure(
    paths: &Paths,
    error: &anyhow::Error,
    plan: &Plan,
    labels: &HashMap<u64, String>,
) -> Failure {
    let name = plan.environment();
    let hint = || finish_hint(paths, name);
    if let Some(UpdateError::Canceled { journal }) = error.downcast_ref() {
        return Failure::new(if *journal {
            format!("Install interrupted. {}", hint())
        } else {
            CANCELED.to_owned()
        });
    }
    match error.downcast_ref::<SetupError>() {
        // The game's own installer was canceled before it changed anything,
        // but the install it belongs to is still there to finish.
        Some(SetupError::Canceled { .. }) => Failure::new(format!("{CANCELED} {}", hint())),
        Some(SetupError::NotStarted {
            product,
            code,
            wizard,
            ..
        }) => {
            let label = labels
                .get(product)
                .cloned()
                .unwrap_or_else(|| format!("GOG product {product}"));
            Failure {
                message: format!(
                    "{label}'s installer stopped before installing anything (exit code {code}). {}",
                    hint()
                ),
                setup_window: wizard.then_some(label),
            }
        }
        // Its own message names the CLI's way to finish it.
        Some(SetupError::Partway { label, detail, .. }) => Failure::new(format!(
            "{label}'s installer stopped partway ({detail}). {}",
            hint()
        )),
        _ => match error.downcast_ref::<GalaxyError>() {
            // A setup step in the prefix failed; installing again carries
            // on from it.
            Some(GalaxyError::Setup {
                title,
                label,
                detail,
            }) => Failure::new(format!(
                "Setting up {title} stopped: {label} failed ({detail}). {}",
                hint()
            )),
            // Whatever else stopped a Galaxy install, a journal still in
            // the folder is there to finish.
            _ if matches!(plan, Plan::Galaxy(_))
                && journal_for(paths, plan.environment(), plan.gog_id()).is_some() =>
            {
                let message = format!("{error:#}");
                let hint = hint();
                let sep = if message.ends_with('.') { "" } else { "." };
                Failure::new(if message.contains(&hint) {
                    message
                } else {
                    format!("{message}{sep} {hint}")
                })
            }
            _ => Failure::new(format!("{error:#}")),
        },
    }
}

/// The journal an install left, found by its environment or, when that has
/// no config yet, by its game.
fn journal_for(paths: &Paths, name: &str, gog_id: u64) -> Option<wineo_core::gog::folder::Journal> {
    update::journal_of(paths, name).or_else(|| {
        wineo_core::gog::folder::find(paths, name, &gog_id.to_string())
            .ok()
            .flatten()
            .and_then(|state| state.journal)
    })
}

/// Says what the install did, adds the extras and desktop entries, and for
/// a Windows game that names no program, lists what it installed.
fn finish(
    paths: &Paths,
    plan: &Plan,
    names: &Plan,
    installed: &Installed,
    token: &Token,
    say: &mut impl FnMut(String),
    extras: impl FnOnce(&Paths, &GlobalConfig, &str, u64, &str, &Token) -> Extras,
) -> Done {
    let name = installed.environment.clone();
    let title = names.title().to_owned();
    let mut done = Done {
        environment: name.clone(),
        ..Done::default()
    };
    // A plan carried on after the setup window question is still the same
    // install.
    done.lines.push(if plan.resumes() && names.resumes() {
        format!("Finished the interrupted install of {name}.")
    } else {
        format!("Installed {title} as {name}.")
    });
    done.lines.extend(restart_lines(&installed.needs_restart));
    if !installed.saved.is_empty() {
        done.lists.push((
            "Moved your files aside where the game has files:",
            installed
                .saved
                .iter()
                .map(|saved| format!("{} -> {}", saved.path, saved.saved_as))
                .collect(),
        ));
    }
    if !installed.kept.is_empty() {
        done.lists.push((
            "Kept these files you changed, which the game no longer ships:",
            installed.kept.clone(),
        ));
    }
    if !installed.overwritten.is_empty() {
        done.lists.push((
            "Overwrote these files that differed from the game's:",
            installed.overwritten.clone(),
        ));
    }
    // The plan's warnings came first, and the review already showed them.
    done.warnings.extend(
        installed
            .warnings
            .iter()
            .skip(plan.warnings().len())
            .cloned(),
    );
    let global = GlobalConfig::load(paths);
    match &global {
        Ok(global) if !token.is_canceled() => {
            say(format!("Adding GOG playtime and art for {title}..."));
            let added = extras(paths, global, &name, installed.gog_id, &title, token);
            done.lines.extend(added.lines);
            done.warnings.extend(added.notes);
        }
        Ok(_) => done
            .warnings
            .push("Skipped GOG playtime and art: canceled.".to_owned()),
        Err(error) => done.warnings.push(error.to_string()),
    }
    // Last, so a new icon is in the entries.
    if desktop::supported(paths).is_ok()
        && let Err(error) = desktop::apply(paths, &name, false)
    {
        done.warnings
            .push(format!("Couldn't update desktop entries: {error:#}"));
    }
    if plan.platform() == Platform::Windows && installed.target.is_none() {
        done.programs = programs_to_pick(paths, &name, &installed.folder);
    }
    done
}

/// What a Windows game that names no program installed, its own folder's
/// programs first, like `import gog` lists them. None once a target exists.
fn programs_to_pick(paths: &Paths, name: &str, folder: &Path) -> Option<Vec<(String, String)>> {
    let environment = Environment::load(paths, name).ok()?;
    if !environment.config.targets.is_empty() {
        return None;
    }
    let mut listing = programs::scan_folder(folder);
    // Scanned the same way `wineo programs` does, so both list the same.
    if let Some(prefix) = environment
        .prefix(paths)
        .ok()
        .and_then(|prefix| wineo_core::prefix::wine_prefix_in(&prefix))
        && let Ok(found) = programs::scan(&prefix)
    {
        listing.programs.extend(found.programs);
    }
    Some(
        listing
            .programs
            .into_iter()
            .map(|program| (program.name, program.exe))
            .collect(),
    )
}

/// Makes `exe` the environment's default target, named like `wineo setup`
/// names a picked program and titled with the game's title.
fn add_program(paths: &Paths, name: &str, title: &str, exe: &str) -> Result<Vec<String>, String> {
    let environment = Environment::load(paths, name).map_err(|error| error.to_string())?;
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let wine = environment
        .config
        .wine()
        .ok_or_else(|| format!("{name} isn't a Windows environment."))?;
    let runner = Runners::new(paths, &global)
        .and_then(|runners| runners.resolve(&wine.runner))
        .map_err(|error| error.to_string())?;
    let prefix = environment
        .prefix(paths)
        .map_err(|error| error.to_string())?;
    desktop::check(paths, &runner.wine_prefix(&prefix), exe).map_err(|error| error.to_string())?;
    let mut batch = Batch::new(environment.config.targets.clone());
    let stem = Path::new(exe)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suggested = targets::suggest_name(&stem, exe);
    let target = targets::free_name(&suggested, |taken| batch.contains(taken));
    batch
        .add(
            &target,
            Spec {
                exe: exe.to_owned(),
                name: Some(title.to_owned()),
                // GOG games live in the host library, where entries go.
                desktop: true,
                ..Spec::default()
            },
        )
        .map_err(|error| error.to_string())?;
    batch.set_default(Some(&target));
    match targets::write(paths, name, &batch).map_err(|error| error.to_string())? {
        Written::Saved => {}
        Written::Changed(_) => {
            return Err("Targets changed on disk. Open Settings and review them.".to_owned());
        }
    }
    let environment = Environment::load(paths, name).map_err(|error| error.to_string())?;
    Ok(
        desktop::reconcile(paths, &global, &environment, Some(&runner), &prefix)
            .err()
            .map(|error| format!("Desktop entry: {error}"))
            .into_iter()
            .collect(),
    )
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::RefCell;
    use std::ffi::OsString;
    use std::fs;
    use std::sync::{Arc, Mutex};

    use wineo_core::gog::update::windows::set_graphical_session;

    use super::*;
    use crate::gog_fake::{
        Library, Server, inno_runner, library_server, owned_game, product, save_login,
    };
    use crate::settings::steam_key::tests::TestKeyring;

    pub(in crate::flow) const RIMWORLD: u64 = 1094900565;
    pub(in crate::flow) const ROYALTY: u64 = 1233017772;
    pub(in crate::flow) const IDEOLOGY: u64 = 1827857764;
    pub(in crate::flow) const DISCO: u64 = 1771589310;

    /// A logged-in home and a fake GOG where RimWorld has a Linux installer
    /// and a Windows one, with Royalty for Linux and Ideology only for
    /// Windows, and Disco Elysium has only a Windows installer.
    pub(in crate::flow) struct World {
        pub(in crate::flow) root: tempfile::TempDir,
        pub(in crate::flow) paths: Paths,
        pub(in crate::flow) keyring: TestKeyring,
        pub(in crate::flow) library: Arc<Mutex<Library>>,
        pub(in crate::flow) server: Server,
    }

    impl World {
        pub(in crate::flow) fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().to_owned();
            let paths =
                Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
                    .unwrap();
            let keyring = TestKeyring(RefCell::new(None));
            save_login(&paths, &keyring);
            let library = Arc::new(Mutex::new(Library::default()));
            {
                let mut library = library.lock().unwrap();
                library.owned = vec![
                    owned_game(RIMWORLD, "RimWorld", true, true),
                    owned_game(DISCO, "Disco Elysium", false, true),
                ];
                library.owned_ids = vec![RIMWORLD, ROYALTY, IDEOLOGY, DISCO];
                let base = library.linux_installer(RIMWORLD, "1.0", &[("start.sh", "#!/bin/sh\n")]);
                let windows = library.windows_installer(
                    RIMWORLD,
                    "1.0",
                    "mkdir -p \"$DIR\"\n: > \"$DIR/RimWorldWin64.exe\"\nexit 0\n",
                );
                let royalty =
                    library.linux_installer(ROYALTY, "1.0", &[("Data/Royalty/about.txt", "r")]);
                let ideology = library.windows_installer(IDEOLOGY, "1.0", "exit 0\n");
                let details = product(
                    RIMWORLD,
                    "RimWorld",
                    vec![base, windows],
                    vec![
                        product(ROYALTY, "RimWorld - Royalty", vec![royalty], vec![]),
                        product(IDEOLOGY, "RimWorld - Ideology", vec![ideology], vec![]),
                    ],
                );
                library.offer(details);
            }
            let server = library_server(Arc::clone(&library));
            inno_runner(&paths, &root.path().join("inno-runs"));
            let world = Self {
                root,
                paths,
                keyring,
                library,
                server,
            };
            world.disco("1", "", true);
            world
        }

        /// Offers Disco Elysium at `version`, its setup running `before`
        /// first, then installing the game and its info file, which names
        /// `disco.exe` to start when `play_task`.
        pub(in crate::flow) fn disco(&self, version: &str, before: &str, play_task: bool) {
            let tasks = if play_task {
                serde_json::json!([{
                    "isPrimary": true, "type": "FileTask", "path": "disco.exe", "category": "game",
                }])
            } else {
                serde_json::json!([])
            };
            let info = serde_json::json!({
                "gameId": DISCO.to_string(),
                "rootGameId": DISCO.to_string(),
                "name": "Disco Elysium",
                "language": "English",
                "playTasks": tasks,
                "version": 1,
            });
            let script = format!(
                "{before}\nmkdir -p \"$DIR\"\nprintf '{version}' > \"$DIR/disco.exe\"\ncat > \"$DIR/goggame-{DISCO}.info\" <<'INFO'\n{info}\nINFO\nexit 0\n"
            );
            let mut library = self.library.lock().unwrap();
            let installer = library.windows_installer(DISCO, version, &script);
            library.offer(product(DISCO, "Disco Elysium", vec![installer], vec![]));
        }

        /// A setup script line that exits with `code` the first time only.
        pub(in crate::flow) fn fails_once(&self, code: i32) -> String {
            let flag = self.runs().join(format!("failed-once-{code}"));
            format!(
                "if [ ! -f '{flag}' ]; then : > '{flag}'; exit {code}; fi",
                flag = flag.display()
            )
        }

        pub(in crate::flow) fn runs(&self) -> PathBuf {
            self.root.path().join("inno-runs")
        }

        pub(in crate::flow) fn games(&self) -> PathBuf {
            fs::canonicalize(self.root.path()).unwrap().join("Games")
        }

        pub(in crate::flow) fn prepare(
            &self,
            offer: &Offer,
            choices: &Choices,
            wizard: bool,
        ) -> Prepared {
            prepare_with(
                &self.paths,
                &self.keyring,
                self.server.client(),
                offer,
                choices,
                wizard,
                &Token::new(),
            )
            .unwrap()
        }

        pub(in crate::flow) fn install(&self, plan: &Plan, names: &Plan) -> Result<Done, Failure> {
            install_with(
                &self.paths,
                &self.keyring,
                self.server.client(),
                plan,
                names,
                &Token::new(),
                |_| {},
                no_extras,
            )
        }

        /// Installs `offer` with `choices`, as the review would.
        pub(in crate::flow) fn installed(
            &self,
            offer: &Offer,
            choices: &Choices,
        ) -> Result<Done, Failure> {
            let prepared = self.prepare(offer, choices, false);
            let plan = prepared.plan.unwrap();
            self.install(&plan, &plan)
        }
    }

    fn no_extras(_: &Paths, _: &GlobalConfig, _: &str, _: u64, _: &str, _: &Token) -> Extras {
        Extras::default()
    }

    pub(in crate::flow) fn rimworld() -> Offer {
        Offer {
            gog_id: RIMWORLD,
            title: "RimWorld".to_owned(),
            linux: true,
            windows: true,
            unfinished: None,
        }
    }

    pub(in crate::flow) fn disco() -> Offer {
        Offer {
            gog_id: DISCO,
            title: "Disco Elysium".to_owned(),
            linux: false,
            windows: true,
            unfinished: None,
        }
    }

    pub(in crate::flow) fn inno() -> Choices {
        Choices {
            runner: Some("inno".to_owned()),
            ..Choices::default()
        }
    }

    fn page(paths: &Paths, offer: Offer, prepared: Prepared) -> Install {
        let (mut page, _) = Install::new(paths.clone(), offer);
        let generation = page.generation;
        let _ = page.update(Event::Prepared(generation, Ok(Box::new(prepared))));
        page
    }

    fn fits(page: &Install, sizes: &[(f32, f32)]) {
        for &(width, height) in sizes {
            let (body, footer) = page.view(iced::widget::column![], Palette::DARK);
            let mut view =
                iced::widget::column![super::heroic::scroll_body(body, None, Palette::DARK)]
                    .height(Fill);
            if let Some(footer) = footer {
                view = view.push(footer);
            }
            let mut view: Element<'_, Event> = view.into();
            let inset = iced::Rectangle::new(iced::Point::ORIGIN, iced::Size::new(width, height));
            let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
            assert!(probe.outside(inset).is_empty(), "{width}x{height}");
        }
    }

    #[test]
    fn the_review_offers_platform_language_runner_and_folder_with_their_defaults() {
        let world = World::new();
        set_graphical_session(Some(true));
        fs::create_dir_all(world.paths.global_config().parent().unwrap()).unwrap();
        fs::write(world.paths.global_config(), "[gog]\nlanguage = \"fr-FR\"\n").unwrap();
        let prepared = world.prepare(&rimworld(), &Choices::default(), false);
        assert_eq!(prepared.name, "rimworld");
        assert_eq!(prepared.default_language, "fr-FR");
        assert_eq!(
            prepared.default_folder,
            world.root.path().join("Games/RimWorld")
        );
        let plan = prepared.plan.as_deref().unwrap();
        assert_eq!(plan.platform(), Platform::Linux, "Linux when GOG has it");
        assert_eq!(plan.requested_language(), "fr-FR");
        assert_eq!(plan.language_name(), "English");
        assert_eq!(
            product_rows(plan),
            [
                ("Version", "1.0".to_owned()),
                ("DLC", format!("Royalty ({ROYALTY}) 1.0")),
                ("", format!("Ideology ({IDEOLOGY}) unavailable")),
            ]
        );
        let notes = review_notes(plan);
        assert!(
            notes.contains(&"RimWorld has no Linux installer in fr-FR, so it installs in English."),
            "{notes:?}"
        );
        assert!(
            notes.iter().any(|note| note.contains("Ideology")),
            "{notes:?}"
        );
        assert!(plan.creation().is_none());

        let mut page = page(&world.paths, rimworld(), prepared);
        assert_eq!(page.stage, Stage::Review);
        assert!(page.ready());
        fits(&page, &[(280.0, 240.0), (360.0, 480.0), (900.0, 720.0)]);
        // A choice plans again; nothing changes until it lands.
        let _ = page.update(Event::Platform(Platform::Windows));
        assert_eq!(page.choices.platform, Some(Platform::Windows));
        assert_eq!(page.status, "Checking RimWorld with GOG...");
        assert!(!page.ready());
        let _ = page.update(Event::Language("de-AT".to_owned()));
        let _ = page.update(Event::Folder("~/Elsewhere".to_owned()));
        // What's typed isn't installed until it's planned.
        page.status.clear();
        assert!(!page.ready());
        page.choices = Choices {
            platform: Some(Platform::Windows),
            arch: Some(Arch::Win32),
            dir: "~/Elsewhere".to_owned(),
            ..inno()
        };

        let windows = world.prepare(
            &rimworld(),
            &Choices {
                platform: Some(Platform::Windows),
                language: "english".to_owned(),
                ..inno()
            },
            false,
        );
        assert_eq!(
            windows.plan.as_ref().unwrap_err(),
            "Invalid gog.language: use a locale tag like en-US."
        );
        let windows = world.prepare(
            &rimworld(),
            &Choices {
                platform: Some(Platform::Windows),
                arch: Some(Arch::Win32),
                dir: "~/Elsewhere".to_owned(),
                ..inno()
            },
            false,
        );
        assert!(windows.runners.iter().any(|choice| choice.name == "inno"));
        let plan = windows.plan.as_deref().unwrap();
        assert_eq!(plan.platform(), Platform::Windows);
        let creation = plan.creation().unwrap();
        assert_eq!(creation.runner().name(), "inno");
        assert_eq!(creation.resolved_arch(), Arch::Win32);
        assert_eq!(plan.folder(), world.root.path().join("Elsewhere"));
        // Ideology installs on Windows too.
        assert_eq!(
            product_rows(plan)[2],
            ("", format!("Ideology ({IDEOLOGY}) 1.0"))
        );
        let generation = page.generation;
        let _ = page.update(Event::Prepared(generation, Ok(Box::new(windows))));
        assert!(page.ready());
        fits(&page, &[(280.0, 240.0), (900.0, 720.0)]);
        set_graphical_session(None);
    }

    #[test]
    fn a_native_game_installs_end_to_end_and_returns_to_its_card() {
        let world = World::new();
        let prepared = world.prepare(&rimworld(), &Choices::default(), false);
        let plan = prepared.plan.clone().unwrap();
        let mut page = page(&world.paths, rimworld(), prepared);
        let _ = page.update(Event::Install);
        assert_eq!(page.stage, Stage::Working);
        assert!(page.busy());
        assert_eq!(page.status, "Installing RimWorld...");
        let generation = page.generation;
        let mut lines = Vec::new();
        let done = install_with(
            &world.paths,
            &world.keyring,
            world.server.client(),
            &plan,
            &plan,
            &Token::new(),
            |line| lines.push(line),
            no_extras,
        )
        .unwrap();
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("Downloading RimWorld")),
            "{lines:?}"
        );
        assert!(lines.contains(&"Adding GOG playtime and art for RimWorld...".to_owned()));
        assert_eq!(done.lines, ["Installed RimWorld as rimworld."]);
        assert!(done.programs.is_none());
        let folder = world.games().join("RimWorld");
        assert!(folder.join("start.sh").is_file());
        assert!(folder.join("Data/Royalty/about.txt").is_file());
        assert!(Environment::load(&world.paths, "rimworld").is_ok());

        let _ = page.update(Event::Finished(generation, Ok(Box::new(done))));
        assert_eq!(page.stage, Stage::Finished);
        assert!(!page.busy());
        fits(&page, &[(280.0, 240.0), (900.0, 720.0)]);
        let _ = page.submit();
        assert!(matches!(
            page.take_leave(),
            Some(Leave::Library(Some(name))) if name == "rimworld"
        ));
    }

    #[test]
    fn a_windows_game_that_names_no_program_offers_its_own_first() {
        let world = World::new();
        set_graphical_session(Some(true));
        world.disco("1", "", false);
        let prepared = world.prepare(&disco(), &inno(), false);
        let plan = prepared.plan.clone().unwrap();
        let done = world.install(&plan, &plan).unwrap();
        assert_eq!(done.lines[0], "Installed Disco Elysium as disco-elysium.");
        let args = fs::read_to_string(world.runs().join("setup_1771589310_1.exe.args")).unwrap();
        assert!(args.contains("/SILENT"), "{args}");
        let exe = world
            .games()
            .join("Disco Elysium/disco.exe")
            .display()
            .to_string();
        let programs = done.programs.clone().unwrap();
        assert_eq!(programs.first(), Some(&("disco".to_owned(), exe.clone())));

        let mut page = page(&world.paths, disco(), prepared);
        let _ = page.update(Event::Install);
        let generation = page.generation;
        let _ = page.update(Event::Finished(generation, Ok(Box::new(done))));
        assert_eq!(page.stage, Stage::Target);
        fits(&page, &[(280.0, 240.0), (900.0, 720.0)]);
        let _ = page.update(Event::Program(exe.clone()));
        assert_eq!(page.target_path, exe);
        let _ = page.update(Event::AddTarget);
        assert_eq!(page.status, "Adding the program...");
        let added = add_program(&world.paths, "disco-elysium", "Disco Elysium", &exe);
        let generation = page.generation;
        let _ = page.update(Event::TargetAdded(generation, added));
        assert_eq!(page.stage, Stage::Finished, "{:?}", page.error);
        let config = Environment::load(&world.paths, "disco-elysium")
            .unwrap()
            .config;
        let (name, target) = config.targets.iter().next().unwrap();
        assert_eq!(name, "disco");
        assert_eq!(target.exe, exe);
        assert!(target.default);
        set_graphical_session(None);
    }

    #[test]
    fn an_installer_that_never_starts_offers_its_setup_window_once() {
        let world = World::new();
        set_graphical_session(Some(true));
        world.disco("1", &world.fails_once(3), true);
        let prepared = world.prepare(&disco(), &inno(), false);
        let first = prepared.plan.clone().unwrap();
        let failure = world.install(&first, &first).unwrap_err();
        assert_eq!(failure.setup_window.as_deref(), Some("Disco Elysium"));

        // Declining says how to finish it later.
        let mut declined = page_after(&world, prepared.clone(), failure.clone());
        let _ = declined.update(Event::SetupWindow(false));
        assert_eq!(declined.stage, Stage::Finished);
        assert_eq!(
            declined.error.as_deref(),
            Some(
                "Disco Elysium's installer stopped before installing anything. Use Finish install on disco-elysium in the library."
            )
        );

        let mut page = page_after(&world, prepared, failure);
        assert_eq!(page.stage, Stage::Question);
        assert_eq!(
            setup_window_question(page.question.as_deref().unwrap()),
            "Disco Elysium's installer didn't start. Run its own setup window?"
        );
        fits(&page, &[(280.0, 240.0), (900.0, 720.0)]);
        let _ = page.update(Event::SetupWindow(true));
        assert!(page.wizard);
        assert_eq!(page.stage, Stage::Preparing);
        // Planned again, it carries on the install as the setup window.
        let again = world.prepare(&disco(), &inno(), true);
        let plan = again.plan.clone().unwrap();
        assert!(plan.resumes() && plan.wizard());
        let generation = page.generation;
        let _ = page.update(Event::Prepared(generation, Ok(Box::new(again))));
        assert_eq!(page.stage, Stage::Working, "it runs without another review");
        let done = world.install(&plan, &first).unwrap();
        assert_eq!(done.lines[0], "Installed Disco Elysium as disco-elysium.");
        let args = fs::read_to_string(world.runs().join("setup_1771589310_1.exe.args")).unwrap();
        assert!(!args.contains("/SILENT"), "{args}");
        set_graphical_session(None);
    }

    /// A page at the setup window question after `failure`.
    fn page_after(world: &World, prepared: Prepared, failure: Failure) -> Install {
        let mut page = page(&world.paths, disco(), prepared);
        let _ = page.update(Event::Install);
        let generation = page.generation;
        let _ = page.update(Event::Finished(generation, Err(failure)));
        page
    }

    #[test]
    fn canceling_says_whether_the_install_is_left_to_finish() {
        let world = World::new();
        let prepared = world.prepare(&rimworld(), &Choices::default(), false);
        let plan = prepared.plan.clone().unwrap();
        let token = Token::new();
        token.cancel();
        let failure = install_with(
            &world.paths,
            &world.keyring,
            world.server.client(),
            &plan,
            &plan,
            &token,
            |_| {},
            no_extras,
        )
        .unwrap_err();
        assert_eq!(failure.message, "Install canceled.");
        let mut page = page(&world.paths, rimworld(), prepared);
        let _ = page.update(Event::Install);
        let _ = page.back();
        assert_eq!(page.stage, Stage::Working, "Back waits for the work");
        let _ = page.update(Event::Cancel);
        assert_eq!(page.status, "Canceling...");
        let _ = page.update(Event::Force);
        assert_eq!(page.status, "Force stop requested...");
        let generation = page.generation;
        let _ = page.update(Event::Finished(generation, Err(failure)));
        assert_eq!(page.stage, Stage::Finished);
        assert_eq!(page.error.as_deref(), Some("Install canceled."));
    }

    #[test]
    fn logged_out_says_to_log_in() {
        let world = World::new();
        let keyring = TestKeyring(RefCell::new(None));
        let error = prepare_with(
            &world.paths,
            &keyring,
            world.server.client(),
            &rimworld(),
            &Choices::default(),
            false,
            &Token::new(),
        )
        .unwrap_err();
        assert_eq!(error, LOGGED_OUT);
    }

    #[test]
    fn a_failed_windows_plan_keeps_its_runner_and_arch_controls() {
        let world = World::new();
        set_graphical_session(Some(true));
        let prepared = world.prepare(
            &disco(),
            &Choices {
                runner: Some("missing-runner".to_owned()),
                ..Choices::default()
            },
            false,
        );
        assert!(prepared.plan.is_err());
        let mut page = page(&world.paths, disco(), prepared);
        // A Windows-only game stays Windows without a plan to say so.
        assert_eq!(page.platform(), Some(Platform::Windows));
        assert!(page.corrects_runner());
        assert!(page.offers_runner_download());
        fits(&page, &[(280.0, 240.0), (900.0, 720.0)]);
        let _ = page.update(Event::Arch(Arch::Win32));
        assert_eq!(page.choices.arch, Some(Arch::Win32));
        page.status.clear();
        let _ = page.update(Event::Runner("inno".to_owned()));
        assert_eq!(page.choices.runner.as_deref(), Some("inno"));
        let fixed = world.prepare(&disco(), &page.choices.clone(), false);
        let generation = page.generation;
        let _ = page.update(Event::Prepared(generation, Ok(Box::new(fixed))));
        assert!(page.ready());
        assert!(!page.corrects_runner());

        // A game with both picks Windows and keeps it when that plan fails.
        let failed = world.prepare(
            &rimworld(),
            &Choices {
                platform: Some(Platform::Windows),
                runner: Some("missing-runner".to_owned()),
                ..Choices::default()
            },
            false,
        );
        assert!(failed.plan.is_err());
        let mut both = super::tests::page(&world.paths, rimworld(), failed);
        both.choices.platform = Some(Platform::Windows);
        both.choices.runner = Some("missing-runner".to_owned());
        assert!(both.corrects_runner());
        set_graphical_session(None);
    }
}

#[cfg(test)]
mod galaxy_tests;
