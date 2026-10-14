//! The games an account owns, from Steam's Web API, cached for an hour.
//! Installed games never need this: they come from the local scan.

use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::api::{ApiError, SteamApi};
use super::{Install, active_steam_id, is_tool};
use crate::files::write_unique;
use crate::paths::Paths;

const PATH: &str = "IPlayerService/GetOwnedGames/v1/";

/// How long a fetched list is used before asking again.
const FRESH_SECONDS: i64 = 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedGame {
    pub app_id: String,
    pub name: String,
}

/// Why the list couldn't be fetched, for people to read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OwnedError {
    #[error("{0}")]
    Api(#[from] ApiError),
    /// Steam answers an empty `response` when the key can't see the library.
    #[error("the key can't see this library")]
    Hidden,
    #[error("Steam's answer isn't shaped as expected")]
    Malformed,
    /// The active account couldn't be chosen.
    #[error("{0}")]
    Account(String),
}

/// A failed load with nothing cached to fall back on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Couldn't get owned games: {0}")]
pub struct LoadError(pub OwnedError);

/// An owned list, and a note for the user when it isn't a fresh one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owned {
    pub games: Vec<OwnedGame>,
    /// `Couldn't refresh owned games: REASON` when an old list stands in, or
    /// a note that the new list couldn't be cached.
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    /// There is no Steam Web API key, so no list was looked for.
    NoKey,
    Owned(Owned),
}

#[derive(Serialize, Deserialize)]
struct Cache {
    fetched_at: i64,
    games: Vec<OwnedGame>,
}

/// Fetches the account's owned games, without touching the cache. An empty
/// `response` is `Hidden`, and `game_count: 0` is a real, empty library.
pub fn fetch(api: &SteamApi, key: &str, steam_id: u64) -> Result<Vec<OwnedGame>, OwnedError> {
    let steam_id = steam_id.to_string();
    let json = api.get_json(
        PATH,
        &[
            ("steamid", &steam_id),
            ("include_appinfo", "1"),
            ("include_played_free_games", "1"),
            ("include_free_sub", "1"),
        ],
        key,
    )?;
    parse(&json)
}

fn parse(json: &Value) -> Result<Vec<OwnedGame>, OwnedError> {
    let response = json
        .get("response")
        .and_then(Value::as_object)
        .ok_or(OwnedError::Malformed)?;
    let count = response
        .get("game_count")
        .ok_or(OwnedError::Hidden)?
        .as_u64()
        .ok_or(OwnedError::Malformed)?;
    let Some(games) = response.get("games") else {
        return if count == 0 {
            Ok(Vec::new())
        } else {
            Err(OwnedError::Malformed)
        };
    };
    games
        .as_array()
        .ok_or(OwnedError::Malformed)?
        .iter()
        .map(|game| {
            let app_id = game
                .get("appid")
                .and_then(Value::as_u64)
                .ok_or(OwnedError::Malformed)?
                .to_string();
            // Without include_appinfo's name, the id is all there is to show.
            let name = game
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map_or_else(|| format!("App {app_id}"), str::to_owned);
            Ok(OwnedGame { app_id, name })
        })
        .collect()
}

/// Checks a key before it is saved, with one `GetOwnedGames` call for the
/// active account. A 200 isn't enough: the answer must carry `game_count`.
pub fn check_key(api: &SteamApi, install: &Install, key: &str) -> Result<(), OwnedError> {
    let steam_id = active_steam_id(install).map_err(OwnedError::Account)?;
    check_key_for(api, steam_id, key)
}

pub fn check_key_for(api: &SteamApi, steam_id: u64, key: &str) -> Result<(), OwnedError> {
    fetch(api, key, steam_id).map(|_| ())
}

/// Loads the owned list once for a command. A cache under an hour old is
/// used as is; otherwise (or with `refresh`) the list is fetched and cached.
/// A failed fetch falls back to the old cache with a warning, or fails when
/// there is none. The key is the caller's to find: without one, nothing is
/// loaded, even if a cache exists.
pub fn load(
    paths: &Paths,
    steam_id: u64,
    key: Option<&str>,
    api: &SteamApi,
    refresh: bool,
) -> Result<Loaded, LoadError> {
    load_at(
        paths,
        steam_id,
        key,
        api,
        refresh,
        jiff::Timestamp::now().as_second(),
    )
}

/// `load` for a dry run: the same answers, but a fetched list is never saved,
/// so nothing on disk changes.
pub fn load_without_saving(
    paths: &Paths,
    steam_id: u64,
    key: Option<&str>,
    api: &SteamApi,
    refresh: bool,
) -> Result<Loaded, LoadError> {
    load_inner(
        paths,
        steam_id,
        key,
        api,
        refresh,
        jiff::Timestamp::now().as_second(),
        false,
    )
}

fn load_at(
    paths: &Paths,
    steam_id: u64,
    key: Option<&str>,
    api: &SteamApi,
    refresh: bool,
    now: i64,
) -> Result<Loaded, LoadError> {
    load_inner(paths, steam_id, key, api, refresh, now, true)
}

fn load_inner(
    paths: &Paths,
    steam_id: u64,
    key: Option<&str>,
    api: &SteamApi,
    refresh: bool,
    now: i64,
    save: bool,
) -> Result<Loaded, LoadError> {
    let Some(key) = key else {
        return Ok(Loaded::NoKey);
    };
    let path = paths.steam_owned_cache(steam_id);
    let cached = read_cache(&path);
    if !refresh
        && let Some(cache) = &cached
        && now
            .checked_sub(cache.fetched_at)
            .is_some_and(|age| (0..FRESH_SECONDS).contains(&age))
    {
        return Ok(owned(cache.games.clone(), None));
    }
    match fetch(api, key, steam_id) {
        Ok(games) => {
            let cache = Cache {
                fetched_at: now,
                games,
            };
            let warning = if save {
                write_cache(&path, &cache)
                    .err()
                    .map(|reason| format!("Couldn't save owned games: {reason}"))
            } else {
                None
            };
            Ok(owned(cache.games, warning))
        }
        // An error never replaces the cache, which includes an empty
        // `response`.
        Err(reason) => match cached {
            Some(cache) => Ok(owned(
                cache.games,
                Some(format!("Couldn't refresh owned games: {reason}")),
            )),
            None => Err(LoadError(reason)),
        },
    }
}

/// Applies 0047's tools filter, so Proton and the runtimes never show up.
fn owned(games: Vec<OwnedGame>, warning: Option<String>) -> Loaded {
    Loaded::Owned(Owned {
        games: games
            .into_iter()
            .filter(|game| !is_tool(&game.app_id, &game.name))
            .collect(),
        warning,
    })
}

/// A cache that is missing or unreadable is the same as none.
fn read_cache(path: &std::path::Path) -> Option<Cache> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn write_cache(path: &std::path::Path, cache: &Cache) -> Result<(), String> {
    let text = serde_json::to_string(cache).map_err(|error| error.to_string())?;
    write_unique(path, text.as_bytes()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
