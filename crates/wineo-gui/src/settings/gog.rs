//! The GOG login row: who is logged in, logging in through the browser with
//! the address GOG's login page lands on, and logging out. The address holds
//! a one-time login code, so it only ever reaches a secure input and the
//! login call, never a message, log, or `Debug` output.

use std::path::{Path, PathBuf};

use iced::Task;
use wineo_core::gog::account::{self, KEY_SPEC};
use wineo_core::gog::api::{Client, Endpoints};
use wineo_core::key_store::{self, KeySource, Keyring, Saved};
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;

use super::steamgriddb::{Secret, one_line};
use crate::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Busy {
    Opening,
    LoggingIn,
    LoggingOut,
}

impl Busy {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Opening => "Opening GOG's login page...",
            Self::LoggingIn => "Logging in...",
            Self::LoggingOut => "Logging out...",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    LoggedOut,
    LoggedIn {
        user_id: u64,
        /// GOG's public name for the account, when it is known.
        username: Option<String>,
        /// The key file, when there is no keyring and the login is kept in
        /// plain text.
        plain_text: Option<PathBuf>,
    },
}

impl Status {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::LoggedOut => "Not logged in".to_owned(),
            Self::LoggedIn {
                user_id, username, ..
            } => logged_in(*user_id, username.as_deref()),
        }
    }

    pub(crate) fn warning(&self) -> Option<String> {
        match self {
            Self::LoggedIn {
                plain_text: Some(path),
                ..
            } => Some(plain_text_warning(path)),
            _ => None,
        }
    }

    fn logged_in(&self) -> bool {
        matches!(self, Self::LoggedIn { .. })
    }
}

/// The name when GOG gave one, else the account number.
fn logged_in(user_id: u64, username: Option<&str>) -> String {
    match username {
        Some(name) => format!("Logged in to GOG as {name}."),
        None => format!("Logged in to GOG as account {user_id}."),
    }
}

/// `import gog --login`'s warning.
fn plain_text_warning(path: &Path) -> String {
    format!(
        "warning: no keyring, so the login is stored in plain text in {}",
        path.display()
    )
}

/// A login that worked. The Account row already says who is logged in, so
/// there is no result line to repeat it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoggedIn {
    pub(crate) status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoggedOut {
    pub(crate) status: Status,
    pub(crate) result: String,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// Opens GOG's login page and the field for the address.
    LoginOpen,
    LoginCancel,
    LoginText(Secret),
    Login,
    Logout,
    CopyLink,
    StatusLoaded(u64, u64, Result<Status, String>),
    /// The login page's address once the browser was asked to open it.
    LoginReady(u64, u64, Result<String, String>),
    LoggedIn(u64, u64, Option<u64>, Result<LoggedIn, String>),
    LoggedOut(u64, u64, Option<u64>, Result<LoggedOut, String>),
}

impl Event {
    /// The sequence of a finished login or logout, which the app hands to
    /// whichever global page is open.
    pub(crate) fn operation(&self) -> Option<u64> {
        match self {
            Self::LoggedIn(_, sequence, _, _) | Self::LoggedOut(_, sequence, _, _) => {
                Some(*sequence)
            }
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

#[derive(Debug, Clone, Default)]
pub(crate) struct Settings {
    pub(crate) status: Option<Status>,
    pub(crate) status_error: Option<String>,
    /// GOG's login page, once the form is open.
    pub(crate) login_url: Option<String>,
    pub(crate) input_open: bool,
    pub(crate) input: Secret,
    pub(crate) busy: Option<Busy>,
    pub(crate) error: Option<String>,
    pub(crate) result: Option<String>,
    pub(crate) warning: Option<String>,
    /// Only the latest status lands.
    pub(crate) sequence: u64,
    pub(crate) busy_sequence: Option<u64>,
    /// The open form, so a result for an older one doesn't close it.
    pub(crate) input_session: Option<u64>,
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
        status_task(paths.clone(), generation, sequence)
    }

    pub(crate) fn logged_in(&self) -> bool {
        self.status.as_ref().is_some_and(Status::logged_in)
    }

    /// Whether the row offers Log out: only to someone logged in, and while
    /// a logout runs so the button doesn't vanish under the click.
    pub(crate) fn shows_logout(&self) -> bool {
        self.logged_in() || self.busy == Some(Busy::LoggingOut)
    }

    /// Whether the row offers Log in: once the status says logged out, or the
    /// check failed with nothing known, so a logged-in row doesn't flash it
    /// while it loads. Like Log out, it stays while the login page opens.
    pub(crate) fn shows_login(&self) -> bool {
        let known = match &self.status {
            Some(status) => !status.logged_in(),
            None => self.status_error.is_some(),
        };
        known || self.busy == Some(Busy::Opening)
    }

    pub(crate) fn update(
        &mut self,
        paths: &Paths,
        context: Context,
        event: Event,
    ) -> Task<Message> {
        let idle = context.idle && self.busy.is_none();
        match event {
            Event::LoginOpen => {
                if !idle || self.input_open {
                    return Task::none();
                }
                self.busy = Some(Busy::Opening);
                self.error = None;
                self.result = None;
                self.input = Secret::default();
                let session = self.bump();
                self.input_session = Some(session);
                self.busy_sequence = Some(session);
                open_task(context.generation, session)
            }
            Event::LoginReady(generation, sequence, result) => {
                if generation != context.generation || self.busy_sequence != Some(sequence) {
                    return Task::none();
                }
                self.busy = None;
                self.busy_sequence = None;
                match result {
                    Ok(url) => {
                        self.login_url = Some(url);
                        self.input_open = true;
                    }
                    Err(error) => {
                        self.input_session = None;
                        self.error = Some(error);
                    }
                }
                Task::none()
            }
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
            Event::CopyLink => match &self.login_url {
                Some(url) if self.input_open => iced::clipboard::write(url.clone()),
                _ => Task::none(),
            },
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
                        self.warning = status.warning();
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
                    Ok(done) => {
                        self.warning = done.status.warning();
                        self.status = Some(done.status);
                        if same_form {
                            self.close_form();
                        }
                    }
                    Err(error) => {
                        self.error = Some(error);
                        // The form stays for another try, but the address
                        // was used up, so it starts empty.
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
                    Ok(done) => {
                        self.warning = None;
                        self.status = Some(done.status);
                        self.result = Some(done.result);
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

    fn close_form(&mut self) {
        self.input_open = false;
        self.input = Secret::default();
        self.input_session = None;
        self.login_url = None;
    }
}

fn keyring() -> SecretServiceKeyring {
    SecretServiceKeyring::for_spec(&KEY_SPEC)
}

/// Who is logged in, read from what's saved without asking GOG.
pub(crate) fn status_with(paths: &Paths, keyring: &dyn Keyring) -> Result<Status, String> {
    let found = account::stored_login(paths, keyring).map_err(one_line)?;
    Ok(match found {
        None => Status::LoggedOut,
        Some((login, source)) => Status::LoggedIn {
            user_id: login.user_id.get(),
            username: login.username,
            plain_text: (source == KeySource::File).then(|| key_store::key_file(&KEY_SPEC, paths)),
        },
    })
}

pub(crate) fn login_with(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    pasted: &str,
) -> Result<LoggedIn, String> {
    // AccountError never carries what was pasted, and GOG's errors name the
    // endpoint rather than the address.
    let done = account::login(paths, keyring, client, pasted).map_err(one_line)?;
    let plain_text = match &done.saved {
        Saved::File(path) => Some(path.clone()),
        Saved::Keyring => None,
    };
    Ok(LoggedIn {
        status: Status::LoggedIn {
            user_id: done.user_id.get(),
            username: done.username,
            plain_text,
        },
    })
}

/// Logs out and says what was removed, like `import gog --logout`.
pub(crate) fn logout_with(paths: &Paths, keyring: &dyn Keyring) -> Result<LoggedOut, String> {
    let out = account::logout(paths, keyring).map_err(one_line)?;
    let result = if !(out.removed.keyring || out.removed.file) {
        "No GOG login was saved.".to_owned()
    } else {
        match out.user_id.filter(|_| out.cache_removed) {
            Some(user_id) => format!(
                "Removed the GOG login. Removed the owned-games list for account {user_id}."
            ),
            None => "Removed the GOG login.".to_owned(),
        }
    };
    Ok(LoggedOut {
        status: Status::LoggedOut,
        result,
    })
}

pub(crate) fn status_task(paths: Paths, generation: u64, sequence: u64) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || status_with(&paths, &keyring()))
                .await
                .unwrap_or_else(|error| Err(one_line(error)))
        },
        move |result| gog_message(Event::StatusLoaded(generation, sequence, result)),
    )
}

/// The login page's address, after asking the browser to open it. Opening
/// is best effort, since the address is shown too.
fn open_task(generation: u64, sequence: u64) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(|| {
                let client = wineo_core::gog::client_from_env().map_err(one_line)?;
                let url = client.login_url();
                open_browser(&client, &url);
                Ok(url)
            })
            .await
            .unwrap_or_else(|error| Err(one_line(error)))
        },
        move |result| gog_message(Event::LoginReady(generation, sequence, result)),
    )
}

fn open_browser(client: &Client, url: &str) {
    // A fake server in a test isn't GOG, and a browser on it is just noise.
    if client.endpoints() != &Endpoints::production() {
        return;
    }
    // xdg-open can run the browser itself and not return until it closes,
    // so it's waited on elsewhere: reaped, but the address shows now.
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
    generation: u64,
    sequence: u64,
    input_session: Option<u64>,
    pasted: Secret,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let client = wineo_core::gog::client_from_env().map_err(one_line)?;
                login_with(&paths, &keyring(), client, pasted.as_str())
            })
            .await
            .unwrap_or_else(|error| Err(one_line(error)))
        },
        move |result| gog_message(Event::LoggedIn(generation, sequence, input_session, result)),
    )
}

fn logout_task(
    paths: Paths,
    generation: u64,
    sequence: u64,
    input_session: Option<u64>,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || logout_with(&paths, &keyring()))
                .await
                .unwrap_or_else(|error| Err(one_line(error)))
        },
        move |result| {
            gog_message(Event::LoggedOut(
                generation,
                sequence,
                input_session,
                result,
            ))
        },
    )
}

fn gog_message(event: Event) -> Message {
    Message::Settings(super::Event::Gog(event))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::ffi::OsString;

    use wineo_core::key_store::NoKeyring;

    use super::*;
    use crate::gog_fake::{Server, reply, tokens};
    use crate::settings::steam_key::tests::TestKeyring;

    const CODE: &str = "PASTEDCODEabcdef0123456789";

    fn pasted() -> String {
        format!("https://embed.gog.com/on_login_success?origin=client&code={CODE}")
    }

    fn setup() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
            None,
        )
        .unwrap();
        (root, paths)
    }

    fn keyring() -> TestKeyring {
        TestKeyring(RefCell::new(None))
    }

    #[test]
    fn logging_in_says_which_account_and_logging_out_what_was_removed() {
        let (_root, paths) = setup();
        let keyring = keyring();
        assert_eq!(status_with(&paths, &keyring).unwrap(), Status::LoggedOut);
        assert_eq!(Status::LoggedOut.label(), "Not logged in");

        let server = Server::start(|_| tokens("REFRESH1", 48151623));
        let done = login_with(&paths, &keyring, server.client(), &pasted()).unwrap();
        // This server has no profile to give, so it falls back to the number.
        assert_eq!(done.status.label(), "Logged in to GOG as account 48151623.");
        assert!(server.requests()[0].contains(CODE));
        let status = status_with(&paths, &keyring).unwrap();
        assert_eq!(status, done.status);
        assert_eq!(status.label(), "Logged in to GOG as account 48151623.");
        assert_eq!(status.warning(), None);

        let out = logout_with(&paths, &keyring).unwrap();
        assert_eq!(out.result, "Removed the GOG login.");
        assert_eq!(status_with(&paths, &keyring).unwrap(), Status::LoggedOut);
        let again = logout_with(&paths, &keyring).unwrap();
        assert_eq!(again.result, "No GOG login was saved.");
    }

    #[test]
    fn the_status_shows_the_name_stored_with_the_login_without_asking_gog() {
        let (_root, paths) = setup();
        let keyring = keyring();
        let server = Server::start(|line| {
            if line.starts_with("GET /users/") {
                reply(200, r#"{"id":"48151623","username":"Hunter42"}"#)
            } else {
                tokens("REFRESH1", 48151623)
            }
        });
        let done = login_with(&paths, &keyring, server.client(), &pasted()).unwrap();
        assert_eq!(done.status.label(), "Logged in to GOG as Hunter42.");
        let requests = server.requests().len();

        let status = status_with(&paths, &keyring).unwrap();
        assert_eq!(status, done.status);
        assert_eq!(status.label(), "Logged in to GOG as Hunter42.");
        assert_eq!(
            server.requests().len(),
            requests,
            "a status check is offline"
        );
    }

    #[test]
    fn the_account_row_shows_the_name_in_place_of_the_number() {
        let row = Settings {
            status: Some(Status::LoggedIn {
                user_id: 9,
                username: Some("Hunter42".to_owned()),
                plain_text: None,
            }),
            ..Settings::default()
        };
        let texts = super::super::view::gog_texts(&row);
        assert_eq!(texts[1], "Logged in to GOG as Hunter42.");
        assert!(!texts.iter().any(|text| text.contains("account 9")));
    }

    #[test]
    fn without_a_keyring_the_login_is_plain_text_with_a_warning() {
        let (_root, paths) = setup();
        let server = Server::start(|_| tokens("REFRESH1", 7));
        let done = login_with(&paths, &NoKeyring, server.client(), &pasted()).unwrap();
        let warning = done.status.warning().unwrap();
        assert!(warning.contains("plain text"), "{warning}");
        assert!(warning.contains(&key_store::key_file(&KEY_SPEC, &paths).display().to_string()));
        assert_eq!(status_with(&paths, &NoKeyring).unwrap(), done.status);
    }

    #[test]
    fn failed_logins_never_show_the_pasted_address() {
        let (_root, paths) = setup();
        let keyring = keyring();
        let rejected = Server::start(|_| {
            reply(
                400,
                r#"{"error":"invalid_grant","error_description":"bad code"}"#,
            )
        });
        let down = Server::start(|_| reply(503, "{}"));
        let errors = [
            login_with(&paths, &keyring, rejected.client(), &pasted()).unwrap_err(),
            login_with(&paths, &keyring, down.client(), &pasted()).unwrap_err(),
            login_with(
                &paths,
                &keyring,
                rejected.client(),
                &format!("https://example.com/?code={CODE}"),
            )
            .unwrap_err(),
        ];
        assert_eq!(
            errors[0],
            "GOG didn't accept that login. Log in again and paste the new address."
        );
        for error in &errors {
            assert!(!error.contains(CODE), "{error}");
            assert!(!error.contains("on_login_success"), "{error}");
        }
        assert_eq!(status_with(&paths, &keyring).unwrap(), Status::LoggedOut);
    }

    #[test]
    fn an_unreadable_login_is_an_error_status() {
        let (_root, paths) = setup();
        let keyring = TestKeyring(RefCell::new(Some("not a login".to_owned())));
        let error = status_with(&paths, &keyring).unwrap_err();
        assert!(error.contains("can't be read"), "{error}");
        assert!(!error.contains("not a login"));
    }

    fn context() -> Context {
        Context {
            generation: 1,
            idle: true,
        }
    }

    #[test]
    fn the_row_takes_the_address_and_forgets_it() {
        let (_root, paths) = setup();
        let mut row = Settings::default();
        let _ = row.update(&paths, context(), Event::LoginOpen);
        assert_eq!(row.busy, Some(Busy::Opening));
        let session = row.busy_sequence.unwrap();
        let url = "https://auth.gog.com/auth?client_id=1".to_owned();
        let _ = row.update(
            &paths,
            context(),
            Event::LoginReady(1, session, Ok(url.clone())),
        );
        assert!(row.input_open);
        assert_eq!(row.login_url.as_deref(), Some(url.as_str()));
        let texts = super::super::view::gog_texts(&row);
        assert!(texts.contains(&url));
        assert!(texts.contains(&"Address".to_owned()));

        let _ = row.update(&paths, context(), Event::LoginText(Secret::new(pasted())));
        assert!(!format!("{row:?}").contains(CODE));
        assert!(
            !super::super::view::gog_texts(&row)
                .iter()
                .any(|text| text.contains(CODE))
        );
        let _ = row.update(&paths, context(), Event::Login);
        assert_eq!(row.busy, Some(Busy::LoggingIn));
        assert_eq!(row.input.as_str(), "");
        let sequence = row.busy_sequence.unwrap();
        let event = Event::LoggedIn(
            1,
            sequence,
            Some(session),
            Err("GOG didn't accept that login. Log in again and paste the new address.".to_owned()),
        );
        assert!(!format!("{event:?}").contains(CODE));
        let _ = row.update(&paths, context(), event);
        // A failed login keeps the form for another try, empty.
        assert!(row.input_open);
        assert_eq!(row.input.as_str(), "");
        assert!(row.error.as_deref().unwrap().contains("didn't accept"));

        let _ = row.update(&paths, context(), Event::LoginText(Secret::new(pasted())));
        let _ = row.update(&paths, context(), Event::Login);
        let sequence = row.busy_sequence.unwrap();
        let _ = row.update(
            &paths,
            context(),
            Event::LoggedIn(
                1,
                sequence,
                Some(session),
                Ok(LoggedIn {
                    status: Status::LoggedIn {
                        user_id: 9,
                        username: None,
                        plain_text: None,
                    },
                }),
            ),
        );
        assert!(!row.input_open);
        assert!(row.login_url.is_none());
        let texts = super::super::view::gog_texts(&row);
        assert_eq!(texts[1], "Logged in to GOG as account 9.");
        // The Account row says it, so no result line repeats it.
        assert!(row.result.is_none());
        assert_eq!(
            texts
                .iter()
                .filter(|text| text.starts_with("Logged in to GOG"))
                .count(),
            1,
            "{texts:?}"
        );
        assert!(texts.contains(&"Log out".to_owned()));
        assert!(row.logged_in());
    }

    #[test]
    fn the_row_shows_logged_out_and_error_states() {
        let (_root, paths) = setup();
        let mut row = Settings::default();
        assert_eq!(super::super::view::gog_texts(&row)[1], "Checking...");
        let sequence = row.sequence;
        let _ = row.update(
            &paths,
            context(),
            Event::StatusLoaded(1, sequence, Ok(Status::LoggedOut)),
        );
        let texts = super::super::view::gog_texts(&row);
        assert_eq!(texts[1], "Not logged in");
        assert!(!row.logged_in());
        // Nobody to log out, so only Log in is offered.
        assert!(texts.contains(&"Log in".to_owned()));
        assert!(!texts.contains(&"Log out".to_owned()));

        let _ = row.update(
            &paths,
            context(),
            Event::StatusLoaded(
                1,
                sequence,
                Err(
                    "The saved GOG login can't be read. Log in again: wineo import gog --login."
                        .to_owned(),
                ),
            ),
        );
        let texts = super::super::view::gog_texts(&row);
        assert!(texts.iter().any(|text| text.contains("can't be read")));

        // A stale status never lands.
        let _ = row.update(
            &paths,
            context(),
            Event::StatusLoaded(
                0,
                sequence,
                Ok(Status::LoggedIn {
                    user_id: 1,
                    username: None,
                    plain_text: None,
                }),
            ),
        );
        assert_eq!(row.status, Some(Status::LoggedOut));
    }

    #[test]
    fn log_in_and_log_out_each_show_only_when_they_apply() {
        let (_root, paths) = setup();
        let mut row = Settings::default();
        // Still checking: nothing to log in to or out of yet.
        assert!(!row.shows_logout());
        assert!(!row.shows_login());
        assert!(!super::super::view::gog_texts(&row).contains(&"Log out".to_owned()));
        let sequence = row.sequence;
        let _ = row.update(
            &paths,
            context(),
            Event::StatusLoaded(
                1,
                sequence,
                Ok(Status::LoggedIn {
                    user_id: 9,
                    username: None,
                    plain_text: None,
                }),
            ),
        );
        assert!(row.shows_logout());
        assert!(super::super::view::gog_texts(&row).contains(&"Log out".to_owned()));
        assert!(!row.shows_login());
        assert!(!super::super::view::gog_texts(&row).contains(&"Log in".to_owned()));
        let _ = row.update(&paths, context(), Event::Logout);
        assert_eq!(row.busy, Some(Busy::LoggingOut));
        assert!(super::super::view::gog_texts(&row).contains(&"Log out".to_owned()));
        // Even if the status were already gone, the button stays while it runs.
        row.status = Some(Status::LoggedOut);
        assert!(row.shows_logout());
        assert!(row.shows_login());

        // A failed check with nothing known still offers a way in.
        let mut row = Settings {
            status_error: Some("GOG login check failed".to_owned()),
            ..Settings::default()
        };
        assert!(row.shows_login());
        row.status = Some(Status::LoggedOut);
        assert!(super::super::view::gog_texts(&row).contains(&"Log in".to_owned()));
    }

    #[test]
    fn a_busy_or_locked_page_starts_nothing() {
        let (_root, paths) = setup();
        let mut row = Settings::default();
        let locked = Context {
            generation: 1,
            idle: false,
        };
        let _ = row.update(&paths, locked, Event::LoginOpen);
        let _ = row.update(&paths, locked, Event::Logout);
        assert_eq!(row.busy, None);
    }
}
