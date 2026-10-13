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
use wineo_core::gog::folder::{Platform, ProductStatus};
use wineo_core::gog::install_dir::install_folder;
use wineo_core::gog::update::UpdateError;
use wineo_core::gog::update::install::{self, Installed, Plan, Request};
use wineo_core::gog::update::windows::{Event as WindowsEvent, SetupError};
use wineo_core::gog::{language, product};
use wineo_core::key_store::Keyring;
use wineo_core::paths::Paths;
use wineo_core::programs;
use wineo_core::runners::Runners;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::supervisor::{Notice, Watched};
use wineo_core::targets::{self, Batch, Spec, Written};

use super::gog::{Extras, import_extras};
use super::gog_update::{LOGGED_OUT, Reporter, setup_window_buttons, setup_window_question};
use super::heroic::{self, Prerequisite};
use crate::runner_label::Choice;
use crate::size::format_bytes;
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
    pub(crate) language: String,
    pub(crate) runner: Option<String>,
    pub(crate) arch: Option<Arch>,
    pub(crate) dir: String,
}

/// A planned install and what its review offers.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    name: String,
    /// The plan, or why the choices can't be installed.
    plan: Result<Box<Plan>, String>,
    runners: Vec<Choice>,
    /// The `gog.language` setting, which an empty language uses.
    default_language: String,
    /// `<games_root>/<title>`, which an empty folder uses.
    default_folder: PathBuf,
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

    fn ready(&self) -> bool {
        self.plan().is_some() && !self.waiting()
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
        // GOG's answers can't be interrupted, so the token only drops a plan
        // that arrives after Back.
        let token = Token::new();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let client =
                        wineo_core::gog::client_from_env().map_err(|error| error.to_string())?;
                    prepare_with(&paths, &keyring(), client, &offer, &choices, wizard, &token)
                        .map(Box::new)
                })
                .await
                .map_err(|error| error.to_string())?
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
            async move {
                tokio::task::spawn_blocking(move || {
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
                })
                .await
                .map_err(|error| Failure::new(error.to_string()))?
            },
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
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    heroic::install_prerequisite(&paths, operation, &token, &|line| {
                        let _ = sender.unbounded_send(Event::Progress(generation, line));
                    })
                })
                .await
                .map_err(|error| error.to_string())?
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
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || add_program(&paths, &name, &title, &exe))
                    .await
                    .map_err(|error| error.to_string())?
            },
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
        spin: f32,
    ) -> (iced::widget::Column<'a, Event>, Option<Element<'a, Event>>) {
        body = body.push(crate::ui::page_title(self.title(), palette));
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
            Stage::Finished => None,
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
            summary = summary.push(control_row(
                "Folder",
                row![
                    input,
                    secondary_button("Browse...", palette)
                        .on_press_maybe(idle.then_some(Event::Browse))
                ]
                .spacing(8)
                .align_y(Center)
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
        let platform = plan.map(Plan::platform).or(self.choices.platform);
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
        if let Some(plan) = plan {
            summary = summary.push(shown_row("Installs in", plan.language_name().to_owned()));
        }
        let windows = platform == Some(Platform::Windows);
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
        } else if editable && windows && plan.is_none() {
            // The plan failed, perhaps on the runner, so another can be
            // picked.
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
        }
        if let Some(plan) = plan {
            for (label, value) in product_rows(plan) {
                summary = summary.push(shown_row(label, value));
            }
            summary = summary.push(shown_row(
                "Download",
                if plan.is_offline() {
                    "none, uses the packages already downloaded".to_owned()
                } else {
                    format!("about {} (an estimate)", format_bytes(plan.download_size()))
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
        body = body.push(container(summary).padding(16).width(Fill).style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(12).width(1).color(palette.border))
        }));
        if let Err(error) = &prepared.plan {
            body = body.push(text(error).color(palette.danger));
        }
        if editable && windows && prepared.runners.is_empty() {
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
    SecretServiceKeyring::for_spec(&wineo_core::gog::account::KEY_SPEC)
}

fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Linux => "Linux",
        Platform::Windows => "Windows",
    }
}

/// The version row and one row per DLC, `NAME (ID)` with its version, like
/// `import gog` prints them.
fn product_rows(plan: &Plan) -> Vec<(&'static str, String)> {
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

/// The plan's warnings, then why each product falls back or is unavailable.
fn review_notes(plan: &Plan) -> Vec<&str> {
    plan.warnings()
        .iter()
        .map(String::as_str)
        .chain(
            plan.products()
                .iter()
                .filter_map(|product| product.note.as_deref()),
        )
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
pub(crate) fn prepare_with(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    offer: &Offer,
    choices: &Choices,
    wizard: bool,
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
        };
        install::plan(paths, &mut session, &request, token)
            .map(Box::new)
            .map_err(|error| format!("{error:#}"))
    })();
    if token.is_canceled() {
        return Err(CANCELED.to_owned());
    }
    Ok(Prepared {
        name,
        plan,
        runners,
        default_language: global.gog_language().to_owned(),
        default_folder,
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
    let labels: HashMap<u64, String> = names
        .products()
        .iter()
        .map(|product| {
            let label = if product.id == names.gog_id() {
                names.title().to_owned()
            } else {
                names.dlc_label(product.id)
            };
            (product.id, label)
        })
        .collect();
    let say = std::cell::RefCell::new(&mut say);
    let mut reporter = Reporter::with_labels(labels.clone());
    let installed = install::apply_with_events(
        paths,
        &mut session,
        plan,
        token,
        |step| {
            if let Some(line) = reporter.line(&step) {
                (say.borrow_mut())(line);
            }
        },
        |event| {
            let line = match event {
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
            };
            if let Some(line) = line {
                (say.borrow_mut())(line);
            }
        },
    )
    .map_err(|error| failure(paths, &error, plan, &labels))?;
    let say = say.into_inner();
    Ok(finish(paths, plan, names, &installed, token, say, extras))
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
        _ => Failure::new(format!("{error:#}")),
    }
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
