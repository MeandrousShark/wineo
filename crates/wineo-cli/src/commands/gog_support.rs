//! What `import gog` and `update` share: the saved GOG login, how update
//! states read, and the JSON for them.

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wineo_core::gog;
use wineo_core::gog::account::Session;
use wineo_core::gog::folder::Track;
use wineo_core::gog::update::windows::SetupError;
use wineo_core::gog::update::{
    DlcState, DlcStatus, GameError, Progress, Reason, Status, UpdateState,
};
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;

use crate::prompt::{Canceled, Prompt, Reply};
use crate::style;

pub const LOGGED_OUT: &str = "Log in to GOG first: wineo import gog --login.";

/// The keyring item that holds the GOG login. Sessions borrow it, so it has
/// to outlive them.
pub struct Account {
    keyring: SecretServiceKeyring,
}

impl Account {
    pub fn new() -> Self {
        Self {
            keyring: SecretServiceKeyring::for_spec(&gog::account::KEY_SPEC),
        }
    }

    pub fn keyring(&self) -> &SecretServiceKeyring {
        &self.keyring
    }

    /// The saved login, ready to use, or None when logged out. Finishing an
    /// interrupted update from saved packages works without one.
    pub fn open(&self, paths: &Paths) -> anyhow::Result<Option<Session<'_>>> {
        let client = gog::client_from_env()?;
        Ok(Session::open(paths, &self.keyring, client)?)
    }

    /// The saved login, ready to use. Logged out is an error that says how
    /// to log in.
    pub fn session(&self, paths: &Paths) -> anyhow::Result<Session<'_>> {
        let client = gog::client_from_env()?;
        Session::open(paths, &self.keyring, client)?.ok_or_else(|| anyhow::anyhow!(LOGGED_OUT))
    }
}

/// Prints `error: MESSAGE` and gives the exit code for a command line that
/// can't do what it was asked, without having changed anything.
pub fn usage_error(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("{}", style::error_stderr(format!("error: {message}")));
    ExitCode::from(2)
}

pub fn warn(message: impl std::fmt::Display) {
    eprintln!("{}", style::warning_stderr(format!("warning: {message}")));
}

pub fn track_label(track: Track) -> &'static str {
    match track {
        Track::Installer => "installer",
        Track::Galaxy => "galaxy",
    }
}

/// How a game's update state reads in a table.
pub fn update_label(state: UpdateState) -> &'static str {
    match state {
        UpdateState::UpToDate => "up to date",
        UpdateState::Available => "update available",
        UpdateState::OnGog => "update on GOG",
        UpdateState::CantCompare => "can't compare",
    }
}

/// Why an update can't be applied, or can't be told, as a sentence.
pub fn reason_note(reason: &Reason) -> String {
    match reason {
        Reason::WindowsInstaller => {
            "Updating Windows installer games comes in a later release.".to_owned()
        }
        Reason::Galaxy => "Updating Galaxy builds comes in a later release.".to_owned(),
        Reason::CantCompare(why) => why.clone(),
    }
}

/// The lines beneath a game for its DLC, when they differ or aren't known.
pub fn dlc_notes(status: &Status) -> Vec<String> {
    status
        .dlc
        .iter()
        .filter(|dlc| dlc.state != DlcState::UpToDate)
        .map(|dlc| {
            let label = dlc.label(&status.title);
            let what = match dlc.state {
                DlcState::Available => format!(
                    "{} -> {}",
                    dlc.installed.as_deref().unwrap_or("?"),
                    dlc.available.as_deref().unwrap_or("?")
                ),
                DlcState::Unknown => "version unknown".to_owned(),
                DlcState::CantCompare => "can't compare".to_owned(),
                DlcState::Failed => "failed".to_owned(),
                DlcState::Unavailable => "unavailable".to_owned(),
                DlcState::NotInstalled => "not installed".to_owned(),
                DlcState::UpToDate => unreachable!("filtered out"),
            };
            // An unnamed DLC's label already starts with "DLC".
            let prefix = if label.starts_with("DLC (") {
                ""
            } else {
                "DLC "
            };
            format!("  {prefix}{label}: {what}")
        })
        .collect()
}

pub fn dlc_json(dlc: &[DlcStatus]) -> Vec<Value> {
    dlc.iter()
        .map(|dlc| {
            json!({
                "id": dlc.id.to_string(),
                "title": dlc.title,
                "state": dlc.state.as_str(),
            })
        })
        .collect()
}

pub fn reason_json(reason: &Option<Reason>) -> Value {
    reason
        .as_ref()
        .map_or(Value::Null, |reason| json!(reason.as_str()))
}

/// A game that couldn't be checked, for `errors`. The name is the owning
/// environment, which a state file that can't be read doesn't have.
pub fn error_json(error: &GameError) -> Value {
    json!({
        "name": error.environment,
        "id": error.id.map(|id| id.to_string()),
        "message": error.message,
    })
}

/// Names a failed game for a line on stderr.
pub fn error_subject(error: &GameError) -> String {
    match (&error.environment, error.id) {
        (Some(name), _) => name.clone(),
        (None, Some(id)) => id.to_string(),
        (None, None) => "A GOG game folder".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use wineo_core::gog::folder::{Platform, Track};

    use super::*;

    fn dlc(id: u64, title: Option<&str>, name: Option<&str>) -> DlcStatus {
        DlcStatus {
            id,
            title: title.map(str::to_owned),
            name: name.map(str::to_owned),
            installed: None,
            available: None,
            state: DlcState::Unknown,
        }
    }

    fn status(dlc: Vec<DlcStatus>) -> Status {
        Status {
            environment: "rimworld".to_owned(),
            gog_id: 1,
            title: "RimWorld".to_owned(),
            folder: PathBuf::from("/games/rimworld"),
            platform: Platform::Linux,
            track: Track::Installer,
            language: "en".to_owned(),
            installed: None,
            available: None,
            state: UpdateState::UpToDate,
            applicable: false,
            reason: None,
            unfinished: false,
            warnings: Vec::new(),
            dlc,
        }
    }

    #[test]
    fn dlc_notes_name_each_dlc_with_its_id() {
        let status = status(vec![
            dlc(1233017772, Some("Royalty"), Some("RimWorld - Royalty")),
            dlc(1827857764, None, Some("RimWorld - Ideology")),
            dlc(1865538500, None, None),
        ]);
        assert_eq!(
            dlc_notes(&status),
            [
                "  DLC Royalty (1233017772): version unknown",
                "  DLC Ideology (1827857764): version unknown",
                "  DLC (1865538500): version unknown",
            ]
        );
    }
}

/// Prints one line per stage and a progress line about once a second, the
/// way other downloads do.
pub struct Reporter {
    labels: HashMap<u64, String>,
    last: Option<Instant>,
    file: Option<(u64, String)>,
    verified: HashSet<u64>,
    unpacking: HashSet<u64>,
    publishing: bool,
}

impl Reporter {
    /// `labels` names each product, by id, the way the review does.
    pub fn new(labels: HashMap<u64, String>) -> Self {
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

    /// Whether a second has passed since the last progress line.
    fn due(&self) -> bool {
        self.last
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(1))
    }

    fn say(&mut self, line: String) {
        eprintln!("{line}");
        self.last = Some(Instant::now());
    }

    pub fn show(&mut self, step: &Progress) {
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
                if first || done == total || self.due() {
                    self.say(format!(
                        "Downloading {}: {} / {}",
                        self.label(*product),
                        style::size(*done),
                        style::size(*total)
                    ));
                }
            }
            Progress::Verify { product } => {
                if self.verified.insert(*product) {
                    self.say(format!("Verifying {}", self.label(*product)));
                }
            }
            Progress::Stage {
                product,
                done,
                total,
            } => {
                if self.unpacking.insert(*product) {
                    self.say(format!("Unpacking {}", self.label(*product)));
                } else if self.due() {
                    self.say(format!(
                        "Unpacking {}: {} / {}",
                        self.label(*product),
                        style::size(*done),
                        style::size(*total)
                    ));
                }
            }
            Progress::Publish { done, total } => {
                if !self.publishing {
                    self.publishing = true;
                    self.say(format!("Publishing {total} files"));
                } else if done == total || self.due() {
                    self.say(format!("Published {done} of {total} files"));
                }
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
                self.say(line);
            }
        }
    }
}

/// The game's label when its Windows installer never started and running
/// it as its own wizard may get further.
pub fn offers_wizard(error: &anyhow::Error) -> Option<String> {
    match error.downcast_ref::<SetupError>() {
        Some(SetupError::NotStarted {
            label,
            wizard: true,
            ..
        }) => Some(label.clone()),
        _ => None,
    }
}

/// Asks once whether to run an installer that never started as its own
/// setup window. No is false; Esc and Ctrl+C cancel.
pub fn ask_wizard<I: crate::prompt::Input, W: std::io::Write>(
    prompt: &mut Prompt<I, W>,
    label: &str,
) -> anyhow::Result<bool> {
    // Nothing comes before it to go back to, so Esc cancels.
    prompt.set_first_question(true);
    match prompt.confirm(
        &format!("{label}'s installer didn't start. Run its own setup window?"),
        true,
    )? {
        Reply::Value(answer) => Ok(answer),
        Reply::Back => Err(Canceled.into()),
    }
}

/// Lets a test stop an update at a known point: with
/// `WINEO_TEST_UPDATE_PAUSE=download` or `publish`, the first step of that
/// kind waits for Ctrl+C, and `publish:NAME` only for that environment.
/// Release builds never look at it.
#[cfg(debug_assertions)]
pub fn pause_for_test(environment: &str, step: &Progress, token: &wineo_core::cancel::Token) {
    let wanted = std::env::var("WINEO_TEST_UPDATE_PAUSE").unwrap_or_default();
    let (at, only) = wanted.split_once(':').unwrap_or((wanted.as_str(), ""));
    let here = matches!(
        (at, step),
        ("download", Progress::Download { .. }) | ("publish", Progress::Publish { .. })
    ) && (only.is_empty() || only == environment);
    while here && !token.is_canceled() {
        std::thread::sleep(Duration::from_millis(20));
    }
}
