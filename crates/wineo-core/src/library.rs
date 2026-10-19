//! Lenient display state for environments.
//!
//! Every write sets one field under the file lock, so a window never writes
//! back state it merely read. `Ok(false)` means the file wasn't touched.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

use crate::error::{Error, IoContext, Result};
use crate::operation::Locks;
use crate::paths::Paths;

mod cache;
mod check;
mod repair;
mod sort;

pub use cache::Cache;
pub(crate) use check::{FindingKind, inspect, remove_strays};
pub use repair::{RepairReport, ViewRecovery, repair};
pub use sort::{SortFields, compare};

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortKey {
    #[default]
    Name,
    LastPlayed,
    Playtime,
    Added,
    Source,
}

impl SortKey {
    pub const ALL: [Self; 5] = [
        Self::Name,
        Self::LastPlayed,
        Self::Playtime,
        Self::Added,
        Self::Source,
    ];

    /// Whether the key starts descending when picked: names run A to Z, and
    /// the rest lead with the most recent, most played, or newest.
    pub fn starts_descending(self) -> bool {
        self != Self::Name
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "name" => Some(Self::Name),
            "last-played" => Some(Self::LastPlayed),
            "playtime" => Some(Self::Playtime),
            "added" => Some(Self::Added),
            "source" => Some(Self::Source),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    pub sort: SortKey,
    pub descending: bool,
    pub favorites_first: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub favorite: bool,
    pub hidden: bool,
    pub added: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub reason: String,
    pub line: Option<usize>,
    pub newer_version: bool,
}

impl Problem {
    /// What `validate` and Checkup say about the file.
    pub fn message(&self) -> String {
        if self.newer_version {
            self.reason.clone()
        } else {
            unreadable_message(self.line, &self.reason)
        }
    }
}

/// `library.toml can't be read (line N: reason)`, shared with `validate`.
pub(crate) fn unreadable_message(line: Option<usize>, reason: &str) -> String {
    let reason = reason.trim();
    match line {
        Some(line) => format!("library.toml can't be read (line {line}: {reason})"),
        None => format!("library.toml can't be read ({reason})"),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    pub view: View,
    pub environments: BTreeMap<String, Entry>,
    pub problem: Option<Problem>,
}

/// Why the file can't be used. `newer` says a newer wineo wrote it: its
/// `schema_version` is above this one's, even when the rest doesn't parse,
/// so there's nothing to repair, only wineo to update.
enum ReadFailure {
    Io(io::Error),
    NotUtf8 {
        newer: bool,
    },
    Parse {
        error: toml_edit::TomlError,
        line: Option<usize>,
        newer: bool,
    },
    Schema {
        found: String,
        newer: bool,
    },
}

const NEWER: &str = "library.toml was written by a newer wineo; update wineo to use it.";
const NOT_UTF8: &str = "stream did not contain valid UTF-8";

impl ReadFailure {
    fn newer(&self) -> bool {
        match self {
            Self::Io(_) => false,
            Self::NotUtf8 { newer } | Self::Parse { newer, .. } | Self::Schema { newer, .. } => {
                *newer
            }
        }
    }

    /// The one classification every reader shares, so the banner, a
    /// refused write, and `validate` always agree on whether to repair.
    fn problem(&self) -> Problem {
        let (reason, line) = match self {
            _ if self.newer() => {
                return Problem {
                    reason: NEWER.to_owned(),
                    line: None,
                    newer_version: true,
                };
            }
            Self::Io(error) => (error.to_string(), None),
            Self::NotUtf8 { .. } => (NOT_UTF8.to_owned(), None),
            Self::Parse { error, line, .. } => (error.message().trim().to_owned(), *line),
            Self::Schema { found, .. } => {
                (format!("unsupported schema_version {}", found.trim()), None)
            }
        };
        Problem {
            reason,
            line,
            newer_version: false,
        }
    }
}

/// Reads display state, falling back to defaults when the file can't be used.
pub fn load(paths: &Paths) -> Library {
    library_from(read_document(&paths.library_file()))
}

fn library_from(read: std::result::Result<Option<DocumentMut>, ReadFailure>) -> Library {
    match read {
        Ok(Some(document)) => read_library(&document),
        Ok(None) => Library::default(),
        Err(failure) => Library {
            problem: Some(failure.problem()),
            ..Library::default()
        },
    }
}

pub fn set_favorite(paths: &Paths, name: &str, favorite: bool) -> Result<bool> {
    edit(paths, |document| {
        set_environment_bool(document, name, "favorite", favorite)
    })
}

pub fn set_hidden(paths: &Paths, name: &str, hidden: bool) -> Result<bool> {
    edit(paths, |document| {
        set_environment_bool(document, name, "hidden", hidden)
    })
}

/// What a write for one particular environment did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guarded {
    /// The field was set, and whether that changed the file.
    Set(bool),
    /// The environment is gone, or it was removed and made again since its
    /// entry was read, so nothing was written.
    Gone,
}

/// `set_favorite` for the environment whose entry had `added`, which only
/// creating an environment stamps, so it tells a recreated one apart. Both
/// checks happen under the lock, so a write that lands after `rm` cleaned
/// up can't bring the entry back.
pub fn set_favorite_for(
    paths: &Paths,
    name: &str,
    added: Option<Timestamp>,
    favorite: bool,
) -> Result<Guarded> {
    set_bool_for(paths, name, added, "favorite", favorite)
}

/// `set_favorite_for` for `hidden`.
pub fn set_hidden_for(
    paths: &Paths,
    name: &str,
    added: Option<Timestamp>,
    hidden: bool,
) -> Result<Guarded> {
    set_bool_for(paths, name, added, "hidden", hidden)
}

fn set_bool_for(
    paths: &Paths,
    name: &str,
    added: Option<Timestamp>,
    field: &str,
    value: bool,
) -> Result<Guarded> {
    let mut gone = false;
    let changed = edit(paths, |document| {
        let current = document
            .get("environments")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get(name))
            .and_then(|item| read_entry(item).added);
        if current != added || !paths.environment_config(name).exists() {
            gone = true;
            return false;
        }
        set_environment_bool(document, name, field, value)
    })?;
    Ok(if gone {
        Guarded::Gone
    } else {
        Guarded::Set(changed)
    })
}

pub fn set_sort(paths: &Paths, sort: SortKey) -> Result<bool> {
    edit(paths, |document| set_view_sort(document, sort))
}

/// `set_sort` and `set_descending` in one write, for picking a key that
/// starts in its own direction.
pub fn set_order(paths: &Paths, sort: SortKey, descending: bool) -> Result<bool> {
    edit(paths, |document| {
        let sorted = set_view_sort(document, sort);
        let directed = set_view_bool(document, "descending", descending);
        sorted || directed
    })
}

pub fn set_descending(paths: &Paths, descending: bool) -> Result<bool> {
    edit(paths, |document| {
        set_view_bool(document, "descending", descending)
    })
}

pub fn set_favorites_first(paths: &Paths, favorites_first: bool) -> Result<bool> {
    edit(paths, |document| {
        set_view_bool(document, "favorites_first", favorites_first)
    })
}

pub fn stamp_added(paths: &Paths, name: &str) -> Result<bool> {
    set_added(paths, name, Timestamp::now())
}

pub fn fresh(paths: &Paths, name: &str) -> Result<bool> {
    let entry = Entry {
        added: Some(Timestamp::now()),
        ..Entry::default()
    };
    put(paths, name, Some(&entry))
}

pub fn remove(paths: &Paths, name: &str) -> Result<bool> {
    put(paths, name, None)
}

pub fn put(paths: &Paths, name: &str, entry: Option<&Entry>) -> Result<bool> {
    edit(paths, |document| match entry {
        Some(entry) => {
            if document
                .get("environments")
                .and_then(Item::as_table_like)
                .and_then(|table| table.get(name))
                .is_some_and(|item| entry_matches(item, entry))
            {
                return false;
            }
            environments_table_mut(document).insert(name, entry_item(entry));
            true
        }
        None => document
            .get_mut("environments")
            .and_then(Item::as_table_like_mut)
            .is_some_and(|table| table.remove(name).is_some()),
    })
}

/// Reads, changes, and replaces the file under one lock. `change` returns
/// whether it altered the document.
fn edit(paths: &Paths, change: impl FnOnce(&mut DocumentMut) -> bool) -> Result<bool> {
    let path = paths.library_file();
    let _guard = Locks::new(paths).config(&path)?;
    let (mut document, missing) = match read_document(&path) {
        Ok(Some(document)) => (document, false),
        Ok(None) => (DocumentMut::new(), true),
        Err(ReadFailure::Io(error)) => {
            return Err(error).context(|| format!("failed to read {}", path.display()));
        }
        Err(ReadFailure::NotUtf8 { newer: false }) => {
            return Err(io::Error::new(io::ErrorKind::InvalidData, NOT_UTF8))
                .context(|| format!("failed to read {}", path.display()));
        }
        Err(failure) => return Err(unreadable_error(&path, failure)),
    };
    let add_schema_version = missing || document.get("schema_version").is_none();
    if !change(&mut document) {
        return Ok(false);
    }
    if add_schema_version {
        document["schema_version"] = Item::Value(Value::from(SCHEMA_VERSION));
    }
    crate::files::write_atomic(&path, document.to_string())?;
    Ok(true)
}

fn read_document(path: &Path) -> std::result::Result<Option<DocumentMut>, ReadFailure> {
    match fs::read(path) {
        Ok(bytes) => document_from_bytes(bytes).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ReadFailure::Io(error)),
    }
}

/// `read_document` for bytes already read.
fn document_from_bytes(bytes: Vec<u8>) -> std::result::Result<DocumentMut, ReadFailure> {
    let text = String::from_utf8(bytes).map_err(|error| ReadFailure::NotUtf8 {
        newer: repair::declares_newer_version(error.as_bytes()),
    })?;
    parse_document(&text)
}

fn parse_document(text: &str) -> std::result::Result<DocumentMut, ReadFailure> {
    let document = text.parse::<DocumentMut>().map_err(|error| {
        let line = error.span().map(|span| {
            1 + text.as_bytes()[..span.start]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
        });
        // A file that doesn't parse may still say a newer wineo wrote it,
        // and repairing it would throw that wineo's data away.
        let newer = repair::declares_newer_version(text.as_bytes());
        ReadFailure::Parse { error, line, newer }
    })?;
    if let Some(version) = document.get("schema_version")
        && version.as_integer() != Some(SCHEMA_VERSION)
    {
        return Err(ReadFailure::Schema {
            found: version.to_string(),
            newer: check::is_newer(&document),
        });
    }
    Ok(document)
}

fn unreadable_error(path: &Path, failure: ReadFailure) -> Error {
    let newer_version = failure.newer();
    let source = match failure {
        ReadFailure::Io(error) => {
            return Error::Io {
                context: format!("failed to read {}", path.display()),
                source: error,
            };
        }
        ReadFailure::NotUtf8 { .. } => Error::Io {
            context: format!("failed to read {}", path.display()),
            source: io::Error::new(io::ErrorKind::InvalidData, NOT_UTF8),
        },
        ReadFailure::Parse { error: source, .. } => Error::ConfigParse {
            path: path.to_owned(),
            source: Box::new(source),
        },
        ReadFailure::Schema { found, .. } => Error::SchemaVersion {
            path: path.to_owned(),
            found,
        },
    };
    Error::LibraryUnreadable {
        newer_version,
        source: Box::new(source),
    }
}

fn read_library(document: &DocumentMut) -> Library {
    let view = document
        .get("view")
        .and_then(Item::as_table_like)
        .map(|table| View {
            sort: table
                .get("sort")
                .and_then(Item::as_str)
                .and_then(SortKey::from_str)
                .unwrap_or_default(),
            descending: table
                .get("descending")
                .and_then(Item::as_bool)
                .unwrap_or_default(),
            favorites_first: table
                .get("favorites_first")
                .and_then(Item::as_bool)
                .unwrap_or_default(),
        })
        .unwrap_or_default();
    let environments = document
        .get("environments")
        .and_then(Item::as_table_like)
        .map(|table| {
            table
                .iter()
                .map(|(name, item)| (name.to_owned(), read_entry(item)))
                .collect()
        })
        .unwrap_or_default();
    Library {
        view,
        environments,
        problem: None,
    }
}

fn read_entry(item: &Item) -> Entry {
    let Some(table) = item.as_table_like() else {
        return Entry::default();
    };
    Entry {
        favorite: table
            .get("favorite")
            .and_then(Item::as_bool)
            .unwrap_or_default(),
        hidden: table
            .get("hidden")
            .and_then(Item::as_bool)
            .unwrap_or_default(),
        added: table
            .get("added")
            .and_then(Item::as_datetime)
            .and_then(|value| value.to_string().parse().ok()),
    }
}

fn set_environment_bool(document: &mut DocumentMut, name: &str, field: &str, value: bool) -> bool {
    let current = document
        .get("environments")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get(name))
        .and_then(Item::as_table_like)
        .and_then(|table| table.get(field));
    let matches = match current {
        None => !value,
        Some(item) => item.as_bool() == Some(value),
    };
    if matches {
        return false;
    }
    store_value(
        environment_table_mut(document, name),
        field,
        Value::from(value),
    );
    true
}

fn set_view_sort(document: &mut DocumentMut, sort: SortKey) -> bool {
    let current = document
        .get("view")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get("sort"));
    let matches = match current {
        None => sort == SortKey::default(),
        Some(item) => item
            .as_str()
            .and_then(SortKey::from_str)
            .is_some_and(|current| current == sort),
    };
    if matches {
        return false;
    }
    store_value(
        view_table_mut(document),
        "sort",
        Value::from(sort_name(sort)),
    );
    true
}

fn set_view_bool(document: &mut DocumentMut, field: &str, value: bool) -> bool {
    let current = document
        .get("view")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get(field));
    let matches = match current {
        None => !value,
        Some(item) => item.as_bool() == Some(value),
    };
    if matches {
        return false;
    }
    store_value(view_table_mut(document), field, Value::from(value));
    true
}

fn set_added(paths: &Paths, name: &str, added: Timestamp) -> Result<bool> {
    edit(paths, |document| {
        let current = document
            .get("environments")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get(name))
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("added"));
        let matches = current
            .and_then(Item::as_datetime)
            .and_then(|value| value.to_string().parse::<Timestamp>().ok())
            == Some(added);
        if matches {
            return false;
        }
        let datetime = added
            .to_string()
            .parse::<toml_edit::Datetime>()
            .expect("jiff timestamps are TOML offset datetimes");
        store_value(
            environment_table_mut(document, name),
            "added",
            Value::from(datetime),
        );
        true
    })
}

fn entry_matches(item: &Item, expected: &Entry) -> bool {
    let Some(table) = item.as_table_like() else {
        return false;
    };
    let favorite = table.get("favorite");
    let hidden = table.get("hidden");
    let added = table.get("added");
    let bool_matches = |field: Option<&Item>, expected: bool| match field {
        None => !expected,
        Some(value) => value.as_bool() == Some(expected),
    };
    let added_value = match added {
        None => None,
        Some(value) => match value.as_datetime() {
            Some(value) => value.to_string().parse::<Timestamp>().ok(),
            None => return false,
        },
    };
    bool_matches(favorite, expected.favorite)
        && bool_matches(hidden, expected.hidden)
        && added_value == expected.added
}

fn entry_item(entry: &Entry) -> Item {
    let mut table = Table::new();
    if entry.favorite {
        table.insert("favorite", Item::Value(Value::from(true)));
    }
    if entry.hidden {
        table.insert("hidden", Item::Value(Value::from(true)));
    }
    if let Some(added) = entry.added {
        let datetime = added
            .to_string()
            .parse::<toml_edit::Datetime>()
            .expect("jiff timestamps are TOML offset datetimes");
        table.insert("added", Item::Value(Value::from(datetime)));
    }
    Item::Table(table)
}

fn view_table_mut(document: &mut DocumentMut) -> &mut dyn TableLike {
    table_mut(document.as_table_mut(), "view")
}

fn environments_table_mut(document: &mut DocumentMut) -> &mut dyn TableLike {
    table_mut(document.as_table_mut(), "environments")
}

fn environment_table_mut<'a>(document: &'a mut DocumentMut, name: &str) -> &'a mut dyn TableLike {
    let environments = table_mut(document.as_table_mut(), "environments");
    if environments
        .get(name)
        .is_none_or(|item| !item.is_table_like())
    {
        environments.insert(name, Item::Table(Table::new()));
    }
    environments
        .get_mut(name)
        .and_then(Item::as_table_like_mut)
        .expect("environment entry was just made a table")
}

fn table_mut<'a>(parent: &'a mut Table, name: &str) -> &'a mut dyn TableLike {
    // Implicit, so a parent that only holds sub-tables writes no header of its own.
    let implicit = || {
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    };
    let item = parent.entry(name).or_insert_with(implicit);
    if !item.is_table_like() {
        *item = implicit();
    }
    item.as_table_like_mut()
        .expect("table item was just made a table")
}

fn store_value(table: &mut dyn TableLike, field: &str, mut value: Value) {
    if let Some(old) = table.get(field).and_then(Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    table.insert(field, Item::Value(value));
}

fn sort_name(sort: SortKey) -> &'static str {
    match sort {
        SortKey::Name => "name",
        SortKey::LastPlayed => "last-played",
        SortKey::Playtime => "playtime",
        SortKey::Added => "added",
        SortKey::Source => "source",
    }
}

#[cfg(test)]
mod tests;
