use std::path::PathBuf;

use iced::widget::{column, container, row, text, text_input};
use iced::{Element, Fill, Task, border};
use wineo_core::cancel::Token;
use wineo_core::clone::{self, CloneInspection, NativeCloneInspection};
use wineo_core::config::{GlobalConfig, Kind};
use wineo_core::environment::Environment;
use wineo_core::import::{self, ImportProgress};
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;

use super::frame;
use crate::chips::input_style;
use crate::task;
use crate::theme::Palette;
use crate::ui::{accent_base, secondary_button};

#[derive(Debug, Clone)]
pub(crate) enum Event {
    SourceLoaded(Result<bool, String>),
    Name(String),
    Destination(String),
    PickDestination,
    DestinationPicked(u64, Result<Option<PathBuf>, String>),
    Review,
    ReviewLoaded(u64, Result<Box<Review>, String>),
    Progress(u64, String),
    Warning(u64, String),
    Clone,
    CloneFinished(u64, Result<String, String>),
    Dependencies(bool),
    KeepEditing,
    Discard,
    Back,
    Library,
    Cancel,
    ForceStop,
}

#[derive(Debug, Clone)]
pub(crate) enum Review {
    Wine(Box<CloneInspection>),
    Native(Box<NativeCloneInspection>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Details,
    Review,
    Working,
    Finished,
}

pub(crate) struct Page {
    paths: Paths,
    source: String,
    name: String,
    destination: String,
    native: Option<bool>,
    review: Option<Review>,
    step: Step,
    error: Option<String>,
    warning: Option<String>,
    status: String,
    show_dependencies: bool,
    dirty: bool,
    discard_prompt: bool,
    generation: u64,
    token: Option<Token>,
    action: Option<super::Action>,
}

impl Page {
    pub(crate) fn new(paths: Paths, name: String) -> (Self, Task<Event>) {
        let page = Self {
            paths: paths.clone(),
            source: name.clone(),
            name: format!("{name}-copy"),
            destination: String::new(),
            native: None,
            review: None,
            step: Step::Details,
            error: None,
            warning: None,
            status: String::new(),
            show_dependencies: false,
            dirty: false,
            discard_prompt: false,
            generation: 0,
            token: None,
            action: None,
        };
        let load = Task::perform(
            async move {
                task::join(
                    move || {
                        let environment = Environment::load(&paths, &name)?;
                        match &environment.config.kind {
                            Kind::Wine(_) => Ok(false),
                            Kind::Native => Ok(true),
                            Kind::Proxied(_) => Err(environment.config.no_prefix(&name).into()),
                        }
                    },
                    anyhow::Error::from,
                )
                .await
                .map_err(|error| error.to_string())
            },
            Event::SourceLoaded,
        );
        (page, load)
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::SourceLoaded(Ok(native)) => {
                self.native = Some(native);
                Task::none()
            }
            Event::SourceLoaded(Err(error)) => {
                self.error = Some(error);
                Task::none()
            }
            Event::Name(name) => {
                self.generation = self.generation.wrapping_add(1);
                self.name = name;
                self.dirty = true;
                self.clear_review();
                Task::none()
            }
            Event::Destination(destination) => {
                self.generation = self.generation.wrapping_add(1);
                self.destination = destination;
                self.dirty = true;
                self.clear_review();
                Task::none()
            }
            Event::PickDestination => {
                self.generation = self.generation.wrapping_add(1);
                let generation = self.generation;
                Task::perform(
                    crate::picker::pick("Choose a prefix destination".to_owned(), true),
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
            Event::Review => self.begin_review(),
            Event::ReviewLoaded(generation, result) => {
                if generation != self.generation || self.step != Step::Working {
                    return Task::none();
                }
                self.token = None;
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
            Event::Clone => self.begin_clone(),
            Event::CloneFinished(generation, result) => {
                if generation != self.generation || self.step != Step::Working {
                    return Task::none();
                }
                self.token = None;
                match result {
                    Ok(name) => {
                        self.name = name;
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
            Event::Dependencies(show) => {
                self.show_dependencies = show;
                Task::none()
            }
            Event::KeepEditing => {
                self.discard_prompt = false;
                Task::none()
            }
            Event::Discard => {
                self.discard_prompt = false;
                self.action = Some(super::Action::Library {
                    name: None,
                    launch: false,
                });
                Task::none()
            }
            Event::Back => self.back(),
            Event::Library => {
                self.action = Some(super::Action::Library {
                    name: Some(self.name.clone()),
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
        }
    }

    pub(crate) fn view(&self, palette: Palette) -> Element<'_, Event> {
        let body = match self.step {
            Step::Details => self.details(palette),
            Step::Review => self.review_view(palette),
            Step::Working => column![
                text(self.status.as_str()).color(palette.muted),
                self.token.as_ref().map(|token| {
                    let (label, event) = if token.is_canceled() {
                        ("Force stop", Event::ForceStop)
                    } else {
                        ("Cancel", Event::Cancel)
                    };
                    secondary_button(label, palette).on_press(event)
                }),
            ]
            .spacing(14)
            .into(),
            Step::Finished => column![
                text(format!("Cloned {} as {}.", self.source, self.name)).color(palette.success),
                secondary_button("Back to library", palette).on_press(Event::Library),
            ]
            .spacing(12)
            .into(),
        };
        let mut content = column![
            text("Clone environment").size(22).color(palette.foreground),
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
                    .on_press(Event::Review)
                    .into(),
            ),
            Step::Review if self.review.is_some() => Some(
                accent_base("Clone", palette.primary)
                    .on_press(Event::Clone)
                    .into(),
            ),
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
        self.step == Step::Working
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
            Step::Review => self.begin_clone(),
            Step::Finished => {
                self.action = Some(super::Action::Library {
                    name: Some(self.name.clone()),
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
                if self.dirty {
                    self.discard_prompt = true;
                } else {
                    self.action = Some(super::Action::Library {
                        name: None,
                        launch: false,
                    });
                }
                Task::none()
            }
            Step::Review => {
                self.step = Step::Details;
                self.error = None;
                Task::none()
            }
            Step::Finished => {
                self.action = Some(super::Action::Library {
                    name: Some(self.name.clone()),
                    launch: false,
                });
                Task::none()
            }
            Step::Working => Task::none(),
        }
    }

    fn details(&self, palette: Palette) -> Element<'_, Event> {
        let native = self.native.unwrap_or(false);
        let mut fields = column![
            text(format!("Source: {}", self.source)).color(palette.muted),
            text("New environment name").color(palette.foreground),
            text_input("Name", &self.name)
                .on_input(Event::Name)
                .padding(8)
                .style(move |_, status| input_style(palette, status)),
        ]
        .spacing(8);
        if !native {
            fields = fields
                .push(text("Optional prefix destination").color(palette.foreground))
                .push(
                    row![
                        text_input("Default: configured prefix root/name", &self.destination)
                            .on_input(Event::Destination)
                            .width(Fill)
                            .padding(8)
                            .style(move |_, status| input_style(palette, status)),
                        secondary_button("Browse...", palette).on_press(Event::PickDestination),
                    ]
                    .spacing(8),
                );
        }
        fields.into()
    }

    fn review_view(&self, palette: Palette) -> Element<'_, Event> {
        let mut content = column![].spacing(10);
        match &self.review {
            Some(Review::Wine(review)) => {
                content = content
                    .push(
                        text(format!(
                            "Clone {} as {}",
                            review.source_name, review.new_name
                        ))
                        .size(18)
                        .color(palette.foreground),
                    )
                    .push(
                        text(format!("Prefix: {}", review.source_root.display()))
                            .color(palette.foreground),
                    )
                    .push(
                        text(format!("Copy to: {}", review.destination.display()))
                            .color(palette.foreground),
                    )
                    .push(
                        text(format!(
                            "Runner: {} | layout: {:?}",
                            review.runner, review.layout
                        ))
                        .color(palette.foreground),
                    )
                    .push(
                        text(format!("Targets: {}", display_targets(&review.targets)))
                            .color(palette.foreground),
                    )
                    .push(text("External game files stay in place. Keep the source and external launchers idle during the copy.").color(palette.warning));
                content = review_notes(
                    content,
                    &review.changed_settings,
                    &review.external_target_paths,
                    &review.literal_source_paths,
                    review.art,
                    palette,
                );
                content = dependencies(
                    content,
                    &review.external_dependencies,
                    self.show_dependencies,
                    palette,
                );
            }
            Some(Review::Native(review)) => {
                content = content
                    .push(text(format!("Clone {} as {}", review.source_name, review.new_name)).size(18).color(palette.foreground))
                    .push(text(format!("Targets: {}", display_targets(&review.targets))).color(palette.foreground))
                    .push(text("Native clones copy the registration and art, and share the same game files.").color(palette.warning));
                content = review_notes(
                    content,
                    &review.changed_settings,
                    &[],
                    &[],
                    review.art,
                    palette,
                );
            }
            None => return text("Preparing review...").into(),
        }
        content.into()
    }

    fn begin_review(&mut self) -> Task<Event> {
        if self.busy() {
            return Task::none();
        }
        if self.name.trim().is_empty() {
            self.error = Some("Choose a name for the clone.".to_owned());
            return Task::none();
        }
        self.generation = self.generation.wrapping_add(1);
        self.step = Step::Working;
        self.status = "Waiting for the source to become idle...".to_owned();
        self.error = None;
        let generation = self.generation;
        let token = Token::new();
        self.token = Some(token.clone());
        let paths = self.paths.clone();
        let source = self.source.clone();
        let name = self.name.clone();
        let destination = self.destination.clone();
        Task::perform(
            async move {
                task::join(
                    move || inspect_clone(&paths, &source, &name, &destination, &token),
                    anyhow::Error::from,
                )
                .await
                .map_err(|error| error.to_string())
            },
            move |result| Event::ReviewLoaded(generation, result.map(Box::new)),
        )
    }

    fn begin_clone(&mut self) -> Task<Event> {
        if self.step != Step::Review || self.busy() {
            return Task::none();
        }
        let Some(review) = self.review.clone() else {
            return Task::none();
        };
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Working;
        self.status = "Cloning environment...".to_owned();
        self.error = None;
        self.warning = None;
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        let progress_generation = generation;
        let work = Task::perform(
            async move {
                task::join(
                    move || match review {
                        Review::Wine(review) => {
                            let name = review.new_name.clone();
                            clone::execute_reviewed(
                                &paths,
                                &GlobalConfig::load(&paths)?,
                                &review,
                                &token,
                                {
                                    let mut copied = 0_u64;
                                    move |progress| {
                                        if let ImportProgress::Warning(warning) = &progress {
                                            let _ = sender.unbounded_send(Event::Warning(
                                                progress_generation,
                                                warning.clone(),
                                            ));
                                        }
                                        if let ImportProgress::Copied { .. } = progress {
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
                            Ok::<_, anyhow::Error>(name)
                        }
                        Review::Native(review) => {
                            let name = review.new_name.clone();
                            clone::execute_native(&paths, &review, |warning| {
                                let _ = sender
                                    .unbounded_send(Event::Warning(progress_generation, warning));
                            })?;
                            Ok(name)
                        }
                    },
                    anyhow::Error::from,
                )
                .await
                .map_err(|error| error.to_string())
            },
            move |result| Event::CloneFinished(generation, result),
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

fn inspect_clone(
    paths: &Paths,
    source: &str,
    name: &str,
    destination: &str,
    token: &Token,
) -> Result<Review, anyhow::Error> {
    inspect_clone_with_wait(
        paths,
        source,
        name,
        destination,
        token,
        |runner, prefix, token| runner.wait_idle_for_copy(prefix, token),
    )
}

fn inspect_clone_with_wait(
    paths: &Paths,
    source: &str,
    name: &str,
    destination: &str,
    token: &Token,
    wait: impl FnOnce(
        &wineo_core::runner::Runner,
        &std::path::Path,
        &Token,
    ) -> wineo_core::error::Result<()>,
) -> anyhow::Result<Review> {
    let environment = Environment::load(paths, source)?;
    let Some(wine) = environment.config.wine() else {
        return Ok(Review::Native(Box::new(clone::inspect_native(
            paths, source, name,
        )?)));
    };
    let global = GlobalConfig::load(paths)?;
    let (runner, _build_guard) =
        Runners::new(paths, &global)?.resolve_locked(paths, &wine.runner)?;
    let prefix = environment.prefix(paths)?;
    wait(&runner, &prefix, token)?;
    if token.is_canceled() {
        anyhow::bail!("clone canceled while waiting for the source to become idle");
    }
    let destination = if destination.trim().is_empty() {
        None
    } else {
        Some(paths.absolute(destination)?)
    };
    let review = clone::inspect(paths, &global, source, name, destination.as_deref())?;
    Ok(Review::Wine(Box::new(review)))
}

fn display_targets(targets: &[String]) -> String {
    if targets.is_empty() {
        "(none)".to_owned()
    } else {
        targets.join(", ")
    }
}

fn review_notes<'a>(
    mut content: iced::widget::Column<'a, Event>,
    changes: &'a [String],
    external_targets: &'a [String],
    literal_paths: &'a [String],
    art: bool,
    palette: Palette,
) -> iced::widget::Column<'a, Event> {
    content = content
        .push(text(if art { "Artwork will be copied." } else { "No artwork is registered." }).color(palette.muted))
        .push(text("Playtime starts at zero. Desktop entries are disabled and the Heroic id is cleared.").color(palette.warning));
    for change in changes {
        content = content.push(
            text(format!("Changed: {change}"))
                .size(13)
                .color(palette.muted),
        );
    }
    for target in external_targets {
        content = content.push(
            text(format!("External target stays in place: {target}"))
                .size(13)
                .color(palette.warning),
        );
    }
    for path in literal_paths {
        content = content.push(
            text(format!("Literal source path needs review: {path}"))
                .size(13)
                .color(palette.warning),
        );
    }
    content
}

fn dependencies<'a>(
    mut content: iced::widget::Column<'a, Event>,
    dependencies: &'a [import::ExternalDependency],
    show: bool,
    palette: Palette,
) -> iced::widget::Column<'a, Event> {
    if dependencies.is_empty() {
        return content;
    }
    content = content.push(
        secondary_button(
            if show {
                format!("Hide {} external dependencies", dependencies.len())
            } else {
                format!("Show all {} external dependencies", dependencies.len())
            },
            palette,
        )
        .on_press(Event::Dependencies(!show)),
    );
    if show {
        for dependency in dependencies {
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
    content
}

fn discard_prompt(palette: Palette) -> Element<'static, Event> {
    container(
        column![
            text("Discard this clone?")
                .size(18)
                .color(palette.foreground),
            text("Your clone name and destination will be lost.").color(palette.muted),
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

    fn wine_source(paths: &Paths, root: &Path) -> PathBuf {
        let prefix = root.join("source-prefix");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        let config = paths.environment_config("origin");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            config,
            format!(
                "schema_version = 1\nprefix = {:?}\nrunner = \"fake-wine\"\narch = \"win64\"\n",
                prefix.display().to_string()
            ),
        )
        .unwrap();
        prefix
    }

    #[test]
    fn wine_review_holds_the_runner_guard_while_waiting() {
        use wineo_core::build_lock::{self, Kind};

        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_runner(&paths);
        wine_source(&paths, root.path());
        inspect_clone_with_wait(
            &paths,
            "origin",
            "copy",
            "",
            &Token::new(),
            |runner, prefix, token| {
                assert!(build_lock::try_exclusive(&paths, Kind::Runner, "fake-wine")?.is_none());
                runner.wait_idle_for_copy(prefix, token)
            },
        )
        .unwrap();
        assert!(
            build_lock::try_exclusive(&paths, Kind::Runner, "fake-wine")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn wine_review_waits_on_fake_runner_and_refuses_a_changed_source() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_runner(&paths);
        let prefix = wine_source(&paths, root.path());
        let token = Token::new();
        let review = inspect_clone(&paths, "origin", "copy", "", &token).unwrap();
        let Review::Wine(review) = review else {
            panic!("expected Wine clone review");
        };
        assert_eq!(review.source_name, "origin");
        assert_eq!(review.new_name, "copy");
        assert!(!review.destination.exists());
        assert!(!paths.environment_config("copy").exists());

        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\nchanged\n",
        )
        .unwrap();
        let error = clone::execute_reviewed(
            &paths,
            &GlobalConfig::default(),
            &review,
            &Token::new(),
            |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("changed"));
        assert!(!review.destination.exists());
        assert!(!paths.environment_config("copy").exists());
    }

    #[test]
    fn canceled_wine_clone_keeps_reviewed_source_and_does_not_publish() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_runner(&paths);
        wine_source(&paths, root.path());
        let review = inspect_clone(&paths, "origin", "copy", "", &Token::new()).unwrap();
        let Review::Wine(review) = review else {
            panic!("expected Wine clone review");
        };
        let token = Token::new();
        token.cancel();
        assert!(
            clone::execute_reviewed(&paths, &GlobalConfig::default(), &review, &token, |_| {},)
                .is_err()
        );
        assert!(!review.destination.exists());
        assert!(!paths.environment_config("copy").exists());
    }

    #[test]
    fn native_review_is_read_only_and_marks_shared_game_files() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        Environment::create_native(&paths, &global, "origin", None, false, |_| {}).unwrap();
        let review = inspect_clone(&paths, "origin", "copy", "", &Token::new()).unwrap();
        let Review::Native(review) = review else {
            panic!("expected native clone review");
        };
        assert_eq!(review.source_name, "origin");
        assert_eq!(review.new_name, "copy");
        assert!(!paths.environment_config("copy").exists());
    }

    #[test]
    fn submit_uses_the_visible_primary_action_and_ignores_busy_pages() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        Environment::create_native(&paths, &global, "origin", None, false, |_| {}).unwrap();
        let mut page = Page::new(paths, "origin".to_owned()).0;
        page.native = Some(true);
        page.name = "copy".to_owned();
        let _task = page.submit();
        assert_eq!(page.step, Step::Working);
        drop(_task);

        let review = clone::inspect_native(&page.paths, "origin", "copy").unwrap();
        page.step = Step::Review;
        page.review = Some(Review::Native(Box::new(review)));
        let _task = page.submit();
        assert_eq!(page.status, "Cloning environment...");
        drop(_task);

        page.action = None;
        let status = page.status.clone();
        let _task = page.submit();
        assert!(page.action.is_none());
        assert_eq!(page.status, status);

        page.step = Step::Finished;
        let _task = page.submit();
        assert!(matches!(
            page.take_action(),
            Some(super::super::Action::Library {
                name: Some(name),
                launch: false
            }) if name == "copy"
        ));
    }

    #[test]
    fn back_is_at_the_top_and_the_forward_action_is_pinned_at_the_bottom() {
        use crate::layout_probe::{assert_back_on_top_and_forward_pinned, click_at, probe};

        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fake_runner(&paths);
        wine_source(&paths, root.path());
        let wine = inspect_clone(&paths, "origin", "copy", "", &Token::new()).unwrap();
        Environment::create_native(
            &paths,
            &GlobalConfig::default(),
            "native",
            None,
            false,
            |_| {},
        )
        .unwrap();
        let native = inspect_clone(&paths, "native", "copy", "", &Token::new()).unwrap();
        let mut page = Page::new(paths, "origin".to_owned()).0;
        page.name = "copy".to_owned();
        for (label, step, review) in [
            ("details", Step::Details, None),
            ("wine review", Step::Review, Some(wine)),
            ("native review", Step::Review, Some(native)),
        ] {
            page.step = step;
            page.review = review;
            for (width, height) in [(280.0, 240.0), (900.0, 240.0), (900.0, 720.0)] {
                let inset = iced::Rectangle::new(
                    iced::Point::new(24.0, 24.0),
                    iced::Size::new(width - 48.0, height - 48.0),
                );
                let mut view = page.view(Palette::DARK);
                let probe = probe(&mut view, iced::Size::new(width, height));
                let case = format!("{label} at {width}x{height}");
                assert_back_on_top_and_forward_pinned(&probe, inset, &case);
                let spilled = probe.outside(inset);
                assert!(spilled.is_empty(), "{case}: {spilled:?}");
            }
            let size = iced::Size::new(900.0, 720.0);
            let forward = if step == Step::Details {
                "Review"
            } else {
                "Clone"
            };
            assert_eq!(
                click_at(page.view(Palette::DARK), size, &[0, 0, 0]),
                ["Back"],
                "{label}"
            );
            assert_eq!(
                click_at(page.view(Palette::DARK), size, &[0, 2, 0]),
                [forward],
                "{label}"
            );
        }
    }
}
