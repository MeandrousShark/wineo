//! Saved views: a name for a sort, filters, and layout, kept under
//! `[presets.<name>]` in `library.toml`.
//!
//! Read as leniently as `[view]`: a bad field falls back for that field only,
//! and a key or filter id this wineo doesn't know is ignored. Every write
//! sets only the lines it owns, so comments and keys a newer wineo added stay.

use std::collections::BTreeSet;

use toml_edit::{DocumentMut, Entry, Item, Key, Table, TableLike, Value};

use super::{GroupBy, SortKey, ViewLayout, edit, sort_name, store_value, table_mut};
use crate::error::{Error, Result};
use crate::name::fold_case;
use crate::paths::Paths;
use crate::settings::{push_tag, remove_tag};
use crate::source::{Platform, Store};

/// The longest preset name, in characters, so a pasted blurb can't swamp the
/// Filters popup that lists them.
pub const PRESET_NAME_MAX_CHARS: usize = 60;

/// How the library looks when the preset is applied. Search, zoom, and
/// poster-only aren't part of it, so applying one never resizes the grid or
/// leaves a stale query behind.
///
/// Filter ids are the lowercase `Store::id` and `Platform::id`; tags are
/// kept as typed, and a tag no environment has any more is kept too.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preset {
    pub sort: SortKey,
    pub descending: bool,
    pub favorites_first: bool,
    pub stores: BTreeSet<Store>,
    pub platforms: BTreeSet<Platform>,
    pub tags: BTreeSet<String>,
    pub not_stores: BTreeSet<Store>,
    pub not_platforms: BTreeSet<Platform>,
    pub not_tags: BTreeSet<String>,
    pub show_hidden: bool,
    pub group: GroupBy,
    pub layout: ViewLayout,
}

/// What is wrong with a preset name, as stored, finishing "a preset name
/// that ...". None when it's fine. Trimming is the caller's job when it takes
/// what a person typed; see `normalize_preset_name`. `validate` uses this
/// too, so it flags what the write API refuses.
pub(super) fn preset_name_rule(name: &str) -> Option<String> {
    let reason = if name.trim().is_empty() {
        "can't be empty".to_owned()
    } else if name.chars().any(char::is_control) {
        "can't contain control characters".to_owned()
    } else if name.trim() != name {
        "can't start or end with whitespace".to_owned()
    } else if name.chars().count() > PRESET_NAME_MAX_CHARS {
        format!("can't be longer than {PRESET_NAME_MAX_CHARS} characters")
    } else {
        return None;
    };
    Some(reason)
}

/// Why a preset name, as stored, breaks the rules. None when it's fine.
pub fn preset_name_problem(name: &str) -> Option<String> {
    preset_name_rule(name).map(|reason| format!("Invalid preset name: {reason}."))
}

/// What a person typed as a preset name, trimmed and checked.
pub fn normalize_preset_name(raw: &str) -> std::result::Result<String, String> {
    let name = raw.trim();
    match preset_name_problem(name) {
        Some(problem) => Err(problem),
        None => Ok(name.to_owned()),
    }
}

/// A name the reader can show. Anything else about it is `validate`'s
/// business, so a hand-made name is never hidden for being long.
pub(super) fn readable_name(name: &str) -> bool {
    !name.trim().is_empty()
}

/// Every preset the file holds, A to Z ignoring case.
pub(super) fn read_presets(document: &DocumentMut) -> Vec<(String, Preset)> {
    let Some(table) = document.get("presets").and_then(Item::as_table_like) else {
        return Vec::new();
    };
    let mut presets: Vec<(String, Preset)> = table
        .iter()
        .filter(|(name, _)| readable_name(name))
        .filter_map(|(name, item)| Some((name.to_owned(), read_preset(item.as_table_like()?))))
        .collect();
    presets.sort_by_cached_key(|(name, _)| (fold_case(name), name.clone()));
    presets
}

fn read_preset(table: &dyn TableLike) -> Preset {
    let text = |field: &str| table.get(field).and_then(Item::as_str);
    let flag = |field: &str| table.get(field).and_then(Item::as_bool).unwrap_or_default();
    Preset {
        sort: text("sort").and_then(SortKey::from_str).unwrap_or_default(),
        descending: flag("descending"),
        favorites_first: flag("favorites_first"),
        stores: read_list(table.get("stores"), Store::from_id),
        platforms: read_list(table.get("platforms"), Platform::from_id),
        tags: read_list(table.get("tags"), tag),
        not_stores: read_list(table.get("not_stores"), Store::from_id),
        not_platforms: read_list(table.get("not_platforms"), Platform::from_id),
        not_tags: read_list(table.get("not_tags"), tag),
        show_hidden: flag("show_hidden"),
        group: text("group").and_then(GroupBy::parse).unwrap_or_default(),
        layout: text("layout")
            .and_then(ViewLayout::parse)
            .unwrap_or_default(),
    }
}

fn tag(value: &str) -> Option<String> {
    Some(value.to_owned())
}

/// The ids in a list that this wineo knows, so one it doesn't, or a value
/// that isn't text, costs only itself. Not a list at all reads as empty.
pub(super) fn read_list<T: Ord>(
    item: Option<&Item>,
    parse: impl Fn(&str) -> Option<T>,
) -> BTreeSet<T> {
    item.and_then(Item::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| parse(value.as_str()?))
        .collect()
}

/// Saves the preset under `name`, creating it or changing only the lines that
/// differ from what the file reads as. A name that differs from an existing
/// preset's only in case is refused, so the list never holds two that look
/// alike. Returns whether the file changed.
pub fn set_preset(paths: &Paths, name: &str, preset: &Preset) -> Result<bool> {
    save_preset(paths, name, preset, case_variant)
}

/// Saves a new preset under `name`, refusing when one already has that name
/// in any case, so a preset another writer made since the caller last read
/// the file is never overwritten. Returns whether the file changed.
pub fn create_preset(paths: &Paths, name: &str, preset: &Preset) -> Result<bool> {
    save_preset(paths, name, preset, existing_name)
}

/// `set_preset` and `create_preset`, refusing whatever name `taken` finds
/// under the lock.
fn save_preset(
    paths: &Paths,
    name: &str,
    preset: &Preset,
    taken: fn(&DocumentMut, &str) -> Option<String>,
) -> Result<bool> {
    if let Some(problem) = preset_name_problem(name) {
        return Err(Error::InvalidPresetName(problem));
    }
    let mut refused = None;
    let changed = edit(paths, |document| {
        if let Some(existing) = taken(document, name) {
            refused = Some(Error::PresetNameTaken(existing));
            return false;
        }
        write_preset(document, name, preset)
    })?;
    match refused {
        Some(error) => Err(error),
        None => Ok(changed),
    }
}

/// Removes the preset. Returns whether there was one; none is not an error.
pub fn remove_preset(paths: &Paths, name: &str) -> Result<bool> {
    edit(paths, |document| {
        document
            .get_mut("presets")
            .and_then(Item::as_table_like_mut)
            .is_some_and(|presets| {
                presets.get(name).is_some_and(Item::is_table_like) && presets.remove(name).is_some()
            })
    })
}

/// Gives the preset a new name in the one write, so a failure leaves the old
/// one. The table moves whole, with its comments and keys this wineo doesn't
/// know. Any other entry's name is taken whatever its case, a value that
/// isn't a preset included, so nothing in the file is lost; the preset's own
/// name in another case is fine. Renaming to the same name writes nothing.
pub fn rename_preset(paths: &Paths, old: &str, new: &str) -> Result<bool> {
    if old != new
        && let Some(problem) = preset_name_problem(new)
    {
        return Err(Error::InvalidPresetName(problem));
    }
    let mut refused = None;
    let changed = edit(paths, |document| {
        let Some(presets) = document
            .get_mut("presets")
            .and_then(Item::as_table_like_mut)
            .filter(|presets| presets.get(old).is_some_and(Item::is_table_like))
        else {
            refused = Some(Error::UnknownPreset(old.to_owned()));
            return false;
        };
        if old == new {
            return false;
        }
        let folded = fold_case(new);
        let taken = presets
            .iter()
            .find(|(name, _)| *name != old && fold_case(name) == folded)
            .map(|(name, _)| name.to_owned());
        if let Some(taken) = taken {
            refused = Some(Error::PresetNameTaken(taken));
            return false;
        }
        rename_key(presets, old, new);
        true
    })?;
    match refused {
        Some(error) => Err(error),
        None => Ok(changed),
    }
}

/// Gives the entry `old` the key `new` in the same place, with the old key's
/// comments. toml_edit can't rename a key, so the entries from `old` on are
/// taken out and put back in order. That keeps the order of a dotted or
/// inline table, where a headered table's position already holds its place.
fn rename_key(table: &mut dyn TableLike, old: &str, new: &str) {
    let order: Vec<String> = table.iter().map(|(name, _)| name.to_owned()).collect();
    let from = order
        .iter()
        .position(|name| name == old)
        .expect("the preset was just found");
    let moved: Vec<(Key, Item)> = order[from..]
        .iter()
        .filter_map(|name| Some((table.key(name)?.clone(), table.remove(name)?)))
        .collect();
    for (index, (key, item)) in moved.into_iter().enumerate() {
        let key = if index == 0 {
            let mut renamed = Key::new(new);
            *renamed.leaf_decor_mut() = key.leaf_decor().clone();
            *renamed.dotted_decor_mut() = key.dotted_decor().clone();
            renamed
        } else {
            key
        };
        match table.entry_format(&key) {
            Entry::Vacant(slot) => {
                slot.insert(item);
            }
            Entry::Occupied(mut slot) => {
                slot.insert(item);
            }
        }
    }
}

/// The name of an entry under `[presets]` that `name` collides with: it
/// differs from it only in case. An entry that isn't a table counts, since
/// saving over it would drop what a person or a newer wineo wrote there. None
/// when `name` is the exact name of a preset (that's an update) or matches
/// no entry.
fn case_variant(document: &DocumentMut, name: &str) -> Option<String> {
    let presets = document.get("presets").and_then(Item::as_table_like)?;
    if presets.get(name).is_some_and(Item::is_table_like) {
        return None;
    }
    let folded = fold_case(name);
    presets
        .iter()
        .find(|(other, _)| fold_case(other) == folded)
        .map(|(other, _)| other.to_owned())
}

/// The entry named `name` exactly, else one that differs only in case.
fn existing_name(document: &DocumentMut, name: &str) -> Option<String> {
    let presets = document.get("presets").and_then(Item::as_table_like)?;
    if presets.get(name).is_some() {
        return Some(name.to_owned());
    }
    case_variant(document, name)
}

fn write_preset(document: &mut DocumentMut, name: &str, preset: &Preset) -> bool {
    let present = document
        .get("presets")
        .and_then(Item::as_table_like)
        .and_then(|presets| presets.get(name))
        .is_some_and(Item::is_table_like);
    let presets = table_mut(document.as_table_mut(), "presets");
    if !present {
        presets.insert(name, Item::Table(Table::new()));
    }
    let table = presets
        .get_mut(name)
        .and_then(Item::as_table_like_mut)
        .expect("the preset was just made a table");
    // A new preset names every setting, so a default that changes later can't
    // change what it means. An existing one is left as written wherever it
    // already reads the same.
    let fresh = !present;
    let mut changed = fresh;
    changed |= set_choice(
        table,
        "sort",
        preset.sort,
        sort_name(preset.sort),
        SortKey::from_str,
        fresh,
    );
    changed |= set_flag(table, "descending", preset.descending, fresh);
    changed |= set_flag(table, "favorites_first", preset.favorites_first, fresh);
    changed |= set_list(table, "stores", &preset.stores, Store::from_id, |store| {
        store.id().to_owned()
    });
    changed |= set_list(
        table,
        "platforms",
        &preset.platforms,
        Platform::from_id,
        |platform| platform.id().to_owned(),
    );
    changed |= set_list(table, "tags", &preset.tags, tag, String::clone);
    changed |= set_list(
        table,
        "not_stores",
        &preset.not_stores,
        Store::from_id,
        |store| store.id().to_owned(),
    );
    changed |= set_list(
        table,
        "not_platforms",
        &preset.not_platforms,
        Platform::from_id,
        |platform| platform.id().to_owned(),
    );
    changed |= set_list(table, "not_tags", &preset.not_tags, tag, String::clone);
    changed |= set_flag(table, "show_hidden", preset.show_hidden, fresh);
    changed |= set_choice(
        table,
        "group",
        preset.group,
        preset.group.as_str(),
        GroupBy::parse,
        fresh,
    );
    changed |= set_choice(
        table,
        "layout",
        preset.layout,
        preset.layout.as_str(),
        ViewLayout::parse,
        fresh,
    );
    changed
}

fn set_choice<T: Copy + PartialEq + Default>(
    table: &mut dyn TableLike,
    field: &str,
    value: T,
    name: &str,
    parse: fn(&str) -> Option<T>,
    fresh: bool,
) -> bool {
    let matches = !fresh
        && match table.get(field) {
            None => value == T::default(),
            Some(item) => item.as_str().and_then(parse) == Some(value),
        };
    if matches {
        return false;
    }
    store_value(table, field, Value::from(name));
    true
}

fn set_flag(table: &mut dyn TableLike, field: &str, value: bool, fresh: bool) -> bool {
    let matches = !fresh
        && match table.get(field) {
            None => !value,
            Some(item) => item.as_bool() == Some(value),
        };
    if matches {
        return false;
    }
    store_value(table, field, Value::from(value));
    true
}

/// A list is left alone while it reads as the wanted set, so ids a newer
/// wineo wrote beside the ones this one knows survive an update that doesn't
/// touch that list. Otherwise the array is edited in place: the entries that
/// left go, the ones that joined are appended, and the rest keep their place
/// and their comments. An empty set removes the key, so a list is only there
/// when it filters something, unless it still holds a comment.
fn set_list<T: Ord>(
    table: &mut dyn TableLike,
    field: &str,
    wanted: &BTreeSet<T>,
    parse: fn(&str) -> Option<T>,
    name: fn(&T) -> String,
) -> bool {
    let current = table.get(field);
    let have = read_list(current, parse);
    let matches = match current {
        None => wanted.is_empty(),
        Some(item) => item.is_array() && have == *wanted,
    };
    if matches {
        return false;
    }
    if let Some(array) = table.get_mut(field).and_then(Item::as_array_mut) {
        let leaving: Vec<String> = array
            .iter()
            .filter_map(Value::as_str)
            .filter(|text| parse(text).is_some_and(|known| !wanted.contains(&known)))
            .map(str::to_owned)
            .collect();
        for text in &leaving {
            remove_tag(array, text);
        }
        let mut joining: Vec<String> = wanted
            .iter()
            .filter(|known| !have.contains(known))
            .map(name)
            .collect();
        joining.sort();
        // With fewer than two entries there is no spacing to copy, so
        // `push_tag` would run the new ones together as `"a","b"`.
        let bare = array.len() < 2;
        for text in &joining {
            push_tag(array, text);
            let last = array.len() - 1;
            if bare
                && last > 0
                && let Some(added) = array.get_mut(last)
                && added
                    .decor()
                    .prefix()
                    .is_none_or(|prefix| prefix.as_str() == Some(""))
            {
                added.decor_mut().set_prefix(" ");
            }
        }
        if wanted.is_empty() && array.is_empty() && !array.to_string().contains('#') {
            table.remove(field);
        }
        return true;
    }
    if wanted.is_empty() {
        return table.remove(field).is_some();
    }
    let mut names: Vec<String> = wanted.iter().map(name).collect();
    names.sort();
    let array: toml_edit::Array = names.iter().map(String::as_str).collect();
    store_value(table, field, Value::Array(array));
    true
}

#[cfg(test)]
mod tests;
