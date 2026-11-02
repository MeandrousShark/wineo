//! The opt-in import of new Steam installs. A scan's list of pending games is
//! only a hint: each game is claimed in the record first, in a locked edit
//! that reads the setting again, so a dismissal or a change of mind anywhere
//! else wins, and a record that can't be written imports nothing.

use super::{Failure, OwnedGames, PlanOutcome, apply, plan};
use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::heroic_import::default_name;
use crate::library::{self, Claim};
use crate::paths::Paths;
use crate::steam::PendingGame;

/// How a pass went. Only games this pass claimed are listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// App ids with an environment now, whoever made it.
    pub imported: Vec<String>,
    /// Games that went back to pending.
    pub failures: Vec<Failure>,
    /// Why the pass gave up, with the record unwritable or the config
    /// unreadable.
    pub stopped: Option<String>,
    pub canceled: bool,
}

impl Report {
    /// A claim was written, so the record can be written.
    pub fn claimed(&self) -> bool {
        !self.imported.is_empty() || !self.failures.is_empty()
    }
}

/// Imports `games` in order with `import steam --all`'s defaults: installed
/// games only, so no owned list, no key and no network, and the default name.
/// `each` hears each game's app id as it's finished with, whatever became of
/// it. Cancellation stops at the next game.
pub fn run(
    paths: &Paths,
    global: &GlobalConfig,
    games: &[PendingGame],
    cancel: &Token,
    mut each: impl FnMut(&str),
) -> Report {
    let mut report = Report::default();
    if paths.portable_root().is_some() {
        return report;
    }
    for game in games {
        if cancel.is_canceled() {
            report.canceled = true;
            break;
        }
        match library::claim_game(paths, &game.app_id, |library| library.steam.auto_import) {
            Ok(Claim::Claimed) => import_claimed(paths, global, game, cancel, &mut report),
            Ok(Claim::Recorded | Claim::Refused) => {}
            Err(error) => {
                let cause = crate::library_lifecycle::cause(&error);
                report.stopped = Some(format!("can't update library.toml: {cause}"));
                break;
            }
        }
        each(&game.app_id);
    }
    report
}

fn import_claimed(
    paths: &Paths,
    global: &GlobalConfig,
    game: &PendingGame,
    cancel: &Token,
    report: &mut Report,
) {
    let app_id = &game.app_id;
    let imported = plan(paths, global, OwnedGames::Unloaded, app_id, None).and_then(|outcome| {
        match outcome {
            // `apply` settles the claim itself, whichever way it goes.
            PlanOutcome::Ready(plan) => {
                apply(paths, global, OwnedGames::Unloaded, &plan, cancel).map(|_| ())
            }
            // A failed write leaves the claim, which keeps the game off the
            // banner just the same, and the environment is what counts.
            PlanOutcome::AlreadyImported(_) => {
                let _ = library::finish_import(paths, app_id);
                Ok(())
            }
        }
    });
    match imported {
        Ok(()) => report.imported.push(app_id.clone()),
        Err(error) => {
            // A claim that can't be taken back keeps the game unoffered, the
            // safe way to fail, and Add > Import from Steam still lists it.
            let _ = library::release_claim(paths, app_id);
            if cancel.is_canceled() {
                report.canceled = true;
            } else {
                report.failures.push(Failure {
                    app_id: app_id.clone(),
                    name: default_name(&game.title, None),
                    reason: format!("{error:#}"),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
