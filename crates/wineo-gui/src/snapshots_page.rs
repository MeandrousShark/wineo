use std::fs;

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{column, container, pick_list, row, scrollable, text, text_input};
use iced::{Center, Element, Fill, Task};
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::{self, Environment};
use wineo_core::import::{ImportLayout, ImportProgress};
use wineo_core::name;
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;
use wineo_core::snapshot::{
    self, CreateInspection, RestoreInspection, SnapshotEvent, SnapshotInfo,
};

use crate::logview::{self, LogView};
use crate::theme::Palette;
use crate::ui::{
    PickListStyle, accent_button, page_title, secondary_button, section, section_heading,
};

const SNAPSHOT_LIST_HEIGHT: f32 = 180.0;
const REPORT_HEIGHT: f32 = 220.0;
const REPORT_ID: &str = "snapshot-report";
const RESULT_ID: &str = "snapshot-result";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportKind {
    Review,
    Result,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Checkup,
    Select(String),
    Refresh,
    Label(String),
    CreateReview,
    RestoreReview(String),
    DeleteReview(String),
    RecoverReview,
    ShowConfig(bool),
    ReportAction(ReportKind, iced::widget::text_editor::Action),
    ReportScrolled(ReportKind, iced::widget::scrollable::Viewport),
    CopyReport(ReportKind),
    FollowReport(ReportKind),
    Confirm,
    Dismiss,
    Cancel,
    EnvironmentsLoaded(u64, Result<Vec<Choice>, String>),
    Loaded(u64, Result<Loaded, String>),
    Progress(u64, String),
    Finishing(u64),
    Reviewed(u64, Result<Review, String>),
    Finished(u64, Result<Outcome, String>),
}

impl Event {
    pub(crate) fn starts_work(&self) -> bool {
        matches!(
            self,
            Self::CreateReview
                | Self::RestoreReview(_)
                | Self::DeleteReview(_)
                | Self::RecoverReview
                | Self::Confirm
        )
    }

    pub(crate) fn is_input(&self) -> bool {
        !matches!(
            self,
            Self::EnvironmentsLoaded(..)
                | Self::Loaded(..)
                | Self::Progress(..)
                | Self::Finishing(..)
                | Self::Reviewed(..)
                | Self::Finished(..)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    name: String,
    problem: Option<String>,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.problem.is_some() {
            write!(f, "{} (needs attention)", self.name)
        } else {
            f.write_str(&self.name)
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Loaded {
    snapshots: Result<Vec<SnapshotInfo>, String>,
    recovery: Recovery,
}

#[derive(Debug, Clone)]
enum Recovery {
    None,
    Ready,
    Invalid(String),
}

#[derive(Debug, Clone)]
pub(crate) enum Review {
    Create(CreateInspection),
    Restore(RestoreInspection),
    Delete(String),
    Recover,
}

#[derive(Debug, Clone)]
pub(crate) enum Outcome {
    Created(String),
    Restored {
        label: String,
        retained_old_prefix: Option<std::path::PathBuf>,
    },
    Deleted(String),
    Recovered,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Back,
    Checkup,
}

pub(crate) struct Page {
    paths: Paths,
    choices: Vec<Choice>,
    choices_error: Option<String>,
    selected: Option<String>,
    loaded: Option<Loaded>,
    label: String,
    review: Option<Review>,
    review_report: String,
    review_log: LogView,
    show_config: bool,
    status: Option<String>,
    error: Option<String>,
    result: Option<String>,
    result_log: LogView,
    outcome: Option<Outcome>,
    action: Option<Action>,
    generation: u64,
    loading: bool,
    working: bool,
    finishing: bool,
    token: Option<Token>,
}

impl Page {
    pub(crate) fn new(paths: Paths) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            choices: Vec::new(),
            choices_error: None,
            selected: None,
            loaded: None,
            label: String::new(),
            review: None,
            review_report: String::new(),
            review_log: LogView::default(),
            show_config: false,
            status: None,
            error: None,
            result: None,
            result_log: LogView::default(),
            outcome: None,
            action: None,
            generation: 0,
            loading: false,
            working: false,
            finishing: false,
            token: None,
        };
        let task = page.refresh_environments();
        (page, task)
    }

    pub(crate) fn selected_name(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub(crate) fn busy(&self) -> bool {
        self.working
    }

    pub(crate) fn finishing(&self) -> bool {
        self.finishing
    }

    pub(crate) fn cancel(&mut self) {
        if !self.finishing
            && let Some(token) = &self.token
        {
            token.cancel();
            self.status = Some("Canceling snapshot work...".into());
        }
    }

    pub(crate) fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        if self.review.is_some() {
            self.update(Event::Confirm)
        } else {
            self.update(Event::CreateReview)
        }
    }

    pub(crate) fn select(&mut self, name: String) -> Task<Event> {
        if self.working || self.selected.as_deref() == Some(name.as_str()) {
            return Task::none();
        }
        self.selected = Some(name);
        self.clear_review();
        self.loaded = None;
        self.error = None;
        self.refresh_environments()
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Back => {
                if self.review.is_some() && !self.working {
                    self.clear_review();
                } else {
                    self.action = Some(Action::Back);
                }
                Task::none()
            }
            Event::Checkup => {
                self.action = Some(Action::Checkup);
                Task::none()
            }
            Event::Select(name) => self.select(name),
            Event::Refresh if !self.working => self.refresh_environments(),
            Event::Label(label) if !self.working => {
                self.label = label;
                self.error = None;
                self.clear_review();
                Task::none()
            }
            Event::CreateReview if !self.working => self.inspect_create(),
            Event::RestoreReview(label) if !self.working => self.inspect_restore(label),
            Event::DeleteReview(label) if !self.working => {
                self.set_review(Review::Delete(label));
                self.error = None;
                Task::none()
            }
            Event::RecoverReview if !self.working => {
                if self
                    .loaded
                    .as_ref()
                    .is_some_and(|loaded| matches!(loaded.recovery, Recovery::Ready))
                {
                    self.set_review(Review::Recover);
                    self.error = None;
                }
                Task::none()
            }
            Event::ShowConfig(show) => {
                if self.show_config != show {
                    self.show_config = show;
                    self.refresh_review_report();
                }
                Task::none()
            }
            Event::ReportAction(kind, action) => {
                let source = self.report_text(kind).to_owned();
                if self.report_log_mut(kind).perform(action, || source.clone()) {
                    return logview::snap_to_end(report_id(kind));
                }
                Task::none()
            }
            Event::ReportScrolled(kind, viewport) => {
                let source = self.report_text(kind).to_owned();
                let bounds = viewport.absolute_offset();
                let content = viewport.content_bounds();
                let viewport_bounds = viewport.bounds();
                if self.report_log_mut(kind).scrolled(
                    bounds.y,
                    viewport_bounds.height,
                    content.height,
                    || source.clone(),
                ) {
                    return logview::snap_to_end(report_id(kind));
                }
                Task::none()
            }
            Event::CopyReport(kind) => {
                let report = self.report_text(kind).to_owned();
                let copied = copied_report(self.report_log_mut(kind), &report);
                iced::clipboard::write(copied)
            }
            Event::FollowReport(kind) => {
                let source = self.report_text(kind).to_owned();
                if self.report_log_mut(kind).follow(|| source.clone()) {
                    return logview::snap_to_end(report_id(kind));
                }
                Task::none()
            }
            Event::Confirm if !self.working => self.execute_review(),
            Event::Dismiss if !self.working => {
                self.clear_review();
                Task::none()
            }
            Event::Cancel => {
                self.cancel();
                Task::none()
            }
            Event::EnvironmentsLoaded(generation, result) if generation == self.generation => {
                match result {
                    Ok(choices) => {
                        self.choices = choices;
                        self.choices_error = None;
                        if self.selected.as_ref().is_none_or(|name| {
                            !self.choices.iter().any(|choice| &choice.name == name)
                        }) {
                            self.selected = self.choices.first().map(|choice| choice.name.clone());
                        }
                    }
                    Err(error) => self.choices_error = Some(error),
                }
                self.load_selected()
            }
            Event::Loaded(generation, result) if generation == self.generation => {
                self.loading = false;
                match result {
                    Ok(loaded) => self.loaded = Some(loaded),
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Progress(generation, status)
                if generation == self.generation && self.working =>
            {
                self.status = Some(status);
                Task::none()
            }
            Event::Finishing(generation) if generation == self.generation && self.working => {
                self.finishing = true;
                self.status = Some("Finishing changes...".into());
                Task::none()
            }
            Event::Reviewed(generation, result)
                if generation == self.generation && self.working =>
            {
                self.end_work();
                match result {
                    Ok(review) => {
                        self.set_review(review);
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Event::Finished(generation, result)
                if generation == self.generation && self.working =>
            {
                self.end_work();
                self.clear_review();
                match result {
                    Ok(outcome) => {
                        let result = match &outcome {
                            Outcome::Created(label) => format!("Created snapshot {label}."),
                            Outcome::Restored {
                                label,
                                retained_old_prefix,
                            } => {
                                let mut message = format!(
                                    "Restored snapshot {label}. Art and playtime stayed in place."
                                );
                                if let Some(path) = retained_old_prefix {
                                    message.push_str(&format!(
                                        " Remove the old prefix manually: {}",
                                        self.paths.contract_text(&path.display().to_string())
                                    ));
                                }
                                message
                            }
                            Outcome::Deleted(label) => format!("Deleted snapshot {label}."),
                            Outcome::Recovered => "Rolled back the interrupted restore.".into(),
                            Outcome::Failed => unreachable!(),
                        };
                        self.set_result(result);
                        self.error = None;
                        self.outcome = Some(outcome);
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.outcome = Some(Outcome::Failed);
                    }
                }
                self.refresh_environments()
            }
            _ => Task::none(),
        }
    }

    fn refresh_environments(&mut self) -> Task<Event> {
        if self.working {
            return Task::none();
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loading = true;
        let paths = self.paths.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || read_choices(&paths))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Event::EnvironmentsLoaded(generation, result),
        )
    }

    fn load_selected(&mut self) -> Task<Event> {
        let Some(name) = self.selected.clone() else {
            self.loading = false;
            self.loaded = None;
            return Task::none();
        };
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loading = true;
        let paths = self.paths.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || read_selected(&paths, &name))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Event::Loaded(generation, result),
        )
    }

    fn begin_work(&mut self, status: &str, cancellable: bool) -> (u64, Token) {
        self.generation = self.generation.wrapping_add(1);
        self.working = true;
        self.finishing = !cancellable;
        self.status = Some(status.into());
        self.error = None;
        self.result = None;
        self.result_log = LogView::default();
        let token = Token::new();
        self.token = Some(token.clone());
        (self.generation, token)
    }

    fn end_work(&mut self) {
        self.working = false;
        self.finishing = false;
        self.status = None;
        self.token = None;
    }

    fn clear_review(&mut self) {
        self.review = None;
        self.review_report.clear();
        self.review_log = LogView::default();
        self.show_config = false;
    }

    fn set_review(&mut self, review: Review) {
        self.review = Some(review);
        self.show_config = false;
        self.refresh_review_report();
    }

    fn refresh_review_report(&mut self) {
        let Some(review) = &self.review else {
            self.review_report.clear();
            self.review_log = LogView::default();
            return;
        };
        self.review_report = review_report(&self.paths, review, self.show_config);
        self.review_log = LogView::default();
        let _ = self.review_log.resync(&self.review_report);
    }

    fn set_result(&mut self, result: String) {
        self.result = Some(result.clone());
        self.result_log = LogView::default();
        let _ = self.result_log.resync(&result);
    }

    fn report_text(&self, kind: ReportKind) -> &str {
        match kind {
            ReportKind::Review => &self.review_report,
            ReportKind::Result => self.result.as_deref().unwrap_or_default(),
        }
    }

    fn report_log_mut(&mut self, kind: ReportKind) -> &mut LogView {
        match kind {
            ReportKind::Review => &mut self.review_log,
            ReportKind::Result => &mut self.result_log,
        }
    }

    fn inspect_create(&mut self) -> Task<Event> {
        let Some(name) = self.selected.clone() else {
            return Task::none();
        };
        let label = self.label.clone();
        if !name::is_valid(&label) {
            self.error = Some("Invalid snapshot label: use letters, numbers, - or _.".into());
            return Task::none();
        }
        self.clear_review();
        let (generation, token) = self.begin_work("Waiting for the prefix, then checking...", true);
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || -> Result<Review, String> {
                    let global = GlobalConfig::load(&paths).map_err(|error| error.to_string())?;
                    wait_idle(&paths, &global, &name, &token).map_err(|error| error.to_string())?;
                    let review = snapshot::inspect_create_with(
                        &paths,
                        &global,
                        &name,
                        &label,
                        &token,
                        |progress| {
                            let _ = sender.unbounded_send(Event::Progress(
                                generation,
                                progress_status(&progress),
                            ));
                        },
                    )
                    .map_err(|error| error.to_string())?;
                    Ok(Review::Create(review))
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Event::Reviewed(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn inspect_restore(&mut self, label: String) -> Task<Event> {
        let Some(name) = self.selected.clone() else {
            return Task::none();
        };
        self.clear_review();
        let (generation, token) = self.begin_work("Waiting for the prefix, then checking...", true);
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || -> Result<Review, String> {
                    let global = GlobalConfig::load(&paths).map_err(|error| error.to_string())?;
                    wait_idle(&paths, &global, &name, &token).map_err(|error| error.to_string())?;
                    let review = snapshot::inspect_restore_with(
                        &paths,
                        &global,
                        &name,
                        &label,
                        &token,
                        |progress| {
                            let _ = sender.unbounded_send(Event::Progress(
                                generation,
                                progress_status(&progress),
                            ));
                        },
                    )
                    .map_err(|error| error.to_string())?;
                    Ok(Review::Restore(review))
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Event::Reviewed(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn execute_review(&mut self) -> Task<Event> {
        let Some(review) = self.review.clone() else {
            return Task::none();
        };
        let Some(name) = self.selected.clone() else {
            return Task::none();
        };
        let cancellable = matches!(review, Review::Create(_) | Review::Restore(_));
        let status = match &review {
            Review::Create(_) | Review::Restore(_) => "Checking reviewed snapshot...",
            Review::Delete(_) => "Deleting snapshot...",
            Review::Recover => "Rolling back restore...",
        };
        let (generation, token) = self.begin_work(status, cancellable);
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || -> Result<Outcome, String> {
                    let result = match review {
                        Review::Create(review) => {
                            let global =
                                GlobalConfig::load(&paths).map_err(|error| error.to_string())?;
                            snapshot::create_reviewed_with(
                                &paths,
                                &global,
                                &review,
                                &token,
                                |event| {
                                    let _ = sender.unbounded_send(event_message(generation, event));
                                },
                            )
                            .map(|info| Outcome::Created(info.label))
                            .map_err(|error| error.to_string())
                        }
                        Review::Restore(review) => {
                            let global =
                                GlobalConfig::load(&paths).map_err(|error| error.to_string())?;
                            snapshot::restore_reviewed_with(
                                &paths,
                                &global,
                                &review,
                                &token,
                                |event| {
                                    let _ = sender.unbounded_send(event_message(generation, event));
                                },
                            )
                            .map(|outcome| Outcome::Restored {
                                label: review.label,
                                retained_old_prefix: outcome.retained_old_prefix,
                            })
                            .map_err(|error| error.to_string())
                        }
                        Review::Delete(label) => snapshot::delete(&paths, &name, &label)
                            .map(|()| Outcome::Deleted(label))
                            .map_err(|error| error.to_string()),
                        Review::Recover => snapshot::recover(&paths, &name)
                            .map(|()| Outcome::Recovered)
                            .map_err(|error| error.to_string()),
                    };
                    result.map_err(|error| paths.contract_text(&error))
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Event::Finished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    pub(crate) fn view(&self, palette: Palette, busy: bool) -> Element<'_, Event> {
        let enabled = !busy && !self.working;
        let reviewing = self.review.is_some();
        let mut content = column![
            row![
                page_title("Snapshots", palette).width(Fill),
                secondary_button("Back", palette).on_press(Event::Back),
                secondary_button("Refresh", palette)
                    .on_press_maybe(enabled.then_some(Event::Refresh)),
            ]
            .spacing(8)
            .align_y(Center)
        ]
        .spacing(16);
        let selected_choice = self
            .choices
            .iter()
            .find(|choice| self.selected.as_deref() == Some(&choice.name));
        let mut environment = column![
            section_heading("Environment", palette),
            pick_list(self.choices.as_slice(), selected_choice, |choice| {
                Event::Select(choice.name)
            })
            .on_open(Event::Refresh)
            .wineo_style(palette),
        ]
        .spacing(12);
        if let Some(error) = &self.choices_error {
            environment = environment.push(text(error).color(palette.danger));
        }
        if let Some(choice) = selected_choice
            && let Some(problem) = &choice.problem
        {
            environment = environment.push(text(problem).color(palette.warning));
        }
        content = content.push(section(environment, palette));

        if !reviewing {
            let mut snapshot_rows =
                column![section_heading("Saved snapshots", palette)].spacing(12);
            if let Some(loaded) = &self.loaded {
                match &loaded.recovery {
                    Recovery::Ready => {
                        snapshot_rows = snapshot_rows.push(
                            text("An interrupted restore needs recovery.").color(palette.warning),
                        );
                        snapshot_rows = snapshot_rows.push(accent_button(
                            "Roll back restore",
                            palette.warning,
                            enabled,
                            Event::RecoverReview,
                        ));
                    }
                    Recovery::Invalid(error) => {
                        snapshot_rows = snapshot_rows.push(
                            text(format!("Restore recovery marker: {error}")).color(palette.danger),
                        )
                    }
                    Recovery::None => {}
                }
                match &loaded.snapshots {
                    Ok(snapshots) => {
                        if snapshots.is_empty() {
                            snapshot_rows =
                                snapshot_rows.push(text("No snapshots yet.").color(palette.muted));
                        } else {
                            let mut rows = column![].spacing(8);
                            for (index, info) in snapshots.iter().enumerate() {
                                if index > 0 {
                                    rows = rows.push(crate::ui::separator(palette));
                                }
                                rows = rows.push(
                                    column![
                                        row![
                                            text(info.label.as_str())
                                                .width(Fill)
                                                .color(palette.foreground),
                                            secondary_button("Restore", palette).on_press_maybe(
                                                enabled.then(|| {
                                                    Event::RestoreReview(info.label.clone())
                                                })
                                            ),
                                            accent_button(
                                                "Delete",
                                                palette.danger,
                                                enabled,
                                                Event::DeleteReview(info.label.clone()),
                                            ),
                                        ]
                                        .spacing(8)
                                        .align_y(Center),
                                        text(info.created_at.as_str())
                                            .size(13)
                                            .color(palette.muted)
                                            .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                                    ]
                                    .spacing(4),
                                );
                            }
                            snapshot_rows =
                                snapshot_rows.push(scrollable(rows).height(SNAPSHOT_LIST_HEIGHT));
                        }
                    }
                    Err(error) => {
                        snapshot_rows = snapshot_rows.push(
                            text(format!("Could not list snapshots: {error}"))
                                .color(palette.danger),
                        )
                    }
                }
            }
            if self.loading {
                snapshot_rows =
                    snapshot_rows.push(text("Loading snapshots...").color(palette.muted));
            }
            if !self.loading && self.selected.is_none() {
                snapshot_rows = snapshot_rows
                    .push(text("No Wine environments in this library.").color(palette.muted));
            }
            content = content.push(section(snapshot_rows, palette));

            if self.selected.is_some()
                && self
                    .loaded
                    .as_ref()
                    .is_some_and(|loaded| matches!(loaded.recovery, Recovery::None))
            {
                content = content.push(section(
                    column![
                        section_heading("Create snapshot", palette),
                        row![
                            text_input("Snapshot label", &self.label).on_input(Event::Label),
                            accent_button(
                                "Create snapshot...",
                                palette.primary,
                                enabled,
                                Event::CreateReview,
                            ),
                        ]
                        .spacing(8)
                    ]
                    .spacing(12),
                    palette,
                ));
            }
        }
        if let Some(review) = &self.review {
            content = content.push(section(
                review_view(
                    review,
                    &self.review_report,
                    &self.review_log,
                    self.show_config,
                    palette,
                    enabled,
                ),
                palette,
            ));
        }
        let mut feedback = column![].spacing(8);
        if let Some(status) = &self.status {
            feedback = feedback.push(text(status).color(palette.muted));
            if self.working && !self.finishing {
                feedback =
                    feedback.push(secondary_button("Cancel", palette).on_press(Event::Cancel));
            }
        }
        if let Some(error) = &self.error {
            feedback = feedback.push(text(error).color(palette.danger)).push(
                secondary_button("Refresh", palette)
                    .on_press_maybe(enabled.then_some(Event::Refresh)),
            );
        }
        if let Some(result) = &self.result {
            feedback = feedback.push(section_heading("Result", palette).color(palette.success));
            feedback = feedback.push(report_panel(
                &self.result_log,
                result,
                ReportKind::Result,
                palette,
                72.0,
            ));
            if result.starts_with("Restored snapshot") {
                feedback = feedback
                    .push(secondary_button("Check for problems", palette).on_press(Event::Checkup));
            }
        }
        if self.status.is_some() || self.error.is_some() || self.result.is_some() {
            content = content.push(section(feedback, palette));
        }
        scrollable(container(content).padding(24).width(Fill))
            .height(Fill)
            .into()
    }
}

fn read_choices(paths: &Paths) -> Result<Vec<Choice>, String> {
    let names = environment::names(paths).map_err(|error| error.to_string())?;
    let mut choices = Vec::new();
    for name in names {
        match Environment::load(paths, &name) {
            Ok(env) if env.config.wine().is_some() => choices.push(Choice {
                name,
                problem: None,
            }),
            Ok(_) => {}
            Err(error) => choices.push(Choice {
                name,
                problem: Some(paths.contract_text(&error.to_string())),
            }),
        }
    }
    Ok(choices)
}

fn read_selected(paths: &Paths, name: &str) -> Result<Loaded, String> {
    let recovery = match fs::symlink_metadata(paths.restore_marker(name)) {
        Ok(_) => match snapshot::inspect_recovery(paths, name) {
            Ok(()) => Recovery::Ready,
            Err(error) => Recovery::Invalid(paths.contract_text(&error.to_string())),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Recovery::None,
        Err(error) => Recovery::Invalid(paths.contract_text(&error.to_string())),
    };
    let snapshots =
        snapshot::list(paths, name).map_err(|error| paths.contract_text(&error.to_string()));
    Ok(Loaded {
        snapshots,
        recovery,
    })
}

fn wait_idle(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    token: &Token,
) -> anyhow::Result<()> {
    let env = Environment::load(paths, name)?;
    let wine = env.wine()?;
    let (runner, _build_guard) =
        Runners::new(paths, global)?.resolve_locked(paths, &wine.runner)?;
    runner.wait_idle_for_copy(&env.prefix(paths)?, token)?;
    Ok(())
}

fn progress_status(progress: &ImportProgress) -> String {
    match progress {
        ImportProgress::Checking { entries } => format!("Checking {entries} entries..."),
        ImportProgress::Started { entries } => format!("Copying {entries} entries..."),
        ImportProgress::Copied { path } => format!("Copying {}...", path.display()),
        ImportProgress::Finished { .. } => "Copy complete.".into(),
    }
}

fn event_message(generation: u64, event: SnapshotEvent) -> Event {
    match event {
        SnapshotEvent::Progress(progress) => {
            Event::Progress(generation, progress_status(&progress))
        }
        SnapshotEvent::Finishing => Event::Finishing(generation),
    }
}

fn layout_label(layout: ImportLayout) -> &'static str {
    match layout {
        ImportLayout::Wine => "Wine",
        ImportLayout::Proton => "Proton compatdata",
    }
}

fn review_view<'a>(
    review: &'a Review,
    report: &'a str,
    report_log: &'a LogView,
    show_config: bool,
    palette: Palette,
    enabled: bool,
) -> Element<'a, Event> {
    let mut content = column![].spacing(8);
    match review {
        Review::Create(review) => {
            content = content
                .push(section_heading("Create snapshot", palette))
                .push(text(format!("Create snapshot {}?", review.label)))
                .push(
                    text("Snapshots need reflinks. External files aren't included.")
                        .color(palette.warning),
                )
                .push(report_panel(
                    report_log,
                    report,
                    ReportKind::Review,
                    palette,
                    REPORT_HEIGHT,
                ));
        }
        Review::Restore(review) => {
            content = content.push(section_heading("Restore snapshot", palette))
                .push(text(format!("Restore {} from {}?", review.name, review.label)))
                .push(text("The prefix and config will be replaced. External files, art, and playtime stay in place.").color(palette.warning))
                .push(secondary_button(if show_config { "Hide config changes" } else { "Show config changes" }, palette).on_press(Event::ShowConfig(!show_config)))
                .push(report_panel(
                    report_log,
                    report,
                    ReportKind::Review,
                    palette,
                    REPORT_HEIGHT,
                ));
        }
        Review::Delete(label) => {
            content = content
                .push(section_heading("Delete snapshot", palette))
                .push(text(format!("Delete snapshot {label}?")))
                .push(text("It cannot be restored from Trash.").color(palette.warning));
        }
        Review::Recover => {
            content = content
                .push(section_heading("Recover interrupted restore", palette))
                .push(text("Roll back the interrupted restore?"))
                .push(text("This restores the old prefix and config.").color(palette.warning));
        }
    }
    content = content.push(
        row![
            secondary_button("Cancel", palette).on_press_maybe(enabled.then_some(Event::Dismiss)),
            accent_button(
                match review {
                    Review::Create(_) => "Create",
                    Review::Restore(_) => "Restore",
                    Review::Delete(_) => "Delete",
                    Review::Recover => "Roll back restore",
                },
                if matches!(review, Review::Delete(_)) {
                    palette.danger
                } else {
                    palette.primary
                },
                enabled,
                Event::Confirm,
            ),
        ]
        .spacing(8),
    );
    container(content).width(Fill).into()
}

fn report_panel<'a>(
    log: &'a LogView,
    report: &str,
    kind: ReportKind,
    palette: Palette,
    height: f32,
) -> Element<'a, Event> {
    column![
        logview::controls(
            log,
            !report.is_empty(),
            Event::CopyReport(kind),
            Event::FollowReport(kind),
            palette,
        ),
        logview::editor(
            log,
            palette,
            height,
            report_id(kind),
            move |action| Event::ReportAction(kind, action),
            move |viewport| Event::ReportScrolled(kind, viewport),
        ),
    ]
    .spacing(6)
    .into()
}

fn report_id(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Review => REPORT_ID,
        ReportKind::Result => RESULT_ID,
    }
}

fn copied_report(log: &mut LogView, report: &str) -> String {
    log.mark_copied();
    report.to_owned()
}

fn review_report(paths: &Paths, review: &Review, show_config: bool) -> String {
    match review {
        Review::Create(review) => {
            let mut report = format!(
                "Prefix: {}\nRunner: {}\nLayout: {}\nDestination: {}\n",
                paths.contract_text(&review.prefix.display().to_string()),
                review.runner,
                layout_label(review.layout),
                paths.contract_text(&review.snapshot.display().to_string()),
            );
            if review.external_dependencies.is_empty() {
                report.push_str("External links: none\n");
            } else {
                for link in &review.external_dependencies {
                    report.push_str(&format!(
                        "External link: {} -> {}\n",
                        link.source.display(),
                        link.target.display()
                    ));
                }
            }
            report
        }
        Review::Restore(review) => {
            let mut report = format!(
                "Prefix: {}\nSnapshot: {}\nRunner: {} -> {}\nLayout: {}\n",
                paths.contract_text(&review.prefix.display().to_string()),
                paths.contract_text(&review.snapshot.display().to_string()),
                review.current_runner,
                review.saved_runner,
                layout_label(review.layout),
            );
            if show_config {
                report.push_str("\nCurrent config:\n");
                report.push_str(&review.current_config);
                report.push_str("\n\nSaved config:\n");
                report.push_str(&review.saved_config);
                report.push('\n');
            }
            report
        }
        Review::Delete(_) | Review::Recover => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text_editor::{Action, Edit};
    use std::sync::Arc;

    fn isolated_paths() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().as_os_str().to_owned();
        let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.clone())).unwrap();
        (root, paths)
    }

    #[test]
    fn large_review_report_stays_complete_read_only_and_bounded() {
        let report = (0..5_000)
            .map(|index| format!("entry {index}\n"))
            .collect::<String>();
        let mut log = LogView::default();
        assert!(log.resync(&report));

        assert_eq!(log.content().text(), report);
        assert!(log.content().text().ends_with("entry 4999\n"));
        assert!(!log.perform(
            Action::Edit(Edit::Paste(Arc::new("changed".into()))),
            || unreachable!(),
        ));
        assert_eq!(log.content().text(), report);
        assert_eq!(copied_report(&mut log, &report), report);
        assert!(log.copied());

        let editor = logview::editor(
            &log,
            crate::theme::Palette::DARK,
            REPORT_HEIGHT,
            REPORT_ID,
            |action| Event::ReportAction(ReportKind::Review, action),
            |viewport| Event::ReportScrolled(ReportKind::Review, viewport),
        );
        assert_eq!(
            editor.as_widget().size().height,
            iced::Length::Fixed(REPORT_HEIGHT)
        );
    }

    #[test]
    fn stale_worker_events_do_not_change_page() {
        let (mut page, _) = Page::new(test_paths());
        let _ = page.update(Event::Progress(999, "old".into()));
        let _ = page.update(Event::Finished(999, Ok(Outcome::Recovered)));
        assert!(!page.busy());
        assert!(page.take_outcome().is_none());
    }

    #[test]
    fn cancel_does_not_interrupt_finishing_work() {
        let (mut page, _) = Page::new(test_paths());
        let (generation, token) = page.begin_work("working", true);
        let _ = page.update(Event::Finishing(generation));
        page.cancel();
        assert!(page.finishing());
        assert!(!token.is_canceled());
    }

    #[test]
    fn picker_includes_broken_registrations_and_skips_native() {
        let (_root, paths) = isolated_paths();
        fs::create_dir_all(paths.environments_dir()).unwrap();
        fs::write(paths.environment_config("broken"), "this is invalid").unwrap();
        fs::write(paths.environment_config("native"), "kind = 'native'\n").unwrap();
        let choices = read_choices(&paths).unwrap();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].name, "broken");
        assert!(choices[0].problem.is_some());
    }

    #[test]
    fn malformed_restore_marker_is_not_offered_for_rollback() {
        let (_root, paths) = isolated_paths();
        let marker = paths.restore_marker("broken");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, "bad marker").unwrap();
        let loaded = read_selected(&paths, "broken").unwrap();
        assert!(matches!(loaded.recovery, Recovery::Invalid(_)));
    }

    #[test]
    fn selection_cannot_replace_active_worker() {
        let (mut page, _) = Page::new(test_paths());
        page.selected = Some("first".into());
        let (generation, _) = page.begin_work("working", true);
        let _ = page.select("second".into());
        assert_eq!(page.selected_name(), Some("first"));
        assert_eq!(page.generation, generation);
    }

    fn test_paths() -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| "/tmp".into())).unwrap()
    }
}
