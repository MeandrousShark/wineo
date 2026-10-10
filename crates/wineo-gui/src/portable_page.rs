use std::path::{Path, PathBuf};

use iced::widget::{column, container, row, scrollable, text, text_input};
use iced::{Element, Fill, Task};
use wineo_core::cancel::Token;
use wineo_core::config::{GlobalConfig, Kind};
use wineo_core::environment::Environment;
use wineo_core::import::ImportProgress;
use wineo_core::paths::Paths;
use wineo_core::portable::{self, CopyEvent, CopyInspection, NativeCopyInspection, Storage};
use wineo_core::runners::Runners;

use crate::chips::input_style;
use crate::theme::Palette;
use crate::ui::{accent_base, secondary_button};

#[derive(Debug, Clone)]
pub(crate) enum Event {
    SourceLoaded(Result<bool, String>),
    Folder(String),
    Browse,
    Picked(u64, Result<Option<PathBuf>, String>),
    Review,
    Submit,
    ReviewLoaded(u64, Result<Box<Review>, String>),
    Progress(u64, String),
    Publishing(u64),
    Copy,
    CopyFinished(u64, Result<(), String>),
    CopyAnother,
    Cancel,
    Back,
    KeepEditing,
    Discard,
}

impl Event {
    pub(crate) fn starts_work(&self) -> bool {
        matches!(self, Self::Review | Self::Copy | Self::Submit)
    }

    pub(crate) fn is_input(&self) -> bool {
        matches!(
            self,
            Self::Folder(_)
                | Self::Browse
                | Self::Review
                | Self::Submit
                | Self::Copy
                | Self::CopyAnother
                | Self::Cancel
                | Self::Back
                | Self::KeepEditing
                | Self::Discard
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Review {
    Wine(Box<CopyInspection>),
    Native(Box<NativeCopyInspection>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Details,
    Inspecting,
    Review,
    Copying,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Back,
}

pub(crate) struct Page {
    paths: Paths,
    source: String,
    folder: String,
    native: Option<bool>,
    source_error: Option<String>,
    review: Option<Review>,
    step: Step,
    generation: u64,
    token: Option<Token>,
    status: String,
    error: Option<String>,
    dirty: bool,
    discard_prompt: bool,
    finishing: bool,
    action: Option<Action>,
    outcome: Option<String>,
}

impl Page {
    pub(crate) fn new(paths: Paths, name: String) -> (Self, Task<Event>) {
        let page = Self {
            paths: paths.clone(),
            source: name.clone(),
            folder: String::new(),
            native: None,
            source_error: None,
            review: None,
            step: Step::Details,
            generation: 0,
            token: None,
            status: String::new(),
            error: None,
            dirty: false,
            discard_prompt: false,
            finishing: false,
            action: None,
            outcome: None,
        };
        let load = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    Environment::load(&paths, &name)
                        .map_err(|error| error.to_string())
                        .and_then(|environment| match &environment.config.kind {
                            Kind::Wine(_) => Ok(false),
                            Kind::Native => Ok(true),
                            Kind::Proxied(_) => {
                                Err(environment.config.no_prefix(&name).to_string())
                            }
                        })
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            Event::SourceLoaded,
        );
        (page, load)
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Submit => self.submit(),
            Event::SourceLoaded(Ok(native)) => {
                self.native = Some(native);
                Task::none()
            }
            Event::SourceLoaded(Err(error)) => {
                self.source_error = Some(error);
                Task::none()
            }
            Event::Folder(folder) => {
                if folder != self.folder {
                    self.folder = folder;
                    self.dirty = true;
                    self.clear_review();
                }
                Task::none()
            }
            Event::Browse => {
                self.generation = self.generation.wrapping_add(1);
                let generation = self.generation;
                Task::perform(
                    crate::picker::pick("Choose a portable folder".to_owned(), true),
                    move |result| Event::Picked(generation, result),
                )
            }
            Event::Picked(generation, _) if generation != self.generation => Task::none(),
            Event::Picked(_, Ok(Some(path))) => {
                self.folder = path.display().to_string();
                self.dirty = true;
                self.clear_review();
                Task::none()
            }
            Event::Picked(_, Ok(None)) => Task::none(),
            Event::Picked(_, Err(error)) => {
                self.error = Some(error);
                Task::none()
            }
            Event::Review => self.begin_review(),
            Event::ReviewLoaded(generation, result) => {
                if generation != self.generation || self.step != Step::Inspecting {
                    return Task::none();
                }
                self.token = None;
                self.outcome = Some(self.source.clone());
                match result {
                    Ok(review) => {
                        self.review = Some(*review);
                        self.step = Step::Review;
                        self.error = None;
                    }
                    Err(error) => {
                        self.step = Step::Details;
                        self.error = Some(error);
                    }
                }
                Task::none()
            }
            Event::Progress(generation, status) => {
                if generation == self.generation
                    && matches!(self.step, Step::Inspecting | Step::Copying)
                {
                    self.status = status;
                }
                Task::none()
            }
            Event::Publishing(generation) => {
                if generation == self.generation && self.step == Step::Copying {
                    self.finishing = true;
                    self.status = "Finishing changes...".into();
                }
                Task::none()
            }
            Event::Copy => self.begin_copy(),
            Event::CopyAnother => {
                if self.step == Step::Finished {
                    self.folder.clear();
                    self.review = None;
                    self.step = Step::Details;
                    self.status.clear();
                    self.error = None;
                    self.dirty = false;
                    self.outcome = None;
                }
                Task::none()
            }
            Event::CopyFinished(generation, result) => {
                if generation != self.generation || self.step != Step::Copying {
                    return Task::none();
                }
                self.token = None;
                self.finishing = false;
                self.outcome = Some(self.source.clone());
                match result {
                    Ok(()) => {
                        self.step = Step::Finished;
                        self.error = None;
                    }
                    Err(error) => {
                        self.step = Step::Review;
                        self.error = Some(error);
                    }
                }
                Task::none()
            }
            Event::Cancel => {
                self.cancel();
                Task::none()
            }
            Event::Back => self.back(),
            Event::KeepEditing => {
                self.discard_prompt = false;
                Task::none()
            }
            Event::Discard => {
                self.discard_prompt = false;
                self.folder.clear();
                self.dirty = false;
                self.clear_review();
                self.action = Some(Action::Back);
                Task::none()
            }
        }
    }

    pub(crate) fn view(&self, palette: Palette, busy: bool) -> Element<'_, Event> {
        let can_edit = !busy && !self.busy() && self.paths.portable_root().is_none();
        let body = match self.step {
            Step::Details => self.details(palette, can_edit),
            Step::Inspecting | Step::Copying => column![
                text(self.status.as_str()).color(palette.muted),
                secondary_button(
                    if self.finishing {
                        "Finishing changes..."
                    } else {
                        "Cancel"
                    },
                    palette
                )
                .on_press_maybe((!self.finishing).then_some(Event::Cancel)),
            ]
            .spacing(14)
            .into(),
            Step::Review => self.review_view(palette, !busy),
            Step::Finished => self.finished_view(palette),
        };
        let mut content = column![
            text("Copy to portable folder")
                .size(22)
                .color(palette.foreground),
            body
        ]
        .spacing(18)
        .max_width(820);
        if let Some(error) = &self.source_error {
            content = content.push(text(error.as_str()).size(13).color(palette.danger));
        }
        if let Some(error) = &self.error {
            content = content.push(text(error.as_str()).size(13).color(palette.danger));
        }
        let base: Element<'_, Event> = container(scrollable(content).height(Fill))
            .padding(24)
            .width(Fill)
            .height(Fill)
            .into();
        if self.discard_prompt {
            iced::widget::stack![base, discard_prompt(palette)].into()
        } else {
            base
        }
    }

    pub(crate) fn busy(&self) -> bool {
        matches!(self.step, Step::Inspecting | Step::Copying)
    }

    pub(crate) fn finishing(&self) -> bool {
        self.finishing
    }

    pub(crate) fn cancel(&mut self) {
        if self.finishing {
            return;
        }
        if let Some(token) = &self.token {
            token.cancel();
            self.status = "Canceling...".into();
        }
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        match self.step {
            Step::Details => self.begin_review(),
            Step::Review => self.begin_copy(),
            Step::Finished => {
                self.action = Some(Action::Back);
                Task::none()
            }
            Step::Inspecting | Step::Copying => Task::none(),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        match self.step {
            Step::Details => {
                if self.dirty {
                    self.discard_prompt = true;
                } else {
                    self.action = Some(Action::Back);
                }
                Task::none()
            }
            Step::Review => {
                self.step = Step::Details;
                self.error = None;
                Task::none()
            }
            Step::Finished => {
                self.action = Some(Action::Back);
                Task::none()
            }
            Step::Inspecting | Step::Copying => Task::none(),
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    pub(crate) fn take_outcome(&mut self) -> Option<String> {
        self.outcome.take()
    }

    pub(crate) fn success_message(&self) -> Option<String> {
        (self.step == Step::Finished).then(|| {
            let folder = self
                .review
                .as_ref()
                .map(review_folder)
                .unwrap_or_else(|| self.folder.clone());
            format!("Copied {} to {}.", self.source, folder)
        })
    }

    fn details(&self, palette: Palette, enabled: bool) -> Element<'_, Event> {
        let source_root = self.paths.portable_root().is_some();
        let mut fields = column![
            text(format!("Source: {}", self.source)).color(palette.muted),
            text("Portable folder").color(palette.foreground),
            row![
                text_input("Choose a folder or enter a path", &self.folder)
                    .on_input_maybe(enabled.then_some(Event::Folder))
                    .width(Fill)
                    .padding(8)
                    .style(move |_, status| input_style(palette, status)),
                secondary_button("Browse...", palette)
                    .on_press_maybe(enabled.then_some(Event::Browse)),
            ]
            .spacing(8),
        ]
        .spacing(8);
        if source_root {
            fields = fields.push(
                text("Copy from the standard library.")
                    .size(13)
                    .color(palette.warning),
            );
        }
        fields = fields.push(
            row![
                secondary_button("Back", palette)
                    .on_press_maybe((!self.busy()).then_some(Event::Back)),
                accent_base("Review", palette.primary).on_press_maybe(
                    (enabled && self.native.is_some() && self.source_error.is_none())
                        .then_some(Event::Review)
                ),
            ]
            .spacing(8),
        );
        fields.into()
    }

    fn review_view(&self, palette: Palette, enabled: bool) -> Element<'_, Event> {
        let mut content = column![
            text(format!("Copy {}", self.source))
                .size(18)
                .color(palette.foreground)
        ]
        .spacing(8);
        if let Some(review) = &self.review {
            content = push_review(content, review, palette);
        } else {
            content = content.push(text("Preparing review...").color(palette.muted));
        }
        content = content.push(
            row![
                secondary_button("Back", palette).on_press_maybe(enabled.then_some(Event::Back)),
                secondary_button("Review again", palette)
                    .on_press_maybe(enabled.then_some(Event::Review)),
                accent_base("Copy", palette.primary)
                    .on_press_maybe((enabled && self.review.is_some()).then_some(Event::Copy)),
            ]
            .spacing(8),
        );
        content.into()
    }

    fn finished_view(&self, palette: Palette) -> Element<'_, Event> {
        let folder = self
            .review
            .as_ref()
            .map(review_folder)
            .unwrap_or_else(|| self.folder.clone());
        let mut content = column![
            text(format!("Copied {} to the portable folder.", self.source)).color(palette.success),
            text(folder).color(palette.foreground),
            text("The source environment is unchanged.").color(palette.muted),
        ]
        .spacing(8);
        if let Some(review) = &self.review {
            content = push_warnings(content, review, palette);
        }
        content = content.push(
            row![
                secondary_button("Copy to another folder...", palette).on_press(Event::CopyAnother),
                secondary_button("Back", palette).on_press(Event::Back),
            ]
            .spacing(8),
        );
        content.into()
    }

    fn begin_review(&mut self) -> Task<Event> {
        if self.busy() || self.paths.portable_root().is_some() {
            return Task::none();
        }
        if self.source_error.is_some() || self.native.is_none() {
            self.error = Some("The source environment could not be loaded.".into());
            return Task::none();
        }
        if self.folder.trim().is_empty() {
            self.error = Some("Choose a portable folder.".into());
            return Task::none();
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let folder = self.folder.trim().to_owned();
        let source = self.source.clone();
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Inspecting;
        self.status = "Waiting for the source to become idle...".into();
        self.error = None;
        self.outcome = None;
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        let progress_generation = generation;
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let to = explicit_destination(&folder)?;
                    inspect_copy(&paths, &to, &source, &token, |progress| {
                        send_progress(&sender, progress_generation, progress);
                    })
                })
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result)
                .map_err(|error| error.to_string())
            },
            move |result| Event::ReviewLoaded(generation, result.map(Box::new)),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn begin_copy(&mut self) -> Task<Event> {
        if self.step != Step::Review || self.busy() {
            return Task::none();
        }
        let Some(review) = self.review.clone() else {
            return Task::none();
        };
        let folder = review_folder(&review);
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Copying;
        self.status = "Checking that the review is still current...".into();
        self.error = None;
        self.finishing = false;
        self.outcome = None;
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        let progress_generation = generation;
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let to = explicit_destination(&folder)?;
                    let mut copied = 0_u64;
                    match review {
                        Review::Wine(review) => {
                            let global = GlobalConfig::load(&paths)?;
                            portable::execute_reviewed_with(
                                &paths,
                                &to,
                                &global,
                                &review,
                                &token,
                                |event| {
                                    send_copy_event(
                                        &sender,
                                        progress_generation,
                                        event,
                                        &mut copied,
                                    )
                                },
                            )?;
                        }
                        Review::Native(review) => {
                            portable::execute_native_with(&paths, &to, &review, &token, |event| {
                                send_copy_event(&sender, progress_generation, event, &mut copied)
                            })?;
                        }
                    }
                    Ok::<_, anyhow::Error>(())
                })
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result)
                .map_err(|error| error.to_string())
            },
            move |result| Event::CopyFinished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn clear_review(&mut self) {
        self.review = None;
        if self.step == Step::Review {
            self.step = Step::Details;
        }
        self.error = None;
    }
}

fn explicit_destination(folder: &str) -> anyhow::Result<Paths> {
    Ok(Paths::from_env(Some(Path::new(folder)))?)
}

fn inspect_copy(
    paths: &Paths,
    to: &Paths,
    source: &str,
    token: &Token,
    progress: impl FnMut(ImportProgress),
) -> anyhow::Result<Review> {
    let environment = Environment::load(paths, source)?;
    if let Kind::Proxied(_) = environment.config.kind {
        return Err(environment.config.no_prefix(source).into());
    }
    if let Some(wine) = environment.config.wine() {
        let global = GlobalConfig::load(paths)?;
        let (runner, _build_guard) =
            Runners::new(paths, &global)?.resolve_locked(paths, &wine.runner)?;
        let prefix = environment.prefix(paths)?;
        runner.wait_idle_for_copy(&prefix, token)?;
        if token.is_canceled() {
            anyhow::bail!("portable copy canceled while waiting for the source to become idle");
        }
        Ok(Review::Wine(Box::new(portable::inspect_with(
            paths, to, &global, source, token, progress,
        )?)))
    } else {
        Ok(Review::Native(Box::new(portable::inspect_native_with(
            paths, to, source, token, progress,
        )?)))
    }
}

fn send_progress(
    sender: &iced::futures::channel::mpsc::UnboundedSender<Event>,
    generation: u64,
    progress: ImportProgress,
) {
    let status = match progress {
        ImportProgress::Checking { entries } if entries.is_multiple_of(250) => {
            Some(format!("Checking source files... {entries} checked"))
        }
        ImportProgress::Checking { .. } => None,
        ImportProgress::Started { entries } => Some(format!("Copying {entries} files...")),
        ImportProgress::Copied { .. } => None,
        ImportProgress::Finished { entries } => Some(format!("Checked {entries} files.")),
    };
    if let Some(status) = status {
        let _ = sender.unbounded_send(Event::Progress(generation, status));
    }
}

fn send_copy_event(
    sender: &iced::futures::channel::mpsc::UnboundedSender<Event>,
    generation: u64,
    event: CopyEvent,
    copied: &mut u64,
) {
    match event {
        CopyEvent::Progress(ImportProgress::Copied { .. }) => {
            *copied += 1;
            if (*copied).is_multiple_of(100) {
                let _ = sender.unbounded_send(Event::Progress(
                    generation,
                    format!("Copied {copied} files..."),
                ));
            }
        }
        CopyEvent::Progress(ImportProgress::Finished { entries }) => {
            let _ = sender.unbounded_send(Event::Progress(
                generation,
                format!("Copied {entries} files."),
            ));
        }
        CopyEvent::Progress(progress) => send_progress(sender, generation, progress),
        CopyEvent::Publishing => {
            let _ = sender.unbounded_send(Event::Publishing(generation));
        }
    }
}

fn push_review<'a>(
    mut content: iced::widget::Column<'a, Event>,
    review: &Review,
    palette: Palette,
) -> iced::widget::Column<'a, Event> {
    match review {
        Review::Wine(review) => {
            content = content
                .push(label(
                    format!("Source prefix: {}", review.source_root.display()),
                    palette,
                ))
                .push(label(
                    format!("Portable folder: {}", review.folder.display()),
                    palette,
                ))
                .push(label(
                    format!("Destination prefix: {}", review.destination.display()),
                    palette,
                ))
                .push(label(
                    format!("Runner: {} | layout: {:?}", review.runner, review.layout),
                    palette,
                ))
                .push(label(
                    format!("Targets: {}", list_or_none(&review.targets)),
                    palette,
                ))
                .push(label(
                    format!("Snapshots: {}", list_or_none(&review.snapshots)),
                    palette,
                ))
                .push(label("Art and playtime are copied.".into(), palette))
                .push(label("Desktop entries stay disabled.".into(), palette));
            content = push_copy_policy(
                content,
                review.first_copy,
                review.copies_global_config,
                &review.copied_templates,
                palette,
            );
            match review.storage {
                Storage::Reflink => {}
                Storage::FullCopy => {
                    content = content.push(
                        label("This copy takes full space.".into(), palette).color(palette.warning),
                    )
                }
                Storage::Unknown => {
                    content = content.push(
                        label("Reflink support is unknown.".into(), palette).color(palette.warning),
                    )
                }
            }
            if !review.snapshots.is_empty() {
                content = content.push(label(
                    "Copied snapshots need reflinks for future create or restore.".into(),
                    palette,
                ));
            }
            content = push_values(
                content,
                "Changed settings",
                &review.changed_settings,
                palette,
            );
            content = push_values(
                content,
                "External target paths",
                &review.external_target_paths,
                palette,
            );
            content = push_values(
                content,
                "Literal source paths",
                &review.literal_source_paths,
                palette,
            );
            content = push_values(
                content,
                "External files and links (they stay at the source)",
                &review
                    .external_dependencies
                    .iter()
                    .map(|link| format!("{} -> {}", link.source.display(), link.target.display()))
                    .collect::<Vec<_>>(),
                palette,
            );
            content = push_values(
                content,
                "External links in snapshots",
                &review
                    .snapshot_links
                    .iter()
                    .map(|link| format!("{} -> {}", link.source.display(), link.target.display()))
                    .collect::<Vec<_>>(),
                palette,
            );
        }
        Review::Native(review) => {
            content = content
                .push(label(format!("Source: {}", review.name), palette))
                .push(label(
                    format!("Portable folder: {}", review.folder.display()),
                    palette,
                ))
                .push(label(
                    format!("Targets: {}", list_or_none(&review.targets)),
                    palette,
                ))
                .push(
                    label("Program files stay where they are.".into(), palette)
                        .color(palette.warning),
                )
                .push(label("Art and playtime are copied.".into(), palette))
                .push(label("Desktop entries stay disabled.".into(), palette));
            content = push_copy_policy(
                content,
                review.first_copy,
                review.copies_global_config,
                &review.copied_templates,
                palette,
            );
            content = push_values(
                content,
                "Changed settings",
                &review.changed_settings,
                palette,
            );
            content = push_values(
                content,
                "External target paths",
                &review.external_target_paths,
                palette,
            );
        }
    }
    content
}

fn push_copy_policy<'a>(
    mut content: iced::widget::Column<'a, Event>,
    first_copy: bool,
    copies_global_config: bool,
    templates: &[String],
    palette: Palette,
) -> iced::widget::Column<'a, Event> {
    if first_copy {
        content = content.push(label(
            if copies_global_config {
                "First copy: the source global config comes along."
            } else {
                "First copy: the portable folder has no existing config directory."
            }
            .into(),
            palette,
        ));
        content = push_values(content, "Copied templates", templates, palette);
    } else {
        content = content.push(label(
            "The folder keeps its own global config and templates.".into(),
            palette,
        ));
    }
    content
}

fn push_warnings<'a>(
    mut content: iced::widget::Column<'a, Event>,
    review: &Review,
    palette: Palette,
) -> iced::widget::Column<'a, Event> {
    let warnings = match review {
        Review::Wine(review) => {
            let mut warnings = Vec::new();
            if review.storage == Storage::FullCopy {
                warnings.push("This copy took full space.".to_owned());
            } else if review.storage == Storage::Unknown {
                warnings.push("Reflink support was unknown during review.".to_owned());
            }
            if !review.external_dependencies.is_empty()
                || !review.external_target_paths.is_empty()
                || !review.literal_source_paths.is_empty()
            {
                warnings.push(
                    "Some source paths and external files still point to the original locations."
                        .to_owned(),
                );
            }
            if !review.snapshot_links.is_empty() {
                warnings.push(
                    "Some copied snapshot links still point outside the portable folder."
                        .to_owned(),
                );
            }
            warnings
        }
        Review::Native(review) => {
            let mut warnings = vec!["Program files stay where they are.".to_owned()];
            if !review.external_target_paths.is_empty() {
                warnings
                    .push("Some target paths still point outside the portable folder.".to_owned());
            }
            warnings
        }
    };
    for warning in warnings {
        content = content.push(label(warning, palette).color(palette.warning));
    }
    content
}

fn review_folder(review: &Review) -> String {
    match review {
        Review::Wine(review) => review.folder.display().to_string(),
        Review::Native(review) => review.folder.display().to_string(),
    }
}

fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "None".into()
    } else {
        values.join(", ")
    }
}

fn push_values<'a>(
    mut content: iced::widget::Column<'a, Event>,
    title: &str,
    values: &[String],
    palette: Palette,
) -> iced::widget::Column<'a, Event> {
    content = content.push(label(format!("{title}:"), palette));
    if values.is_empty() {
        content = content.push(label("None".into(), palette));
    } else {
        for value in values {
            content = content.push(label(value.clone(), palette));
        }
    }
    content
}

fn label<'a>(value: String, palette: Palette) -> iced::widget::Text<'a> {
    text(value).size(13).color(palette.foreground)
}

fn discard_prompt(palette: Palette) -> Element<'static, Event> {
    container(
        column![
            text("Discard portable copy details?")
                .size(18)
                .color(palette.foreground),
            text("The folder path will be cleared.").color(palette.muted),
            row![
                secondary_button("Keep editing", palette).on_press(Event::KeepEditing),
                secondary_button("Discard", palette).on_press(Event::Discard),
            ]
            .spacing(8),
        ]
        .spacing(12),
    )
    .padding(24)
    .style(move |_| {
        iced::widget::container::Style::default()
            .background(palette.surface)
            .border(iced::border::rounded(10).width(1).color(palette.border))
    })
    .center(Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn paths(root: &Path) -> Paths {
        let home = root.to_owned();
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap()
    }

    #[test]
    fn publishing_is_a_non_cancellable_boundary() {
        let root = tempfile::tempdir().unwrap();
        let mut page = Page {
            paths: paths(root.path()),
            source: "origin".into(),
            folder: String::new(),
            native: Some(false),
            source_error: None,
            review: None,
            step: Step::Copying,
            generation: 4,
            token: Some(Token::new()),
            status: String::new(),
            error: None,
            dirty: false,
            discard_prompt: false,
            finishing: false,
            action: None,
            outcome: None,
        };
        let token = page.token.as_ref().unwrap().clone();

        let _ = page.update(Event::Publishing(4));
        page.cancel();

        assert!(page.finishing());
        assert!(!token.is_canceled());
        assert_eq!(page.status, "Finishing changes...");
    }

    #[test]
    fn copy_completion_is_persistent_and_refreshable_on_failure() {
        let root = tempfile::tempdir().unwrap();
        let mut page = Page {
            paths: paths(root.path()),
            source: "origin".into(),
            folder: String::new(),
            native: Some(false),
            source_error: None,
            review: None,
            step: Step::Copying,
            generation: 7,
            token: Some(Token::new()),
            status: "Copying...".into(),
            error: None,
            dirty: false,
            discard_prompt: false,
            finishing: false,
            action: None,
            outcome: None,
        };

        let _ = page.update(Event::CopyFinished(
            7,
            Err("move the copied files aside".into()),
        ));

        assert_eq!(page.step, Step::Review);
        assert_eq!(page.error.as_deref(), Some("move the copied files aside"));
        assert_eq!(page.take_outcome().as_deref(), Some("origin"));
        assert_eq!(page.take_outcome(), None);
    }

    #[test]
    fn canceled_review_keeps_its_error_and_refreshes_the_source() {
        let root = tempfile::tempdir().unwrap();
        let mut page = Page {
            paths: paths(root.path()),
            source: "origin".into(),
            folder: "/tmp/portable".into(),
            native: Some(false),
            source_error: None,
            review: None,
            step: Step::Inspecting,
            generation: 8,
            token: Some(Token::new()),
            status: "Canceling...".into(),
            error: None,
            dirty: true,
            discard_prompt: false,
            finishing: false,
            action: None,
            outcome: None,
        };

        let _ = page.update(Event::ReviewLoaded(8, Err("import cancelled".into())));

        assert_eq!(page.step, Step::Details);
        assert_eq!(page.error.as_deref(), Some("import cancelled"));
        assert_eq!(page.take_outcome().as_deref(), Some("origin"));
    }

    #[test]
    fn stale_worker_events_do_not_change_the_current_page() {
        let root = tempfile::tempdir().unwrap();
        let mut page = Page {
            paths: paths(root.path()),
            source: "origin".into(),
            folder: "/tmp/portable".into(),
            native: Some(false),
            source_error: None,
            review: None,
            step: Step::Details,
            generation: 3,
            token: None,
            status: "Ready".into(),
            error: None,
            dirty: true,
            discard_prompt: false,
            finishing: false,
            action: None,
            outcome: None,
        };

        let _ = page.update(Event::Progress(2, "old status".into()));
        let _ = page.update(Event::CopyFinished(2, Ok(())));

        assert_eq!(page.status, "Ready");
        assert_eq!(page.step, Step::Details);
        assert_eq!(page.take_outcome(), None);
    }

    #[test]
    fn finished_page_keeps_result_until_another_copy_is_requested() {
        let root = tempfile::tempdir().unwrap();
        let mut page = Page {
            paths: paths(root.path()),
            source: "origin".into(),
            folder: "/tmp/first-copy".into(),
            native: Some(false),
            source_error: None,
            review: None,
            step: Step::Finished,
            generation: 2,
            token: None,
            status: String::new(),
            error: None,
            dirty: false,
            discard_prompt: false,
            finishing: false,
            action: None,
            outcome: Some("origin".into()),
        };

        let _ = page.update(Event::Back);
        assert_eq!(page.step, Step::Finished);
        assert_eq!(page.outcome.as_deref(), Some("origin"));

        let _ = page.update(Event::CopyAnother);

        assert_eq!(page.step, Step::Details);
        assert_eq!(page.source, "origin");
        assert!(page.folder.is_empty());
        assert_eq!(page.outcome, None);
    }
}
