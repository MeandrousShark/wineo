//! The games a GOG account owns, cached for an hour per account. A fetch
//! replaces the cache only when every page arrived; a failed one falls back
//! to the last complete list with a warning, like Steam's.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use wineo_gog::{OwnedProduct, UserId};

use super::account::{AccountError, CallError, Session};
use crate::error::IoContext;
use crate::files::write_unique;
use crate::paths::Paths;

/// How long a fetched list is used before asking again.
const FRESH_SECONDS: i64 = 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owned {
    pub games: Vec<OwnedProduct>,
    /// `Couldn't refresh owned GOG games: REASON` when an old list stands
    /// in, or a note that the new list couldn't be cached.
    pub warning: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// Logged out, or the login couldn't be read or renewed, with no cached
    /// list to stand in.
    #[error(transparent)]
    Account(AccountError),
    #[error("Couldn't get owned GOG games: {0}")]
    Fetch(wineo_gog::Error),
}

#[derive(Serialize, Deserialize)]
struct Cache {
    fetched_at: i64,
    user_id: UserId,
    games: Vec<OwnedProduct>,
}

/// Loads the account's owned games. A cache under an hour old is used as is;
/// otherwise, or with `refresh`, the list is fetched and cached. Being
/// logged out is always an error, even with a cache.
pub fn load(session: &mut Session, refresh: bool) -> Result<Owned, LoadError> {
    load_at(session, refresh, jiff::Timestamp::now().as_second())
}

fn load_at(session: &mut Session, refresh: bool, now: i64) -> Result<Owned, LoadError> {
    // A session left over from another account never reads its cache.
    session.check().map_err(LoadError::Account)?;
    let user_id = session.user_id();
    let path = session.paths().gog_owned_cache(user_id.get());
    let cached = read_cache(&path).filter(|cache| cache.user_id == user_id);
    if !refresh
        && let Some(cache) = &cached
        && now
            .checked_sub(cache.fetched_at)
            .is_some_and(|age| (0..FRESH_SECONDS).contains(&age))
    {
        return Ok(Owned {
            games: cache.games.clone(),
            warning: None,
        });
    }
    // Page by page through the session, so the login is checked around
    // every request and a logout stops the next page from being asked for.
    let pages = wineo_gog::collect_pages(|page| {
        session.call(|client, token| client.owned_page(token, page))
    });
    let fetched = match pages {
        Ok(games) => Ok(games),
        Err(CallError::Request(error)) => Err((error.to_string(), LoadError::Fetch(error))),
        Err(CallError::Account(
            error @ (AccountError::Temporary(_) | AccountError::Gog(_) | AccountError::Changed),
        )) => Err((error.to_string(), LoadError::Account(error))),
        Err(CallError::Account(error)) => return Err(LoadError::Account(error)),
    };
    match fetched {
        Ok(games) => {
            let cache = Cache {
                fetched_at: now,
                user_id,
                games,
            };
            // Published under the credentials lock, so a logout that ran
            // during the fetch can't have its deletion undone.
            let warning = session
                .while_current(|| write_cache(&path, &cache))
                .map_err(LoadError::Account)?
                .err()
                .map(|reason| format!("Couldn't save owned GOG games: {reason}"));
            Ok(Owned {
                games: cache.games,
                warning,
            })
        }
        Err((reason, error)) => match cached {
            Some(cache) => {
                // The copy in memory was read before the fetch.
                session.while_current(|| ()).map_err(LoadError::Account)?;
                Ok(Owned {
                    games: cache.games,
                    warning: Some(format!("Couldn't refresh owned GOG games: {reason}")),
                })
            }
            None => Err(error),
        },
    }
}

/// Deletes an account's cached list. Returns whether there was one.
pub fn remove_cache(paths: &Paths, user_id: UserId) -> crate::Result<bool> {
    let path = paths.gog_owned_cache(user_id.get());
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context(|| format!("failed to remove {}", path.display())),
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
