use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::error::{Error, IoContext, NoPrefix, PrefixOwner, Result};
use crate::files;
pub use crate::gamescope::GamescopeSetting;
use crate::name;
use crate::paths::Paths;
use crate::winetricks;

pub const DEFAULT_RUNNER: &str = "wine";
pub const DEFAULT_STOP_TIMEOUT: u32 = 10;
/// Megabytes of session log kept before it is cleared. 0 turns the cap off.
pub const DEFAULT_LOG_LIMIT_MB: u32 = 50;
/// The format version wineo writes into global, environment, and template files.
/// A file without the key is legacy version 0 and reads the same as version 1.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    #[default]
    Win64,
    Win32,
}

impl Arch {
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::Win64 => "win64",
            Arch::Win32 => "win32",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Arch {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "win64" => Ok(Arch::Win64),
            "win32" => Ok(Arch::Win32),
            _ => Err(format!("expected win64 or win32, got `{s}`")),
        }
    }
}

/// `~/.config/wineo/config.toml`. Every key is optional.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    /// 0 for a legacy file without the key. Checked by `parse`.
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default = "default_stop_timeout")]
    pub stop_timeout: u32,
    #[serde(default = "default_log_limit_mb")]
    pub log_limit_mb: u32,
    /// Unset everywhere means off, so this stays an option all the way down.
    pub gamemode: Option<bool>,
    pub wayland: Option<bool>,
    pub mangohud: Option<bool>,
    /// Whether builds from Steam, Heroic, and Lutris count as runners too.
    /// Unset means nobody has been asked yet, which is why it isn't a bool.
    pub external_runners: Option<bool>,
    /// Which Steam install games use when both native and Flatpak exist.
    pub steam: Option<crate::steam::InstallKind>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub art: ArtConfig,
    #[serde(default)]
    pub gog: GogConfig,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// GOG settings.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GogConfig {
    /// A BCP 47 locale tag, like `en-US`. Unset means
    /// `gog::language::DEFAULT`.
    pub language: Option<String>,
}

/// Art fetched while `new` and `import` create an environment.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtConfig {
    #[serde(default)]
    pub fetch: ArtFetch,
}

/// Which kinds `new` and `import` fetch. `art fetch` ignores this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtFetch {
    #[default]
    All,
    Icon,
    /// Enabled kinds in the order of `art::Kind::ALL`.
    Selected([bool; 4]),
}

impl ArtFetch {
    pub fn kinds(self) -> Vec<crate::art::Kind> {
        use crate::art::Kind;
        match self {
            Self::All => Kind::ALL.to_vec(),
            Self::Icon => vec![Kind::Icon],
            Self::Selected(selected) => Kind::ALL
                .into_iter()
                .zip(selected)
                .filter_map(|(kind, on)| on.then_some(kind))
                .collect(),
        }
    }

    pub fn with(self, kind: crate::art::Kind, on: bool) -> Self {
        let kinds = self.kinds();
        let selected = crate::art::Kind::ALL.map(|candidate| {
            if candidate == kind {
                on
            } else {
                kinds.contains(&candidate)
            }
        });
        Self::selected(selected)
    }

    fn selected(selected: [bool; 4]) -> Self {
        match selected {
            [true, true, true, true] => Self::All,
            [true, false, false, false] => Self::Icon,
            _ => Self::Selected(selected),
        }
    }
}

impl FromStr for ArtFetch {
    type Err = String;

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        match raw {
            "all" => return Ok(Self::All),
            "icon" => return Ok(Self::Icon),
            "none" => return Ok(Self::Selected([false; 4])),
            _ => {}
        }
        let mut selected = [false; 4];
        for name in raw.split(',') {
            let kind = name.trim().parse::<crate::art::Kind>()?;
            let index = crate::art::Kind::ALL
                .iter()
                .position(|candidate| *candidate == kind)
                .unwrap();
            if selected[index] {
                return Err(format!("duplicate art kind: {name}"));
            }
            selected[index] = true;
        }
        Ok(Self::selected(selected))
    }
}

impl fmt::Display for ArtFetch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::Icon => f.write_str("icon"),
            Self::Selected(_) => {
                let kinds = self.kinds();
                if kinds.is_empty() {
                    return f.write_str("none");
                }
                f.write_str(
                    &kinds
                        .iter()
                        .map(|kind| kind.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                )
            }
        }
    }
}

impl<'de> Deserialize<'de> for ArtFetch {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Input {
            Legacy(String),
            Kinds(Vec<String>),
        }
        let input = Input::deserialize(deserializer)?;
        let raw = match input {
            Input::Legacy(raw) if matches!(raw.as_str(), "all" | "icon" | "none") => raw,
            Input::Legacy(raw) => {
                return Err(serde::de::Error::custom(format!(
                    "expected all, icon, none, or an array of art kinds, got {raw}"
                )));
            }
            Input::Kinds(kinds) if kinds.is_empty() => "none".to_owned(),
            Input::Kinds(kinds) => {
                for kind in &kinds {
                    kind.parse::<crate::art::Kind>()
                        .map_err(serde::de::Error::custom)?;
                }
                kinds.join(",")
            }
        };
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// Values `wineo new` falls back to.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default = "default_runner")]
    pub runner: String,
    #[serde(default)]
    pub arch: Arch,
    pub prefix_root: Option<String>,
    pub runner_root: Option<String>,
    pub dxvk_root: Option<String>,
    pub vkd3d_root: Option<String>,
    /// Where GOG installs put new games. Unset means `~/Games`.
    pub games_root: Option<String>,
}

#[derive(Debug)]
pub struct EnvironmentConfig {
    /// 0 for a legacy file without the key. Checked by `parse`.
    pub schema_version: u32,
    pub kind: Kind,
    pub gamemode: Option<bool>,
    /// Always None for a native environment, which has no Wine to switch.
    pub wayland: Option<bool>,
    pub mangohud: Option<bool>,
    pub gamescope: Option<GamescopeSetting>,
    pub hooks: Option<HooksSetting>,
    /// The Heroic game id this environment was imported from. Only Heroic
    /// imports set this; older configs load fine without it.
    pub heroic_id: Option<String>,
    /// The GOG product id of the game this environment owns. Its install
    /// state lives in the game folder's state file, not here.
    pub gog_id: Option<String>,
    pub tags: Vec<String>,
    pub winetricks: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub targets: BTreeMap<String, Target>,
}

/// What an environment runs on. A config without `kind` is Wine.
#[derive(Debug)]
pub enum Kind {
    Wine(WineConfig),
    Native,
    /// A game another launcher owns, along with its prefix and its process.
    Proxied(ProxiedConfig),
}

impl Kind {
    /// The `kind` value as written in a config.
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Wine(_) => "wine",
            Kind::Native => "native",
            Kind::Proxied(_) => "proxied",
        }
    }
}

/// The settings only a proxied environment has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxiedConfig {
    pub launcher: Launcher,
    /// A string, since not every store's ids are numbers.
    pub app_id: String,
    /// Shown when the launcher no longer knows the game.
    pub title: Option<String>,
    /// Whether the environment has a desktop entry.
    pub desktop: bool,
}

/// Who launches a proxied environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Launcher {
    Steam,
}

impl Launcher {
    /// The `launcher` value as written in a config.
    pub fn as_str(self) -> &'static str {
        match self {
            Launcher::Steam => "steam",
        }
    }

    /// The launcher's name in messages.
    pub fn display_name(self) -> &'static str {
        match self {
            Launcher::Steam => "Steam",
        }
    }
}

/// The settings only a Wine or Proton environment has.
#[derive(Debug)]
pub struct WineConfig {
    pub prefix: String,
    pub runner: String,
    pub arch: Arch,
    /// The umu id umu looks protonfixes up by. Unset means no fixes.
    pub gameid: Option<String>,
    /// The store the game came from, which some protonfixes branch on.
    pub store: Option<String>,
    /// The DXVK and vkd3d builds this prefix should have. `latest` means the
    /// newest installed, resolved on use rather than written back.
    pub dxvk: Option<String>,
    pub vkd3d: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum KindName {
    Wine,
    Native,
    Proxied,
}

/// The file as written, before the kind decides which keys it may have.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnvironmentConfig {
    #[serde(default)]
    schema_version: u32,
    kind: Option<KindName>,
    prefix: Option<String>,
    runner: Option<String>,
    arch: Option<Arch>,
    gamemode: Option<bool>,
    wayland: Option<bool>,
    mangohud: Option<bool>,
    gamescope: Option<GamescopeSetting>,
    hooks: Option<HooksSetting>,
    gameid: Option<String>,
    store: Option<String>,
    dxvk: Option<String>,
    vkd3d: Option<String>,
    heroic_id: Option<String>,
    gog_id: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    winetricks: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
    targets: Option<BTreeMap<String, Target>>,
    launcher: Option<Launcher>,
    app_id: Option<String>,
    title: Option<String>,
    desktop: Option<bool>,
}

impl RawEnvironmentConfig {
    fn into_config(self, name: &str) -> Result<EnvironmentConfig, String> {
        let kind_name = self.kind.unwrap_or(KindName::Wine);
        if kind_name != KindName::Proxied
            && let Some(key) = PROXIED_ONLY_KEYS.iter().find(|key| self.has(key))
        {
            return Err(proxied_only_key_message(name, key));
        }
        if kind_name != KindName::Proxied
            && let Some(gog_id) = &self.gog_id
        {
            validate_gog_id(gog_id)?;
        }
        let kind = match kind_name {
            KindName::Wine => Kind::Wine(WineConfig {
                prefix: self.prefix.ok_or_else(|| missing("prefix"))?,
                runner: self.runner.ok_or_else(|| missing("runner"))?,
                arch: self.arch.ok_or_else(|| missing("arch"))?,
                gameid: self.gameid,
                store: self.store,
                dxvk: self.dxvk,
                vkd3d: self.vkd3d,
            }),
            KindName::Native => {
                let set = |key: &str| match key {
                    "prefix" => self.prefix.is_some(),
                    "runner" => self.runner.is_some(),
                    "arch" => self.arch.is_some(),
                    "gameid" => self.gameid.is_some(),
                    "store" => self.store.is_some(),
                    "dxvk" => self.dxvk.is_some(),
                    "vkd3d" => self.vkd3d.is_some(),
                    "wayland" => self.wayland.is_some(),
                    "winetricks" => self.winetricks.is_some(),
                    _ => unreachable!("not a Wine-only key"),
                };
                if let Some(key) = WINE_ONLY_KEYS.iter().find(|key| set(key)) {
                    return Err(native_key_message(name, key));
                }
                if let Some(target) = self
                    .targets
                    .iter()
                    .flatten()
                    .find_map(|(target, config)| config.wayland.is_some().then_some(target))
                {
                    return Err(native_key_message(
                        name,
                        &format!("targets.{target}.wayland"),
                    ));
                }
                Kind::Native
            }
            KindName::Proxied => {
                let launcher = self.launcher.ok_or_else(|| missing("launcher"))?;
                if let Some(key) = PROXIED_REFUSED_KEYS.iter().find(|key| self.has(key)) {
                    return Err(proxied_key_message(name, launcher, key));
                }
                let app_id = self.app_id.clone().ok_or_else(|| missing("app_id"))?;
                validate_app_id(&app_id)?;
                Kind::Proxied(ProxiedConfig {
                    launcher,
                    app_id,
                    title: self.title.clone(),
                    desktop: self.desktop.unwrap_or(false),
                })
            }
        };
        Ok(EnvironmentConfig {
            schema_version: self.schema_version,
            kind,
            gamemode: self.gamemode,
            wayland: self.wayland,
            mangohud: self.mangohud,
            gamescope: self.gamescope,
            hooks: self.hooks,
            heroic_id: self.heroic_id,
            gog_id: self.gog_id,
            tags: self.tags,
            winetricks: self.winetricks.unwrap_or_default(),
            env: self.env.unwrap_or_default(),
            targets: self.targets.unwrap_or_default(),
        })
    }

    /// Whether the file sets `key`, for the keys one kind refuses.
    fn has(&self, key: &str) -> bool {
        match key {
            "prefix" => self.prefix.is_some(),
            "runner" => self.runner.is_some(),
            "arch" => self.arch.is_some(),
            "gamemode" => self.gamemode.is_some(),
            "wayland" => self.wayland.is_some(),
            "mangohud" => self.mangohud.is_some(),
            "gamescope" => self.gamescope.is_some(),
            "gameid" => self.gameid.is_some(),
            "store" => self.store.is_some(),
            "dxvk" => self.dxvk.is_some(),
            "vkd3d" => self.vkd3d.is_some(),
            "heroic_id" => self.heroic_id.is_some(),
            "gog_id" => self.gog_id.is_some(),
            "winetricks" => self.winetricks.is_some(),
            "env" => self.env.is_some(),
            "targets" => self.targets.is_some(),
            "launcher" => self.launcher.is_some(),
            "app_id" => self.app_id.is_some(),
            "title" => self.title.is_some(),
            "desktop" => self.desktop.is_some(),
            _ => unreachable!("not a kind-specific key: {key}"),
        }
    }
}

fn missing(key: &str) -> String {
    format!("missing field `{key}`")
}

fn validate_app_id(app_id: &str) -> Result<(), String> {
    if app_id.is_empty() {
        return Err("Invalid app_id: empty.".to_owned());
    }
    if app_id.contains('\0') {
        return Err("Invalid app_id: contains a NUL byte.".to_owned());
    }
    Ok(())
}

/// A GOG product id: a number that fits in 64 bits, with no leading zeros.
pub fn validate_gog_id(gog_id: &str) -> Result<(), String> {
    if gog_id.is_empty() {
        return Err("Invalid gog_id: empty.".to_owned());
    }
    if !gog_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("Invalid gog_id: {gog_id} isn't a number."));
    }
    // Written exactly as the number, so one game has one spelling.
    if gog_id.parse::<u64>().map(|id| id.to_string()).as_deref() != Ok(gog_id) {
        return Err(format!("Invalid gog_id: {gog_id} isn't a GOG id."));
    }
    Ok(())
}

/// Keys only a proxied environment can have.
pub const PROXIED_ONLY_KEYS: [&str; 4] = ["launcher", "app_id", "title", "desktop"];

/// Keys a proxied environment can't have, since its launcher owns the prefix
/// and the launch. Everything but `schema_version`, `kind`, `tags`, `hooks`,
/// and the proxied-only keys.
pub const PROXIED_REFUSED_KEYS: [&str; 16] = [
    "prefix",
    "runner",
    "arch",
    "gameid",
    "store",
    "dxvk",
    "vkd3d",
    "wayland",
    "winetricks",
    "gamemode",
    "mangohud",
    "gamescope",
    "heroic_id",
    "gog_id",
    "env",
    "targets",
];

/// The refusal for a key a proxied environment can't have.
pub fn proxied_key_message(name: &str, launcher: Launcher, key: &str) -> String {
    format!(
        "{name} is a {} game, so it can't set {key}.",
        launcher.display_name()
    )
}

/// The refusal for a proxied-only key on a Wine or native environment.
pub fn proxied_only_key_message(name: &str, key: &str) -> String {
    format!("{name} isn't a Steam game, so it can't set {key}.")
}

/// Keys only a Wine environment can have.
pub const WINE_ONLY_KEYS: [&str; 9] = [
    "prefix",
    "runner",
    "arch",
    "gameid",
    "store",
    "dxvk",
    "vkd3d",
    "wayland",
    "winetricks",
];

/// The refusal for a Wine-only key on a native environment.
pub fn native_key_message(name: &str, key: &str) -> String {
    format!("native environment {name} can't set {key}")
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub exe: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub default: bool,
    pub gamemode: Option<bool>,
    pub wayland: Option<bool>,
    pub mangohud: Option<bool>,
    /// Replaces the environment's whole, so `false` turns it off here.
    pub gamescope: Option<GamescopeSetting>,
    pub hooks: Option<HooksSetting>,
    /// Whether this target has a desktop entry. Reconciled, so setting it
    /// writes the entry and clearing it takes the entry away.
    #[serde(default)]
    pub desktop: bool,
    /// What the desktop entry is called. Defaults to the target's own name.
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HooksSetting {
    Off,
    On(Hooks),
}

impl HooksSetting {
    pub fn on(&self) -> Option<&Hooks> {
        match self {
            Self::Off => None,
            Self::On(hooks) => Some(hooks),
        }
    }

    pub fn to_item(&self) -> Item {
        match self {
            Self::Off => value(false),
            Self::On(hooks) => {
                let mut table = Table::new();
                for (key, command) in [("before", &hooks.before), ("after", &hooks.after)] {
                    if let Some(command) = command {
                        table[key] = value(command.as_str());
                    }
                }
                Item::Table(table)
            }
        }
    }
}

impl<'de> Deserialize<'de> for HooksSetting {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{self, MapAccess, Visitor};

        struct SettingVisitor;

        impl<'de> Visitor<'de> for SettingVisitor {
            type Value = HooksSetting;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a [hooks] table or false")
            }

            fn visit_bool<E: de::Error>(self, on: bool) -> Result<Self::Value, E> {
                if on {
                    Err(E::custom("hooks takes a [hooks] table or false, not true"))
                } else {
                    Ok(HooksSetting::Off)
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                Hooks::deserialize(de::value::MapAccessDeserializer::new(map)).map(HooksSetting::On)
            }
        }

        deserializer.deserialize_any(SettingVisitor)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawHooks")]
pub struct Hooks {
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHooks {
    before: Option<String>,
    after: Option<String>,
}

impl TryFrom<RawHooks> for Hooks {
    type Error = String;

    fn try_from(raw: RawHooks) -> Result<Self, Self::Error> {
        for (key, command) in [("before", &raw.before), ("after", &raw.after)] {
            if command
                .as_ref()
                .is_some_and(|command| command.trim().is_empty())
            {
                return Err(format!("Invalid hooks.{key}: empty command."));
            }
        }
        Ok(Self {
            before: raw.before,
            after: raw.after,
        })
    }
}

fn default_stop_timeout() -> u32 {
    DEFAULT_STOP_TIMEOUT
}

fn default_log_limit_mb() -> u32 {
    DEFAULT_LOG_LIMIT_MB
}

fn default_runner() -> String {
    DEFAULT_RUNNER.to_owned()
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            schema_version: 0,
            stop_timeout: DEFAULT_STOP_TIMEOUT,
            log_limit_mb: DEFAULT_LOG_LIMIT_MB,
            gamemode: None,
            wayland: None,
            mangohud: None,
            external_runners: None,
            steam: None,
            defaults: Defaults::default(),
            art: ArtConfig::default(),
            gog: GogConfig::default(),
            env: BTreeMap::new(),
        }
    }
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            runner: default_runner(),
            arch: Arch::default(),
            prefix_root: None,
            runner_root: None,
            dxvk_root: None,
            vkd3d_root: None,
            games_root: None,
        }
    }
}

impl GlobalConfig {
    /// Loads the global config, treating a missing file as all defaults.
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = paths.global_config();
        #[cfg(test)]
        crate::read_log::note(&path);
        match files::read_to_string_optional(&path)
            .context(|| format!("failed to read {}", path.display()))?
        {
            Some(text) => Self::parse(&path, &text),
            None => Ok(Self::default()),
        }
    }

    /// Like `load`, but a broken file means defaults. A file in a format this
    /// wineo doesn't know is still an error, so nothing acts on guessed settings.
    pub fn load_or_default(paths: &Paths) -> Result<Self> {
        match Self::load(paths) {
            Err(err @ Error::SchemaVersion { .. }) => Err(err),
            result => Ok(result.unwrap_or_default()),
        }
    }

    /// `path` is only used for error messages.
    pub fn parse(path: &Path, text: &str) -> Result<Self> {
        check_schema(path, text)?;
        let config: Self = parse_toml(path, text)?;
        validate_env(&config.env).map_err(|message| invalid(path, message))?;
        if let Some(language) = &config.gog.language {
            crate::gog::language::validate(language).map_err(|message| invalid(path, message))?;
        }
        Ok(config)
    }

    pub fn prefix_root(&self, paths: &Paths) -> Result<std::path::PathBuf> {
        match &self.defaults.prefix_root {
            Some(root) => paths.config_path(root),
            None => Ok(paths.default_prefix_root()),
        }
    }

    pub fn runner_root(&self, paths: &Paths) -> Result<std::path::PathBuf> {
        match &self.defaults.runner_root {
            Some(root) => paths.config_path(root),
            None => Ok(paths.default_runner_root()),
        }
    }

    /// Where new GOG games get their folders. GOG commands refuse a portable
    /// root before they get here, so this never lands in one.
    pub fn games_root(&self, paths: &Paths) -> Result<std::path::PathBuf> {
        match &self.defaults.games_root {
            Some(root) => paths.config_path(root),
            None => Ok(paths.default_games_root()),
        }
    }

    /// The locale tag GOG installs are asked for, as written.
    pub fn gog_language(&self) -> &str {
        self.gog
            .language
            .as_deref()
            .unwrap_or(crate::gog::language::DEFAULT)
    }

    /// GOG's installer language code for `gog_language`.
    #[cfg(test)]
    pub fn gog_installer_language(&self) -> String {
        crate::gog::language::installer_code(self.gog_language())
            .expect("a loaded config's language was validated")
    }
}

impl EnvironmentConfig {
    /// `path` is only used for error messages.
    pub fn parse(path: &Path, text: &str) -> Result<Self> {
        check_schema(path, text)?;
        let raw: RawEnvironmentConfig = parse_toml(path, text)?;
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let config = raw
            .into_config(&name)
            .map_err(|message| invalid(path, message))?;
        config
            .validate()
            .map_err(|message| invalid(path, message))?;
        Ok(config)
    }

    pub fn wine(&self) -> Option<&WineConfig> {
        match &self.kind {
            Kind::Wine(wine) => Some(wine),
            Kind::Native | Kind::Proxied(_) => None,
        }
    }

    pub fn proxied(&self) -> Option<&ProxiedConfig> {
        match &self.kind {
            Kind::Proxied(proxied) => Some(proxied),
            Kind::Wine(_) | Kind::Native => None,
        }
    }

    pub fn is_native(&self) -> bool {
        matches!(self.kind, Kind::Native)
    }

    /// The Wine settings, or the error saying why `name` has no prefix.
    pub fn require_wine(&self, name: &str) -> Result<&WineConfig> {
        match &self.kind {
            Kind::Wine(wine) => Ok(wine),
            Kind::Native | Kind::Proxied(_) => Err(self.no_prefix(name)),
        }
    }

    /// `require_wine` for changing the Wine settings.
    pub fn require_wine_mut(&mut self, name: &str) -> Result<&mut WineConfig> {
        let error = self.no_prefix(name);
        match &mut self.kind {
            Kind::Wine(wine) => Ok(wine),
            Kind::Native | Kind::Proxied(_) => Err(error),
        }
    }

    /// The error for a prefix operation on an environment that has none of
    /// its own. Only meaningful for a native or proxied environment.
    pub fn no_prefix(&self, name: &str) -> Error {
        let owner = match &self.kind {
            Kind::Proxied(proxied) => PrefixOwner::Launcher(proxied.launcher),
            Kind::Wine(_) | Kind::Native => PrefixOwner::None,
        };
        Error::NoPrefix(NoPrefix {
            name: name.to_owned(),
            owner,
        })
    }

    fn validate(&self) -> Result<(), String> {
        validate_tags(&self.tags)?;
        validate_winetricks(&self.winetricks)?;
        // Only wine reads WINEPREFIX, so a native program may have its own.
        match self.kind {
            Kind::Wine(_) => validate_env(&self.env)?,
            Kind::Native => validate_env_vars(&self.env)?,
            // Refused at parse time, so always empty.
            Kind::Proxied(_) => {}
        }
        if let Some(bad) = self.targets.keys().find(|name| !name::is_valid(name)) {
            return Err(format!(
                "invalid target name `{bad}`: use letters, digits, `-`, and `_`"
            ));
        }
        let defaults: Vec<&str> = self
            .targets
            .iter()
            .filter(|(_, target)| target.default)
            .map(|(name, _)| name.as_str())
            .collect();
        if defaults.len() > 1 {
            return Err(format!(
                "only one target can be the default, found {}",
                defaults.join(", ")
            ));
        }
        Ok(())
    }
}

pub fn validate_tags(tags: &[String]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for tag in tags {
        if let Some(reason) = name::problem(tag) {
            return Err(format!("Invalid tag {tag:?}: {reason}."));
        }
        if !seen.insert(tag) {
            return Err(format!("Duplicate tag {tag:?}."));
        }
    }
    Ok(())
}

pub fn validate_winetricks(verbs: &[String]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for verb in verbs {
        winetricks::validate_verb(verb).map_err(|err| err.to_string())?;
        if !seen.insert(verb) {
            return Err(format!("Duplicate winetricks verb {verb:?}."));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct SchemaProbe {
    schema_version: Option<toml::Value>,
}

/// Checks `schema_version` before anything else is read, so a file from a
/// newer wineo is reported as such rather than as unknown keys.
pub(crate) fn check_schema(path: &Path, text: &str) -> Result<()> {
    let probe: SchemaProbe = parse_toml(path, text)?;
    match probe.schema_version {
        None => Ok(()),
        Some(toml::Value::Integer(version)) if version == i64::from(SCHEMA_VERSION) => Ok(()),
        Some(found) => Err(Error::SchemaVersion {
            path: path.to_owned(),
            found: found.to_string(),
        }),
    }
}

/// An empty config document for a new file, with the version on its first line.
pub(crate) fn new_document() -> DocumentMut {
    let mut doc = DocumentMut::new();
    doc["schema_version"] = value(i64::from(SCHEMA_VERSION));
    doc
}

/// Marks a document wineo changed as the current version. A legacy file gets
/// the key after its other top-level values, so a leading comment stays on top.
pub(crate) fn stamp_schema(doc: &mut DocumentMut) {
    if !doc.contains_key("schema_version") {
        doc["schema_version"] = value(i64::from(SCHEMA_VERSION));
    }
}

pub(crate) fn parse_toml<T: serde::de::DeserializeOwned>(path: &Path, text: &str) -> Result<T> {
    toml::from_str(text).map_err(|source| Error::ConfigSyntax {
        path: path.to_owned(),
        source: Box::new(source),
    })
}

pub(crate) fn invalid(path: &Path, message: String) -> Error {
    Error::ConfigInvalid {
        path: path.to_owned(),
        message,
    }
}

fn validate_env(env: &BTreeMap<String, String>) -> Result<(), String> {
    // Reported ahead of any other bad name, whatever order the keys sort in.
    if env.contains_key("WINEPREFIX") {
        validate_env_key("WINEPREFIX", true)?;
    }
    validate_env_vars(env)
}

/// Checks a name for an environment's `[env]` table, so an editor can refuse
/// it before a save would. Only wine reads `WINEPREFIX`, so a native
/// environment may set it.
pub fn validate_env_key(key: &str, wine: bool) -> Result<(), String> {
    if wine && key == "WINEPREFIX" {
        return Err("WINEPREFIX is managed by wineo and can't be set in [env]".to_owned());
    }
    validate_env_var(key, "")
}

fn validate_env_vars(env: &BTreeMap<String, String>) -> Result<(), String> {
    for (key, value) in env {
        validate_env_var(key, value)?;
    }
    Ok(())
}

/// Checks that a variable can actually be passed to a process.
pub fn validate_env_var(key: &str, value: &str) -> Result<(), String> {
    if key.is_empty() || key.contains(['=', '\0']) {
        return Err(format!("`{key}` is not a valid environment variable name"));
    }
    if value.contains('\0') {
        return Err(format!("the value of `{key}` contains a NUL byte"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
