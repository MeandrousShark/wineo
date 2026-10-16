//! Updating a GOG game, or finishing an interrupted update or install, like
//! `wineo update NAME`, on the track the game follows: a review of what
//! changes, the update with progress and cancel, then what changed and which
//! of your files it touched. A Windows installer that never started is
//! offered once as its own setup window.

use std::cell::RefCell;
use std::time::Duration;

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{column, container, row, text, text_input};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::cancel::Token;
use wineo_core::gog::account::{AccountError, Session};
use wineo_core::gog::api::{Client, Secret};
use wineo_core::gog::folder::{JournalKind, Track};
use wineo_core::gog::update::galaxy::present::{
    Label, Reporter as GalaxyReporter, Wording, restart_lines,
};
use wineo_core::gog::update::galaxy::{self as core_galaxy, GalaxyError, PlannedUpdate};
use wineo_core::gog::update::windows::{Event as WindowsEvent, SetupError};
use wineo_core::gog::update::{
    self, Options, Pending, Plan, Reporter, ReviewLabel, UpdateError, Updated,
};
use wineo_core::key_store::Keyring;
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::supervisor::{Notice, Watched};

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
    /// Asking for the password of a private branch the plan needs.
    Password,
    Finished,
}

/// What planning found, cloneable for the page's events.
#[derive(Debug, Clone)]
pub(crate) enum Planned {
    UpToDate,
    /// A pinned game, with what to say instead of updating it.
    Pinned(String),
    Update(Box<Pending>),
    Galaxy(Box<core_galaxy::update::Plan>),
    /// Only the branch or pin changes.
    Follow(Box<core_galaxy::FollowPlan>),
    /// The plan needs a private branch's password, and none is saved.
    NeedsPassword {
        title: String,
        branch: String,
    },
    /// GOG didn't list the branch with the password typed for it, which
    /// may be wrong, so it can be typed again.
    Refused {
        title: String,
        branch: String,
        message: String,
    },
}

/// An update to review and apply, on the track the game is on.
#[derive(Debug, Clone)]
pub(crate) enum Ready {
    Installer(Box<Pending>),
    Galaxy(Box<core_galaxy::update::Plan>),
    /// A change of branch or pin that moves no files.
    Follow(Box<core_galaxy::FollowPlan>),
}

impl Ready {
    fn action(&self) -> &'static str {
        match self {
            Self::Installer(pending) => pending.action(),
            Self::Galaxy(plan) => plan.action(),
            Self::Follow(plan) => plan.action(),
        }
    }

    fn kind(&self) -> JournalKind {
        match self {
            Self::Installer(pending) => pending.kind(),
            Self::Galaxy(plan) => plan.kind(),
            Self::Follow(_) => JournalKind::Update,
        }
    }

    fn is_recovery(&self) -> bool {
        match self {
            Self::Installer(pending) => pending.is_recovery(),
            Self::Galaxy(plan) => plan.is_recovery(),
            Self::Follow(_) => false,
        }
    }

    fn warnings(&self) -> &[String] {
        match self {
            Self::Installer(pending) => pending.warnings(),
            Self::Galaxy(plan) => plan.warnings(),
            Self::Follow(plan) => plan.warnings(),
        }
    }

    /// The review's main button.
    fn button(&self) -> &'static str {
        if matches!(self, Self::Follow(_)) {
            return "Switch";
        }
        match (self.kind(), self.is_recovery()) {
            (_, true) => "Finish",
            (JournalKind::Repair, false) => "Repair",
            (JournalKind::Reinstall, false) => "Reinstall",
            _ => "Update",
        }
    }

    /// Whether this runs an installer as its own setup window, which only
    /// an installer-track update does.
    fn wizard(&self) -> bool {
        matches!(self, Self::Installer(pending) if pending.wizard())
    }
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
    /// The private branch's password, as typed.
    Password(Secret),
    /// Plans again with the password typed.
    Continue,
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
    ready: Option<Ready>,
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
    /// What planning asks for: a branch or build, a repair, or a move.
    options: Options,
    /// The private branch the password question is about, and its title.
    asked: Option<(String, String)>,
    password: Secret,
    action: Option<Action>,
}

impl Page {
    pub(crate) fn new(paths: Paths, name: String) -> (Self, Task<Event>) {
        Self::with_options(paths, name, Options::default())
    }

    /// A page that plans with `options`, as the Version picker asks.
    pub(crate) fn with_options(
        paths: Paths,
        name: String,
        options: Options,
    ) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            name,
            step: Step::Planning,
            generation: 0,
            ready: None,
            updated: None,
            done: None,
            token: None,
            status: String::new(),
            error: None,
            note: None,
            question: None,
            wizard: false,
            options,
            asked: None,
            password: Secret::new(""),
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
            Step::Password => self.update(Event::Continue),
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
        self.asked = None;
        self.password = Secret::new("");
        self.action = Some(Action::Library {
            name: Some(self.name.clone()),
            launch: false,
        });
    }

    fn plan(&mut self) -> Task<Event> {
        self.plan_with(None)
    }

    /// Plans, with `password` for the private branch the plan uses.
    fn plan_with(&mut self, password: Option<Secret>) -> Task<Event> {
        self.plan_job(password).spawn()
    }

    /// Starts planning, with the work left to run.
    fn plan_job(&mut self, password: Option<Secret>) -> PlanJob {
        self.step = Step::Planning;
        self.status = format!("Checking {} with GOG...", self.name);
        let generation = self.next_generation();
        let (paths, name) = (self.paths.clone(), self.name.clone());
        // Planning is work too: Back waits for it, Cancel stops it, and
        // closing the window asks first.
        let token = Token::new();
        self.token = Some(token.clone());
        let options = self.planning_options(password);
        PlanJob {
            generation,
            paths,
            name,
            options,
            token,
        }
    }

    /// Continue: plans again with the password typed, which the page then
    /// forgets. None while there's nothing to send.
    fn continue_job(&mut self) -> Option<PlanJob> {
        if self.step != Step::Password || self.password.expose().is_empty() {
            return None;
        }
        let password = std::mem::replace(&mut self.password, Secret::new(""));
        self.error = None;
        Some(self.plan_job(Some(password)))
    }

    /// What the page was opened to do.
    #[cfg(test)]
    pub(crate) fn options(&self) -> &Options {
        &self.options
    }

    /// What the next plan asks core for.
    fn planning_options(&self, password: Option<Secret>) -> Options {
        Options {
            wizard: self.wizard,
            password,
            ..self.options.clone()
        }
    }

    fn start(&mut self) -> Task<Event> {
        let Some(ready) = self.ready.clone() else {
            return Task::none();
        };
        if self.busy() || self.waiting() {
            return Task::none();
        }
        let token = Token::new();
        self.token = Some(token.clone());
        self.step = Step::Working;
        self.error = None;
        self.status = format!("{} {}...", ready.action(), self.name);
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = Task::perform(
            task::join(
                move || {
                    let client = wineo_core::gog::client_from_env()
                        .map_err(|error| Failure::Failed(error.to_string()))?;
                    apply_ready(&paths, &keyring(), client, &ready, &token, |line| {
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
                self.asked = None;
                match result {
                    Ok(Planned::UpToDate) => {
                        self.note = Some("Up to date.".to_owned());
                        self.done = Some(self.name.clone());
                        self.step = Step::Finished;
                    }
                    Ok(Planned::Pinned(message)) => {
                        self.note = Some(message);
                        self.done = Some(self.name.clone());
                        self.step = Step::Finished;
                    }
                    Ok(Planned::Update(pending)) => {
                        return self.reviewed(Ready::Installer(pending));
                    }
                    Ok(Planned::Galaxy(plan)) => return self.reviewed(Ready::Galaxy(plan)),
                    Ok(Planned::Follow(plan)) => return self.reviewed(Ready::Follow(plan)),
                    Ok(Planned::NeedsPassword { title, branch }) => {
                        self.asked = Some((branch, title));
                        self.password = Secret::new("");
                        self.error = None;
                        self.step = Step::Password;
                    }
                    Ok(Planned::Refused {
                        title,
                        branch,
                        message,
                    }) => {
                        self.asked = Some((branch, title));
                        self.password = Secret::new("");
                        self.error = Some(message);
                        self.step = Step::Password;
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
            Event::Password(password) if self.step == Step::Password => {
                self.password = password;
                Task::none()
            }
            Event::Continue => self.continue_job().map_or_else(Task::none, PlanJob::spawn),
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
                        if self.ready.as_ref().is_some_and(|ready| !ready.wizard()) =>
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

    /// Shows `ready` for review, or runs it at once when the setup window
    /// was asked for.
    fn reviewed(&mut self, ready: Ready) -> Task<Event> {
        self.ready = Some(ready);
        self.step = Step::Review;
        if std::mem::take(&mut self.wizard) {
            return self.start();
        }
        Task::none()
    }

    /// Whether this finishes an interrupted install rather than updating.
    fn installs(&self) -> bool {
        self.ready
            .as_ref()
            .is_some_and(|ready| ready.kind() == JournalKind::Install)
    }

    pub(crate) fn view(&self, palette: Palette) -> Element<'_, Event> {
        let title = match (self.step, self.ready.as_ref()) {
            (Step::Planning, _) => format!("Update {}", self.name),
            (_, Some(ready)) => format!("{} {}", ready.action(), self.name),
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
        if let (Step::Password, Some((branch, title))) = (self.step, &self.asked) {
            body = body.push(text(format!("Branch {branch} of {title} is private.")));
            body = body.push(
                text_input("Password", self.password.expose())
                    .secure(true)
                    .on_input(|value| Event::Password(Secret::new(value)))
                    .on_submit(Event::Continue)
                    .padding([6, 10])
                    .width(Fill)
                    .style(move |_, status| crate::chips::input_style(palette, status)),
            );
        }
        match (self.step, &self.ready, &self.updated) {
            (Step::Review | Step::Working, Some(ready), _) => {
                let mut summary = column![].spacing(8);
                for (label, value) in ready_rows(&self.paths, ready) {
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
                if let Ready::Galaxy(plan) = ready {
                    for note in plan.note_lines() {
                        body = body.push(text(note).size(13).color(palette.muted));
                    }
                }
                for warning in ready.warnings() {
                    body = body.push(text(warning).size(13).color(palette.warning));
                }
            }
            (Step::Finished, Some(ready), Some(updated)) => {
                for line in ready_lines(&self.paths, ready, updated) {
                    body = body.push(text(line));
                }
                for (heading, items) in ready_lists(ready, updated) {
                    let mut list = column![crate::ui::section_heading(heading, palette)].spacing(4);
                    for item in items {
                        list = list.push(text(item).size(13));
                    }
                    body = body.push(list);
                }
                for line in restart_lines(&updated.needs_restart) {
                    body = body.push(text(line).size(13));
                }
                // The plan's warnings came first, and the review showed them.
                for warning in updated.warnings.iter().skip(ready.warnings().len()) {
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
            Step::Review => self.ready.as_ref().map(|ready| {
                accent_base(ready.button(), palette.primary)
                    .on_press(Event::Update)
                    .into()
            }),
            Step::Password => Some(
                accent_base("Continue", palette.primary)
                    .on_press_maybe((!self.password.expose().is_empty()).then_some(Event::Continue))
                    .into(),
            ),
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

/// Planning the page started, to run off the UI thread. The options hold a
/// typed password only until it runs.
pub(crate) struct PlanJob {
    pub(crate) generation: u64,
    paths: Paths,
    name: String,
    options: Options,
    token: Token,
}

impl PlanJob {
    pub(crate) fn run(self, keyring: &dyn Keyring, client: Client) -> Result<Planned, String> {
        plan_with_options(
            &self.paths,
            keyring,
            client,
            &self.name,
            self.options,
            &self.token,
        )
    }

    fn spawn(self) -> Task<Event> {
        let generation = self.generation;
        task::blocking(
            move || {
                let client =
                    wineo_core::gog::client_from_env().map_err(|error| error.to_string())?;
                self.run(&keyring(), client)
            },
            move |result| Event::Planned(generation, result),
        )
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

/// `plan_with`, with `options`: a branch or build, a repair, a move, a
/// password, or a Windows installer run as its own setup window. A private
/// branch with no password saved asks for one rather than failing.
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
    let typed = options.password.is_some();
    let planned = core_galaxy::plan_update(paths, session.as_mut(), name, options, token);
    if token.is_canceled() {
        return Err(canceled_message(paths, name));
    }
    if let Err(error) = &planned
        && let Some(GalaxyError::NeedsPassword { title, branch, .. }) =
            error.downcast_ref::<GalaxyError>()
    {
        return Ok(Planned::NeedsPassword {
            title: title.clone(),
            branch: branch.clone(),
        });
    }
    if typed
        && let Err(error) = &planned
        && let Some(refused @ GalaxyError::Refused { title, branch }) =
            error.downcast_ref::<GalaxyError>()
    {
        return Ok(Planned::Refused {
            title: title.clone(),
            branch: branch.clone(),
            message: refused.to_string(),
        });
    }
    match planned {
        Ok(
            PlannedUpdate::Installer(Plan::UpToDate { .. })
            | PlannedUpdate::Galaxy(core_galaxy::update::Planned::UpToDate { .. }),
        ) => Ok(Planned::UpToDate),
        Ok(PlannedUpdate::Installer(Plan::Update(pending))) => {
            Ok(Planned::Update(Box::new(pending)))
        }
        Ok(PlannedUpdate::Galaxy(core_galaxy::update::Planned::Update(plan))) => {
            Ok(Planned::Galaxy(Box::new(plan)))
        }
        Ok(PlannedUpdate::Galaxy(core_galaxy::update::Planned::Pinned(pinned))) => {
            Ok(Planned::Pinned(pinned.message()))
        }
        Ok(PlannedUpdate::Galaxy(core_galaxy::update::Planned::Follow(plan))) => {
            Ok(Planned::Follow(Box::new(plan)))
        }

        Err(error)
            if logged_out
                && matches!(
                    error.downcast_ref::<AccountError>(),
                    Some(AccountError::LoggedOut)
                ) =>
        {
            Err(LOGGED_OUT.to_owned())
        }
        Err(error) => Err(planning_failed(paths, name, &error)),
    }
}

/// What a cancel during planning says: that an update is left to finish when
/// the folder has a journal, else only that it was canceled.
fn canceled_message(paths: &Paths, name: &str) -> String {
    match update::journal_of(paths, name) {
        Some(journal) => Failure::Canceled { journal: true }
            .message_for(name, journal.kind == JournalKind::Install),
        None => CANCELED.to_owned(),
    }
}

/// A planning failure, ending with how to finish the game's unfinished
/// operation when the folder has a journal. A Galaxy install's journal can't
/// be finished here, so it says to use the library instead of core's
/// refusal.
fn planning_failed(paths: &Paths, name: &str, error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    let Some(journal) = update::journal_of(paths, name) else {
        return message;
    };
    if journal.kind == JournalKind::Install && journal.target.track == Track::Galaxy {
        return format!(
            "{name}'s install didn't finish. Use Finish install on {name} in the library."
        );
    }
    with_hint(&message, &finish_hint(journal.kind, name))
}

/// `message` ending with `hint`, once.
fn with_hint(message: &str, hint: &str) -> String {
    if message.contains(hint) {
        return message.to_owned();
    }
    let sep = if message.ends_with('.') { "" } else { "." };
    format!("{message}{sep} {hint}")
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
    .map_err(|failure| hinted(paths, pending.environment(), failure))
}

/// Applies `ready` on its track.
pub(crate) fn apply_ready(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    ready: &Ready,
    token: &Token,
    say: impl FnMut(String),
) -> Result<Updated, Failure> {
    match ready {
        Ready::Installer(pending) => apply_with(paths, keyring, client, pending, token, say),
        Ready::Galaxy(plan) => apply_galaxy_with(paths, keyring, client, plan, token, say),
        Ready::Follow(plan) => apply_follow(paths, plan, token),
    }
}

/// Records a change of branch or pin, which needs neither GOG nor a login.
pub(crate) fn apply_follow(
    paths: &Paths,
    plan: &core_galaxy::FollowPlan,
    token: &Token,
) -> Result<Updated, Failure> {
    core_galaxy::switch::apply(paths, plan, token).map_err(|error| {
        if let Some(UpdateError::Canceled { journal }) = error.downcast_ref::<UpdateError>() {
            return Failure::Canceled { journal: *journal };
        }
        Failure::Failed(format!("{error:#}"))
    })
}

/// Applies a Galaxy `plan`, sending a status line for each stage and about
/// ten a second while bytes move.
pub(crate) fn apply_galaxy_with(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    plan: &core_galaxy::update::Plan,
    token: &Token,
    say: impl FnMut(String),
) -> Result<Updated, Failure> {
    let mut session = Session::open(paths, keyring, client)
        .map_err(|error| Failure::Failed(error.to_string()))?;
    // A Galaxy update fetches from GOG whatever it finishes.
    let Some(session) = session.as_mut() else {
        return Err(Failure::Failed(LOGGED_OUT.to_owned()));
    };
    let say = RefCell::new(say);
    let mut reporter = GalaxyReporter::new(PROGRESS_INTERVAL);
    core_galaxy::update::apply_with_events(
        paths,
        session,
        plan,
        token,
        |step| {
            if let Some(line) = reporter.line(&step) {
                (say.borrow_mut())(line);
            }
        },
        |event| {
            let line = match event {
                WindowsEvent::Watched(Watched::Notice(Notice::Stopping)) => {
                    Some("Canceling setup...".to_owned())
                }
                WindowsEvent::Watched(Watched::Notice(Notice::Killing)) => {
                    Some("Force stopping setup...".to_owned())
                }
                WindowsEvent::Watched(Watched::Notice(Notice::Warning(error))) => {
                    Some(format!("Setup warning: {error}"))
                }
                _ => None,
            };
            if let Some(line) = line {
                (say.borrow_mut())(line);
            }
        },
    )
    .map_err(|error| {
        if let Some(UpdateError::Canceled { journal }) = error.downcast_ref::<UpdateError>() {
            return Failure::Canceled { journal: *journal };
        }
        if let Some(GalaxyError::Setup {
            title,
            label,
            detail,
        }) = error.downcast_ref::<GalaxyError>()
        {
            return Failure::Failed(format!(
                "Setting up {title} stopped: {label} failed ({detail})."
            ));
        }
        Failure::Failed(format!("{error:#}"))
    })
    .map_err(|failure| hinted(paths, plan.environment(), failure))
}

/// How to finish what `kind` left, in the app's words.
fn finish_hint(kind: JournalKind, name: &str) -> String {
    if kind == JournalKind::Install {
        format!("Use Finish install on {name} to finish it.")
    } else {
        format!("Use Finish update on {name} to finish it.")
    }
}

/// Ends a failure's message with how to finish the game's unfinished
/// operation, decided by the journal still being in its folder and not by
/// what failed. A cancel says its own, and a message that has the hint
/// already is left alone.
fn hinted(paths: &Paths, name: &str, failure: Failure) -> Failure {
    let Failure::Failed(message) = failure else {
        return failure;
    };
    let Some(journal) = update::journal_of(paths, name) else {
        return Failure::Failed(message);
    };
    Failure::Failed(with_hint(&message, &finish_hint(journal.kind, name)))
}

/// How a Windows installer that stopped on its own left things, like
/// `wineo update` says it. An installer that never started on a first
/// attempt changed nothing, since the update drops its journal then;
/// finishing one keeps it.
fn setup_failure(error: &anyhow::Error, pending: &Pending) -> Option<Failure> {
    let name = pending.environment();
    let recovery = pending.is_recovery();
    let finish = finish_hint(pending.kind(), name);
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

/// The review's rows on either track.
fn ready_rows(paths: &Paths, ready: &Ready) -> Vec<(String, String)> {
    match ready {
        Ready::Installer(pending) => review_rows(pending),
        Ready::Galaxy(plan) => galaxy_review_rows(paths, plan),
        Ready::Follow(plan) => plan
            .review_rows(|path| path.display().to_string(), Wording::App)
            .into_iter()
            .map(|(label, value)| (galaxy_label(label), value))
            .collect(),
    }
}

/// A Galaxy update's review rows, the ones `wineo update NAME` prints.
pub(crate) fn galaxy_review_rows(
    paths: &Paths,
    plan: &core_galaxy::update::Plan,
) -> Vec<(String, String)> {
    plan.review_rows(paths, |path| path.display().to_string(), Wording::App)
        .into_iter()
        .map(|(label, value)| (galaxy_label(label), value))
        .collect()
}

/// A Galaxy row's label as the app words it.
pub(crate) fn galaxy_label(label: Label) -> String {
    match label {
        Label::Folder => "Folder".to_owned(),
        Label::Track => "Track".to_owned(),
        Label::Branch => "Branch".to_owned(),
        Label::Pin => "Pin".to_owned(),
        Label::Owner => "Owner".to_owned(),
        Label::Product(label) => label,
        Label::Version => "Version".to_owned(),
        Label::Dlc => "DLC".to_owned(),
        Label::Support => "Support".to_owned(),
        Label::Check => "Check".to_owned(),
        Label::Setup => "Setup".to_owned(),
        Label::Download => "Download".to_owned(),
    }
}

/// What a finished update says on either track.
fn ready_lines(paths: &Paths, ready: &Ready, updated: &Updated) -> Vec<String> {
    match ready {
        Ready::Installer(pending) => result_lines(pending, updated),
        Ready::Galaxy(plan) => galaxy_result_lines(paths, plan, updated),
        Ready::Follow(plan) => {
            let mut lines = vec![format!("{}.", plan.finished())];
            if updated.claimed {
                lines.push(format!(
                    "{} now owns {}.",
                    updated.environment,
                    updated.folder.display()
                ));
            }
            lines
        }
    }
}

/// What a finished Galaxy update says, before the lists of your files.
pub(crate) fn galaxy_result_lines(
    paths: &Paths,
    plan: &core_galaxy::update::Plan,
    updated: &Updated,
) -> Vec<String> {
    let mut lines = vec![format!("{}.", plan.finished())];
    lines.extend(
        plan.changes(paths, updated)
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

/// The files of yours a finished update touched, on either track.
fn ready_lists(ready: &Ready, updated: &Updated) -> Vec<(&'static str, Vec<String>)> {
    match ready {
        Ready::Installer(_) | Ready::Follow(_) => result_lists(updated),
        Ready::Galaxy(plan) => lists_under(updated, plan.saved_heading()),
    }
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
    lists_under(
        updated,
        "Moved your files aside where the new version has files:",
    )
}

fn lists_under(updated: &Updated, saved_heading: &'static str) -> Vec<(&'static str, Vec<String>)> {
    let mut lists = Vec::new();
    if !updated.saved.is_empty() {
        lists.push((
            saved_heading,
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
