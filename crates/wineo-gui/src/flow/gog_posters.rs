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
use wineo_core::gog::covers::{self, CoverError};
use wineo_core::paths::Paths;

use super::posters;

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

/// How many covers in a row may fail because the network or GOG is down
/// before the rest are left alone for this load.
const OUTAGE_LIMIT: u32 = 3;

/// Loads each job's poster and hands it to `emit`: everything that needs no
/// network first, so stored art and covers already cached show up before
/// the first download. A game with no stored art emits nothing. Stops when
/// `token` is canceled, and stops fetching after `OUTAGE_LIMIT` covers in a
/// row fail because the network or GOG is down, so an outage doesn't make a
/// big library wait out hundreds of timeouts.
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
        match job.source {
            Source::Stored(environment) => match stored_grid(paths, &environment) {
                Ok(Some(path)) => emit(job.id, decode(paths, &path)),
                Ok(None) => {}
                Err(error) => emit(job.id, Err(error)),
            },
            Source::Cover(gog_id) => match covers::cached(paths, gog_id) {
                Some(path) => emit(job.id, decode_cover(paths, gog_id, &path)),
                None => missing.push((job.id, gog_id)),
            },
        }
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
    let mut outages = 0;
    for (id, gog_id) in missing {
        if token.is_canceled() || outages >= OUTAGE_LIMIT {
            return;
        }
        let poster = match covers::cover(paths, &network.client, &*network.transport, gog_id, token)
        {
            Ok(path) => {
                outages = 0;
                decode_cover(paths, gog_id, &path)
            }
            Err(error) => {
                if matches!(error, CoverError::Unavailable(_)) {
                    outages += 1;
                } else {
                    outages = 0;
                }
                Err(error.to_string())
            }
        };
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

fn decode(paths: &Paths, path: &Path) -> Result<Handle, String> {
    posters::decode(path, &paths.art_cache_dir()).map(|(handle, _)| handle)
}

/// Decodes a cached cover. One that can't be decoded, like a truncated
/// download, is removed so the next load fetches it again instead of failing
/// on it forever. Art an environment stored is never removed.
fn decode_cover(paths: &Paths, gog_id: u64, path: &Path) -> Result<Handle, String> {
    let poster = decode(paths, path);
    if poster.is_err() {
        let _ = covers::discard(paths, gog_id);
    }
    poster
}
