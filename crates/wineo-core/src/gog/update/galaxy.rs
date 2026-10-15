//! Galaxy builds: installing a Windows game from GOG's content system, file
//! by file, rather than running its offline installer.
//!
//! The folder's state file stays at version 1 and authoritative. What it
//! has no room for, the installed manifest and an operation's target, lives
//! in a record beside it (`record`). Support files go to a support folder
//! per game in the data directory, and redistributables to a store shared
//! by every game (`store`).
//!
//! Nothing here is reachable from the CLI or the app yet: they still call
//! `install::plan` for the offline installers. `plan` here picks the track
//! as installs will once they do.

mod build;
mod fetch;
pub mod install;
pub mod record;
mod setup;
mod store;

use anyhow::Result;

pub use build::LayoutError;
pub use install::{GalaxyError, PlannedProduct, Progress, Retarget};

use super::install as offline;
use crate::cancel::Token;
use crate::gog::account::Session;
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

/// Plans installing `request.gog_id` on `track`, or, with none, on the
/// Galaxy track when it's a Windows install and GOG has a generation 2
/// build on the default branch, else from its offline installer. An
/// interrupted install carries on on the track it began on. An explicit
/// installer track is `install::plan` unchanged.
pub fn plan(
    paths: &Paths,
    session: &mut Session,
    request: &offline::Request,
    track: Option<Track>,
    token: &Token,
) -> Result<Planned> {
    host_only(paths)?;
    if let Some(pending) = offline::own_pending(paths, request)? {
        let began = pending.journal.as_ref().map(|journal| journal.target.track);
        return Ok(match began {
            Some(Track::Galaxy) => Planned::Galaxy(install::plan(paths, session, request, token)?),
            _ => Planned::Installer(offline::plan(paths, session, request, token)?),
        });
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
                let builds = session
                    .call(|client, access| client.windows_builds(id, Some(access)))
                    .map_err(anyhow::Error::from)?;
                build::latest(&builds, None).is_some()
            }
        }
    };
    Ok(if galaxy {
        Planned::Galaxy(install::plan(paths, session, request, token)?)
    } else {
        Planned::Installer(offline::plan(paths, session, request, token)?)
    })
}

#[cfg(test)]
mod tests;
