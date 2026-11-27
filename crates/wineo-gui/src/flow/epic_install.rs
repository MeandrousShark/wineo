//! Installing an Epic game wineo doesn't have yet, or finishing one whose
//! install stopped, like `wineo import epic --game`: a review from what
//! Legendary says about the game, offering the folder, runner, and arch,
//! then download progress for the game and each DLC, then Epic's art and
//! the desktop entries. With no Legendary to run, it first asks to download
//! one, in the terminal's words.

use std::path::PathBuf;
use std::time::Duration;

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{column, container, pick_list, row, text, text_input};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::cancel::Token;
use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::create::CreateEvent;
use wineo_core::desktop;
use wineo_core::epic::install::{
    self, Event as InstallEvent, Existing, InstallError, Plan, Request,
};
use wineo_core::epic::legendary::{Legendary, Missing};
use wineo_core::epic::owned;
use wineo_core::gog::install_dir::install_folder;
use wineo_core::paths::Paths;

use super::heroic;
use crate::epic_legendary::{self as legendary, Found, Jobs, Offer, Outcome};
use crate::runner_label::Choice;
use crate::size::format_bytes;
use crate::task;
use crate::theme::Palette;
use crate::ui::{PickListStyle, accent_base, secondary_button};

const CANCELED: &str = "Install canceled.";

const LABEL_WIDTH: f32 = 120.0;

/// A game from the owned list, as the import grid knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Game {
    pub(crate) app: String,
    pub(crate) title: String,
    /// Its install stopped, and installing it again finishes it.
    pub(crate) unfinished: bool,
}

/// What the review lets you change. Empty text keeps the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Choices {
    pub(crate) runner: Option<String>,
    pub(crate) arch: Option<Arch>,
    pub(crate) dir: String,
}

/// A planned install and what its review offers.
#[derive(Debug, Clone)]
pub(crate) struct Review {
    /// The plan, or why the choices can't be installed.
    plan: Result<Box<Plan>, String>,
    runners: Vec<Choice>,
    /// `<games_root>/<title>`, which an empty folder uses.
    default_folder: PathBuf,
    /// What it was planned with.
    choices: Choices,
    /// The plan failed in a way no folder, runner, or arch fixes, so the
    /// review shows only why.
    fatal: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum Prepared {
    Review(Box<Review>),
    /// There is no Legendary to run, and one can be downloaded.
    Offer(Missing),
}

/// What a finished install says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Done {
    pub(crate) environment: String,
    pub(crate) lines: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

/// What installing does after the game is in place: Epic's art, then the
/// desktop entries, as the terminal does. Lines and warnings.
pub(crate) type Extras = fn(&Paths, &Plan, &str, &Token) -> (Vec<String>, Vec<String>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Preparing,
    /// Asking to download Legendary, then downloading it.
    Asking,
    Review,
    Working,
    Finished,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Prepared(u64, Result<Prepared, String>),
    Offer(legendary::Event),
    Runner(String),
    Arch(Arch),
    Folder(String),
    FolderDone,
    Browse,
    Picked(u64, String, Result<Option<PathBuf>, String>),
    Install,
    Cancel,
    Force,
    Progress(u64, String),
    /// No download progress for this many minutes.
    Stalled(u64, u64),
    Finished(u64, Result<Box<Done>, String>),
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
    game: Game,
    stage: Stage,
    generation: u64,
    choices: Choices,
    review: Option<Review>,
    offer: Option<Offer>,
    done: Option<Done>,
    token: Option<Token>,
    status: String,
    stalled: Option<u64>,
    error: Option<String>,
    leave: Option<Leave>,
    /// The review's call to Legendary.
    job: Jobs,
}

impl Install {
    pub(crate) fn new(paths: Paths, game: Game) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            game,
            stage: Stage::Preparing,
            generation: 0,
            choices: Choices::default(),
            review: None,
            offer: None,
            done: None,
            token: None,
            status: String::new(),
            stalled: None,
            error: None,
            leave: None,
            job: Jobs::default(),
        };
        let task = page.prepare();
        (page, task)
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Installing, or downloading Legendary for it: work Back waits for.
    pub(crate) fn working(&self) -> bool {
        self.token.is_some() || self.offer.as_ref().is_some_and(Offer::downloading)
    }

    /// Work closing the window waits for, the review's Legendary call too.
    pub(crate) fn busy(&self) -> bool {
        self.working() || self.job.running()
    }

    pub(crate) fn cancel_jobs(&self) {
        self.job.cancel();
    }

    /// Whether its install or Legendary download was told to stop.
    #[cfg(test)]
    pub(crate) fn canceled(&self) -> bool {
        self.token.as_ref().is_some_and(Token::is_canceled)
            || self.offer.as_ref().is_some_and(Offer::canceled)
    }

    pub(crate) fn waiting(&self) -> bool {
        !self.status.is_empty()
    }

    pub(crate) fn finished(&self) -> bool {
        self.stage == Stage::Finished
    }

    pub(crate) fn take_leave(&mut self) -> Option<Leave> {
        self.leave.take()
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        self.job.cancel();
        if let Some(offer) = &mut self.offer {
            offer.cancel();
            return;
        }
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
        self.review.as_ref()?.plan.as_deref().ok()
    }

    /// Whether the review shows what installing would do now: a typed
    /// folder only counts once it's planned.
    fn current(&self) -> bool {
        self.review
            .as_ref()
            .is_some_and(|review| review.choices == self.choices)
    }

    /// Whether the review can still change things, which a resumed install
    /// can't.
    fn editable(&self) -> bool {
        self.stage == Stage::Review
            && !self.plan().is_some_and(Plan::resumes)
            && !self.game.unfinished
            && !self.review.as_ref().is_some_and(|review| review.fatal)
    }

    fn ready(&self) -> bool {
        self.plan().is_some() && !self.waiting() && self.current()
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        match self.stage {
            Stage::Review => self.update(Event::Install),
            Stage::Finished => {
                self.exit();
                Task::none()
            }
            Stage::Preparing | Stage::Asking | Stage::Working => Task::none(),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        if self.working() {
            return Task::none();
        }
        match self.stage {
            Stage::Preparing | Stage::Asking | Stage::Review => {
                // A reply to the plan must not reopen it, and Legendary
                // stops asking.
                self.job.cancel();
                self.next_generation();
                self.status.clear();
                self.offer = None;
                self.leave = Some(Leave::Games);
                Task::none()
            }
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
            .filter(|name| self.paths.environment_config(name).exists());
        self.leave = Some(Leave::Library(name));
    }

    fn prepare(&mut self) -> Task<Event> {
        self.stage = if self.review.is_some() {
            Stage::Review
        } else {
            Stage::Preparing
        };
        self.status = format!("Checking {} with Epic...", self.game.title);
        self.error = None;
        let generation = self.next_generation();
        let token = self.job.start(generation);
        let paths = self.paths.clone();
        let game = self.game.clone();
        let choices = self.choices.clone();
        task::blocking(
            move || prepare_with(&paths, &game, &choices, &token),
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
        let token = Token::new();
        self.token = Some(token.clone());
        self.stage = Stage::Working;
        self.error = None;
        self.stalled = None;
        self.status = if plan.resumes() {
            format!("Finishing the install of {}...", plan.title())
        } else {
            format!("Installing {}...", plan.title())
        };
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = task::blocking_value(
            move || {
                install_with(
                    &paths,
                    &plan,
                    install::STALL,
                    &token,
                    |event| {
                        let _ = sender.unbounded_send(event.into_event(generation));
                    },
                    extras,
                )
                .map(Box::new)
            },
            move |result| Event::Finished(generation, result.and_then(|done| done)),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    /// Plans again after a choice changed, unless the review is busy.
    fn replan(&mut self) -> Task<Event> {
        if self.stage != Stage::Review || self.working() {
            return Task::none();
        }
        self.prepare()
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Prepared(generation, _) if generation != self.generation => {
                self.job.ended(generation);
                Task::none()
            }
            Event::Prepared(generation, result) => {
                self.job.ended(generation);
                self.status.clear();
                match result {
                    Ok(Prepared::Review(review)) => {
                        self.review = Some(*review);
                        self.stage = Stage::Review;
                    }
                    Ok(Prepared::Offer(missing)) => {
                        self.offer = Some(Offer::new(&missing));
                        self.stage = Stage::Asking;
                    }
                    Err(error) => {
                        self.error = Some(error);
                        if self.review.is_none() {
                            self.stage = Stage::Finished;
                        }
                    }
                }
                Task::none()
            }
            Event::Offer(event) if self.stage == Stage::Asking => {
                let Some(offer) = &mut self.offer else {
                    return Task::none();
                };
                let (task, outcome) = offer.update(&self.paths, event);
                match outcome {
                    Outcome::Pending => task.map(Event::Offer),
                    Outcome::Declined => {
                        self.offer = None;
                        self.error = Some(CANCELED.to_owned());
                        self.stage = Stage::Finished;
                        Task::none()
                    }
                    Outcome::Failed(error) => {
                        self.offer = None;
                        self.error = Some(error);
                        self.stage = Stage::Finished;
                        Task::none()
                    }
                    Outcome::Ready => {
                        self.offer = None;
                        self.prepare()
                    }
                }
            }
            Event::Runner(name) if self.editable() => {
                self.choices.runner = Some(name);
                self.replan()
            }
            Event::Arch(arch) if self.editable() => {
                self.choices.arch = Some(arch);
                self.replan()
            }
            Event::Folder(value) if self.editable() => {
                self.choices.dir = value;
                Task::none()
            }
            Event::FolderDone => self.replan(),
            Event::Browse if self.editable() && !self.busy() => {
                let previous = self.choices.dir.clone();
                let generation = self.generation;
                Task::perform(
                    crate::picker::pick("Select install folder".to_owned(), true),
                    move |result| Event::Picked(generation, previous.clone(), result),
                )
            }
            Event::Picked(generation, previous, result)
                if generation == self.generation
                    && self.editable()
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
                    self.stalled = None;
                }
                Task::none()
            }
            Event::Stalled(generation, minutes)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                self.stalled = Some(minutes);
                Task::none()
            }
            Event::Finished(generation, result) if generation == self.generation => {
                self.token = None;
                self.status.clear();
                self.stalled = None;
                match result {
                    Ok(done) => self.done = Some(*done),
                    Err(error) => self.error = Some(error),
                }
                self.stage = Stage::Finished;
                Task::none()
            }
            Event::BackToLibrary if self.stage == Stage::Finished => {
                self.exit();
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn title(&self) -> &'static str {
        match self.stage {
            Stage::Preparing | Stage::Asking | Stage::Review => "Review Epic install",
            Stage::Working => "Installing from Epic",
            Stage::Finished => "Epic install finished",
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
        if let Some(minutes) = self.stalled {
            body = body.push(text(stalled_line(minutes)).size(13).color(palette.warning));
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(palette.danger));
        }
        match self.stage {
            Stage::Preparing => {}
            Stage::Asking => {
                if let Some(offer) = &self.offer {
                    body = body.push(offer.view(palette).map(Event::Offer));
                }
            }
            Stage::Review | Stage::Working => body = self.review_view(body, palette),
            Stage::Finished => {
                if let Some(done) = &self.done {
                    for line in &done.lines {
                        body = body.push(text(line));
                    }
                    for warning in &done.warnings {
                        body = body.push(text(warning).size(13).color(palette.warning));
                    }
                }
            }
        }
        let footer: Option<Element<'a, Event>> = match self.stage {
            Stage::Review => {
                let label = if self.plan().is_some_and(Plan::resumes) {
                    "Finish install"
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
            Stage::Finished => Some(
                secondary_button("Back to library", palette)
                    .on_press(Event::BackToLibrary)
                    .into(),
            ),
            Stage::Preparing | Stage::Asking => None,
        };
        (body, footer)
    }

    fn review_view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Event>,
        palette: Palette,
    ) -> iced::widget::Column<'a, Event> {
        let Some(review) = &self.review else {
            return body;
        };
        let plan = review.plan.as_deref().ok();
        let editable = self.editable();
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
        let title = plan.map_or(self.game.title.as_str(), Plan::title);
        let mut summary = column![crate::ui::section_heading(title, palette)].spacing(8);
        if let Some(plan) = plan {
            let kind = if plan.creation().is_some() {
                " (new Windows environment)"
            } else {
                ""
            };
            summary = summary.push(shown_row(
                "Environment",
                format!("{}{kind}", plan.environment()),
            ));
        }
        if editable {
            let input = text_input(
                &review.default_folder.display().to_string(),
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
        let creation = plan.and_then(Plan::creation);
        if editable && (creation.is_some() || plan.is_none()) {
            let runner = creation
                .map(|creation| creation.runner().selection().to_owned())
                .or_else(|| self.choices.runner.clone());
            summary = summary.push(control_row(
                "Runner",
                pick_list(
                    review.runners.clone(),
                    runner.map(|name| Choice::find(&review.runners, &name)),
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
                    creation
                        .map(|creation| creation.resolved_arch())
                        .or(self.choices.arch),
                    Event::Arch,
                )
                .placeholder("Choose an arch")
                .wineo_style(palette)
                .into(),
            ));
        } else if let Some(creation) = creation {
            summary = summary.push(shown_row("Runner", creation.runner().name().to_owned()));
            summary = summary.push(shown_row("Arch", creation.resolved_arch().to_string()));
        }
        if let Some(plan) = plan {
            for (label, value) in plan_rows(plan) {
                summary = summary.push(shown_row(label, value));
            }
        }
        body = body.push(container(summary).padding(16).width(Fill).style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(12).width(1).color(palette.border))
        }));
        if editable && !self.current() && !self.waiting() {
            body = body.push(
                text("Press Enter to check the new folder.")
                    .size(13)
                    .color(palette.muted),
            );
        } else if let Err(error) = &review.plan {
            body = body.push(text(error).color(palette.danger));
        }
        if let Some(plan) = plan {
            for warning in plan.warnings() {
                body = body.push(text(warning).size(13).color(palette.warning));
            }
        }
        body
    }
}

/// The version, DLC, and size rows, like `import epic --game` prints them.
pub(crate) fn plan_rows(plan: &Plan) -> Vec<(&'static str, String)> {
    let mut rows = vec![("Version", plan.version().unwrap_or("unknown").to_owned())];
    // One DLC per row; the rest have no label, so they sit under the first.
    for (index, dlc) in plan.dlc().iter().enumerate() {
        rows.push((if index == 0 { "DLC" } else { "" }, dlc.title.clone()));
    }
    // Legendary can't say a DLC's size ahead.
    let size = |bytes: Option<u64>| {
        let size = bytes.map_or_else(|| "unknown".to_owned(), format_bytes);
        match plan.dlc().len() {
            0 => size,
            count => format!("{size} plus {count} DLC"),
        }
    };
    rows.push(("Download", size(plan.download_size())));
    rows.push(("Installed", size(plan.install_size())));
    rows
}

/// What a download with no progress says. The terminal's own line names
/// Ctrl+C; here it is Cancel.
pub(crate) fn stalled_line(minutes: u64) -> String {
    format!(
        "No download progress for {minutes} minutes. It keeps trying; Cancel stops it, and installing again resumes."
    )
}

/// The line an install's event shows, like the terminal's.
#[derive(Debug)]
pub(crate) enum Said {
    Line(String),
    Stalled(u64),
}

impl Said {
    fn into_event(self, generation: u64) -> Event {
        match self {
            Self::Line(line) => Event::Progress(generation, line),
            Self::Stalled(minutes) => Event::Stalled(generation, minutes),
        }
    }
}

/// Turns the install's events into lines, a run's start and then its whole
/// percentage as it changes.
pub(crate) struct Lines {
    /// What each line starts with: `Downloading`, or `Repairing`.
    verb: &'static str,
    said: Option<(usize, u64)>,
}

impl Default for Lines {
    fn default() -> Self {
        Self::new("Downloading")
    }
}

impl Lines {
    pub(crate) fn new(verb: &'static str) -> Self {
        Self { verb, said: None }
    }

    pub(crate) fn line(&mut self, event: &InstallEvent) -> Option<Said> {
        let verb = self.verb;
        let label = |index: usize, total: usize| {
            if total == 1 {
                verb.to_owned()
            } else {
                format!("{verb} {index} of {total}")
            }
        };
        match *event {
            InstallEvent::Started {
                index,
                total,
                ref title,
            } => {
                self.said = None;
                Some(Said::Line(format!("{}: {title}", label(index, total))))
            }
            InstallEvent::Progress {
                index,
                total,
                percent,
            } => {
                let whole = percent.floor() as u64;
                if self.said == Some((index, whole)) {
                    return None;
                }
                self.said = Some((index, whole));
                Some(Said::Line(format!("{}: {whole}%", label(index, total))))
            }
            InstallEvent::Stalled { minutes, .. } => Some(Said::Stalled(minutes)),
            InstallEvent::Create(CreateEvent::BuildingPrefix { ref dir, .. }) => {
                Some(Said::Line(format!("Building prefix in {}", dir.display())))
            }
            // A line Legendary printed that says nothing keeps the last one,
            // and the spinner says work goes on.
            _ => None,
        }
    }
}

/// Plans installing `game` with `choices`. A plan that fails is part of
/// what's prepared, so the review can still offer other choices; failing to
/// reach that point is the error.
pub(crate) fn prepare_with(
    paths: &Paths,
    game: &Game,
    choices: &Choices,
    token: &Token,
) -> Result<Prepared, String> {
    let legendary = match legendary::find(paths, token)? {
        Found::Usable(legendary) => legendary,
        Found::Offer(missing) => return Ok(Prepared::Offer(missing)),
        Found::Unusable(why) => return Err(why),
    };
    prepare_found(paths, &legendary, game, choices, token)
        .map(|review| Prepared::Review(Box::new(review)))
}

fn prepare_found(
    paths: &Paths,
    legendary: &Legendary,
    game: &Game,
    choices: &Choices,
    token: &Token,
) -> Result<Review, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let existing = install::existing(paths, &game.app).map_err(|error| format!("{error:#}"))?;
    if let Existing::Installed { name, folder } = &existing {
        return Err(format!(
            "Already installed. {name} owns {}.",
            paths.contract_text(&folder.display().to_string())
        ));
    }
    let owned = owned::load(legendary, false, token).map_err(|error| error.to_string())?;
    let Some(listed) = owned
        .games
        .iter()
        .find(|listed| listed.app_name == game.app)
    else {
        return Err(InstallError::NotOwned {
            app: game.app.clone(),
        }
        .to_string());
    };
    let default_folder = global
        .games_root(paths)
        .ok()
        .and_then(|root| install_folder(&root, &listed.title, None).ok())
        .unwrap_or_default();
    let runners = heroic::runner_choices(paths, &global, true)
        .map(|(_, choices)| choices)
        .unwrap_or_default();
    let dir = choices.dir.trim();
    let mut fatal = false;
    let plan = (|| {
        let dir = (!dir.is_empty())
            .then(|| paths.absolute(dir))
            .transpose()
            .map_err(|error| format!("{error:#}"))?;
        let request = Request {
            app_name: game.app.clone(),
            dir,
            runner: choices.runner.clone(),
            arch: choices.arch,
            // New environments get desktop entries, as in the terminal.
            desktop: true,
        };
        install::plan(legendary, &request, listed, &existing, token)
            .map(Box::new)
            .map_err(|error| {
                fatal = unrecoverable(&error);
                format!("{error:#}")
            })
    })();
    if token.is_canceled() {
        return Err(CANCELED.to_owned());
    }
    Ok(Review {
        plan,
        runners,
        default_folder,
        choices: choices.clone(),
        fatal,
    })
}

/// Whether a plan's failure is one no other folder, runner, or arch can
/// fix: the game can't be installed, Legendary failed, or the game is
/// already on disk.
fn unrecoverable(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<InstallError>(),
        Some(
            InstallError::Unavailable { .. }
                | InstallError::Downloaded { .. }
                | InstallError::NotOwned { .. }
                | InstallError::Portable(_)
                | InstallError::Running { .. }
        )
    ) || error.downcast_ref::<wineo_core::epic::Error>().is_some()
}

/// How to finish an install that stopped: from the library once its
/// environment exists, else by installing it again, as GOG's says.
fn finish_hint(paths: &Paths, name: &str) -> String {
    if !name.is_empty() && paths.environment_config(name).exists() {
        format!("Use Finish install on {name} in the library.")
    } else {
        "Install it again here to finish it.".to_owned()
    }
}

/// Runs the install with a line for each step, then adds `extras`.
pub(crate) fn install_with(
    paths: &Paths,
    plan: &Plan,
    stall: Duration,
    token: &Token,
    mut say: impl FnMut(Said),
    extras: Extras,
) -> Result<Done, String> {
    // A cancel while Legendary is looked for stops before anything starts.
    let found = legendary::find(paths, token).map_err(|error| {
        if token.is_canceled() {
            CANCELED.to_owned()
        } else {
            error
        }
    })?;
    let legendary = match found {
        Found::Usable(legendary) => legendary,
        Found::Offer(_) => {
            return Err("Legendary is gone. Install the game again to get it.".to_owned());
        }
        Found::Unusable(why) => return Err(why),
    };
    let app = plan.app_name();
    let mut lines = Lines::default();
    let mut created = Vec::new();
    let applied = install::apply(&legendary, plan, stall, token, |event| {
        if let InstallEvent::Create(CreateEvent::Warning(warning)) = &event {
            created.push(warning.clone());
        }
        if let Some(said) = lines.line(&event) {
            say(said);
        }
    });
    let installed = match applied {
        Ok(installed) => installed,
        Err(error) if install::is_cancel(&error) || token.is_canceled() => {
            return Err(if install::left_unfinished(paths, app) {
                format!(
                    "Install interrupted. {}",
                    finish_hint(paths, plan.environment())
                )
            } else {
                CANCELED.to_owned()
            });
        }
        // The mark there is the other install's, which is still going.
        Err(error) if matches!(error.downcast_ref(), Some(InstallError::Running { .. })) => {
            return Err(format!("{error:#}"));
        }
        Err(error) if install::left_unfinished(paths, app) => {
            return Err(format!(
                "{error:#} {}",
                finish_hint(paths, plan.environment())
            ));
        }
        Err(error) => return Err(format!("{error:#}")),
    };
    let name = installed.environment.clone();
    let mut done = Done {
        environment: name.clone(),
        lines: vec![if installed.resumed {
            format!("Finished the interrupted install of {name}.")
        } else {
            format!("Installed {} as {name}.", installed.title)
        }],
        warnings: installed.warnings.clone(),
    };
    done.warnings.extend(created);
    if token.is_canceled() {
        done.warnings.push("Skipped Epic art: canceled.".to_owned());
    } else {
        say(Said::Line(format!(
            "Adding Epic art for {}...",
            installed.title
        )));
        let (lines, warnings) = extras(paths, plan, &name, token);
        done.lines.extend(lines);
        done.warnings.extend(warnings);
    }
    Ok(done)
}

/// Epic's art, then the desktop entries, as the terminal adds them. The
/// install already succeeded, so each problem is a warning.
fn extras(paths: &Paths, plan: &Plan, name: &str, token: &Token) -> (Vec<String>, Vec<String>) {
    let mut lines = Vec::new();
    let mut warnings = Vec::new();
    match GlobalConfig::load(paths) {
        Ok(global) => {
            let kinds = global.art.fetch.kinds();
            if !kinds.is_empty() && !plan.images().is_empty() {
                let https = wineo_core::download::Lazy::new();
                match https.https() {
                    Ok(transport) => {
                        let request = wineo_core::epic::art::Request {
                            name,
                            app_name: plan.app_name(),
                            images: plan.images(),
                            kinds: &kinds,
                            transport,
                        };
                        match wineo_core::epic::art::import(paths, &request, token) {
                            Ok(report) => {
                                lines.extend(report.line());
                                warnings.extend(report.notes);
                            }
                            Err(error) => warnings.push(format!("Skipped Epic art: {error:#}")),
                        }
                    }
                    Err(error) => warnings.push(format!("Skipped Epic art: {error:#}")),
                }
            }
        }
        Err(error) => warnings.push(error.to_string()),
    }
    // Last, so a new icon is in the entries.
    if desktop::supported(paths).is_ok()
        && let Err(error) = desktop::apply(paths, name, false)
    {
        warnings.push(format!("Couldn't update desktop entries: {error:#}"));
    }
    (lines, warnings)
}

#[cfg(test)]
pub(super) mod tests;
