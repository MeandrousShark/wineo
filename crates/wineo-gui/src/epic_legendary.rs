//! Legendary in the desktop app: finding it without ever asking, the line
//! Settings shows about it, and the offer to download wineo's own copy that
//! Log in, an install, and an update make when there is none to run, in the
//! terminal's words.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc::unbounded;
use iced::widget::{Space, column, row, text};
use iced::{Center, Element, Fill, Task};
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::epic::legendary::{self, Legendary, Missing, Source};
use wineo_core::epic::{self, own_copy};
use wineo_core::paths::Paths;

use crate::theme::Palette;
use crate::ui;

const NO_ID: &str = "epic-download-no";
const YES_ID: &str = "epic-download-yes";

/// How often the download's count may change on screen.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// What looking for Legendary found.
pub(crate) enum Found {
    Usable(Box<Legendary>),
    /// None to run, and wineo's own copy can be downloaded here.
    Offer(Missing),
    /// None to run, and none can be downloaded: no new enough Python, or
    /// another machine. Says why.
    Unusable(String),
}

impl std::fmt::Debug for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usable(legendary) => f.debug_tuple("Usable").field(legendary).finish(),
            Self::Offer(missing) => f.debug_tuple("Offer").field(missing).finish(),
            Self::Unusable(why) => f.debug_tuple("Unusable").field(why).finish(),
        }
    }
}

/// Looks for Legendary, never asking to download it. Nothing is offered
/// that couldn't run: on another machine, or without a new enough Python,
/// it is `Unusable`, as the terminal never asks then either. An
/// `[epic] legendary` that can't be used is the error.
pub(crate) fn find(paths: &Paths, token: &Token) -> Result<Found, String> {
    let global = GlobalConfig::load(paths).map_err(|error| format!("{error:#}"))?;
    // A test names its fake, so the machine's own Legendary never runs.
    if cfg!(test) && global.epic.legendary.is_none() {
        return Ok(Found::Unusable(
            "A test must set [epic] legendary.".to_owned(),
        ));
    }
    match legendary::find(paths, &global, token) {
        Ok(legendary::Found::Usable(legendary)) => Ok(Found::Usable(legendary)),
        Ok(legendary::Found::Missing(missing)) => {
            if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
                return Ok(Found::Unusable(epic::Error::Unsupported.to_string()));
            }
            Ok(offerable(missing, legendary::python(token)))
        }
        Err(error @ epic::Error::Python) => Ok(Found::Unusable(error.to_string())),
        Err(error) => Err(error.to_string()),
    }
}

/// The offer, when Python can run what it downloads.
fn offerable(missing: Missing, python: Result<PathBuf, epic::Error>) -> Found {
    match python {
        Ok(_) => Found::Offer(missing),
        Err(error) => Found::Unusable(error.to_string()),
    }
}

/// Which Legendary is in use and its version, for Settings.
pub(crate) fn in_use(paths: &Paths, legendary: &Legendary) -> String {
    let version = legendary.version();
    match legendary.source() {
        Source::Own => format!("Legendary {version}, downloaded by wineo"),
        Source::Setting => format!(
            "Legendary {version}, from [epic] legendary ({})",
            shown(paths, legendary.path())
        ),
        Source::Path => format!(
            "Legendary {version}, on PATH ({})",
            shown(paths, legendary.path())
        ),
    }
}

fn shown(paths: &Paths, path: &Path) -> String {
    paths.contract_text(&path.display().to_string())
}

/// The Legendary calls a page started and is waiting on, like a listing or
/// a review asking `info`. Each is canceled when the page moves on or goes
/// away, so it gives up the host-wide Epic lock, and each counts as work
/// until its answer lands, so closing the window waits for Legendary to
/// stop rather than leaving it running.
#[derive(Default)]
pub(crate) struct Jobs(Vec<(u64, Token)>);

impl Jobs {
    /// A token for the call answering with `generation`. Calls started
    /// before it are canceled, and still count until they answer.
    pub(crate) fn start(&mut self, generation: u64) -> Token {
        self.cancel();
        let token = Token::new();
        self.0.push((generation, token.clone()));
        token
    }

    pub(crate) fn cancel(&self) {
        for (_, token) in &self.0 {
            token.cancel();
        }
    }

    /// The call that answers with `generation` has stopped.
    pub(crate) fn ended(&mut self, generation: u64) {
        self.0.retain(|(started, _)| *started != generation);
    }

    pub(crate) fn running(&self) -> bool {
        !self.0.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn all_canceled(&self) -> bool {
        self.0.iter().all(|(_, token)| token.is_canceled())
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Where the offer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Asking,
    Downloading { received: u64, size: u64 },
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// Yes: download it.
    Download,
    /// No, which cancels what needed it.
    Decline,
    /// Stops a download under way.
    Cancel,
    Progress(u64, u64, u64),
    Done(u64, Result<(), Failure>),
}

/// Why a download didn't end with Legendary in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    Canceled,
    Failed(String),
}

/// What the host does once the offer has its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Still asking or downloading.
    Pending,
    /// No, or a canceled download: what needed Legendary stops quietly.
    Declined,
    /// Downloaded, or another wineo got it first: look for it again.
    Ready,
    Failed(String),
}

/// The question and then the download, which a page shows in place of what
/// needed Legendary.
pub(crate) struct Offer {
    older: Option<String>,
    stage: Stage,
    token: Option<Token>,
    generation: u64,
}

impl Offer {
    pub(crate) fn new(missing: &Missing) -> Self {
        Self {
            older: missing.older_line(),
            stage: Stage::Asking,
            token: None,
            generation: 0,
        }
    }

    pub(crate) fn stage(&self) -> Stage {
        self.stage
    }

    #[cfg(test)]
    pub(crate) fn canceled(&self) -> bool {
        self.token.as_ref().is_some_and(Token::is_canceled)
    }

    /// A download is running, which leaving the page has to wait for.
    pub(crate) fn downloading(&self) -> bool {
        self.token.is_some()
    }

    pub(crate) fn cancel(&mut self) {
        if let Some(token) = &self.token {
            token.cancel();
        }
    }

    /// Handles `event`, with the task that downloads when it starts one.
    pub(crate) fn update(&mut self, paths: &Paths, event: Event) -> (Task<Event>, Outcome) {
        match event {
            Event::Download if self.stage == Stage::Asking => {
                self.generation = self.generation.wrapping_add(1);
                let token = Token::new();
                self.token = Some(token.clone());
                self.stage = Stage::Downloading {
                    received: 0,
                    size: own_copy::PINNED.size,
                };
                (
                    download_task(paths.clone(), token, self.generation),
                    Outcome::Pending,
                )
            }
            Event::Decline if self.stage == Stage::Asking => (Task::none(), Outcome::Declined),
            Event::Cancel => {
                self.cancel();
                (Task::none(), Outcome::Pending)
            }
            Event::Progress(generation, received, size)
                if generation == self.generation && self.token.is_some() =>
            {
                self.stage = Stage::Downloading { received, size };
                (Task::none(), Outcome::Pending)
            }
            Event::Done(generation, result) if generation == self.generation => {
                self.token = None;
                self.stage = Stage::Asking;
                let outcome = match result {
                    Ok(()) => Outcome::Ready,
                    Err(Failure::Canceled) => Outcome::Declined,
                    Err(Failure::Failed(message)) => Outcome::Failed(message),
                };
                (Task::none(), outcome)
            }
            _ => (Task::none(), Outcome::Pending),
        }
    }

    pub(crate) fn view<'a>(&'a self, palette: Palette) -> Element<'a, Event> {
        let mut content = column![].spacing(12);
        match self.stage {
            Stage::Asking => {
                if let Some(older) = &self.older {
                    content = content.push(text(older).size(13).color(palette.muted));
                }
                content = content.push(
                    text(own_copy::question())
                        .size(14)
                        .color(palette.foreground),
                );
                content = content.push(
                    row![
                        Space::new().width(Fill),
                        crate::focus_button::answer(
                            ui::secondary_button("No", palette)
                                .on_press(Event::Decline)
                                .into(),
                            true,
                            NO_ID,
                            Event::Decline,
                        ),
                        crate::focus_button::answer(
                            ui::accent_button("Yes", palette.primary, true, Event::Download),
                            true,
                            YES_ID,
                            Event::Download,
                        ),
                    ]
                    .spacing(8)
                    .align_y(Center),
                );
            }
            Stage::Downloading { received, size } => {
                let canceling = self.token.as_ref().is_some_and(Token::is_canceled);
                content = content.push(crate::progress::bar(
                    usize::try_from(received).unwrap_or(usize::MAX),
                    usize::try_from(size).unwrap_or(usize::MAX),
                    progress_label(received, size, canceling),
                    palette,
                ));
                content = content.push(
                    ui::secondary_button("Cancel", palette)
                        .on_press_maybe((!canceling).then_some(Event::Cancel)),
                );
            }
        }
        ui::section(content, palette).into()
    }
}

/// The download's state in words beside its bar.
pub(crate) fn progress_label(received: u64, size: u64, canceling: bool) -> String {
    if canceling {
        return "Canceling...".to_owned();
    }
    format!(
        "Downloading Legendary {}: {} of {}",
        own_copy::PINNED.version,
        crate::size::format_bytes(received),
        crate::size::format_bytes(size)
    )
}

/// Downloads the pinned Legendary on the blocking pool, sending its count
/// at most every `PROGRESS_INTERVAL`.
fn download_task(paths: Paths, token: Token, generation: u64) -> Task<Event> {
    let (sender, receiver) = unbounded();
    let work = crate::task::blocking_value(
        move || {
            let mut last: Option<Instant> = None;
            let fetched = own_copy::get(&paths, &token, |received, size| {
                if last.is_some_and(|last| last.elapsed() < PROGRESS_INTERVAL) && received < size {
                    return;
                }
                last = Some(Instant::now());
                let _ = sender.unbounded_send(Event::Progress(generation, received, size));
            });
            match fetched {
                Ok(_) => Ok(()),
                Err(error) if error.is_canceled() || token.is_canceled() => Err(Failure::Canceled),
                Err(error) => Err(Failure::Failed(error.to_string())),
            }
        },
        move |result| {
            Event::Done(
                generation,
                result.unwrap_or_else(|error| Err(Failure::Failed(error))),
            )
        },
    );
    Task::batch([Task::run(receiver, |event| event), work])
}

#[cfg(test)]
mod tests;
