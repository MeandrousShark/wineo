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
mod preset;
mod record;
mod repair;
mod sort;

pub use cache::Cache;
pub(crate) use check::{FindingKind, inspect, remove_strays};
pub use preset::{
    PRESET_NAME_MAX_CHARS, Preset, create_preset, normalize_preset_name, preset_name_problem,
    preset_name_taken, remove_preset, rename_preset, set_preset,
};
pub use record::{
    Claim, RecordChanges, Recorded, SteamRecord, apply_changes, claim_game, dismiss_game,
    dismiss_games, finish_import, mark_baselined, record_games, release_claim, set_auto_import,
};
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
    Tags,
}

impl SortKey {
    pub const ALL: [Self; 6] = [
        Self::Name,
        Self::LastPlayed,
        Self::Playtime,
        Self::Added,
        Self::Source,
        Self::Tags,
    ];

    /// Whether the key starts descending when picked: the text keys run A
    /// to Z, and the rest lead with the most recent, most played, or newest.
    pub fn starts_descending(self) -> bool {
        !matches!(self, Self::Name | Self::Source | Self::Tags)
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "name" => Some(Self::Name),
            "last-played" => Some(Self::LastPlayed),
            "playtime" => Some(Self::Playtime),
            "added" => Some(Self::Added),
            "source" => Some(Self::Source),
            "tags" => Some(Self::Tags),
            _ => None,
        }
    }
}

/// What the desktop app's random picker draws from, saved as `random`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RandomPool {
    /// Shown games that can launch now, unless none can.
    #[default]
    Launchable,
    /// Every shown game.
    Any,
}

impl RandomPool {
    pub const ALL: [Self; 2] = [Self::Launchable, Self::Any];

    /// The value `library.toml` holds.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Launchable => "launchable",
            Self::Any => "any",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "launchable" => Some(Self::Launchable),
            "any" => Some(Self::Any),
            _ => None,
        }
    }
}

/// How the desktop app lays out the library, saved as `layout`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ViewLayout {
    /// Posters in rows.
    #[default]
    Grid,
    /// One row per game, with columns.
    List,
}

impl ViewLayout {
    pub const ALL: [Self; 2] = [Self::Grid, Self::List];

    /// The value `library.toml` holds.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::List => "list",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "grid" => Some(Self::Grid),
            "list" => Some(Self::List),
            _ => None,
        }
    }
}

/// What the desktop app groups the library into sections by, saved as
/// `group`. Recently played and Hidden sit around any grouping but `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum GroupBy {
    /// One flat library, with no sections.
    #[default]
    None,
    /// Nothing but the pinned sections and an All with the whole library
    /// in the shown order.
    Recent,
    /// A section per tag, so a game with several shows in each.
    Tag,
    /// A section per store, else platform.
    Source,
    /// Running, not installed, and everything else.
    State,
}

impl GroupBy {
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Recent,
        Self::Tag,
        Self::Source,
        Self::State,
    ];

    /// The value `library.toml` holds.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Recent => "recent",
            Self::Tag => "tag",
            Self::Source => "source",
            Self::State => "state",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "recent" => Some(Self::Recent),
            "tag" => Some(Self::Tag),
            "source" => Some(Self::Source),
            "state" => Some(Self::State),
            _ => None,
        }
    }
}

/// The grid's zoom in whole percent: any integer in range is valid, so the
/// slider can rest anywhere.
pub const ZOOM_MIN: u16 = 60;
pub const ZOOM_MAX: u16 = 200;
pub const ZOOM_DEFAULT: u16 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub sort: SortKey,
    pub descending: bool,
    pub favorites_first: bool,
    pub zoom: u16,
    pub poster_only: bool,
    pub random: RandomPool,
    pub layout: ViewLayout,
    pub group: GroupBy,
}

impl Default for View {
    fn default() -> Self {
        Self {
            sort: SortKey::default(),
            descending: false,
            favorites_first: false,
            zoom: ZOOM_DEFAULT,
            poster_only: false,
            random: RandomPool::default(),
            layout: ViewLayout::default(),
            group: GroupBy::default(),
        }
    }
}

/// How the desktop app is themed. Core only stores the ids; the app owns the
/// palettes, and an id it can't use falls back there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appearance {
    pub follow_system: bool,
    /// Used while following the system and it's dark.
    pub dark: String,
    /// Used while following the system and it's light.
    pub light: String,
    /// Used when `follow_system` is off.
    pub theme: String,
    pub improve_readability: bool,
    pub motion: Motion,
}

/// Whether the desktop app animates movement. `System` follows the
/// desktop's own animation setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Motion {
    #[default]
    System,
    Full,
    Reduced,
}

impl Motion {
    pub const ALL: [Self; 3] = [Self::System, Self::Full, Self::Reduced];

    /// The value `library.toml` holds.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Full => "full",
            Self::Reduced => "reduced",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "full" => Some(Self::Full),
            "reduced" => Some(Self::Reduced),
            _ => None,
        }
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            follow_system: true,
            dark: "midnight".to_owned(),
            light: "daylight".to_owned(),
            theme: "midnight".to_owned(),
            improve_readability: false,
            motion: Motion::default(),
        }
    }
}

/// A saved zoom, or None for a value that isn't an integer in range.
fn read_zoom(item: &Item) -> Option<u16> {
    item.as_integer()
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| (ZOOM_MIN..=ZOOM_MAX).contains(value))
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
    pub appearance: Appearance,
    pub environments: BTreeMap<String, Entry>,
    /// Saved views by name, A to Z ignoring case.
    pub presets: Vec<(String, Preset)>,
    /// The keys under `[presets]` that aren't presets, whose names are
    /// taken all the same.
    pub other_presets: Vec<String>,
    pub steam: SteamRecord,
    pub problem: Option<Problem>,
}

impl Library {
    /// The preset saved under exactly `name`.
    pub fn preset(&self, name: &str) -> Option<&Preset> {
        self.presets
            .iter()
            .find(|(saved, _)| saved == name)
            .map(|(_, preset)| preset)
    }

    /// The entry under `[presets]` that a new preset called `name` would
    /// collide with; see `preset_name_taken`. The app offers to update it
    /// instead of saving a second, look-alike one, unless it isn't a preset.
    pub fn taken_preset_name(&self, name: &str) -> Option<&str> {
        let names = self
            .presets
            .iter()
            .map(|(saved, _)| saved.as_str())
            .chain(self.other_presets.iter().map(String::as_str));
        preset_name_taken(names, name, None)
    }
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
    edit(paths, |document| {
        set_view_choice(document, "sort", sort, sort_name(sort), SortKey::from_str)
    })
}

/// `set_sort` and `set_descending` in one write, for picking a key that
/// starts in its own direction.
pub fn set_order(paths: &Paths, sort: SortKey, descending: bool) -> Result<bool> {
    edit(paths, |document| {
        let sorted = set_view_choice(document, "sort", sort, sort_name(sort), SortKey::from_str);
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

/// Out-of-range zooms clamp, so a caller never saves a value the reader
/// would ignore.
pub fn set_zoom(paths: &Paths, zoom: u16) -> Result<bool> {
    let zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    edit(paths, |document| set_view_zoom(document, zoom))
}

pub fn set_poster_only(paths: &Paths, poster_only: bool) -> Result<bool> {
    edit(paths, |document| {
        set_view_bool(document, "poster_only", poster_only)
    })
}

pub fn set_random(paths: &Paths, random: RandomPool) -> Result<bool> {
    edit(paths, |document| {
        set_view_choice(
            document,
            "random",
            random,
            random.as_str(),
            RandomPool::parse,
        )
    })
}

pub fn set_layout(paths: &Paths, layout: ViewLayout) -> Result<bool> {
    edit(paths, |document| {
        set_view_choice(
            document,
            "layout",
            layout,
            layout.as_str(),
            ViewLayout::parse,
        )
    })
}

pub fn set_group(paths: &Paths, group: GroupBy) -> Result<bool> {
    edit(paths, |document| {
        set_view_choice(document, "group", group, group.as_str(), GroupBy::parse)
    })
}

/// Writes only the fields that differ from what the file reads as, so an
/// unchanged `Appearance` never rewrites it.
pub fn set_appearance(paths: &Paths, appearance: &Appearance) -> Result<bool> {
    edit(paths, |document| {
        let default = Appearance::default();
        let mut changed = false;
        changed |= set_appearance_bool(
            document,
            "follow_system",
            appearance.follow_system,
            default.follow_system,
        );
        changed |= set_appearance_text(document, "dark", &appearance.dark, &default.dark);
        changed |= set_appearance_text(document, "light", &appearance.light, &default.light);
        changed |= set_appearance_text(document, "theme", &appearance.theme, &default.theme);
        changed |= set_appearance_bool(
            document,
            "improve_readability",
            appearance.improve_readability,
            default.improve_readability,
        );
        changed |= set_appearance_motion(document, appearance.motion);
        changed
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
    let guard = Locks::new(paths).config(&path)?;
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
    crate::files::write_config(&path, guard.config_target(), document.to_string())?;
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
            zoom: table
                .get("zoom")
                .and_then(read_zoom)
                .unwrap_or(ZOOM_DEFAULT),
            poster_only: table
                .get("poster_only")
                .and_then(Item::as_bool)
                .unwrap_or_default(),
            random: table
                .get("random")
                .and_then(Item::as_str)
                .and_then(RandomPool::parse)
                .unwrap_or_default(),
            layout: table
                .get("layout")
                .and_then(Item::as_str)
                .and_then(ViewLayout::parse)
                .unwrap_or_default(),
            group: table
                .get("group")
                .and_then(Item::as_str)
                .and_then(GroupBy::parse)
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
        appearance: read_appearance(document),
        environments,
        presets: preset::read_presets(document),
        other_presets: preset::read_other_presets(document),
        steam: record::read_record(document),
        problem: None,
    }
}

fn read_appearance(document: &DocumentMut) -> Appearance {
    let default = Appearance::default();
    let Some(table) = document.get("appearance").and_then(Item::as_table_like) else {
        return default;
    };
    let text = |field: &str, default: String| {
        table
            .get(field)
            .and_then(Item::as_str)
            .map_or(default, str::to_owned)
    };
    let flag =
        |field: &str, default: bool| table.get(field).and_then(Item::as_bool).unwrap_or(default);
    Appearance {
        follow_system: flag("follow_system", default.follow_system),
        dark: text("dark", default.dark),
        light: text("light", default.light),
        theme: text("theme", default.theme),
        improve_readability: flag("improve_readability", default.improve_readability),
        motion: read_motion(table.get("motion")),
    }
}

/// An unknown or mistyped value reads as the default.
fn read_motion(item: Option<&Item>) -> Motion {
    item.and_then(Item::as_str)
        .and_then(Motion::parse)
        .unwrap_or_default()
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

/// Sets a `[view]` field that holds one of an enum's names. A missing field
/// reads as the default, and one that already reads as `value` is left as
/// written, so a no-op edit never rewrites the file.
fn set_view_choice<T: Copy + PartialEq + Default>(
    document: &mut DocumentMut,
    field: &str,
    value: T,
    name: &str,
    parse: fn(&str) -> Option<T>,
) -> bool {
    let current = document
        .get("view")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get(field));
    if preset::choice_matches(current, value, parse) {
        return false;
    }
    store_value(view_table_mut(document), field, Value::from(name));
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

fn set_view_zoom(document: &mut DocumentMut, zoom: u16) -> bool {
    let current = document
        .get("view")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get("zoom"));
    let matches = match current {
        None => zoom == ZOOM_DEFAULT,
        Some(item) => read_zoom(item) == Some(zoom),
    };
    if matches {
        return false;
    }
    store_value(
        view_table_mut(document),
        "zoom",
        Value::from(i64::from(zoom)),
    );
    true
}

fn set_appearance_bool(
    document: &mut DocumentMut,
    field: &str,
    value: bool,
    default: bool,
) -> bool {
    let current = document
        .get("appearance")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get(field));
    let matches = match current {
        None => value == default,
        Some(item) => item.as_bool() == Some(value),
    };
    if matches {
        return false;
    }
    store_value(appearance_table_mut(document), field, Value::from(value));
    true
}

fn set_appearance_text(
    document: &mut DocumentMut,
    field: &str,
    value: &str,
    default: &str,
) -> bool {
    let current = document
        .get("appearance")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get(field));
    let matches = match current {
        None => value == default,
        Some(item) => item.as_str() == Some(value),
    };
    if matches {
        return false;
    }
    store_value(appearance_table_mut(document), field, Value::from(value));
    true
}

/// Compares with what the file reads as, so a value this wineo doesn't know,
/// maybe a newer one's, stays until the user picks another.
fn set_appearance_motion(document: &mut DocumentMut, motion: Motion) -> bool {
    let current = document
        .get("appearance")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get("motion"));
    if read_motion(current) == motion {
        return false;
    }
    store_value(
        appearance_table_mut(document),
        "motion",
        Value::from(motion.as_str()),
    );
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

fn appearance_table_mut(document: &mut DocumentMut) -> &mut dyn TableLike {
    table_mut(document.as_table_mut(), "appearance")
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

pub(crate) fn table_mut<'a>(parent: &'a mut Table, name: &str) -> &'a mut dyn TableLike {
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

pub(crate) fn store_value(table: &mut dyn TableLike, field: &str, mut value: Value) {
    // Replacing in place keeps the key, and with it a comment line above it.
    if let Some(old) = table.get_mut(field).and_then(Item::as_value_mut) {
        *value.decor_mut() = old.decor().clone();
        *old = value;
        return;
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
        SortKey::Tags => "tags",
    }
}

#[cfg(test)]
mod tests;
