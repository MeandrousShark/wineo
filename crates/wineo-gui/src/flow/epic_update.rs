//! Updating an Epic game, or finishing an update or repair that stopped,
//! like `wineo update NAME`: a review of each run Legendary makes, then
//! progress, then what changed. With no Legendary to run, it first asks to
//! download one, in the terminal's words.

use std::time::Duration;

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{column, container, row, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::cancel::Token;
use wineo_core::desktop;
use wineo_core::epic::game::Work;
use wineo_core::epic::legendary::Missing;
use wineo_core::epic::owned;
use wineo_core::epic::update::{self, Plan, Planned, Updated};
use wineo_core::paths::Paths;

use super::Action;
use super::epic_install::{Lines, Said, stalled_line};
use super::frame;
use crate::epic_legendary::{self as legendary, Found, Offer, Outcome};
use crate::task;
use crate::theme::Palette;
use crate::ui::{accent_base, secondary_button};

const CANCELED: &str = "Update canceled.";

const LABEL_WIDTH: f32 = 160.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Planning,
    Asking,
    Review,
    Working,
    Finished,
}

#[derive(Debug, Clone)]
pub(crate) enum Planning {
    Ready(Box<Plan>),
    UpToDate,
    /// There is no Legendary to run, and one can be downloaded.
    Offer(Missing),
}

/// What a finished update says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Done {
    pub(crate) lines: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Planned(u64, Result<Planning, String>),
    Offer(legendary::Event),
    Update,
    Cancel,
    Force,
    Progress(u64, String),
    Stalled(u64, u64),
    Finished(u64, Result<Done, String>),
    Back,
    BackToLibrary,
}

pub(crate) struct Page {
    paths: Paths,
    name: String,
    stage: Stage,
    generation: u64,
    plan: Option<Plan>,
    offer: Option<Offer>,
    token: Option<Token>,
    status: String,
    stalled: Option<u64>,
    error: Option<String>,
    done: Option<Done>,
    /// The environment an update changed, for the library to check again,
    /// once.
    updated: Option<String>,
    action: Option<Action>,
}

impl Page {
    pub(crate) fn new(paths: Paths, name: String) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            name,
            stage: Stage::Planning,
            generation: 0,
            plan: None,
            offer: None,
            token: None,
            status: String::new(),
            stalled: None,
            error: None,
            done: None,
            updated: None,
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
        self.token.is_some() || self.offer.as_ref().is_some_and(Offer::downloading)
    }

    pub(crate) fn waiting(&self) -> bool {
        !self.status.is_empty()
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    pub(crate) fn take_updated(&mut self) -> Option<String> {
        self.updated.take()
    }

    pub(crate) fn cancel(&mut self, force: bool) {
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

    pub(crate) fn submit(&mut self) -> Task<Event> {
        match self.stage {
            Stage::Review => self.update(Event::Update),
            Stage::Finished => self.update(Event::BackToLibrary),
            Stage::Planning | Stage::Asking | Stage::Working => Task::none(),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        if self.busy() || self.stage == Stage::Working {
            return Task::none();
        }
        self.next_generation();
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
        self.stage = Stage::Planning;
        self.status = format!("Checking {} with Epic...", self.name);
        self.error = None;
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let name = self.name.clone();
        task::blocking(
            move || plan_with(&paths, &name, &Token::new()),
            move |result| Event::Planned(generation, result),
        )
    }

    fn start(&mut self) -> Task<Event> {
        let Some(plan) = self.plan.clone() else {
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
        self.status = format!("{} {}...", working(&plan), plan.environment());
        let generation = self.next_generation();
        let paths = self.paths.clone();
        let (sender, receiver) = unbounded();
        let work = task::blocking_value(
            move || {
                update_with(&paths, &plan, update_stall(), &token, |said| {
                    let _ = sender.unbounded_send(match said {
                        Said::Line(line) => Event::Progress(generation, line),
                        Said::Stalled(minutes) => Event::Stalled(generation, minutes),
                    });
                })
            },
            move |result| Event::Finished(generation, result.and_then(|done| done)),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Planned(generation, result) if generation == self.generation => {
                self.status.clear();
                match result {
                    Ok(Planning::Ready(plan)) => {
                        self.plan = Some(*plan);
                        self.stage = Stage::Review;
                    }
                    Ok(Planning::UpToDate) => {
                        self.done = Some(Done {
                            lines: vec!["Up to date.".to_owned()],
                            warnings: Vec::new(),
                        });
                        self.stage = Stage::Finished;
                    }
                    Ok(Planning::Offer(missing)) => {
                        self.offer = Some(Offer::new(&missing));
                        self.stage = Stage::Asking;
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.stage = Stage::Finished;
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
                        self.plan()
                    }
                }
            }
            Event::Update if self.stage == Stage::Review && !self.waiting() => self.start(),
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
                    Ok(done) => self.done = Some(done),
                    Err(error) => self.error = Some(error),
                }
                // Whatever happened, the game's state is worth checking again.
                self.updated = Some(self.name.clone());
                self.stage = Stage::Finished;
                Task::none()
            }
            Event::Back => self.back(),
            Event::BackToLibrary if self.stage == Stage::Finished => {
                self.exit();
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn title(&self) -> &'static str {
        let repair = self
            .plan
            .as_ref()
            .is_some_and(|plan| plan.work() == Work::Repair);
        match (self.stage, repair) {
            (Stage::Planning | Stage::Asking | Stage::Review, false) => "Review Epic update",
            (Stage::Planning | Stage::Asking | Stage::Review, true) => "Review Epic repair",
            (Stage::Working, false) => "Updating from Epic",
            (Stage::Working, true) => "Repairing from Epic",
            (Stage::Finished, false) => "Epic update finished",
            (Stage::Finished, true) => "Epic repair finished",
        }
    }

    pub(crate) fn view(&self, palette: Palette) -> Element<'_, Event> {
        let back = secondary_button("Back", palette).on_press_maybe(
            (!self.busy() && !matches!(self.stage, Stage::Working | Stage::Finished))
                .then_some(Event::Back),
        );
        let mut body = column![crate::ui::page_title(self.title(), palette)]
            .spacing(14)
            .width(Fill);
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
            Stage::Planning => {}
            Stage::Asking => {
                if let Some(offer) = &self.offer {
                    body = body.push(offer.view(palette).map(Event::Offer));
                }
            }
            Stage::Review | Stage::Working => {
                if let Some(plan) = &self.plan {
                    body = body.push(review(&self.paths, plan, palette));
                }
            }
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
        let footer: Option<Element<'_, Event>> = match self.stage {
            Stage::Review => self.plan.as_ref().map(|plan| {
                accent_base(button_label(plan), palette.primary)
                    .on_press_maybe((!self.waiting()).then_some(Event::Update))
                    .into()
            }),
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
            Stage::Planning | Stage::Asking => None,
        };
        let mut page = column![row![back].align_y(Center)]
            .spacing(12)
            .max_width(frame::PAGE_MAX_WIDTH)
            .height(Fill)
            .push(frame::scroll_body(Element::from(body), None, palette));
        if let Some(footer) = footer {
            page = page.push(frame::pinned(footer, palette));
        }
        frame::page_frame(page)
    }
}

/// The page's main button, naming what it does.
pub(crate) fn button_label(plan: &Plan) -> &'static str {
    match (plan.work(), plan.resumes()) {
        (_, true) => "Finish",
        (Work::Repair, false) => "Repair",
        (_, false) => "Update",
    }
}

fn working(plan: &Plan) -> &'static str {
    match (plan.work(), plan.resumes()) {
        (Work::Repair, false) => "Repairing",
        (Work::Repair, true) => "Finishing the interrupted repair of",
        (_, true) => "Finishing the interrupted update of",
        (_, false) => "Updating",
    }
}

fn version(version: Option<&str>) -> &str {
    version.unwrap_or("unknown")
}

/// The review's rows, like `wineo update NAME` prints them.
pub(crate) fn review_rows(paths: &Paths, plan: &Plan) -> Vec<(String, String)> {
    let mut rows = vec![(
        "Folder".to_owned(),
        paths.contract_text(&plan.folder().display().to_string()),
    )];
    if plan.rebuilds() {
        rows.push((
            "Record".to_owned(),
            "missing, rebuilt from Legendary's".to_owned(),
        ));
    }
    let repair = plan.work() == Work::Repair;
    for run in &plan.runs()[plan.first()..] {
        let value = if repair {
            version(run.from.as_deref()).to_owned()
        } else if run.new {
            format!("not installed -> {}", version(run.to.as_deref()))
        } else {
            format!(
                "{} -> {}",
                version(run.from.as_deref()),
                version(run.to.as_deref())
            )
        };
        rows.push((run.title.clone(), value));
    }
    if repair {
        rows.push(("Check".to_owned(), "every file".to_owned()));
        rows.push((
            "Download".to_owned(),
            "only what's missing or damaged".to_owned(),
        ));
    }
    rows
}

fn review<'a>(paths: &Paths, plan: &'a Plan, palette: Palette) -> Element<'a, Event> {
    let mut summary = column![crate::ui::section_heading(plan.title(), palette)].spacing(8);
    for (label, value) in review_rows(paths, plan) {
        summary = summary.push(
            row![
                text(label).color(palette.muted).width(LABEL_WIDTH),
                text(value).width(Fill)
            ]
            .spacing(12),
        );
    }
    container(summary)
        .padding(16)
        .width(Fill)
        .style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(12).width(1).color(palette.border))
        })
        .into()
}

/// How long a download may go without progress before it is said.
fn update_stall() -> Duration {
    wineo_core::epic::install::STALL
}

/// Plans `name`'s update from the owned list. Unfinished work is finished
/// as what it was, as `wineo update NAME` does.
pub(crate) fn plan_with(paths: &Paths, name: &str, token: &Token) -> Result<Planning, String> {
    let legendary = match legendary::find(paths, token)? {
        Found::Usable(legendary) => legendary,
        Found::Offer(missing) => return Ok(Planning::Offer(missing)),
        Found::Unusable(why) => return Err(why),
    };
    let owned = owned::load(&legendary, false, token).map_err(|error| error.to_string())?;
    match update::plan(paths, name, Some(&owned), false).map_err(|error| format!("{error:#}"))? {
        Planned::UpToDate => Ok(Planning::UpToDate),
        Planned::Ready(plan) => Ok(Planning::Ready(plan)),
    }
}

/// Runs the update or repair with a line for each step, and says what
/// changed, or how a stop left it.
pub(crate) fn update_with(
    paths: &Paths,
    plan: &Plan,
    stall: Duration,
    token: &Token,
    mut say: impl FnMut(Said),
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
            return Err("Legendary is gone. Update the game again to get it.".to_owned());
        }
        Found::Unusable(why) => return Err(why),
    };
    let repair = plan.work() == Work::Repair;
    let mut lines = Lines::new(if repair { "Repairing" } else { "Downloading" });
    let applied = update::apply(&legendary, plan, stall, token, |event| {
        if let Some(said) = lines.line(&event) {
            say(said);
        }
    });
    let left = update::left_unfinished(paths, plan.app_name());
    let updated = match applied {
        Ok(updated) => updated,
        Err(error) if update::is_cancel(&error) || token.is_canceled() => {
            return Err(match left {
                Some(work) => interrupted(plan.environment(), work),
                None => CANCELED.to_owned(),
            });
        }
        Err(error) => {
            let message = format!("{error:#}");
            return Err(match left {
                Some(_) => {
                    let sep = if message.ends_with('.') { "" } else { "." };
                    format!("{message}{sep} {}", finish_hint(plan.environment()))
                }
                None => message,
            });
        }
    };
    Ok(report(paths, plan, &updated))
}

/// Which button finishes it, as GOG's update page says.
fn finish_hint(name: &str) -> String {
    format!("Use Finish update on {name} to finish it.")
}

fn interrupted(name: &str, work: Work) -> String {
    let noun = if work == Work::Repair {
        "Repair"
    } else {
        "Update"
    };
    format!("{noun} interrupted. {}", finish_hint(name))
}

/// What the update did, like the terminal's report.
fn report(paths: &Paths, plan: &Plan, updated: &Updated) -> Done {
    let name = plan.environment();
    let mut done = Done {
        lines: vec![match (plan.work(), plan.resumes()) {
            (Work::Repair, false) => format!("Repaired {name}."),
            (Work::Repair, true) => format!("Finished the interrupted repair of {name}."),
            (_, true) => format!("Finished the interrupted update of {name}."),
            (_, false) => format!("Updated {name}."),
        }],
        warnings: Vec::new(),
    };
    let repair = plan.work() == Work::Repair;
    for change in &updated.changes {
        let value = if repair {
            version(change.to.as_deref()).to_owned()
        } else {
            format!(
                "{} -> {}",
                change.from.as_deref().unwrap_or(if change.dlc {
                    "not installed"
                } else {
                    "unknown"
                }),
                version(change.to.as_deref())
            )
        };
        done.lines.push(format!("{}: {value}", change.title));
    }
    if updated.program_changed {
        // The entry's window class and icon come from the program.
        if desktop::supported(paths).is_ok()
            && let Err(error) = desktop::apply(paths, &updated.environment, false)
        {
            done.warnings
                .push(format!("Couldn't update desktop entries: {error:#}"));
        }
        if let Ok(environment) =
            wineo_core::environment::Environment::load(paths, &updated.environment)
            && let Some(target) = environment
                .config
                .targets
                .get(wineo_core::epic::install::TARGET)
        {
            done.lines.push(format!(
                "{}'s main program is now {}.",
                updated.environment,
                paths.contract_text(&target.exe)
            ));
        }
    }
    done.warnings.extend(updated.warnings.iter().cloned());
    done
}

#[cfg(test)]
pub(crate) mod tests;
