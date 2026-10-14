//! Updating a GOG game, or finishing an interrupted update or install, like
//! `wineo update NAME`: a review of what changes, the update with progress
//! and cancel, then what changed and which of your files it touched. A
//! Windows installer that never started is offered once as its own setup
//! window.

use std::time::Duration;

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{column, container, row, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::cancel::Token;
use wineo_core::gog::account::{AccountError, Session};
use wineo_core::gog::api::Client;
use wineo_core::gog::folder::JournalKind;
use wineo_core::gog::update::windows::SetupError;
use wineo_core::gog::update::{
    self, Options, Pending, Plan, Reporter, ReviewLabel, UpdateError, Updated,
};
use wineo_core::key_store::Keyring;
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;

use super::Action;
use super::heroic;
use crate::task;
use crate::theme::Palette;
use crate::ui::{accent_base, secondary_button};

/// What a logged-out update that can't finish from saved packages says.
pub(crate) const LOGGED_OUT: &str = "Log in to GOG first, in Settings.";

const CANCELED: &str = "Update canceled.";

pub(super) const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Planning,
    Review,
    Working,
    /// Asking whether to run an installer that never started as its own
    /// setup window.
    Question,
    Finished,
}

/// What planning found, cloneable for the page's events.
#[derive(Debug, Clone)]
pub(crate) enum Planned {
    UpToDate,
    Update(Box<Pending>),
}

/// Why an update didn't finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// Canceled. `journal` says whether it had started changing the game
    /// folder, which leaves an update to finish.
    Canceled {
        journal: bool,
    },
    /// The game's Windows installer never started on a first attempt, and
    /// running it as its own setup window may get further. `message` says
    /// what happened, for when that's declined.
    NotStarted {
        label: String,
        message: String,
    },
    Failed(String),
}

impl Failure {
    #[cfg(test)]
    fn message(&self, name: &str) -> String {
        self.message_for(name, false)
    }

    /// What the page says, naming an interrupted install as one.
    fn message_for(&self, name: &str, install: bool) -> String {
        match self {
            Self::Canceled { journal: true } if install => {
                format!("Install interrupted. Use Finish install on {name} to finish it.")
            }
            Self::Canceled { journal: true } => {
                format!("Update interrupted. Use Finish update on {name} to finish it.")
            }
            Self::Canceled { journal: false } => CANCELED.to_owned(),
            Self::NotStarted { message, .. } | Self::Failed(message) => message.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    Planned(u64, Result<Planned, String>),
    Update,
    /// The answer to the setup window question.
    SetupWindow(bool),
    Cancel,
    Force,
    Progress(u64, String),
    Finished(u64, Result<Box<Updated>, Failure>),
    BackToLibrary,
}

pub(crate) struct Page {
    paths: Paths,
    name: String,
    step: Step,
    generation: u64,
    pending: Option<Box<Pending>>,
    updated: Option<Box<Updated>>,
    /// Set once an update finished, until the app takes it to clear the
    /// card's badge.
    done: Option<String>,
    token: Option<Token>,
    status: String,
    error: Option<String>,
    note: Option<String>,
    /// The installer the setup window question is about.
    question: Option<Failure>,
    /// Set while planning again to run the setup window, which then starts
    /// without another review.
    wizard: bool,
    action: Option<Action>,
}

impl Page {
    pub(crate) fn new(paths: Paths, name: String) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            name,
            step: Step::Planning,
            generation: 0,
            pending: None,
            updated: None,
            done: None,
            token: None,
            status: String::new(),
            error: None,
            note: None,
            question: None,
            wizard: false,
            action: None,
        };
        let task = page.plan();
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

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    /// The environment an update just finished for, once.
    pub(crate) fn take_updated(&mut self) -> Option<String> {
        self.done.take()
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        match self.step {
            Step::Review => self.start(),
            Step::Question => self.update(Event::SetupWindow(true)),
            Step::Finished => self.update(Event::BackToLibrary),
            Step::Planning | Step::Working => Task::none(),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        if self.busy() {
            return Task::none();
        }
        self.next_generation();
        self.status.clear();
        self.exit();
        Task::none()
    }

    fn exit(&mut self) {
        self.action = Some(Action::Library {
            name: Some(self.name.clone()),
            launch: false,
        });
    }

    fn plan(&mut self) -> Task<Event> {
        self.step = Step::Planning;
        self.status = format!("Checking {} with GOG...", self.name);
        let generation = self.next_generation();
        let (paths, name) = (self.paths.clone(), self.name.clone());
        // Planning is work too: Back waits for it, Cancel stops it, and
        // closing the window asks first.
        let token = Token::new();
        self.token = Some(token.clone());
        let options = Options {
            wizard: self.wizard,
            ..Options::default()
        };
        task::blocking(
            move || {
                let client =
                    wineo_core::gog::client_from_env().map_err(|error| error.to_string())?;
                plan_with_options(&paths, &keyring(), client, &name, options, &token)
            },
            move |result| Event::Planned(generation, result),
        )
    }

    fn start(&mut self) -> Task<Event> {
        let Some(pending) = self.pending.clone() else {
            return Task::none();
        };
        if self.busy() || self.waiting() {
            return Task::none();
        }
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Working;
        self.error = None;
        self.status = format!("{} {}...", pending.action(), self.name);
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            task::join(
                move || {
                    let client = wineo_core::gog::client_from_env()
                        .map_err(|error| Failure::Failed(error.to_string()))?;
                    apply_with(&paths, &keyring(), client, &pending, &token, |line| {
                        let _ = sender.unbounded_send(Event::Progress(generation, line));
                    })
                    .map(Box::new)
                },
                |error| Failure::Failed(error.to_string()),
            ),
            move |result| Event::Finished(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Back => self.back(),
            Event::Planned(generation, result) if generation == self.generation => {
                self.status.clear();
                self.token = None;
                match result {
                    Ok(Planned::UpToDate) => {
                        self.note = Some("Up to date.".to_owned());
                        self.done = Some(self.name.clone());
                        self.step = Step::Finished;
                    }
                    Ok(Planned::Update(pending)) => {
                        self.pending = Some(pending);
                        self.step = Step::Review;
                        // The setup window was asked for, so it runs now.
                        if std::mem::take(&mut self.wizard) {
                            return self.start();
                        }
                    }
                    Err(error) => {
                        if self.wizard {
                            self.step = Step::Finished;
                        }
                        self.wizard = false;
                        self.error = Some(error);
                    }
                }
                Task::none()
            }
            Event::Update if self.step == Step::Review => self.start(),
            Event::SetupWindow(run) if self.step == Step::Question => {
                let Some(question) = self.question.take() else {
                    return Task::none();
                };
                if run {
                    self.wizard = true;
                    self.error = None;
                    return self.plan();
                }
                let install = self.installs();
                self.error = Some(question.message_for(&self.name, install));
                self.step = Step::Finished;
                Task::none()
            }
            Event::Cancel if matches!(self.step, Step::Planning | Step::Working) => {
                self.cancel(false);
                Task::none()
            }
            Event::Force if matches!(self.step, Step::Planning | Step::Working) => {
                self.cancel(true);
                Task::none()
            }
            Event::Progress(generation, line)
                if generation == self.generation && self.step == Step::Working =>
            {
                if !self.token.as_ref().is_some_and(Token::is_canceled) {
                    self.status = line;
                }
                Task::none()
            }
            Event::Finished(generation, result) if generation == self.generation => {
                self.token = None;
                self.status.clear();
                self.step = Step::Finished;
                match result {
                    Ok(updated) => {
                        self.done = Some(self.name.clone());
                        self.updated = Some(updated);
                    }
                    // Asked once: a setup window that also never starts is
                    // only reported.
                    Err(failure @ Failure::NotStarted { .. })
                        if self
                            .pending
                            .as_ref()
                            .is_some_and(|pending| !pending.wizard()) =>
                    {
                        self.question = Some(failure);
                        self.step = Step::Question;
                    }
                    Err(failure) => {
                        let install = self.installs();
                        self.error = Some(failure.message_for(&self.name, install));
                    }
                }
                Task::none()
            }
            Event::BackToLibrary if self.step == Step::Finished => {
                self.exit();
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// Whether this finishes an interrupted install rather than updating.
    fn installs(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.kind() == JournalKind::Install)
    }

    pub(crate) fn view(&self, palette: Palette) -> Element<'_, Event> {
        let title = match (self.step, self.pending.as_deref()) {
            (Step::Planning, _) => format!("Update {}", self.name),
            (_, Some(pending)) => format!("{} {}", pending.action(), self.name),
            (_, None) => format!("Update {}", self.name),
        };
        let heading = text(title)
            .size(22)
            .font(iced::Font {
                weight: iced::font::Weight::Semibold,
                ..iced::Font::DEFAULT
            })
            .color(palette.foreground);
        let mut body = column![heading].spacing(14).width(Fill);
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
        if let Some(note) = &self.note {
            body = body.push(text(note));
        }
        if let Some(Failure::NotStarted { label, .. }) = &self.question {
            body = body.push(text(setup_window_question(label)));
        }
        match (self.step, &self.pending, &self.updated) {
            (Step::Review | Step::Working, Some(pending), _) => {
                let mut summary = column![].spacing(8);
                for (label, value) in review_rows(pending) {
                    summary = summary.push(
                        row![
                            text(label).color(palette.muted).width(160),
                            text(value).width(Fill)
                        ]
                        .spacing(12),
                    );
                }
                body = body.push(container(summary).padding(16).width(Fill).style(move |_| {
                    container::Style::default()
                        .background(palette.surface)
                        .border(border::rounded(12).width(1).color(palette.border))
                }));
                for warning in pending.warnings() {
                    body = body.push(text(warning).size(13).color(palette.warning));
                }
            }
            (Step::Finished, Some(pending), Some(updated)) => {
                for line in result_lines(pending, updated) {
                    body = body.push(text(line));
                }
                for (heading, items) in result_lists(updated) {
                    let mut list = column![crate::ui::section_heading(heading, palette)].spacing(4);
                    for item in items {
                        list = list.push(text(item).size(13));
                    }
                    body = body.push(list);
                }
                // The plan's warnings came first, and the review showed them.
                for warning in updated.warnings.iter().skip(pending.warnings().len()) {
                    body = body.push(text(warning).size(13).color(palette.warning));
                }
            }
            _ => {}
        }
        let back = secondary_button("Back", palette)
            .on_press_maybe((!self.busy() && self.step != Step::Finished).then_some(Event::Back));
        let mut page = column![row![back], heroic::scroll_body(body, None, palette)]
            .spacing(12)
            .max_width(heroic::PAGE_MAX_WIDTH)
            .height(Fill);
        let footer: Option<Element<'_, Event>> = match self.step {
            Step::Review => self.pending.as_ref().map(|pending| {
                let label = if pending.is_recovery() {
                    "Finish"
                } else {
                    "Update"
                };
                accent_base(label, palette.primary)
                    .on_press(Event::Update)
                    .into()
            }),
            Step::Planning | Step::Working => self.token.as_ref().map(|token| {
                let (label, event) = if token.is_canceled() {
                    ("Force stop", Event::Force)
                } else {
                    ("Cancel", Event::Cancel)
                };
                secondary_button(label, palette).on_press(event).into()
            }),
            Step::Question => Some(setup_window_buttons(palette, Event::SetupWindow)),
            Step::Finished => Some(
                secondary_button("Back to library", palette)
                    .on_press(Event::BackToLibrary)
                    .into(),
            ),
        };
        if let Some(footer) = footer {
            page = page.push(container(footer).padding(12).width(Fill).style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(8).width(1).color(palette.border))
            }));
        }
        heroic::page_frame(page)
    }
}

fn keyring() -> SecretServiceKeyring {
    SecretServiceKeyring::gog()
}

/// The question offered when a Windows installer never started.
pub(crate) fn setup_window_question(label: &str) -> String {
    format!("{label}'s installer didn't start. Run its own setup window?")
}

/// The answers to `setup_window_question`, the forward one accented.
pub(crate) fn setup_window_buttons<'a, E: Clone + 'a>(
    palette: Palette,
    answer: impl Fn(bool) -> E,
) -> Element<'a, E> {
    row![
        secondary_button("No", palette).on_press(answer(false)),
        accent_base("Run setup window", palette.primary).on_press(answer(true)),
    ]
    .spacing(12)
    .align_y(Center)
    .wrap()
    .vertical_spacing(8)
    .into()
}

/// Plans updating `name`. Logged out, a game whose interrupted update has
/// its packages saved can still finish; anything else needs the login.
/// GOG's answers can't be interrupted, so a cancel takes effect once they
/// arrive, and the plan is then dropped.
#[cfg(test)]
pub(crate) fn plan_with(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    name: &str,
    token: &Token,
) -> Result<Planned, String> {
    plan_with_options(paths, keyring, client, name, Options::default(), token)
}

/// `plan_with`, with `options` saying whether a Windows installer runs as
/// its own setup window.
pub(crate) fn plan_with_options(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    name: &str,
    options: Options,
    token: &Token,
) -> Result<Planned, String> {
    let mut session = Session::open(paths, keyring, client).map_err(|error| error.to_string())?;
    let logged_out = session.is_none();
    let planned = update::plan(paths, session.as_mut(), name, options, token);
    if token.is_canceled() {
        return Err(CANCELED.to_owned());
    }
    match planned {
        Ok(Plan::UpToDate { .. }) => Ok(Planned::UpToDate),
        Ok(Plan::Update(pending)) => Ok(Planned::Update(Box::new(pending))),
        Err(error)
            if logged_out
                && matches!(
                    error.downcast_ref::<AccountError>(),
                    Some(AccountError::LoggedOut)
                ) =>
        {
            Err(LOGGED_OUT.to_owned())
        }
        Err(error) => Err(format!("{error:#}")),
    }
}

/// Applies `pending`, sending a status line for each step and about ten a
/// second while bytes move.
pub(crate) fn apply_with(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    pending: &Pending,
    token: &Token,
    mut say: impl FnMut(String),
) -> Result<Updated, Failure> {
    let mut session = Session::open(paths, keyring, client)
        .map_err(|error| Failure::Failed(error.to_string()))?;
    let logged_out = session.is_none();
    let mut reporter = Reporter::new(pending.product_labels(), PROGRESS_INTERVAL);
    update::apply(paths, session.as_mut(), pending, token, |step| {
        if let Some(line) = reporter.line(&step) {
            say(line);
        }
    })
    .map_err(|error| {
        if let Some(UpdateError::Canceled { journal }) = error.downcast_ref::<UpdateError>() {
            return Failure::Canceled { journal: *journal };
        }
        if let Some(failure) = setup_failure(&error, pending) {
            return failure;
        }
        if logged_out
            && matches!(
                error.downcast_ref::<AccountError>(),
                Some(AccountError::LoggedOut)
            )
        {
            return Failure::Failed(LOGGED_OUT.to_owned());
        }
        Failure::Failed(format!("{error:#}"))
    })
}

/// How a Windows installer that stopped on its own left things, like
/// `wineo update` says it. An installer that never started on a first
/// attempt changed nothing, since the update drops its journal then;
/// finishing one keeps it.
fn setup_failure(error: &anyhow::Error, pending: &Pending) -> Option<Failure> {
    let name = pending.environment();
    let recovery = pending.is_recovery();
    let finish = if pending.kind() == JournalKind::Install {
        format!("Use Finish install on {name} to finish it.")
    } else {
        format!("Use Finish update on {name} to finish it.")
    };
    match error.downcast_ref::<SetupError>()? {
        SetupError::Canceled { .. } if recovery => Some(Failure::Canceled { journal: true }),
        SetupError::Canceled { .. } => Some(Failure::Canceled { journal: false }),
        SetupError::NotStarted {
            label,
            code,
            wizard,
            ..
        } => {
            let message = if recovery {
                format!("{label}'s installer never started (exit code {code}). {finish}")
            } else {
                format!("{label}'s installer never started (exit code {code}), so nothing changed.")
            };
            Some(if *wizard {
                Failure::NotStarted {
                    label: label.clone(),
                    message,
                }
            } else {
                Failure::Failed(message)
            })
        }
        // Its own message names the CLI's way to finish it.
        SetupError::Partway { label, detail, .. } => Some(Failure::Failed(format!(
            "{label}'s installer stopped partway ({detail}). {finish}"
        ))),
        _ => None,
    }
}

/// The review's rows, the ones `wineo update NAME` prints.
pub(crate) fn review_rows(pending: &Pending) -> Vec<(String, String)> {
    pending
        .review_rows(|path| path.display().to_string())
        .into_iter()
        .map(|(label, value)| {
            let label = match label {
                ReviewLabel::Folder => "Folder".to_owned(),
                ReviewLabel::Product(label) => label,
                ReviewLabel::Owner => "Owner".to_owned(),
                ReviewLabel::Download => "Download".to_owned(),
                ReviewLabel::Unpacked => "Unpacked".to_owned(),
            };
            (label, value)
        })
        .collect()
}

/// What a finished update says, before the lists of your files.
pub(crate) fn result_lines(pending: &Pending, updated: &Updated) -> Vec<String> {
    let mut lines = vec![format!("{}.", pending.finished())];
    lines.extend(
        pending
            .changes(updated)
            .into_iter()
            .map(|(label, change)| format!("{label}: {change}")),
    );
    if updated.claimed {
        lines.push(format!(
            "{} now owns {}.",
            updated.environment,
            updated.folder.display()
        ));
    }
    lines
}

/// The files of yours an update touched, under the CLI's headings.
pub(crate) fn result_lists(updated: &Updated) -> Vec<(&'static str, Vec<String>)> {
    let mut lists = Vec::new();
    if !updated.saved.is_empty() {
        lists.push((
            "Moved your files aside where the new version has files:",
            updated
                .saved
                .iter()
                .map(|saved| format!("{} -> {}", saved.path, saved.saved_as))
                .collect(),
        ));
    }
    if !updated.kept.is_empty() {
        lists.push((
            "Kept these old files you changed, which the new version no longer ships:",
            updated.kept.clone(),
        ));
    }
    if !updated.overwritten.is_empty() {
        lists.push((
            "Overwrote these files that differed from the new version:",
            updated.overwritten.clone(),
        ));
    }
    lists
}

#[cfg(test)]
mod tests;
