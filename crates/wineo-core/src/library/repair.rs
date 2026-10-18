//! Explicit repair of a `library.toml` that can't be read: back it up, then
//! write a fresh file with what can still be read line by line.

use std::borrow::Cow;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use jiff::Zoned;
use toml_edit::{DocumentMut, Item, Key};

use super::check::{is_newer, is_offset_datetime};
use super::{SCHEMA_VERSION, SortKey};
use crate::error::{Error, IoContext, Result};
use crate::operation::Locks;
use crate::paths::Paths;

/// How much of `[view]` came back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewRecovery {
    /// The file had no `[view]` the repair could find.
    Absent,
    Recovered,
    Partly,
    Lost,
}

/// What a repair salvaged. Counts cover only what it could recognize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairReport {
    /// `[environments.<name>]` headers found, duplicates counted once.
    pub entries: usize,
    /// Entries whose every recognized field was kept, including empty ones.
    pub full: usize,
    /// Entries with some, but not all, recognized fields kept.
    pub partly: usize,
    pub view: ViewRecovery,
    /// Text that couldn't be attributed to a recognized entry or field was dropped.
    pub dropped_text: bool,
    pub backup: PathBuf,
}

impl RepairReport {
    /// Entries with none of their recognized fields kept.
    pub fn lost(&self) -> usize {
        self.entries - self.full - self.partly
    }

    /// Whether everything the repair recognized came back.
    pub fn complete(&self) -> bool {
        self.full == self.entries
            && !self.dropped_text
            && matches!(self.view, ViewRecovery::Absent | ViewRecovery::Recovered)
    }
}

impl fmt::Display for RepairReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entries = self.entries;
        if entries == 0 {
            f.write_str("No library entries were recognized.")?;
        } else if self.full == entries && entries == 1 {
            f.write_str("Recovered the one recognized entry.")?;
        } else if self.full == entries {
            write!(f, "Recovered all {entries} recognized entries.")?;
        } else {
            let noun = if entries == 1 { "entry" } else { "entries" };
            write!(
                f,
                "Recovered {} of {entries} recognized {noun} fully",
                self.full
            )?;
            if self.partly > 0 {
                write!(f, " and {} partly", self.partly)?;
            }
            f.write_str(".")?;
        }
        if !self.complete() {
            f.write_str(" Some data could not be recovered.")?;
        }
        match self.view {
            ViewRecovery::Absent => {}
            ViewRecovery::Recovered => f.write_str(" Sort settings recovered.")?,
            ViewRecovery::Partly => f.write_str(" Sort settings partly recovered.")?,
            ViewRecovery::Lost => f.write_str(" Sort settings could not be recovered.")?,
        }
        let backup = self.backup.file_name().map_or_else(
            || self.backup.to_string_lossy(),
            |name| name.to_string_lossy(),
        );
        write!(f, "\nOriginal saved as {backup}.")
    }
}

/// Repairs `library.toml` when it can't be read, holding its lock throughout.
/// Returns None, changing nothing, when the file is missing or reads fine.
/// Refuses a file a newer wineo wrote.
pub fn repair(paths: &Paths) -> Result<Option<RepairReport>> {
    let stamp = Zoned::now().strftime("%Y-%m-%dT%H-%M-%S").to_string();
    repair_at(paths, &stamp)
}

pub(super) fn repair_at(paths: &Paths, stamp: &str) -> Result<Option<RepairReport>> {
    let path = paths.library_file();
    let _guard = Locks::new(paths).config(&path)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context(|| format!("failed to read {}", path.display())),
    };
    let text = String::from_utf8_lossy(&bytes);
    if let Cow::Borrowed(text) = &text
        && let Ok(document) = text.parse::<DocumentMut>()
    {
        if let Some(version) = document.get("schema_version")
            && is_newer(&document)
        {
            return Err(Error::SchemaVersion {
                path,
                found: version.to_string().trim().to_owned(),
            });
        }
        if document
            .get("schema_version")
            .is_none_or(|version| version.as_integer() == Some(SCHEMA_VERSION))
        {
            return Ok(None);
        }
    }

    let backup = back_up(&path, stamp, &bytes)?;
    let salvaged = salvage(&text);
    let contents = salvaged.render();
    if let Err(error) = contents.parse::<DocumentMut>() {
        return Err(Error::ConfigParse {
            path,
            source: Box::new(error),
        });
    }
    crate::files::write_atomic(&path, contents)?;
    Ok(Some(salvaged.report(backup)))
}

/// Copies the file beside itself, never replacing an existing file.
fn back_up(path: &Path, stamp: &str, bytes: &[u8]) -> Result<PathBuf> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = format!("{name}.broken-{stamp}");
    let mut attempt = 1;
    loop {
        let candidate = if attempt == 1 {
            path.with_file_name(&base)
        } else {
            path.with_file_name(format!("{base}-{attempt}"))
        };
        let context = || {
            format!(
                "failed to back up {} to {}",
                path.display(),
                candidate.display()
            )
        };
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                attempt += 1;
                continue;
            }
            Err(error) => return Err(error).context(context),
        };
        if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&candidate);
            return Err(error).context(context);
        }
        return Ok(candidate);
    }
}

#[derive(Debug, Default)]
struct Salvaged {
    view: Option<Section>,
    entries: Vec<(String, Section)>,
    dropped_text: bool,
}

/// One recognized table: how many fields the repair recognized in it, and
/// the lines it kept.
#[derive(Debug, Default)]
struct Section {
    fields: usize,
    keys: Vec<String>,
    lines: Vec<String>,
}

impl Section {
    fn recovery(&self) -> ViewRecovery {
        if self.keys.len() == self.fields {
            ViewRecovery::Recovered
        } else if self.keys.is_empty() {
            ViewRecovery::Lost
        } else {
            ViewRecovery::Partly
        }
    }
}

#[derive(Clone, Copy)]
enum Current {
    Top,
    View,
    Entry(usize),
    Unattributed,
}

enum Header {
    View,
    Entry(String),
}

fn salvage(text: &str) -> Salvaged {
    let mut salvaged = Salvaged::default();
    let mut current = Current::Top;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            current = match header(line) {
                Some(Header::View) => {
                    salvaged.view.get_or_insert_with(Section::default);
                    Current::View
                }
                Some(Header::Entry(name)) => {
                    let index = match salvaged.entries.iter().position(|(seen, _)| *seen == name) {
                        Some(index) => index,
                        None => {
                            salvaged.entries.push((name, Section::default()));
                            salvaged.entries.len() - 1
                        }
                    };
                    Current::Entry(index)
                }
                None => {
                    salvaged.dropped_text = true;
                    Current::Unattributed
                }
            };
            continue;
        }
        let parsed = line.parse::<DocumentMut>().ok();
        let (section, valid): (&mut Section, fn(&str, &Item) -> bool) = match current {
            Current::Top => {
                // Written fresh, so an old one isn't a loss.
                let schema = parsed
                    .as_ref()
                    .is_some_and(|document| document.contains_key("schema_version"));
                if !schema {
                    salvaged.dropped_text = true;
                }
                continue;
            }
            Current::Unattributed => {
                salvaged.dropped_text = true;
                continue;
            }
            Current::View => (
                salvaged.view.as_mut().expect("view section was recorded"),
                valid_view_field,
            ),
            Current::Entry(index) => (&mut salvaged.entries[index].1, valid_entry_field),
        };
        let field = parsed
            .as_ref()
            .and_then(|document| document.iter().next())
            .map(|(key, item)| (key.to_owned(), item));
        match field {
            Some((key, item)) => {
                section.fields += 1;
                if !section.keys.contains(&key) && valid(&key, item) {
                    section.keys.push(key);
                    section.lines.push(line.to_owned());
                }
            }
            None if looks_like_field(line) => section.fields += 1,
            None => salvaged.dropped_text = true,
        }
    }
    salvaged
}

/// A header line on its own: exactly `[view]` or `[environments.<name>]`.
fn header(line: &str) -> Option<Header> {
    let document = line.parse::<DocumentMut>().ok()?;
    let mut keys = document.iter();
    let (key, item) = keys.next()?;
    if keys.next().is_some() {
        return None;
    }
    let table = item.as_table()?;
    match key {
        "view" => table.is_empty().then_some(Header::View),
        "environments" => {
            let mut children = table.iter();
            let (name, child) = children.next()?;
            if children.next().is_some() {
                return None;
            }
            child
                .as_table()
                .is_some_and(|child| child.is_empty())
                .then(|| Header::Entry(name.to_owned()))
        }
        _ => None,
    }
}

/// A `key = value` line whose value didn't parse still counts as a field
/// the repair recognized.
fn looks_like_field(line: &str) -> bool {
    line.split_once('=')
        .is_some_and(|(key, _)| Key::parse(key.trim()).is_ok_and(|keys| !keys.is_empty()))
}

fn valid_entry_field(key: &str, item: &Item) -> bool {
    match key {
        "favorite" | "hidden" => item.as_bool().is_some(),
        "added" => is_offset_datetime(item),
        // A newer release may have written it.
        _ => true,
    }
}

fn valid_view_field(key: &str, item: &Item) -> bool {
    match key {
        "sort" => item.as_str().and_then(SortKey::from_str).is_some(),
        "descending" | "favorites_first" => item.as_bool().is_some(),
        _ => true,
    }
}

impl Salvaged {
    fn render(&self) -> String {
        let mut out = format!("schema_version = {SCHEMA_VERSION}\n");
        if let Some(view) = self.view.as_ref().filter(|view| !view.lines.is_empty()) {
            out.push_str("\n[view]\n");
            for line in &view.lines {
                out.push_str(line);
                out.push('\n');
            }
        }
        for (name, entry) in &self.entries {
            out.push_str(&format!(
                "\n[environments.{}]\n",
                Key::new(name.as_str()).display_repr()
            ));
            for line in &entry.lines {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    fn report(&self, backup: PathBuf) -> RepairReport {
        let mut full = 0;
        let mut partly = 0;
        for (_, entry) in &self.entries {
            match entry.recovery() {
                ViewRecovery::Recovered => full += 1,
                ViewRecovery::Partly => partly += 1,
                ViewRecovery::Lost | ViewRecovery::Absent => {}
            }
        }
        RepairReport {
            entries: self.entries.len(),
            full,
            partly,
            view: self
                .view
                .as_ref()
                .map_or(ViewRecovery::Absent, Section::recovery),
            dropped_text: self.dropped_text,
            backup,
        }
    }
}

#[cfg(test)]
mod tests;
