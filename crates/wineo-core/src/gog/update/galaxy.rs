//! Galaxy builds: installing a Windows game from GOG's content system, file
//! by file, rather than running its offline installer.
//!
//! The folder's state file stays at version 1 and authoritative. What it
//! has no room for, the installed manifest and an operation's target, lives
//! in a record beside it (`record`). Support files go to a support folder
//! per game in the data directory, and redistributables to a store shared
//! by every game (`store`).
//!
//! `plan` picks the track an install takes, and `plan_update` sends a Galaxy
//! game's update to `update` and every other game's to the installers'.
//! `present` words reviews and progress for both front ends.

mod build;
mod fetch;
pub mod install;
pub mod present;
pub mod record;
mod setup;
mod store;
pub mod update;

use anyhow::Result;

pub use build::LayoutError;
pub(super) use build::latest_default as latest;
pub use install::{GalaxyError, PlannedProduct, Progress, Retarget};

use super::UpdateError;
use super::apply::{self, Options};
use super::install as offline;
use crate::cancel::Token;
use crate::environment::Environment;
use crate::gog::account::{AccountError, CallError, Session};
use crate::gog::folder::{Platform, Track};
use crate::gog::migrate::host_only;
use crate::gog::product;
use crate::paths::Paths;

/// An install planned on one track or the other.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Planned {
    Installer(offline::Plan),
    Galaxy(install::Plan),
}

/// The track an install of `request.gog_id` takes: the one an interrupted
/// install began on, else `track`, else Galaxy when it's a Windows install
/// and GOG has a generation 2 build on the default branch, else the
/// offline installers. Asks GOG for its builds only for that last choice,
/// so a frontend can choose first and plan with the answer.
pub fn track_for(
    paths: &Paths,
    session: &mut Session,
    request: &offline::Request,
    track: Option<Track>,
) -> Result<Track> {
    host_only(paths)?;
    if let Some(pending) = offline::own_pending(paths, request)? {
        return Ok(
            match pending.journal.as_ref().map(|journal| journal.target.track) {
                Some(Track::Galaxy) => Track::Galaxy,
                _ => Track::Installer,
            },
        );
    }
    let galaxy = match (track, request.platform) {
        (Some(track), _) => track == Track::Galaxy,
        (None, Some(Platform::Linux)) => false,
        (None, platform) => {
            let details = product::load_with(
                paths,
                session.client(),
                request.gog_id,
                request.refresh,
                !request.read_only,
            )?;
            let native = details
                .product
                .installers
                .iter()
                .any(|installer| installer.os == "linux");
            // Natively when GOG has a Linux installer and no platform was
            // asked for, as offline installs choose.
            if platform.is_none() && native {
                false
            } else {
                let id = request.gog_id;
                // A game GOG has no content system for answers 404, which
                // is the installers' to install, not a failure.
                match session.call(|client, access| client.windows_builds(id, Some(access))) {
                    Ok(builds) => build::latest(&builds, None).is_some(),
                    Err(CallError::Request(error))
                        if error.kind() == wineo_gog::ErrorKind::Status(404) =>
                    {
                        false
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
    };
    Ok(if galaxy {
        Track::Galaxy
    } else {
        Track::Installer
    })
}

/// Plans installing `request.gog_id` on `track`, or, with none, on the
/// track `track_for` chooses. An interrupted install carries on on the
/// track it began on. An explicit installer track is `install::plan`
/// unchanged.
pub fn plan(
    paths: &Paths,
    session: &mut Session,
    request: &offline::Request,
    track: Option<Track>,
    token: &Token,
) -> Result<Planned> {
    Ok(match track_for(paths, session, request, track)? {
        Track::Galaxy => Planned::Galaxy(install::plan(paths, session, request, token)?),
        Track::Installer => Planned::Installer(offline::plan(paths, session, request, token)?),
    })
}

/// An update planned on the game's track.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum PlannedUpdate {
    Installer(apply::Plan),
    Galaxy(update::Planned),
}

/// Plans updating `name`'s GOG game on the track it follows: a Galaxy
/// Windows game, or its interrupted Galaxy operation, through
/// `update::plan`, which needs a session; anything else through
/// `apply::plan`, unchanged.
///
/// A reinstall asking for a track moves the game onto it: onto Galaxy
/// through `update::plan`, off it through `apply::plan`'s reinstall. An
/// interrupted operation is finished on the track its journal targets,
/// whatever the options say.
pub fn plan_update(
    paths: &Paths,
    session: Option<&mut Session>,
    name: &str,
    options: Options,
    token: &Token,
) -> Result<PlannedUpdate> {
    host_only(paths)?;
    options.check()?;
    Environment::ensure_exists(paths, name)?;
    let state = super::game_of(paths, name)?;
    let (platform, track, journaled) = match (&state.journal, &state.committed) {
        (Some(journal), _) => (journal.target.platform, journal.target.track, true),
        (None, Some(committed)) => (committed.platform, committed.track, false),
        (None, None) => {
            return Ok(PlannedUpdate::Installer(apply::plan(
                paths, session, name, options, token,
            )?));
        }
    };
    let galaxy = if journaled {
        platform == Platform::Windows && track == Track::Galaxy
    } else {
        let to_galaxy = options.reinstall && options.track == Some(Track::Galaxy);
        match (platform, track) {
            (Platform::Windows, Track::Galaxy) => {
                !(options.reinstall && options.track == Some(Track::Installer))
            }
            (Platform::Windows, Track::Installer) if to_galaxy => true,
            (Platform::Windows, Track::Installer) if options.repair => {
                return Err(UpdateError::NotGalaxy {
                    name: name.to_owned(),
                }
                .into());
            }
            (Platform::Linux, _) if options.repair || to_galaxy => {
                return Err(GalaxyError::NotWindows.into());
            }
            _ => false,
        }
    };
    if galaxy {
        let session = session.ok_or(AccountError::LoggedOut)?;
        return Ok(PlannedUpdate::Galaxy(update::plan(
            paths, session, name, options, token,
        )?));
    }
    Ok(PlannedUpdate::Installer(apply::plan(
        paths, session, name, options, token,
    )?))
}

#[cfg(test)]
mod tests;
