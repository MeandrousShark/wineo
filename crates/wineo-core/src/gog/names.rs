//! The names GOG installs record for their products, for showing a DLC as
//! `Royalty [1233017772]` instead of by its bare id. Display only: nothing
//! here reads a name to decide anything, and nothing is written.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use super::migrate::id_text;

/// The names of a folder's products, by id, from the `goggame-<id>.info`
/// files GOG installs ship in the folder or in its `game/` directory. A file
/// counts only when its `gameId` is the id in its own name, and anything
/// unreadable or invalid is skipped, since a missing name only costs a label.
/// When both places name a product, the folder itself wins.
pub fn product_names(folder: &Path) -> HashMap<u64, String> {
    let mut names = HashMap::new();
    for dir in [folder.to_owned(), folder.join("game")] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
        // A stable pick if two files somehow claim one id.
        files.sort_by_key(|entry| entry.file_name());
        for entry in files {
            let file_name = entry.file_name();
            let Some(id) = file_name
                .to_str()
                .and_then(|name| name.strip_prefix("goggame-"))
                .and_then(|name| name.strip_suffix(".info"))
                .and_then(|id| id.parse::<u64>().ok())
            else {
                continue;
            };
            if names.contains_key(&id) {
                continue;
            }
            if let Some(name) = read_name(&entry.path(), id) {
                names.insert(id, name);
            }
        }
    }
    names
}

fn read_name(path: &Path, id: u64) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    if id_text(root.get("gameId")?) != id.to_string() {
        return None;
    }
    let name = root.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// A DLC as `NAME [ID]`. A name that starts with the base game's title and a
/// separator (`RimWorld - Royalty`) drops it, unless nothing would be left.
/// With no name the label is `DLC [ID]`.
pub fn dlc_label(base_title: &str, id: u64, name: Option<&str>) -> String {
    let name = name.map(str::trim).filter(|name| !name.is_empty());
    let Some(name) = name else {
        return format!("DLC [{id}]");
    };
    let short = if base_title.is_empty() {
        name
    } else {
        [" - ", ": "]
            .into_iter()
            .find_map(|separator| name.strip_prefix(base_title)?.strip_prefix(separator))
            .map(str::trim)
            .filter(|rest| !rest.is_empty())
            .unwrap_or(name)
    };
    format!("{short} [{id}]")
}

#[cfg(test)]
mod tests;
