//! The library's offer of new Steam installs: the banner, Import through the
//! usual Steam review, and Dismiss. Pending games are worked out by each scan
//! and held only in memory; the record on disk only ever gains decisions.

use std::collections::HashSet;
use std::ops::ControlFlow;

use iced::widget::row;
use iced::{Element, Task};
use wineo_core::library as saved;
use wineo_core::paths::Paths;
use wineo_core::steam::PendingGame;

use crate::{App, Early, Message, flow, focus_button, rescan, task, ui};

pub(crate) const IMPORT_ID: &str = "library-steam-nudge-import";
pub(crate) const DISMISS_ID: &str = "library-steam-nudge-dismiss";

/// The games the banner offers, and whether a dismissal is being written.
#[derive(Debug, Default)]
pub(crate) struct Nudge {
    pending: Vec<PendingGame>,
    /// A dismissal is being written, so the banner's buttons wait for it.
    writing: bool,
    /// Games dismissed this session. A dismissal is permanent, but a result
    /// read before it can still arrive after (a flow's reload, say), so
    /// anything that shows pending games is filtered through this.
    dismissed: HashSet<String>,
}

impl Nudge {
    pub(crate) fn show(&mut self, mut pending: Vec<PendingGame>) {
        pending.retain(|game| !self.dismissed.contains(&game.app_id));
        self.pending = pending;
    }

    /// Remembers `ids` as dismissed and takes them off the banner.
    fn dismissed(&mut self, ids: &[String]) {
        self.dismissed.extend(ids.iter().cloned());
        let left = std::mem::take(&mut self.pending);
        self.show(left);
    }

    pub(crate) fn pending(&self) -> &[PendingGame] {
        &self.pending
    }

    #[cfg(test)]
    pub(crate) fn writing(&self) -> bool {
        self.writing
    }
}

/// The banner's words.
pub(crate) fn words(games: &[PendingGame]) -> String {
    match games {
        [game] => format!("1 new Steam game: {}", game.title),
        games => format!("{} new Steam games", games.len()),
    }
}

fn ids(games: &[PendingGame]) -> Vec<String> {
    games.iter().map(|game| game.app_id.clone()).collect()
}

impl App {
    /// The games the banner offers, if it shows.
    pub(crate) fn steam_offer(&self) -> Option<&[PendingGame]> {
        let games = self.steam_nudge.pending();
        (!games.is_empty() && self.library_problem.is_none()).then_some(games)
    }

    /// The offer above the grid, if a scan found installs that wineo hasn't
    /// dealt with. A `library.toml` that can't be used hides it, since neither
    /// choice could be recorded.
    pub(crate) fn steam_banner(&self) -> Option<Element<'_, Message>> {
        let games = self.steam_offer()?;
        let palette = self.palette;
        // The buttons carry the ids the banner shows, so a game that arrives
        // between drawing and pressing is never swept up by the press.
        let shown = ids(games);
        let importable = !self.management_busy() && self.operation.is_none();
        let idle = !self.steam_nudge.writing;
        let import = Message::NudgeImport(shown.clone());
        let dismiss = Message::NudgeDismiss(shown);
        let actions = row![
            focus_button::control(
                ui::accent_control(
                    "Import",
                    palette.primary,
                    importable && idle,
                    import.clone()
                )
                .into(),
                importable && idle,
                IMPORT_ID,
                import,
            ),
            focus_button::control(
                ui::secondary_button("Dismiss", palette)
                    .on_press_maybe(idle.then(|| dismiss.clone()))
                    .into(),
                idle,
                DISMISS_ID,
                dismiss,
            ),
        ]
        .spacing(8);
        Some(self.strip(palette.primary, words(games), Some(actions.into())))
    }

    /// Opens the Steam review for `ids`, the games the banner showed.
    pub(crate) fn nudge_import(&mut self, ids: Vec<String>) -> Early {
        if self.management_busy()
            || self.operation.is_some()
            || self.settings.is_some()
            || self.art_page.is_some()
            || self.flow.is_some()
            || self.section != crate::sidebar::Section::Library
            || self.steam_nudge.writing
        {
            return ControlFlow::Break(Task::none());
        }
        self.close_log();
        self.action_menu = None;
        self.flow_generation = self.flow_generation.wrapping_add(1);
        let generation = self.flow_generation;
        let (page, task) = flow::Page::steam_games(self.paths.clone(), ids);
        self.flow = Some(page);
        ControlFlow::Continue(Task::batch([
            task.map(move |event| Message::Flow(generation, event)),
            iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus()),
        ]))
    }

    /// Records `ids` as dismissed in one write. They stay offered until it
    /// succeeds.
    pub(crate) fn nudge_dismiss(&mut self, ids: Vec<String>) -> Early {
        if ids.is_empty() || self.steam_nudge.writing {
            return ControlFlow::Break(Task::none());
        }
        self.steam_nudge.writing = true;
        let (paths, watch) = (self.paths.clone(), self.shown.clone());
        let done = ids.clone();
        ControlFlow::Continue(task::blocking(
            move || dismiss(&paths, &watch, &ids),
            move |result| Message::NudgeDismissed(done, result),
        ))
    }

    /// The write answered. Reads from before it are old, so a scan can't bring
    /// a dismissed game back; a failure leaves everything offered, with one
    /// notice and no retry.
    pub(crate) fn nudge_dismissed(
        &mut self,
        ids: &[String],
        result: Result<(), String>,
    ) -> Task<Message> {
        self.steam_nudge.writing = false;
        match result {
            Ok(()) => {
                self.steam_nudge.dismissed(ids);
                self.shown.drop_pending(ids);
                self.outdate_reads()
            }
            Err(error) => {
                self.flag_notice(format!("Couldn't dismiss the Steam games: {error}"));
                Task::none()
            }
        }
    }
}

fn dismiss(paths: &Paths, watch: &rescan::Watch, ids: &[String]) -> Result<(), String> {
    let result = saved::dismiss_games(paths, ids);
    watch.refresh_library(paths);
    result.map(|_| ()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
