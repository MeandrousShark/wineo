//! Which installed Steam games are new to wineo, and what a scan wants added
//! to the record. Reads nothing and writes nothing: the refresh gathers the
//! inputs, and the app applies the proposals after a scan.

use std::cell::LazyCell;
use std::collections::HashSet;
use std::path::Path;

use super::appinfo::AppInfo;
use super::{InstallState, LibraryState, Manifest, Scan, all_digits};
use crate::library::{RecordChanges, SteamRecord};

/// A game that could be offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingGame {
    pub app_id: String,
    pub title: String,
}

/// What one scan found. Worked out on every refresh and never stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    pub pending: Vec<PendingGame>,
    pub proposed: RecordChanges,
    /// The setting as this scan read it. It's part of the result so that
    /// turning it on reaches the app with nothing else changed, and only a
    /// hint: an import checks the file again under its lock.
    pub auto_import: bool,
}

/// Compares a scan with the record. `imported` holds the app ids that have an
/// environment.
///
/// Libraries are matched by the text of the path `libraries` gave, with no
/// normalizing. A path that stops matching only baselines again, which is
/// silent; one that isn't UTF-8 can't be stored, so it never counts as done.
///
/// A library that isn't complete adds nothing. A complete one seen for the
/// first time proposes what's on it, so its games never nudge. Only a complete
/// library with its baseline done has pending games.
///
/// `appinfo` runs only once a game gets as far as the type check, so a caller
/// can read the file lazily. It gives None when appinfo can't be read at all.
pub fn discover<'a>(
    scan: &Scan,
    imported: &HashSet<&str>,
    record: &SteamRecord,
    appinfo: impl FnOnce() -> Option<&'a AppInfo>,
) -> Discovery {
    let mut discovery = Discovery {
        auto_import: record.auto_import,
        ..Discovery::default()
    };
    if !scan.readable() {
        return discovery;
    }
    let mut missing: Vec<String> = imported
        .iter()
        .filter(|id| all_digits(id) && !record.contains(id))
        .map(|id| (*id).to_owned())
        .collect();
    missing.sort();
    discovery.proposed.imported = missing;

    let complete = |path: &Path| {
        scan.libraries
            .iter()
            .any(|library| library.path == path && library.state == LibraryState::Complete)
    };
    let baselined = |path: &Path| {
        path.to_str()
            .is_some_and(|it| record.baselined.contains(it))
    };
    discovery.proposed.libraries = scan
        .libraries
        .iter()
        .filter(|library| library.state == LibraryState::Complete && !baselined(&library.path))
        .filter_map(|library| library.path.to_str().map(str::to_owned))
        .collect();

    let appinfo = LazyCell::new(appinfo);
    let mut seen = HashSet::new();
    for entry in &scan.manifests {
        let Ok(manifest) = &entry.manifest else {
            continue;
        };
        if !complete(&entry.library)
            || imported.contains(entry.app_id.as_str())
            || record.contains(&entry.app_id)
        {
            continue;
        }
        if baselined(&entry.library) {
            if manifest.is_fully_installed()
                && offered(manifest, *appinfo)
                && seen.insert(&entry.app_id)
            {
                discovery.pending.push(PendingGame {
                    app_id: entry.app_id.clone(),
                    title: manifest.name.clone(),
                });
            }
        } else if entry.library.to_str().is_some()
            && manifest.state() != InstallState::NotInstalled
            && seen.insert(&entry.app_id)
        {
            discovery.proposed.baseline.push(entry.app_id.clone());
        }
    }
    discovery
}

/// Whether this kind of app gets a nudge. wineo isn't only for games, so
/// `application` counts. Without a type the manifest's name and id decide, so
/// a change to Steam's format costs a few stray rows rather than the feature.
fn offered(manifest: &Manifest, appinfo: Option<&AppInfo>) -> bool {
    let kind = manifest
        .app_id
        .parse()
        .ok()
        .and_then(|id| appinfo?.get(id)?.kind.as_deref())
        .filter(|kind| !kind.is_empty());
    match kind {
        Some(kind) => kind.eq_ignore_ascii_case("game") || kind.eq_ignore_ascii_case("application"),
        None => !manifest.is_tool(),
    }
}

#[cfg(test)]
mod tests;
