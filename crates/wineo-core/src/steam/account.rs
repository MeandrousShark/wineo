//! Steam's accounts and what they played: `config/loginusers.vdf`,
//! `userdata/<account>/config/localconfig.vdf`, and `config/config.vdf`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::vdf;
use super::{Install, number};

/// A SteamID64 is the account id plus this.
const ACCOUNT_BASE: u64 = 76_561_197_960_265_728;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playtime {
    /// `last_played` is None when Steam has no launch on record.
    Known {
        seconds: u64,
        last_played: Option<u64>,
    },
    /// The reason can't be read, for people to see.
    Unknown(String),
}

/// The account id whose playtime Steam shows: the one marked `MostRecent`, or
/// the only account listed.
pub fn active_account(install: &Install) -> Result<u64, String> {
    active_steam_id(install).map(|steam_id| steam_id - ACCOUNT_BASE)
}

/// The SteamID64 of the same account, which the Web API takes.
pub fn active_steam_id(install: &Install) -> Result<u64, String> {
    let path = install.root().join("config/loginusers.vdf");
    #[cfg(test)]
    crate::read_log::note(&path);
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("{} can't be read: {error}", path.display()))?;
    let document = vdf::parse(&text)
        .map_err(|error| format!("{} can't be parsed: {error}", path.display()))?;

    let accounts: Vec<(u64, bool)> = document
        .object("users")
        .into_iter()
        .flat_map(|users| users.entries())
        .filter_map(|(steam_id, value)| {
            let steam_id = steam_id.parse().ok()?;
            let recent = value.as_object()?.text("MostRecent") == Some("1");
            Some((steam_id, recent))
        })
        .collect();

    let steam_id = match (
        accounts.iter().find(|(_, recent)| *recent),
        accounts.as_slice(),
    ) {
        (Some((steam_id, _)), _) => *steam_id,
        (None, [(steam_id, _)]) => *steam_id,
        (None, []) => return Err("Steam has no logged-in account".to_owned()),
        (None, _) => return Err("Steam doesn't say which account was used last".to_owned()),
    };
    if steam_id < ACCOUNT_BASE {
        return Err(format!("{steam_id} isn't a SteamID64"));
    }
    Ok(steam_id)
}

/// Steam's own playtime for an app, from the active account's
/// `localconfig.vdf`. A readable file with no entry for the app is zero hours.
pub fn playtime(install: &Install, app_id: &str) -> Playtime {
    Playtimes::load(install).get(app_id)
}

/// The active account's `localconfig.vdf`, read and parsed once, for
/// looking up several apps. `playtime` for each would read `loginusers.vdf`
/// and `localconfig.vdf` again every time.
pub struct Playtimes {
    /// The file and its path, or why it can't be had.
    file: Result<(std::path::PathBuf, vdf::Object), String>,
}

impl Playtimes {
    pub fn load(install: &Install) -> Self {
        Self {
            file: read_localconfig(install),
        }
    }

    pub fn get(&self, app_id: &str) -> Playtime {
        match &self.file {
            Ok((path, document)) => {
                lookup(path, document, app_id).unwrap_or_else(Playtime::Unknown)
            }
            Err(reason) => Playtime::Unknown(reason.clone()),
        }
    }
}

fn read_localconfig(install: &Install) -> Result<(std::path::PathBuf, vdf::Object), String> {
    let account = active_account(install)?;
    let path = install
        .root()
        .join("userdata")
        .join(account.to_string())
        .join("config/localconfig.vdf");
    #[cfg(test)]
    crate::read_log::note(&path);
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("{} can't be read: {error}", path.display()))?;
    let document = vdf::parse(&text)
        .map_err(|error| format!("{} can't be parsed: {error}", path.display()))?;
    Ok((path, document))
}

fn lookup(
    path: &std::path::Path,
    document: &vdf::Object,
    app_id: &str,
) -> Result<Playtime, String> {
    let Some(app) = document.path(&[
        "UserLocalConfigStore",
        "Software",
        "Valve",
        "Steam",
        "apps",
        app_id,
    ]) else {
        return Ok(Playtime::Known {
            seconds: 0,
            last_played: None,
        });
    };
    let app = app
        .as_object()
        .ok_or_else(|| format!("{}: app {app_id} isn't a block", path.display()))?;
    let minutes =
        number(app, "Playtime").map_err(|error| format!("{}: {error}", path.display()))?;
    let last_played =
        number(app, "LastPlayed").map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Playtime::Known {
        seconds: minutes.saturating_mul(60),
        last_played: (last_played > 0).then_some(last_played),
    })
}

/// The configured compatibility tools by app ID. A malformed first entry is
/// kept with an empty name so a later duplicate can't override it, matching
/// Steam's first-key lookup. Empty names make lookups fall back to "0".
/// Keys are lowercased, so use `compat_tool_in` instead of indexing directly.
pub fn compat_tools(install: &Install) -> HashMap<String, String> {
    let mut tools = HashMap::new();
    let Ok(text) = fs::read_to_string(install.root().join("config/config.vdf")) else {
        return tools;
    };
    let Ok(document) = vdf::parse(&text) else {
        return tools;
    };
    let Some(mapping) = document
        .path(&[
            "InstallConfigStore",
            "Software",
            "Valve",
            "Steam",
            "CompatToolMapping",
        ])
        .and_then(|value| value.as_object())
    else {
        return tools;
    };
    for (app_id, value) in mapping.entries() {
        let name = value
            .as_object()
            .and_then(|entry| entry.text("name"))
            .unwrap_or_default()
            .to_owned();
        tools.entry(app_id.to_ascii_lowercase()).or_insert(name);
    }
    tools
}

/// Windows when Steam has run a game under Proton: it has a Proton entry of
/// its own in `tools` (not the `"0"` default, which only covers games without
/// a Linux build), or a prefix exists under its library's `compatdata`. None
/// when neither shows it, since Steam doesn't say whether a game uses Proton.
/// `library` is the folder holding the game's `steamapps`; the prefix check
/// is one stat.
pub fn proton_platform(
    tools: &HashMap<String, String>,
    app_id: &str,
    library: Option<&Path>,
) -> Option<crate::source::Platform> {
    let own_entry = tools
        .get(&app_id.to_ascii_lowercase())
        .is_some_and(|name| !name.is_empty());
    // The id becomes a path component, so only plain digits get one.
    let prefix = library.is_some_and(|library| {
        !app_id.is_empty()
            && app_id.bytes().all(|byte| byte.is_ascii_digit())
            && library
                .join("steamapps/compatdata")
                .join(app_id)
                .join("pfx")
                .exists()
    });
    (own_entry || prefix).then_some(crate::source::Platform::Windows)
}

/// The Proton build a game is set to, or the `"0"` default when the game has
/// no entry of its own. None when neither is set, or `config.vdf` can't be
/// read.
pub fn compat_tool(install: &Install, app_id: &str) -> Option<String> {
    compat_tool_in(&compat_tools(install), app_id)
}

/// Look up an app's tool in `compat_tools`, falling back to "0" for missing
/// or empty names. App IDs are case-insensitive.
pub fn compat_tool_in(tools: &HashMap<String, String>, app_id: &str) -> Option<String> {
    [app_id.to_ascii_lowercase(), "0".to_owned()]
        .into_iter()
        .find_map(|key| tools.get(&key).filter(|name| !name.is_empty()).cloned())
}
