//! The names GOG installs record for their products, for showing a DLC as
//! `Royalty (1233017772)` instead of by its bare id. Display only: nothing
//! here reads a name to decide anything, and nothing is written.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde_json::Value;

use super::migrate::id_text;

/// Bigger than any real `goggame-*.info`; a larger file isn't one.
const MAX_INFO_BYTES: u64 = 64 * 1024;

/// The names of a folder's products, by id, from the `goggame-<id>.info`
/// files GOG installs ship in the folder or in its `game/` directory. A file
/// counts only when it is a small regular file and its `gameId` is the id in
/// its own name, and anything else is skipped, since a missing name only
/// costs a label. When both places name a product, the folder itself wins.
pub fn product_names(folder: &Path) -> HashMap<u64, String> {
    let mut names = HashMap::new();
    for dir in [folder.to_owned(), folder.join("game")] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let file_name = entry.file_name();
            // The id has to be written the way a number prints, so one id
            // never has two files in a directory.
            let Some(id) = file_name
                .to_str()
                .and_then(|name| name.strip_prefix("goggame-"))
                .and_then(|name| name.strip_suffix(".info"))
                .and_then(|text| Some((text, text.parse::<u64>().ok()?)))
                .filter(|(text, id)| *text == id.to_string())
                .map(|(_, id)| id)
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

/// Reads one info file without ever waiting on it: a FIFO or a device named
/// like one would block the open or the read forever.
fn read_name(path: &Path, id: u64) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_INFO_BYTES {
        return None;
    }
    // The file can change after the check, so the open doesn't wait either
    // and the read is capped again.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC).bits() as i32)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    File::take(file, MAX_INFO_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_INFO_BYTES {
        return None;
    }
    let root: Value = serde_json::from_slice(&bytes).ok()?;
    if id_text(root.get("gameId")?) != id.to_string() {
        return None;
    }
    let name = root.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// A DLC as `NAME (ID)`. A name that starts with the base game's title and a
/// separator (`RimWorld - Royalty`) drops it, unless nothing would be left.
/// With no name the label is `DLC (ID)`.
pub fn dlc_label(base_title: &str, id: u64, name: Option<&str>) -> String {
    let name = name.map(str::trim).filter(|name| !name.is_empty());
    let Some(name) = name else {
        return format!("DLC ({id})");
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
    format!("{short} ({id})")
}

#[cfg(test)]
mod tests;
