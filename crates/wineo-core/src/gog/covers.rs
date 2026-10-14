//! GOG's box art for games a user owns, kept in a host-only cache so a
//! library can show posters before wineo has stored any art for a game.
//!
//! The cover endpoint is public, so no login is needed. A cover is fetched at
//! most once: later calls find the file and never ask GOG.

use std::fs;
use std::io;
use std::path::PathBuf;

use wineo_gog::{Class, Client};

use super::extras::{fetch_image, link};
use crate::art::{Format, Kind};
use crate::cancel::Token;
use crate::download::Transport;
use crate::files::{self, write_atomic};
use crate::paths::Paths;
use crate::steamgriddb::{DownloadRead, DownloadStatus};

/// The cached cover for a game, if one was fetched before. Reads only.
pub fn cached(paths: &Paths, gog_id: u64) -> Option<PathBuf> {
    let dir = paths.gog_covers_dir();
    Format::ALL
        .into_iter()
        .filter(|format| Kind::Grid.accepts(*format))
        .map(|format| dir.join(format!("{gog_id}.{}", format.extension())))
        .find(|path| fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()))
}

/// Why a cover couldn't be had.
#[derive(Debug, thiserror::Error)]
pub enum CoverError {
    /// The network or GOG's servers are failing (no connection, a timeout, a
    /// 429 or 5xx), so asking for more covers right now is likely to fail
    /// the same way.
    #[error("{0:#}")]
    Unavailable(anyhow::Error),
    /// Only this game's cover is out of reach: GOG has none, it isn't an
    /// image, or the fetch was canceled.
    #[error("{0:#}")]
    Skipped(anyhow::Error),
}

/// Removes a game's cached cover, so the next load fetches it again. For a
/// file that was saved but can't be decoded. Only the exact `<id>.<ext>`
/// names of the cache are touched, and `gog_id` is a number, so it can't
/// name anything else.
pub fn discard(paths: &Paths, gog_id: u64) -> io::Result<()> {
    let dir = paths.gog_covers_dir();
    for format in Format::ALL
        .into_iter()
        .filter(|format| Kind::Grid.accepts(*format))
    {
        files::remove_if_present(&dir.join(format!("{gog_id}.{}", format.extension())))?;
    }
    Ok(())
}

/// The cached cover for a game, fetching it first when there is none. Only
/// image formats a Grid accepts are kept, and nothing is written for a
/// failed or non-image answer.
///
/// `Client::game_art` takes no token, so a cancel is honored before it and
/// again before the image is downloaded, not during the art request.
pub fn cover(
    paths: &Paths,
    client: &Client,
    transport: &dyn Transport,
    gog_id: u64,
    token: &Token,
) -> Result<PathBuf, CoverError> {
    if let Some(path) = cached(paths, gog_id) {
        return Ok(path);
    }
    let canceled = || CoverError::Skipped(anyhow::anyhow!("canceled"));
    if token.is_canceled() {
        return Err(canceled());
    }
    let links = client.game_art(gog_id).map_err(|error| {
        let outage = error.class() == Class::Temporary;
        let error = anyhow::Error::new(error);
        if outage {
            CoverError::Unavailable(error)
        } else {
            CoverError::Skipped(error)
        }
    })?;
    let Some(url) = link(&links, Kind::Grid) else {
        return Err(CoverError::Skipped(anyhow::anyhow!(
            "GOG has no cover for game {gog_id}"
        )));
    };
    if token.is_canceled() {
        return Err(canceled());
    }
    let (bytes, format) = fetch_image(transport, url, Kind::Grid, token).map_err(|error| {
        if outage(&error) {
            CoverError::Unavailable(error)
        } else {
            CoverError::Skipped(error)
        }
    })?;
    let path = paths
        .gog_covers_dir()
        .join(format!("{gog_id}.{}", format.extension()));
    write_atomic(&path, bytes).map_err(|error| CoverError::Skipped(error.into()))?;
    Ok(path)
}

/// Whether an image download failed because the server or network did.
fn outage(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<DownloadStatus>()
        .is_some_and(|status| status.0 == 429 || status.0 >= 500)
        || error.downcast_ref::<DownloadRead>().is_some()
        || matches!(
            error.downcast_ref::<crate::error::Error>(),
            Some(crate::error::Error::Network(_))
        )
}

#[cfg(test)]
mod tests;
