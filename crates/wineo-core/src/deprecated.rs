//! Config keys that were renamed, and how a file with an old name still loads.
//!
//! Before a config is deserialized, `apply` moves each old key to its new
//! name in memory and records the move, so the result rides with the loaded
//! config. The file is never rewritten on read. A wineo edit keeps the
//! spelling the file has, since a release from before the rename only knows
//! the old one; `wineo validate --fix` renames it in place.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, InlineTable, Item, Key, Table, Value};

use crate::config::{EnvironmentConfig, GlobalConfig};
use crate::error::{Error, IoContext, Result};
use crate::files;
use crate::operation::Locks;
use crate::paths::Paths;

/// Which kind of config file a key lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Global,
    Environment,
    Template,
    Art,
}

/// One renamed key. Paths are dotted, and `*` stands for any key of a table
/// keyed by user names, like `targets.*.exe`. Only the last part is renamed,
/// so old and new share everything before it.
#[derive(Debug)]
pub struct Deprecated {
    pub file: FileKind,
    pub old: &'static str,
    pub new: &'static str,
    /// The release that deprecated the old name.
    pub since: &'static str,
}

impl Deprecated {
    fn parent(&self) -> Vec<&'static str> {
        let mut parts: Vec<&'static str> = self.old.split('.').collect();
        parts.pop();
        parts
    }

    fn old_leaf(&self) -> &'static str {
        leaf(self.old)
    }

    fn new_leaf(&self) -> &'static str {
        leaf(self.new)
    }
}

fn leaf(path: &'static str) -> &'static str {
    path.rsplit('.').next().unwrap_or(path)
}

/// Nothing is deprecated yet. The test entries alias real keys under names
/// no release ever wrote, so the machinery is proven without one.
#[cfg(not(any(test, feature = "test-support")))]
pub const TABLE: &[Deprecated] = &[];

#[cfg(any(test, feature = "test-support"))]
pub const TABLE: &[Deprecated] = &[
    Deprecated {
        file: FileKind::Global,
        old: "test_old_stop_timeout",
        new: "stop_timeout",
        since: "0.0.0",
    },
    Deprecated {
        file: FileKind::Global,
        old: "defaults.test_old_runner",
        new: "defaults.runner",
        since: "0.0.0",
    },
    Deprecated {
        file: FileKind::Environment,
        old: "test_old_gamemode",
        new: "gamemode",
        since: "0.0.0",
    },
    Deprecated {
        file: FileKind::Environment,
        old: "hooks.test_old_before",
        new: "hooks.before",
        since: "0.0.0",
    },
    Deprecated {
        file: FileKind::Environment,
        old: "targets.*.test_old_exe",
        new: "targets.*.exe",
        since: "0.0.0",
    },
    Deprecated {
        file: FileKind::Template,
        old: "test_old_tags",
        new: "tags",
        since: "0.0.0",
    },
    Deprecated {
        file: FileKind::Art,
        old: "test_old_game",
        new: "game",
        since: "0.0.0",
    },
    Deprecated {
        file: FileKind::Art,
        old: "targets.*.test_old_icon",
        new: "targets.*.icon",
        since: "0.0.0",
    },
];

/// A deprecated key a loaded config used, with its concrete path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub old: String,
    pub new: String,
    pub since: &'static str,
}

/// A deprecated key found in a file, before it is moved.
struct Found {
    parent: Vec<String>,
    entry: &'static Deprecated,
}

impl Found {
    fn moved(&self) -> Moved {
        let dotted = |leaf: &str| {
            let mut parts = self.parent.clone();
            parts.push(leaf.to_owned());
            parts.join(".")
        };
        Moved {
            old: dotted(self.entry.old_leaf()),
            new: dotted(self.entry.new_leaf()),
            since: self.entry.since,
        }
    }
}

/// The tables the pass walks, so the in-memory pass and the in-place rename
/// find the same keys.
trait Node {
    fn keys(&self) -> Vec<String>;
    fn child(&self, key: &str) -> Option<&Self>;
    fn has(&self, key: &str) -> bool;
}

impl Node for toml::Table {
    fn keys(&self) -> Vec<String> {
        self.keys().cloned().collect()
    }

    fn child(&self, key: &str) -> Option<&Self> {
        self.get(key)?.as_table()
    }

    fn has(&self, key: &str) -> bool {
        self.contains_key(key)
    }
}

/// Any table: a `[header]`, dotted keys, or an inline table.
impl Node for Item {
    fn keys(&self) -> Vec<String> {
        self.as_table_like()
            .map(|table| table.iter().map(|(key, _)| key.to_owned()).collect())
            .unwrap_or_default()
    }

    fn child(&self, key: &str) -> Option<&Self> {
        let child = self.as_table_like()?.get(key)?;
        child.is_table_like().then_some(child)
    }

    fn has(&self, key: &str) -> bool {
        self.as_table_like()
            .is_some_and(|table| table.contains_key(key))
    }
}

/// Every concrete path `pattern` names in `node`, `*` matching each key.
fn parents<N: Node + ?Sized>(node: &N, pattern: &[&str], at: Vec<String>) -> Vec<Vec<String>> {
    let Some((first, rest)) = pattern.split_first() else {
        return vec![at];
    };
    let keys = if *first == "*" {
        node.keys()
    } else {
        vec![(*first).to_owned()]
    };
    let mut found = Vec::new();
    for key in keys {
        if let Some(child) = node.child(&key) {
            let mut path = at.clone();
            path.push(key);
            found.extend(parents(child, rest, path));
        }
    }
    found
}

fn walk<'a, N: Node + ?Sized>(mut node: &'a N, path: &[String]) -> Option<&'a N> {
    for key in path {
        node = node.child(key)?;
    }
    Some(node)
}

/// The deprecated keys `root` uses. Both spellings at once is an error, since
/// there is no telling which one was meant.
fn find<N: Node + ?Sized>(file: FileKind, path: &Path, root: &N) -> Result<Vec<Found>> {
    let mut found = Vec::new();
    for entry in TABLE.iter().filter(|entry| entry.file == file) {
        for parent in parents(root, &entry.parent(), Vec::new()) {
            let node = walk(root, &parent).expect("just found");
            if !node.has(entry.old_leaf()) {
                continue;
            }
            let at = Found { parent, entry };
            if node.has(entry.new_leaf()) {
                let moved = at.moved();
                return Err(Error::ConfigInvalid {
                    path: path.to_owned(),
                    message: format!(
                        "both `{}` and `{}` are set; `{}` is the deprecated name, so remove it",
                        moved.old, moved.new, moved.old
                    ),
                });
            }
            found.push(at);
        }
    }
    Ok(found)
}

/// Moves every deprecated key in a parsed file to its new name. Runs right
/// before deserializing, on every config parse.
pub(crate) fn apply(file: FileKind, path: &Path, table: &mut toml::Table) -> Result<Vec<Moved>> {
    let found = find(file, path, &*table)?;
    for at in &found {
        let mut parent = &mut *table;
        for key in &at.parent {
            parent = parent
                .get_mut(key)
                .and_then(toml::Value::as_table_mut)
                .expect("just found");
        }
        let value = parent.remove(at.entry.old_leaf()).expect("just found");
        parent.insert(at.entry.new_leaf().to_owned(), value);
    }
    Ok(found.iter().map(Found::moved).collect())
}

/// A key `rename_document` moved, so `restore_document` can put it back
/// exactly as it was written.
#[derive(Debug, Clone)]
pub(crate) struct Renamed {
    parent: Vec<String>,
    old: Key,
    new: &'static str,
}

/// Renames every deprecated key in a document in place, keeping its comments,
/// formatting, and position. The same keys `apply` moves.
pub(crate) fn rename_document(
    file: FileKind,
    path: &Path,
    doc: &mut DocumentMut,
) -> Result<Vec<Renamed>> {
    let found = find(file, path, doc.as_item())?;
    let mut renamed = Vec::new();
    for at in found {
        let parent = walk_mut(doc.as_table_mut(), &at.parent).expect("just found");
        let old = parent
            .rename(at.entry.old_leaf(), Key::new(at.entry.new_leaf()))
            .expect("just found");
        renamed.push(Renamed {
            parent: at.parent,
            old,
            new: at.entry.new_leaf(),
        });
    }
    Ok(renamed)
}

/// Undoes `rename_document` for every key still there, so an edit keeps the
/// spelling the file had. A key the edit removed stays removed.
pub(crate) fn restore_document(doc: &mut DocumentMut, renamed: &[Renamed]) {
    for renamed in renamed.iter().rev() {
        if let Some(parent) = walk_mut(doc.as_table_mut(), &renamed.parent) {
            parent.rename(renamed.new, renamed.old.clone());
        }
    }
}

/// A key as the current release spells it, for an edit given either name.
pub(crate) fn current_name(file: FileKind, key: &str) -> String {
    let parts: Vec<&str> = key.split('.').collect();
    for entry in TABLE.iter().filter(|entry| entry.file == file) {
        let parent = entry.parent();
        if parts.len() <= parent.len()
            || parts[parent.len()] != entry.old_leaf()
            || !parent
                .iter()
                .zip(&parts)
                .all(|(pattern, part)| *pattern == "*" || pattern == part)
        {
            continue;
        }
        let mut renamed = parts.clone();
        renamed[parent.len()] = entry.new_leaf();
        return renamed.join(".");
    }
    key.to_owned()
}

enum TableMut<'a> {
    Table(&'a mut Table),
    Inline(&'a mut InlineTable),
}

fn walk_mut<'a>(root: &'a mut Table, path: &[String]) -> Option<TableMut<'a>> {
    let mut node = TableMut::Table(root);
    for key in path {
        node = node.child(key)?;
    }
    Some(node)
}

impl<'a> TableMut<'a> {
    fn child(self, key: &str) -> Option<TableMut<'a>> {
        match self {
            TableMut::Table(table) => match table.get_mut(key)? {
                Item::Table(table) => Some(TableMut::Table(table)),
                Item::Value(Value::InlineTable(table)) => Some(TableMut::Inline(table)),
                _ => None,
            },
            TableMut::Inline(table) => match table.get_mut(key)? {
                Value::InlineTable(table) => Some(TableMut::Inline(table)),
                _ => None,
            },
        }
    }

    /// Renames `from` to `to`, keeping its decor and its place among its
    /// siblings. Returns the key as it was.
    fn rename(self, from: &str, to: Key) -> Option<Key> {
        match self {
            TableMut::Table(table) => {
                let keys: Vec<String> = table.iter().map(|(key, _)| key.to_owned()).collect();
                let at = keys.iter().position(|key| key == from)?;
                // Entries only append, so everything from the key on is put
                // back in order behind it.
                let mut tail: Vec<(Key, Item)> = keys[at..]
                    .iter()
                    .filter_map(|key| table.remove_entry(key))
                    .collect();
                let (old, item) = tail.remove(0);
                table.insert_formatted(&with_decor(to, &old), item);
                for (key, item) in tail {
                    table.insert_formatted(&key, item);
                }
                Some(old)
            }
            TableMut::Inline(table) => {
                let keys: Vec<String> = table.iter().map(|(key, _)| key.to_owned()).collect();
                let at = keys.iter().position(|key| key == from)?;
                let mut tail: Vec<(Key, Value)> = keys[at..]
                    .iter()
                    .filter_map(|key| table.remove_entry(key))
                    .collect();
                let (old, value) = tail.remove(0);
                table.insert_formatted(&with_decor(to, &old), value);
                for (key, value) in tail {
                    table.insert_formatted(&key, value);
                }
                Some(old)
            }
        }
    }
}

/// `to` with `from`'s surroundings, unless `to` already has its own: a key
/// being put back carries the decor it was written with.
fn with_decor(to: Key, from: &Key) -> Key {
    if to.as_repr().is_some() {
        return to;
    }
    to.with_leaf_decor(from.leaf_decor().clone())
        .with_dotted_decor(from.dotted_decor().clone())
}

/// A config file that can hold deprecated keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Config {
    Global,
    Environment(String),
    Template(String),
    /// An environment's `art.toml`.
    Art(String),
}

impl Config {
    pub fn kind(&self) -> FileKind {
        match self {
            Config::Global => FileKind::Global,
            Config::Environment(_) => FileKind::Environment,
            Config::Template(_) => FileKind::Template,
            Config::Art(_) => FileKind::Art,
        }
    }

    pub fn path(&self, paths: &Paths) -> PathBuf {
        match self {
            Config::Global => paths.global_config(),
            Config::Environment(name) => paths.environment_config(name),
            Config::Template(name) => paths.templates_dir().join(format!("{name}.toml")),
            Config::Art(name) => paths.art_dir(name).join(crate::art::ART_FILE),
        }
    }

    /// Checks the renamed text the way a read would.
    fn check(&self, path: &Path, text: &str) -> Result<()> {
        match self {
            Config::Global => GlobalConfig::parse(path, text).map(drop),
            Config::Environment(_) => EnvironmentConfig::parse(path, text).map(drop),
            Config::Template(_) => crate::templates::parse(path, text).map(drop),
            Config::Art(_) => crate::art::ArtConfig::parse(path, text).map(drop),
        }
    }
}

/// Renames every deprecated key in `config` in place, keeping its comments,
/// formatting, and order. Returns whether anything changed.
pub fn rename_in_place(paths: &Paths, config: &Config) -> Result<bool> {
    if let Config::Environment(name) | Config::Template(name) | Config::Art(name) = config
        && !crate::name::is_valid(name)
    {
        return Err(Error::InvalidName(name.clone()));
    }
    let path = config.path(paths);
    let _guard = Locks::new(paths).config(&path)?;
    let Some(text) = files::read_to_string_optional(&path)
        .context(|| format!("failed to read {}", path.display()))?
    else {
        return Ok(false);
    };
    crate::config::check_schema(&path, &text)?;
    let mut doc: DocumentMut = text.parse().map_err(|source| Error::ConfigParse {
        path: path.clone(),
        source: Box::new(source),
    })?;
    if rename_document(config.kind(), &path, &mut doc)?.is_empty() {
        return Ok(false);
    }
    crate::config::stamp_schema(&mut doc);
    let updated = doc.to_string();
    config.check(&path, &updated)?;
    files::write_atomic(&path, &updated)?;
    Ok(true)
}

/// How many configs use a deprecated key, for the line a command prints.
/// A config that can't be read isn't counted; `validate` reports it.
pub fn count_configs(paths: &Paths) -> usize {
    if TABLE.is_empty() {
        return 0;
    }
    let mut count = 0;
    if GlobalConfig::load(paths).is_ok_and(|global| !global.deprecated.is_empty()) {
        count += 1;
    }
    let templates = crate::templates::Templates::new(paths);
    for listed in templates.list().unwrap_or_default() {
        if !listed.built_in
            && templates
                .resolve(&listed.name)
                .is_ok_and(|template| !template.deprecated.is_empty())
        {
            count += 1;
        }
    }
    for name in crate::environment::names(paths).unwrap_or_default() {
        let path = paths.environment_config(&name);
        if let Ok(Some(text)) = files::read_to_string_optional(&path)
            && EnvironmentConfig::parse(&path, &text)
                .is_ok_and(|config| !config.deprecated.is_empty())
        {
            count += 1;
        }
        if crate::art::ArtConfig::load(&paths.art_dir(&name))
            .is_ok_and(|art| !art.deprecated.is_empty())
        {
            count += 1;
        }
    }
    count
}

/// The line a command prints when any config uses a deprecated key.
pub fn summary(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some("1 config uses deprecated keys; see wineo validate".to_owned()),
        count => Some(format!(
            "{count} configs use deprecated keys; see wineo validate"
        )),
    }
}

#[cfg(test)]
mod tests;
