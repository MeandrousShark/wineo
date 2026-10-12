//! GOG product details, cached for an hour per product. They are the same
//! for every account, so they are fetched without a login and kept across
//! logouts.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use wineo_gog::{Client, Product};

use crate::files::write_unique;
use crate::paths::Paths;

/// How long fetched details are used before asking again.
const FRESH_SECONDS: i64 = 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Details {
    pub product: Product,
    /// `Couldn't refresh GOG product details: REASON` when old details stand
    /// in, or a note that the new ones couldn't be cached.
    pub warning: Option<String>,
}

/// A failed fetch with nothing cached to fall back on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Couldn't get GOG product details: {0}")]
pub struct LoadError(pub wineo_gog::Error);

#[derive(Serialize, Deserialize)]
struct Cache {
    fetched_at: i64,
    product: Product,
}

/// Loads a product's details. A cache under an hour old is used as is;
/// otherwise, or with `refresh`, they are fetched and cached. A failed fetch
/// falls back to the old cache with a warning.
pub fn load(paths: &Paths, client: &Client, id: u64, refresh: bool) -> Result<Details, LoadError> {
    load_with(paths, client, id, refresh, true)
}

/// Like `load`, but with `save` off nothing is written: fetched details are
/// used and not cached, for a dry run.
pub fn load_with(
    paths: &Paths,
    client: &Client,
    id: u64,
    refresh: bool,
    save: bool,
) -> Result<Details, LoadError> {
    load_at(
        paths,
        client,
        id,
        refresh,
        save,
        jiff::Timestamp::now().as_second(),
    )
}

fn load_at(
    paths: &Paths,
    client: &Client,
    id: u64,
    refresh: bool,
    save: bool,
    now: i64,
) -> Result<Details, LoadError> {
    let path = paths.gog_product_cache(id);
    let cached = read_cache(&path).filter(|cache| cache.product.id == id);
    if !refresh
        && let Some(cache) = &cached
        && now
            .checked_sub(cache.fetched_at)
            .is_some_and(|age| (0..FRESH_SECONDS).contains(&age))
    {
        return Ok(Details {
            product: cache.product.clone(),
            warning: None,
        });
    }
    match client.product(id, None) {
        Ok(product) => {
            let cache = Cache {
                fetched_at: now,
                product,
            };
            let warning = if save {
                write_cache(&path, &cache)
                    .err()
                    .map(|reason| format!("Couldn't save GOG product details: {reason}"))
            } else {
                None
            };
            Ok(Details {
                product: cache.product,
                warning,
            })
        }
        Err(error) => fallback(cached, error),
    }
}

fn fallback(cached: Option<Cache>, error: wineo_gog::Error) -> Result<Details, LoadError> {
    match cached {
        Some(cache) => Ok(Details {
            product: cache.product,
            warning: Some(format!("Couldn't refresh GOG product details: {error}")),
        }),
        None => Err(LoadError(error)),
    }
}

/// A cache that is missing or unreadable is the same as none.
fn read_cache(path: &Path) -> Option<Cache> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn write_cache(path: &Path, cache: &Cache) -> Result<(), String> {
    let text = serde_json::to_string(cache).map_err(|error| error.to_string())?;
    write_unique(path, text.as_bytes()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
