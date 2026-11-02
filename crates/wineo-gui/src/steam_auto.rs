//! The opt-in import of new Steam installs. After a scan, with the setting on,
//! one background pass takes the pending games through
//! `steam_import::auto::run`. What a game may do is decided there, by its
//! claim in the record, so this only keeps the pass single and the banner
//! honest.

use iced::Task;
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::paths::Paths;
use wineo_core::steam::{Discovery, PendingGame};
use wineo_core::steam_import::auto::{self, Report};

use crate::{App, Message, rescan, task};

#[derive(Debug, Default)]
pub(crate) struct AutoImport {
    /// The pass running, which there's never more than one of.
    running: Option<Token>,
    /// The last pass couldn't write the record, and that's been said.
    stuck: bool,
}

impl AutoImport {
    pub(crate) fn running(&self) -> bool {
        self.running.is_some()
    }

    /// Stops a pass at its next game, for a closing window. It reports back,
    /// so the close can wait for it.
    pub(crate) fn cancel(&self) {
        if let Some(token) = &self.running {
            token.cancel();
        }
    }
}

impl App {
    /// Starts a pass for what `discovery` found, unless one is running: its
    /// finish reads again, so nothing found meanwhile is lost.
    ///
    /// This runs before the banner is given the list. With the setting on,
    /// pending games are the pass's and the banner keeps only those a pass
    /// gave up on, so a game passing through is never drawn and `show` needs
    /// no claim. While the record can't be written the banner keeps them all,
    /// since nothing's going to take them.
    pub(crate) fn auto_import_read(&mut self, discovery: &Discovery) -> Task<Message> {
        let on = discovery.auto_import
            && self.paths.portable_root().is_none()
            && !self.closing
            && !self.exiting;
        self.steam_nudge.hold(on && !self.steam_auto.stuck);
        if !on || self.steam_auto.running() {
            return Task::none();
        }
        let games: Vec<PendingGame> = discovery
            .pending
            .iter()
            .filter(|game| self.steam_nudge.takes(&game.app_id))
            .cloned()
            .collect();
        if games.is_empty() {
            return Task::none();
        }
        let token = Token::new();
        self.steam_auto.running = Some(token.clone());
        let (paths, watch) = (self.paths.clone(), self.shown.clone());
        task::blocking_value(
            move || run_pass(&paths, &watch, &games, &token),
            Message::AutoImported,
        )
    }

    /// A pass finished. Failed games stay out of later passes until the next
    /// launch and go on the banner. Reads from before now are old, and the
    /// fresh one shows the new cards and starts on anything found meanwhile.
    /// A pass that got nowhere doesn't read again, since that read would only
    /// start the same pass: the next scan to reach the app tries it.
    pub(crate) fn auto_imported(&mut self, result: Result<Report, String>) -> Task<Message> {
        self.steam_auto.running = None;
        if self.closing || self.exiting {
            return Task::none();
        }
        let report = result.unwrap_or_else(|error| Report {
            stopped: Some(error),
            ..Report::default()
        });
        self.steam_nudge
            .gave_up(report.failures.iter().map(|failure| &failure.app_id));
        if let Some(error) = &report.stopped {
            if !self.steam_auto.stuck {
                self.flag_notice(format!("Couldn't import new Steam games: {error}"));
            }
            self.steam_auto.stuck = true;
            self.steam_nudge.hold(false);
            if !report.claimed() {
                return Task::none();
            }
        } else if report.claimed() {
            self.steam_auto.stuck = false;
        }
        let reads = self.outdate_reads();
        Task::batch([reads, self.reload_library_once()])
    }
}

/// One pass, then the rescan's cache rereads the file.
pub(crate) fn run_pass(
    paths: &Paths,
    watch: &rescan::Watch,
    games: &[PendingGame],
    cancel: &Token,
) -> Report {
    let report = match GlobalConfig::load(paths) {
        Ok(global) => auto::run(paths, &global, games, cancel, |_| {}),
        Err(error) => Report {
            stopped: Some(error.to_string()),
            ..Report::default()
        },
    };
    watch.refresh_library(paths);
    report
}

#[cfg(test)]
mod tests;
