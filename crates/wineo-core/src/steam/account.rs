//! Steam's accounts and what they played: `config/loginusers.vdf`,
//! `userdata/<account>/config/localconfig.vdf`, and `config/config.vdf`.

use std::collections::HashMap;
use std::fs;

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
    let path = install.root().join("config/loginusers.vdf");
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
    steam_id
        .checked_sub(ACCOUNT_BASE)
        .ok_or_else(|| format!("{steam_id} isn't a SteamID64"))
}

/// Steam's own playtime for an app, from the active account's
/// `localconfig.vdf`. A readable file with no entry for the app is zero hours.
pub fn playtime(install: &Install, app_id: &str) -> Playtime {
    match read_playtime(install, app_id) {
        Ok(playtime) => playtime,
        Err(reason) => Playtime::Unknown(reason),
    }
}

fn read_playtime(install: &Install, app_id: &str) -> Result<Playtime, String> {
    let account = active_account(install)?;
    let path = install
        .root()
        .join("userdata")
        .join(account.to_string())
        .join("config/localconfig.vdf");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("{} can't be read: {error}", path.display()))?;
    let document = vdf::parse(&text)
        .map_err(|error| format!("{} can't be parsed: {error}", path.display()))?;

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

/// The configured compatibility tools by app ID. Keys are lowercase because
/// VDF keys are case-insensitive. Empty names retain malformed entries so
/// they don't shadow the default differently from Steam's first-key lookup.
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

/// The Proton build a game is set to, or the `"0"` default when the game has
/// no entry of its own. None when neither is set, or `config.vdf` can't be
/// read.
pub fn compat_tool(install: &Install, app_id: &str) -> Option<String> {
    let tools = compat_tools(install);
    [app_id.to_ascii_lowercase(), "0".to_owned()]
        .into_iter()
        .find_map(|key| tools.get(&key).filter(|name| !name.is_empty()).cloned())
}
