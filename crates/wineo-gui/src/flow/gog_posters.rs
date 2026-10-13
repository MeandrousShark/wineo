//! Posters for the GOG import grid. A game wineo already has shows the Grid
//! art it stored; any other shows GOG's box art from a host-only cache,
//! fetched at most once. One worker walks the games in turn, so a library of
//! hundreds never opens more than one connection, and a failure only leaves
//! that game's text tile.

use std::path::{Path, PathBuf};

use iced::widget::image::Handle;
use wineo_core::art::{self, ArtConfig, Kind};
use wineo_core::cancel::Token;
use wineo_core::download::{Https, Transport};
use wineo_core::gog::api::Client;
use wineo_core::gog::covers;
use wineo_core::paths::Paths;

use super::artwork::preview_with_format;

/// Where a game's poster comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Source {
    /// The Grid art stored for this environment.
    Stored(String),
    /// GOG's box art for this game id.
    Cover(u64),
}

/// A tile's poster to load, under the id its tile looks it up by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Job {
    pub id: String,
    pub source: Source,
}

/// What reaching GOG takes.
pub(super) struct Network {
    pub client: Client,
    pub transport: Box<dyn Transport>,
}

impl Network {
    pub(super) fn open() -> Result<Self, String> {
        Ok(Self {
            client: wineo_core::gog::client_from_env().map_err(|error| error.to_string())?,
            transport: Box::new(Https::new().map_err(|error| error.to_string())?),
        })
    }
}

/// Loads each job's poster and hands it to `emit`: everything that needs no
/// network first, so stored art and covers already cached show up before
/// the first download. A game with no stored art emits nothing. Stops when
/// `token` is canceled.
pub(super) fn run(
    paths: &Paths,
    jobs: Vec<Job>,
    token: &Token,
    network: impl FnOnce() -> Result<Network, String>,
    mut emit: impl FnMut(String, Result<Handle, String>),
) {
    let mut missing = Vec::new();
    for job in jobs {
        if token.is_canceled() {
            return;
        }
        let path = match job.source {
            Source::Stored(environment) => match stored_grid(paths, &environment) {
                Ok(Some(path)) => path,
                Ok(None) => continue,
                Err(error) => {
                    emit(job.id, Err(error));
                    continue;
                }
            },
            Source::Cover(gog_id) => match covers::cached(paths, gog_id) {
                Some(path) => path,
                None => {
                    missing.push((job.id, gog_id));
                    continue;
                }
            },
        };
        emit(job.id, decode(&path));
    }
    if missing.is_empty() {
        return;
    }
    let network = match network() {
        Ok(network) => network,
        Err(error) => {
            for (id, _) in missing {
                emit(id, Err(error.clone()));
            }
            return;
        }
    };
    for (id, gog_id) in missing {
        if token.is_canceled() {
            return;
        }
        let poster = covers::cover(paths, &network.client, &*network.transport, gog_id, token)
            .map_err(|error| format!("{error:#}"))
            .and_then(|path| decode(&path));
        emit(id, poster);
    }
}

/// The Grid art an environment resolves to: art.toml's record, then the
/// store's format order, like everywhere else art is shown.
fn stored_grid(paths: &Paths, environment: &str) -> Result<Option<PathBuf>, String> {
    let dir = paths.art_dir(environment);
    let config = ArtConfig::load(&dir).map_err(|error| error.to_string())?;
    let resolved =
        art::resolve(&dir, &config, None, Kind::Grid).map_err(|error| error.to_string())?;
    Ok(resolved.map(|resolved| resolved.found.path))
}

fn decode(path: &Path) -> Result<Handle, String> {
    preview_with_format(path, Kind::Grid).map(|(handle, _)| handle)
}
