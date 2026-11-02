//! The opt-in import of new Steam installs. After a scan, with the setting on,
//! one background pass takes the pending games through
//! `steam_import::auto::run`. What a game may do is decided there, by its
//! claim in the record, so this only keeps the pass single and the banner
//! honest.

use std::collections::HashSet;

use iced::Task;
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::library as saved;
use wineo_core::paths::Paths;
use wineo_core::steam::{Discovery, PendingGame};
use wineo_core::steam_import::auto::{self, Report};

use crate::{App, Message, rescan, task};

#[derive(Debug, Default)]
pub(crate) struct AutoImport {
    /// The pass or claim retry running, never more than one between them.
    running: Option<Token>,
    /// The last pass couldn't write the record, and that's been said.
    stuck: bool,
    /// Claims this session couldn't take back. Only these are retried, since
    /// any other claim may be another window's, live.
    unreleased: HashSet<String>,
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

    /// A write of the record went through.
    pub(crate) fn recovered(&mut self) {
        self.stuck = false;
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
        let closing = self.closing || self.exiting;
        let on = discovery.auto_import && self.paths.portable_root().is_none() && !closing;
        self.steam_nudge.hold(on && !self.steam_auto.stuck);
        if closing || self.steam_auto.running() {
            return Task::none();
        }
        if !self.steam_auto.unreleased.is_empty() {
            return self.retry_claims();
        }
        if !on {
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

    /// Takes back what a pass couldn't, whether or not anything is pending.
    /// It holds the pass's place, so the pass waits and a close does too.
    fn retry_claims(&mut self) -> Task<Message> {
        let token = Token::new();
        self.steam_auto.running = Some(token.clone());
        let ids: Vec<String> = self.steam_auto.unreleased.iter().cloned().collect();
        let (paths, watch) = (self.paths.clone(), self.shown.clone());
        task::blocking_value(
            move || release_claims(&paths, &watch, &ids, &token),
            Message::ClaimsReleased,
        )
    }

    /// A claim retry finished. A released game is pending again, and the read
    /// that shows it offers it if it failed and passes it on if it didn't.
    /// What's left has nothing to bring it back to a scan but this, and a
    /// failure stays quiet: the record's trouble has been said already.
    pub(crate) fn claims_released(&mut self, result: Result<Vec<String>, String>) -> Task<Message> {
        self.steam_auto.running = None;
        if self.closing || self.exiting {
            return Task::none();
        }
        let released = result.unwrap_or_default();
        for id in &released {
            self.steam_auto.unreleased.remove(id);
        }
        if !self.steam_auto.unreleased.is_empty() {
            self.shown.discovery_due();
        }
        if released.is_empty() {
            return Task::none();
        }
        self.steam_auto.recovered();
        let reads = self.outdate_reads();
        Task::batch([reads, self.reload_library_once()])
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
        if !report.unreleased.is_empty() {
            self.steam_auto
                .unreleased
                .extend(report.unreleased.iter().cloned());
            // Nothing pending changes, so a scan has no other reason to report.
            self.shown.discovery_due();
        }
        if let Some(error) = &report.stopped {
            if !self.steam_auto.stuck {
                self.flag_notice(format!("Couldn't import new Steam games: {error}"));
            }
            self.steam_auto.stuck = true;
            self.steam_nudge.hold(false);
            if !report.claimed() {
                return Task::none();
            }
        } else if report.claimed() && report.unreleased.is_empty() {
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

/// Tries to take back `ids`, then the rescan's cache rereads the file. Gives
/// the ones that are settled. `release_claim` leaves anything but a claim, so
/// a dismissal made since counts as settled and stays.
pub(crate) fn release_claims(
    paths: &Paths,
    watch: &rescan::Watch,
    ids: &[String],
    cancel: &Token,
) -> Vec<String> {
    let released = ids
        .iter()
        .filter(|id| !cancel.is_canceled() && saved::release_claim(paths, id).is_ok())
        .cloned()
        .collect();
    watch.refresh_library(paths);
    released
}

#[cfg(test)]
mod tests;
