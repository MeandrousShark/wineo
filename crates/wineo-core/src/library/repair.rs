//! Explicit repair of a `library.toml` that can't be read: back it up, then
//! write a fresh file with what can still be read line by line.

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
    repair_at(paths, &stamp, crate::files::sync_dir)
}

/// `sync_dir` makes the backup's name durable before the original is replaced.
pub(super) fn repair_at(
    paths: &Paths,
    stamp: &str,
    sync_dir: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<Option<RepairReport>> {
    let path = paths.library_file();
    let _guard = Locks::new(paths).config(&path)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context(|| format!("failed to read {}", path.display())),
    };
    if let Ok(text) = std::str::from_utf8(&bytes)
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
    let units = units(&bytes);
    if let Some(version) = newer_version(&units) {
        return Err(Error::SchemaVersion {
            path,
            found: version.to_string(),
        });
    }

    let backup = back_up(&path, stamp, &bytes)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    sync_dir(dir).context(|| format!("failed to sync {}", dir.display()))?;
    let salvaged = salvage(units);
    let contents = salvaged.render();
    if let Err(error) = contents.parse::<DocumentMut>() {
        return Err(Error::ConfigParse {
            path,
            source: Box::new(error),
        });
    }
    crate::files::write_atomic_synced(&path, contents)?;
    Ok(Some(salvaged.report(backup)))
}

/// Whether a file that can't be parsed still says a newer wineo wrote it.
pub(super) fn declares_newer_version(bytes: &[u8]) -> bool {
    newer_version(&units(bytes)).is_some()
}

/// A top-level `schema_version` above this wineo's, read on its own.
fn newer_version(units: &[Unit]) -> Option<i64> {
    units
        .iter()
        .take_while(|unit| !matches!(unit, Unit::Header(_)))
        .filter_map(|unit| match unit {
            Unit::Value {
                text: Some(text), ..
            } => text.parse::<DocumentMut>().ok(),
            _ => None,
        })
        .filter_map(|document| document.get("schema_version").and_then(Item::as_integer))
        .find(|version| *version > SCHEMA_VERSION)
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

/// A header line, or one `key = value` with every line its value spans.
enum Unit {
    /// None for a header that isn't `[view]` or `[environments.<name>]`.
    Header(Option<Header>),
    Value {
        /// None when any of its lines isn't valid UTF-8.
        text: Option<String>,
        key_shaped: bool,
    },
}

enum Header {
    View,
    Entry(String),
}

/// Splits the file into units, so nothing inside a multi-line string or
/// array is mistaken for a header or a field.
fn units(bytes: &[u8]) -> Vec<Unit> {
    let bytes = bytes.strip_prefix("\u{feff}".as_bytes()).unwrap_or(bytes);
    let lines: Vec<&[u8]> = bytes
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .collect();
    let mut units = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let first = String::from_utf8_lossy(lines[index]);
        let trimmed = first.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            index += 1;
            continue;
        }
        if trimmed.starts_with('[') {
            let header = std::str::from_utf8(lines[index])
                .ok()
                .and_then(|line| header(line.trim()));
            units.push(Unit::Header(header));
            index += 1;
            continue;
        }
        let mut scan = Scan::default();
        scan.line(trimmed, true);
        let key_shaped = scan.equals.is_some_and(|equals| {
            Key::parse(trimmed[..equals].trim()).is_ok_and(|keys| !keys.is_empty())
        });
        let start = index;
        index += 1;
        // A value left open swallows lines until it closes, or to the end.
        while key_shaped && scan.open() && index < lines.len() {
            scan.line(&String::from_utf8_lossy(lines[index]), false);
            index += 1;
        }
        let text = lines[start..index]
            .iter()
            .map(|line| std::str::from_utf8(line).ok())
            .collect::<Option<Vec<&str>>>()
            .map(|lines| lines.join("\n").trim().to_owned());
        units.push(Unit::Value { text, key_shaped });
    }
    units
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Quote {
    #[default]
    None,
    Basic,
    Literal,
    MultiBasic,
    MultiLiteral,
}

/// Just enough of a TOML lexer to tell where a value ends.
#[derive(Debug, Default)]
struct Scan {
    quote: Quote,
    /// Open brackets and braces in the value.
    depth: usize,
    /// Where the first line's `=` is, outside any quotes.
    equals: Option<usize>,
}

impl Scan {
    fn open(&self) -> bool {
        matches!(self.quote, Quote::MultiBasic | Quote::MultiLiteral) || self.depth > 0
    }

    fn line(&mut self, line: &str, first: bool) {
        let bytes = line.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let rest = &bytes[index..];
            match self.quote {
                Quote::None => match bytes[index] {
                    b'#' => break,
                    b'"' if rest.starts_with(b"\"\"\"") => {
                        self.quote = Quote::MultiBasic;
                        index += 3;
                        continue;
                    }
                    b'\'' if rest.starts_with(b"'''") => {
                        self.quote = Quote::MultiLiteral;
                        index += 3;
                        continue;
                    }
                    b'"' => self.quote = Quote::Basic,
                    b'\'' => self.quote = Quote::Literal,
                    b'=' if first && self.equals.is_none() => self.equals = Some(index),
                    b'[' | b'{' if self.equals.is_some() => self.depth += 1,
                    b']' | b'}' if self.equals.is_some() => {
                        self.depth = self.depth.saturating_sub(1);
                    }
                    _ => {}
                },
                Quote::Basic => match bytes[index] {
                    b'\\' => index += 1,
                    b'"' => self.quote = Quote::None,
                    _ => {}
                },
                Quote::Literal => {
                    if bytes[index] == b'\'' {
                        self.quote = Quote::None;
                    }
                }
                Quote::MultiBasic => {
                    if bytes[index] == b'\\' {
                        index += 1;
                    } else if rest.starts_with(b"\"\"\"") {
                        self.quote = Quote::None;
                        index += 3;
                        continue;
                    }
                }
                Quote::MultiLiteral => {
                    if rest.starts_with(b"'''") {
                        self.quote = Quote::None;
                        index += 3;
                        continue;
                    }
                }
            }
            index += 1;
        }
        // A single-line string ends with its line, closed or not.
        if matches!(self.quote, Quote::Basic | Quote::Literal) {
            self.quote = Quote::None;
        }
    }
}

#[derive(Debug, Default)]
struct Salvaged {
    view: Option<Section>,
    entries: Vec<(String, Section)>,
    dropped_text: bool,
}

/// One recognized table: how many fields the repair recognized in it, and
/// the ones it kept.
#[derive(Debug, Default)]
struct Section {
    fields: usize,
    lines: Vec<String>,
}

impl Section {
    fn recovery(&self) -> ViewRecovery {
        if self.lines.len() == self.fields {
            ViewRecovery::Recovered
        } else if self.lines.is_empty() {
            ViewRecovery::Lost
        } else {
            ViewRecovery::Partly
        }
    }

    /// Keeps a field that parses alone, is valid, and doesn't clash with a
    /// field already kept, so the first value wins and the result parses.
    fn offer(&mut self, text: Option<String>, valid: fn(&str, &Item) -> bool) {
        self.fields += 1;
        let Some(text) = text else {
            return;
        };
        let Ok(document) = text.parse::<DocumentMut>() else {
            return;
        };
        let mut keys = document.iter();
        let (Some((key, item)), None) = (keys.next(), keys.next()) else {
            return;
        };
        if !valid(key, item) {
            return;
        }
        let mut combined = self.lines.join("\n");
        combined.push('\n');
        combined.push_str(&text);
        if combined.parse::<DocumentMut>().is_ok() {
            self.lines.push(text);
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

fn salvage(units: Vec<Unit>) -> Salvaged {
    let mut salvaged = Salvaged::default();
    let mut current = Current::Top;
    for unit in units {
        let (text, key_shaped) = match unit {
            Unit::Header(header) => {
                current = match header {
                    Some(Header::View) => {
                        salvaged.view.get_or_insert_with(Section::default);
                        Current::View
                    }
                    Some(Header::Entry(name)) => {
                        let index =
                            match salvaged.entries.iter().position(|(seen, _)| *seen == name) {
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
            Unit::Value { text, key_shaped } => (text, key_shaped),
        };
        match current {
            Current::Top => {
                // Written fresh, so an old one isn't a loss.
                let schema = text
                    .and_then(|text| text.parse::<DocumentMut>().ok())
                    .is_some_and(|document| document.contains_key("schema_version"));
                if !schema {
                    salvaged.dropped_text = true;
                }
            }
            Current::Unattributed => salvaged.dropped_text = true,
            _ if !key_shaped => salvaged.dropped_text = true,
            Current::View => salvaged
                .view
                .as_mut()
                .expect("view section was recorded")
                .offer(text, valid_view_field),
            Current::Entry(index) => salvaged.entries[index].1.offer(text, valid_entry_field),
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
