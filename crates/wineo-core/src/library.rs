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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    pub view: View,
    pub environments: BTreeMap<String, Entry>,
    pub problem: Option<Problem>,
}

enum ReadFailure {
    Io(io::Error),
    Parse {
        error: toml_edit::TomlError,
        line: Option<usize>,
    },
    Schema(String),
}

/// Reads display state, falling back to defaults when the file can't be used.
pub fn load(paths: &Paths) -> Library {
    library_from(read_document(&paths.library_file()))
}

fn library_from(read: std::result::Result<Option<DocumentMut>, ReadFailure>) -> Library {
    let document = match read {
        Ok(Some(document)) => document,
        Ok(None) => return Library::default(),
        Err(ReadFailure::Io(error)) => {
            return Library {
                problem: Some(Problem {
                    reason: error.to_string(),
                    line: None,
                    newer_version: false,
                }),
                ..Library::default()
            };
        }
        Err(ReadFailure::Parse { error, line }) => {
            return Library {
                problem: Some(Problem {
                    reason: error.message().to_owned(),
                    line,
                    newer_version: false,
                }),
                ..Library::default()
            };
        }
        Err(ReadFailure::Schema(_)) => {
            return Library {
                problem: Some(Problem {
                    reason: "library.toml was written by a newer wineo; update wineo to use it."
                        .to_owned(),
                    line: None,
                    newer_version: true,
                }),
                ..Library::default()
            };
        }
    };

    read_library(&document)
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

pub fn set_sort(paths: &Paths, sort: SortKey) -> Result<bool> {
    edit(paths, |document| {
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
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ReadFailure::Io(error)),
    };
    parse_document(&text).map(Some)
}

/// `read_document` for bytes already read.
fn document_from_bytes(bytes: Vec<u8>) -> std::result::Result<DocumentMut, ReadFailure> {
    let text = String::from_utf8(bytes).map_err(|_| {
        // What `fs::read_to_string` says, so both reads report alike.
        ReadFailure::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        ))
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
        ReadFailure::Parse { error, line }
    })?;
    if let Some(version) = document.get("schema_version")
        && version.as_integer() != Some(SCHEMA_VERSION)
    {
        return Err(ReadFailure::Schema(version.to_string()));
    }
    Ok(document)
}

fn unreadable_error(path: &Path, failure: ReadFailure) -> Error {
    let (newer_version, source) = match failure {
        ReadFailure::Io(error) => {
            return Error::Io {
                context: format!("failed to read {}", path.display()),
                source: error,
            };
        }
        ReadFailure::Parse { error: source, .. } => (
            false,
            Error::ConfigParse {
                path: path.to_owned(),
                source: Box::new(source),
            },
        ),
        ReadFailure::Schema(found) => (
            true,
            Error::SchemaVersion {
                path: path.to_owned(),
                found,
            },
        ),
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
