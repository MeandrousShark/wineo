use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, value};

use crate::error::{Error, IoContext, Result};
use crate::name;
use crate::paths::Paths;

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
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub art: ArtConfig,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Art fetched while `new` and `import` create an environment.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtConfig {
    #[serde(default)]
    pub fetch: ArtFetch,
}

/// Which kinds `new` and `import` fetch. `art fetch` ignores this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtFetch {
    #[default]
    All,
    Icon,
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
}

#[derive(Debug)]
pub struct EnvironmentConfig {
    /// 0 for a legacy file without the key. Checked by `parse`.
    pub schema_version: u32,
    pub kind: Kind,
    pub gamemode: Option<bool>,
    /// The Heroic game id this environment was imported from. Only Heroic
    /// imports set this; older configs load fine without it.
    pub heroic_id: Option<String>,
    pub env: BTreeMap<String, String>,
    pub targets: BTreeMap<String, Target>,
}

/// What an environment runs on. A config without `kind` is Wine.
#[derive(Debug)]
pub enum Kind {
    Wine(WineConfig),
    Native,
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
    gameid: Option<String>,
    store: Option<String>,
    dxvk: Option<String>,
    vkd3d: Option<String>,
    heroic_id: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    targets: BTreeMap<String, Target>,
}

impl RawEnvironmentConfig {
    fn into_config(self, name: &str) -> Result<EnvironmentConfig, String> {
        let kind = match self.kind.unwrap_or(KindName::Wine) {
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
                    _ => unreachable!("not a Wine-only key"),
                };
                if let Some(key) = WINE_ONLY_KEYS.iter().find(|key| set(key)) {
                    return Err(native_key_message(name, key));
                }
                Kind::Native
            }
        };
        Ok(EnvironmentConfig {
            schema_version: self.schema_version,
            kind,
            gamemode: self.gamemode,
            heroic_id: self.heroic_id,
            env: self.env,
            targets: self.targets,
        })
    }
}

fn missing(key: &str) -> String {
    format!("missing field `{key}`")
}

/// Keys only a Wine environment can have.
pub const WINE_ONLY_KEYS: [&str; 7] = [
    "prefix", "runner", "arch", "gameid", "store", "dxvk", "vkd3d",
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
    /// Whether this target has a desktop entry. Reconciled, so setting it
    /// writes the entry and clearing it takes the entry away.
    #[serde(default)]
    pub desktop: bool,
    /// What the desktop entry is called. Defaults to the target's own name.
    pub name: Option<String>,
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
            defaults: Defaults::default(),
            art: ArtConfig::default(),
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
        }
    }
}

impl GlobalConfig {
    /// Loads the global config, treating a missing file as all defaults.
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = paths.global_config();
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&path, &text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).context(|| format!("failed to read {}", path.display())),
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
            Kind::Native => None,
        }
    }

    pub fn wine_mut(&mut self) -> Option<&mut WineConfig> {
        match &mut self.kind {
            Kind::Wine(wine) => Some(wine),
            Kind::Native => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        // Only wine reads WINEPREFIX, so a native program may have its own.
        match self.kind {
            Kind::Wine(_) => validate_env(&self.env)?,
            Kind::Native => validate_env_vars(&self.env)?,
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
    if env.contains_key("WINEPREFIX") {
        return Err("WINEPREFIX is managed by wineo and can't be set in [env]".to_owned());
    }
    validate_env_vars(env)
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
mod tests {
    use super::*;

    const PATH: &str = "/test/config.toml";

    fn global(text: &str) -> Result<GlobalConfig> {
        GlobalConfig::parse(Path::new(PATH), text)
    }

    fn environment(text: &str) -> Result<EnvironmentConfig> {
        EnvironmentConfig::parse(Path::new(PATH), text)
    }

    #[test]
    fn empty_global_config_uses_defaults() {
        let config = global("").unwrap();
        assert_eq!(config.stop_timeout, DEFAULT_STOP_TIMEOUT);
        assert_eq!(config.defaults.runner, "wine");
        assert_eq!(config.defaults.arch, Arch::Win64);
        assert!(config.defaults.prefix_root.is_none());
        assert!(config.env.is_empty());
    }

    #[test]
    fn full_global_config_parses() {
        let config = global(
            r#"
            stop_timeout = 30

            [defaults]
            runner = "wine"
            arch = "win32"
            prefix_root = "~/Games"

            [env]
            WINEDEBUG = "-all"
            "#,
        )
        .unwrap();
        assert_eq!(config.stop_timeout, 30);
        assert_eq!(config.defaults.arch, Arch::Win32);
        assert_eq!(config.defaults.prefix_root.as_deref(), Some("~/Games"));
        assert_eq!(config.env["WINEDEBUG"], "-all");
    }

    #[test]
    fn art_fetch_defaults_to_all() {
        assert_eq!(global("").unwrap().art.fetch, ArtFetch::All);
        assert_eq!(global("[art]").unwrap().art.fetch, ArtFetch::All);
    }

    #[test]
    fn art_fetch_can_narrow_to_the_icon() {
        let config = global("[art]\nfetch = \"icon\"").unwrap();
        assert_eq!(config.art.fetch, ArtFetch::Icon);
        assert_eq!(
            global("[art]\nfetch = \"all\"").unwrap().art.fetch,
            ArtFetch::All
        );
    }

    #[test]
    fn unknown_art_fetch_values_and_keys_are_errors() {
        assert!(matches!(
            global("[art]\nfetch = \"grid\""),
            Err(Error::ConfigSyntax { .. })
        ));
        assert!(matches!(
            global("[art]\nfecth = \"icon\""),
            Err(Error::ConfigSyntax { .. })
        ));
    }

    #[test]
    fn unknown_keys_are_errors() {
        assert!(matches!(
            global("stop_timeuot = 3"),
            Err(Error::ConfigSyntax { .. })
        ));
        assert!(matches!(
            environment("prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\nprefx = 1"),
            Err(Error::ConfigSyntax { .. })
        ));
    }

    #[test]
    fn environment_config_parses_targets() {
        let config = environment(
            r#"
            prefix = "~/Apps/Photoshop"
            runner = "wine"
            arch = "win64"

            [env]
            DXVK_HUD = "1"

            [targets.photoshop]
            exe = 'C:\Program Files\Adobe\Photoshop.exe'
            default = true

            [targets.bridge]
            exe = 'C:\Program Files\Adobe\Bridge.exe'
            args = ["-nosplash"]
            cwd = 'C:\users\Public'
            "#,
        )
        .unwrap();
        assert_eq!(config.wine().unwrap().prefix, "~/Apps/Photoshop");
        assert_eq!(config.env["DXVK_HUD"], "1");
        let photoshop = &config.targets["photoshop"];
        assert!(photoshop.default);
        assert!(photoshop.args.is_empty());
        let bridge = &config.targets["bridge"];
        assert_eq!(bridge.exe, r"C:\Program Files\Adobe\Bridge.exe");
        assert_eq!(bridge.args, ["-nosplash"]);
        assert_eq!(bridge.cwd.as_deref(), Some(r"C:\users\Public"));
        assert!(!bridge.default);
    }

    #[test]
    fn heroic_id_is_optional_and_round_trips() {
        let config = environment("prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"").unwrap();
        assert_eq!(config.heroic_id, None);
        let config = environment(
            "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\nheroic_id = \"abc123\"",
        )
        .unwrap();
        assert_eq!(config.heroic_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn environment_requires_prefix_runner_and_arch() {
        assert!(environment("runner = \"wine\"\narch = \"win64\"").is_err());
        assert!(environment("prefix = \"/p\"\narch = \"win64\"").is_err());
        assert!(environment("prefix = \"/p\"\nrunner = \"wine\"").is_err());
    }

    const NATIVE: &str = r#"
        schema_version = 1
        kind = "native"
        gamemode = true

        [env]
        SDL_VIDEODRIVER = "wayland"

        [targets.game]
        exe = "/usr/bin/dusklight"
        default = true
        desktop = true
        name = "Dusklight"
    "#;

    fn named(name: &str, text: &str) -> Result<EnvironmentConfig> {
        EnvironmentConfig::parse(Path::new(&format!("/test/{name}.toml")), text)
    }

    #[test]
    fn a_native_config_parses() {
        let config = environment(NATIVE).unwrap();
        assert!(matches!(config.kind, Kind::Native));
        assert!(config.wine().is_none());
        assert_eq!(config.gamemode, Some(true));
        assert_eq!(config.env["SDL_VIDEODRIVER"], "wayland");
        assert_eq!(config.targets["game"].exe, "/usr/bin/dusklight");
    }

    #[test]
    fn a_native_config_needs_no_targets() {
        assert!(matches!(
            environment("kind = \"native\"").unwrap().kind,
            Kind::Native
        ));
    }

    #[test]
    fn each_wine_only_key_is_refused_for_native() {
        for (key, value) in [
            ("prefix", "\"/p\""),
            ("runner", "\"wine\""),
            ("arch", "\"win64\""),
            ("gameid", "\"umu-1\""),
            ("store", "\"gog\""),
            ("dxvk", "\"latest\""),
            ("vkd3d", "\"latest\""),
        ] {
            let err = named(
                "dusklight",
                &format!("kind = \"native\"\n{key} = {value}\n"),
            )
            .unwrap_err();
            assert!(matches!(err, Error::ConfigInvalid { .. }), "{key}: {err}");
            assert_eq!(
                err.to_string(),
                format!(
                    "invalid config /test/dusklight.toml: native environment dusklight can't set {key}"
                )
            );
        }
    }

    #[test]
    fn a_config_without_kind_is_wine() {
        let config = environment(ENV).unwrap();
        let wine = config.wine().unwrap();
        assert_eq!(wine.prefix, "/p");
        assert_eq!(wine.runner, "wine");
        assert_eq!(wine.arch, Arch::Win64);
    }

    #[test]
    fn an_explicit_wine_kind_is_wine() {
        let config = environment(&format!("kind = \"wine\"\n{ENV}")).unwrap();
        assert_eq!(config.wine().unwrap().prefix, "/p");
    }

    #[test]
    fn an_explicit_wine_kind_still_needs_prefix_runner_and_arch() {
        let err = environment("kind = \"wine\"\nrunner = \"wine\"\narch = \"win64\"").unwrap_err();
        assert!(err.to_string().contains("missing field `prefix`"), "{err}");
    }

    #[test]
    fn an_unknown_kind_is_refused() {
        for kind in ["\"proton\"", "\"Native\"", "1"] {
            assert!(
                matches!(
                    environment(&format!("kind = {kind}\n{ENV}")),
                    Err(Error::ConfigSyntax { .. })
                ),
                "{kind}"
            );
        }
    }

    #[test]
    fn unknown_keys_are_refused_for_native_too() {
        assert!(matches!(
            environment("kind = \"native\"\nprefx = 1"),
            Err(Error::ConfigSyntax { .. })
        ));
    }

    #[test]
    fn wineprefix_in_env_is_rejected() {
        let err = global("[env]\nWINEPREFIX = \"/x\"").unwrap_err();
        assert!(err.to_string().contains("WINEPREFIX"), "{err}");
        let err = environment(
            "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n[env]\nWINEPREFIX = \"/x\"",
        )
        .unwrap_err();
        assert!(err.to_string().contains("WINEPREFIX"), "{err}");
    }

    #[test]
    fn wineprefix_in_env_is_allowed_for_native() {
        let config = environment("kind = \"native\"\n[env]\nWINEPREFIX = \"/x\"").unwrap();
        assert_eq!(config.env["WINEPREFIX"], "/x");
        assert!(environment("kind = \"native\"\n[env]\n\"A=B\" = \"1\"").is_err());
    }

    #[test]
    fn bad_env_names_and_values_are_rejected() {
        assert!(global("[env]\n\"A=B\" = \"1\"").is_err());
        assert!(global("[env]\n\"\" = \"1\"").is_err());
        assert!(global("[env]\nA = \"x\\u0000y\"").is_err());
    }

    #[test]
    fn several_default_targets_are_rejected() {
        let err = environment(
            r#"
            prefix = "/p"
            runner = "wine"
            arch = "win64"
            [targets.a]
            exe = "a"
            default = true
            [targets.b]
            exe = "b"
            default = true
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("only one target"), "{err}");
    }

    #[test]
    fn invalid_target_names_are_rejected() {
        let err = environment(
            "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n[targets.\"a b\"]\nexe = \"a\"",
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid target name"), "{err}");
    }

    const ENV: &str = "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n";

    #[test]
    fn a_missing_schema_version_is_legacy() {
        assert_eq!(global("").unwrap().schema_version, 0);
        assert_eq!(environment(ENV).unwrap().schema_version, 0);
    }

    #[test]
    fn schema_version_one_parses() {
        assert_eq!(global("schema_version = 1").unwrap().schema_version, 1);
        let config = environment(&format!("schema_version = 1\n{ENV}")).unwrap();
        assert_eq!(config.schema_version, 1);
    }

    #[test]
    fn unsupported_schema_versions_are_rejected() {
        for (value, found) in [
            ("0", "0"),
            ("2", "2"),
            ("-1", "-1"),
            ("\"1\"", "\"1\""),
            ("1.0", "1.0"),
            ("true", "true"),
        ] {
            let text = format!("schema_version = {value}\n");
            let err = global(&text).unwrap_err();
            assert!(matches!(err, Error::SchemaVersion { .. }), "{value}: {err}");
            assert_eq!(
                err.to_string(),
                format!("unsupported schema_version {found} in {PATH}")
            );
            let err = environment(&format!("{text}{ENV}")).unwrap_err();
            assert!(matches!(err, Error::SchemaVersion { .. }), "{value}: {err}");
        }
    }

    // A newer format may add keys, and the version is what should be reported.
    #[test]
    fn a_newer_version_is_reported_before_unknown_keys() {
        let err = environment(&format!("schema_version = 2\n{ENV}new_key = 1\n")).unwrap_err();
        assert!(matches!(err, Error::SchemaVersion { .. }), "{err}");
    }

    #[test]
    fn a_nested_schema_version_is_an_unknown_key() {
        assert!(matches!(
            global("[defaults]\nschema_version = 1"),
            Err(Error::ConfigSyntax { .. })
        ));
    }

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| std::ffi::OsString::from(home.as_os_str()))
        })
        .unwrap();
        (root, paths)
    }

    fn write_global(paths: &Paths, text: &str) {
        let path = paths.global_config();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn a_missing_global_config_stays_missing() {
        let (_root, paths) = temp_paths();
        assert_eq!(GlobalConfig::load(&paths).unwrap().schema_version, 0);
        assert_eq!(
            GlobalConfig::load_or_default(&paths)
                .unwrap()
                .schema_version,
            0
        );
        assert!(!paths.global_config().exists());
    }

    #[test]
    fn loading_a_legacy_global_config_leaves_it_alone() {
        let (_root, paths) = temp_paths();
        let text = "# mine\nstop_timeout = 3\n";
        write_global(&paths, text);
        let path = paths.global_config();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(GlobalConfig::load(&paths).unwrap().stop_timeout, 3);
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn load_or_default_only_forgives_a_broken_file() {
        let (_root, paths) = temp_paths();
        write_global(&paths, "stop_timeout = [\n");
        let config = GlobalConfig::load_or_default(&paths).unwrap();
        assert_eq!(config.stop_timeout, DEFAULT_STOP_TIMEOUT);

        for version in ["0", "2", "\"1\""] {
            write_global(
                &paths,
                &format!("schema_version = {version}\nstop_timeout = 3\n"),
            );
            assert!(matches!(
                GlobalConfig::load_or_default(&paths),
                Err(Error::SchemaVersion { .. })
            ));
        }
    }

    #[test]
    fn stamping_keeps_a_leading_comment_on_top() {
        let text = "# mine\nstop_timeout = 3 # short\n\n[defaults]\nrunner = \"x\"\n";
        let mut doc: DocumentMut = text.parse().unwrap();
        stamp_schema(&mut doc);
        assert_eq!(
            doc.to_string(),
            "# mine\nstop_timeout = 3 # short\nschema_version = 1\n\n[defaults]\nrunner = \"x\"\n"
        );
        let stamped = doc.to_string();
        stamp_schema(&mut doc);
        assert_eq!(doc.to_string(), stamped);
    }

    #[test]
    fn a_new_document_starts_with_the_version() {
        let mut doc = new_document();
        doc["prefix"] = value("/p");
        assert_eq!(doc.to_string(), "schema_version = 1\nprefix = \"/p\"\n");
    }

    const EXAMPLE: &str = include_str!("../../../docs/environment.toml.example");

    /// A commented-out setting is a line `# key = value` or `# [table]`; a
    /// prose comment never starts a key right after `# `, so this tells the
    /// two apart without having to track table context.
    fn is_example_setting(line: &str) -> bool {
        let Some(rest) = line.strip_prefix("# ") else {
            return false;
        };
        if rest.starts_with('[') {
            return true;
        }
        let key_end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
            .unwrap_or(rest.len());
        key_end > 0 && rest[key_end..].trim_start().starts_with('=')
    }

    /// Uncomments every setting line in the example, leaving prose comments
    /// and blank lines alone.
    fn uncommented_example() -> String {
        EXAMPLE
            .lines()
            .map(|line| {
                if is_example_setting(line) {
                    &line[2..]
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn example_config_parses_as_shipped() {
        let config = environment(EXAMPLE).unwrap();
        assert_eq!(config.schema_version, 1);
        assert_eq!(config.wine().unwrap().prefix, "[PREFIX-DIR]");
        assert_eq!(config.wine().unwrap().runner, "GE-Proton-Latest");
        assert_eq!(config.wine().unwrap().arch, Arch::Win64);
        assert!(config.gamemode.is_none());
        assert!(config.wine().unwrap().gameid.is_none());
        assert!(config.wine().unwrap().store.is_none());
        assert!(config.wine().unwrap().dxvk.is_none());
        assert!(config.wine().unwrap().vkd3d.is_none());
        assert!(config.heroic_id.is_none());
        assert!(config.env.is_empty());
        assert!(config.targets.is_empty());
    }

    /// Every optional field of `EnvironmentConfig` and `Target` must have an
    /// example line, so a new field with no line here fails this test.
    #[test]
    fn example_config_parses_with_every_setting_uncommented() {
        let text = uncommented_example();
        let config = environment(&text)
            .unwrap_or_else(|err| panic!("uncommented example did not parse: {err}\n---\n{text}"));

        assert!(config.gamemode.unwrap());
        assert!(config.wine().unwrap().gameid.is_some());
        assert!(config.wine().unwrap().store.is_some());
        assert!(config.wine().unwrap().dxvk.is_some());
        assert!(config.wine().unwrap().vkd3d.is_some());
        assert!(config.heroic_id.is_some());
        assert!(!config.env.is_empty());
        assert_eq!(config.env["DXVK_HUD"], "1");

        let game = &config.targets["game"];
        assert!(!game.exe.is_empty());
        assert!(!game.args.is_empty());
        assert!(game.cwd.is_some());
        assert!(game.default);
        assert!(game.gamemode.unwrap());
        assert!(game.desktop);
        assert!(game.name.is_some());

        let launcher = &config.targets["launcher"];
        assert!(!launcher.exe.is_empty());
    }

    #[test]
    fn arch_round_trips_through_strings() {
        assert_eq!("win64".parse::<Arch>().unwrap(), Arch::Win64);
        assert_eq!("win32".parse::<Arch>().unwrap(), Arch::Win32);
        assert!("x86".parse::<Arch>().is_err());
        assert_eq!(Arch::Win32.to_string(), "win32");
    }
}
