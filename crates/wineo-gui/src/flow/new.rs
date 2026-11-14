//! Guided creation of Windows and native environments.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{button, checkbox, column, container, pick_list, row, text, text_input};
use iced::{Center, Element, Fill, Task};
use wineo_core::cancel::Token;
use wineo_core::config::{Arch, GamescopeSetting, GlobalConfig, title_problem};
use wineo_core::create::{self, AfterInstall, CreateEvent, CreateRequest, Stepped, TargetOutcome};
use wineo_core::desktop;
use wineo_core::download::{Client, Provider};
use wineo_core::environment::{Environment, NewEnvironment};
use wineo_core::error::Error as CoreError;
use wineo_core::install::{self, Wanted};
use wineo_core::installer::{self, Outcome, Output};
use wineo_core::launch;
use wineo_core::name;
use wineo_core::paths::Paths;
use wineo_core::programs;
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;
use wineo_core::runtime::Runtime;
use wineo_core::supervisor::{Notice, Watched};
use wineo_core::targets::{self, Batch, Spec, Written};
use wineo_core::templates::{Template, Templates};

use super::artwork::{ArtOffer, Fetcher, default_fetcher, fetch_art};
use super::{Action, frame};
use crate::focus_button::ListRows;
use crate::logview::{self, LogView};
use crate::picker;
use crate::runner_label::{self, Choice};
use crate::task;
use crate::theme::Palette;
use crate::ui::{self, PickListStyle};

const LOG_ID: &str = "install-log";

const PROGRAM_LIST: &str = "new-programs";

/// The Name field's hint. Regular software goes through this flow too.
const NAME_HINT: &str = "Name";

const START_LIST: &str = "new-starts";

/// A row on the choice page: the kind and starting point it picks, and what
/// Details is titled for it.
struct Start {
    kind: Kind,
    source: Source,
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    heading: &'static str,
}

/// The choice page's rows in order. Number keys pick them from 1.
const STARTS: [Start; 5] = [
    Start {
        kind: Kind::Windows,
        source: Source::Installer,
        id: "new-windows-installer",
        title: "Run an installer",
        detail: "Set up a prefix, then run its setup.",
        heading: "New environment - Windows installer",
    },
    Start {
        kind: Kind::Windows,
        source: Source::Executable,
        id: "new-windows-existing",
        title: "Existing install",
        detail: "A program already on disk.",
        heading: "New environment - Windows existing install",
    },
    Start {
        kind: Kind::Windows,
        source: Source::Empty,
        id: "new-windows-empty",
        title: "Empty",
        detail: "A prefix to fill later.",
        heading: "New environment - Empty Windows",
    },
    Start {
        kind: Kind::Native,
        source: Source::Executable,
        id: "new-native-existing",
        title: "Existing install",
        detail: "A Linux program already on disk.",
        heading: "New environment - Native Linux existing install",
    },
    Start {
        kind: Kind::Native,
        source: Source::Empty,
        id: "new-native-empty",
        title: "Empty",
        detail: "Register it, add a target later.",
        heading: "New environment - Empty Native Linux",
    },
];

fn start_of(kind: Kind, source: Source) -> Option<&'static Start> {
    STARTS
        .iter()
        .find(|start| start.kind == kind && start.source == source)
}

/// A program row's id on the target step.
fn program_id(exe: &str) -> String {
    format!("new-program:{exe}")
}
const LOG_CAP: usize = 128 * 1024;
const LOG_KEEP: usize = 64 * 1024;
/// How long the draft has to stay put before the Summary checks it again.
const SETTLE: Duration = Duration::from_millis(300);
/// The most of a reason the strip beside Create shows. The Summary has all
/// of it, and a long path would leave a short dialog no room to scroll.
const PINNED_REASON_MAX: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Windows,
    Native,
}

impl Kind {
    const ALL: [Self; 2] = [Self::Windows, Self::Native];
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Windows => "Windows",
            Self::Native => "Native Linux",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Empty,
    Executable,
    Installer,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Empty => "Empty registration",
            Self::Executable => "Existing install",
            Self::Installer => "Run an installer",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Choose,
    Details,
    Working,
    Target,
    Finished,
}

#[derive(Debug, Clone)]
pub(crate) struct Options {
    runner: String,
    templates: Vec<String>,
    runners: Vec<Choice>,
    /// Whether to ask about other launchers' builds for a game.
    offer: bool,
    arch: Arch,
    prefix_root: PathBuf,
    /// Whether Art to fetch has any kind on.
    fetch_kinds: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Review {
    runner: Option<Runner>,
    /// The runner as the review shows it, worked out on the worker since
    /// naming its launcher reads the disk.
    runner_row: Option<String>,
    template: Option<Template>,
    arch: Arch,
    prefix: Option<PathBuf>,
    adopted: bool,
    omitted_components: bool,
    prerequisite: Option<Prerequisite>,
}

/// What the last check found, and the draft it checked. The Summary shows
/// it until the next check lands, so it doesn't blank on every keystroke.
#[derive(Debug, Clone)]
struct Summary {
    draft: Draft,
    found: Result<Review, Problem>,
}

/// What stops a draft before or during its check.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Problem {
    /// A required field the user hasn't touched yet, said quietly.
    Untouched(&'static str),
    Error(String),
}

/// Why Create is off.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hold {
    Checking,
    Problem(Problem),
    /// A runner or component the Summary offers to download.
    Missing(String),
}

#[derive(Debug, Clone)]
enum Prerequisite {
    Runner(String),
    Component {
        provider: Provider,
        version: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Report {
    registered: bool,
    playable: bool,
    cleanup_resolved: bool,
    /// Whether to ask which installed program to use.
    offer_programs: bool,
    notes: Vec<String>,
    programs: Vec<(String, String)>,
    art_error: Option<String>,
}

impl Report {
    fn new() -> Self {
        Self {
            registered: false,
            playable: false,
            cleanup_resolved: true,
            offer_programs: false,
            notes: Vec::new(),
            programs: Vec::new(),
            art_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Draft {
    /// The free-text game title. The environment name derives from it.
    title: String,
    /// The user's own environment name. None follows the title, and clearing
    /// the text resumes following it.
    environment: Option<String>,
    kind: Kind,
    source: Source,
    path: String,
    prefix: String,
    runner: String,
    template: String,
    arch: Option<Arch>,
    desktop: bool,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            title: String::new(),
            environment: None,
            kind: Kind::Windows,
            source: Source::Empty,
            path: String::new(),
            prefix: String::new(),
            runner: String::new(),
            template: String::new(),
            arch: None,
            desktop: false,
        }
    }
}

impl Draft {
    fn derived_environment(&self) -> String {
        name::from_title(&self.title)
    }

    /// The name used for the environment, its files, and its config.
    fn environment_name(&self) -> String {
        match self.environment.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => self.derived_environment(),
        }
    }

    /// What the Environment name override shows while it's empty: the name it
    /// falls back to, once there is one.
    fn environment_placeholder(&self) -> String {
        let derived = self.derived_environment();
        if derived.is_empty() {
            "Environment name".to_owned()
        } else {
            derived
        }
    }

    /// What the desktop entry is called and what the card shows, when the
    /// title says anything.
    fn target_title(&self) -> Option<String> {
        let title = self.title.trim();
        (!title.is_empty()).then(|| title.to_owned())
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Back,
    OpenExisting,
    Discard,
    KeepEditing,
    /// A row on the choice page.
    Start(Kind, Source),
    /// Details' Change, back to the choice page.
    Change,
    Name(String),
    EditEnvironment,
    EnvironmentName(String),
    Path(String),
    Prefix(String),
    Runner(String),
    Template(String),
    ToggleAdditionalWineOptions,
    Arch(Arch),
    /// The architecture override back to unset.
    ArchDefault,
    Desktop(bool),
    Browse,
    BrowsePrefix,
    PrefixPicked(u64, Result<Option<PathBuf>, String>),
    Picked(u64, Result<Option<PathBuf>, String>),
    /// The draft stayed put after the edit with this check number.
    Settled(u64),
    /// A check's result, with the draft it checked.
    Checked(u64, Box<Draft>, Result<Box<Review>, String>),
    Create,
    Download(Provider),
    Downloaded(u64, Result<String, String>),
    Cancel,
    Force,
    OpenSettings,
    ToLibrary,
    Launch(bool),
    TargetPath(String),
    TargetDesktop(bool),
    Program(String),
    AddTarget,
    SkipTarget,
    CleanUp,
    Cleaned(u64, Result<bool, String>),
    RetryArt,
    /// Finished's way back after a creation that didn't register.
    BackToDetails,
    SkipArt,
    FetchArt(bool),
    SkipFetch,
    ChooseArt,
    ArtFetched(u64, Result<Vec<String>, String>),
    TargetAdded(u64, Result<Vec<String>, String>),
    Options(Result<Options, String>),
    /// The answer to using other launchers' builds.
    Offer(bool),
    Offered(u64, Result<Options, String>),
    Progress(u64, String),
    LogChunk(u64, String),
    LogAction(iced::widget::text_editor::Action),
    LogScrolled(iced::widget::scrollable::Viewport),
    LogFollow,
    CopyLog,
    Done(u64, Report),
}

pub(crate) struct Page {
    paths: Paths,
    draft: Draft,
    /// Set once a row on the choice page is picked, so coming back focuses it.
    picked: bool,
    /// The fetch checkbox: None until it's clicked, then what it was set to.
    /// Whether it shows depends on the key, which can come and go.
    fetch_choice: Option<bool>,
    /// The checkbox as it was at Create.
    fetch_art: bool,
    /// Set once the fetch after creation starts, so it never runs again.
    art_started: bool,
    /// Set while Working fetches art rather than doing work Cancel stops.
    fetching: bool,
    fetch_token: Option<Token>,
    fetcher: Fetcher,
    stage: Stage,
    options: Option<Options>,
    template_preview: Option<Template>,
    additional_wine_options: bool,
    editing_environment: bool,
    /// Whether Name or Environment name has been typed into.
    name_touched: bool,
    /// Whether the path has been typed or browsed for, since the row was picked.
    path_touched: bool,
    summary: Option<Summary>,
    /// Bumped by every edit and by leaving Details, so an older check, or
    /// its wait, drops. Downloads and creation keep `generation`.
    check_sequence: u64,
    /// From an edit until the check of the draft as it is lands.
    checking: bool,
    report: Option<Report>,
    error: Option<String>,
    status: String,
    log: String,
    log_view: LogView,
    target_path: String,
    target_desktop: bool,
    launch: bool,
    leave_prompt: bool,
    /// Where Discard goes instead of the Add choices, when the prompt was
    /// opened by a way out other than Back.
    leave_for: Option<Action>,
    action: Option<Action>,
    generation: u64,
    token: Option<Token>,
    downloading: Option<Provider>,
    picker_sequence: u64,
    /// Set while an answer to the offer is being saved.
    answering: bool,
    offer_sequence: u64,
}

impl Page {
    pub(crate) fn new(paths: Paths) -> (Self, Task<Event>) {
        let mut page = Self {
            paths,
            draft: Draft::default(),
            picked: false,
            fetch_choice: None,
            fetch_art: false,
            art_started: false,
            fetching: false,
            fetch_token: None,
            fetcher: default_fetcher(),
            stage: Stage::Choose,
            options: None,
            template_preview: None,
            additional_wine_options: false,
            editing_environment: false,
            name_touched: false,
            path_touched: false,
            summary: None,
            check_sequence: 0,
            checking: false,
            report: None,
            error: None,
            status: String::new(),
            log: String::new(),
            log_view: LogView::default(),
            target_path: String::new(),
            target_desktop: false,
            launch: false,
            leave_prompt: false,
            leave_for: None,
            action: None,
            generation: 0,
            token: None,
            downloading: None,
            picker_sequence: 0,
            answering: false,
            offer_sequence: 0,
        };
        let task = page.load_options();
        (page, task)
    }

    fn load_options(&mut self) -> Task<Event> {
        self.generation = self.generation.wrapping_add(1);
        let paths = self.paths.clone();
        task::blocking(move || read_options(&paths), Event::Options)
    }

    /// The question shows for a game, which is where Steam, Heroic, and
    /// Lutris builds matter, until it's answered.
    fn offer_shown(&self) -> bool {
        self.stage == Stage::Details
            && self.draft.kind == Kind::Windows
            && self.options.as_ref().is_some_and(|options| options.offer)
            && self
                .template_preview
                .as_ref()
                .is_some_and(|template| template.tags.iter().any(|tag| tag == "game"))
    }

    #[must_use]
    fn append_log(&mut self, text: &str) -> bool {
        let text = logview::normalize(text);
        self.log.push_str(&text);
        let mut dropped = String::new();
        if self.log.len() > LOG_CAP {
            let mut start = self.log.len() - LOG_KEEP;
            while !self.log.is_char_boundary(start) {
                start += 1;
            }
            dropped = self.log.drain(..start).collect();
        }
        let log = &self.log;
        self.log_view.push(&text, &dropped, || log.clone())
    }

    /// Fetching art isn't busy: closing or Settings may go ahead, since the
    /// fetch stops at its token and writes only to an environment that's
    /// still there.
    pub(crate) fn busy(&self) -> bool {
        self.stage == Stage::Working && !self.fetching
    }

    fn art_offer(&self, key_available: bool) -> ArtOffer {
        ArtOffer::new(
            self.options.as_ref().map(|options| options.fetch_kinds),
            key_available,
            self.fetch_choice,
        )
    }

    /// A page on Finished for `name`, which registered.
    #[cfg(test)]
    pub(crate) fn finished_for_tests(paths: Paths, name: &str) -> Self {
        let mut page = Self::new(paths).0;
        page.draft.environment = Some(name.to_owned());
        let mut report = Report::new();
        report.registered = true;
        page.report = Some(report);
        page.show_finished();
        page
    }

    /// The options as the page loads them, read at once.
    #[cfg(test)]
    pub(crate) fn options_loaded(paths: &Paths) -> Event {
        Event::Options(read_options(paths))
    }

    /// Whether Details shows the fetch checkbox, and it's on.
    #[cfg(test)]
    pub(crate) fn fetch_checked(&self, key_available: bool) -> bool {
        self.art_offer(key_available) == ArtOffer::Fetch(true)
    }

    /// Stops the art fetch, if one runs, when the window closes.
    pub(crate) fn cancel_fetch(&self) {
        if let Some(token) = &self.fetch_token {
            token.cancel();
        }
    }

    /// The kind and starting point Details is open for, if it shows.
    #[cfg(test)]
    pub(crate) fn details_for(&self) -> Option<(Kind, Source)> {
        (self.stage == Stage::Details).then_some((self.draft.kind, self.draft.source))
    }

    #[cfg(test)]
    pub(crate) fn leave_prompt_open(&self) -> bool {
        self.leave_prompt
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        if let Some(token) = &self.token {
            if force {
                token.force();
                self.status = "Force stopping...".to_owned();
            } else {
                token.cancel();
                self.status = "Canceling and waiting for cleanup...".to_owned();
            }
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    /// The choice page's rows, landing on the one Details came from, or the
    /// target step's program list.
    pub(crate) fn rows(&self) -> Option<ListRows> {
        match self.stage {
            Stage::Choose => Some(ListRows {
                list: START_LIST,
                ids: STARTS.iter().map(|start| start.id.to_owned()).collect(),
                focus: self
                    .picked
                    .then(|| start_of(self.draft.kind, self.draft.source))
                    .flatten()
                    .map(|start| start.id.to_owned()),
            }),
            Stage::Target => Some(ListRows {
                list: PROGRAM_LIST,
                ids: self
                    .report
                    .iter()
                    .flat_map(|report| &report.programs)
                    .map(|(_, exe)| program_id(exe))
                    .collect(),
                focus: None,
            }),
            _ => None,
        }
    }

    /// Whether the choice page shows, where number keys pick its rows.
    pub(crate) fn choosing(&self) -> bool {
        self.stage == Stage::Choose
    }

    /// The choice page's row for number key `digit`.
    pub(crate) fn numbered(digit: &str) -> Option<Event> {
        let index = digit.parse::<usize>().ok()?.checked_sub(1)?;
        let start = STARTS.get(index)?;
        Some(Event::Start(start.kind, start.source))
    }

    /// Whether the draft has anything typed or chosen on Details that its
    /// kind and starting point show. The row picked and the fetch checkbox
    /// don't count.
    fn has_edits(&self) -> bool {
        let draft = &self.draft;
        let blank = Draft::default();
        let mut edited = draft.title.trim() != blank.title
            || draft
                .environment
                .as_deref()
                .is_some_and(|name| !name.trim().is_empty());
        if draft.source != Source::Empty {
            edited |= draft.path.trim() != blank.path;
        }
        if draft.source == Source::Executable {
            edited |= draft.desktop != blank.desktop;
        }
        if draft.kind == Kind::Windows {
            edited |= draft.prefix.trim() != blank.prefix
                || draft.runner.trim() != blank.runner
                || draft.template != blank.template
                || draft.arch != blank.arch;
        }
        edited
    }

    /// Opens Details for a row and checks the draft. Another row than the
    /// draft's keeps the names but not the path, since an installer and a
    /// program path mean different things, and resets the desktop entry.
    fn pick(&mut self, kind: Kind, source: Source) -> Task<Event> {
        if (kind, source) != (self.draft.kind, self.draft.source) {
            self.draft.kind = kind;
            self.draft.source = source;
            self.draft.path.clear();
            self.draft.desktop = Draft::default().desktop;
            self.path_touched = false;
            self.error = None;
        }
        self.picked = true;
        self.stage = Stage::Details;
        self.check_now()
    }

    /// Back to the choice page with the draft kept. A check still running
    /// or waiting lands nowhere: it only lands on Details, and opening
    /// Details checks again under a new number.
    fn change(&mut self) {
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
        self.status.clear();
        self.stage = Stage::Choose;
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        // Esc answers the discard prompt like Keep editing instead of asking again.
        if self.leave_prompt {
            return self.update(Event::KeepEditing);
        }
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
        match self.stage {
            Stage::Choose => {
                if self.has_edits() {
                    self.leave_prompt = true;
                } else {
                    self.action = Some(Action::Choose);
                }
            }
            Stage::Details => self.change(),
            Stage::Working if self.fetching => self.skip_fetch(),
            Stage::Working => self.cancel(false),
            Stage::Target => return self.finish(),
            Stage::Finished => self.leave(false),
        }
        Task::none()
    }

    /// Ctrl+Enter. `key_available` matters, since Create captures the fetch
    /// checkbox as it shows.
    pub(crate) fn submit(&mut self, key_available: bool) -> Task<Event> {
        if self.busy() || self.leave_prompt {
            return Task::none();
        }
        let event = match self.stage {
            Stage::Choose => return open_focused_start(),
            // Create checks its own gate, so a press while the Summary
            // checks does nothing and isn't kept for later.
            Stage::Details => Event::Create,
            Stage::Target => Event::AddTarget,
            Stage::Finished => Event::ToLibrary,
            Stage::Working => return Task::none(),
        };
        self.update_with_key_availability(event, key_available)
    }

    /// After an edit on Details: the Summary checks the draft once it has
    /// stayed put for `SETTLE`, and Create waits for that check.
    fn edited(&mut self) -> Task<Event> {
        self.picker_sequence = self.picker_sequence.wrapping_add(1);
        self.error = None;
        self.check_sequence = self.check_sequence.wrapping_add(1);
        self.checking = true;
        let sequence = self.check_sequence;
        Task::perform(async { tokio::time::sleep(SETTLE).await }, move |()| {
            Event::Settled(sequence)
        })
    }

    /// Checks the draft at once, dropping any check before it.
    fn check_now(&mut self) -> Task<Event> {
        self.check_sequence = self.check_sequence.wrapping_add(1);
        self.run_check()
    }

    /// Checks the draft under the current check number: the fields here,
    /// then `prepare` on the worker.
    fn run_check(&mut self) -> Task<Event> {
        let sequence = self.check_sequence;
        if let Some(problem) = self.problem() {
            self.checking = false;
            self.summary = Some(Summary {
                draft: self.draft.clone(),
                found: Err(problem),
            });
            return Task::none();
        }
        self.checking = true;
        let paths = self.paths.clone();
        let draft = self.draft.clone();
        let checked = Box::new(self.draft.clone());
        task::blocking(
            move || prepare(&paths, &draft),
            move |result| Event::Checked(sequence, checked, result.map(Box::new)),
        )
    }

    /// What `validate` finds, said quietly for a blank field the user hasn't
    /// touched yet.
    fn problem(&self) -> Option<Problem> {
        let reason = |touched: bool, reason: &'static str| {
            if touched {
                Problem::Error(reason.to_owned())
            } else {
                Problem::Untouched(reason)
            }
        };
        let draft = &self.draft;
        let no_override = draft
            .environment
            .as_deref()
            .is_none_or(|name| name.trim().is_empty());
        if draft.title.trim().is_empty() && no_override {
            return Some(reason(self.name_touched, "Enter a name"));
        }
        if let Err(error) = self.validate_name() {
            return Some(Problem::Error(error));
        }
        if draft.source != Source::Empty && draft.path.trim().is_empty() {
            let field = if draft.source == Source::Installer {
                "Choose an installer"
            } else {
                "Choose a program"
            };
            return Some(reason(self.path_touched, field));
        }
        self.validate().err().map(Problem::Error)
    }

    /// Whether the Summary may not match what Create would do yet: a check
    /// is pending or running, or an answer to the offer, which changes which
    /// runners resolve, is still being saved.
    fn rechecking(&self) -> bool {
        self.checking || self.answering
    }

    /// The last check's review when it's of the draft as it is now and
    /// nothing is missing, or why Create is off.
    fn checked(&self) -> Result<&Review, Hold> {
        // The draft comparison holds even for an edit that skipped `edited`.
        let summary = match &self.summary {
            Some(summary) if !self.rechecking() && summary.draft == self.draft => summary,
            _ => return Err(Hold::Checking),
        };
        let review = summary
            .found
            .as_ref()
            .map_err(|problem| Hold::Problem(problem.clone()))?;
        match &review.prerequisite {
            Some(Prerequisite::Runner(name)) => {
                Err(Hold::Missing(format!("Runner {name} isn't installed")))
            }
            Some(Prerequisite::Component { provider, .. }) => {
                Err(Hold::Missing(format!("{provider} isn't installed")))
            }
            None => Ok(review),
        }
    }

    fn additional_wine_options_visible(&self) -> bool {
        self.draft.kind == Kind::Windows && self.additional_wine_options
    }

    fn validate(&self) -> Result<(), String> {
        self.validate_name()?;
        if self.draft.source != Source::Empty && self.draft.path.trim().is_empty() {
            return Err("Choose an executable or installer".to_owned());
        }
        if self.draft.kind == Kind::Native && self.draft.source == Source::Installer {
            return Err("Native environments cannot run a Windows installer".to_owned());
        }
        Ok(())
    }

    fn validate_name(&self) -> Result<(), String> {
        let name = self.draft.environment_name();
        if name.is_empty() {
            return Err("Name needs letters or digits.".to_owned());
        }
        if let Some(problem) = name::problem(&name) {
            return Err(problem);
        }
        if name.len() > name::TITLE_MAX {
            return Err("Name is too long.".to_owned());
        }
        if self.paths.environment_config(&name).exists() {
            return Err(format!(
                "An environment named {name} already exists. Open its Settings from the library."
            ));
        }
        // The Name is written as the title and core refuses a bad one at
        // create, so catch it here, where the field can still be fixed.
        if let Some(title) = self.draft.target_title()
            && let Some(problem) = title_problem(&title)
        {
            return Err(problem);
        }
        Ok(())
    }

    fn start(&mut self, key_available: bool) -> Task<Event> {
        let Ok(review) = self.checked() else {
            return Task::none();
        };
        let review = review.clone();
        // The disk may have changed since the check, like another
        // environment taking the name.
        if let Err(error) = self.validate() {
            self.summary = Some(Summary {
                draft: self.draft.clone(),
                found: Err(Problem::Error(error)),
            });
            return Task::none();
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let draft = self.draft.clone();
        // The fetch may wait until after Target, by when the key may be gone.
        self.fetch_art = self.art_offer(key_available) == ArtOffer::Fetch(true);
        let token = Token::new();
        self.token = Some(token.clone());
        self.stage = Stage::Working;
        self.status = "Creating environment...".to_owned();
        self.log.clear();
        self.log_view = LogView::default();
        let (sender, receiver) = unbounded();
        let log_sender = sender.clone();
        let work = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    perform(
                        &paths,
                        draft,
                        review,
                        token,
                        |line| {
                            let _ = sender.unbounded_send(Event::Progress(generation, line));
                        },
                        |chunk| {
                            let _ = log_sender.unbounded_send(Event::LogChunk(generation, chunk));
                        },
                    )
                })
                .await
                .unwrap_or_else(|error| {
                    let mut report = Report::new();
                    report.notes.push(error.to_string());
                    report
                })
            },
            move |report| Event::Done(generation, report),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    fn download(&mut self, provider: Provider) -> Task<Event> {
        let Err(Hold::Missing(_)) = self.checked() else {
            return Task::none();
        };
        let Some(prerequisite) = self
            .summary
            .as_ref()
            .and_then(|summary| summary.found.as_ref().ok())
            .and_then(|review| review.prerequisite.clone())
        else {
            return Task::none();
        };
        let valid = match &prerequisite {
            Prerequisite::Runner(name) => match name.as_str() {
                "GE-Proton-Latest" => provider == Provider::GeProton,
                "Proton-CachyOS-Latest" => provider == Provider::ProtonCachyos,
                _ => matches!(provider, Provider::GeProton | Provider::ProtonCachyos),
            },
            Prerequisite::Component {
                provider: expected, ..
            } => *expected == provider,
        };
        if !valid {
            return Task::none();
        }
        // An earlier download's failure is out of date once this one starts.
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = self.paths.clone();
        let token = Token::new();
        self.token = Some(token.clone());
        self.downloading = Some(provider);
        self.stage = Stage::Working;
        self.status = format!("Finding {provider} release...");
        let (sender, receiver) = unbounded();
        let work = task::blocking(
            move || {
                let client = Client::new().map_err(|e| e.to_string())?;
                let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                let release = match prerequisite {
                    Prerequisite::Component {
                        version: Some(version),
                        ..
                    } => client.select_pinned(provider, &version),
                    _ => client.select(provider, None),
                }
                .map_err(|e| e.to_string())?;
                let name = release.installed_name.clone();
                let _ = sender.unbounded_send(Event::Progress(
                    generation,
                    format!("Downloading {} ({})", release.tag, release.verification()),
                ));
                crate::download_progress::install(
                    &client,
                    &paths,
                    &global,
                    &release,
                    &token,
                    Duration::from_millis(250),
                    |line| {
                        let _ = sender.unbounded_send(Event::Progress(generation, line));
                    },
                )
                .map_err(|e| e.to_string())?;
                Ok::<_, String>(name)
            },
            move |result| Event::Downloaded(generation, result),
        );
        Task::batch([Task::run(receiver, |event| event), work])
    }

    /// Moves on to Finished, fetching art first the first time once the
    /// environment registered and the checkbox was on at Create. Everything
    /// on its way to Finished goes through here.
    fn finish(&mut self) -> Task<Event> {
        let registered = self.report.as_ref().is_some_and(|report| report.registered);
        if registered && self.fetch_art && !self.art_started {
            return self.start_fetch();
        }
        self.show_finished();
        Task::none()
    }

    fn show_finished(&mut self) {
        self.stage = Stage::Finished;
        self.token = None;
    }

    /// Fetches art on Working, with Skip, then reapplies the desktop entry.
    /// Retry artwork comes here too.
    fn start_fetch(&mut self) -> Task<Event> {
        self.art_started = true;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let token = Token::new();
        self.fetch_token = Some(token.clone());
        self.token = None;
        self.fetching = true;
        self.stage = Stage::Working;
        self.status = "Fetching artwork...".to_owned();
        let paths = self.paths.clone();
        let name = self.draft.environment_name();
        let title = self.draft.target_title().unwrap_or_else(|| name.clone());
        let fetcher = self.fetcher.clone();
        task::blocking(
            move || fetch_art(&paths, &name, &title, &token, &fetcher),
            move |result| Event::ArtFetched(generation, result),
        )
    }

    /// The fetch's result lands, and Working leaves, once the worker sees
    /// the token.
    fn skip_fetch(&mut self) {
        if let Some(token) = &self.fetch_token
            && !token.is_canceled()
        {
            token.cancel();
            self.status = "Skipping artwork...".to_owned();
        }
    }

    fn leave(&mut self, settings: bool) {
        let name = self.draft.environment_name();
        let registered = self.report.as_ref().is_some_and(|report| report.registered);
        let launch = self.launch
            && self
                .report
                .as_ref()
                .is_some_and(|report| report.playable && report.cleanup_resolved);
        self.action = Some(if settings && registered {
            Action::Settings { name, launch }
        } else {
            Action::Library {
                name: registered.then_some(name),
                launch,
            }
        });
    }

    fn add_target(&mut self) -> Task<Event> {
        let exe = self.target_path.trim().to_owned();
        if exe.is_empty() {
            self.error = Some("Choose a program".to_owned());
            return Task::none();
        }
        let paths = self.paths.clone();
        let name = self.draft.environment_name();
        let title = self.draft.target_title();
        let desktop = self.target_desktop;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.stage = Stage::Working;
        self.status = "Adding target...".to_owned();
        task::blocking(
            move || {
                let environment = Environment::load(&paths, &name).map_err(|e| e.to_string())?;
                let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                if let Some(wine) = environment.config.wine() {
                    let runner = Runners::new(&paths, &global)
                        .and_then(|runners| runners.resolve(&wine.runner))
                        .map_err(|e| e.to_string())?;
                    let prefix = environment.prefix(&paths).map_err(|e| e.to_string())?;
                    desktop::check(&paths, &runner.wine_prefix(&prefix), &exe)
                        .map_err(|e| e.to_string())?;
                } else {
                    launch::check_native_exe(&paths, &global, &name, &Default::default(), &exe)
                        .map_err(|e| e.to_string())?;
                }
                let mut batch = Batch::new(environment.config.targets);
                batch
                    .add(&name, target_spec(exe, title, desktop))
                    .map_err(|e| e.to_string())?;
                batch.set_default(Some(&name));
                match targets::write(&paths, &name, &batch).map_err(|e| e.to_string())? {
                    Written::Saved => {}
                    Written::Changed(_) => {
                        return Err(
                            "Targets changed on disk. Open Settings and review them.".to_owned()
                        );
                    }
                }
                let environment = Environment::load(&paths, &name).map_err(|e| e.to_string())?;
                let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                let note = if let Some(wine) = environment.config.wine() {
                    let runner = Runners::new(&paths, &global)
                        .and_then(|runners| runners.resolve(&wine.runner))
                        .map_err(|e| e.to_string())?;
                    let prefix = environment.prefix(&paths).map_err(|e| e.to_string())?;
                    desktop::reconcile(&paths, &global, &environment, Some(&runner), &prefix)
                        .err()
                        .map(|error| format!("Desktop entry: {error}"))
                } else {
                    desktop::reconcile_native(&paths, &global, &environment)
                        .err()
                        .map(|error| format!("Desktop entry: {error}"))
                };
                Ok::<_, String>(note.into_iter().collect())
            },
            move |result| Event::TargetAdded(generation, result),
        )
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        self.update_with_key_availability(event, false)
    }

    pub(crate) fn update_with_key_availability(
        &mut self,
        event: Event,
        key_available: bool,
    ) -> Task<Event> {
        // A finished check, or the offer's answer that Create waits on,
        // still lands, so the Summary matches what Create would do once the
        // prompt is answered.
        if self.leave_prompt
            && !matches!(
                event,
                Event::Back
                    | Event::KeepEditing
                    | Event::Discard
                    | Event::Settled(_)
                    | Event::Checked(..)
                    | Event::Offered(..)
            )
        {
            return Task::none();
        }
        match event {
            Event::Back => return self.back(),
            Event::OpenExisting if self.stage == Stage::Details => {
                let name = self.draft.environment_name();
                if self.paths.environment_config(&name).exists() {
                    let settings = Action::Settings {
                        name,
                        launch: false,
                    };
                    // Leaving drops the draft, so a draft with edits asks first.
                    if self.has_edits() {
                        self.leave_prompt = true;
                        self.leave_for = Some(settings);
                    } else {
                        self.action = Some(settings);
                    }
                }
            }
            Event::Discard => {
                self.leave_prompt = false;
                self.action = Some(self.leave_for.take().unwrap_or(Action::Choose));
            }
            Event::KeepEditing => {
                self.leave_prompt = false;
                self.leave_for = None;
            }
            Event::Start(kind, source)
                if self.stage == Stage::Choose && start_of(kind, source).is_some() =>
            {
                return self.pick(kind, source);
            }
            Event::Change if self.stage == Stage::Details => self.change(),
            Event::Name(value) if self.stage == Stage::Details => {
                self.draft.title = value;
                self.name_touched = true;
                return self.edited();
            }
            Event::EditEnvironment if self.stage == Stage::Details => {
                self.editing_environment = true;
            }
            Event::EnvironmentName(value) if self.stage == Stage::Details => {
                self.draft.environment = Some(value);
                self.name_touched = true;
                return self.edited();
            }
            Event::Path(value) if self.stage == Stage::Details => {
                self.draft.path = value;
                self.path_touched = true;
                return self.edited();
            }
            Event::Prefix(value) if self.stage == Stage::Details => {
                self.draft.prefix = value;
                return self.edited();
            }
            Event::BrowsePrefix if self.stage == Stage::Details => {
                self.picker_sequence = self.picker_sequence.wrapping_add(1);
                let sequence = self.picker_sequence;
                return Task::perform(
                    picker::pick("Select prefix folder".to_owned(), true),
                    move |result| Event::PrefixPicked(sequence, result),
                );
            }
            Event::PrefixPicked(sequence, result)
                if sequence == self.picker_sequence && self.stage == Stage::Details =>
            {
                match result {
                    Ok(Some(path)) => {
                        self.draft.prefix = path.to_string_lossy().into_owned();
                        return self.edited();
                    }
                    Ok(None) => {}
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Runner(value) if self.stage == Stage::Details => {
                self.draft.runner = value;
                return self.edited();
            }
            Event::Template(value) if self.stage == Stage::Details => {
                self.draft.template = value;
                let check = self.edited();
                self.template_preview = if self.draft.template.is_empty() {
                    None
                } else {
                    match Templates::new(&self.paths).resolve(&self.draft.template) {
                        Ok(template) => Some(template),
                        Err(error) => {
                            self.error = Some(error.to_string());
                            None
                        }
                    }
                };
                return check;
            }
            Event::ToggleAdditionalWineOptions if self.stage == Stage::Details => {
                self.additional_wine_options = !self.additional_wine_options;
            }
            Event::Arch(value) if self.stage == Stage::Details => {
                self.draft.arch = Some(value);
                return self.edited();
            }
            Event::ArchDefault if self.stage == Stage::Details => {
                self.draft.arch = None;
                return self.edited();
            }
            Event::Desktop(value) if self.stage == Stage::Details => {
                self.draft.desktop = value;
                return self.edited();
            }
            Event::Browse if matches!(self.stage, Stage::Details | Stage::Target) => {
                self.picker_sequence = self.picker_sequence.wrapping_add(1);
                let sequence = self.picker_sequence;
                return Task::perform(
                    picker::pick("Choose program".to_owned(), false),
                    move |result| Event::Picked(sequence, result),
                );
            }
            Event::Picked(sequence, result) if sequence == self.picker_sequence => match result {
                Ok(Some(path)) => {
                    if self.stage == Stage::Target {
                        self.target_path = path.display().to_string();
                    } else if self.stage == Stage::Details {
                        self.draft.path = path.display().to_string();
                        self.path_touched = true;
                        return self.edited();
                    }
                }
                Ok(None) => {}
                Err(error) => self.error = Some(error),
            },
            Event::Settled(sequence)
                if sequence == self.check_sequence
                    && self.checking
                    && self.stage == Stage::Details =>
            {
                return self.run_check();
            }
            Event::Checked(sequence, draft, result)
                if sequence == self.check_sequence && self.stage == Stage::Details =>
            {
                self.checking = false;
                self.summary = Some(Summary {
                    draft: *draft,
                    found: result.map(|review| *review).map_err(Problem::Error),
                });
            }
            Event::BackToDetails
                if self.stage == Stage::Finished
                    && self
                        .report
                        .as_ref()
                        .is_some_and(|report| !report.registered) =>
            {
                self.stage = Stage::Details;
                self.report = None;
                self.error = None;
                return self.check_now();
            }
            Event::Create if self.stage == Stage::Details => return self.start(key_available),
            Event::Download(provider) if self.stage == Stage::Details => {
                return self.download(provider);
            }
            Event::Downloaded(generation, result)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                // A cancel the user chose isn't an error to show.
                let canceled = self.token.take().is_some_and(|token| token.is_canceled());
                self.downloading = None;
                match result {
                    Ok(name) => {
                        if self.summary.as_ref().is_some_and(|summary| {
                            matches!(&summary.found, Ok(Review { prerequisite: Some(Prerequisite::Runner(runner)), .. })
                                if runner != "GE-Proton-Latest" && runner != "Proton-CachyOS-Latest")
                        }) {
                            self.draft.runner = name;
                        }
                    }
                    Err(_) if canceled => {}
                    Err(error) => self.error = Some(error),
                }
                self.status.clear();
                self.stage = Stage::Details;
                return self.check_now();
            }
            Event::Progress(generation, line)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                self.status.clone_from(&line);
                if self.append_log(&format!("{line}\n")) {
                    return snap_log();
                }
            }
            Event::LogChunk(generation, chunk)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                if self.append_log(&chunk) {
                    return snap_log();
                }
            }
            Event::LogAction(action) => {
                let log = &self.log;
                if self.log_view.perform(action, || log.clone()) {
                    return snap_log();
                }
            }
            Event::LogScrolled(viewport) => {
                let log = &self.log;
                let snap = self.log_view.scrolled(
                    viewport.absolute_offset().y,
                    viewport.bounds().height,
                    viewport.content_bounds().height,
                    || log.clone(),
                );
                if snap {
                    return snap_log();
                }
            }
            Event::LogFollow => {
                let log = &self.log;
                if self.log_view.follow(|| log.clone()) {
                    return snap_log();
                }
            }
            Event::CopyLog => {
                self.log_view.mark_copied();
                return iced::clipboard::write(self.log.clone());
            }
            Event::Done(generation, report)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                let can_select = report.registered && report.offer_programs;
                self.report = Some(report);
                self.token = None;
                if can_select {
                    self.stage = Stage::Target;
                } else {
                    return self.finish();
                }
            }
            Event::Cancel if self.busy() => self.cancel(false),
            Event::Force if self.busy() => self.cancel(true),
            Event::TargetPath(value) if self.stage == Stage::Target => {
                self.picker_sequence = self.picker_sequence.wrapping_add(1);
                self.target_path = value;
            }
            Event::TargetDesktop(value) if self.stage == Stage::Target => {
                self.target_desktop = value
            }
            Event::Program(exe) if self.stage == Stage::Target => self.target_path = exe,
            Event::AddTarget if self.stage == Stage::Target => return self.add_target(),
            Event::SkipTarget if self.stage == Stage::Target => return self.finish(),
            Event::TargetAdded(generation, result)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                match result {
                    Ok(notes) => {
                        if let Some(report) = &mut self.report {
                            report.playable = true;
                            report.notes.extend(notes);
                        }
                        return self.finish();
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.stage = Stage::Target;
                    }
                }
            }
            Event::ArtFetched(generation, result)
                if generation == self.generation && self.fetching =>
            {
                self.fetching = false;
                let skipped = self
                    .fetch_token
                    .take()
                    .is_some_and(|token| token.is_canceled());
                if let Some(report) = &mut self.report {
                    match result {
                        Ok(notes) => {
                            report.notes.extend(notes);
                            report.art_error = None;
                        }
                        Err(_) if skipped => {
                            report.art_error = None;
                            report.notes.push("Artwork skipped".to_owned());
                        }
                        Err(error) => report.art_error = Some(error),
                    }
                }
                self.show_finished();
            }
            Event::SkipFetch if self.fetching => self.skip_fetch(),
            Event::CleanUp
                if self.stage == Stage::Finished
                    && self.report.as_ref().is_some_and(|r| !r.cleanup_resolved) =>
            {
                self.generation = self.generation.wrapping_add(1);
                let generation = self.generation;
                let paths = self.paths.clone();
                let name = self.draft.environment_name();
                self.stage = Stage::Working;
                self.status = "Cleaning up installer...".to_owned();
                return task::blocking(
                    move || {
                        let global = GlobalConfig::load(&paths).map_err(|e| e.to_string())?;
                        let outcome = installer::clean_up(
                            &paths,
                            &global,
                            &name,
                            installer::clean_up_timeout(&global),
                        )
                        .map_err(|e| e.to_string())?;
                        Ok::<_, String>(!matches!(
                            outcome,
                            Some(installer::Cleaned::Unresolved | installer::Cleaned::Untracked)
                        ))
                    },
                    move |result| Event::Cleaned(generation, result),
                );
            }
            Event::Cleaned(generation, result)
                if generation == self.generation && self.stage == Stage::Working =>
            {
                match result {
                    Ok(true) => {
                        if let Some(report) = &mut self.report {
                            report.cleanup_resolved = true;
                            report.notes.push("Installer cleaned up".to_owned());
                        }
                        if self.draft.source == Source::Installer {
                            self.stage = Stage::Target;
                        } else {
                            return self.finish();
                        }
                    }
                    Ok(false) => {
                        self.error = Some("Installer cleanup is still unresolved".to_owned());
                        return self.finish();
                    }
                    Err(error) => {
                        self.error = Some(error);
                        return self.finish();
                    }
                }
            }
            Event::SkipArt if self.stage == Stage::Finished => {
                if let Some(report) = &mut self.report
                    && report.art_error.take().is_some()
                {
                    report.notes.push("Artwork skipped".to_owned());
                }
            }
            Event::RetryArt
                if self.stage == Stage::Finished
                    && self.report.as_ref().is_some_and(|r| r.art_error.is_some()) =>
            {
                return self.start_fetch();
            }
            Event::Launch(value) if self.stage == Stage::Finished => self.launch = value,
            Event::OpenSettings if self.stage == Stage::Finished => self.leave(true),
            Event::ToLibrary if self.stage == Stage::Finished => self.leave(false),
            // Never launches: hide on launch would hide the Art page.
            Event::ChooseArt
                if self.stage == Stage::Finished
                    && self.report.as_ref().is_some_and(|r| r.registered) =>
            {
                self.action = Some(Action::Art {
                    name: self.draft.environment_name(),
                });
            }
            // Not an edit, so it never makes Back ask about discarding.
            Event::FetchArt(on) if self.stage == Stage::Details => self.fetch_choice = Some(on),
            Event::Options(result) => match result {
                Ok(options) => {
                    self.options = Some(options);
                }
                Err(error) => self.error = Some(error),
            },
            Event::Offer(on) if self.offer_shown() && !self.answering => {
                self.answering = true;
                self.offer_sequence = self.offer_sequence.wrapping_add(1);
                let sequence = self.offer_sequence;
                let paths = self.paths.clone();
                return task::blocking(
                    move || answer_offer(&paths, on),
                    move |result| Event::Offered(sequence, result),
                );
            }
            Event::Offered(sequence, result) if sequence == self.offer_sequence => {
                self.answering = false;
                match result {
                    Ok(options) => self.options = Some(options),
                    Err(error) => self.error = Some(error),
                }
                // The answer changes which runners resolve, and a failure may
                // come after it was saved, so the Summary checks again either
                // way to match what Create does.
                if self.stage == Stage::Details {
                    return self.check_now();
                }
            }
            _ => {}
        }
        Task::none()
    }

    /// The fetch checkbox, or a note on why there's none.
    fn art_offer_view(&self, palette: Palette, key_available: bool) -> Option<Element<'_, Event>> {
        self.art_offer(key_available)
            .view(palette, "Create", Event::FetchArt)
    }

    fn details_form(&self, palette: Palette, key_available: bool) -> Element<'_, Event> {
        let mut form = column![].spacing(10).width(Fill);
        let environment = self.draft.environment_name();
        let mut name_field = column![
            text("Name"),
            text_input(NAME_HINT, &self.draft.title).on_input(Event::Name)
        ]
        .spacing(4)
        .width(Fill);
        let mut derived = row![
            text(format!(
                "Environment: {}",
                if environment.is_empty() {
                    "(none yet)"
                } else {
                    &environment
                }
            ))
            .size(12)
            .color(palette.muted)
        ]
        .spacing(8);
        if !self.editing_environment {
            derived = derived.push(
                button(text("Edit").size(12))
                    .padding(0)
                    .on_press(Event::EditEnvironment)
                    .style(move |_, status| button::Style {
                        text_color: if matches!(status, button::Status::Hovered) {
                            palette.foreground
                        } else {
                            palette.primary
                        },
                        ..button::Style::default()
                    }),
            );
        }
        // A long environment name in a narrow dialog would squeeze Edit away.
        name_field = name_field.push(derived.wrap());
        if self.editing_environment {
            name_field = name_field.push(text("Environment name")).push(
                text_input(
                    &self.draft.environment_placeholder(),
                    self.draft
                        .environment
                        .as_deref()
                        .unwrap_or(&self.draft.derived_environment()),
                )
                .on_input(Event::EnvironmentName),
            );
        }
        form = form.push(name_field);
        if !environment.is_empty() && self.paths.environment_config(&environment).exists() {
            form = form.push(
                ui::secondary_button("Open existing environment settings", palette)
                    .on_press(Event::OpenExisting),
            );
        }
        if self.draft.source != Source::Empty {
            let hint = if self.draft.source == Source::Installer {
                "Installer path"
            } else {
                "Executable path"
            };
            form = form.push(
                row![
                    text_input(hint, &self.draft.path).on_input(Event::Path),
                    ui::secondary_button("Browse...", palette).on_press(Event::Browse),
                ]
                .spacing(8),
            );
            if self.draft.source == Source::Executable {
                form = form.push(
                    checkbox(self.draft.desktop)
                        .label("Create desktop entry")
                        .on_toggle(Event::Desktop),
                );
            }
        }
        if self.draft.kind == Kind::Windows {
            form = form.push(
                column![
                    text("Template"),
                    pick_list(
                        std::iter::once("None".to_owned())
                            .chain(
                                self.options
                                    .as_ref()
                                    .into_iter()
                                    .flat_map(|o| o.templates.iter().cloned()),
                            )
                            .collect::<Vec<_>>(),
                        Some(if self.draft.template.is_empty() {
                            "None".to_owned()
                        } else {
                            self.draft.template.clone()
                        }),
                        |value| {
                            Event::Template(if value == "None" {
                                String::new()
                            } else {
                                value
                            })
                        },
                    )
                    .wineo_style(palette)
                ]
                .spacing(4),
            );
            if self.offer_shown() {
                form = form.push(runner_label::offer_card(
                    palette,
                    !self.answering,
                    Event::Offer,
                ));
            }
            if let Some(template) = &self.template_preview {
                let effects = template_effects(template, &self.draft);
                let mut summary = column![text("This template sets:").size(12)].spacing(2);
                for effect in effects {
                    summary = summary.push(text(effect).size(12));
                }
                form = form.push(summary);
            }
            form = form.push(self.art_offer_view(palette, key_available));
            form = form.push(
                ui::secondary_button(
                    if self.additional_wine_options_visible() {
                        "Additional wine options (hide)"
                    } else {
                        "Additional wine options"
                    },
                    palette,
                )
                .on_press(Event::ToggleAdditionalWineOptions),
            );
            if self.additional_wine_options_visible() {
                let mut options_body = column![
                    text("Runner override"),
                    text_input("Use template or global default", &self.draft.runner)
                        .on_input(Event::Runner)
                ]
                .spacing(6);
                if let Some(options) = &self.options {
                    options_body = options_body.push(
                        pick_list(
                            std::iter::once(Pick::Default)
                                .chain(options.runners.iter().cloned().map(Pick::Runner))
                                .collect::<Vec<_>>(),
                            Some(if self.draft.runner.is_empty() {
                                Pick::Default
                            } else {
                                Pick::Runner(Choice::find(&options.runners, &self.draft.runner))
                            }),
                            |pick| {
                                Event::Runner(match pick {
                                    Pick::Default => String::new(),
                                    Pick::Runner(choice) => choice.name,
                                })
                            },
                        )
                        .wineo_style(palette)
                        .width(Fill),
                    );
                    options_body = options_body.push(
                        text(format!(
                            "Global default: {} ({})",
                            options.runner, options.arch
                        ))
                        .size(12)
                        .color(palette.muted),
                    );
                }
                options_body = options_body.push(text("Architecture override")).push(
                    pick_list(
                        [
                            ArchPick::Default,
                            ArchPick::Arch(Arch::Win64),
                            ArchPick::Arch(Arch::Win32),
                        ],
                        Some(self.draft.arch.map_or(ArchPick::Default, ArchPick::Arch)),
                        ArchPick::event,
                    )
                    .wineo_style(palette),
                );
                options_body = options_body.push(text("Prefix folder override")).push(
                    row![
                        text_input("Use default path", &self.draft.prefix).on_input(Event::Prefix),
                        ui::secondary_button("Browse...", palette).on_press(Event::BrowsePrefix)
                    ]
                    .spacing(8),
                );
                if self.draft.prefix.is_empty()
                    && let Some(options) = &self.options
                {
                    options_body = options_body.push(
                        text(format!(
                            "Default: {}",
                            options.prefix_root.join(&environment).display()
                        ))
                        .size(12)
                        .color(palette.muted),
                    );
                }
                form = form.push(container(options_body).padding(10));
            }
        } else {
            form = form.push(self.art_offer_view(palette, key_available));
        }
        form.into()
    }

    /// The last check's result at the bottom of Details, dimmed under
    /// "Checking..." while the next one runs.
    fn summary_view(&self, palette: Palette) -> Element<'_, Event> {
        // On the heading's line, so starting a check never moves the rest.
        let mut heading = row![ui::section_heading("Summary", palette)]
            .spacing(12)
            .align_y(Center);
        if self.rechecking() {
            heading = heading.push(text("Checking...").size(12).color(palette.muted));
        }
        let mut summary = column![heading].spacing(10).width(Fill);
        let Some(last) = &self.summary else {
            return summary.into();
        };
        let palette = if self.rechecking() {
            dimmed(palette)
        } else {
            palette
        };
        let review = match &last.found {
            Ok(review) => review,
            Err(problem) => {
                let (reason, color) = match problem {
                    Problem::Untouched(reason) => ((*reason).to_owned(), palette.muted),
                    Problem::Error(error) => (error.clone(), palette.danger),
                };
                return summary.push(wrapped(reason).color(color)).into();
            }
        };
        summary = summary.push(review_summary(summary_groups(&last.draft, review), palette));
        let mut notes = Vec::new();
        for note in consequences(review) {
            notes.push(wrapped(note).color(palette.warning).into());
        }
        if let Some(prerequisite) = &review.prerequisite {
            notes.push(self.prerequisite_note(prerequisite, palette));
        }
        if !notes.is_empty() {
            let mut section = column![ui::section_heading("Notes", palette)]
                .spacing(8)
                .width(Fill);
            for (index, note) in notes.into_iter().enumerate() {
                if index > 0 {
                    section = section.push(ui::separator(palette));
                }
                section = section.push(note);
            }
            summary = summary.push(ui::section(section, palette));
        }
        summary.into()
    }

    /// A missing runner or component, with its download buttons, which wait
    /// while a newer check runs.
    fn prerequisite_note(
        &self,
        prerequisite: &Prerequisite,
        palette: Palette,
    ) -> Element<'_, Event> {
        let download = |label: &'static str, provider: Provider| {
            ui::secondary_button(label, palette)
                .on_press_maybe((!self.rechecking()).then_some(Event::Download(provider)))
        };
        let mut note = column![].spacing(8).width(Fill);
        match prerequisite {
            Prerequisite::Runner(name) => {
                note = note.push(wrapped(format!("Runner {name} is unavailable. Choose a provider to download its latest release:")).color(palette.warning));
                if name != "Proton-CachyOS-Latest" {
                    note = note.push(download("Download GE-Proton", Provider::GeProton));
                }
                if name != "GE-Proton-Latest" {
                    note = note.push(download("Download Proton-CachyOS", Provider::ProtonCachyos));
                }
            }
            Prerequisite::Component { provider, version } => {
                let description = version.as_ref().map_or_else(
                    || "latest release".to_owned(),
                    |name| format!("exact build {name}"),
                );
                note = note.push(wrapped(format!("Missing {provider} {description}. Download it before creating this environment?")).color(palette.warning));
                let label = match provider {
                    Provider::Dxvk => "Download DXVK",
                    Provider::Vkd3d => "Download vkd3d",
                    Provider::GeProton => "Download GE-Proton",
                    Provider::ProtonCachyos => "Download Proton-CachyOS",
                };
                note = note.push(download(label, *provider));
            }
        }
        note.into()
    }

    /// Create, with why it's off beside it, or the notes that change what it
    /// does, so they show without scrolling to the Summary.
    fn create_strip(&self, palette: Palette) -> Element<'_, Event> {
        let checked = self.checked();
        let create = ui::accent_button("Create", palette.primary, checked.is_ok(), Event::Create);
        // Beside Create rather than wrapped under it, which keeps the strip
        // shortest in a narrow dialog.
        let mut beside = column![].spacing(4).width(Fill);
        match checked {
            // One paragraph, so the second note starts on the first one's
            // last line, which keeps the strip short in a narrow dialog.
            Ok(review) => {
                let notes = pinned_consequences(review).join(" ");
                if !notes.is_empty() {
                    beside = beside.push(wrapped(notes).size(12).color(palette.warning));
                }
            }
            Err(hold) => {
                let (reason, color) = hold_text(hold, palette);
                beside = beside.push(wrapped(cut(reason)).size(12).color(color));
            }
        }
        row![create, beside].spacing(12).align_y(Center).into()
    }

    pub(crate) fn view_with_key_availability(
        &self,
        palette: Palette,
        key_available: bool,
    ) -> Element<'_, Event> {
        let mut footer: Option<Element<'_, Event>> = None;
        let title = ui::page_title(
            match self.stage {
                Stage::Choose => "New environment",
                Stage::Details => start_of(self.draft.kind, self.draft.source)
                    .map_or("New environment", |start| start.heading),
                Stage::Working if self.downloading.is_some() => "Downloading prerequisite",
                Stage::Working if self.fetching => "Getting artwork",
                Stage::Working => "Creating environment",
                Stage::Target => "Choose a target",
                Stage::Finished => "Environment created",
            },
            palette,
        );
        let heading: Element<'_, Event> = if self.stage == Stage::Details {
            crate::ends::ends(
                title.width(Fill),
                button("Change")
                    .padding(0)
                    .style(move |_, status| crate::chips::link(palette, status))
                    .on_press(Event::Change),
                12.0,
                crate::ends::MIN_LEFT,
            )
        } else {
            title.into()
        };
        let mut body = column![heading].spacing(10).width(Fill);
        match self.stage {
            Stage::Choose => body = body.push(start_list(palette)),
            Stage::Details => {
                body = body
                    .push(self.details_form(palette, key_available))
                    .push(self.summary_view(palette));
                footer = Some(self.create_strip(palette));
            }
            Stage::Working => {
                body = body.push(text(&self.status));
                if self.fetching {
                    let skipping = self.fetch_token.as_ref().is_some_and(Token::is_canceled);
                    body = body.push(
                        ui::secondary_button("Skip", palette)
                            .on_press_maybe((!skipping).then_some(Event::SkipFetch)),
                    );
                // Only creation and downloads set a token, so only they have
                // anything to cancel. Force stop only makes sense after a
                // cancel, like the other flows.
                } else if let Some(token) = &self.token {
                    body = body.push(if token.is_canceled() {
                        ui::secondary_button("Force stop", palette).on_press(Event::Force)
                    } else {
                        ui::secondary_button("Cancel", palette).on_press(Event::Cancel)
                    });
                }
                if !self.log.is_empty() {
                    body = body.push(logview::controls(
                        &self.log_view,
                        true,
                        Event::CopyLog,
                        Event::LogFollow,
                        palette,
                    ));
                    body = body.push(logview::editor(
                        &self.log_view,
                        palette,
                        220,
                        LOG_ID,
                        Event::LogAction,
                        Event::LogScrolled,
                    ));
                }
            }
            Stage::Target => {
                body = body.push(text(
                    "Choose the program to run. The installer itself is not a target.",
                ));
                if let Some(report) = &self.report
                    && !report.programs.is_empty()
                {
                    body = body.push(ui::list_group(
                        PROGRAM_LIST,
                        report.programs.iter().map(|(label, exe)| {
                            ui::ListRow::new(
                                program_id(exe),
                                ui::list_row(
                                    ui::list_row_text(format!("{label} - {exe}")),
                                    palette,
                                ),
                                Event::Program(exe.clone()),
                            )
                        }),
                        palette,
                    ));
                }
                body = body.push(
                    row![
                        text_input("Executable path", &self.target_path)
                            .on_input(Event::TargetPath),
                        ui::secondary_button("Browse...", palette).on_press(Event::Browse)
                    ]
                    .spacing(8),
                );
                body = body.push(
                    checkbox(self.target_desktop)
                        .label("Create desktop entry")
                        .on_toggle(Event::TargetDesktop),
                );
                footer = Some(
                    row![
                        ui::accent_base("Confirm", palette.primary).on_press(Event::AddTarget),
                        button("Skip for now")
                            .style(move |_, status| crate::chips::link(palette, status))
                            .on_press(Event::SkipTarget),
                    ]
                    .spacing(16)
                    .align_y(Center)
                    .into(),
                );
            }
            Stage::Finished => {
                if let Some(report) = &self.report {
                    if report.registered {
                        body = body.push(text(format!(
                            "{} is in your library.",
                            self.draft
                                .target_title()
                                .unwrap_or_else(|| self.draft.environment_name())
                        )));
                    } else {
                        body = body.push(
                            text("Creation did not register an environment.").color(palette.danger),
                        );
                    }
                    for note in &report.notes {
                        body = body.push(text(note));
                    }
                    if !report.cleanup_resolved {
                        body = body.push(
                            ui::secondary_button("Clean up installer", palette)
                                .on_press(Event::CleanUp),
                        );
                    }
                    if let Some(error) = &report.art_error {
                        body = body.push(text(format!("Artwork: {error}")).color(palette.warning));
                        body = body.push(
                            ui::secondary_button("Retry artwork", palette)
                                .on_press(Event::RetryArt),
                        );
                        body = body.push(
                            ui::secondary_button("Skip artwork", palette).on_press(Event::SkipArt),
                        );
                    }
                    if report.registered && report.playable && report.cleanup_resolved {
                        body = body.push(
                            checkbox(self.launch)
                                .label("Launch it when this page closes")
                                .on_toggle(Event::Launch),
                        );
                    }
                    if report.registered {
                        body = body.push(
                            row![
                                ui::secondary_button("Open settings", palette)
                                    .on_press(Event::OpenSettings),
                                ui::secondary_button("Choose artwork...", palette)
                                    .on_press(Event::ChooseArt),
                                ui::secondary_button("Back to library", palette)
                                    .on_press(Event::ToLibrary)
                            ]
                            .spacing(8)
                            .wrap()
                            .vertical_spacing(8),
                        );
                    } else {
                        body = body.push(
                            ui::secondary_button("Back to details", palette)
                                .on_press(Event::BackToDetails),
                        );
                        body = body.push(
                            ui::secondary_button("Back to library", palette)
                                .on_press(Event::ToLibrary),
                        );
                    }
                }
            }
        }
        if let Some(error) = &self.error
            && self.stage != Stage::Choose
        {
            body = body.push(wrapped(error).color(palette.danger));
        }
        let max_width = if self.stage == Stage::Details {
            900.0
        } else {
            frame::PAGE_MAX_WIDTH
        };
        let mut page = column![].spacing(12).max_width(max_width).height(Fill);
        // Back and the discard prompt share one slot ahead of the scroll body,
        // so showing the prompt never shifts the body's place in the widget tree
        // (iced matches a column's children by position) and its scroll offset
        // and focus survive. The prompt sits above Back, outside the scroll, so
        // it is never below the fold of a long form.
        if matches!(self.stage, Stage::Choose | Stage::Details | Stage::Target) {
            let mut top = column![].spacing(12);
            if self.leave_prompt {
                top = top.push(
                    column![
                        text("Discard this new environment draft?"),
                        row![
                            ui::secondary_button("Keep editing", palette)
                                .on_press(Event::KeepEditing),
                            ui::secondary_button("Discard", palette).on_press(Event::Discard)
                        ]
                        .spacing(8)
                    ]
                    .spacing(8),
                );
            }
            top = top.push(row![
                ui::secondary_button("Back", palette).on_press(Event::Back)
            ]);
            page = page.push(top);
        }
        page = page.push(frame::scroll_body(body, None, palette));
        if let Some(footer) = footer {
            page = page.push(frame::pinned(footer, palette));
        }
        frame::page_frame(page)
    }
}

/// Ctrl+Enter on the choice page opens the row that has focus, if one does.
fn open_focused_start() -> Task<Event> {
    use iced::advanced::widget::{operate, operation::focusable::find_focused};
    operate(find_focused()).then(|id| {
        STARTS
            .iter()
            .find(|start| iced::widget::Id::new(start.id) == id)
            .map_or_else(Task::none, |start| {
                Task::done(Event::Start(start.kind, start.source))
            })
    })
}

/// The choice page's rows, under their kinds, in one surface.
fn start_list(palette: Palette) -> Element<'static, Event> {
    let rows = |kind: Kind| -> Vec<ui::ListRow<'static, Event>> {
        STARTS
            .iter()
            .enumerate()
            .filter(|(_, start)| start.kind == kind)
            .map(|(index, start)| {
                ui::ListRow::new(
                    start.id,
                    ui::list_row(
                        column![
                            ui::list_row_text(format!("{} - {}", index + 1, start.title)),
                            text(start.detail).size(12).color(palette.muted)
                        ]
                        .spacing(2),
                        palette,
                    ),
                    Event::Start(start.kind, start.source),
                )
            })
            .collect()
    };
    ui::labeled_list_group(
        START_LIST,
        Kind::ALL.map(|kind| (kind.to_string(), rows(kind))),
        palette,
    )
}

fn review_summary(
    groups: Vec<(&'static str, Vec<(&'static str, String)>)>,
    palette: Palette,
) -> Element<'static, Event> {
    let mut content = column![].spacing(8).width(Fill);
    for (group_index, (title, fields)) in groups.into_iter().enumerate() {
        if group_index > 0 {
            content = content.push(ui::separator(palette));
        }
        content = content.push(ui::section_heading(title, palette));
        for (field_index, (label, value)) in fields.into_iter().enumerate() {
            if field_index > 0 {
                content = content.push(ui::separator(palette));
            }
            content = content.push(review_field(label, value, palette));
        }
    }
    ui::section(content, palette).into()
}

fn review_field(label: &'static str, value: String, palette: Palette) -> Element<'static, Event> {
    column![
        text(label).size(12).color(palette.muted),
        text(value)
            .size(14)
            .color(palette.foreground)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
    ]
    .spacing(2)
    .width(Fill)
    .into()
}

/// The Summary's rows: the draft as it was checked, and what the check found.
fn summary_groups(
    draft: &Draft,
    review: &Review,
) -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    let mut application = vec![("Environment", draft.environment_name())];
    if let Some(title) = draft.target_title() {
        application.insert(0, ("Name", title));
    }
    let mut source = vec![
        ("Type", draft.kind.to_string()),
        ("Starting point", draft.source.to_string()),
    ];
    if !draft.path.is_empty() {
        source.push(("Program", draft.path.clone()));
    }
    let mut groups = vec![("Application", application), ("Source", source)];
    if draft.kind == Kind::Windows {
        let mut runtime = Vec::new();
        if let Some(row) = &review.runner_row {
            runtime.push(("Runner", row.clone()));
        } else if let Some(Prerequisite::Runner(name)) = &review.prerequisite {
            runtime.push(("Runner", format!("{name} (not installed)")));
        }
        runtime.push(("Architecture", review.arch.to_string()));
        if let Some(prefix) = &review.prefix {
            runtime.push(("Prefix", prefix.display().to_string()));
        }
        groups.push(("Runtime", runtime));
    }
    groups
}

/// The notes that change what Create does, as the Summary says them.
fn consequences(review: &Review) -> Vec<&'static str> {
    let mut notes = Vec::new();
    if review.adopted {
        notes.push("This compatible existing prefix will be registered in place.");
    }
    if review.omitted_components {
        notes.push("This Proton runner bundles DXVK and vkd3d-proton, so the template's component picks will be omitted.");
    }
    notes
}

/// `consequences`, shorter, for the strip beside Create.
fn pinned_consequences(review: &Review) -> Vec<&'static str> {
    let mut notes = Vec::new();
    if review.adopted {
        notes.push("This will register the existing prefix in place.");
    }
    if review.omitted_components {
        notes.push("Proton's own DXVK and vkd3d-proton replace the template's.");
    }
    notes
}

/// Why Create is off, as the strip says it: quiet while checking or for a
/// field not touched yet, a warning for a download, danger for an error.
fn hold_text(hold: Hold, palette: Palette) -> (String, iced::Color) {
    match hold {
        Hold::Checking => ("Checking...".to_owned(), palette.muted),
        Hold::Problem(Problem::Untouched(reason)) => (reason.to_owned(), palette.muted),
        Hold::Problem(Problem::Error(error)) => (error, palette.danger),
        Hold::Missing(reason) => (reason, palette.warning),
    }
}

/// `reason`, cut to `PINNED_REASON_MAX` characters with "..." when longer.
fn cut(reason: String) -> String {
    match reason.char_indices().nth(PINNED_REASON_MAX) {
        Some((end, _)) => format!("{}...", reason[..end].trim_end()),
        None => reason,
    }
}

/// Text that breaks anywhere, so a long path in a message never spills.
fn wrapped<'a>(message: impl text::IntoFragment<'a>) -> iced::widget::Text<'a> {
    text(message).wrapping(iced::widget::text::Wrapping::WordOrGlyph)
}

/// The Summary's colors while it waits on a newer check: every text role
/// reads as muted, which keeps its contrast in every theme.
fn dimmed(palette: Palette) -> Palette {
    Palette {
        foreground: palette.muted,
        warning: palette.muted,
        danger: palette.muted,
        ..palette
    }
}

/// An architecture picker entry: the template or global default, or one set
/// by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchPick {
    Default,
    Arch(Arch),
}

impl ArchPick {
    fn event(self) -> Event {
        match self {
            Self::Default => Event::ArchDefault,
            Self::Arch(arch) => Event::Arch(arch),
        }
    }
}

impl std::fmt::Display for ArchPick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => f.write_str("Use template or global default"),
            Self::Arch(arch) => arch.fmt(f),
        }
    }
}

/// A picker entry: the template or global default, or a named runner.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pick {
    Default,
    Runner(Choice),
}

impl std::fmt::Display for Pick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => f.write_str("Use template or global default"),
            Self::Runner(choice) => choice.fmt(f),
        }
    }
}

fn read_options(paths: &Paths) -> Result<Options, String> {
    let global = GlobalConfig::load(paths).map_err(|e| e.to_string())?;
    let available = Runners::new(paths, &global).map_err(|e| e.to_string())?;
    let mut runners = vec![
        Choice::plain(global.defaults.runner.clone()),
        Choice::plain(Runner::SYSTEM),
    ];
    runners.extend(
        available
            .list()
            .map_err(|e| e.to_string())?
            .runners
            .iter()
            .map(|runner| Choice::of(paths, runner)),
    );
    // A listed build knows its launcher, so it wins over the plain default.
    runners.sort_by(|a, b| a.name.cmp(&b.name).then(b.launcher.cmp(&a.launcher)));
    runners.dedup_by(|a, b| a.name == b.name);
    let templates = Templates::new(paths)
        .list()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    Ok(Options {
        fetch_kinds: !global.art.fetch.kinds().is_empty(),
        prefix_root: global.prefix_root(paths).map_err(|e| e.to_string())?,
        offer: runner_label::offer(paths, &global),
        runner: global.defaults.runner,
        templates,
        runners,
        arch: global.defaults.arch,
    })
}

fn answer_offer(paths: &Paths, on: bool) -> Result<Options, String> {
    runner_label::answer(paths, on)?;
    read_options(paths)
}

fn template_effects(template: &Template, draft: &Draft) -> Vec<String> {
    let mut effects = Vec::new();
    if !draft.runner.is_empty() {
        effects.push(format!("Runner: {} (your override)", draft.runner));
    } else if let Some(runner) = &template.runner {
        let label = if runner == "proton" {
            "newest installed Proton, or the global default".to_owned()
        } else {
            runner.clone()
        };
        effects.push(format!("Runner: {label}"));
    }
    if draft.arch.is_none()
        && let Some(arch) = template.arch
    {
        effects.push(format!("Architecture: {arch}"));
    }
    if template.dxvk.is_some() || template.vkd3d.is_some() {
        let mut components = Vec::new();
        if let Some(dxvk) = &template.dxvk {
            components.push(format!("DXVK {dxvk}"));
        }
        if let Some(vkd3d) = &template.vkd3d {
            components.push(format!("vkd3d {vkd3d}"));
        }
        effects.push(format!(
            "Graphics: {} (Proton uses its bundled versions)",
            components.join(" + ")
        ));
    }
    for (label, enabled) in [
        ("GameMode", template.gamemode),
        ("Wayland", template.wayland),
        ("MangoHud", template.mangohud),
    ] {
        if let Some(enabled) = enabled {
            effects.push(format!("{label}: {}", if enabled { "on" } else { "off" }));
        }
    }
    if let Some(gamescope) = &template.gamescope {
        effects.push(format!(
            "Gamescope: {}",
            if matches!(gamescope, GamescopeSetting::Off) {
                "off"
            } else {
                "on"
            }
        ));
    }
    if let Some(hooks) = template
        .hooks
        .as_ref()
        .and_then(wineo_core::config::HooksSetting::on)
    {
        let phases: Vec<_> = [("before", &hooks.before), ("after", &hooks.after)]
            .into_iter()
            .filter_map(|(phase, command)| command.as_ref().map(|_| phase))
            .collect();
        if !phases.is_empty() {
            effects.push(format!("Hooks: {}", phases.join(", ")));
        }
    }
    for (label, value) in [("Game ID", &template.gameid), ("Store", &template.store)] {
        if let Some(value) = value {
            effects.push(format!("{label}: {value}"));
        }
    }
    if !template.winetricks.is_empty() {
        effects.push(format!("Winetricks: {}", template.winetricks.join(", ")));
    }
    if !template.tags.is_empty() {
        effects.push(format!("Tags: {}", template.tags.join(", ")));
    }
    if !template.env.is_empty() {
        effects.push(format!(
            "Environment variables: {}",
            template.env.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    effects
}

fn snap_log() -> Task<Event> {
    logview::snap_to_end(LOG_ID)
}

fn prepare(paths: &Paths, draft: &Draft) -> Result<Review, String> {
    let global = GlobalConfig::load(paths).map_err(|error| error.to_string())?;
    let name = draft.environment_name();
    let name = name.as_str();
    if paths.environment_config(name).exists() {
        return Err(format!(
            "An environment named {name} already exists. Open its Settings from the library."
        ));
    }
    if draft.kind == Kind::Native {
        if draft.source == Source::Executable {
            launch::check_native_exe(paths, &global, name, &Default::default(), &draft.path)
                .map_err(|error| error.to_string())?;
        }
        return Ok(Review {
            runner: None,
            runner_row: None,
            template: None,
            arch: global.defaults.arch,
            prefix: None,
            adopted: false,
            omitted_components: false,
            prerequisite: None,
        });
    }
    if draft.source == Source::Installer {
        launch::check_program_path(paths, &draft.path).map_err(|error| error.to_string())?;
        let lower = draft.path.to_ascii_lowercase();
        if !lower.ends_with(".exe") && !lower.ends_with(".msi") {
            return Err("Choose an .exe or .msi installer".to_owned());
        }
    }
    let template = if draft.template.is_empty() {
        None
    } else {
        Some(
            Templates::new(paths)
                .resolve(&draft.template)
                .map_err(|error| error.to_string())?,
        )
    };
    let runners = Runners::new(paths, &global).map_err(|error| error.to_string())?;
    let runner_name = if !draft.runner.is_empty() {
        draft.runner.clone()
    } else if let Some(template) = &template {
        template
            .runner_name(&runners, &global.defaults.runner)
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| global.defaults.runner.clone())
    } else {
        global.defaults.runner.clone()
    };
    // A missing runner is one more note on the same check. Every download
    // offered for it installs a Proton build, so it's treated as Proton for
    // the architecture, the prefix layout, and the omitted components. That
    // keeps the Review from changing after a download except for the runner.
    let runner = match runners.resolve(&runner_name) {
        Ok(runner) => Some(runner),
        Err(error)
            if matches!(
                &error,
                CoreError::UnknownRunner { .. } | CoreError::ProgramNotFound("wine" | "wineserver")
            ) || matches!(&error, CoreError::RunnerAlias { reason, .. } if reason.starts_with("no compatible stable release")) =>
        {
            None
        }
        Err(error) => return Err(error.to_string()),
    };
    let proton = runner.as_ref().is_none_or(Runner::is_proton);
    let arch = if proton {
        Arch::Win64
    } else {
        draft
            .arch
            .or_else(|| template.as_ref().and_then(|template| template.arch))
            .unwrap_or(global.defaults.arch)
    };
    if let Some(runner) = &runner {
        create::check_arch(runner, Some(arch)).map_err(|error| error.to_string())?;
    }
    let prefix = NewEnvironment {
        name: name.to_owned(),
        dir: (!draft.prefix.is_empty()).then(|| draft.prefix.clone()),
        arch: Some(arch),
        template: template.clone(),
        gog_id: None,
        title: None,
    }
    .prefix_dir(paths, &global)
    .map_err(|error| error.to_string())?;
    let actual = match &runner {
        Some(runner) => runner.wine_prefix(&prefix),
        None => prefix.join("pfx"),
    };
    if draft.source == Source::Installer {
        let installer = if wineo_core::winpath::is_windows_path(&draft.path) {
            wineo_core::winpath::to_unix(&actual, &draft.path)
        } else {
            Some(
                paths
                    .absolute(&draft.path)
                    .map_err(|error| error.to_string())?,
            )
        };
        if !installer.as_ref().is_some_and(|path| path.is_file()) {
            return Err(format!("Installer {} was not found", draft.path));
        }
    }
    let adopted = wineo_core::prefix::is_prefix(&actual);
    if prefix.exists() && !adopted {
        let occupied = fs::read_dir(&prefix)
            .map_err(|error| error.to_string())?
            .next()
            .is_some();
        if occupied {
            return Err(format!(
                "{} is occupied and is not a compatible prefix",
                prefix.display()
            ));
        }
    }
    let omitted_components = template.as_ref().is_some_and(|template| match &runner {
        Some(runner) => template.for_runner(runner).1,
        None => template.dxvk.is_some() || template.vkd3d.is_some(),
    });
    let prerequisite = match &runner {
        None => Some(Prerequisite::Runner(runner_name)),
        Some(runner) => match &template {
            Some(template) => {
                let (template, _) = template.for_runner(runner);
                let wanted = Wanted {
                    dxvk: template.dxvk,
                    vkd3d: template.vkd3d,
                };
                match install::check_available(paths, &global, name, runner, &wanted) {
                    Ok(()) => None,
                    Err(CoreError::NoBuilds { component, .. }) => Some(Prerequisite::Component {
                        provider: component_provider(component)?,
                        version: None,
                    }),
                    Err(CoreError::UnknownBuild {
                        component, name, ..
                    }) => Some(Prerequisite::Component {
                        provider: component_provider(component)?,
                        version: Some(name),
                    }),
                    Err(error) => return Err(error.to_string()),
                }
            }
            None => None,
        },
    };
    Ok(Review {
        runner_row: runner
            .as_ref()
            .map(|runner| runner_label::resolved(paths, runner)),
        runner,
        template,
        arch,
        prefix: Some(prefix),
        adopted,
        omitted_components,
        prerequisite,
    })
}

fn component_provider(component: &str) -> Result<Provider, String> {
    match component {
        "dxvk" => Ok(Provider::Dxvk),
        "vkd3d" => Ok(Provider::Vkd3d),
        _ => Err(format!("No download provider for {component}")),
    }
}

fn create_request(draft: &Draft, review: Review, runner: Runner) -> CreateRequest {
    CreateRequest {
        name: draft.environment_name(),
        dir: review
            .prefix
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        resolved_runner: Some(runner),
        arch: Some(review.arch),
        template: review.template,
        exe: (draft.source == Source::Executable).then(|| draft.path.clone()),
        target_name: draft.target_title(),
        title: draft.target_title(),
        desktop: draft.desktop,
        expected_adopted: Some(review.adopted),
        ..CreateRequest::default()
    }
}

/// The picked installer program is the environment's only target, so it takes
/// the game title for its desktop entry.
fn target_spec(exe: String, title: Option<String>, desktop: bool) -> Spec {
    Spec {
        exe,
        name: title,
        desktop,
        ..Spec::default()
    }
}

fn perform(
    paths: &Paths,
    draft: Draft,
    review: Review,
    token: Token,
    mut progress: impl FnMut(String),
    mut log_output: impl FnMut(String),
) -> Report {
    let mut report = Report::new();
    let name = draft.environment_name();
    let name = name.as_str();
    if draft.kind == Kind::Native {
        let global = match GlobalConfig::load(paths) {
            Ok(global) => global,
            Err(error) => {
                report.notes.push(error.to_string());
                return report;
            }
        };
        let exe = (draft.source == Source::Executable).then_some(draft.path.as_str());
        match Environment::create_native_titled(
            paths,
            &global,
            name,
            exe,
            draft.target_title().as_deref(),
            draft.desktop,
            |warning| report.notes.push(warning),
        ) {
            Ok(environment) => {
                report.registered = true;
                report.playable = exe.is_some();
                progress("Native environment registered".to_owned());
                if let Err(error) = desktop::reconcile_native(paths, &global, &environment) {
                    report.notes.push(error.to_string());
                }
            }
            Err(error) => report.notes.push(error.to_string()),
        }
        return report;
    }
    let Some(runner) = review.runner.clone() else {
        report.notes.push("Missing resolved runner".to_owned());
        return report;
    };
    let request = create_request(&draft, review, runner.clone());
    let log = Runtime::new(paths, name).install_log_path();
    let step = (draft.source == Source::Installer).then(|| create::Step::Installer {
        program: &draft.path,
        output: Output::Log(&log),
        token: &token,
    });
    let mut hooks = Hooks {
        paths,
        report: &mut report,
        progress: &mut progress,
        log_output: &mut log_output,
        log: &log,
        log_offset: 0,
    };
    let created = create::create_and_install(paths, request, step, &token, &mut hooks);
    let mut log_offset = hooks.log_offset;
    let created = match created {
        Ok(created) => created,
        Err(error) => {
            report.registered = paths.environment_config(name).is_file();
            report.notes.push(error.to_string());
            return report;
        }
    };
    let (outcome, resolved) = match created.step {
        Stepped::None => return report,
        Stepped::Stopped(note) => {
            report.notes.push(note);
            return report;
        }
        Stepped::NotPrepared(error) => {
            report
                .notes
                .push(format!("Installer could not start: {error}"));
            return report;
        }
        Stepped::Ran { outcome, resolved } => (outcome, resolved),
    };
    pump_log(&log, &mut log_offset, &mut log_output);
    let next = create::after_install(&outcome, resolved);
    match outcome {
        Ok(Outcome::Finished { exit_code: Some(0) }) => {
            report.notes.push("Installed successfully".to_owned());
        }
        Ok(Outcome::Finished {
            exit_code: Some(code),
        }) => report
            .notes
            .push(format!("Installer reported an error (code {code})")),
        Ok(Outcome::Finished { exit_code: None }) => report
            .notes
            .push("Installer finished, but its result is unknown".to_owned()),
        // After a cancel the code only reflects how it was stopped.
        Ok(Outcome::Canceled { .. }) => report
            .notes
            .push("Installer canceled. Add a target in Settings.".to_owned()),
        Ok(Outcome::Unresolved { error, .. }) => report.notes.push(format!(
            "Programs the installer started may still be running: {error}"
        )),
        Err(error) => report.notes.push(format!("Installer failed: {error}")),
    }
    if next == AfterInstall::Unresolved {
        report.cleanup_resolved = false;
    }
    if next == AfterInstall::Programs {
        report.offer_programs = true;
        match programs::scan(&runner.wine_prefix(&created.prefix)) {
            Ok(listing) => {
                report.programs = listing
                    .programs
                    .into_iter()
                    .map(|program| (program.name, program.exe))
                    .collect()
            }
            Err(error) => report.notes.push(format!("Program scan: {error}")),
        }
    }
    report
}

/// The desktop app's side of creating a Windows environment, and of its
/// installer.
struct Hooks<'a, P, L> {
    paths: &'a Paths,
    report: &'a mut Report,
    progress: &'a mut P,
    log_output: &'a mut L,
    log: &'a std::path::Path,
    log_offset: usize,
}

impl<P: FnMut(String), L: FnMut(String)> create::Hooks for Hooks<'_, P, L> {
    /// A note for the report.
    type Stop = String;

    fn event(&mut self, event: CreateEvent) {
        let progress = &mut self.progress;
        match event {
            CreateEvent::Warning(warning) => self.report.notes.push(warning),
            CreateEvent::BuildingPrefix { dir, .. } => {
                progress(format!("Building prefix in {}", dir.display()))
            }
            CreateEvent::Saved => progress("Environment registered".to_owned()),
            CreateEvent::TemplateComponentsOmitted { .. } => {
                progress("Template components bundled by Proton were omitted".to_owned())
            }
            CreateEvent::Components(changes) => {
                progress(format!("Applied {} component changes", changes.len()))
            }
            CreateEvent::Desktop(changes) => {
                progress(format!("Applied {} desktop changes", changes.len()))
            }
            CreateEvent::Registered { .. } => progress("Default target checked".to_owned()),
        }
    }

    fn created(&mut self, created: &create::Created) -> Result<(), String> {
        let report = &mut *self.report;
        report.registered = true;
        report.playable = created.target == TargetOutcome::Added;
        if created.target == TargetOutcome::ExeNotFound {
            report
                .notes
                .push("The executable was not found. Add a target in Settings.".to_owned());
        }
        if let Err(error) = desktop::reconcile(
            self.paths,
            &created.global,
            &created.environment,
            Some(&created.runner),
            &created.prefix,
        ) {
            report.notes.push(format!("Desktop entry: {error}"));
        }
        Ok(())
    }

    fn ready(&mut self, _launch: &launch::Launch) -> Result<(), String> {
        if let Some(parent) = self.log.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        (self.progress)("Running installer...".to_owned());
        Ok(())
    }

    fn watched(&mut self, watched: Watched) {
        pump_log(self.log, &mut self.log_offset, &mut self.log_output);
        let progress = &mut self.progress;
        match watched {
            Watched::Notice(Notice::Stopping) => progress("Canceling installer...".to_owned()),
            Watched::Notice(Notice::Killing) => progress("Force stopping installer...".to_owned()),
            Watched::Notice(Notice::Warning(error)) => {
                progress(format!("Installer warning: {error}"))
            }
            Watched::Notice(Notice::Heartbeat) | Watched::Exited(_) => {}
        }
    }
}

fn pump_log(path: &std::path::Path, offset: &mut usize, output: &mut impl FnMut(String)) {
    if let Ok(bytes) = fs::read(path) {
        if bytes.len() < *offset {
            *offset = 0;
        }
        if bytes.len() > *offset {
            output(String::from_utf8_lossy(&bytes[*offset..]).into_owned());
            *offset = bytes.len();
        }
    }
}

#[cfg(test)]
mod tests;
