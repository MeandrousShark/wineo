//! What `validate` reports about `library.toml`, and its stray-entry fix.

use std::collections::BTreeSet;
use std::fs;

use toml_edit::{DocumentMut, Item, Key};

use super::{ReadFailure, SCHEMA_VERSION, SortKey, edit, read_document};
use crate::paths::Paths;

/// The file as a whole can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unreadable {
    pub(crate) message: String,
    /// A newer wineo wrote it, so repairing would throw its data away.
    pub(crate) newer: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FindingKind {
    FieldInvalid,
    KeyUnknown,
}

/// One field or key worth a warning. `environment` is None for the top
/// level and `[view]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) kind: FindingKind,
    pub(crate) environment: Option<String>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Inspection {
    pub(crate) unreadable: Option<Unreadable>,
    /// Every name under `environments`, whatever its value.
    pub(crate) entries: Vec<String>,
    pub(crate) findings: Vec<Finding>,
}

const BOOL: &str = "expected true or false";
const DATETIME: &str = "expected a date and time with an offset, like 2026-10-17T18:00:00Z";
const TABLE: &str = "expected a table";

pub(crate) fn inspect(paths: &Paths) -> Inspection {
    let path = paths.library_file();
    let document = match read_document(&path) {
        Ok(Some(document)) => document,
        Ok(None) => return Inspection::default(),
        Err(failure) => {
            return Inspection {
                unreadable: Some(unreadable(&path, failure)),
                ..Inspection::default()
            };
        }
    };
    let mut inspection = Inspection::default();
    for (key, item) in document.iter() {
        match key {
            "schema_version" => {}
            "view" => inspect_view(item, &mut inspection),
            "environments" => inspect_environments(item, &mut inspection),
            other => inspection
                .findings
                .push(unknown(None, &display_path(&[other]))),
        }
    }
    inspection
}

fn unreadable(path: &std::path::Path, failure: ReadFailure) -> Unreadable {
    match failure {
        ReadFailure::Io(error) => Unreadable {
            message: format!("library.toml can't be read ({error})"),
            newer: false,
        },
        ReadFailure::Parse { error, line } => {
            let reason = error.message().trim();
            Unreadable {
                message: match line {
                    Some(line) => format!("library.toml can't be read (line {line}: {reason})"),
                    None => format!("library.toml can't be read ({reason})"),
                },
                newer: false,
            }
        }
        ReadFailure::Schema(found) => {
            // read_document only returns Schema for a file that parsed.
            let newer = fs::read_to_string(path)
                .ok()
                .and_then(|text| text.parse::<DocumentMut>().ok())
                .is_some_and(|document| is_newer(&document));
            Unreadable {
                message: if newer {
                    "library.toml was written by a newer wineo; update wineo to use it.".to_owned()
                } else {
                    format!(
                        "library.toml can't be read (unsupported schema_version {})",
                        found.trim()
                    )
                },
                newer,
            }
        }
    }
}

/// Whether `schema_version` is an integer above the one this wineo writes.
pub(super) fn is_newer(document: &DocumentMut) -> bool {
    document
        .get("schema_version")
        .and_then(Item::as_integer)
        .is_some_and(|version| version > SCHEMA_VERSION)
}

fn inspect_view(item: &Item, inspection: &mut Inspection) {
    let Some(table) = item.as_table_like() else {
        inspection
            .findings
            .push(invalid(None, &display_path(&["view"]), TABLE, item));
        return;
    };
    for (key, value) in table.iter() {
        let path = display_path(&["view", key]);
        let expected = match key {
            "sort" => match value.as_str() {
                Some(name) if SortKey::from_str(name).is_some() => continue,
                _ => sort_expected(),
            },
            "descending" | "favorites_first" if value.as_bool().is_some() => continue,
            "descending" | "favorites_first" => BOOL.to_owned(),
            _ => {
                inspection.findings.push(unknown(None, &path));
                continue;
            }
        };
        inspection
            .findings
            .push(invalid(None, &path, &expected, value));
    }
}

fn inspect_environments(item: &Item, inspection: &mut Inspection) {
    let Some(table) = item.as_table_like() else {
        inspection
            .findings
            .push(invalid(None, &display_path(&["environments"]), TABLE, item));
        return;
    };
    for (name, entry) in table.iter() {
        inspection.entries.push(name.to_owned());
        inspect_entry(name, entry, inspection);
    }
}

fn inspect_entry(name: &str, item: &Item, inspection: &mut Inspection) {
    let environment = Some(name.to_owned());
    let Some(table) = item.as_table_like() else {
        inspection.findings.push(invalid(
            environment,
            &display_path(&["environments", name]),
            TABLE,
            item,
        ));
        return;
    };
    for (key, value) in table.iter() {
        let path = display_path(&["environments", name, key]);
        let expected = match key {
            "favorite" | "hidden" if value.as_bool().is_some() => continue,
            "favorite" | "hidden" => BOOL,
            "added" if is_offset_datetime(value) => continue,
            "added" => DATETIME,
            _ => {
                inspection
                    .findings
                    .push(unknown(environment.clone(), &path));
                continue;
            }
        };
        inspection
            .findings
            .push(invalid(environment.clone(), &path, expected, value));
    }
}

pub(super) fn is_offset_datetime(item: &Item) -> bool {
    item.as_datetime()
        .is_some_and(|value| value.date.is_some() && value.time.is_some() && value.offset.is_some())
}

pub(super) fn sort_expected() -> String {
    format!(
        "expected one of {}",
        ["name", "last-played", "playtime", "added", "source"].join(", ")
    )
}

fn invalid(environment: Option<String>, path: &str, expected: &str, found: &Item) -> Finding {
    let found = match found {
        Item::Value(value) => value.to_string().trim().to_owned(),
        Item::Table(_) => "a table".to_owned(),
        Item::ArrayOfTables(_) => "an array of tables".to_owned(),
        Item::None => "nothing".to_owned(),
    };
    Finding {
        kind: FindingKind::FieldInvalid,
        environment,
        message: format!("library.toml: {path}: {expected}, found {found}"),
    }
}

fn unknown(environment: Option<String>, path: &str) -> Finding {
    Finding {
        kind: FindingKind::KeyUnknown,
        environment,
        message: format!("library.toml: {path}: unknown key, ignored"),
    }
}

fn display_path(keys: &[&str]) -> String {
    keys.iter()
        .map(|key| Key::new(*key).display_repr().into_owned())
        .collect::<Vec<_>>()
        .join(".")
}

/// Removes every entry whose environment isn't in what `known` returns,
/// which is read under the file's lock so a name created meanwhile stays.
/// Never writes a file that can't be read. Returns the names removed.
pub(crate) fn remove_strays(
    paths: &Paths,
    known: impl FnOnce() -> anyhow::Result<BTreeSet<String>>,
) -> anyhow::Result<Vec<String>> {
    let mut failure = None;
    let mut removed = Vec::new();
    edit(paths, |document| {
        let known = match known() {
            Ok(known) => known,
            Err(error) => {
                failure = Some(error);
                return false;
            }
        };
        let Some(table) = document
            .get_mut("environments")
            .and_then(Item::as_table_like_mut)
        else {
            return false;
        };
        let strays: Vec<String> = table
            .iter()
            .map(|(name, _)| name.to_owned())
            .filter(|name| !known.contains(name))
            .collect();
        for name in &strays {
            table.remove(name);
        }
        removed = strays;
        !removed.is_empty()
    })?;
    match failure {
        Some(error) => Err(error),
        None => Ok(removed),
    }
}

#[cfg(test)]
mod tests;
