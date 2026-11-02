//! The record of Steam games wineo has already offered or won't offer, kept in
//! `library.toml`'s `[steam]` table. The permanent answer to "has this game
//! been dealt with?", since a removed environment leaves nothing else behind.
//!
//! Every write is one locked `edit`, so additions from two processes merge and
//! a decision made elsewhere is seen before anything replaces it.

use std::collections::{BTreeMap, BTreeSet};

use toml_edit::{Array, DocumentMut, Item, Table, Value};

use super::{Library, edit, read_library, store_value};
use crate::error::Result;
use crate::paths::Paths;

/// Why a game is in the record. Any reason keeps it from being offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// It was already installed when its library was first scanned.
    Baseline,
    Dismissed,
    Imported,
    /// An import has it and hasn't finished, so nothing else starts one.
    Claimed,
}

impl Recorded {
    pub(super) const NAMES: &str = "baseline, dismissed, imported, claimed";

    pub(super) fn from_str(value: &str) -> Option<Self> {
        match value {
            "baseline" => Some(Self::Baseline),
            "dismissed" => Some(Self::Dismissed),
            "imported" => Some(Self::Imported),
            "claimed" => Some(Self::Claimed),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Dismissed => "dismissed",
            Self::Imported => "imported",
            Self::Claimed => "claimed",
        }
    }
}

/// The `[steam]` table as read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SteamRecord {
    /// By app id, as text.
    pub games: BTreeMap<String, Recorded>,
    /// Libraries whose installed games have been recorded, by the path
    /// `steam::libraries` gives.
    pub baselined: BTreeSet<String>,
}

impl SteamRecord {
    pub fn contains(&self, app_id: &str) -> bool {
        self.games.contains_key(app_id)
    }
}

/// What one scan wants added, applied in one write so a library is never
/// marked done without its games.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordChanges {
    pub baseline: Vec<String>,
    pub imported: Vec<String>,
    pub libraries: Vec<String>,
}

impl RecordChanges {
    pub fn is_empty(&self) -> bool {
        self.baseline.is_empty() && self.imported.is_empty() && self.libraries.is_empty()
    }
}

/// How a claim went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    Claimed,
    /// Already in the record, whoever put it there.
    Recorded,
    /// The check said no, so nothing was written.
    Refused,
}

/// Adds the games that aren't in the record yet. An entry that's there keeps
/// its reason, so a baseline can't replace a dismissal.
pub fn record_games(paths: &Paths, ids: &[String], reason: Recorded) -> Result<bool> {
    edit(paths, |document| add_games(document, ids, reason))
}

/// Applies a scan's proposals. See `RecordChanges`.
pub fn apply_changes(paths: &Paths, changes: &RecordChanges) -> Result<bool> {
    if changes.is_empty() {
        return Ok(false);
    }
    edit(paths, |document| {
        let imported = add_games(document, &changes.imported, Recorded::Imported);
        let baseline = add_games(document, &changes.baseline, Recorded::Baseline);
        let mut marked = false;
        for library in &changes.libraries {
            marked |= add_baselined(document, library);
        }
        imported || baseline || marked
    })
}

/// Records a dismissal, which replaces a claim.
pub fn dismiss_game(paths: &Paths, app_id: &str) -> Result<bool> {
    edit(paths, |document| {
        settle(document, app_id, Recorded::Dismissed)
    })
}

/// Records dismissals for all of `ids` in one locked write, so a crash can't
/// leave some of them and another window's choice is seen first. Each id
/// follows `dismiss_game`'s rule.
pub fn dismiss_games(paths: &Paths, ids: &[String]) -> Result<bool> {
    edit(paths, |document| {
        let mut changed = false;
        for id in ids {
            changed |= settle(document, id, Recorded::Dismissed);
        }
        changed
    })
}

/// Records a finished import, which replaces a claim.
pub fn finish_import(paths: &Paths, app_id: &str) -> Result<bool> {
    edit(paths, |document| {
        settle(document, app_id, Recorded::Imported)
    })
}

/// Claims a game for an import about to start, unless it's in the record or
/// `check` says no. `check` runs on the file as read under the lock, so a
/// decision made anywhere else since is seen. An id the record can't hold is
/// refused, since a claim that isn't written protects nothing.
pub fn claim_game(
    paths: &Paths,
    app_id: &str,
    check: impl FnOnce(&Library) -> bool,
) -> Result<Claim> {
    if !valid_app_id(app_id) {
        return Ok(Claim::Refused);
    }
    let mut claim = Claim::Recorded;
    edit(paths, |document| {
        if entry(document, app_id).is_some() {
            return false;
        }
        if !check(&read_library(document)) {
            claim = Claim::Refused;
            return false;
        }
        claim = Claim::Claimed;
        add_games(document, &[app_id.to_owned()], Recorded::Claimed)
    })?;
    Ok(claim)
}

/// Takes back a claim after a failed import. Anything else in its place, like
/// a dismissal made while the import ran, stays.
pub fn release_claim(paths: &Paths, app_id: &str) -> Result<bool> {
    edit(paths, |document| {
        entry(document, app_id) == Some(Recorded::Claimed)
            && games(document).is_some_and(|games| games.remove(app_id).is_some())
    })
}

/// Marks a library's baseline as done.
pub fn mark_baselined(paths: &Paths, library: &str) -> Result<bool> {
    edit(paths, |document| add_baselined(document, library))
}

pub(super) fn valid_app_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// The record as `load` shows it. Anything invalid reads as absent.
pub(super) fn read_record(document: &DocumentMut) -> SteamRecord {
    let Some(steam) = document.get("steam").and_then(Item::as_table_like) else {
        return SteamRecord::default();
    };
    let games = steam
        .get("games")
        .and_then(Item::as_table_like)
        .map(|games| {
            games
                .iter()
                .filter(|(id, _)| valid_app_id(id))
                .filter_map(|(id, item)| Some((id.to_owned(), reason(item)?)))
                .collect()
        })
        .unwrap_or_default();
    let baselined = steam
        .get("baselined")
        .and_then(Item::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    SteamRecord { games, baselined }
}

fn reason(item: &Item) -> Option<Recorded> {
    item.as_str().and_then(Recorded::from_str)
}

fn entry(document: &DocumentMut, app_id: &str) -> Option<Recorded> {
    document
        .get("steam")
        .and_then(Item::as_table_like)?
        .get("games")
        .and_then(Item::as_table_like)?
        .get(app_id)
        .and_then(reason)
}

/// Sets a game's final reason. Only a claim or an absent game changes: the
/// other reasons are all final, and the first one stands.
fn settle(document: &mut DocumentMut, app_id: &str, reason: Recorded) -> bool {
    if !valid_app_id(app_id) || entry(document, app_id).is_some_and(|it| it != Recorded::Claimed) {
        return false;
    }
    store_value(games_mut(document), app_id, Value::from(reason.as_str()));
    true
}

/// An entry that isn't valid is replaced, like any other invalid value a
/// writer finds.
fn add_games(document: &mut DocumentMut, ids: &[String], reason: Recorded) -> bool {
    let mut changed = false;
    for id in ids {
        if valid_app_id(id) && entry(document, id).is_none() {
            store_value(games_mut(document), id, Value::from(reason.as_str()));
            changed = true;
        }
    }
    changed
}

fn add_baselined(document: &mut DocumentMut, library: &str) -> bool {
    if read_record(document).baselined.contains(library) {
        return false;
    }
    let steam = steam_mut(document);
    if steam.get("baselined").is_none_or(|item| !item.is_array()) {
        store_value(steam, "baselined", Value::Array(Array::new()));
    }
    steam
        .get_mut("baselined")
        .and_then(Item::as_array_mut)
        .expect("baselined was just made an array")
        .push(library);
    true
}

fn games(document: &mut DocumentMut) -> Option<&mut dyn toml_edit::TableLike> {
    document
        .get_mut("steam")
        .and_then(Item::as_table_like_mut)?
        .get_mut("games")
        .and_then(Item::as_table_like_mut)
}

fn games_mut(document: &mut DocumentMut) -> &mut Table {
    sub_table(steam_mut(document), "games")
}

fn steam_mut(document: &mut DocumentMut) -> &mut Table {
    sub_table(document.as_table_mut(), "steam")
}

/// `table_mut` that turns a hand-written inline table into a real one, since
/// `games` can't be nested inside an inline table.
fn sub_table<'a>(parent: &'a mut Table, name: &str) -> &'a mut Table {
    let implicit = || {
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    };
    let item = parent.entry(name).or_insert_with(implicit);
    if !item.is_table() {
        *item = std::mem::take(item)
            .into_table()
            .map_or_else(|_| implicit(), Item::Table);
    }
    item.as_table_mut().expect("item was just made a table")
}

#[cfg(test)]
mod tests;
