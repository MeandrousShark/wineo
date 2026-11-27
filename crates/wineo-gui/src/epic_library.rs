//! Epic games in the library: what wineo's own record says about each card
//! (work left to finish, which stops it from starting), and what the
//! background check says Epic offers, like GOG's.

use std::collections::{HashMap, HashSet};

use wineo_core::cancel::Token;
use wineo_core::environment::Environment;
use wineo_core::epic::account;
use wineo_core::epic::game::{self, Work};
use wineo_core::epic::owned;
use wineo_core::epic::update::{self, Batch, Status, UpdateState};
use wineo_core::paths::Paths;

use crate::epic_legendary::{self as legendary, Found};
use crate::gog_library::Badge;

/// What a card knows about Epic without asking Epic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EpicCard {
    pub(crate) app: String,
    /// Work on the game that didn't finish. It won't start until it is.
    pub(crate) unfinished: Option<Work>,
}

impl EpicCard {
    /// The label of the action that finishes what was interrupted.
    pub(crate) fn finish_label(&self) -> &'static str {
        if self.unfinished == Some(Work::Install) {
            "Finish install"
        } else {
            "Finish update"
        }
    }

    /// The panel's line about what was interrupted, like GOG's.
    pub(crate) fn interrupted_note(&self) -> &'static str {
        if self.unfinished == Some(Work::Install) {
            "Its install was interrupted. Finish it before playing."
        } else {
            "Its last update was interrupted. Finish it before playing."
        }
    }
}

/// What `env`'s card shows about Epic, None for one that isn't an Epic game.
/// Epic games live in the host library, so a portable session has none.
pub(crate) fn card(paths: &Paths, env: &Environment) -> Option<EpicCard> {
    if paths.portable_root().is_some() {
        return None;
    }
    let app = env.config.epic_id.clone()?;
    let unfinished = game::peek(paths, &app)
        .ok()
        .flatten()
        .and_then(|record| record.in_flight)
        .map(|mark| mark.work);
    Some(EpicCard { app, unfinished })
}

pub(crate) fn badge(epic: Option<&EpicCard>, status: Option<&Status>) -> Option<Badge> {
    match epic?.unfinished {
        Some(Work::Install) => Some(Badge::FinishInstall),
        Some(_) => Some(Badge::FinishUpdate),
        None => status
            .is_some_and(updatable)
            .then_some(Badge::UpdateAvailable),
    }
}

/// Whether this wineo can update the game now.
pub(crate) fn updatable(status: &Status) -> bool {
    status.applicable && status.state == UpdateState::Available
}

/// How a game's update state reads in the panel, as GOG's do.
pub(crate) fn status_label(status: &Status) -> &'static str {
    crate::gog_library::state_label(status.state)
}

/// What the background check found, or None when there is nothing to ask:
/// a portable session, no Epic game, no Legendary to run, or logged out.
/// Never offers a download.
pub(crate) fn check_with(paths: &Paths, token: &Token) -> Result<Option<Batch>, String> {
    if paths.portable_root().is_some()
        || update::environments(paths)
            .map_err(|error| format!("{error:#}"))?
            .is_empty()
    {
        return Ok(None);
    }
    let Found::Usable(found) = legendary::find(paths, token)? else {
        return Ok(None);
    };
    if account::status(&found, token).map_err(|error| error.to_string())?
        == account::Status::LoggedOut
    {
        return Ok(None);
    }
    let owned = owned::load(&found, false, token).map_err(|error| error.to_string())?;
    update::check_all(paths, Ok(&owned))
        .map(Some)
        .map_err(|error| format!("{error:#}"))
}

/// What checking `name` found, or None when there is nothing to ask.
pub(crate) fn recheck_with(
    paths: &Paths,
    name: &str,
    token: &Token,
) -> Result<Option<Box<Status>>, String> {
    if paths.portable_root().is_some() {
        return Ok(None);
    }
    let Found::Usable(found) = legendary::find(paths, token)? else {
        return Ok(None);
    };
    if account::status(&found, token).map_err(|error| error.to_string())?
        == account::Status::LoggedOut
    {
        return Ok(None);
    }
    let owned = owned::load(&found, false, token).map_err(|error| error.to_string())?;
    update::check(paths, name, Ok(&owned))
        .map(|status| Some(Box::new(status)))
        .map_err(|error| format!("{error:#}"))
}

/// Checks every Epic game for updates, once after start.
pub(crate) fn check_task(paths: Paths, asked: HashMap<String, u64>) -> iced::Task<crate::Message> {
    if paths.portable_root().is_some() {
        return iced::Task::none();
    }
    crate::task::blocking(
        move || check_with(&paths, &Token::new()),
        move |result| crate::Message::EpicChecked(asked, result),
    )
}

/// Checks `name` again once an update changed it.
pub(crate) fn recheck(app: &mut crate::App, name: String) -> iced::Task<crate::Message> {
    let generation = app.epic_updates.asked(&name);
    let paths = app.paths.clone();
    if paths.portable_root().is_some() {
        return iced::Task::none();
    }
    let checked = name.clone();
    crate::task::blocking(
        move || recheck_with(&paths, &checked, &Token::new()),
        move |result| crate::Message::EpicRechecked(name, generation, result),
    )
}

/// What the app knows from Epic about each game. Like `GogState`, every
/// request captures the game's generation when it is asked, and its reply
/// counts only while that generation still holds.
#[derive(Debug, Default)]
pub(crate) struct EpicState {
    statuses: HashMap<String, Status>,
    generations: HashMap<String, u64>,
}

impl EpicState {
    pub(crate) fn status(&self, name: &str) -> Option<&Status> {
        self.statuses.get(name)
    }

    pub(crate) fn asked(&self, name: &str) -> u64 {
        self.generations.get(name).copied().unwrap_or_default()
    }

    pub(crate) fn asked_all(&self) -> HashMap<String, u64> {
        self.generations.clone()
    }

    fn accept(&mut self, name: &str, asked: u64) -> bool {
        if self.asked(name) != asked {
            return false;
        }
        self.generations
            .insert(name.to_owned(), asked.wrapping_add(1));
        true
    }

    /// `name`'s game changed, so nothing known or asked about it before
    /// counts.
    pub(crate) fn changed(&mut self, name: &str) {
        let generation = self.asked(name).wrapping_add(1);
        self.generations.insert(name.to_owned(), generation);
        self.statuses.remove(name);
    }

    pub(crate) fn accept_status(&mut self, name: &str, asked: u64, status: Status) -> bool {
        if !self.accept(name, asked) {
            return false;
        }
        self.statuses.insert(name.to_owned(), status);
        true
    }

    /// A check of every game: one reported takes the new status, one no
    /// longer reported loses its old one, and one whose generation moved on
    /// keeps what it has.
    pub(crate) fn accept_batch(&mut self, asked: &HashMap<String, u64>, statuses: Vec<Status>) {
        let asked_for = |name: &str| asked.get(name).copied().unwrap_or_default();
        let reported: HashSet<String> = statuses
            .iter()
            .map(|status| status.environment.clone())
            .collect();
        let gone: Vec<String> = self
            .statuses
            .keys()
            .filter(|name| !reported.contains(*name))
            .cloned()
            .collect();
        for name in gone {
            if self.accept(&name, asked_for(&name)) {
                self.statuses.remove(&name);
            }
        }
        for status in statuses {
            let name = status.environment.clone();
            if self.accept(&name, asked_for(&name)) {
                self.statuses.insert(name, status);
            }
        }
    }

    /// Seeds a status for a test, as an accepted check would.
    #[cfg(test)]
    pub(crate) fn insert(&mut self, name: &str, status: Status) {
        let asked = self.asked(name);
        assert!(self.accept_status(name, asked, status));
    }
}

/// Lands the background check: statuses mark cards, and problems only go
/// to the terminal, since nobody asked for the check.
pub(crate) fn checked_since(
    app: &mut crate::App,
    asked: &HashMap<String, u64>,
    result: Result<Option<Batch>, String>,
) {
    match result {
        Ok(Some(batch)) => {
            for error in &batch.errors {
                eprintln!(
                    "wineo-gui: couldn't check {} for Epic updates: {}",
                    error.environment, error.message
                );
            }
            app.epic_updates.accept_batch(asked, batch.statuses);
        }
        Ok(None) => {}
        Err(error) => eprintln!("wineo-gui: couldn't check for Epic updates: {error}"),
    }
}

/// Lands a check of one game asked at `generation`.
pub(crate) fn rechecked(
    app: &mut crate::App,
    name: String,
    generation: u64,
    result: Result<Option<Box<Status>>, String>,
) {
    match result {
        Ok(Some(status)) => {
            app.epic_updates.accept_status(&name, generation, *status);
        }
        Ok(None) => {}
        Err(error) => eprintln!("wineo-gui: couldn't check {name} for Epic updates: {error}"),
    }
}

#[cfg(test)]
mod tests;
