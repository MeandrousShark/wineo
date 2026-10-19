//! Explicit repair of a `library.toml` that can't be read: back it up, then
//! write a fresh file with what can still be read. The TOML parser reads as
//! much of the file as it accepts; only the broken rest is scanned by hand.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use jiff::Zoned;
use toml_edit::{DocumentMut, Item, Key, Table};

use super::check::{is_newer, is_offset_datetime};
use super::{SCHEMA_VERSION, SortKey, read_zoom};
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
    let salvaged = salvage(&bytes);
    if let Some(version) = salvaged.newer_version {
        return Err(Error::SchemaVersion {
            path,
            found: version.to_string(),
        });
    }

    let backup = back_up(&path, stamp, &bytes)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    sync_dir(dir).context(|| format!("failed to sync {}", dir.display()))?;
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
    salvage(bytes).newer_version.is_some()
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

/// The file split into lines, keeping where each starts so a run of whole
/// lines can be handed to the parser as written.
struct Lines<'a> {
    bytes: &'a [u8],
    /// Where each line starts, then the end of the file.
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        let bytes = bytes.strip_prefix("\u{feff}".as_bytes()).unwrap_or(bytes);
        let mut starts = vec![0];
        starts.extend(
            bytes
                .iter()
                .enumerate()
                .filter(|(_, byte)| **byte == b'\n')
                .map(|(index, _)| index + 1),
        );
        if starts.last() != Some(&bytes.len()) {
            starts.push(bytes.len());
        }
        Self { bytes, starts }
    }

    fn len(&self) -> usize {
        self.starts.len() - 1
    }

    fn line(&self, index: usize) -> &'a [u8] {
        let line = &self.bytes[self.starts[index]..self.starts[index + 1]];
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        line.strip_suffix(b"\r").unwrap_or(line)
    }

    /// The text of lines `..end`, if it's all UTF-8.
    fn prefix(&self, end: usize) -> Option<&'a str> {
        std::str::from_utf8(&self.bytes[..self.starts[end]]).ok()
    }

    /// The line a byte offset is on.
    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|start| *start <= offset) - 1
    }
}

/// A header line, or one `key = value` with every line its value spans.
enum Unit {
    /// None for a header that isn't `[view]` or `[environments.<name>]`.
    Header {
        start: usize,
        header: Option<Header>,
    },
    Value {
        start: usize,
        /// None when any of its lines isn't valid UTF-8.
        text: Option<String>,
        key_shaped: bool,
    },
}

impl Unit {
    fn start(&self) -> usize {
        match self {
            Unit::Header { start, .. } | Unit::Value { start, .. } => *start,
        }
    }
}

enum Header {
    View,
    Entry(String),
}

/// Splits lines `from..` into units, so nothing inside a multi-line string or
/// array is mistaken for a header or a field. Only trusted where the parser
/// gave up; see `salvage`.
fn units(lines: &Lines<'_>, from: usize) -> Vec<Unit> {
    let mut units = Vec::new();
    let mut index = from;
    while index < lines.len() {
        let first = String::from_utf8_lossy(lines.line(index));
        let trimmed = first.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            index += 1;
            continue;
        }
        let start = index;
        if trimmed.starts_with('[') {
            let header = std::str::from_utf8(lines.line(index))
                .ok()
                .and_then(|line| header(line.trim()));
            units.push(Unit::Header { start, header });
            index += 1;
            continue;
        }
        let mut scan = Scan::default();
        scan.line(trimmed, true);
        let key_shaped = scan.equals.is_some_and(|equals| {
            Key::parse(trimmed[..equals].trim()).is_ok_and(|keys| !keys.is_empty())
        });
        index += 1;
        // A value left open swallows lines until it closes, or to the end.
        while key_shaped && scan.open() && index < lines.len() {
            scan.line(&String::from_utf8_lossy(lines.line(index)), false);
            index += 1;
        }
        let text = (start..index)
            .map(|line| std::str::from_utf8(lines.line(line)).ok())
            .collect::<Option<Vec<&str>>>()
            .map(|lines| lines.join("\n").trim().to_owned());
        units.push(Unit::Value {
            start,
            text,
            key_shaped,
        });
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
        let run = |at: usize, quote: u8| bytes[at..].iter().take_while(|b| **b == quote).count();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            match self.quote {
                Quote::None => match byte {
                    b'#' => break,
                    b'"' | b'\'' if bytes[index..].starts_with(&[byte; 3]) => {
                        self.quote = if byte == b'"' {
                            Quote::MultiBasic
                        } else {
                            Quote::MultiLiteral
                        };
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
                Quote::Basic => match byte {
                    b'\\' => index += 1,
                    b'"' => self.quote = Quote::None,
                    _ => {}
                },
                Quote::Literal => {
                    if byte == b'\'' {
                        self.quote = Quote::None;
                    }
                }
                // A run of three to five quotes closes the string, the ones
                // past three being content.
                Quote::MultiBasic | Quote::MultiLiteral => {
                    let quote = if self.quote == Quote::MultiBasic {
                        b'"'
                    } else {
                        b'\''
                    };
                    if byte == b'\\' && quote == b'"' {
                        index += 2;
                        continue;
                    }
                    if byte == quote {
                        let length = run(index, quote);
                        if length >= 3 {
                            self.quote = Quote::None;
                        }
                        index += length;
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
    newer_version: Option<i64>,
}

/// One recognized table: how many fields the repair recognized in it, and
/// the ones it kept.
#[derive(Debug, Default)]
struct Section {
    fields: usize,
    kept: usize,
    table: Table,
}

impl Section {
    fn recovery(&self) -> ViewRecovery {
        if self.kept == self.fields {
            ViewRecovery::Recovered
        } else if self.kept == 0 {
            ViewRecovery::Lost
        } else {
            ViewRecovery::Partly
        }
    }

    /// Keeps a valid field the parser read, which can't clash with another.
    fn read(&mut self, key: &Key, item: &Item, valid: fn(&str, &Item) -> bool) {
        self.fields += 1;
        if valid(key.get(), item) {
            self.table.insert_formatted(key, item.clone());
            self.kept += 1;
        }
    }

    /// Keeps a field from the broken tail that parses alone, is valid, and
    /// doesn't clash with a field already kept, so the first value wins.
    fn offer(&mut self, text: Option<String>, valid: fn(&str, &Item) -> bool) {
        self.fields += 1;
        let Some(document) = text.and_then(|text| text.parse::<DocumentMut>().ok()) else {
            return;
        };
        let mut keys = document.iter();
        let (Some((name, item)), None) = (keys.next(), keys.next()) else {
            return;
        };
        let key = document
            .as_table()
            .key(name)
            .cloned()
            .unwrap_or_else(|| Key::new(name));
        if valid(name, item) && merge(&mut self.table, &key, item.clone()) {
            self.kept += 1;
        }
    }
}

/// Adds `item` unless it redefines something, merging dotted keys like
/// `future.a` and `future.b`. All or nothing.
fn merge(table: &mut Table, key: &Key, item: Item) -> bool {
    let Some(existing) = table.get_mut(key.get()) else {
        table.insert_formatted(key, item);
        return true;
    };
    let (Some(existing), Item::Table(new)) = (existing.as_table_mut(), &item) else {
        return false;
    };
    if !existing.is_dotted() || !new.is_dotted() {
        return false;
    }
    let mut merged = existing.clone();
    for (name, child) in new.iter() {
        let key = new.key(name).cloned().unwrap_or_else(|| Key::new(name));
        if !merge(&mut merged, &key, child.clone()) {
            return false;
        }
    }
    *existing = merged;
    true
}

#[derive(Clone, Copy)]
enum Current {
    Top,
    View,
    Entry(usize),
    Unattributed,
}

/// Reads as much of the file as the TOML parser accepts, then scans the
/// broken rest unit by unit.
fn salvage(bytes: &[u8]) -> Salvaged {
    let lines = Lines::new(bytes);
    let (end, document) = parsed_prefix(&lines);
    let mut salvaged = Salvaged::default();
    let mut current = salvaged.read_prefix(&document);
    let mut top = true;
    for unit in units(&lines, end) {
        let (text, key_shaped) = match unit {
            Unit::Header { header, .. } => {
                top = false;
                current = match header {
                    Some(Header::View) => {
                        salvaged.view.get_or_insert_with(Section::default);
                        Current::View
                    }
                    Some(Header::Entry(name)) => Current::Entry(salvaged.entry(name)),
                    None => {
                        salvaged.dropped_text = true;
                        Current::Unattributed
                    }
                };
                continue;
            }
            Unit::Value {
                text, key_shaped, ..
            } => (text, key_shaped),
        };
        match current {
            Current::Top => {
                let version = text
                    .and_then(|text| text.parse::<DocumentMut>().ok())
                    .and_then(|document| {
                        document.get("schema_version").map(|item| item.as_integer())
                    });
                match version {
                    // Written fresh, so an old one isn't a loss.
                    Some(version) => {
                        if top
                            && salvaged.newer_version.is_none()
                            && document.get("schema_version").is_none()
                        {
                            salvaged.newer_version = version.filter(|v| *v > SCHEMA_VERSION);
                        }
                    }
                    None => salvaged.dropped_text = true,
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

/// The longest run of whole lines from the start that parses, and its
/// document. Stops before any line that isn't UTF-8.
fn parsed_prefix(lines: &Lines<'_>) -> (usize, DocumentMut) {
    let limit = (0..lines.len())
        .find(|index| std::str::from_utf8(lines.line(*index)).is_err())
        .unwrap_or(lines.len());
    // Where a unit starts is only a hint for where to try next; whatever
    // prefix is used, the parser decides what it means.
    let mut boundaries: Vec<usize> = units(lines, 0)
        .iter()
        .map(Unit::start)
        .filter(|start| *start < limit)
        .collect();
    boundaries.insert(0, 0);
    let mut end = limit;
    loop {
        let text = lines.prefix(end).unwrap_or_default();
        match text.parse::<DocumentMut>() {
            Ok(document) => return (end, document),
            Err(error) => {
                let at = error.span().map_or(text.len(), |span| span.start);
                let bound = lines.line_of(at).min(end.saturating_sub(1));
                end = boundaries
                    .iter()
                    .rev()
                    .copied()
                    .find(|start| *start <= bound)
                    .unwrap_or(0);
            }
        }
    }
}

impl Salvaged {
    fn entry(&mut self, name: String) -> usize {
        match self.entries.iter().position(|(seen, _)| *seen == name) {
            Some(index) => index,
            None => {
                self.entries.push((name, Section::default()));
                self.entries.len() - 1
            }
        }
    }

    /// Takes what the parser read, and says which table the broken tail
    /// continues.
    fn read_prefix(&mut self, document: &DocumentMut) -> Current {
        if let Some(version) = document.get("schema_version").and_then(Item::as_integer)
            && version > SCHEMA_VERSION
        {
            self.newer_version = Some(version);
        }
        for (key, item) in document.iter() {
            match (key, item.as_table_like()) {
                ("schema_version", _) => {}
                ("view", Some(table)) => {
                    let view = self.view.get_or_insert_with(Section::default);
                    for (name, field) in table.iter() {
                        let key = table.key(name).cloned().unwrap_or_else(|| Key::new(name));
                        view.read(&key, field, valid_view_field);
                    }
                }
                ("environments", Some(environments)) => {
                    for (name, entry) in environments.iter() {
                        let Some(entry) = entry.as_table_like() else {
                            self.dropped_text = true;
                            continue;
                        };
                        let index = self.entry(name.to_owned());
                        let section = &mut self.entries[index].1;
                        for (field, value) in entry.iter() {
                            let key = entry.key(field).cloned().unwrap_or_else(|| Key::new(field));
                            section.read(&key, value, valid_entry_field);
                        }
                    }
                }
                _ => self.dropped_text = true,
            }
        }
        match last_header(document.as_table(), &mut Vec::new(), &mut None) {
            None => Current::Top,
            Some(path) => match path.as_slice() {
                [view] if view == "view" && self.view.is_some() => Current::View,
                [environments, name] if environments == "environments" => {
                    match self.entries.iter().position(|(seen, _)| seen == name) {
                        Some(index) => Current::Entry(index),
                        None => Current::Unattributed,
                    }
                }
                _ => Current::Unattributed,
            },
        }
    }
}

/// The path of the header that comes last in the file, which is the table
/// lines after it would belong to.
fn last_header(
    table: &Table,
    path: &mut Vec<String>,
    last: &mut Option<(isize, Vec<String>)>,
) -> Option<Vec<String>> {
    for (key, item) in table.iter() {
        path.push(key.to_owned());
        let mut visit = |child: &Table, path: &mut Vec<String>| {
            if !child.is_implicit()
                && !child.is_dotted()
                && let Some(position) = child.position()
                && last.as_ref().is_none_or(|(seen, _)| position > *seen)
            {
                *last = Some((position, path.clone()));
            }
            last_header(child, path, last);
        };
        match item {
            Item::Table(child) => visit(child, path),
            Item::ArrayOfTables(array) => {
                for child in array.iter() {
                    // A line after an array of tables header belongs to
                    // no recognized table.
                    path.push(String::new());
                    visit(child, path);
                    path.pop();
                }
            }
            _ => {}
        }
        path.pop();
    }
    last.as_ref().map(|(_, path)| path.clone())
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
        "descending" | "favorites_first" | "poster_only" => item.as_bool().is_some(),
        "zoom" => read_zoom(item).is_some(),
        _ => true,
    }
}

impl Salvaged {
    fn render(&self) -> String {
        let mut document = DocumentMut::new();
        document.insert("schema_version", Item::Value(SCHEMA_VERSION.into()));
        if let Some(view) = self.view.as_ref().filter(|view| view.kept > 0) {
            document.insert("view", Item::Table(fresh_table(&view.table)));
        }
        if !self.entries.is_empty() {
            let mut environments = Table::new();
            environments.set_implicit(true);
            for (name, entry) in &self.entries {
                environments.insert(name, Item::Table(fresh_table(&entry.table)));
            }
            document.insert("environments", Item::Table(environments));
        }
        let mut text = document.to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text
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

/// A header table with the kept fields, laid out in the order they're
/// written rather than where they were in the broken file.
fn fresh_table(fields: &Table) -> Table {
    let mut table = fields.clone();
    table.set_implicit(false);
    table.set_dotted(false);
    *table.decor_mut() = Default::default();
    clear_positions(&mut table);
    table
}

fn clear_positions(table: &mut Table) {
    table.set_position(None);
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(child) => clear_positions(child),
            Item::ArrayOfTables(array) => array.iter_mut().for_each(clear_positions),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
