//! The GOG games Heroic installed, from `gog_store/installed.json`, with
//! titles from its library cache. Read-only, like the rest of `heroic`, and
//! nothing here reads Heroic's GOG login.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::{
    HeroicImport, HeroicIssue, HeroicNativeImport, HeroicSource, Store, invalid_json,
    read_optional_json, string_field, valid_game_id,
};
use crate::error::Result;
use crate::gog::folder::Platform;

/// A GOG game as Heroic recorded installing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroicGogGame {
    /// Heroic's `appName`, the GOG product id.
    pub id: String,
    /// From Heroic's library cache, or the id when it has none.
    pub title: String,
    pub platform: Platform,
    /// Where Heroic installed it, as Heroic wrote it.
    pub folder: PathBuf,
    /// The installer version Heroic recorded. May be empty.
    pub version: String,
    /// The language Heroic downloaded the game in.
    pub language: String,
    /// The Galaxy build, for a game Heroic installed from Galaxy.
    pub build_id: Option<String>,
    /// Installed DLC, in Heroic's order.
    pub dlc: Vec<u64>,
}

/// Every GOG game Heroic installed, and the entries that couldn't be read.
#[derive(Debug, Clone, Default)]
pub struct HeroicGogListing {
    pub games: Vec<HeroicGogGame>,
    pub issues: Vec<HeroicIssue>,
}

impl HeroicSource {
    pub(super) fn gog_installed_path(&self) -> PathBuf {
        self.config_dir.join("gog_store/installed.json")
    }

    fn gog_library_path(&self) -> PathBuf {
        self.config_dir.join("store_cache/gog_library.json")
    }

    /// The GOG games Heroic installed. DLC entries are left out, since they
    /// belong to their game. No `installed.json` means none.
    pub fn gog_installed(&self) -> Result<HeroicGogListing> {
        let mut listing = HeroicGogListing::default();
        let Some(entries) = self.gog_entries()? else {
            return Ok(listing);
        };
        let titles = self.gog_titles();
        for (index, value) in entries.iter().enumerate() {
            match gog_entry(value, &titles) {
                Ok(Some(game)) => listing.games.push(game),
                Ok(None) => {}
                Err(message) => listing.issues.push(HeroicIssue::invalid(
                    Some(
                        value
                            .get("appName")
                            .and_then(Value::as_str)
                            .map_or_else(|| format!("installed[{index}]"), str::to_owned),
                    ),
                    message,
                )),
            }
        }
        Ok(listing)
    }

    /// The installed GOG game `id`, None when Heroic hasn't installed it, or
    /// an error when its entry can't be used.
    pub fn gog_game(&self, id: &str) -> Result<Option<HeroicGogGame>> {
        let Some(entries) = self.gog_entries()? else {
            return Ok(None);
        };
        let titles = self.gog_titles();
        for value in entries {
            if value.get("appName").and_then(Value::as_str) != Some(id)
                || value.get("is_dlc").and_then(Value::as_bool) == Some(true)
            {
                continue;
            }
            return gog_entry(&value, &titles)
                .map_err(|message| invalid_json(&self.gog_installed_path(), message));
        }
        Ok(None)
    }

    fn gog_entries(&self) -> Result<Option<Vec<Value>>> {
        let path = self.gog_installed_path();
        let Some(root) = read_optional_json(&path)? else {
            return Ok(None);
        };
        root.get("installed")
            .and_then(Value::as_array)
            .cloned()
            .map(Some)
            .ok_or_else(|| invalid_json(&path, "expected an installed array"))
    }

    /// Titles by id. A missing or unreadable cache just means no titles.
    fn gog_titles(&self) -> Map<String, Value> {
        let Ok(Some(root)) = read_optional_json(&self.gog_library_path()) else {
            return Map::new();
        };
        let mut titles = Map::new();
        for game in root
            .get("games")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
        {
            if let (Some(id), Some(title)) = (
                string_field(game, &["app_name"]),
                string_field(game, &["title"]),
            ) {
                titles.insert(id.to_owned(), Value::from(title));
            }
        }
        titles
    }

    /// What a migration of `game` read: Heroic's files and the game folder's
    /// `info` file.
    pub(crate) fn gog_fingerprint(
        &self,
        game: &HeroicGogGame,
        info: &Path,
    ) -> Result<super::InputFingerprint> {
        self.store_fingerprint(&game.id, Store::Gog { info })
    }

    /// The prefixes Heroic's `winePrefix` for `game` may mean, read without
    /// resolving its runner, so an environment already using it can be found
    /// even when that runner is unusable. A Proton prefix's `pfx` folder
    /// stands for its parent, which inspection only decides once the runner
    /// is known, so both are given then. Empty when Heroic names none.
    pub(crate) fn gog_prefixes(&self, game: &HeroicGogGame) -> Result<Vec<PathBuf>> {
        let global = self.global_settings()?;
        let (effective, _) = self.effective_settings(&game.id, &global)?;
        let Some(prefix) = effective
            .get("winePrefix")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return Ok(Vec::new());
        };
        let prefix = self.expand_path(prefix).0;
        let mut prefixes = vec![prefix.clone()];
        let proton = super::normalize_heroic_prefix(prefix, super::HeroicRunnerKind::Proton);
        if !prefixes.contains(&proton) {
            prefixes.push(proton);
        }
        Ok(prefixes)
    }

    /// Heroic's settings for the Windows GOG game `game`, launching `exe`.
    /// `info` is the game folder's info file, which a later apply rechecks
    /// along with Heroic's files.
    pub(crate) fn inspect_gog(
        &self,
        game: &HeroicGogGame,
        exe: &Path,
        info: &Path,
    ) -> Result<HeroicImport> {
        let global = self.global_settings()?;
        self.inspect_entry(
            &game.id,
            &gog_store_entry(game, exe),
            &global,
            Store::Gog { info },
        )
    }

    /// Heroic's settings for the native GOG game `game`, launching `exe`.
    pub(crate) fn inspect_gog_native(
        &self,
        game: &HeroicGogGame,
        exe: &Path,
        info: &Path,
    ) -> Result<HeroicNativeImport> {
        let global = self.global_settings()?;
        self.inspect_native_entry(
            &game.id,
            &gog_store_entry(game, exe),
            &global,
            Store::Gog { info },
        )
    }
}

/// The shape of a sideload entry, so GOG games are inspected by the same
/// code.
fn gog_store_entry(game: &HeroicGogGame, exe: &Path) -> Map<String, Value> {
    let mut entry = Map::new();
    entry.insert("app_name".to_owned(), Value::from(game.id.as_str()));
    entry.insert("title".to_owned(), Value::from(game.title.as_str()));
    entry.insert("runner".to_owned(), Value::from("gog"));
    entry.insert(
        "platform".to_owned(),
        Value::from(match game.platform {
            Platform::Linux => "linux",
            Platform::Windows => "windows",
        }),
    );
    entry.insert(
        "executable".to_owned(),
        Value::from(exe.to_string_lossy().into_owned()),
    );
    entry
}

/// One `installed.json` entry. None for DLC, an error for an entry that
/// can't be migrated.
fn gog_entry(
    value: &Value,
    titles: &Map<String, Value>,
) -> std::result::Result<Option<HeroicGogGame>, String> {
    let entry = value
        .as_object()
        .ok_or_else(|| "installed entry is not an object".to_owned())?;
    if entry.get("is_dlc").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    let id = string_field(entry, &["appName"])
        .ok_or_else(|| "installed entry has no appName".to_owned())?;
    if !valid_game_id(id) || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{id:?} isn't a GOG game id"));
    }
    let platform = match string_field(entry, &["platform"]).map(str::to_ascii_lowercase) {
        Some(platform) if platform == "linux" => Platform::Linux,
        Some(platform) if platform == "windows" => Platform::Windows,
        Some(platform) => return Err(format!("{platform} games can't be migrated")),
        None => return Err("installed entry has no platform".to_owned()),
    };
    let folder = string_field(entry, &["install_path"])
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "installed entry has no install_path".to_owned())?;
    let mut dlc = Vec::new();
    for value in entry
        .get("installedDLCs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let parsed = match value {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.parse().ok(),
            _ => None,
        };
        let parsed = parsed.ok_or_else(|| format!("installedDLCs has {value}, not a GOG id"))?;
        if !dlc.contains(&parsed) {
            dlc.push(parsed);
        }
    }
    Ok(Some(HeroicGogGame {
        id: id.to_owned(),
        title: titles
            .get(id)
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
            .unwrap_or(id)
            .to_owned(),
        platform,
        folder: PathBuf::from(folder),
        version: string_field(entry, &["version"])
            .unwrap_or_default()
            .to_owned(),
        language: string_field(entry, &["language"])
            .unwrap_or_default()
            .to_owned(),
        build_id: string_field(entry, &["buildId"])
            .filter(|build| !build.is_empty())
            .map(str::to_owned),
        dlc,
    }))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn source(root: &Path, installed: &str, library: Option<&str>) -> HeroicSource {
        let config = root.join("heroic");
        fs::create_dir_all(config.join("gog_store")).unwrap();
        fs::write(config.join("gog_store/installed.json"), installed).unwrap();
        if let Some(library) = library {
            fs::create_dir_all(config.join("store_cache")).unwrap();
            fs::write(config.join("store_cache/gog_library.json"), library).unwrap();
        }
        HeroicSource::from_config_dir_with_home(config, root.to_owned())
    }

    const INSTALLED: &str = r#"{"installed": [
      {"platform": "linux", "executable": "", "install_path": "/home/u/Games/RimWorld",
       "install_size": "5.03 GiB", "is_dlc": false, "version": "1.6.4871 rev588",
       "appName": "1094900565", "installedDLCs": [1233017772, 1827857764, 1865538500],
       "language": "en", "versionEtag": "", "buildId": "", "pinnedVersion": false},
      {"platform": "windows", "executable": "", "install_path": "/home/u/Games/Disco Elysium - The Final Cut",
       "install_size": "9.57 GiB", "is_dlc": false, "version": "WINkay16062026_memfix",
       "appName": "1771589310", "installedDLCs": [], "language": "en-US",
       "versionEtag": "\"0576\"", "buildId": "59781086834418085", "pinnedVersion": false},
      {"platform": "linux", "install_path": "/x", "is_dlc": true, "appName": "1233017772"},
      {"platform": "osx", "install_path": "/y", "appName": "5"}
    ]}"#;

    #[test]
    fn lists_games_with_titles_and_leaves_out_dlc() {
        let root = tempfile::tempdir().unwrap();
        let source = source(
            root.path(),
            INSTALLED,
            Some(
                r#"{"games":[{"runner":"gog","app_name":"1094900565","title":"RimWorld"}],"__timestamp":1}"#,
            ),
        );
        let listing = source.gog_installed().unwrap();
        assert_eq!(listing.games.len(), 2);
        let rimworld = &listing.games[0];
        assert_eq!(rimworld.title, "RimWorld");
        assert_eq!(rimworld.platform, Platform::Linux);
        assert_eq!(rimworld.folder, Path::new("/home/u/Games/RimWorld"));
        assert_eq!(rimworld.version, "1.6.4871 rev588");
        assert_eq!(rimworld.language, "en");
        assert_eq!(rimworld.build_id, None);
        assert_eq!(rimworld.dlc, [1233017772, 1827857764, 1865538500]);
        let disco = &listing.games[1];
        // No title in the cache, so the id stands in.
        assert_eq!(disco.title, "1771589310");
        assert_eq!(disco.platform, Platform::Windows);
        assert_eq!(disco.build_id.as_deref(), Some("59781086834418085"));
        assert_eq!(listing.issues.len(), 1);
        assert_eq!(listing.issues[0].field.as_deref(), Some("5"));
        assert_eq!(listing.issues[0].message, "osx games can't be migrated");
    }

    #[test]
    fn finds_one_game_and_reports_what_is_not_installed() {
        let root = tempfile::tempdir().unwrap();
        let source = source(root.path(), INSTALLED, None);
        assert_eq!(
            source.gog_game("1771589310").unwrap().unwrap().language,
            "en-US"
        );
        assert!(source.gog_game("1").unwrap().is_none());
        // A DLC isn't a game of its own.
        assert!(source.gog_game("1233017772").unwrap().is_none());
        assert!(source.gog_game("5").is_err());
    }

    #[test]
    fn no_installed_file_means_no_games() {
        let root = tempfile::tempdir().unwrap();
        let source =
            HeroicSource::from_config_dir_with_home(root.path().join("h"), root.path().to_owned());
        assert!(source.gog_installed().unwrap().games.is_empty());
        assert!(source.gog_game("1").unwrap().is_none());
    }
}
