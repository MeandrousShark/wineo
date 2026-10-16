//! What `import gog` and `update` share: the saved GOG login, how update
//! states read, the JSON for them, progress lines, and the question that
//! offers a Windows installer's own setup window.

use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};
use wineo_core::gog;
use wineo_core::gog::account::Session;
use wineo_core::gog::api::Secret;
use wineo_core::gog::folder::{self, FolderError, Journal};
use wineo_core::gog::update::galaxy::GalaxyError;
use wineo_core::gog::update::windows::SetupError;
use wineo_core::gog::update::{
    self, DlcState, DlcStatus, GameError, Progress, Reason, Status, UpdateState,
};
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;

use crate::prompt::{self, Canceled, Prompt, Reply};
use crate::style;

pub const LOGGED_OUT: &str = "Log in to GOG first: wineo import gog --login.";

/// The journal an install or update left in the folder of `name`'s game,
/// read from its state alone. A first install that stopped before its
/// environment had a config is found by `gog_id`.
pub fn journal_for(paths: &Paths, name: &str, gog_id: Option<u64>) -> Option<Journal> {
    update::journal_of(paths, name).or_else(|| {
        folder::find(paths, name, &gog_id?.to_string())
            .ok()
            .flatten()
            .and_then(|state| state.journal)
    })
}

/// How `journal` is finished: a Galaxy first install by installing the game
/// again, since `wineo update` has no way to carry one on; anything else by
/// updating.
pub fn finish_command(journal: &Journal, name: &str) -> String {
    match journal.galaxy_install() {
        Some(gog_id) => format!("wineo import gog --game {gog_id}"),
        None => format!("wineo update {name}"),
    }
}

/// Ends the message of a failure with how to finish the game's unfinished
/// install or update, decided by the journal still being in its folder and
/// not by what failed or which command was running. Failures that are
/// cancellations (`canceled`) say their own, and a message that already
/// names the command is left alone. For an install that `wineo update`
/// can't finish, core's "install it again" is said with the command instead.
pub fn with_finish_hint(
    paths: &Paths,
    error: anyhow::Error,
    name: &str,
    gog_id: Option<u64>,
    canceled: bool,
) -> anyhow::Error {
    if canceled {
        return error;
    }
    let Some(journal) = journal_for(paths, name, gog_id) else {
        return error;
    };
    let command = finish_command(&journal, name);
    let message = if let Some(FolderError::UnfinishedInstall { name, .. }) = error.downcast_ref() {
        format!("{name}'s GOG install didn't finish.")
    } else {
        format!("{error:#}")
    };
    if message.contains(&command) {
        return error;
    }
    let sep = if message.ends_with('.') { "" } else { "." };
    anyhow::anyhow!("{message}{sep} Finish it with: {command}")
}

/// The keyring item that holds the GOG login. Sessions borrow it, so it has
/// to outlive them.
pub struct Account {
    keyring: SecretServiceKeyring,
}

impl Account {
    pub fn new() -> Self {
        Self {
            keyring: SecretServiceKeyring::gog(),
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

/// How a game's update state reads in a table: a pinned game with a newer
/// build waiting says so instead of offering it.
pub fn state_label(status: &Status) -> &'static str {
    match &status.reason {
        Some(Reason::Pinned { .. }) => "pinned",
        _ => update_label(status.state),
    }
}

/// How an update state reads in a table.
pub fn update_label(state: UpdateState) -> &'static str {
    match state {
        UpdateState::UpToDate => "up to date",
        UpdateState::Available => "update available",
        UpdateState::OnGog => "update on GOG",
        UpdateState::CantCompare => "can't compare",
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

/// Prints one line per stage and a progress line about once a second, the
/// way other downloads do.
pub struct Reporter(update::Reporter);

impl Reporter {
    /// `labels` names each product, by id, the way the review does.
    pub fn new(labels: HashMap<u64, String>) -> Self {
        Self(update::Reporter::new(labels, Duration::from_secs(1)))
    }

    pub fn show(&mut self, step: &Progress) {
        if let Some(line) = self.0.line(step) {
            eprintln!("{line}");
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

/// What a failed plan says about a private branch it needs a password for,
/// when asking could get the plan through.
pub struct NeedsPassword {
    pub title: String,
    pub branch: String,
    /// How to run the command again with the branch named.
    pub command: String,
}

pub fn needs_password(error: &anyhow::Error) -> Option<NeedsPassword> {
    match error.downcast_ref::<GalaxyError>() {
        Some(GalaxyError::NeedsPassword {
            title,
            branch,
            command,
        }) => Some(NeedsPassword {
            title: title.clone(),
            branch: branch.clone(),
            command: command.clone(),
        }),
        _ => None,
    }
}

/// A password read from standard input for `--password-stdin`: one line,
/// without echo when it comes from a terminal. Spaces around it are part
/// of it.
pub fn password_from_stdin() -> anyhow::Result<Secret> {
    let Some(password) = prompt::secret("Password: ")?.filter(|password| !password.is_empty())
    else {
        anyhow::bail!("No password was given on standard input.");
    };
    Ok(Secret::new(password))
}

/// What to say when a password is needed and there is no terminal. With
/// `--branch` on the command line only `--password-stdin` is missing;
/// without it, the command that works is named.
fn without_terminal(needed: &NeedsPassword, branch_named: bool) -> String {
    let NeedsPassword {
        branch, command, ..
    } = needed;
    if branch_named {
        format!("Branch {branch} is private. Give its password with --password-stdin.")
    } else {
        format!("Branch {branch} is private. Give its password with: {command} --password-stdin")
    }
}

/// Asks for a private branch's password at a terminal, without echo.
/// Without one it says how to give it: `branch_named` is whether `--branch`
/// is on the command line, which `--password-stdin` needs.
pub fn ask_password(needed: &NeedsPassword, branch_named: bool) -> anyhow::Result<Secret> {
    let NeedsPassword { title, branch, .. } = needed;
    if !prompt::interactive() {
        anyhow::bail!("{}", without_terminal(needed, branch_named));
    }
    let Some(password) = prompt::secret(&format!(
        "Branch {branch} of {title} is private. Password: "
    ))?
    .filter(|password| !password.is_empty()) else {
        anyhow::bail!("No password entered.");
    };
    Ok(Secret::new(password))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use wineo_core::gog::folder::{Platform, Track};

    use super::*;

    #[test]
    fn the_no_terminal_message_names_a_command_that_works() {
        let needed = |command: &str| NeedsPassword {
            title: "Game".to_owned(),
            branch: "Legacy (64-bit)".to_owned(),
            command: command.to_owned(),
        };
        assert_eq!(
            without_terminal(
                &needed("wineo update game --branch 'Legacy (64-bit)'"),
                true
            ),
            "Branch Legacy (64-bit) is private. Give its password with --password-stdin."
        );
        assert_eq!(
            without_terminal(
                &needed("wineo update game --branch 'Legacy (64-bit)'"),
                false
            ),
            "Branch Legacy (64-bit) is private. Give its password with: wineo update game --branch 'Legacy (64-bit)' --password-stdin"
        );
        assert_eq!(
            without_terminal(&needed("wineo import gog --game 7 --branch beta"), false),
            "Branch Legacy (64-bit) is private. Give its password with: wineo import gog --game 7 --branch beta --password-stdin"
        );
    }

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
            installed_name: None,
            available_name: None,
            branch: None,
            pinned: None,
            pinned_name: None,
            state: UpdateState::UpToDate,
            applicable: false,
            reason: None,
            unfinished: false,
            galaxy_install: false,
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
