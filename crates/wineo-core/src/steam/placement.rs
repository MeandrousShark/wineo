//! What a Steam game runs on and which Proton build it uses, decided once so
//! the library's platform filter and the panel's Proton row can't disagree.

use std::collections::HashMap;

use super::ManifestEntry;
use super::appinfo::AppInfo;
use crate::source::Platform;

/// A tool name with this prefix is a Steam Linux Runtime, which isn't Proton.
const RUNTIME_PREFIX: &str = "steamlinuxruntime";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    /// None when Steam doesn't show what the game runs on.
    pub platform: Option<Platform>,
    /// Only set for a Windows game.
    pub proton: Option<String>,
}

/// Places the game `app_id`, whose manifest (if any) is `entry`.
///
/// Rules, first match wins:
/// 1. Not fully installed, or no readable manifest: nothing, whatever the
///    mapping or prefix says.
/// 2. An own `CompatToolMapping` entry that isn't a Steam Linux Runtime:
///    Windows.
/// 3. A non-empty `oslist`: Native with a `linux` item, otherwise Windows.
/// 4. Otherwise a `compatdata` prefix means Windows, and no prefix means
///    nothing.
///
/// `appinfo` runs only when rule 3 needs it, so a caller can read the file
/// lazily. It gives None when appinfo can't be read at all.
pub fn place<'a>(
    entry: Option<&ManifestEntry>,
    tools: &HashMap<String, String>,
    app_id: &str,
    appinfo: impl FnOnce() -> Option<&'a AppInfo>,
) -> Placement {
    let Some(entry) = entry.filter(|entry| {
        entry
            .manifest
            .as_ref()
            .is_ok_and(|manifest| manifest.is_fully_installed())
    }) else {
        return Placement::default();
    };
    // An app id of "0" would otherwise find the default as its own entry.
    let own = (app_id != "0")
        .then(|| tools.get(&app_id.to_ascii_lowercase()))
        .flatten()
        .filter(|name| is_proton(name));
    let platform = if own.is_some() {
        Some(Platform::Windows)
    } else {
        match oslist(appinfo(), app_id) {
            Some(list)
                if list
                    .split(',')
                    .any(|os| os.trim().eq_ignore_ascii_case("linux")) =>
            {
                Some(Platform::Native)
            }
            Some(_) => Some(Platform::Windows),
            None => has_prefix(&entry.library, app_id).then_some(Platform::Windows),
        }
    };
    let proton = (platform == Some(Platform::Windows))
        .then(|| own.or_else(|| tools.get("0").filter(|name| is_proton(name))))
        .flatten()
        .cloned();
    Placement { platform, proton }
}

/// A tool name counts unless it is empty or a Steam Linux Runtime, which
/// Steam may map without it being a Proton build. Anything else is taken as
/// Proton, so custom builds keep working.
fn is_proton(name: &str) -> bool {
    !name.is_empty()
        && !name
            .as_bytes()
            .get(..RUNTIME_PREFIX.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(RUNTIME_PREFIX.as_bytes()))
}

/// The app's `oslist` when appinfo has a non-empty one, lowercased.
fn oslist<'a>(appinfo: Option<&'a AppInfo>, app_id: &str) -> Option<&'a str> {
    if !all_digits(app_id) {
        return None;
    }
    let list = appinfo?
        .get(app_id.parse().ok()?)?
        .oslist
        .as_deref()?
        .trim();
    (!list.is_empty()).then_some(list)
}

fn all_digits(app_id: &str) -> bool {
    !app_id.is_empty() && app_id.bytes().all(|byte| byte.is_ascii_digit())
}

/// One stat. The id becomes a path component, so only plain digits get one.
fn has_prefix(library: &std::path::Path, app_id: &str) -> bool {
    all_digits(app_id)
        && library
            .join("steamapps/compatdata")
            .join(app_id)
            .join("pfx")
            .exists()
}

#[cfg(test)]
mod tests;
