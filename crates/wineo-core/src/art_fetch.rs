//! Fetching an environment's art from SteamGridDB, one kind at a time.

use anyhow::{Result, anyhow, bail};

use crate::art::{self, Downloaded, Kind, Source, Stored, detect, find, spot_dir};
use crate::cancel::Token;
use crate::environment::EnvironmentIdentity;
use crate::error::Error;
use crate::paths::Paths;
use crate::steamgriddb::{Api, Filters, Image, rank};

/// Which kinds a fetch may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only kinds without art.
    Fill,
    /// Every kind, manual and unrecorded art included.
    Replace,
    /// This one asset, replacing whatever the kind has.
    Asset(u64),
}

/// How fetching one kind went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Stored {
        asset: u64,
        stored: Stored,
    },
    /// The kind already has art. None is unrecorded art.
    Skipped {
        source: Option<Source>,
    },
    /// Nothing usable was left after the filters.
    NoneFound,
    /// `Mode::Asset` named an image the game's filtered list doesn't have.
    AssetNotFound,
}

/// Fetches `kind` for `target`, or the environment for None, from `game`.
/// Candidates are tried best first, and one whose bytes are not a format the
/// kind takes is passed over for the next.
#[allow(clippy::too_many_arguments)]
pub fn fetch_kind(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    api: &Api,
    game: u64,
    language: &str,
    mode: Mode,
) -> Result<Fetched> {
    fetch_kind_for(
        paths,
        env,
        target,
        kind,
        api,
        game,
        language,
        mode,
        None,
        &Token::new(),
    )
}

/// Like `fetch_kind`, except that nothing is written unless the environment
/// is still the one `expected_identity` was read from, and `cancel` stops
/// the download and anything after it. Both are checked under the art lock,
/// after the download.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fetch_kind_for(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    api: &Api,
    game: u64,
    language: &str,
    mode: Mode,
    expected_identity: Option<EnvironmentIdentity>,
    cancel: &Token,
) -> Result<Fetched> {
    if cancel.is_canceled() {
        return Err(crate::Error::Canceled("download").into());
    }
    art::check_spot(paths, env, target)?;
    if mode == Mode::Fill
        && let Some(skipped) = existing(paths, env, target, kind)?
    {
        return Ok(skipped);
    }
    let image = match choose_for(api, game, kind, language, mode, cancel)? {
        Choice::Image(image) => image,
        Choice::NoneFound => return Ok(Fetched::NoneFound),
        Choice::AssetNotFound => return Ok(Fetched::AssetNotFound),
    };
    publish_with_cancel(
        paths,
        env,
        target,
        kind,
        &image,
        mode,
        Some(cancel),
        expected_identity,
    )
}

/// Fetches SteamGridDB art for missing kinds, using Steam's app ID to find the
/// corresponding SteamGridDB game. A per-kind failure is returned alongside
/// that kind so later kinds can still be tried.
pub fn fetch_steam(
    paths: &Paths,
    env: &str,
    app_id: u64,
    api: &Api<'_>,
    language: &str,
    kinds: &[Kind],
) -> Result<Vec<(Kind, Result<Fetched>)>> {
    let mut missing = Vec::new();
    for &kind in kinds {
        if existing(paths, env, None, kind)?.is_none() {
            missing.push(kind);
        }
    }
    if missing.is_empty() {
        return Ok(Vec::new());
    }

    let game = api
        .steam_game(app_id)?
        .ok_or_else(|| anyhow!("SteamGridDB has no game for Steam app {app_id}"))?;
    art::save_game(paths, env, game.id)?;

    Ok(missing
        .into_iter()
        .map(|kind| {
            (
                kind,
                fetch_kind(paths, env, None, kind, api, game.id, language, Mode::Fill),
            )
        })
        .collect())
}

/// Chooses and downloads the best usable ranked image without an environment.
pub fn choose(api: &Api, game: u64, kind: Kind, language: &str) -> Result<Option<Downloaded>> {
    match choose_for(api, game, kind, language, Mode::Fill, &Token::new())? {
        Choice::Image(image) => Ok(Some(image)),
        Choice::NoneFound | Choice::AssetNotFound => Ok(None),
    }
}

/// Keeps existing art intact until the selected image is downloaded and validated.
#[allow(clippy::too_many_arguments)]
pub fn use_image(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    api: &Api,
    image: &Image,
    progress: impl FnMut(u64, Option<u64>),
    cancel: &Token,
    expected_identity: Option<EnvironmentIdentity>,
) -> Result<Fetched> {
    art::check_spot(paths, env, target)?;
    let bytes = api.download_with_progress(&image.url, progress, cancel)?;
    let Some(format) = detect(&bytes).filter(|format| kind.accepts(*format)) else {
        bail!("asset {} is not a {} image", image.id, art::accepted(kind));
    };
    if cancel.is_canceled() {
        return Err(Error::Canceled("download").into());
    }
    publish_with_cancel(
        paths,
        env,
        target,
        kind,
        &Downloaded {
            asset: image.id,
            bytes,
            format,
        },
        Mode::Replace,
        Some(cancel),
        expected_identity,
    )
}

enum Choice {
    Image(Downloaded),
    NoneFound,
    AssetNotFound,
}

fn choose_for(
    api: &Api,
    game: u64,
    kind: Kind,
    language: &str,
    mode: Mode,
    cancel: &Token,
) -> Result<Choice> {
    // The user picked an asset by hand, so none of ranking's filters apply.
    let filters = match mode {
        Mode::Asset(_) => Filters::None,
        Mode::Fill | Mode::Replace => Filters::Ranked,
    };
    let mut images = api.images_with_cancel(game, kind, filters, cancel)?;
    if let Mode::Asset(asset) = mode {
        images.retain(|image| image.id == asset);
        if images.is_empty() {
            return Ok(Choice::AssetNotFound);
        }
    }
    rank(kind, &mut images, language);
    for image in images {
        let bytes = api.download_with_progress(&image.url, |_, _| {}, cancel)?;
        let Some(format) = detect(&bytes).filter(|format| kind.accepts(*format)) else {
            if let Mode::Asset(asset) = mode {
                bail!("asset {asset} is not a {} image", art::accepted(kind));
            }
            continue;
        };
        return Ok(Choice::Image(Downloaded {
            asset: image.id,
            bytes,
            format,
        }));
    }
    Ok(Choice::NoneFound)
}

/// Publishes a downloaded image, keeping existing art in Fill mode.
pub fn publish(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    image: &Downloaded,
    mode: Mode,
) -> Result<Fetched> {
    publish_with_cancel(paths, env, target, kind, image, mode, None, None)
}

#[allow(clippy::too_many_arguments)]
fn publish_with_cancel(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    image: &Downloaded,
    mode: Mode,
    cancel: Option<&Token>,
    expected_identity: Option<EnvironmentIdentity>,
) -> Result<Fetched> {
    art::check_spot(paths, env, target)?;
    if mode == Mode::Fill
        && let Some(skipped) = existing(paths, env, target, kind)?
    {
        return Ok(skipped);
    }
    let published = match cancel {
        Some(cancel) => art::publish_fetched_with_cancel(
            paths,
            env,
            target,
            kind,
            image,
            mode != Mode::Fill,
            Some(cancel),
            expected_identity,
        )?,
        None => match expected_identity {
            Some(expected_identity) => art::publish_fetched_with_cancel(
                paths,
                env,
                target,
                kind,
                image,
                mode != Mode::Fill,
                None,
                Some(expected_identity),
            )?,
            None => art::publish_fetched(paths, env, target, kind, image, mode != Mode::Fill)?,
        },
    };
    Ok(match published {
        Some(stored) => Fetched::Stored {
            asset: image.asset,
            stored,
        },
        None => existing(paths, env, target, kind)?.unwrap_or(Fetched::Skipped { source: None }),
    })
}

/// The skip for a kind that already has art, if it has any.
pub(crate) fn existing(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
) -> Result<Option<Fetched>> {
    let art_dir = paths.art_dir(env);
    let config = art::ArtConfig::load(&art_dir)?;
    let record = config.record(target, kind);
    let found = find(&spot_dir(&art_dir, target), kind, record.map(|r| r.format))?;
    Ok(found.map(|found| Fetched::Skipped {
        source: record
            .filter(|record| record.format == found.format)
            .map(|record| record.source.clone()),
    }))
}

#[cfg(test)]
mod tests;
