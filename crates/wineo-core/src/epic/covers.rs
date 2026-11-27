//! Epic's box art for games a user owns, kept in a host-only cache so the
//! import grid can show posters before wineo has stored any art for a game,
//! like GOG's covers.
//!
//! The address comes from the owned list, so nothing here asks Legendary or
//! needs a login. A cover is fetched at most once: later calls find the file
//! and never download it again.

use std::fs;
use std::io;
use std::path::PathBuf;

use crate::art::{Format, Kind};
use crate::cancel::Token;
use crate::download::Transport;
use crate::files::{self, write_atomic};
use crate::gog::covers::CoverError;
use crate::paths::Paths;
use crate::steamgriddb::{DownloadRead, DownloadStatus};

use super::owned::Image;

/// A cover's file name without its extension. App names can hold anything,
/// so they never name a file themselves.
fn stem(app_name: &str) -> String {
    super::game::key(app_name)
}

fn candidates(paths: &Paths, app_name: &str) -> impl Iterator<Item = PathBuf> {
    let dir = paths.epic_covers_dir();
    let stem = stem(app_name);
    Format::ALL
        .into_iter()
        .filter(|format| Kind::Grid.accepts(*format))
        .map(move |format| dir.join(format!("{stem}.{}", format.extension())))
}

/// The cached cover for a game, if one was fetched before. Reads only.
pub fn cached(paths: &Paths, app_name: &str) -> Option<PathBuf> {
    candidates(paths, app_name)
        .find(|path| fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()))
}

/// Removes a game's cached cover, so the next load fetches it again. For a
/// file that was saved but can't be decoded.
pub fn discard(paths: &Paths, app_name: &str) -> io::Result<()> {
    for path in candidates(paths, app_name) {
        files::remove_if_present(&path)?;
    }
    Ok(())
}

/// The cached cover for a game, fetching it from the owned list's poster
/// address first when there is none. Only image formats a Grid accepts are
/// kept, and nothing is written for a failed or non-image answer.
pub fn cover(
    paths: &Paths,
    transport: &dyn Transport,
    app_name: &str,
    images: &[Image],
    token: &Token,
) -> Result<PathBuf, CoverError> {
    if let Some(path) = cached(paths, app_name) {
        return Ok(path);
    }
    let Some(url) = super::art::link(images, Kind::Grid) else {
        return Err(CoverError::Skipped(anyhow::anyhow!(
            "Epic has no cover for {app_name}"
        )));
    };
    let (bytes, format) = crate::gog::extras::fetch_image(transport, url, Kind::Grid, token)
        .map_err(|error| {
            if outage(&error) {
                CoverError::Unavailable(error)
            } else {
                CoverError::Skipped(error)
            }
        })?;
    let path = paths
        .epic_covers_dir()
        .join(format!("{}.{}", stem(app_name), format.extension()));
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
