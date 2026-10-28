//! Custom GUI themes: one `themes/<id>.toml` each in the config root.
//!
//! Core only stores them. The app owns the palettes and every fallback
//! between colors, so a missing or bad color reads as None here and the app
//! fills it from `based_on`. Reads are lenient, like `library.toml`, so a
//! theme file never stops the app.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Value};

use crate::error::{Error, IoContext, Result};
use crate::library::{store_value, table_mut};
use crate::operation::Locks;
use crate::paths::Paths;

const SCHEMA_VERSION: i64 = 1;

/// `custom:<id>` in `library.toml` names the theme file `<id>.toml`, so a
/// custom theme can never shadow a built-in id.
pub const CUSTOM_PREFIX: &str = "custom:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    Dark,
    Light,
}

impl ThemeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }
}

/// The built-in theme ids and their kinds, in the order the app lists them.
pub const BUILT_IN: [(&str, ThemeKind); 8] = [
    ("midnight", ThemeKind::Dark),
    ("daylight", ThemeKind::Light),
    ("cellar", ThemeKind::Dark),
    ("cellar-light", ThemeKind::Light),
    ("high-contrast", ThemeKind::Dark),
    ("dracula", ThemeKind::Dark),
    ("catppuccin-macchiato", ThemeKind::Dark),
    ("catppuccin-latte", ThemeKind::Light),
];

pub fn builtin_kind(id: &str) -> Option<ThemeKind> {
    BUILT_IN
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, kind)| *kind)
}

/// The color roles, as the keys under `[colors]`.
pub const ROLES: [&str; 9] = [
    "background",
    "surface",
    "border",
    "foreground",
    "muted",
    "primary",
    "success",
    "warning",
    "danger",
];

/// One optional color per role, each a lowercase `#rrggbb` when set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Colors {
    pub background: Option<String>,
    pub surface: Option<String>,
    pub border: Option<String>,
    pub foreground: Option<String>,
    pub muted: Option<String>,
    pub primary: Option<String>,
    pub success: Option<String>,
    pub warning: Option<String>,
    pub danger: Option<String>,
}

impl Colors {
    /// Each role's key with its color, in `ROLES` order.
    pub fn roles(&self) -> [(&'static str, Option<&str>); 9] {
        [
            ("background", self.background.as_deref()),
            ("surface", self.surface.as_deref()),
            ("border", self.border.as_deref()),
            ("foreground", self.foreground.as_deref()),
            ("muted", self.muted.as_deref()),
            ("primary", self.primary.as_deref()),
            ("success", self.success.as_deref()),
            ("warning", self.warning.as_deref()),
            ("danger", self.danger.as_deref()),
        ]
    }

    fn slot(&mut self, role: &str) -> Option<&mut Option<String>> {
        Some(match role {
            "background" => &mut self.background,
            "surface" => &mut self.surface,
            "border" => &mut self.border,
            "foreground" => &mut self.foreground,
            "muted" => &mut self.muted,
            "primary" => &mut self.primary,
            "success" => &mut self.success,
            "warning" => &mut self.warning,
            "danger" => &mut self.danger,
            _ => return None,
        })
    }
}

/// `#rrggbb` in lowercase, or None for anything else.
pub fn normalize_color(text: &str) -> Option<String> {
    let digits = text.strip_prefix('#')?;
    (digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeFile {
    /// The file name without `.toml`.
    pub id: String,
    /// The `name` key, or the id when there is none.
    pub name: String,
    /// None when missing or unknown; the app treats it as dark.
    pub kind: Option<ThemeKind>,
    /// Only a built-in id; a custom, unknown, or missing one is None.
    pub based_on: Option<String>,
    pub colors: Colors,
    /// A newer wineo wrote it. It reads, but edits are refused so its data
    /// isn't thrown away.
    pub newer_version: bool,
}

/// A file in `themes/` that couldn't be read as a theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidTheme {
    /// The file name without `.toml`.
    pub id: String,
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Themes {
    /// Sorted by name, ignoring case, then id.
    pub themes: Vec<ThemeFile>,
    /// Sorted by path.
    pub invalid: Vec<InvalidTheme>,
}

/// Reads every `*.toml` in the themes folder. A missing folder is an empty
/// list; one that can't be listed is reported as invalid so `validate` says so.
pub fn list(paths: &Paths) -> Themes {
    let dir = paths.themes_dir();
    let mut themes = Themes::default();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return themes,
        Err(error) => {
            themes.invalid.push(InvalidTheme {
                id: String::new(),
                path: dir,
                error: error.to_string(),
            });
            return themes;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "toml") || !path.is_file() {
            continue;
        }
        // A name that isn't UTF-8 can't be written in library.toml.
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let id = id.to_owned();
        let read = fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|text| parse(&id, &text));
        match read {
            Ok(theme) => themes.themes.push(theme),
            Err(error) => themes.invalid.push(InvalidTheme { id, path, error }),
        }
    }
    themes.themes.sort_by(|a, b| {
        (a.name.to_lowercase(), &a.name, &a.id).cmp(&(b.name.to_lowercase(), &b.name, &b.id))
    });
    themes.invalid.sort_by(|a, b| a.path.cmp(&b.path));
    themes
}

fn theme_path(paths: &Paths, id: &str) -> Result<PathBuf> {
    if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\0']) {
        return Err(Error::InvalidThemeId(id.to_owned()));
    }
    Ok(paths.themes_dir().join(format!("{id}.toml")))
}

/// Parses a theme file; Err is why it doesn't.
fn parse(id: &str, text: &str) -> std::result::Result<ThemeFile, String> {
    let document = parse_document(text)?;
    let newer_version = match document.get("schema_version") {
        None => false,
        Some(item) => match item.as_integer() {
            Some(SCHEMA_VERSION) => false,
            Some(version) if version > SCHEMA_VERSION => true,
            _ => {
                return Err(format!(
                    "unsupported schema_version {}",
                    item.to_string().trim()
                ));
            }
        },
    };
    let name = document
        .get("name")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(id);
    let kind = document
        .get("kind")
        .and_then(Item::as_str)
        .and_then(ThemeKind::from_str);
    let based_on = document
        .get("based_on")
        .and_then(Item::as_str)
        .filter(|id| builtin_kind(id).is_some())
        .map(str::to_owned);
    let mut colors = Colors::default();
    if let Some(table) = document.get("colors").and_then(Item::as_table_like) {
        for role in ROLES {
            let color = table
                .get(role)
                .and_then(Item::as_str)
                .and_then(normalize_color);
            if let Some(slot) = colors.slot(role) {
                *slot = color;
            }
        }
    }
    Ok(ThemeFile {
        id: id.to_owned(),
        name: name.to_owned(),
        kind,
        based_on,
        colors,
        newer_version,
    })
}

fn parse_document(text: &str) -> std::result::Result<DocumentMut, String> {
    text.parse::<DocumentMut>().map_err(|error| {
        let reason = error.message().trim().to_owned();
        match error.span() {
            Some(span) => {
                let line = 1 + text.as_bytes()[..span.start]
                    .iter()
                    .filter(|byte| **byte == b'\n')
                    .count();
                format!("line {line}: {reason}")
            }
            None => reason,
        }
    })
}

/// `My Cellar!` becomes `my-cellar`: lowercase ASCII letters and digits, with
/// any run of anything else as one `-`, trimmed. Empty becomes `theme`.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "theme".to_owned()
    } else {
        slug.to_owned()
    }
}

/// Every write holds one lock on the folder, since a new id and a name
/// another theme uses are both facts about the whole folder, not one file.
fn lock(paths: &Paths) -> Result<crate::operation::ConfigGuard> {
    Locks::new(paths).config(&paths.themes_dir())
}

/// Writes a new theme and returns its id: a slug of `name`, with `-2`, `-3`,
/// and so on while that file exists. The id never changes afterwards.
pub fn create(
    paths: &Paths,
    name: &str,
    kind: ThemeKind,
    based_on: Option<&str>,
    colors: &Colors,
) -> Result<String> {
    let _guard = lock(paths)?;
    let base = slug(name);
    let mut id = base.clone();
    let mut attempt = 1;
    let path = loop {
        let path = theme_path(paths, &id)?;
        if fs::symlink_metadata(&path).is_err() {
            break path;
        }
        attempt += 1;
        id = format!("{base}-{attempt}");
    };
    let mut document = DocumentMut::new();
    document["schema_version"] = Item::Value(Value::from(SCHEMA_VERSION));
    document["name"] = Item::Value(Value::from(name.trim()));
    document["kind"] = Item::Value(Value::from(kind.as_str()));
    if let Some(based_on) = based_on.filter(|id| builtin_kind(id).is_some()) {
        document["based_on"] = Item::Value(Value::from(based_on));
    }
    set_colors(&path, &mut document, colors)?;
    crate::files::write_atomic(&path, document.to_string())?;
    Ok(id)
}

/// Rewrites the kind, `based_on`, and colors of an existing theme, leaving
/// its name, comments, and keys it doesn't know alone. A None `based_on` or
/// color removes the key. Returns whether the file changed.
pub fn update(
    paths: &Paths,
    id: &str,
    kind: ThemeKind,
    based_on: Option<&str>,
    colors: &Colors,
) -> Result<bool> {
    let path = theme_path(paths, id)?;
    let _guard = lock(paths)?;
    let mut document = editable(&path, id)?;
    let mut changed = false;
    if document.get("kind").and_then(Item::as_str) != Some(kind.as_str()) {
        store_value(document.as_table_mut(), "kind", Value::from(kind.as_str()));
        changed = true;
    }
    match based_on.filter(|id| builtin_kind(id).is_some()) {
        Some(based_on) => {
            if document.get("based_on").and_then(Item::as_str) != Some(based_on) {
                store_value(document.as_table_mut(), "based_on", Value::from(based_on));
                changed = true;
            }
        }
        None => changed |= document.remove("based_on").is_some(),
    }
    changed |= set_colors(&path, &mut document, colors)?;
    if !changed {
        return Ok(false);
    }
    crate::files::write_atomic(&path, document.to_string())?;
    Ok(true)
}

/// Changes only the `name` key, so the id and file name, and with them
/// `library.toml`'s `custom:<id>`, stay put. Another theme's name is taken
/// whatever its case; this theme's own name in another case is fine.
pub fn rename(paths: &Paths, id: &str, new_name: &str) -> Result<bool> {
    let path = theme_path(paths, id)?;
    let _guard = lock(paths)?;
    let mut document = editable(&path, id)?;
    let new_name = new_name.trim();
    let taken = list(paths)
        .themes
        .iter()
        .any(|other| other.id != id && other.name.to_lowercase() == new_name.to_lowercase());
    if taken {
        return Err(Error::ThemeNameTaken);
    }
    if document.get("name").and_then(Item::as_str) == Some(new_name) {
        return Ok(false);
    }
    store_value(document.as_table_mut(), "name", Value::from(new_name));
    crate::files::write_atomic(&path, document.to_string())?;
    Ok(true)
}

/// Removes the file. Returns whether there was one; none is not an error.
/// A newer wineo's theme can still be deleted, since that is an explicit ask.
pub fn delete(paths: &Paths, id: &str) -> Result<bool> {
    let path = theme_path(paths, id)?;
    let _guard = lock(paths)?;
    crate::files::remove_if_present(&path)
        .context(|| format!("failed to remove {}", path.display()))
}

/// The file's document, refusing one that doesn't parse or that a newer
/// wineo wrote.
fn editable(path: &Path, id: &str) -> Result<DocumentMut> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(Error::UnknownTheme(id.to_owned()));
        }
        Err(error) => {
            return Err(error).context(|| format!("failed to read {}", path.display()));
        }
    };
    let document = text
        .parse::<DocumentMut>()
        .map_err(|error| Error::ConfigParse {
            path: path.to_owned(),
            source: Box::new(error),
        })?;
    if let Some(version) = document.get("schema_version")
        && version.as_integer() != Some(SCHEMA_VERSION)
    {
        return Err(Error::SchemaVersion {
            path: path.to_owned(),
            found: version.to_string().trim().to_owned(),
        });
    }
    Ok(document)
}

/// Sets each role's key, or removes it for None. Returns whether anything
/// changed. A value that isn't `#rrggbb` is an error, so a typo never
/// silently drops a saved color.
fn set_colors(path: &Path, document: &mut DocumentMut, colors: &Colors) -> Result<bool> {
    let mut wanted = Vec::new();
    for (role, color) in colors.roles() {
        let color = match color {
            None => None,
            Some(text) => Some(normalize_color(text).ok_or_else(|| Error::ConfigInvalid {
                path: path.to_owned(),
                message: format!("{role}: use #rrggbb"),
            })?),
        };
        wanted.push((role, color));
    }
    let mut changed = false;
    for (role, color) in wanted {
        let current = document
            .get("colors")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get(role));
        match color {
            Some(color) => {
                // Compared normalized, so a hand-written uppercase value
                // isn't rewritten for nothing.
                let same = current
                    .and_then(Item::as_str)
                    .and_then(normalize_color)
                    .is_some_and(|current| current == color);
                if !same {
                    store_value(
                        table_mut(document.as_table_mut(), "colors"),
                        role,
                        Value::from(color),
                    );
                    changed = true;
                }
            }
            None if current.is_some() => {
                if let Some(table) = document.get_mut("colors").and_then(Item::as_table_like_mut) {
                    table.remove(role);
                    changed = true;
                }
            }
            None => {}
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests;
