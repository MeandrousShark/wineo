use std::path::PathBuf;

use iced::widget::{column, container, pick_list, row, text, text_input};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::cancel::Token;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::external_runners::Source;
use wineo_core::import::{self, ImportRequest, Inspection};
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;

use super::frame;
use crate::chips::input_style;
use crate::runner_label;
use crate::task;
use crate::theme::Palette;
use crate::ui::{PickListStyle, accent_base, secondary_button};

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Source(String),
    PickSource,
    SourcePicked(u64, Result<Option<PathBuf>, String>),
    Name(String),
    Runner(String),
    Mode(bool),
    Destination(String),
    PickDestination,
    DestinationPicked(u64, Result<Option<PathBuf>, String>),
    Arch(Option<Arch>),
    Dependencies(bool),
    RunnersLoaded(Result<Box<RunnerListing>, String>),
    ReviewLoaded(u64, Result<Box<Review>, String>),
    Progress(u64, String),
    Warning(u64, String),
    Next,
    Import,
    ImportFinished(u64, Result<Box<ImportOutcome>, String>),
    RetryArt,
    ArtRetried(u64, Result<(), String>),
    SkipArt,
    ArtSkipped(u64, Result<(), String>),
    UseRetained,
    Back,
    KeepEditing,
    Discard,
    OpenSettings,
    Library,
    Cancel,
    ForceStop,
    Art(super::artwork::Event),
}

#[derive(Debug, Clone)]
struct RunnerChoice {
    selection: String,
    name: String,
    proton: bool,
    launcher: Option<Source>,
}

impl RunnerChoice {
    fn label(&self) -> String {
        runner_label::noted(
            &format!("{} -> {}", self.selection, self.name),
            if self.proton { "Proton" } else { "Wine" },
            self.launcher,
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RunnerListing {
    choices: Vec<RunnerChoice>,
    default: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Review {
    inspection: Inspection,
    destination: PathBuf,
    runner_name: String,
    arch_missing: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ImportOutcome {
    name: String,
    art_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Details,
    Review,
    Working,
    ArtFailed,
    Finished,
}

pub(crate) struct Page {
    paths: Paths,
    source: String,
    name: String,
    runner: String,
    runners: Vec<RunnerChoice>,
    adopt: bool,
    destination: String,
    arch: Option<Arch>,
    review: Option<Review>,
    step: Step,
    error: Option<String>,
    warning: Option<String>,
    status: String,
    show_dependencies: bool,
    dirty: bool,
    discard_prompt: bool,
    token: Option<Token>,
    generation: u64,
    action: Option<super::Action>,
    art: super::artwork::Draft,
    imported_name: Option<String>,
    recovery_path: Option<PathBuf>,
}

impl Page {
    pub(crate) fn new(paths: Paths) -> (Self, Task<Event>) {
        let mut page = Self {
            paths: paths.clone(),
            source: String::new(),
            name: String::new(),
            runner: String::new(),
            runners: Vec::new(),
            adopt: false,
            destination: String::new(),
            arch: None,
            review: None,
            step: Step::Details,
            error: None,
            warning: None,
            status: String::new(),
            show_dependencies: false,
            dirty: false,
            discard_prompt: false,
            token: None,
            generation: 0,
            action: None,
            art: super::artwork::Draft::new(),
            imported_name: None,
            recovery_path: None,
        };
        let load_paths = paths.clone();
        let load = task::blocking(
            move || runner_choices(&load_paths),
            |result| Event::RunnersLoaded(result.map(Box::new)),
        );
        let art = page.art.configure(paths, String::new()).map(Event::Art);
        (page, Task::batch([load, art]))
    }

    pub(crate) fn update_with_key_availability(
        &mut self,
        event: Event,
        key_available: bool,
    ) -> Task<Event> {
        match event {
            Event::Source(value) => {
                self.generation = self.generation.wrapping_add(1);
                self.source = value;
                self.dirty = true;
                self.clear_review();
                Task::none()
            }
            Event::PickSource => {
                self.generation = self.generation.wrapping_add(1);
                let generation = self.generation;
                Task::perform(
                    crate::picker::pick("Choose a prefix folder".to_owned(), true),
                    move |result| Event::SourcePicked(generation, result),
                )
            }
            Event::SourcePicked(generation, _) if generation != self.generation => Task::none(),
            Event::SourcePicked(_, Ok(Some(path))) => {
                self.generation = self.generation.wrapping_add(1);
                self.source = path.display().to_string();
                self.dirty = true;
                self.clear_review();
                Task::none()
            }
            Event::SourcePicked(_, Ok(None)) => Task::none(),
            Event::SourcePicked(_, Err(error)) => {
                self.error = Some(error);
                Task::none()
            }
            Event::Name(value) => {
                self.name = value;
                self.dirty = true;
                self.clear_review();
                self.art.set_title(&self.name).map(Event::Art)
            }
            Event::Runner(value) => {
                self.runner = value;
                self.dirty = true;
                self.clear_review();
                Task::none()
            }
            Event::Mode(adopt) => {
                self.adopt = adopt;
                self.dirty = true;
                self.clear_review();
                Task::none()
            }
            Event::Destination(value) => {
                self.generation = self.generation.wrapping_add(1);
                self.destination = value;
                self.dirty = true;
                self.clear_review();
                Task::none()
            }
            Event::PickDestination => {
                self.generation = self.generation.wrapping_add(1);
                let generation = self.generation;
                Task::perform(
                    crate::picker::pick("Choose a copy destination".to_owned(), true),
                    move |result| Event::DestinationPicked(generation, result),
                )
            }
            Event::DestinationPicked(generation, _) if generation != self.generation => {
                Task::none()
            }
            Event::DestinationPicked(_, Ok(Some(path))) => {
                self.generation = self.generation.wrapping_add(1);
                self.destination = path.display().to_string();
                self.dirty = true;
                self.clear_review();
                Task::none()
            }
            Event::DestinationPicked(_, Ok(None)) => Task::none(),
            Event::DestinationPicked(_, Err(error)) => {
                self.error = Some(error);
                Task::none()
            }
            Event::Arch(arch) => {
                self.arch = arch;
                self.clear_review();
                self.begin_review()
            }
            Event::Dependencies(show) => {
                self.show_dependencies = show;
                Task::none()
            }
            Event::RunnersLoaded(Ok(listing)) => {
                if self.runner.is_empty() {
                    self.runner = listing.default;
                }
                self.runners = listing.choices;
                Task::none()
            }
            Event::RunnersLoaded(Err(error)) => {
                self.error = Some(error);
                Task::none()
            }
            Event::ReviewLoaded(generation, result) => {
                if generation != self.generation || self.step != Step::Working {
                    return Task::none();
                }
                match result {
                    Ok(review) => {
                        let review = *review;
                        if review.arch_missing && self.arch.is_none() {
                            self.review = Some(review);
                            self.step = Step::Review;
                            self.error = Some(
                                "The prefix has no architecture marker. Choose win64 or win32 and review again.".to_owned(),
                            );
                        } else {
                            self.review = Some(review);
                            self.step = Step::Review;
                            self.error = None;
                        }
                    }
                    Err(error) => {
                        self.step = Step::Details;
                        self.error = Some(error);
                    }
                }
                Task::none()
            }
            Event::Progress(generation, status) => {
                if generation == self.generation && self.step == Step::Working {
                    self.status = status;
                }
                Task::none()
            }
            Event::Warning(generation, warning) => {
                if generation == self.generation {
                    self.warning = Some(warning);
                }
                Task::none()
            }
            Event::Import => self.begin_import(),
            Event::Next => self.begin_review(),
            Event::ImportFinished(generation, result) => {
                if generation != self.generation || self.step != Step::Working {
                    return Task::none();
                }
                self.token = None;
                match result {
                    Ok(outcome) => {
                        let outcome = *outcome;
                        self.imported_name = Some(outcome.name);
                        self.step = if let Some(error) = outcome.art_error {
                            self.error = Some(error);
                            Step::ArtFailed
                        } else {
                            self.error = None;
                            Step::Finished
                        };
                    }
                    Err(error) => {
                        if !self.adopt
                            && error.contains("retry this import with --adopt")
                            && let Some(review) = &self.review
                        {
                            self.recovery_path = Some(review.destination.clone());
                        }
                        self.error = Some(error);
                        self.step = Step::Review;
                    }
                }
                Task::none()
            }
            Event::RetryArt => self.retry_art(),
            Event::ArtRetried(generation, result) => {
                if generation != self.generation || self.step != Step::ArtFailed {
                    return Task::none();
                }
                self.token = None;
                match result {
                    Ok(_) => {
                        self.error = None;
                        self.step = Step::Finished;
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::SkipArt => self.skip_art(),
            Event::ArtSkipped(generation, result) => {
                if generation != self.generation || self.step != Step::Working {
                    return Task::none();
                }
                self.token = None;
                self.step = Step::Finished;
                self.error = result.err();
                Task::none()
            }
            Event::UseRetained => {
                if let Some(path) = self.recovery_path.take() {
                    self.source = path.display().to_string();
                    self.adopt = true;
                    self.destination.clear();
                    self.arch = None;
                    self.review = None;
                    self.step = Step::Details;
                    self.error = None;
                    self.dirty = true;
                }
                Task::none()
            }
            Event::Back => self.back(),
            Event::KeepEditing => {
                self.discard_prompt = false;
                Task::none()
            }
            Event::Discard => {
                self.discard_prompt = false;
                self.action = Some(super::Action::Choose);
                Task::none()
            }
            Event::OpenSettings => {
                if let Some(name) = self.imported_name.clone() {
                    self.action = Some(super::Action::Settings {
                        name,
                        launch: false,
                    });
                }
                Task::none()
            }
            Event::Library => {
                self.action = Some(super::Action::Library {
                    name: self.imported_name.clone(),
                    launch: false,
                });
                Task::none()
            }
            Event::Cancel => {
                self.cancel(false);
                Task::none()
            }
            Event::ForceStop => {
                self.cancel(true);
                Task::none()
            }
            Event::Art(event) => self
                .art
                .update_with_key_availability(event, key_available)
                .map(Event::Art),
        }
    }

    pub(crate) fn view_with_key_availability(
        &self,
        palette: Palette,
        key_available: bool,
    ) -> Element<'_, Event> {
        let body = match self.step {
            Step::Details => self.details(palette, key_available),
            Step::Review => self.review_view(palette, key_available),
            Step::Working => column![
                text(self.status.as_str()).color(palette.muted),
                self.token.as_ref().map(|token| {
                    let (label, event) = if token.is_canceled() {
                        ("Force stop", Event::ForceStop)
                    } else {
                        ("Cancel", Event::Cancel)
                    };
                    secondary_button(label, palette)
                        .on_press(event)
                })
            ]
            .spacing(14)
            .into(),
            Step::ArtFailed => column![
                text(format!(
                    "{} was imported, but artwork could not be copied.",
                    self.imported_name.as_deref().unwrap_or("Environment")
                ))
                .color(palette.warning),
                row![
                    secondary_button("Retry artwork", palette)
                        .on_press(Event::RetryArt),
                    secondary_button("Skip artwork", palette)
                        .on_press(Event::SkipArt)
                ]
                .spacing(8),
            ]
            .spacing(12)
            .into(),
            Step::Finished => column![
                text(format!(
                    "Imported {}.",
                    self.imported_name.as_deref().unwrap_or("environment")
                ))
                .color(palette.success),
                text("Scan installed programs and choose game targets now, or add them later in Settings.")
                    .color(palette.muted),
                row![
                    secondary_button("Set up targets", palette)
                        .on_press(Event::OpenSettings),
                    secondary_button("Back to library", palette)
                        .on_press(Event::Library)
                ]
                .spacing(8),
            ]
            .spacing(12)
            .into(),
        };
        let mut content = column![
            text("Import prefix").size(22).color(palette.foreground),
            body
        ]
        .spacing(18);
        if let Some(error) = &self.error {
            content = content.push(text(error.as_str()).size(13).color(palette.danger));
        }
        if let Some(warning) = &self.warning {
            content = content.push(text(warning.as_str()).size(13).color(palette.warning));
        }
        // Back goes with the forward action, so neither shows while a review is
        // still being prepared.
        let footer: Option<Element<'_, Event>> = match self.step {
            Step::Details => Some(
                accent_base("Review", palette.primary)
                    .on_press(Event::Next)
                    .into(),
            ),
            // The retained copy is the likely next move after a failed import, so
            // it stays in view beside Import instead of scrolling away.
            Step::Review if self.review.is_some() => {
                let mut actions =
                    row![accent_base("Import", palette.primary).on_press(Event::Import)]
                        .spacing(12)
                        .align_y(Center);
                if self.recovery_path.is_some() {
                    actions = actions.push(
                        secondary_button("Use retained copy in place", palette)
                            .on_press(Event::UseRetained),
                    );
                }
                Some(actions.wrap().vertical_spacing(8).into())
            }
            _ => None,
        };
        let mut page = column![]
            .spacing(12)
            .max_width(frame::PAGE_MAX_WIDTH)
            .height(Fill);
        if footer.is_some() {
            page = page.push(row![
                secondary_button("Back", palette).on_press(Event::Back)
            ]);
        }
        page = page.push(frame::scroll_body(content, None, palette));
        if let Some(footer) = footer {
            page = page.push(frame::pinned(footer, palette));
        }
        let base = frame::page_frame(page);
        if self.discard_prompt {
            iced::widget::stack![base, discard_prompt(palette)].into()
        } else {
            base
        }
    }

    pub(crate) fn busy(&self) -> bool {
        matches!(self.step, Step::Working)
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        if let Some(token) = &self.token {
            if force {
                token.force();
                self.status = "Force stopping...".to_owned();
            } else {
                token.cancel();
                self.status = "Canceling...".to_owned();
            }
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<super::Action> {
        self.action.take()
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        if self.busy() {
            return Task::none();
        }
        match self.step {
            Step::Details => self.begin_review(),
            Step::Review => self.begin_import(),
            Step::ArtFailed => self.retry_art(),
            Step::Finished => {
                self.action = Some(super::Action::Library {
                    name: self.imported_name.clone(),
                    launch: false,
                });
                Task::none()
            }
            Step::Working => Task::none(),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        match self.step {
            Step::Details => {
                if self.dirty || self.art.is_dirty() {
                    self.discard_prompt = true;
                } else {
                    self.action = Some(super::Action::Choose);
                }
                Task::none()
            }
            Step::Review => {
                self.step = Step::Details;
                self.error = None;
                Task::none()
            }
            Step::ArtFailed | Step::Finished => {
                self.action = Some(super::Action::Library {
                    name: self.imported_name.clone(),
                    launch: false,
                });
                Task::none()
            }
            Step::Working => Task::none(),
        }
    }

    fn details(&self, palette: Palette, key_available: bool) -> Element<'_, Event> {
        let runner_options: Vec<String> = self.runners.iter().map(RunnerChoice::label).collect();
        let selected = self
            .runners
            .iter()
            .find(|runner| runner.selection == self.runner)
            .map(RunnerChoice::label);
        let mut fields = column![
            text("Source folder").color(palette.foreground),
            row![
                text_input("Path to Wine prefix or Proton compatdata", &self.source)
                    .on_input(Event::Source)
                    .width(Fill)
                    .padding(8)
                    .style(move |_, status| input_style(palette, status)),
                secondary_button("Browse...", palette).on_press(Event::PickSource)
            ]
            .spacing(8),
            text("Environment name").color(palette.foreground),
            text_input("Name", &self.name)
                .on_input(Event::Name)
                .padding(8)
                .style(move |_, status| input_style(palette, status)),
            text("Compatible runner").color(palette.foreground),
            pick_list(runner_options, selected, |value| Event::Runner(
                value.split(" -> ").next().unwrap_or(&value).to_owned()
            ))
            .placeholder("Choose a runner")
            .width(Fill)
            .wineo_style(palette),
            row![
                secondary_button(
                    if self.adopt {
                        "Use in place (selected)"
                    } else {
                        "Use in place"
                    },
                    palette
                )
                .on_press(Event::Mode(true)),
                secondary_button(
                    if !self.adopt {
                        "Copy (default)"
                    } else {
                        "Copy"
                    },
                    palette
                )
                .on_press(Event::Mode(false))
            ]
            .spacing(8),
        ]
        .spacing(8);
        if self.adopt {
            fields = fields.push(
                text("Later launches will modify the original prefix.")
                    .size(13)
                    .color(palette.warning),
            );
        } else {
            fields = fields
                .push(text("Copy destination").color(palette.foreground))
                .push(
                    row![
                        text_input("Default: configured prefix root/name", &self.destination)
                            .on_input(Event::Destination)
                            .width(Fill)
                            .padding(8)
                            .style(move |_, status| input_style(palette, status)),
                        secondary_button("Browse...", palette).on_press(Event::PickDestination)
                    ]
                    .spacing(8),
                );
        }
        fields = fields.push(self.art.view(palette, key_available).map(Event::Art));
        fields.into()
    }

    fn review_view(&self, palette: Palette, key_available: bool) -> Element<'_, Event> {
        let Some(review) = &self.review else {
            return text("Preparing review...").into();
        };
        let destination = if self.adopt {
            format!("Use in place: {}", review.inspection.source_root.display())
        } else {
            format!("Copy to: {}", review.destination.display())
        };
        let mut content = column![
            text(format!("Import {}", self.name)).size(18).color(palette.foreground),
            text(format!("Source: {}", review.inspection.source_root.display())).color(palette.foreground),
            text(destination).color(palette.foreground),
            text(format!("Runner: {} -> {}", self.runner, review.runner_name)).color(palette.foreground),
            text(format!("Layout: {:?}, {}", review.inspection.layout, review.inspection.arch)).color(palette.foreground),
            text("The source game and external launcher must stay idle during import. External game files and saves stay where they are.").size(13).color(palette.warning),
        ].spacing(10);
        if review.arch_missing {
            content = content
                .push(text("Architecture was not detected. Choose one:").color(palette.warning));
            content = content.push(
                pick_list(vec![Arch::Win64, Arch::Win32], self.arch, |arch| {
                    Event::Arch(Some(arch))
                })
                .placeholder("Choose architecture")
                .wineo_style(palette),
            );
        }
        if !review.inspection.external_dependencies.is_empty() {
            let count = review.inspection.external_dependencies.len();
            content = content.push(
                secondary_button(
                    if self.show_dependencies {
                        format!("Hide {count} external dependencies")
                    } else {
                        format!("Show all {count} external dependencies")
                    },
                    palette,
                )
                .on_press(Event::Dependencies(!self.show_dependencies)),
            );
            if self.show_dependencies {
                for dependency in &review.inspection.external_dependencies {
                    content = content.push(
                        text(format!(
                            "{} -> {}",
                            dependency.source.display(),
                            dependency.target.display()
                        ))
                        .size(13)
                        .color(palette.muted),
                    );
                }
            }
        }
        content = content.push(self.art.view(palette, key_available).map(Event::Art));
        if self.recovery_path.is_some() {
            content = content.push(text("A complete copy was retained. Review it with Use in place to register that folder.").size(13).color(palette.warning));
        }
        content.into()
    }

    fn begin_review(&mut self) -> Task<Event> {
        if self.step == Step::Working {
            return Task::none();
        }
        let source = self.source.clone();
        let name = self.name.clone();
        let runner = self.runner.clone();
        if source.trim().is_empty() || name.trim().is_empty() || runner.is_empty() {
            self.error = Some("Choose a source, name, and runner before reviewing.".to_owned());
            return Task::none();
        }
        self.error = None;
        self.step = Step::Working;
        self.status = "Inspecting the prefix...".to_owned();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let adopt = self.adopt;
        let destination = self.destination.clone();
        let arch = self.arch;
        let task = task::blocking(
            move || inspect_review(&paths, &source, &name, &runner, adopt, &destination, arch),
            move |result| Event::ReviewLoaded(generation, result.map(Box::new)),
        );
        let art = self
            .art
            .configure(self.paths.clone(), self.name.clone())
            .map(Event::Art);
        Task::batch([task, art])
    }

    fn begin_import(&mut self) -> Task<Event> {
        if self.step != Step::Review || self.busy() {
            return Task::none();
        }
        let Some(review) = self.review.clone() else {
            return Task::none();
        };
        if review.arch_missing && self.arch.is_none() {
            self.error = Some("Choose the prefix architecture before importing.".to_owned());
            return Task::none();
        }
        if let Err(error) = self.art.validate() {
            self.error = Some(error);
            return Task::none();
        }
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Working;
        self.status = "Importing prefix...".to_owned();
        self.error = None;
        self.warning = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let request = ImportRequest {
            source: review.inspection.source_root.clone(),
            name: self.name.clone(),
            runner: self.runner.clone(),
            destination: (!self.adopt).then(|| review.destination.clone()),
            adopt: self.adopt,
            arch: self.arch,
        };
        let paths = self.paths.clone();
        let art = self.art.clone();
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        let progress_generation = generation;
        let work = Task::perform(
            async move {
                task::join(
                    move || {
                        let global = GlobalConfig::load(&paths)?;
                        let imported = import::execute_reviewed(
                            &paths,
                            &global,
                            request,
                            Some(&review.inspection),
                            None,
                            &token,
                            {
                                let mut copied = 0_u64;
                                move |progress| {
                                    if let import::ImportProgress::Warning(warning) = &progress {
                                        let _ = sender.unbounded_send(Event::Warning(
                                            progress_generation,
                                            warning.clone(),
                                        ));
                                    }
                                    if let import::ImportProgress::Copied { .. } = progress {
                                        copied += 1;
                                        if copied.is_multiple_of(250) {
                                            let _ = sender.unbounded_send(Event::Progress(
                                                progress_generation,
                                                format!("Copied {copied} files..."),
                                            ));
                                        }
                                    }
                                }
                            },
                        )?;
                        let art_error = apply_art(&paths, &imported.name, &art).err();
                        Ok::<_, anyhow::Error>(ImportOutcome {
                            name: imported.name,
                            art_error,
                        })
                    },
                    anyhow::Error::from,
                )
                .await
                .map_err(|error| error.to_string())
            },
            move |result| Event::ImportFinished(generation, result.map(Box::new)),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn retry_art(&mut self) -> Task<Event> {
        if self.step != Step::ArtFailed || self.busy() {
            return Task::none();
        }
        let Some(name) = self.imported_name.clone() else {
            return Task::none();
        };
        self.step = Step::Working;
        self.status = "Copying artwork...".to_owned();
        self.error = None;
        self.token = Some(Token::new());
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let art = self.art.clone();
        task::blocking(
            move || apply_art(&paths, &name, &art),
            move |result| Event::ArtRetried(generation, result),
        )
    }

    fn skip_art(&mut self) -> Task<Event> {
        if self.step != Step::ArtFailed || self.busy() {
            return Task::none();
        }
        let Some(name) = self.imported_name.clone() else {
            return Task::none();
        };
        self.step = Step::Working;
        self.status = "Refreshing desktop entries...".to_owned();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        task::blocking(
            move || {
                wineo_core::desktop::apply(&paths, &name, false)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            },
            move |result| Event::ArtSkipped(generation, result),
        )
    }

    fn clear_review(&mut self) {
        self.review = None;
        self.recovery_path = None;
        if self.step == Step::Review {
            self.step = Step::Details;
        }
        self.error = None;
    }
}

fn runner_choices(paths: &Paths) -> Result<RunnerListing, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let runners = Runners::new(paths, &global).map_err(|error| error.to_string())?;
    let listing = runners.list().map_err(|error| error.to_string())?;
    let mut choices: Vec<_> = runners
        .candidates(&listing)
        .into_iter()
        .filter_map(|candidate| candidate.runner.ok())
        .map(|runner| choice(paths, runner))
        .collect();
    choices.sort_by(|a, b| a.selection.cmp(&b.selection));
    choices.dedup_by(|a, b| a.selection == b.selection);
    let default = choices
        .iter()
        .find(|runner| runner.selection == global.defaults.runner)
        .or_else(|| choices.first())
        .map(|runner| runner.selection.clone())
        .unwrap_or_default();
    Ok(RunnerListing { choices, default })
}

fn choice(paths: &Paths, runner: Runner) -> RunnerChoice {
    RunnerChoice {
        selection: runner.selection().to_owned(),
        name: runner.name().to_owned(),
        proton: runner.is_proton(),
        launcher: runner_label::launcher(paths, &runner),
    }
}

fn apply_art(paths: &Paths, name: &str, art: &super::artwork::Draft) -> Result<(), String> {
    art.apply(paths, name)?;
    wineo_core::desktop::apply(paths, name, false)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn inspect_review(
    paths: &Paths,
    source: &str,
    name: &str,
    runner_selection: &str,
    adopt: bool,
    destination: &str,
    arch: Option<Arch>,
) -> Result<Review, String> {
    let source = paths.absolute(source).map_err(|error| error.to_string())?;
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let runner = Runners::new(paths, &global)
        .and_then(|runners| runners.resolve(runner_selection))
        .map_err(|error| error.to_string())?;
    let inspection = match import::inspect(&source, Some(&runner), arch) {
        Ok(inspection) => (inspection, false),
        Err(error) if error.to_string().contains("#arch marker") && arch.is_none() => {
            let inspection = import::inspect(&source, Some(&runner), Some(Arch::Win64))
                .map_err(|error| error.to_string())?;
            (inspection, true)
        }
        Err(error) => return Err(error.to_string()),
    };
    let request = ImportRequest {
        source,
        name: name.to_owned(),
        runner: runner_selection.to_owned(),
        destination: if adopt || destination.trim().is_empty() {
            None
        } else {
            Some(
                paths
                    .absolute(destination)
                    .map_err(|error| error.to_string())?,
            )
        },
        adopt,
        arch,
    };
    let destination = import::preflight(paths, &global, &request, &inspection.0)
        .map_err(|error| error.to_string())?;
    Ok(Review {
        inspection: inspection.0,
        destination,
        runner_name: runner_label::labeled(runner.name(), runner_label::launcher(paths, &runner)),
        arch_missing: inspection.1,
    })
}

fn discard_prompt(palette: Palette) -> Element<'static, Event> {
    container(
        column![
            text("Discard this import?")
                .size(18)
                .color(palette.foreground),
            text("Your prefix, name, and artwork selections will be lost.").color(palette.muted),
            row![
                secondary_button("Keep editing", palette).on_press(Event::KeepEditing),
                secondary_button("Discard", palette).on_press(Event::Discard)
            ]
            .spacing(8),
        ]
        .spacing(12),
    )
    .padding(24)
    .style(move |_| {
        container::Style::default()
            .background(palette.surface)
            .border(border::rounded(10).width(1).color(palette.border))
    })
    .center(Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use crate::test_script::write_script;
    use std::ffi::OsString;
    use std::fs;

    use std::path::Path;

    use super::*;

    fn paths(root: &Path) -> Paths {
        let home = root.to_owned();
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap()
    }

    fn fake_runner(paths: &Paths) {
        let root = paths.default_runner_root().join("fake-wine").join("bin");
        fs::create_dir_all(&root).unwrap();
        for program in ["wine", "wineserver"] {
            let path = root.join(program);
            write_script(&path, "#!/bin/sh\nexit 0\n");
        }
    }

    fn prefix(root: &Path, arch: Option<Arch>) -> PathBuf {
        let prefix = root.join("source-prefix");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        let marker = arch.map_or(String::new(), |arch| format!("#arch={}\n", arch.as_str()));
        fs::write(
            prefix.join("system.reg"),
            format!("WINE REGISTRY Version 2\n{marker}"),
        )
        .unwrap();
        prefix
    }

    fn request(review: &Review) -> ImportRequest {
        ImportRequest {
            source: review.inspection.source_root.clone(),
            name: "imported".to_owned(),
            runner: "fake-wine".to_owned(),
            destination: Some(review.destination.clone()),
            adopt: false,
            arch: Some(review.inspection.arch),
        }
    }

    #[test]
    fn review_is_read_only_and_stale_source_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_runner(&paths);
        let source = prefix(root.path(), Some(Arch::Win64));
        let review = inspect_review(
            &paths,
            source.to_str().unwrap(),
            "imported",
            "fake-wine",
            false,
            "",
            None,
        )
        .unwrap();
        assert_eq!(review.inspection.layout, import::ImportLayout::Wine);
        assert!(!review.destination.exists());
        assert!(!paths.environment_config("imported").exists());

        fs::write(
            source.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\nchanged\n",
        )
        .unwrap();
        let error = import::execute_reviewed(
            &paths,
            &GlobalConfig::default(),
            request(&review),
            Some(&review.inspection),
            None,
            &Token::new(),
            |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("changed"));
        assert!(!review.destination.exists());
        assert!(!paths.environment_config("imported").exists());
    }

    #[test]
    fn canceled_copy_does_not_register_or_publish_prefix() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_runner(&paths);
        let source = prefix(root.path(), Some(Arch::Win64));
        let review = inspect_review(
            &paths,
            source.to_str().unwrap(),
            "imported",
            "fake-wine",
            false,
            "",
            None,
        )
        .unwrap();
        let token = Token::new();
        token.cancel();
        let result = import::execute_reviewed(
            &paths,
            &GlobalConfig::default(),
            request(&review),
            Some(&review.inspection),
            None,
            &token,
            |_| {},
        );
        assert!(result.is_err());
        assert!(!review.destination.exists());
        assert!(!paths.environment_config("imported").exists());
    }

    #[test]
    fn missing_architecture_is_requested_only_when_inspection_cannot_detect_it() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_runner(&paths);
        let source = prefix(root.path(), None);
        let review = inspect_review(
            &paths,
            source.to_str().unwrap(),
            "imported",
            "fake-wine",
            true,
            "",
            None,
        )
        .unwrap();
        assert!(review.arch_missing);
        assert_eq!(review.inspection.arch, Arch::Win64);
        assert!(!paths.environment_config("imported").exists());
    }

    #[test]
    fn submit_uses_the_visible_primary_action_and_ignores_busy_pages() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_runner(&paths);
        let source = prefix(root.path(), Some(Arch::Win64));
        let mut page = Page::new(paths).0;
        page.source = source.display().to_string();
        page.name = "imported".to_owned();
        page.runner = "fake-wine".to_owned();
        let _task = page.submit();
        assert_eq!(page.step, Step::Working);
        drop(_task);

        let review = inspect_review(
            &page.paths,
            &page.source,
            &page.name,
            &page.runner,
            false,
            "",
            None,
        )
        .unwrap();
        page.step = Step::Review;
        page.review = Some(review);
        let _task = page.submit();
        assert_eq!(page.status, "Importing prefix...");
        drop(_task);

        page.action = None;
        let status = page.status.clone();
        let _task = page.submit();
        assert!(page.action.is_none());
        assert_eq!(page.status, status);
    }

    #[test]
    fn back_is_at_the_top_and_the_forward_action_is_pinned_at_the_bottom() {
        use crate::layout_probe::{
            assert_back_on_top_and_forward_pinned, bounds_at, click_at, probe,
        };

        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_runner(&paths);
        let source = prefix(root.path(), Some(Arch::Win64));
        let mut page = Page::new(paths).0;
        page.source = source.display().to_string();
        page.name = "imported".to_owned();
        page.runner = "fake-wine".to_owned();
        let review = inspect_review(
            &page.paths,
            &page.source,
            &page.name,
            &page.runner,
            false,
            "",
            None,
        )
        .unwrap();
        for (label, step, retained) in [
            ("details", Step::Details, false),
            ("review", Step::Review, false),
            ("review with a retained copy", Step::Review, true),
        ] {
            page.step = step;
            page.review = Some(review.clone());
            page.recovery_path = retained.then(|| root.path().join("retained"));
            for (width, height) in [(280.0, 240.0), (900.0, 240.0), (900.0, 720.0)] {
                let inset = iced::Rectangle::new(
                    iced::Point::new(24.0, 24.0),
                    iced::Size::new(width - 48.0, height - 48.0),
                );
                let mut view = page.view_with_key_availability(Palette::DARK, false);
                let probe = probe(&mut view, iced::Size::new(width, height));
                let case = format!("{label} at {width}x{height}");
                assert_back_on_top_and_forward_pinned(&probe, inset, &case);
                let spilled = probe.outside(inset);
                assert!(spilled.is_empty(), "{case}: {spilled:?}");
            }
            let size = iced::Size::new(900.0, 720.0);
            let view = || page.view_with_key_availability(Palette::DARK, false);
            let forward = if step == Step::Details {
                "Next"
            } else {
                "Import"
            };
            assert_eq!(click_at(view(), size, &[0, 0, 0]), ["Back"], "{label}");
            if !retained {
                assert_eq!(click_at(view(), size, &[0, 2, 0]), [forward], "{label}");
            }
            if retained {
                // Beside Import at a normal width, wrapped under it when narrow.
                for (width, wrapped) in [(900.0, false), (280.0, true)] {
                    let mut strip = view();
                    let size = iced::Size::new(width, 720.0);
                    let probe = probe(&mut strip, size);
                    let import = bounds_at(&probe.node, &[0, 2, 0, 0]);
                    let kept = bounds_at(&probe.node, &[0, 2, 0, 1]);
                    if wrapped {
                        assert!(
                            kept.y >= import.y + import.height - 0.01,
                            "{import:?} {kept:?}"
                        );
                    } else {
                        assert!(
                            kept.x >= import.x + import.width - 0.01,
                            "{import:?} {kept:?}"
                        );
                    }
                    assert_eq!(
                        click_at(view(), size, &[0, 2, 0, 0]),
                        ["Import"],
                        "{label} at {width}"
                    );
                    assert_eq!(
                        click_at(view(), size, &[0, 2, 0, 1]),
                        ["UseRetained"],
                        "{label} at {width}"
                    );
                }
            }
        }
    }
}
