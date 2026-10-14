//! Updating a GOG game, or finishing an interrupted update or install, like
//! `wineo update NAME`: a review of what changes, the update with progress
//! and cancel, then what changed and which of your files it touched. A
//! Windows installer that never started is offered once as its own setup
//! window.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{column, container, row, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::cancel::Token;
use wineo_core::gog::account::{AccountError, Session};
use wineo_core::gog::api::Client;
use wineo_core::gog::folder::JournalKind;
use wineo_core::gog::update::windows::SetupError;
use wineo_core::gog::update::{self, Options, Pending, Plan, Progress, UpdateError, Updated};
use wineo_core::key_store::Keyring;
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;

use super::Action;
use super::heroic;
use crate::size::format_bytes;
use crate::theme::Palette;
use crate::ui::{accent_base, secondary_button};

/// What a logged-out update that can't finish from saved packages says.
pub(crate) const LOGGED_OUT: &str = "Log in to GOG first, in Settings.";

const CANCELED: &str = "Update canceled.";

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

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
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let client =
                        wineo_core::gog::client_from_env().map_err(|error| error.to_string())?;
                    plan_with_options(&paths, &keyring(), client, &name, options, &token)
                })
                .await
                .map_err(|error| error.to_string())?
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
        self.status = format!("{} {}...", action(&pending), self.name);
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let client = wineo_core::gog::client_from_env()
                        .map_err(|error| Failure::Failed(error.to_string()))?;
                    apply_with(&paths, &keyring(), client, &pending, &token, |line| {
                        let _ = sender.unbounded_send(Event::Progress(generation, line));
                    })
                    .map(Box::new)
                })
                .await
                .map_err(|error| Failure::Failed(error.to_string()))?
            },
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
            (_, Some(pending)) => format!("{} {}", action(pending), self.name),
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
    let mut reporter = Reporter::new(pending);
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

/// How the review and the result name what is being done.
fn action(pending: &Pending) -> &'static str {
    match (pending.kind(), pending.is_recovery()) {
        (JournalKind::Reinstall, false) => "Reinstall",
        (JournalKind::Reinstall, true) => "Finish the interrupted reinstall of",
        (JournalKind::Install, true) => "Finish the interrupted install of",
        (_, true) => "Finish the interrupted update of",
        (_, false) => "Update",
    }
}

/// The base game's title, which a recovery from saved packages doesn't
/// have: then the game is named by its environment. A DLC is `NAME (ID)`.
fn product_label(pending: &Pending, id: u64, title: Option<&str>) -> String {
    if id != pending.target().gog_id {
        return pending.dlc_label(id);
    }
    title.map_or_else(|| pending.environment().to_owned(), str::to_owned)
}

/// `1.0 -> 1.1`, or the version alone when a rebuild installs the same one.
fn change(installed: Option<&str>, version: &str) -> String {
    match installed {
        Some(installed) if installed == version => format!("{version} (rebuilt)"),
        Some(installed) => format!("{installed} -> {version}"),
        None => format!("unknown -> {version}"),
    }
}

/// The review's rows, the ones `wineo update NAME` prints.
pub(crate) fn review_rows(pending: &Pending) -> Vec<(String, String)> {
    let mut rows = vec![("Folder".to_owned(), pending.folder().display().to_string())];
    for product in pending.products() {
        let label = product_label(pending, product.id, product.title.as_deref());
        let value = match &product.version {
            Some(version) => change(product.installed.as_deref(), version),
            None => "left as it is".to_owned(),
        };
        rows.push((label, value));
    }
    if pending.claims() {
        rows.push((
            "Owner".to_owned(),
            format!("none yet, {} takes this folder", pending.environment()),
        ));
    }
    rows.push((
        "Download".to_owned(),
        if pending.is_offline() {
            "none, uses the packages already downloaded".to_owned()
        } else {
            format_bytes(pending.download_size())
        },
    ));
    rows.push((
        "Unpacked".to_owned(),
        format!(
            "about {} (an estimate)",
            format_bytes(pending.unpacked_estimate())
        ),
    ));
    rows
}

/// What a finished update says, before the lists of your files.
pub(crate) fn result_lines(pending: &Pending, updated: &Updated) -> Vec<String> {
    let name = pending.environment();
    let mut lines = vec![match (pending.kind(), pending.is_recovery()) {
        (JournalKind::Reinstall, false) => format!("Reinstalled {name}."),
        (JournalKind::Reinstall, true) => format!("Finished the interrupted reinstall of {name}."),
        (JournalKind::Install, true) => format!("Finished the interrupted install of {name}."),
        (_, true) => format!("Finished the interrupted update of {name}."),
        (_, false) => format!("Updated {name}."),
    }];
    for product in &updated.products {
        let Some(version) = &product.version else {
            continue;
        };
        let planned = pending.products().iter().find(|p| p.id == product.id);
        let label = product_label(
            pending,
            product.id,
            planned.and_then(|p| p.title.as_deref()),
        );
        let installed = planned.and_then(|p| p.installed.as_deref());
        lines.push(format!("{label}: {}", change(installed, version)));
    }
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

/// Turns progress into status lines like the CLI's, one per stage and the
/// rest paced.
pub(super) struct Reporter {
    labels: HashMap<u64, String>,
    last: Option<Instant>,
    file: Option<(u64, String)>,
    verified: HashSet<u64>,
    unpacking: HashSet<u64>,
    publishing: bool,
}

impl Reporter {
    fn new(pending: &Pending) -> Self {
        Self::with_labels(
            pending
                .products()
                .iter()
                .map(|product| {
                    (
                        product.id,
                        product_label(pending, product.id, product.title.as_deref()),
                    )
                })
                .collect(),
        )
    }

    /// Names each product, by id, the way the review does.
    pub(super) fn with_labels(labels: HashMap<u64, String>) -> Self {
        Self {
            labels,
            last: None,
            file: None,
            verified: HashSet::new(),
            unpacking: HashSet::new(),
            publishing: false,
        }
    }

    fn label(&self, product: u64) -> String {
        self.labels
            .get(&product)
            .cloned()
            .unwrap_or_else(|| format!("GOG product {product}"))
    }

    fn due(&self) -> bool {
        self.last
            .is_none_or(|last| last.elapsed() >= PROGRESS_INTERVAL)
    }

    fn said(&mut self, line: String) -> Option<String> {
        self.last = Some(Instant::now());
        Some(line)
    }

    pub(super) fn line(&mut self, step: &Progress) -> Option<String> {
        match step {
            Progress::Download {
                product,
                file_id,
                done,
                total,
            } => {
                let key = (*product, file_id.clone());
                let first = self.file.as_ref() != Some(&key);
                self.file = Some(key);
                (first || done == total || self.due()).then_some(())?;
                let line = format!(
                    "Downloading {}: {} / {}",
                    self.label(*product),
                    format_bytes(*done),
                    format_bytes(*total)
                );
                self.said(line)
            }
            Progress::Verify { product } => {
                self.verified.insert(*product).then_some(())?;
                let line = format!("Verifying {}", self.label(*product));
                self.said(line)
            }
            Progress::Stage {
                product,
                done,
                total,
            } => {
                let line = if self.unpacking.insert(*product) {
                    format!("Unpacking {}", self.label(*product))
                } else {
                    self.due().then_some(())?;
                    format!(
                        "Unpacking {}: {} / {}",
                        self.label(*product),
                        format_bytes(*done),
                        format_bytes(*total)
                    )
                };
                self.said(line)
            }
            Progress::Publish { done, total } => {
                let line = if !self.publishing {
                    self.publishing = true;
                    format!("Publishing {total} files")
                } else {
                    (done == total || self.due()).then_some(())?;
                    format!("Published {done} of {total} files")
                };
                self.said(line)
            }
            Progress::Installing {
                product,
                interactive,
            } => {
                let line = if *interactive {
                    format!("Opening {}'s setup window", self.label(*product))
                } else {
                    format!("Running {}'s installer", self.label(*product))
                };
                self.said(line)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::ffi::OsString;
    use std::fs;

    use wineo_core::gog::folder::{Product, ProductStatus};
    use wineo_core::gog::update::SavedAside;

    use super::*;
    use crate::gog_fake::{
        native_game, native_game_with_dlc, native_product, native_product_with_dlc, product_server,
        save_login,
    };
    use crate::settings::steam_key::tests::TestKeyring;

    const RIMWORLD: u64 = 1094900565;

    fn setup() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
                .unwrap();
        (root, paths)
    }

    fn keyring() -> TestKeyring {
        TestKeyring(RefCell::new(None))
    }

    /// RimWorld at 1.0 with GOG offering 1.1, and its planned update.
    fn planned() -> (tempfile::TempDir, Paths, Box<Pending>) {
        let (root, paths) = setup();
        native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.0", false);
        let keyring = keyring();
        save_login(&paths, &keyring);
        let server = product_server(vec![native_product(RIMWORLD, "RimWorld", "1.1")]);
        let planned =
            plan_with(&paths, &keyring, server.client(), "rimworld", &Token::new()).unwrap();
        let Planned::Update(pending) = planned else {
            panic!("expected an update");
        };
        (root, paths, pending)
    }

    #[test]
    fn the_review_shows_versions_sizes_and_the_estimate() {
        let (_root, _paths, pending) = planned();
        assert!(!pending.is_recovery());
        let rows = review_rows(&pending);
        assert_eq!(rows[0].0, "Folder");
        assert!(rows[0].1.ends_with("Games/rimworld"), "{}", rows[0].1);
        assert_eq!(rows[1], ("RimWorld".to_owned(), "1.0 -> 1.1".to_owned()));
        assert_eq!(rows[2], ("Download".to_owned(), format_bytes(7340032)));
        assert_eq!(
            rows[3],
            (
                "Unpacked".to_owned(),
                format!("about {} (an estimate)", format_bytes(7340032))
            )
        );
        assert_eq!(action(&pending), "Update");
    }

    const ROYALTY: u64 = 1233017772;

    /// RimWorld with Royalty installed at 1.0 and GOG offering 1.1 of both,
    /// Royalty titled `gog_title`, and `info` as its `goggame-*.info` name.
    fn planned_with_dlc(gog_title: &str, info: Option<&str>) -> Vec<(String, String)> {
        let (root, paths) = setup();
        let folder = native_game_with_dlc(
            &paths,
            root.path(),
            "rimworld",
            RIMWORLD,
            "1.0",
            &[ROYALTY],
            false,
        );
        if let Some(name) = info {
            fs::create_dir_all(folder.join("game")).unwrap();
            fs::write(
                folder.join(format!("game/goggame-{ROYALTY}.info")),
                serde_json::json!({"gameId": ROYALTY.to_string(), "name": name}).to_string(),
            )
            .unwrap();
        }
        let keyring = keyring();
        save_login(&paths, &keyring);
        let server = product_server(vec![native_product_with_dlc(
            RIMWORLD,
            "RimWorld",
            "1.1",
            &[(ROYALTY, gog_title)],
        )]);
        let planned =
            plan_with(&paths, &keyring, server.client(), "rimworld", &Token::new()).unwrap();
        let Planned::Update(pending) = planned else {
            panic!("expected an update");
        };
        let rows = review_rows(&pending);
        // Skip the folder row and keep the product rows.
        rows[1..3].to_vec()
    }

    #[test]
    fn a_dlc_row_is_its_gog_title_and_id() {
        let rows = planned_with_dlc("RimWorld - Royalty", Some("Something else"));
        assert_eq!(rows[0].0, "RimWorld");
        assert_eq!(
            rows[1],
            ("Royalty (1233017772)".to_owned(), "1.0 -> 1.1".to_owned())
        );
    }

    #[test]
    fn a_dlc_gog_gives_no_title_takes_the_folder_s_name_or_shows_its_id() {
        let rows = planned_with_dlc("", Some("RimWorld - Royalty"));
        assert_eq!(rows[1].0, "Royalty (1233017772)");
        let rows = planned_with_dlc("", None);
        assert_eq!(rows[1].0, "DLC (1233017772)");
    }

    #[test]
    fn up_to_date_and_logged_out_say_so() {
        let (root, paths) = setup();
        native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.1", false);
        let keyring = keyring();
        let server = product_server(vec![native_product(RIMWORLD, "RimWorld", "1.1")]);
        let error =
            plan_with(&paths, &keyring, server.client(), "rimworld", &Token::new()).unwrap_err();
        assert_eq!(error, LOGGED_OUT);
        save_login(&paths, &keyring);
        assert!(matches!(
            plan_with(&paths, &keyring, server.client(), "rimworld", &Token::new()),
            Ok(Planned::UpToDate)
        ));
    }

    fn page_with(pending: Box<Pending>, paths: Paths) -> Page {
        let (mut page, _) = Page::new(paths, "rimworld".to_owned());
        let generation = page.generation;
        let _ = page.update(Event::Planned(generation, Ok(Planned::Update(pending))));
        page
    }

    fn updated() -> Updated {
        Updated {
            environment: "rimworld".to_owned(),
            folder: "/games/rimworld".into(),
            kind: JournalKind::Update,
            recovered: false,
            claimed: true,
            products: vec![Product {
                reported_language: None,
                id: RIMWORLD,
                version: Some("1.1".to_owned()),
                build: None,
                language: None,
                status: ProductStatus::Installed,
            }],
            saved: vec![SavedAside {
                path: "Data/Core/About.xml".to_owned(),
                saved_as: "Data/Core/About.xml.wineo-saved".to_owned(),
            }],
            kept: vec!["Mods/old.dll".to_owned()],
            overwritten: vec!["RimWorldLinux".to_owned()],
            warnings: vec!["Kept the downloaded GOG packages".to_owned()],
        }
    }

    #[test]
    fn an_update_shows_what_changed_and_your_files() {
        let (_root, paths, pending) = planned();
        let mut page = page_with(pending.clone(), paths);
        assert_eq!(page.step, Step::Review);
        let _ = page.update(Event::Update);
        assert_eq!(page.step, Step::Working);
        assert!(page.busy());
        assert_eq!(page.status, "Update rimworld...");
        let generation = page.generation;
        let _ = page.update(Event::Progress(
            generation,
            "Downloading RimWorld: 1.0 MiB / 7.0 MiB".to_owned(),
        ));
        assert_eq!(page.status, "Downloading RimWorld: 1.0 MiB / 7.0 MiB");
        let _ = page.update(Event::Finished(generation, Ok(Box::new(updated()))));
        assert!(!page.busy());
        assert_eq!(page.step, Step::Finished);
        assert_eq!(page.take_updated().as_deref(), Some("rimworld"));
        assert_eq!(page.take_updated(), None);
        let updated = page.updated.as_ref().unwrap();
        assert_eq!(
            result_lines(&pending, updated),
            [
                "Updated rimworld.",
                "RimWorld: 1.0 -> 1.1",
                "rimworld now owns /games/rimworld.",
            ]
        );
        let lists = result_lists(updated);
        assert_eq!(
            lists,
            [
                (
                    "Moved your files aside where the new version has files:",
                    vec!["Data/Core/About.xml -> Data/Core/About.xml.wineo-saved".to_owned()]
                ),
                (
                    "Kept these old files you changed, which the new version no longer ships:",
                    vec!["Mods/old.dll".to_owned()]
                ),
                (
                    "Overwrote these files that differed from the new version:",
                    vec!["RimWorldLinux".to_owned()]
                ),
            ]
        );
        for (width, height) in [(280.0, 240.0), (900.0, 720.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view(Palette::DARK);
            let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
            assert!(probe.outside(inset).is_empty(), "{width}x{height}");
        }
        let _ = page.update(Event::BackToLibrary);
        assert!(matches!(
            page.take_action(),
            Some(Action::Library { name: Some(name), launch: false }) if name == "rimworld"
        ));
    }

    #[test]
    fn canceling_says_whether_there_is_an_update_to_finish() {
        let (_root, paths, pending) = planned();
        for (journal, message) in [
            (
                true,
                "Update interrupted. Use Finish update on rimworld to finish it.",
            ),
            (false, "Update canceled."),
        ] {
            let mut page = page_with(pending.clone(), paths.clone());
            let _ = page.update(Event::Update);
            let _ = page.update(Event::Back);
            assert_eq!(page.step, Step::Working, "Back waits for the work");
            let _ = page.update(Event::Cancel);
            assert!(page.token.as_ref().unwrap().is_canceled());
            assert_eq!(page.status, "Canceling...");
            let generation = page.generation;
            let _ = page.update(Event::Finished(
                generation,
                Err(Failure::Canceled { journal }),
            ));
            assert_eq!(page.error.as_deref(), Some(message));
            assert_eq!(page.take_updated(), None);
        }
    }

    #[test]
    fn closing_the_window_while_updating_asks_first_then_cancels() {
        let (_root, paths, pending) = planned();
        let mut page = page_with(pending, paths.clone());
        let _ = page.update(Event::Update);
        let mut app = crate::App::boot(paths).0;
        app.flow = Some(super::super::Page::GogUpdate(Box::new(page)));
        let _ = app.update(crate::Message::Window(iced::window::Event::CloseRequested));
        assert!(app.close_prompt);
        let _ = app.update(crate::Message::CancelAndClose);
        assert!(app.closing);
        let Some(super::super::Page::GogUpdate(page)) = &app.flow else {
            panic!("the page stays until its work ends");
        };
        assert!(page.token.as_ref().unwrap().is_canceled());
    }

    #[test]
    fn canceling_while_gog_answers_drops_the_plan() {
        let (root, paths) = setup();
        native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.0", false);
        let keyring = keyring();
        save_login(&paths, &keyring);
        let server = crate::gog_fake::product_server_slow(
            vec![native_product(RIMWORLD, "RimWorld", "1.1")],
            std::time::Duration::from_millis(300),
        );
        let token = Token::new();
        let result = std::thread::scope(|scope| {
            let (paths, server, token) = (&paths, &server, &token);
            let worker =
                scope.spawn(move || plan_with(paths, &keyring, server.client(), "rimworld", token));
            let deadline = Instant::now() + Duration::from_secs(10);
            while !server
                .requests()
                .iter()
                .any(|line| line.contains("/products/"))
            {
                assert!(Instant::now() < deadline, "GOG was never asked");
                std::thread::sleep(Duration::from_millis(5));
            }
            token.cancel();
            worker.join().unwrap()
        });
        assert_eq!(result.unwrap_err(), "Update canceled.");
    }

    #[test]
    fn planning_is_work_for_back_cancel_and_close() {
        let (_root, paths) = setup();
        let (mut page, _) = Page::new(paths.clone(), "rimworld".to_owned());
        assert!(page.busy());
        let _ = page.update(Event::Back);
        assert!(page.take_action().is_none(), "Back waits for planning");
        let _ = page.update(Event::Cancel);
        assert!(page.token.as_ref().unwrap().is_canceled());
        assert_eq!(page.status, "Canceling...");
        let generation = page.generation;
        let _ = page.update(Event::Planned(generation, Err(CANCELED.to_owned())));
        assert!(!page.busy());
        assert_eq!(page.error.as_deref(), Some("Update canceled."));
        let _ = page.update(Event::Back);
        assert!(matches!(page.take_action(), Some(Action::Library { .. })));

        let (page, _) = Page::new(paths.clone(), "rimworld".to_owned());
        let mut app = crate::App::boot(paths).0;
        app.flow = Some(super::super::Page::GogUpdate(Box::new(page)));
        let _ = app.update(crate::Message::Window(iced::window::Event::CloseRequested));
        assert!(app.close_prompt, "closing while planning asks first");
        let _ = app.update(crate::Message::CancelAndClose);
        let Some(super::super::Page::GogUpdate(page)) = &app.flow else {
            panic!("the page stays until planning ends");
        };
        assert!(page.token.as_ref().unwrap().is_canceled());
    }

    /// Cancels once bytes have arrived and while more are still coming,
    /// from a fake GOG that sends the installer slowly.
    #[test]
    fn canceling_during_a_download_changes_nothing() {
        let (root, paths) = setup();
        let folder = native_game(&paths, root.path(), "rimworld", RIMWORLD, "1.0", false);
        let keyring = keyring();
        save_login(&paths, &keyring);
        let server = crate::gog_fake::product_server_streaming(vec![native_product(
            RIMWORLD, "RimWorld", "1.1",
        )]);
        let Planned::Update(pending) =
            plan_with(&paths, &keyring, server.client(), "rimworld", &Token::new()).unwrap()
        else {
            panic!("expected an update");
        };
        let before = wineo_core::gog::folder::peek(&paths, &folder).unwrap();
        let token = Token::new();
        let total = format_bytes(crate::gog_fake::FILE_SIZE);
        let started = format!("Downloading RimWorld: {} / {total}", format_bytes(0));
        let finished = format!("Downloading RimWorld: {total} / {total}");
        let mut said = Vec::new();
        let failure = apply_with(
            &paths,
            &keyring,
            server.client(),
            &pending,
            &token,
            |line| {
                // Some bytes are in, and the rest aren't yet.
                if line.starts_with("Downloading") && line != started && line != finished {
                    token.cancel();
                }
                said.push(line);
            },
        )
        .unwrap_err();
        assert_eq!(failure, Failure::Canceled { journal: false }, "{said:?}");
        assert_eq!(failure.message("rimworld"), "Update canceled.");
        assert_eq!(said[0], started);
        assert!(!said.contains(&finished), "{said:?}");
        assert!(
            !said.iter().any(|line| line.starts_with("Verifying")),
            "{said:?}"
        );
        assert_eq!(
            wineo_core::gog::folder::peek(&paths, &folder).unwrap(),
            before
        );
    }

    #[test]
    fn progress_lines_read_like_the_cli_and_are_paced() {
        let (_root, _paths, pending) = planned();
        let mut reporter = Reporter::new(&pending);
        let download = |done| Progress::Download {
            product: RIMWORLD,
            file_id: "f".to_owned(),
            done,
            total: 7340032,
        };
        assert_eq!(
            reporter.line(&download(0)),
            Some(format!(
                "Downloading RimWorld: {} / {}",
                format_bytes(0),
                format_bytes(7340032)
            ))
        );
        // Too soon after the last one.
        assert_eq!(reporter.line(&download(1024)), None);
        assert!(reporter.line(&download(7340032)).is_some());
        assert_eq!(
            reporter
                .line(&Progress::Verify { product: RIMWORLD })
                .as_deref(),
            Some("Verifying RimWorld")
        );
        assert_eq!(
            reporter
                .line(&Progress::Stage {
                    product: RIMWORLD,
                    done: 0,
                    total: 9
                })
                .as_deref(),
            Some("Unpacking RimWorld")
        );
        assert_eq!(
            reporter
                .line(&Progress::Publish { done: 0, total: 12 })
                .as_deref(),
            Some("Publishing 12 files")
        );
        assert_eq!(
            reporter
                .line(&Progress::Installing {
                    product: RIMWORLD,
                    interactive: false
                })
                .as_deref(),
            Some("Running RimWorld's installer")
        );
    }

    #[test]
    fn a_failed_plan_shows_its_error() {
        let (_root, paths) = setup();
        let (mut page, _) = Page::new(paths, "rimworld".to_owned());
        let generation = page.generation;
        let _ = page.update(Event::Planned(generation, Err(LOGGED_OUT.to_owned())));
        assert_eq!(page.error.as_deref(), Some(LOGGED_OUT));
        assert_eq!(page.step, Step::Planning);
        let _ = page.update(Event::Back);
        assert!(matches!(page.take_action(), Some(Action::Library { .. })));
    }

    #[test]
    fn a_windows_update_shows_each_installer_and_offers_the_setup_window_once() {
        use super::super::gog_install::tests::{World, disco, inno};
        use wineo_core::gog::update::windows::set_graphical_session;

        let world = World::new();
        set_graphical_session(Some(true));
        world.installed(&disco(), &inno()).unwrap();
        world.disco("2", &world.fails_once(3), true);
        let name = "disco-elysium";
        // GOG's details are cached for an hour, so the newer ones are fetched.
        let refresh = Options {
            refresh: true,
            ..Options::default()
        };
        let pending = match plan_with_options(
            &world.paths,
            &world.keyring,
            world.server.client(),
            name,
            refresh,
            &Token::new(),
        )
        .unwrap()
        {
            Planned::Update(pending) => pending,
            Planned::UpToDate => panic!("expected an update"),
        };
        let first = apply_with(
            &world.paths,
            &world.keyring,
            world.server.client(),
            &pending,
            &Token::new(),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(
            first,
            Failure::NotStarted {
                label: "Disco Elysium".to_owned(),
                message:
                    "Disco Elysium's installer never started (exit code 3), so nothing changed."
                        .to_owned(),
            }
        );
        let (mut page, _) = Page::new(world.paths.clone(), name.to_owned());
        let generation = page.generation;
        let _ = page.update(Event::Planned(generation, Ok(Planned::Update(pending))));
        let _ = page.update(Event::Update);
        let generation = page.generation;
        let _ = page.update(Event::Finished(generation, Err(first)));
        assert_eq!(page.step, Step::Question);
        for (width, height) in [(280.0, 240.0), (900.0, 720.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view(Palette::DARK);
            let probe = crate::layout_probe::probe(&mut view, iced::Size::new(width, height));
            assert!(probe.outside(inset).is_empty(), "{width}x{height}");
        }
        let _ = page.update(Event::SetupWindow(true));
        assert_eq!(page.step, Step::Planning);
        assert!(page.wizard);
        let again = plan_with_options(
            &world.paths,
            &world.keyring,
            world.server.client(),
            name,
            Options {
                wizard: true,
                ..Options::default()
            },
            &Token::new(),
        )
        .unwrap();
        let generation = page.generation;
        let _ = page.update(Event::Planned(generation, Ok(again)));
        assert_eq!(page.step, Step::Working, "the setup window runs right away");
        let pending = page.pending.clone().unwrap();
        assert!(pending.wizard());
        let mut lines = Vec::new();
        let updated = apply_with(
            &world.paths,
            &world.keyring,
            world.server.client(),
            &pending,
            &Token::new(),
            |line| lines.push(line),
        )
        .unwrap();
        assert!(
            lines.contains(&"Opening Disco Elysium's setup window".to_owned()),
            "{lines:?}"
        );
        let generation = page.generation;
        let _ = page.update(Event::Finished(generation, Ok(Box::new(updated))));
        assert_eq!(page.step, Step::Finished);
        assert_eq!(page.take_updated().as_deref(), Some(name));

        // A plain update runs the installer silently and says so.
        world.disco("3", "", true);
        let Planned::Update(pending) = plan_with_options(
            &world.paths,
            &world.keyring,
            world.server.client(),
            name,
            refresh,
            &Token::new(),
        )
        .unwrap() else {
            panic!("expected an update");
        };
        let mut lines = Vec::new();
        apply_with(
            &world.paths,
            &world.keyring,
            world.server.client(),
            &pending,
            &Token::new(),
            |line| lines.push(line),
        )
        .unwrap();
        assert!(
            lines.contains(&"Running Disco Elysium's installer".to_owned()),
            "{lines:?}"
        );
        set_graphical_session(None);
    }

    #[test]
    fn an_interrupted_install_shows_finish_install_and_finishes_here() {
        use super::super::gog_install::tests::{World, disco, inno};
        use crate::gog_library::{Badge, Folders, badge};
        use wineo_core::environment::Environment;
        use wineo_core::gog::update::windows::set_graphical_session;

        let world = World::new();
        set_graphical_session(Some(true));
        world.disco("1", &world.fails_once(4), true);
        let failure = world.installed(&disco(), &inno()).unwrap_err();
        assert!(format!("{failure:?}").contains(
            "Disco Elysium's installer stopped partway (exit code 4). Use Finish install on disco-elysium in the library."
        ), "{failure:?}");
        let name = "disco-elysium";
        let env = Environment::load(&world.paths, name).unwrap();
        let card = Folders::read(&world.paths)
            .card(&world.paths, &env)
            .unwrap();
        assert!(card.unfinished && card.install);
        assert_eq!(card.finish_label(), "Finish install");
        assert_eq!(badge(Some(&card), None), Some(Badge::FinishInstall));
        assert_eq!(Badge::FinishInstall.label(), "Finish install");

        let Planned::Update(pending) = plan_with(
            &world.paths,
            &world.keyring,
            world.server.client(),
            name,
            &Token::new(),
        )
        .unwrap() else {
            panic!("expected the install to finish");
        };
        assert_eq!(action(&pending), "Finish the interrupted install of");
        let (mut page, _) = Page::new(world.paths.clone(), name.to_owned());
        let generation = page.generation;
        let _ = page.update(Event::Planned(
            generation,
            Ok(Planned::Update(pending.clone())),
        ));
        let _ = page.update(Event::Update);
        let generation = page.generation;
        let _ = page.update(Event::Finished(
            generation,
            Err(Failure::Canceled { journal: true }),
        ));
        assert_eq!(
            page.error.as_deref(),
            Some("Install interrupted. Use Finish install on disco-elysium to finish it.")
        );
        let updated = apply_with(
            &world.paths,
            &world.keyring,
            world.server.client(),
            &pending,
            &Token::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            result_lines(&pending, &updated)[0],
            "Finished the interrupted install of disco-elysium."
        );
        let env = Environment::load(&world.paths, name).unwrap();
        let card = Folders::read(&world.paths)
            .card(&world.paths, &env)
            .unwrap();
        assert!(!card.unfinished);
        set_graphical_session(None);
    }
}
