//! The Epic section: who is logged in, which Legendary is in use, logging
//! in through the browser with the code Epic's login page shows, and
//! logging out. When there is no Legendary to run, Log in first offers to
//! download wineo's own copy, in the terminal's words. The pasted code is a
//! login while it works, so it only ever reaches a secure input and the
//! login call, never a message, log, or `Debug` output.

use iced::Task;
use wineo_core::cancel::Token;
use wineo_core::epic::account::{self, LOGIN_URL, LoginCode};
use wineo_core::epic::legendary::Missing;
use wineo_core::epic::own_copy;
use wineo_core::paths::Paths;

use super::key_settings::Secret;
use crate::Message;
use crate::epic_legendary::{self as legendary, Found, Offer, Outcome};
use crate::epic_library::{Call, Calls};
use crate::task;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Busy {
    Opening,
    LoggingIn,
    LoggingOut,
    /// Downloading Legendary, after Log in asked.
    Downloading,
}

impl Busy {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Opening => "Opening Epic's login page...".to_owned(),
            Self::LoggingIn => "Logging in...".to_owned(),
            Self::LoggingOut => "Logging out...".to_owned(),
            Self::Downloading => {
                format!("Downloading Legendary {}...", own_copy::PINNED.version)
            }
        }
    }
}

/// Who is logged in, as Legendary says without asking Epic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Account {
    LoggedOut,
    LoggedIn(String),
    /// A login is saved, but there is no Legendary to say whose.
    Unread,
}

/// Which Legendary is in use, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Program {
    InUse(String),
    /// None, and Log in offers to download one.
    Missing,
    /// None, and none can be had or used, with why.
    Unusable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) account: Account,
    pub(crate) legendary: Program,
}

impl Status {
    pub(crate) fn label(&self) -> String {
        match &self.account {
            Account::LoggedOut => "Not logged in".to_owned(),
            Account::LoggedIn(name) => format!("Logged in to Epic as {name}."),
            Account::Unread => "A login is saved, but no Legendary can read it.".to_owned(),
        }
    }

    pub(crate) fn legendary_label(&self) -> &str {
        match &self.legendary {
            Program::InUse(line) | Program::Unusable(line) => line,
            Program::Missing => "Not installed. Log in offers to download it.",
        }
    }

    fn logged_in(&self) -> bool {
        matches!(self.account, Account::LoggedIn(_))
    }

    /// Whether there is a login to remove: one Legendary names, or one it
    /// can't read.
    fn saved(&self) -> bool {
        !matches!(self.account, Account::LoggedOut)
    }
}

/// What opening the login form found.
#[derive(Debug, Clone)]
pub(crate) enum Ready {
    /// Legendary is there, and the browser was asked to open the page.
    Open,
    /// There is none, so Log in asks to download it first.
    Offer(Missing),
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// Opens Epic's login page and the field for the code, or the offer to
    /// download Legendary first.
    LoginOpen,
    LoginCancel,
    LoginText(Secret),
    Login,
    Logout,
    CopyLink,
    StatusLoaded(u64, u64, Result<Status, String>),
    LoginReady(u64, u64, Result<Ready, String>),
    /// The download offer's event, with the sequence of the download it
    /// belongs to once one started.
    Offer(u64, legendary::Event),
    LoggedIn(u64, u64, Option<u64>, Result<String, String>),
    LoggedOut(u64, u64, Option<u64>, Result<String, String>),
}

impl Event {
    /// The sequence of a finished login, logout, or download, which the app
    /// hands to whichever global page is open.
    pub(crate) fn operation(&self) -> Option<u64> {
        match self {
            Self::LoggedIn(_, sequence, _, _)
            | Self::LoggedOut(_, sequence, _, _)
            | Self::Offer(sequence, legendary::Event::Done(..)) => Some(*sequence),
            _ => None,
        }
    }

    pub(crate) fn rebind(self, generation: u64) -> Self {
        match self {
            Self::LoggedIn(_, sequence, session, result) => {
                Self::LoggedIn(generation, sequence, session, result)
            }
            Self::LoggedOut(_, sequence, session, result) => {
                Self::LoggedOut(generation, sequence, session, result)
            }
            other => other,
        }
    }
}

#[derive(Default)]
pub(crate) struct Settings {
    pub(crate) status: Option<Status>,
    pub(crate) status_error: Option<String>,
    pub(crate) input_open: bool,
    pub(crate) input: Secret,
    pub(crate) busy: Option<Busy>,
    pub(crate) error: Option<String>,
    pub(crate) result: Option<String>,
    /// The question, then the download, while Log in waits on Legendary.
    pub(crate) offer: Option<Offer>,
    /// Only the latest status lands.
    pub(crate) sequence: u64,
    pub(crate) busy_sequence: Option<u64>,
    /// The open form, so a result for an older one doesn't close it.
    pub(crate) input_session: Option<u64>,
    /// Where its Legendary calls are counted, so closing the window stops
    /// them.
    pub(crate) calls: Calls,
    /// The status and login-page calls, which end with the page. Logging in
    /// and out outlive it.
    own: OwnCalls,
}

/// Tokens of the calls a page drops when it goes.
#[derive(Default)]
struct OwnCalls(Vec<Token>);

impl OwnCalls {
    fn keep(&mut self, call: &Call) {
        self.0.retain(|token| !token.is_canceled());
        self.0.push(call.token().clone());
    }
}

impl Drop for OwnCalls {
    fn drop(&mut self) {
        for token in &self.0 {
            token.cancel();
        }
    }
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("status", &self.status)
            .field("input", &self.input)
            .field("busy", &self.busy)
            .field("offer", &self.offer.as_ref().map(Offer::stage))
            .finish_non_exhaustive()
    }
}

/// What the page tells the row about itself.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Context {
    pub(crate) generation: u64,
    /// The page is global settings, loaded and not saving.
    pub(crate) idle: bool,
}

impl Settings {
    fn bump(&mut self) -> u64 {
        self.sequence = self.sequence.wrapping_add(1);
        self.sequence
    }

    pub(crate) fn refresh(&mut self, paths: &Paths, generation: u64) -> Task<Message> {
        let sequence = self.bump();
        let call = self.calls.start(Some("settings-status"));
        self.own.keep(&call);
        status_task(paths.clone(), call, generation, sequence)
    }

    /// Whether the row offers Log out: to someone with a login saved, and
    /// while a logout runs so the button doesn't vanish under the click.
    pub(crate) fn shows_logout(&self) -> bool {
        self.status.as_ref().is_some_and(Status::saved) || self.busy == Some(Busy::LoggingOut)
    }

    /// Whether the row offers Log in: once the status says logged out, or
    /// the check failed with nothing known, so a logged-in row doesn't flash
    /// it while it loads. It stays while the login page opens.
    pub(crate) fn shows_login(&self) -> bool {
        let known = match &self.status {
            Some(status) => !status.logged_in(),
            None => self.status_error.is_some(),
        };
        known || self.busy == Some(Busy::Opening)
    }

    /// The download's spinner line shows only when the page has no offer
    /// to show it, as after leaving and coming back.
    pub(crate) fn busy_line(&self) -> Option<String> {
        match (self.busy, &self.offer) {
            (Some(Busy::Downloading), Some(_)) => None,
            (busy, _) => busy.map(Busy::label),
        }
    }

    pub(crate) fn update(
        &mut self,
        paths: &Paths,
        context: Context,
        event: Event,
    ) -> Task<Message> {
        let idle = context.idle && self.busy.is_none() && self.offer.is_none();
        match event {
            Event::LoginOpen => {
                if !idle || self.input_open {
                    return Task::none();
                }
                self.error = None;
                self.result = None;
                self.input = Secret::default();
                let session = self.bump();
                self.input_session = Some(session);
                self.open(paths, context.generation)
            }
            Event::LoginReady(generation, sequence, result) => {
                if generation != context.generation || self.busy_sequence != Some(sequence) {
                    return Task::none();
                }
                self.busy = None;
                self.busy_sequence = None;
                match result {
                    Ok(Ready::Open) => self.input_open = true,
                    Ok(Ready::Offer(missing)) => self.offer = Some(Offer::new(&missing)),
                    Err(error) => {
                        self.input_session = None;
                        self.error = Some(error);
                    }
                }
                Task::none()
            }
            Event::Offer(sequence, event) => self.offer_event(paths, context, sequence, event),
            Event::LoginCancel => {
                if self.busy.is_none() {
                    self.close_form();
                    self.error = None;
                }
                Task::none()
            }
            Event::LoginText(value) => {
                if self.input_open && self.busy.is_none() {
                    self.input = value;
                    self.error = None;
                }
                Task::none()
            }
            Event::CopyLink => {
                if self.input_open {
                    iced::clipboard::write(LOGIN_URL.to_owned())
                } else {
                    Task::none()
                }
            }
            Event::Login => {
                if !idle || !self.input_open || self.input.as_str().trim().is_empty() {
                    return Task::none();
                }
                self.busy = Some(Busy::LoggingIn);
                self.error = None;
                self.result = None;
                let pasted = std::mem::take(&mut self.input);
                let sequence = self.bump();
                self.busy_sequence = Some(sequence);
                login_task(
                    paths.clone(),
                    self.calls.start(None),
                    context.generation,
                    sequence,
                    self.input_session,
                    pasted,
                )
            }
            Event::Logout => {
                if !idle {
                    return Task::none();
                }
                self.busy = Some(Busy::LoggingOut);
                self.error = None;
                self.result = None;
                let sequence = self.bump();
                self.busy_sequence = Some(sequence);
                logout_task(
                    paths.clone(),
                    self.calls.start(None),
                    context.generation,
                    sequence,
                    self.input_session,
                )
            }
            Event::StatusLoaded(generation, sequence, result) => {
                if generation != context.generation || sequence != self.sequence {
                    return Task::none();
                }
                match result {
                    Ok(status) => {
                        self.status = Some(status);
                        self.status_error = None;
                    }
                    Err(error) => self.status_error = Some(error),
                }
                Task::none()
            }
            Event::LoggedIn(generation, sequence, session, result) => {
                if generation != context.generation || self.busy_sequence != Some(sequence) {
                    return Task::none();
                }
                self.busy = None;
                self.busy_sequence = None;
                let same_form = session.is_some() && session == self.input_session;
                match result {
                    // The Account row says who, so no result line repeats it.
                    Ok(_) => {
                        if same_form {
                            self.close_form();
                        }
                    }
                    Err(error) => {
                        self.error = Some(error);
                        // The form stays for another try, but the code was
                        // used up, so it starts empty.
                        if !same_form {
                            self.input_open = false;
                        }
                    }
                }
                self.input = Secret::default();
                self.refresh(paths, context.generation)
            }
            Event::LoggedOut(generation, sequence, session, result) => {
                if generation != context.generation || self.busy_sequence != Some(sequence) {
                    return Task::none();
                }
                self.busy = None;
                self.busy_sequence = None;
                match result {
                    Ok(result) => {
                        self.result = Some(result);
                        if session.is_some() && session == self.input_session {
                            self.close_form();
                        }
                    }
                    Err(error) => self.error = Some(error),
                }
                self.refresh(paths, context.generation)
            }
        }
    }

    /// Looks for Legendary and opens the login page, or brings up the offer.
    fn open(&mut self, paths: &Paths, generation: u64) -> Task<Message> {
        self.busy = Some(Busy::Opening);
        let sequence = self.bump();
        self.busy_sequence = Some(sequence);
        let call = self.calls.start(Some("settings-open"));
        self.own.keep(&call);
        open_task(paths.clone(), call, generation, sequence)
    }

    fn offer_event(
        &mut self,
        paths: &Paths,
        context: Context,
        sequence: u64,
        event: legendary::Event,
    ) -> Task<Message> {
        let download = matches!(event, legendary::Event::Download);
        if self.offer.is_none() {
            // The page was left while it downloaded: only the end matters.
            if let legendary::Event::Done(_, result) = event
                && self.busy_sequence == Some(sequence)
            {
                self.busy = None;
                self.busy_sequence = None;
                self.input_session = None;
                if let Err(legendary::Failure::Failed(error)) = result {
                    self.error = Some(error);
                }
                return self.refresh(paths, context.generation);
            }
            return Task::none();
        }
        if !download && self.busy_sequence.is_some_and(|busy| busy != sequence) {
            return Task::none();
        }
        let sequence = if download && self.busy.is_none() {
            let sequence = self.bump();
            self.busy = Some(Busy::Downloading);
            self.busy_sequence = Some(sequence);
            sequence
        } else {
            sequence
        };
        let Some(offer) = &mut self.offer else {
            return Task::none();
        };
        let (task, outcome) = offer.update(paths, event);
        let task = task.map(move |event| epic_message(Event::Offer(sequence, event)));
        match outcome {
            Outcome::Pending => task,
            Outcome::Declined => {
                self.offer = None;
                self.busy = None;
                self.busy_sequence = None;
                self.input_session = None;
                Task::none()
            }
            Outcome::Failed(error) => {
                self.offer = None;
                self.busy = None;
                self.busy_sequence = None;
                self.input_session = None;
                self.error = Some(error);
                self.refresh(paths, context.generation)
            }
            Outcome::Ready => {
                self.offer = None;
                Task::batch([
                    self.open(paths, context.generation),
                    self.refresh(paths, context.generation),
                ])
            }
        }
    }

    fn close_form(&mut self) {
        self.input_open = false;
        self.input = Secret::default();
        self.input_session = None;
    }
}

/// Who is logged in and which Legendary is in use, read without asking
/// Epic or offering a download.
pub(crate) fn status_with(paths: &Paths, token: &Token) -> Result<Status, String> {
    status_from(paths, legendary::find(paths, token)?, token)
}

pub(crate) fn status_from(paths: &Paths, found: Found, token: &Token) -> Result<Status, String> {
    let saved = || {
        if wineo_core::epic::state_dir(paths)
            .join("user.json")
            .exists()
        {
            Account::Unread
        } else {
            Account::LoggedOut
        }
    };
    Ok(match found {
        Found::Usable(found) => {
            let account = match account::status(&found, token).map_err(|error| error.to_string())? {
                account::Status::LoggedIn { account } => Account::LoggedIn(account),
                account::Status::LoggedOut => Account::LoggedOut,
            };
            Status {
                account,
                legendary: Program::InUse(legendary::in_use(paths, &found)),
            }
        }
        Found::Offer(_) => Status {
            account: saved(),
            legendary: Program::Missing,
        },
        Found::Unusable(why) => Status {
            account: saved(),
            legendary: Program::Unusable(why),
        },
    })
}

/// Opens the login page when Legendary is there; otherwise says what Log in
/// offers instead.
pub(crate) fn ready_from(found: Found, open: impl FnOnce(&str)) -> Result<Ready, String> {
    match found {
        Found::Usable(_) => {
            open(LOGIN_URL);
            Ok(Ready::Open)
        }
        Found::Offer(missing) => Ok(Ready::Offer(missing)),
        Found::Unusable(why) => Err(why),
    }
}

/// Logs in with what was pasted and returns the account. Errors never
/// repeat what was pasted.
pub(crate) fn login_with(paths: &Paths, pasted: &str, token: &Token) -> Result<String, String> {
    let code = LoginCode::parse(pasted).map_err(|error| error.to_string())?;
    match legendary::find(paths, token)? {
        Found::Usable(found) => {
            account::login(&found, &code, token).map_err(|error| error.to_string())
        }
        Found::Offer(_) => {
            Err("Legendary isn't installed. Press Log in again to download it.".to_owned())
        }
        Found::Unusable(why) => Err(why),
    }
}

/// Logs out and says what was removed, like `import epic --logout`. It
/// never offers a download: without Legendary, wineo removes the saved
/// login itself.
pub(crate) fn logout_with(paths: &Paths, token: &Token) -> Result<String, String> {
    logout_from(paths, legendary::find(paths, token)?, token)
}

pub(crate) fn logout_from(paths: &Paths, found: Found, token: &Token) -> Result<String, String> {
    let was = match found {
        Found::Usable(found) => {
            account::logout(&found, token).map_err(|error| error.to_string())?
        }
        Found::Offer(_) | Found::Unusable(_) => {
            account::logout_without_legendary(paths).map_err(|error| error.to_string())?
        }
    };
    Ok(if was {
        "Removed the Epic login."
    } else {
        "No Epic login was saved."
    }
    .to_owned())
}

pub(crate) fn status_task(
    paths: Paths,
    call: Call,
    generation: u64,
    sequence: u64,
) -> Task<Message> {
    task::blocking(
        move || status_with(&paths, call.token()),
        move |result| epic_message(Event::StatusLoaded(generation, sequence, result)),
    )
}

fn open_task(paths: Paths, call: Call, generation: u64, sequence: u64) -> Task<Message> {
    task::blocking(
        move || ready_from(legendary::find(&paths, call.token())?, open_browser),
        move |result| epic_message(Event::LoginReady(generation, sequence, result)),
    )
}

/// Best effort, since the address is shown too. xdg-open can run the
/// browser itself and not return until it closes, so it's reaped elsewhere.
fn open_browser(url: &str) {
    // A test never opens a real browser.
    if cfg!(test) {
        return;
    }
    if let Ok(mut child) = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn login_task(
    paths: Paths,
    call: Call,
    generation: u64,
    sequence: u64,
    input_session: Option<u64>,
    pasted: Secret,
) -> Task<Message> {
    task::blocking(
        move || login_with(&paths, pasted.as_str(), call.token()),
        move |result| epic_message(Event::LoggedIn(generation, sequence, input_session, result)),
    )
}

fn logout_task(
    paths: Paths,
    call: Call,
    generation: u64,
    sequence: u64,
    input_session: Option<u64>,
) -> Task<Message> {
    task::blocking(
        move || logout_with(&paths, call.token()),
        move |result| {
            epic_message(Event::LoggedOut(
                generation,
                sequence,
                input_session,
                result,
            ))
        },
    )
}

fn epic_message(event: Event) -> Message {
    Message::Settings(super::Event::Epic(event))
}

#[cfg(test)]
mod tests;
