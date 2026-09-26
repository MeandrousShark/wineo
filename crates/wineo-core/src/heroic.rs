//! Read-only inspection of Heroic sideloaded game data.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

const FLATPAK_ID: &str = "com.heroicgameslauncher.hgl";
/// The unsupported settings that still matter to a native game. The rest are
/// about Wine or Proton, which a native game doesn't run on.
const NATIVE_UNSUPPORTED: [&str; 5] = [
    "wrapperOptions",
    "beforeLaunchScriptPath",
    "afterLaunchScriptPath",
    "gamescope",
    "showMangohud",
];
const MANAGED_ENV: [&str; 5] = [
    "WINEPREFIX",
    "PROTONPATH",
    "STEAM_COMPAT_DATA_PATH",
    "STEAM_COMPAT_CLIENT_INSTALL_PATH",
    "CX_BOTTLE",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeroicOrigin {
    Native,
    Flatpak,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroicSource {
    config_dir: PathBuf,
    origin: HeroicOrigin,
    home: Option<PathBuf>,
}

impl HeroicSource {
    pub fn discover(
        home: &Path,
        xdg_config_home: Option<&Path>,
        xdg_data_home: Option<&Path>,
    ) -> Result<Vec<Self>> {
        if !home.is_absolute() {
            return Err(Error::NoHome);
        }
        let config_home = xdg_config_home
            .filter(|path| path.is_absolute())
            .map(Path::to_owned)
            .unwrap_or_else(|| home.join(".config"));
        let data_home = xdg_data_home
            .filter(|path| path.is_absolute())
            .map(Path::to_owned)
            .unwrap_or_else(|| home.join(".local/share"));
        let roots = [
            (config_home.join("heroic"), HeroicOrigin::Native),
            (
                home.join(".var/app").join(FLATPAK_ID).join("config/heroic"),
                HeroicOrigin::Flatpak,
            ),
            (
                home.join(".var/app")
                    .join(FLATPAK_ID)
                    .join(".config/heroic"),
                HeroicOrigin::Flatpak,
            ),
            (
                home.join(".var/app").join(FLATPAK_ID).join("data/heroic"),
                HeroicOrigin::Flatpak,
            ),
            (data_home.join("heroic"), HeroicOrigin::Native),
        ];
        let mut found = Vec::new();
        let mut seen = BTreeSet::new();
        for (config_dir, origin) in roots {
            if !is_heroic_root(&config_dir) {
                continue;
            }
            let key = crate::paths::resolve(&config_dir);
            if seen.insert(key) {
                found.push(Self {
                    config_dir,
                    origin,
                    home: Some(home.to_owned()),
                });
            }
        }
        found.sort_by(|a, b| a.config_dir.cmp(&b.config_dir));
        Ok(found)
    }

    pub fn discover_from_env() -> Result<Vec<Self>> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(Error::NoHome)?;
        let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
        Self::discover(&home, config.as_deref(), data.as_deref())
    }

    pub fn from_config_dir(config_dir: PathBuf) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        Self {
            config_dir,
            origin: HeroicOrigin::Explicit,
            home,
        }
    }

    pub fn from_config_dir_with_home(config_dir: PathBuf, home: PathBuf) -> Self {
        Self {
            config_dir,
            origin: HeroicOrigin::Explicit,
            home: Some(home),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// `game_id`'s playtime as Heroic tracked it, from `store/timestamp.json`.
    /// None if Heroic never played it, or played it for no time at all.
    pub fn playtime(&self, game_id: &str) -> Result<Option<HeroicPlaytime>> {
        let path = self.config_dir.join("store/timestamp.json");
        let Some(value) = read_optional_json(&path)? else {
            return Ok(None);
        };
        let object = value
            .as_object()
            .ok_or_else(|| invalid_json(&path, "expected an object"))?;
        let Some(entry) = object.get(game_id).and_then(Value::as_object) else {
            return Ok(None);
        };
        let minutes = entry
            .get("totalPlayed")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        if minutes <= 0.0 {
            return Ok(None);
        }
        let last_played = string_field(entry, &["lastPlayed"])
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse().ok());
        Ok(Some(HeroicPlaytime {
            seconds: (minutes * 60.0).round() as i64,
            last_played,
        }))
    }

    pub fn origin(&self) -> HeroicOrigin {
        self.origin
    }

    pub fn list_candidates(&self) -> Result<HeroicListing> {
        let library_path = self.library_path();
        let library = read_json(&library_path)?;
        let entries = library_entries(&library)
            .ok_or_else(|| invalid_json(&library_path, "expected a games array"))?;
        let global = self.global_settings()?;
        let mut listing = HeroicListing {
            source: self.clone(),
            candidates: Vec::new(),
            issues: Vec::new(),
        };

        for (index, value) in entries.iter().enumerate() {
            let path = format!("games[{index}]");
            let Some(entry) = value.as_object() else {
                listing.issues.push(HeroicIssue::invalid(
                    Some(path),
                    "library entry is not an object",
                ));
                continue;
            };
            let Some(raw_id) = string_field(entry, &["app_name", "id"]) else {
                listing.issues.push(HeroicIssue::invalid(
                    Some(path),
                    "library entry has no string app_name",
                ));
                continue;
            };
            let mut candidate = HeroicCandidate {
                id: raw_id.to_owned(),
                title: string_field(entry, &["title", "name"])
                    .unwrap_or(raw_id)
                    .to_owned(),
                kind: game_kind(entry),
                prefix: None,
                runner: None,
                executable: executable(entry),
                issues: Vec::new(),
            };
            if !valid_game_id(raw_id) {
                candidate.issues.push(HeroicIssue::invalid(
                    Some("id".to_owned()),
                    "game id contains a path separator, traversal, control character, or is empty",
                ));
            }
            match candidate.kind {
                HeroicGameKind::Windows => {}
                HeroicGameKind::Native if platform(entry) != Some("linux") => {
                    candidate.issues.push(HeroicIssue::unsupported(
                        Some("platform".to_owned()),
                        "only Linux native games can be imported",
                    ))
                }
                HeroicGameKind::Native => {}
                HeroicGameKind::Browser => candidate.issues.push(HeroicIssue::unsupported(
                    Some("browserUrl".to_owned()),
                    "browser games do not have a Wine prefix",
                )),
                HeroicGameKind::Unknown => candidate.issues.push(HeroicIssue::unsupported(
                    Some("platform".to_owned()),
                    "Heroic did not identify this as a Windows game",
                )),
            }
            if string_field(entry, &["runner"]) != Some("sideload") {
                candidate.issues.push(HeroicIssue::unsupported(
                    Some("runner".to_owned()),
                    "only manually added sideloaded games are supported",
                ));
            }
            if candidate.issues.is_empty() && candidate.kind == HeroicGameKind::Native {
                match self.inspect_native_entry(raw_id, entry, &global) {
                    Ok(import) => {
                        candidate.executable = Some(import.exe.clone());
                        candidate.issues = import.issues.clone();
                    }
                    Err(error) => candidate.issues.push(HeroicIssue::invalid(
                        Some("settings".to_owned()),
                        error.to_string(),
                    )),
                }
            } else if candidate.issues.is_empty() {
                match self.inspect_entry(raw_id, entry, &global) {
                    Ok(import) => {
                        candidate.prefix = Some(import.prefix.clone());
                        candidate.runner = Some(import.runner.clone());
                        candidate.issues = import.issues.clone();
                    }
                    Err(error) => candidate.issues.push(HeroicIssue::invalid(
                        Some("settings".to_owned()),
                        error.to_string(),
                    )),
                }
            }
            listing.candidates.push(candidate);
        }
        Ok(listing)
    }

    pub fn inspect(&self, game_id: &str) -> Result<HeroicImport> {
        if !valid_game_id(game_id) {
            return Err(invalid_json(
                &self.game_config_path(game_id),
                "invalid game id",
            ));
        }
        let library_path = self.library_path();
        let library = read_json(&library_path)?;
        let entries = library_entries(&library)
            .ok_or_else(|| invalid_json(&library_path, "expected a games array"))?;
        let entry = entries
            .iter()
            .filter_map(Value::as_object)
            .find(|entry| string_field(entry, &["app_name", "id"]) == Some(game_id))
            .ok_or_else(|| missing_file(&library_path, "game id is not in sideload library"))?;
        let global = self.global_settings()?;
        self.inspect_entry(game_id, entry, &global)
    }

    /// Inspects a native sideloaded game, which imports as a native
    /// environment. Heroic's Wine settings are ignored.
    pub fn inspect_native(&self, game_id: &str) -> Result<HeroicNativeImport> {
        if !valid_game_id(game_id) {
            return Err(invalid_json(
                &self.game_config_path(game_id),
                "invalid game id",
            ));
        }
        let library_path = self.library_path();
        let library = read_json(&library_path)?;
        let entries = library_entries(&library)
            .ok_or_else(|| invalid_json(&library_path, "expected a games array"))?;
        let entry = entries
            .iter()
            .filter_map(Value::as_object)
            .find(|entry| string_field(entry, &["app_name", "id"]) == Some(game_id))
            .ok_or_else(|| missing_file(&library_path, "game id is not in sideload library"))?;
        let global = self.global_settings()?;
        self.inspect_native_entry(game_id, entry, &global)
    }

    fn inspect_native_entry(
        &self,
        game_id: &str,
        entry: &Map<String, Value>,
        global: &Map<String, Value>,
    ) -> Result<HeroicNativeImport> {
        let game_path = self.game_config_path(game_id);
        let game_root = read_optional_json(&game_path)?.unwrap_or(Value::Object(Map::new()));
        if !supported_schema_root(&game_root) {
            return Err(invalid_json(
                &game_path,
                format!(
                    "unsupported Heroic game config version {}",
                    schema_version(&game_root).unwrap_or_default()
                ),
            ));
        }
        let overrides = game_settings(&game_root, game_id).unwrap_or_default();
        let mut effective = global.clone();
        for (key, value) in &overrides {
            effective.insert(key.clone(), value.clone());
        }
        let mut review = SettingsReview::default();
        let mut issues = Vec::new();
        if game_kind(entry) != HeroicGameKind::Native || platform(entry) != Some("linux") {
            issues.push(HeroicIssue::unsupported(
                Some("platform".to_owned()),
                "only Linux native games import as native environments",
            ));
        }
        if string_field(entry, &["runner"]) != Some("sideload") {
            issues.push(HeroicIssue::unsupported(
                Some("runner".to_owned()),
                "only manually added sideloaded games can be imported",
            ));
        }
        let base_exe = executable(entry)
            .ok_or_else(|| missing_file(&game_path, "sideload entry has no executable"))?;
        let target_exe = effective
            .get("targetExe")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or(base_exe);
        let (exe, relative_exe) = self.expand_path_buf(&target_exe);
        review
            .carried
            .push(review_item("targetExe", exe.display().to_string()));
        if relative_exe {
            issues.push(HeroicIssue::needs_attention(
                Some("targetExe".to_owned()),
                "executable path was relative and was resolved against Heroic config",
            ));
        }
        let launcher_args = effective
            .get("launcherArgs")
            .and_then(Value::as_str)
            .unwrap_or("");
        let args = match crate::heroic_args::parse(launcher_args) {
            Ok(args) => {
                review.carried.push(review_item(
                    "launcherArgs",
                    format!("{launcher_args:?} -> {args:?}"),
                ));
                args
            }
            Err(error) => {
                issues.push(HeroicIssue::invalid(
                    Some("launcherArgs".to_owned()),
                    format!("invalid Heroic quoting: {error}"),
                ));
                review.needs_attention.push(review_item(
                    "launcherArgs",
                    format!("{launcher_args:?} could not be parsed"),
                ));
                Vec::new()
            }
        };
        // wineo manages none of Wine's variables for a native game, so they stay.
        let explicit = parse_environment(
            effective.get("enviromentOptions"),
            false,
            &mut issues,
            &mut review,
        );
        let mut env = prime_environment(&effective, &mut review);
        env.extend(explicit);
        let gamemode = effective.get("useGameMode").and_then(Value::as_bool);
        if let Some(value) = gamemode {
            review
                .carried
                .push(review_item("useGameMode", value.to_string()));
        }
        collect_unsupported(
            &effective,
            global,
            &overrides,
            true,
            &mut issues,
            &mut review,
        );
        Ok(HeroicNativeImport {
            id: game_id.to_owned(),
            title: string_field(entry, &["title", "name"])
                .unwrap_or(game_id)
                .to_owned(),
            exe,
            args,
            launcher_args: launcher_args.to_owned(),
            env,
            gamemode,
            issues,
            fingerprint: self.fingerprint(game_id)?,
            settings_review: review,
        })
    }

    pub fn shared_prefix_candidates(&self, game_id: &str) -> Result<Vec<HeroicSharedPrefix>> {
        let selected = self.inspect(game_id)?;
        let selected_prefix = crate::paths::resolve(&selected.prefix);
        let mut shared = self
            .list_candidates()?
            .candidates
            .into_iter()
            .filter(|candidate| candidate.id != game_id)
            .filter(|candidate| {
                candidate
                    .prefix
                    .as_ref()
                    .is_some_and(|prefix| crate::paths::resolve(prefix) == selected_prefix)
            })
            .map(|candidate| HeroicSharedPrefix {
                id: candidate.id,
                title: candidate.title,
            })
            .collect::<Vec<_>>();
        shared.sort_by(|a, b| a.title.cmp(&b.title).then(a.id.cmp(&b.id)));
        Ok(shared)
    }

    pub fn fingerprint(&self, game_id: &str) -> Result<InputFingerprint> {
        if !valid_game_id(game_id) {
            return Err(invalid_json(
                &self.game_config_path(game_id),
                "invalid game id",
            ));
        }
        let paths = [
            self.config_path(),
            self.library_path(),
            self.game_config_path(game_id),
        ];
        let mut files = Vec::new();
        let mut hasher = Sha256::new();
        for path in paths {
            let (digest, size) = match fs::read(&path) {
                Ok(bytes) => {
                    let mut file_hasher = Sha256::new();
                    file_hasher.update(&bytes);
                    (
                        Some(hex_digest(file_hasher.finalize())),
                        Some(bytes.len() as u64),
                    )
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => (None, None),
                Err(error) => {
                    return Err(Error::Io {
                        context: format!("failed to read {}", path.display()),
                        source: error,
                    });
                }
            };
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update([0]);
            hasher.update(digest.as_deref().unwrap_or("missing").as_bytes());
            hasher.update([0]);
            files.push(InputFileFingerprint { path, digest, size });
        }
        Ok(InputFingerprint {
            digest: hex_digest(hasher.finalize()),
            files,
        })
    }

    pub fn unchanged(&self, game_id: &str, expected: &InputFingerprint) -> Result<bool> {
        Ok(self.fingerprint(game_id)?.digest == expected.digest)
    }

    fn inspect_entry(
        &self,
        game_id: &str,
        entry: &Map<String, Value>,
        global: &Map<String, Value>,
    ) -> Result<HeroicImport> {
        let game_path = self.game_config_path(game_id);
        let game_root = match read_optional_json(&game_path)? {
            Some(root) => root,
            None => Value::Object(Map::new()),
        };
        let version = schema_version(&game_root);
        if !supported_schema_root(&game_root) {
            return Err(invalid_json(
                &game_path,
                format!(
                    "unsupported Heroic game config version {}",
                    version.unwrap_or_default()
                ),
            ));
        }
        let overrides = game_settings(&game_root, game_id).unwrap_or_default();
        let mut effective = global.clone();
        for (key, value) in &overrides {
            effective.insert(key.clone(), value.clone());
        }
        let mut review = SettingsReview::default();
        let mut issues = Vec::new();
        let kind = game_kind(entry);
        if kind != HeroicGameKind::Windows {
            issues.push(HeroicIssue::unsupported(
                Some("platform".to_owned()),
                "only Windows sideloads can be imported",
            ));
        }
        if string_field(entry, &["runner"]) != Some("sideload") {
            issues.push(HeroicIssue::unsupported(
                Some("runner".to_owned()),
                "only manually added sideloaded games can be imported",
            ));
        }

        let prefix = effective
            .get("winePrefix")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| self.expand_path(value).0)
            .ok_or_else(|| missing_file(&game_path, "Heroic has no winePrefix for this game"))?;
        let runner = parse_runner(&effective, &self.home, &mut issues, &mut review)
            .ok_or_else(|| missing_file(&game_path, "Heroic has no usable wineVersion"))?;
        let prefix = normalize_heroic_prefix(prefix, runner.kind);
        let prefix_layout = detect_prefix_layout(&prefix);
        if !prefix.exists() {
            issues.push(HeroicIssue::missing(
                Some("winePrefix".to_owned()),
                format!("prefix {} does not exist", prefix.display()),
            ));
        } else if prefix_layout == PrefixLayout::Invalid {
            issues.push(HeroicIssue::missing(
                Some("winePrefix".to_owned()),
                "prefix is missing system.reg or drive_c",
            ));
        } else if prefix_layout == PrefixLayout::Both {
            issues.push(HeroicIssue::conflict(
                Some("winePrefix".to_owned()),
                "prefix contains both direct and nested Proton registry markers",
            ));
        }
        review
            .carried
            .push(review_item("winePrefix", prefix.display().to_string()));

        if (runner.kind == HeroicRunnerKind::Proton && prefix_layout == PrefixLayout::Wine)
            || (runner.kind == HeroicRunnerKind::Wine && prefix_layout == PrefixLayout::Proton)
        {
            issues.push(HeroicIssue::conflict(
                Some("wineVersion".to_owned()),
                "Heroic runner kind does not match prefix layout",
            ));
        }

        let base_exe = executable(entry)
            .ok_or_else(|| missing_file(&game_path, "sideload entry has no executable"))?;
        let target_exe = effective
            .get("targetExe")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or(base_exe);
        let (exe, relative_exe) = self.expand_path_buf(&target_exe);
        review
            .carried
            .push(review_item("targetExe", exe.display().to_string()));
        if relative_exe {
            issues.push(HeroicIssue::needs_attention(
                Some("targetExe".to_owned()),
                "executable path was relative and was resolved against Heroic config",
            ));
        }

        let launcher_args = effective
            .get("launcherArgs")
            .and_then(Value::as_str)
            .unwrap_or("");
        let args = match crate::heroic_args::parse(launcher_args) {
            Ok(args) => {
                review.carried.push(review_item(
                    "launcherArgs",
                    format!("{launcher_args:?} -> {args:?}"),
                ));
                args
            }
            Err(error) => {
                issues.push(HeroicIssue::invalid(
                    Some("launcherArgs".to_owned()),
                    format!("invalid Heroic quoting: {error}"),
                ));
                review.needs_attention.push(review_item(
                    "launcherArgs",
                    format!("{launcher_args:?} could not be parsed"),
                ));
                Vec::new()
            }
        };
        let cwd = exe.parent().map(Path::to_owned);
        if let Some(cwd) = &cwd {
            review
                .carried
                .push(review_item("workingDirectory", cwd.display().to_string()));
        }
        let explicit_env = parse_environment(
            effective.get("enviromentOptions"),
            true,
            &mut issues,
            &mut review,
        );
        let env = translated_environment(
            &effective,
            runner.kind,
            explicit_env,
            &mut issues,
            &mut review,
        );
        let gamemode = effective.get("useGameMode").and_then(Value::as_bool);
        if let Some(value) = gamemode {
            review
                .carried
                .push(review_item("useGameMode", value.to_string()));
        }

        collect_unsupported(
            &effective,
            global,
            &overrides,
            false,
            &mut issues,
            &mut review,
        );
        if runner.kind == HeroicRunnerKind::Proton
            && bool_setting(&overrides, "disableUMU") == Some(true)
        {
            let detail = "Heroic disabled umu; wineo uses umu for Proton";
            issues.push(HeroicIssue::unsupported(
                Some("disableUMU".to_owned()),
                detail,
            ));
            review.unsupported.push(review_item("disableUMU", detail));
        }
        if matches!(
            runner.kind,
            HeroicRunnerKind::Wine | HeroicRunnerKind::System
        ) && bool_setting(&effective, "autoInstallDxvkNvapi") == Some(true)
        {
            let detail =
                "Wine NVAPI requires dxvk-nvapi DLLs; enabling variables does not install them";
            issues.push(HeroicIssue::needs_attention(
                Some("autoInstallDxvkNvapi".to_owned()),
                detail,
            ));
            review
                .needs_attention
                .push(review_item("autoInstallDxvkNvapi", detail));
        }
        let mut external_dependencies = Vec::new();
        if let Some(runner_source) = runner.source.clone() {
            external_dependencies.push(runner_source.clone());
            review.needs_attention.push(review_item(
                "wineVersion",
                format!(
                    "runner is currently stored outside wineo at {}",
                    runner_source.display()
                ),
            ));
        }
        if !path_is_within(&exe, &prefix) {
            external_dependencies.push(exe.clone());
        }
        if let Some(folder) = string_field(entry, &["folder_name"]) {
            let (folder, _) = self.expand_path(folder);
            if !external_dependencies.contains(&folder) {
                external_dependencies.push(folder.clone());
            }
            review.needs_attention.push(review_item(
                "folder_name",
                format!("external game files remain at {}", folder.display()),
            ));
        }
        if let Some(saves) = effective.get("savesPath").and_then(Value::as_str) {
            let (saves, _) = self.expand_path(saves);
            if !external_dependencies.contains(&saves) {
                external_dependencies.push(saves.clone());
            }
            review.needs_attention.push(review_item(
                "savesPath",
                format!("external saves remain at {}", saves.display()),
            ));
        }
        if !external_dependencies.is_empty() {
            issues.push(HeroicIssue::external(
                Some("external files".to_owned()),
                "game files or saves remain outside the imported prefix",
            ));
        }

        Ok(HeroicImport {
            id: game_id.to_owned(),
            title: string_field(entry, &["title", "name"])
                .unwrap_or(game_id)
                .to_owned(),
            prefix,
            runner,
            kind,
            exe,
            args,
            launcher_args: launcher_args.to_owned(),
            cwd,
            env,
            gamemode,
            dxvk: None,
            vkd3d: None,
            issues,
            external_dependencies,
            fingerprint: self.fingerprint(game_id)?,
            settings_review: review,
        })
    }

    fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    fn library_path(&self) -> PathBuf {
        self.config_dir.join("sideload_apps/library.json")
    }

    fn game_config_path(&self, game_id: &str) -> PathBuf {
        self.config_dir
            .join("GamesConfig")
            .join(format!("{game_id}.json"))
    }

    fn global_settings(&self) -> Result<Map<String, Value>> {
        let path = self.config_path();
        let Some(root) = read_optional_json(&path)? else {
            return Ok(Map::new());
        };
        let version = schema_version(&root);
        if !supported_global_schema_root(&root) {
            return Err(invalid_json(
                &path,
                format!(
                    "unsupported Heroic global config version {}",
                    version.unwrap_or_default()
                ),
            ));
        }
        let stored = root
            .get("defaultSettings")
            .or_else(|| {
                root.get("settings")
                    .and_then(|value| value.get("defaultSettings"))
            })
            .or_else(|| root.get("settings"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        Ok(stored)
    }

    fn expand_path(&self, value: &str) -> (PathBuf, bool) {
        let path = if value == "~" {
            self.home.clone().unwrap_or_else(|| PathBuf::from(value))
        } else if let Some(rest) = value.strip_prefix("~/") {
            self.home
                .as_ref()
                .map(|home| home.join(rest))
                .unwrap_or_else(|| PathBuf::from(value))
        } else {
            PathBuf::from(value)
        };
        if path.is_absolute() {
            (path, false)
        } else {
            (self.config_dir.join(path), true)
        }
    }

    fn expand_path_buf(&self, path: &Path) -> (PathBuf, bool) {
        self.expand_path(&path.to_string_lossy())
    }
}

#[derive(Debug, Clone)]
pub struct HeroicListing {
    pub source: HeroicSource,
    pub candidates: Vec<HeroicCandidate>,
    pub issues: Vec<HeroicIssue>,
}

#[derive(Debug, Clone)]
pub struct HeroicCandidate {
    pub id: String,
    pub title: String,
    pub kind: HeroicGameKind,
    pub prefix: Option<PathBuf>,
    pub runner: Option<HeroicRunner>,
    pub executable: Option<PathBuf>,
    pub issues: Vec<HeroicIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroicSharedPrefix {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeroicGameKind {
    Windows,
    Native,
    Browser,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeroicRunnerKind {
    Wine,
    Proton,
    System,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeroicRunner {
    pub kind: HeroicRunnerKind,
    pub name: String,
    pub path: Option<PathBuf>,
    pub source: Option<PathBuf>,
}

/// One app's playtime as Heroic tracked it, from `store/timestamp.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeroicPlaytime {
    pub seconds: i64,
    pub last_played: Option<Timestamp>,
}

#[derive(Debug, Clone)]
pub struct HeroicImport {
    pub id: String,
    pub title: String,
    pub prefix: PathBuf,
    pub runner: HeroicRunner,
    pub kind: HeroicGameKind,
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub launcher_args: String,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub gamemode: Option<bool>,
    /// `dxvk`/`vkd3d` to carry into the environment config, filled in by the
    /// importer once it knows the chosen wineo runner. Inspection itself
    /// never sets these, since Proton bundles its own and only the CLI
    /// knows which runner was picked.
    pub dxvk: Option<String>,
    pub vkd3d: Option<String>,
    pub issues: Vec<HeroicIssue>,
    pub external_dependencies: Vec<PathBuf>,
    pub fingerprint: InputFingerprint,
    pub settings_review: SettingsReview,
}

/// A native sideloaded game, as a native environment would run it.
#[derive(Debug, Clone)]
pub struct HeroicNativeImport {
    pub id: String,
    pub title: String,
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub launcher_args: String,
    pub env: BTreeMap<String, String>,
    pub gamemode: Option<bool>,
    pub issues: Vec<HeroicIssue>,
    pub fingerprint: InputFingerprint,
    pub settings_review: SettingsReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeroicIssueKind {
    Invalid,
    Unsupported,
    NeedsAttention,
    Conflict,
    Missing,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeroicIssue {
    pub kind: HeroicIssueKind,
    pub field: Option<String>,
    pub message: String,
}

impl HeroicIssue {
    fn new(kind: HeroicIssueKind, field: Option<String>, message: impl Into<String>) -> Self {
        Self {
            kind,
            field,
            message: message.into(),
        }
    }

    fn invalid(field: Option<String>, message: impl Into<String>) -> Self {
        Self::new(HeroicIssueKind::Invalid, field, message)
    }

    fn unsupported(field: Option<String>, message: impl Into<String>) -> Self {
        Self::new(HeroicIssueKind::Unsupported, field, message)
    }

    fn needs_attention(field: Option<String>, message: impl Into<String>) -> Self {
        Self::new(HeroicIssueKind::NeedsAttention, field, message)
    }

    fn conflict(field: Option<String>, message: impl Into<String>) -> Self {
        Self::new(HeroicIssueKind::Conflict, field, message)
    }

    fn missing(field: Option<String>, message: impl Into<String>) -> Self {
        Self::new(HeroicIssueKind::Missing, field, message)
    }

    fn external(field: Option<String>, message: impl Into<String>) -> Self {
        Self::new(HeroicIssueKind::External, field, message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewItem {
    pub field: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsReview {
    pub carried: Vec<ReviewItem>,
    pub needs_attention: Vec<ReviewItem>,
    pub unsupported: Vec<ReviewItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputFileFingerprint {
    pub path: PathBuf,
    pub digest: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputFingerprint {
    pub digest: String,
    pub files: Vec<InputFileFingerprint>,
}

/// Parse Heroic's launcher argument string without invoking a shell.
pub fn parse_launcher_args(value: &str) -> std::result::Result<Vec<String>, String> {
    crate::heroic_args::parse(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrefixLayout {
    Wine,
    Proton,
    Both,
    Invalid,
}

fn review_item(field: impl Into<String>, detail: impl Into<String>) -> ReviewItem {
    ReviewItem {
        field: field.into(),
        detail: detail.into(),
    }
}

fn is_heroic_root(path: &Path) -> bool {
    path.is_dir()
        && ["config.json", "GamesConfig", "sideload_apps"]
            .iter()
            .any(|name| path.join(name).exists())
}

fn library_entries(root: &Value) -> Option<&Vec<Value>> {
    root.get("games")
        .and_then(Value::as_array)
        .or_else(|| root.as_array())
}

fn string_field<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
}

fn executable(object: &Map<String, Value>) -> Option<PathBuf> {
    object
        .get("install")
        .and_then(Value::as_object)
        .and_then(|install| install.get("executable"))
        .and_then(Value::as_str)
        .or_else(|| string_field(object, &["executable"]))
        .map(PathBuf::from)
}

/// The platform an entry names, lowercased, like `linux` or `windows`.
fn platform(object: &Map<String, Value>) -> Option<&'static str> {
    let platform = object
        .get("install")
        .and_then(Value::as_object)
        .and_then(|install| install.get("platform"))
        .and_then(Value::as_str)
        .or_else(|| object.get("platform").and_then(Value::as_str))?;
    [
        "linux", "mac", "darwin", "windows", "win32", "win64", "browser",
    ]
    .into_iter()
    .find(|known| platform.eq_ignore_ascii_case(known))
}

fn game_kind(object: &Map<String, Value>) -> HeroicGameKind {
    if object
        .get("browserUrl")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.is_empty())
    {
        return HeroicGameKind::Browser;
    }
    let platform = object
        .get("install")
        .and_then(Value::as_object)
        .and_then(|install| install.get("platform"))
        .and_then(Value::as_str)
        .or_else(|| object.get("platform").and_then(Value::as_str));
    match platform.map(str::to_ascii_lowercase).as_deref() {
        Some("windows") | Some("win32") | Some("win64") => HeroicGameKind::Windows,
        Some("linux") | Some("mac") | Some("darwin") => HeroicGameKind::Native,
        Some("browser") => HeroicGameKind::Browser,
        _ if executable(object).is_some() => HeroicGameKind::Windows,
        _ => HeroicGameKind::Unknown,
    }
}

fn valid_game_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains(['/', '\\', '\0'])
        && !id.chars().any(char::is_control)
}

fn schema_version(root: &Value) -> Option<String> {
    root.get("version")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn supported_schema(version: Option<&str>) -> bool {
    matches!(version, None | Some("v0") | Some("v0.1"))
}

fn supported_schema_root(root: &Value) -> bool {
    match root.get("version") {
        None => true,
        Some(Value::String(version)) => supported_schema(Some(version)),
        Some(_) => false,
    }
}

fn supported_global_schema_root(root: &Value) -> bool {
    match root.get("version") {
        None => true,
        Some(Value::String(version)) => version == "v0",
        Some(_) => false,
    }
}

fn game_settings(root: &Value, id: &str) -> Option<Map<String, Value>> {
    let object = root.as_object()?;
    if let Some(value) = object.get(id).and_then(Value::as_object) {
        return Some(value.clone());
    }
    if let Some(value) = object.get("settings").and_then(Value::as_object) {
        return Some(value.clone());
    }
    let mut settings = object.clone();
    settings.remove("version");
    settings.remove("explicit");
    Some(settings)
}

fn parse_runner(
    settings: &Map<String, Value>,
    home: &Option<PathBuf>,
    issues: &mut Vec<HeroicIssue>,
    review: &mut SettingsReview,
) -> Option<HeroicRunner> {
    let object = settings.get("wineVersion")?.as_object()?;
    let type_name = object.get("type").and_then(Value::as_str).unwrap_or("");
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let raw_bin = object
        .get("bin")
        .and_then(Value::as_str)
        .map(|value| value.trim_matches('\''));
    let kind = match type_name {
        "wine"
            if raw_bin.is_none()
                || raw_bin == Some("wine")
                || name.to_ascii_lowercase().contains("system") =>
        {
            HeroicRunnerKind::System
        }
        "wine" => HeroicRunnerKind::Wine,
        "proton" => HeroicRunnerKind::Proton,
        "" => HeroicRunnerKind::Unknown,
        _ => HeroicRunnerKind::Unknown,
    };
    let (path, source) = if kind == HeroicRunnerKind::System {
        (None, None)
    } else {
        raw_bin
            .map(|bin| {
                let path = expand_external_path(bin, home);
                let source = runner_root(&path, type_name);
                (Some(path), Some(source))
            })
            .unwrap_or((None, None))
    };
    if kind == HeroicRunnerKind::Unknown {
        issues.push(HeroicIssue::unsupported(
            Some("wineVersion.type".to_owned()),
            format!("Heroic runner type {type_name} is not Wine or Proton"),
        ));
    }
    if type_name == "proton" && path.is_none() {
        issues.push(HeroicIssue::missing(
            Some("wineVersion.bin".to_owned()),
            "Proton runner metadata has no executable path",
        ));
    }
    let (path, source) = if kind == HeroicRunnerKind::System {
        (None, None)
    } else {
        (path, source)
    };
    review
        .carried
        .push(review_item("wineVersion", format!("{name} ({type_name})")));
    Some(HeroicRunner {
        kind,
        name,
        path,
        source,
    })
}

fn runner_root(path: &Path, type_name: &str) -> PathBuf {
    if type_name == "proton" {
        path.parent().unwrap_or(path).to_owned()
    } else if path.file_name().and_then(|name| name.to_str()) == Some("wine") {
        path.parent()
            .and_then(Path::parent)
            .unwrap_or(path)
            .to_owned()
    } else {
        path.parent().unwrap_or(path).to_owned()
    }
}

fn expand_external_path(value: &str, home: &Option<PathBuf>) -> PathBuf {
    if value == "~" {
        return home.clone().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home
            .as_ref()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(value));
    }
    PathBuf::from(value)
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = crate::paths::resolve(path);
    let root = crate::paths::resolve(root);
    path == root || path.starts_with(&root)
}

fn normalize_heroic_prefix(prefix: PathBuf, runner: HeroicRunnerKind) -> PathBuf {
    if runner == HeroicRunnerKind::Proton
        && prefix.file_name().and_then(|name| name.to_str()) == Some("pfx")
        && prefix.parent().is_some_and(|parent| {
            parent.join("pfx") == prefix && detect_prefix_layout(parent) == PrefixLayout::Proton
        })
    {
        prefix.parent().unwrap().to_owned()
    } else {
        prefix
    }
}

fn detect_prefix_layout(prefix: &Path) -> PrefixLayout {
    let direct = prefix.join("system.reg").is_file() && prefix.join("drive_c").is_dir();
    let nested = prefix.join("pfx/system.reg").is_file() && prefix.join("pfx/drive_c").is_dir();
    match (direct, nested) {
        (true, false) => PrefixLayout::Wine,
        (false, true) => PrefixLayout::Proton,
        (true, true) => PrefixLayout::Both,
        (false, false) => PrefixLayout::Invalid,
    }
}

fn read_optional_json(path: &Path) -> Result<Option<Value>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| invalid_json_source(path, source.to_string())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::Io {
            context: format!("failed to read {}", path.display()),
            source: error,
        }),
    }
}

fn read_json(path: &Path) -> Result<Value> {
    read_optional_json(path)?.ok_or_else(|| missing_file(path, "file does not exist"))
}

fn invalid_json(path: &Path, message: impl Into<String>) -> Error {
    invalid_json_source(path, message.into())
}

fn invalid_json_source(path: &Path, message: String) -> Error {
    Error::Io {
        context: format!("invalid Heroic JSON {}: {message}", path.display()),
        source: io::Error::new(io::ErrorKind::InvalidData, message),
    }
}

fn missing_file(path: &Path, message: impl Into<String>) -> Error {
    Error::Io {
        context: format!("{}: {}", path.display(), message.into()),
        source: io::Error::new(io::ErrorKind::NotFound, "Heroic metadata is missing"),
    }
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `managed` drops the variables wineo sets itself for a Windows game.
fn parse_environment(
    value: Option<&Value>,
    managed: bool,
    issues: &mut Vec<HeroicIssue>,
    review: &mut SettingsReview,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let Some(value) = value else {
        return env;
    };
    let Some(entries) = value.as_array() else {
        issues.push(HeroicIssue::invalid(
            Some("enviromentOptions".to_owned()),
            "expected an array of environment objects",
        ));
        return env;
    };
    for (index, entry) in entries.iter().enumerate() {
        let Some(entry) = entry.as_object() else {
            issues.push(HeroicIssue::invalid(
                Some(format!("enviromentOptions[{index}]")),
                "entry is not an object",
            ));
            continue;
        };
        let Some(key) = entry.get("key").and_then(Value::as_str) else {
            issues.push(HeroicIssue::invalid(
                Some(format!("enviromentOptions[{index}].key")),
                "environment entry has no string key",
            ));
            continue;
        };
        let Some(raw_value) = entry.get("value").and_then(Value::as_str) else {
            issues.push(HeroicIssue::invalid(
                Some(format!("enviromentOptions[{index}].value")),
                "environment entry has no string value",
            ));
            continue;
        };
        if key.is_empty() || key.contains(['=', '\0']) {
            issues.push(HeroicIssue::invalid(
                Some(format!("enviromentOptions[{index}].key")),
                "invalid environment variable name",
            ));
            continue;
        }
        if managed && MANAGED_ENV.contains(&key) {
            issues.push(HeroicIssue::conflict(
                Some(key.to_owned()),
                "Heroic setting conflicts with a wineo-managed variable",
            ));
            continue;
        }
        env.insert(key.to_owned(), remove_quotes(raw_value));
    }
    for key in env.keys() {
        review
            .carried
            .push(review_item(format!("env.{key}"), "value retained"));
    }
    env
}

/// The variables Heroic's `nvidiaPrime` sets, for any kind of game. Explicit
/// `enviromentOptions` go on top of these.
fn prime_environment(
    settings: &Map<String, Value>,
    review: &mut SettingsReview,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if bool_setting(settings, "nvidiaPrime") == Some(true) {
        env.insert("DRI_PRIME".to_owned(), "1".to_owned());
        env.insert("__NV_PRIME_RENDER_OFFLOAD".to_owned(), "1".to_owned());
        env.insert("__GLX_VENDOR_LIBRARY_NAME".to_owned(), "nvidia".to_owned());
        review.carried.push(review_item(
            "nvidiaPrime",
            "DRI_PRIME=1 and NVIDIA offload variables",
        ));
    }
    env
}

fn translated_environment(
    settings: &Map<String, Value>,
    runner: HeroicRunnerKind,
    mut explicit: BTreeMap<String, String>,
    issues: &mut Vec<HeroicIssue>,
    review: &mut SettingsReview,
) -> BTreeMap<String, String> {
    let mut env = prime_environment(settings, review);
    let wine_runner = matches!(runner, HeroicRunnerKind::Wine | HeroicRunnerKind::System);
    // Heroic applies explicit enviromentOptions after general settings.
    env.append(&mut explicit);

    if bool_setting(settings, "showFps") == Some(true) {
        env.insert("DXVK_HUD".to_owned(), "fps".to_owned());
        review.carried.push(review_item("showFps", "DXVK_HUD=fps"));
    }
    if bool_setting(settings, "enableDXVKFpsLimit") == Some(true) {
        if let Some(value) = value_string(settings.get("DXVKFpsCap")) {
            env.insert("DXVK_FRAME_RATE".to_owned(), value.clone());
            review.carried.push(review_item(
                "DXVKFpsCap",
                format!("DXVK_FRAME_RATE={value}"),
            ));
        } else {
            issues.push(HeroicIssue::needs_attention(
                Some("DXVKFpsCap".to_owned()),
                "FPS limit is enabled but no cap was stored",
            ));
        }
    }
    if let Some(enable) = bool_setting(settings, "enableFSR") {
        env.insert(
            "WINE_FULLSCREEN_FSR".to_owned(),
            if enable { "1" } else { "0" }.to_owned(),
        );
        if enable {
            let strength =
                value_string(settings.get("maxSharpness")).unwrap_or_else(|| "2".to_owned());
            env.insert("WINE_FULLSCREEN_FSR_STRENGTH".to_owned(), strength.clone());
            review.carried.push(review_item(
                "enableFSR",
                format!("WINE_FULLSCREEN_FSR=1, strength={strength}"),
            ));
        } else {
            review
                .carried
                .push(review_item("enableFSR", "WINE_FULLSCREEN_FSR=0"));
        }
    }
    if let Some(enable) = bool_setting(settings, "enableEsync") {
        if wine_runner && enable {
            env.insert("WINEESYNC".to_owned(), "1".to_owned());
        } else if runner == HeroicRunnerKind::Proton && !enable {
            env.insert("PROTON_NO_ESYNC".to_owned(), "1".to_owned());
        }
        review
            .carried
            .push(review_item("enableEsync", enable.to_string()));
    }
    if let Some(enable) = bool_setting(settings, "enableFsync") {
        if wine_runner && enable {
            env.insert("WINEFSYNC".to_owned(), "1".to_owned());
        } else if runner == HeroicRunnerKind::Proton && !enable {
            env.insert("PROTON_NO_FSYNC".to_owned(), "1".to_owned());
        }
        review
            .carried
            .push(review_item("enableFsync", enable.to_string()));
    }
    if let Some(enable) = bool_setting(settings, "autoInstallDxvkNvapi") {
        match (runner, enable) {
            (HeroicRunnerKind::Proton, true) => {
                env.insert("PROTON_ENABLE_NVAPI".to_owned(), "1".to_owned());
                env.insert("DXVK_NVAPI_ALLOW_OTHER_DRIVERS".to_owned(), "1".to_owned());
            }
            (HeroicRunnerKind::Proton, false) => {
                env.insert("PROTON_DISABLE_NVAPI".to_owned(), "1".to_owned());
            }
            (HeroicRunnerKind::Wine | HeroicRunnerKind::System, true) => {
                env.insert("DXVK_ENABLE_NVAPI".to_owned(), "1".to_owned());
                env.insert("DXVK_NVAPI_ALLOW_OTHER_DRIVERS".to_owned(), "1".to_owned());
            }
            _ => {}
        }
        review
            .carried
            .push(review_item("autoInstallDxvkNvapi", enable.to_string()));
    }
    if bool_setting(settings, "nvidiaPrime") == Some(true) {
        for (key, value) in [
            ("DRI_PRIME", "1"),
            ("__NV_PRIME_RENDER_OFFLOAD", "1"),
            ("__GLX_VENDOR_LIBRARY_NAME", "nvidia"),
        ] {
            env.entry(key.to_owned())
                .or_insert_with(|| value.to_owned());
        }
    }
    env
}

fn collect_unsupported(
    settings: &Map<String, Value>,
    global: &Map<String, Value>,
    overrides: &Map<String, Value>,
    native: bool,
    issues: &mut Vec<HeroicIssue>,
    review: &mut SettingsReview,
) {
    let unsupported = [
        ("wrapperOptions", "Heroic wrappers require manual setup"),
        (
            "beforeLaunchScriptPath",
            "Heroic launch scripts are not executed during import",
        ),
        (
            "afterLaunchScriptPath",
            "Heroic launch scripts are not executed during import",
        ),
        (
            "gamescope",
            "gamescope command construction needs manual setup",
        ),
        ("showMangohud", "MangoHud wrapper setup needs manual setup"),
        (
            "enableWineWayland",
            "Wine Wayland handling is runner-specific",
        ),
        ("enableHDR", "HDR handling is runner-specific"),
        (
            "enableWoW64",
            "architecture mode is not copied into wineo settings",
        ),
        (
            "eacRuntime",
            "Heroic-managed anti-cheat runtime is not imported",
        ),
        (
            "battlEyeRuntime",
            "Heroic-managed anti-cheat runtime is not imported",
        ),
        (
            "steamRuntime",
            "Heroic runtime selection conflicts with wineo runner selection",
        ),
        (
            "autoInstallDxvk",
            "Heroic component installation is not imported",
        ),
        (
            "autoInstallVkd3d",
            "Heroic component installation is not imported",
        ),
        (
            "preferSystemLibs",
            "Heroic system library selection is not imported",
        ),
        (
            "enableMsync",
            "Heroic msync selection is not represented in wineo settings",
        ),
        (
            "wineCrossoverBottle",
            "Heroic CrossOver bottle selection needs manual setup",
        ),
        (
            "advertiseAvxForRosetta",
            "Heroic Rosetta AVX setting is not imported",
        ),
        (
            "enableQuickSavesMenu",
            "Heroic quick save menu setting is not imported",
        ),
    ];
    for (field, detail) in unsupported {
        if native && !NATIVE_UNSUPPORTED.contains(&field) {
            continue;
        }
        let Some(value) = settings.get(field) else {
            continue;
        };
        let active = match value {
            Value::Bool(value) => *value,
            Value::String(value) => !value.is_empty(),
            Value::Array(value) => !value.is_empty(),
            Value::Object(value) => !value.is_empty(),
            Value::Null => false,
            _ => true,
        };
        if !active {
            continue;
        }
        let kind = if field == "steamRuntime" {
            HeroicIssueKind::Conflict
        } else {
            HeroicIssueKind::Unsupported
        };
        issues.push(HeroicIssue::new(kind, Some(field.to_owned()), detail));
        review.unsupported.push(review_item(field, detail));
    }
    let known: BTreeSet<&str> = [
        "version",
        "explicit",
        "winePrefix",
        "wineVersion",
        "launcherArgs",
        "targetExe",
        "enviromentOptions",
        "useGameMode",
        "nvidiaPrime",
        "showFps",
        "enableDXVKFpsLimit",
        "DXVKFpsCap",
        "enableFSR",
        "maxSharpness",
        "enableEsync",
        "enableFsync",
        "enableMsync",
        "autoInstallDxvkNvapi",
        "wrapperOptions",
        "beforeLaunchScriptPath",
        "afterLaunchScriptPath",
        "gamescope",
        "showMangohud",
        "enableWineWayland",
        "enableHDR",
        "enableWoW64",
        "eacRuntime",
        "battlEyeRuntime",
        "steamRuntime",
        "autoInstallDxvk",
        "autoInstallVkd3d",
        "wineCrossoverBottle",
        "advertiseAvxForRosetta",
        "enableQuickSavesMenu",
        "savesPath",
        "preferSystemLibs",
        "offlineMode",
        "verboseLogs",
        "language",
        "disableUMU",
    ]
    .into_iter()
    .collect();
    let ignored_global: BTreeSet<&str> = [
        "addDesktopShortcuts",
        "addStartMenuShortcuts",
        "addSteamShortcuts",
        "analyticsOptIn",
        "autoUpdateGames",
        "checkForUpdatesOnStartup",
        "checkUpdatesInterval",
        "customThemesPath",
        "customWinePaths",
        "darkTrayIcon",
        "defaultInstallPath",
        "defaultSteamPath",
        "defaultWinePrefix",
        "defaultWinePrefixDir",
        "disableGOGPresence",
        "downloadProtonToSteam",
        "enableUpdates",
        "framelessWindow",
        "gamepadInitialRepeatDelay",
        "gamepadRepeatDelay",
        "hideChangelogsOnStartup",
        "hideWindowOnProtocolLaunch",
        "hideWindowOnLaunch",
        "libraryTopSection",
        "maxWorkers",
        "minimizeOnLaunch",
        "noTrayIcon",
        "showValveProton",
        "steamGridDbApiKey",
    ]
    .into_iter()
    .collect();
    let mut fields = BTreeSet::new();
    fields.extend(global.keys().cloned());
    fields.extend(overrides.keys().cloned());
    for field in fields
        .iter()
        .filter(|field| !known.contains(field.as_str()) && !ignored_global.contains(field.as_str()))
    {
        issues.push(HeroicIssue::unsupported(
            Some(field.clone()),
            "unknown Heroic launch-affecting setting requires manual review",
        ));
        review
            .unsupported
            .push(review_item(field, "unknown Heroic setting"));
    }
}

fn bool_setting(settings: &Map<String, Value>, key: &str) -> Option<bool> {
    settings.get(key).and_then(Value::as_bool)
}

fn value_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn remove_quotes(value: &str) -> String {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
        {
            return value[1..value.len() - 1].to_owned();
        }
    }
    value.to_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;

    fn fixture() -> (TempDir, HeroicSource) {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("heroic");
        fs::create_dir_all(config.join("sideload_apps")).unwrap();
        fs::create_dir_all(config.join("GamesConfig")).unwrap();
        let source = HeroicSource::from_config_dir_with_home(config, root.path().to_owned());
        (root, source)
    }

    fn write_fixture(source: &HeroicSource, global: &str, game: &str, library: &str) {
        fs::write(source.config_path(), global).unwrap();
        fs::write(source.library_path(), library).unwrap();
        if !game.is_empty() {
            fs::write(source.game_config_path("hades"), game).unwrap();
        }
    }

    fn wine_prefix(root: &Path) -> PathBuf {
        let prefix = root.join("prefix");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        prefix
    }

    #[test]
    fn effective_settings_preserve_false_empty_arrays_and_shell_boundaries() {
        let (root, source) = fixture();
        let prefix = wine_prefix(root.path());
        let global = format!(
            r#"{{"version":"v0","defaultSettings":{{
                "winePrefix":"{}",
                "wineVersion":{{"type":"wine","name":"System","bin":"wine"}},
                "enviromentOptions":[{{"key":"GLOBAL","value":"global"}},{{"key":"EMPTY","value":"global"}}],
                "enableFSR":true,"enableEsync":true,"useGameMode":true
            }}}}"#,
            prefix.display()
        );
        let game = r#"{
            "version":"v0.1",
            "hades":{"enviromentOptions":[],"enableFSR":false,"enableEsync":false,"useGameMode":false,"launcherArgs":"--name 'two words'"}
        }"#;
        let library = format!(
            r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
            prefix.join("drive_c/Hades.exe").display()
        );
        write_fixture(&source, &global, game, &library);
        let import = source.inspect("hades").unwrap();
        assert_eq!(import.args, ["--name", "two words"]);
        assert!(!import.env.contains_key("GLOBAL"));
        assert_eq!(import.env.get("WINE_FULLSCREEN_FSR"), Some(&"0".to_owned()));
        assert_eq!(import.gamemode, Some(false));
        assert!(
            import
                .fingerprint
                .files
                .iter()
                .all(|file| file.digest.is_some())
        );
    }

    #[test]
    fn list_isolates_invalid_and_native_entries() {
        let (root, source) = fixture();
        let prefix = wine_prefix(root.path());
        let global = format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
            prefix.display()
        );
        let library = format!(
            r#"{{"games":[null,{{"app_name":"../bad","title":"Bad","runner":"sideload","install":{{"platform":"Windows","executable":"x.exe"}}}},{{"app_name":"native","title":"Native","runner":"sideload","install":{{"platform":"Linux","executable":"x"}}}},{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
            prefix.join("game.exe").display()
        );
        write_fixture(&source, &global, "", &library);
        let listing = source.list_candidates().unwrap();
        assert_eq!(listing.candidates.len(), 3);
        assert!(!listing.issues.is_empty());
        assert!(
            listing
                .candidates
                .iter()
                .any(|candidate| candidate.id == "native" && !candidate.issues.is_empty())
        );
        assert!(
            listing
                .candidates
                .iter()
                .any(|candidate| candidate.id == "hades" && candidate.issues.is_empty())
        );
    }

    #[test]
    fn conflicting_managed_environment_is_reported_without_being_imported() {
        let (root, source) = fixture();
        let prefix = wine_prefix(root.path());
        let global = format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}},"enviromentOptions":[{{"key":"WINEPREFIX","value":"/tmp/other"}},{{"key":"KEEP","value":""}}]}}}}"#,
            prefix.display()
        );
        let library = format!(
            r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
            prefix.join("game.exe").display()
        );
        write_fixture(&source, &global, "", &library);
        let import = source.inspect("hades").unwrap();
        assert!(!import.env.contains_key("WINEPREFIX"));
        assert_eq!(import.env.get("KEEP"), Some(&String::new()));
        assert!(
            import
                .issues
                .iter()
                .any(|issue| issue.kind == HeroicIssueKind::Conflict)
        );
    }

    /// A native sideload `hades`, with `settings` as Heroic's defaults.
    fn native_game(settings: &str) -> (TempDir, HeroicSource) {
        let (root, source) = fixture();
        let global = format!(r#"{{"defaultSettings":{{{settings}}}}}"#);
        let library = r#"{"games":[{"app_name":"hades","title":"Hades","runner":"sideload","install":{"platform":"linux","executable":"/usr/bin/hades"}}]}"#;
        write_fixture(&source, &global, "", library);
        (root, source)
    }

    #[test]
    fn a_native_import_carries_nvidia_prime_under_explicit_variables() {
        let (_root, source) = native_game(
            r#""nvidiaPrime":true,"showFps":true,"enviromentOptions":[{"key":"DRI_PRIME","value":"0"}]"#,
        );
        let import = source.inspect_native("hades").unwrap();
        assert_eq!(import.env["DRI_PRIME"], "0");
        assert_eq!(import.env["__NV_PRIME_RENDER_OFFLOAD"], "1");
        assert_eq!(import.env["__GLX_VENDOR_LIBRARY_NAME"], "nvidia");
        // DXVK_HUD is Wine's, so showFps means nothing to a native game.
        assert!(!import.env.contains_key("DXVK_HUD"));
        assert!(import.settings_review.carried.contains(&review_item(
            "nvidiaPrime",
            "DRI_PRIME=1 and NVIDIA offload variables"
        )));
    }

    #[test]
    fn a_native_import_keeps_what_wineo_manages_only_for_wine() {
        let (_root, source) = native_game(
            r#""enviromentOptions":[{"key":"WINEPREFIX","value":"/mine"},{"key":"PROTONPATH","value":"/proton"}]"#,
        );
        let import = source.inspect_native("hades").unwrap();
        assert_eq!(import.env["WINEPREFIX"], "/mine");
        assert_eq!(import.env["PROTONPATH"], "/proton");
        assert!(
            !import
                .issues
                .iter()
                .any(|issue| issue.kind == HeroicIssueKind::Conflict)
        );
    }

    #[test]
    fn fingerprint_detects_source_changes() {
        let (root, source) = fixture();
        let prefix = wine_prefix(root.path());
        let global = format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
            prefix.display()
        );
        let library = format!(
            r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
            prefix.join("game.exe").display()
        );
        write_fixture(&source, &global, "", &library);
        let first = source.fingerprint("hades").unwrap();
        assert!(source.unchanged("hades", &first).unwrap());
        fs::write(source.library_path(), library.replace("Hades", "Changed")).unwrap();
        assert!(!source.unchanged("hades", &first).unwrap());
    }

    #[test]
    fn discovery_finds_native_and_flatpak_and_deduplicates_aliases() {
        use std::os::unix::fs::symlink;

        let home = tempfile::tempdir().unwrap();
        let native = home.path().join(".config/heroic");
        fs::create_dir_all(native.join("sideload_apps")).unwrap();
        let flatpak = home
            .path()
            .join(".var/app/com.heroicgameslauncher.hgl/config/heroic");
        fs::create_dir_all(flatpak.join("sideload_apps")).unwrap();
        let sources = HeroicSource::discover(home.path(), None, None).unwrap();
        assert_eq!(sources.len(), 2);
        assert!(
            sources
                .iter()
                .any(|source| source.origin() == HeroicOrigin::Native)
        );
        assert!(
            sources
                .iter()
                .any(|source| source.origin() == HeroicOrigin::Flatpak)
        );

        let alias_root = home
            .path()
            .join(".var/app/com.heroicgameslauncher.hgl/.config");
        fs::create_dir_all(&alias_root).unwrap();
        symlink(&native, alias_root.join("heroic")).unwrap();
        let sources = HeroicSource::discover(home.path(), None, None).unwrap();
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn unsupported_schema_isolated_to_game_and_global_version_is_strict() {
        let (root, source) = fixture();
        let prefix = wine_prefix(root.path());
        let global = format!(
            r#"{{"version":"v0","defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
            prefix.display()
        );
        let library = format!(
            r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
            prefix.join("game.exe").display()
        );
        write_fixture(&source, &global, r#"{"version":"v9","hades":{}}"#, &library);
        let listing = source.list_candidates().unwrap();
        assert!(
            listing.candidates[0]
                .issues
                .iter()
                .any(|issue| issue.kind == HeroicIssueKind::Invalid)
        );

        fs::write(source.config_path(), global.replace("\"v0\"", "\"v9\"")).unwrap();
        assert!(source.list_candidates().is_err());
    }

    #[test]
    fn toggle_translation_handles_system_wine_target_and_unknown_overrides() {
        let (root, source) = fixture();
        let prefix = wine_prefix(root.path());
        let global = format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}},"enableEsync":true,"enableFsync":true,"enviromentOptions":[{{"key":"DXVK_HUD","value":"full"}}],"analyticsOptIn":true,"steamGridDbApiKey":"secret"}}}}"#,
            prefix.display()
        );
        let game = format!(
            r#"{{"version":"v0.1","hades":{{"targetExe":"{}","launcherArgs":"\"\" --name 'two words'","showFps":true,"unknownLaunchMode":true}}}}"#,
            prefix.join("drive_c/Custom.exe").display()
        );
        let library = format!(
            r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
            prefix.join("game.exe").display()
        );
        write_fixture(&source, &global, &game, &library);
        let import = source.inspect("hades").unwrap();
        assert_eq!(import.exe, prefix.join("drive_c/Custom.exe"));
        assert_eq!(import.args, ["", "--name", "two words"]);
        assert_eq!(import.env.get("DXVK_HUD"), Some(&"fps".to_owned()));
        assert_eq!(import.env.get("WINEESYNC"), Some(&"1".to_owned()));
        assert_eq!(import.env.get("WINEFSYNC"), Some(&"1".to_owned()));
        assert!(
            !import
                .issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("steamGridDbApiKey"))
        );
        assert!(
            import
                .issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("unknownLaunchMode"))
        );
        assert!(
            import
                .settings_review
                .carried
                .iter()
                .all(|item| !item.detail.contains("secret"))
        );
    }

    #[test]
    fn multiple_games_can_share_a_prefix() {
        let (root, source) = fixture();
        let prefix = wine_prefix(root.path());
        let global = format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
            prefix.display()
        );
        let library = format!(
            r#"{{"games":[{{"app_name":"one","title":"One","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}},{{"app_name":"two","title":"Two","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
            prefix.join("one.exe").display(),
            prefix.join("two.exe").display()
        );
        fs::write(source.config_path(), global).unwrap();
        fs::write(source.library_path(), library).unwrap();
        assert_eq!(source.list_candidates().unwrap().candidates.len(), 2);
        assert_eq!(
            source.inspect("one").unwrap().prefix,
            source.inspect("two").unwrap().prefix
        );
    }

    #[test]
    fn shared_prefix_candidates_resolve_aliases_and_proton_pfx() {
        use std::os::unix::fs::symlink;

        let (root, source) = fixture();
        let compatdata = root.path().join("compatdata");
        fs::create_dir_all(compatdata.join("pfx/drive_c")).unwrap();
        fs::write(
            compatdata.join("pfx/system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        symlink(&compatdata, root.path().join("alias")).unwrap();
        let global = format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"proton","name":"GE-Proton","bin":"/runners/GE-Proton/proton"}}}}}}"#,
            compatdata.display()
        );
        let library = format!(
            r#"{{"games":[{{"app_name":"one","title":"One","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}},{{"app_name":"two","title":"Two","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}},{{"app_name":"other","title":"Other","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
            compatdata.join("pfx/drive_c/one.exe").display(),
            compatdata.join("pfx/drive_c/two.exe").display(),
            compatdata.join("pfx/drive_c/other.exe").display(),
        );
        fs::write(source.config_path(), global).unwrap();
        fs::write(source.library_path(), library).unwrap();
        fs::write(
            source.game_config_path("two"),
            format!(
                r#"{{"version":"v0.1","two":{{"winePrefix":"{}"}}}}"#,
                root.path().join("alias/pfx").display()
            ),
        )
        .unwrap();
        let other = wine_prefix(root.path().join("separate").as_path());
        fs::write(
            source.game_config_path("other"),
            format!(
                r#"{{"version":"v0.1","other":{{"winePrefix":"{}"}}}}"#,
                other.display()
            ),
        )
        .unwrap();
        assert_eq!(
            source.inspect("two").unwrap().prefix,
            root.path().join("alias")
        );
        assert_eq!(
            source.shared_prefix_candidates("one").unwrap(),
            vec![HeroicSharedPrefix {
                id: "two".to_owned(),
                title: "Two".to_owned(),
            }]
        );
    }

    #[test]
    fn disable_umu_reports_only_per_game_true_for_proton() {
        let (root, source) = fixture();
        let prefix = root.path().join("compatdata");
        fs::create_dir_all(prefix.join("pfx/drive_c")).unwrap();
        fs::write(prefix.join("pfx/system.reg"), "#arch=win64\n").unwrap();
        let global = format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"proton","name":"GE-Proton","bin":"/runners/GE-Proton/proton"}},"disableUMU":true}}}}"#,
            prefix.display()
        );
        let library = format!(
            r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
            prefix.join("pfx/drive_c/game.exe").display()
        );
        write_fixture(&source, &global, "", &library);
        assert!(
            !source
                .inspect("hades")
                .unwrap()
                .issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("disableUMU"))
        );
        fs::write(
            source.game_config_path("hades"),
            r#"{"version":"v0.1","hades":{"disableUMU":false}}"#,
        )
        .unwrap();
        assert!(
            !source
                .inspect("hades")
                .unwrap()
                .issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("disableUMU"))
        );
        fs::write(
            source.game_config_path("hades"),
            r#"{"version":"v0.1","hades":{"disableUMU":true}}"#,
        )
        .unwrap();
        let imported = source.inspect("hades").unwrap();
        assert!(
            imported
                .issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("disableUMU")
                    && issue.kind == HeroicIssueKind::Unsupported
                    && issue.message.contains("wineo uses umu"))
        );
        assert!(
            imported
                .settings_review
                .unsupported
                .iter()
                .any(|item| item.field == "disableUMU")
        );
    }

    #[test]
    fn wine_nvapi_warns_about_dll_requirement_without_claiming_absence() {
        let (root, source) = fixture();
        let prefix = wine_prefix(root.path());
        let global = format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}},"autoInstallDxvkNvapi":true,"disableUMU":true}}}}"#,
            prefix.display()
        );
        let library = format!(
            r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
            prefix.join("drive_c/game.exe").display()
        );
        write_fixture(
            &source,
            &global,
            r#"{"version":"v0.1","hades":{"disableUMU":true}}"#,
            &library,
        );
        let imported = source.inspect("hades").unwrap();
        assert_eq!(imported.env.get("DXVK_ENABLE_NVAPI"), Some(&"1".to_owned()));
        assert!(imported.issues.iter().any(|issue| issue.field.as_deref()
            == Some("autoInstallDxvkNvapi")
            && issue.message.contains("requires dxvk-nvapi DLLs")
            && !issue.message.contains("missing")));
        assert!(
            !imported
                .issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("disableUMU"))
        );
    }

    #[test]
    fn playtime_reads_total_minutes_and_last_played() {
        let (_root, source) = fixture();
        fs::create_dir_all(source.config_dir().join("store")).unwrap();
        fs::write(
            source.config_dir().join("store/timestamp.json"),
            r#"{"hades":{"firstPlayed":"2026-06-01T10:00:00.000Z","lastPlayed":"2026-09-20T19:02:11.000Z","totalPlayed":5720}}"#,
        )
        .unwrap();
        let playtime = source.playtime("hades").unwrap().unwrap();
        assert_eq!(playtime.seconds, 5720 * 60);
        assert_eq!(
            playtime.last_played,
            Some("2026-09-20T19:02:11Z".parse().unwrap())
        );
    }

    #[test]
    fn playtime_is_none_without_a_played_entry_or_file() {
        let (_root, source) = fixture();
        assert_eq!(source.playtime("hades").unwrap(), None);

        fs::create_dir_all(source.config_dir().join("store")).unwrap();
        fs::write(
            source.config_dir().join("store/timestamp.json"),
            r#"{"hades":{"firstPlayed":"","lastPlayed":"","totalPlayed":0},"other":{"firstPlayed":"2026-01-01T00:00:00.000Z","lastPlayed":"2026-01-02T00:00:00.000Z","totalPlayed":90}}"#,
        )
        .unwrap();
        assert_eq!(source.playtime("hades").unwrap(), None);
        assert_eq!(source.playtime("missing").unwrap(), None);
        let other = source.playtime("other").unwrap().unwrap();
        assert_eq!(other.seconds, 90 * 60);
        assert_eq!(
            other.last_played,
            Some("2026-01-02T00:00:00Z".parse().unwrap())
        );
    }
}
