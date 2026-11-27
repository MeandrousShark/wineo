//! The games the logged-in Epic account owns, from `list --json -T`,
//! cached for an hour. A fetch replaces the cache only when the whole list
//! was read; a failed one falls back to the last whole list with a warning,
//! like Steam's and GOG's.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::account::{self, Status};
use super::legendary::Legendary;
use super::{EpicLock, Error};
use crate::cancel::Token;
use crate::error::IoContext;
use crate::files::{self, write_unique};
use crate::paths::Paths;

/// How long a fetched list is used before asking again.
const FRESH_SECONDS: i64 = 60 * 60;

const COMMAND: &str = "list --json -T";

/// One owned game, as much of Legendary's entry as wineo reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    /// Epic's app name, which Legendary takes as the game's id.
    pub app_name: String,
    pub title: String,
    /// The current Windows build, if the game has one.
    pub version: Option<String>,
    // Defaults let a saved list from an older or newer wineo still read.
    #[serde(default)]
    pub dlc: Vec<Dlc>,
    /// The publisher's `CanRunOffline` attribute.
    #[serde(default)]
    pub can_run_offline: bool,
    /// The raw `ThirdPartyManagedApp` attribute, naming the launcher that
    /// manages the game.
    pub third_party: Option<String>,
    /// The raw `partnerLinkType` attribute, naming the launcher whose
    /// account the game needs.
    pub partner_link: Option<String>,
    /// The `OwnershipToken` attribute: the game checks a token Legendary
    /// writes, which usually means Denuvo.
    #[serde(default)]
    pub ownership_token: bool,
    /// Epic's art addresses for the game, from `metadata.keyImages`.
    #[serde(default)]
    pub images: Vec<Image>,
}

/// One of Epic's images for a game: its type, like `DieselGameBoxTall`, and
/// where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub kind: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dlc {
    pub app_name: String,
    pub title: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owned {
    pub account: String,
    pub games: Vec<Game>,
    /// `Couldn't refresh owned Epic games: REASON` when an old list stands
    /// in, or a note that the new list couldn't be cached.
    pub warning: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Cache {
    /// Missing reads as 0, long stale, so the list is fetched again.
    #[serde(default)]
    fetched_at: i64,
    account: String,
    games: Vec<Game>,
}

/// Loads the account's owned games. A cache under an hour old is used as
/// is; otherwise, or with `refresh`, the list is fetched and cached. Being
/// logged out is always an error, even with a cache.
pub fn load(legendary: &Legendary, refresh: bool, token: &Token) -> Result<Owned, Error> {
    load_at(
        legendary,
        refresh,
        jiff::Timestamp::now().as_second(),
        token,
    )
}

pub(crate) fn load_at(
    legendary: &Legendary,
    refresh: bool,
    now: i64,
    token: &Token,
) -> Result<Owned, Error> {
    let paths = legendary.paths();
    // Listing logs in and may renew the login, so no login or logout runs
    // meanwhile, and a logout can't have its cache deletion undone.
    let _lock = EpicLock::acquire(paths)?;
    let Status::LoggedIn { account } = account::status(legendary, token)? else {
        return Err(Error::LoggedOut);
    };
    let path = paths.epic_owned_cache();
    let cached = read_cache(&path).filter(|cache| cache.account == account);
    if !refresh
        && let Some(cache) = &cached
        && fresh(cache.fetched_at, now)
    {
        return Ok(Owned {
            account,
            games: cache.games.clone(),
            warning: None,
        });
    }
    let fetched = legendary
        .run(&["list", "--json", "-T"], None, token)
        .and_then(|output| parse_list(&output.stdout));
    match fetched {
        Ok(games) => {
            let cache = Cache {
                fetched_at: now,
                account,
                games,
            };
            let warning = write_cache(&path, &cache)
                .err()
                .map(|reason| format!("Couldn't save owned Epic games: {reason}"));
            Ok(Owned {
                account: cache.account,
                games: cache.games,
                warning,
            })
        }
        Err(error) if error.is_canceled() => Err(error),
        Err(error) => {
            // Legendary deletes the saved login itself when Epic refuses
            // it, which no old list can stand in for.
            if account::status(legendary, token)? == Status::LoggedOut {
                return Err(Error::LoggedOut);
            }
            match cached {
                Some(cache) => Ok(Owned {
                    account,
                    games: cache.games,
                    warning: Some(format!("Couldn't refresh owned Epic games: {error}")),
                }),
                None => Err(Error::Owned(Box::new(error))),
            }
        }
    }
}

/// The last whole list, whatever its age, read without Legendary, for a
/// listing that can't run it and a dry run. Its account is the one that
/// fetched it; a logout removes it.
pub fn cached(paths: &Paths) -> Option<Owned> {
    read_cache(&paths.epic_owned_cache()).map(|cache| Owned {
        account: cache.account,
        games: cache.games,
        warning: None,
    })
}

/// Whether the saved list is under an hour old, so listing would use it
/// without asking Epic. For a dry run, which never asks.
pub fn cache_is_fresh(paths: &Paths) -> bool {
    read_cache(&paths.epic_owned_cache())
        .is_some_and(|cache| fresh(cache.fetched_at, jiff::Timestamp::now().as_second()))
}

fn fresh(fetched_at: i64, now: i64) -> bool {
    now.checked_sub(fetched_at)
        .is_some_and(|age| (0..FRESH_SECONDS).contains(&age))
}

/// Deletes the cached list. Returns whether there was one.
pub fn remove_cache(paths: &Paths) -> crate::Result<bool> {
    let path = paths.epic_owned_cache();
    files::remove_if_present(&path).context(|| format!("failed to remove {}", path.display()))
}

/// A cache that is missing or unreadable is the same as none.
fn read_cache(path: &Path) -> Option<Cache> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn write_cache(path: &Path, cache: &Cache) -> Result<(), String> {
    let text = serde_json::to_string(cache).map_err(|error| error.to_string())?;
    write_unique(path, text.as_bytes()).map_err(|error| error.to_string())
}

/// Reads `list --json -T`. A field wineo reads that is missing or of the
/// wrong type is an error naming it.
pub(crate) fn parse_list(stdout: &str) -> Result<Vec<Game>, Error> {
    let value: Value = serde_json::from_str(stdout).map_err(|_| problem("it isn't JSON"))?;
    let games = value.as_array().ok_or_else(|| problem("it isn't a list"))?;
    games
        .iter()
        .enumerate()
        .map(|(index, game)| parse_game(game, &format!("[{index}]")))
        .collect()
}

fn parse_game(game: &Value, at: &str) -> Result<Game, Error> {
    let app_name = text(game, at, "app_name")?;
    let at = format!("{at} ({app_name})");
    let dlc = game
        .get("dlcs")
        .and_then(Value::as_array)
        .ok_or_else(|| mistyped(&at, "dlcs", "a list"))?
        .iter()
        .enumerate()
        .map(|(index, dlc)| {
            let at = format!("{at}.dlcs[{index}]");
            Ok(Dlc {
                app_name: text(dlc, &at, "app_name")?,
                title: text(dlc, &at, "app_title")?,
                version: windows_version(dlc, &at)?,
            })
        })
        .collect::<Result<_, Error>>()?;
    let metadata = match game.get("metadata") {
        None | Some(Value::Null) => None,
        Some(metadata) => Some(
            metadata
                .as_object()
                .ok_or_else(|| mistyped(&at, "metadata", "an object"))?,
        ),
    };
    let attributes = match metadata {
        None => None,
        Some(metadata) => match metadata.get("customAttributes") {
            None | Some(Value::Null) => None,
            Some(attributes) => Some(
                attributes
                    .as_object()
                    .ok_or_else(|| mistyped(&at, "metadata.customAttributes", "an object"))?,
            ),
        },
    };
    let attribute = |name: &str| -> Result<Option<String>, Error> {
        let Some(entry) = attributes.and_then(|attributes| attributes.get(name)) else {
            return Ok(None);
        };
        entry
            .get("value")
            .and_then(Value::as_str)
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| {
                mistyped(
                    &at,
                    &format!("metadata.customAttributes.{name}.value"),
                    "text",
                )
            })
    };
    Ok(Game {
        title: text(game, &at, "app_title")?,
        version: windows_version(game, &at)?,
        dlc,
        can_run_offline: attribute("CanRunOffline")?
            .is_some_and(|value| value.eq_ignore_ascii_case("true")),
        third_party: attribute("ThirdPartyManagedApp")?,
        partner_link: attribute("partnerLinkType")?,
        ownership_token: attribute("OwnershipToken")?
            .is_some_and(|value| value.eq_ignore_ascii_case("true")),
        images: metadata.map_or(Ok(Vec::new()), |metadata| images(metadata, &at))?,
        app_name,
    })
}

/// `metadata.keyImages`. Art is extra, so an entry without a type or an
/// address is skipped rather than failing the list.
fn images(metadata: &serde_json::Map<String, Value>, at: &str) -> Result<Vec<Image>, Error> {
    let images = match metadata.get("keyImages") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(images) => images
            .as_array()
            .ok_or_else(|| mistyped(at, "metadata.keyImages", "a list"))?,
    };
    Ok(images
        .iter()
        .filter_map(|image| {
            let field = |name: &str| {
                image
                    .get(name)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            };
            Some(Image {
                kind: field("type")?,
                url: field("url")?,
            })
        })
        .collect())
}

/// `asset_infos.Windows.build_version`, when there is a Windows build.
fn windows_version(entry: &Value, at: &str) -> Result<Option<String>, Error> {
    let assets = entry
        .get("asset_infos")
        .and_then(Value::as_object)
        .ok_or_else(|| mistyped(at, "asset_infos", "an object"))?;
    let Some(windows) = assets.get("Windows") else {
        return Ok(None);
    };
    windows
        .get("build_version")
        .and_then(Value::as_str)
        .map(|version| Some(version.to_owned()))
        .ok_or_else(|| mistyped(at, "asset_infos.Windows.build_version", "text"))
}

fn text(entry: &Value, at: &str, field: &str) -> Result<String, Error> {
    entry
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| mistyped(at, field, "text"))
}

fn mistyped(at: &str, field: &str, kind: &str) -> Error {
    problem(&format!("{at}: `{field}` is missing or isn't {kind}"))
}

fn problem(problem: &str) -> Error {
    Error::Output {
        command: COMMAND.to_owned(),
        problem: problem.to_owned(),
    }
}

#[cfg(test)]
mod tests;
