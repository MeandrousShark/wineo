use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::name;
use crate::paths::Paths;

pub const DEFAULT_RUNNER: &str = "wine";
pub const DEFAULT_STOP_TIMEOUT: u32 = 10;

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
    #[serde(default = "default_stop_timeout")]
    pub stop_timeout: u32,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentConfig {
    pub prefix: String,
    pub runner: String,
    pub arch: Arch,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub exe: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub default: bool,
}

fn default_stop_timeout() -> u32 {
    DEFAULT_STOP_TIMEOUT
}

fn default_runner() -> String {
    DEFAULT_RUNNER.to_owned()
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            stop_timeout: DEFAULT_STOP_TIMEOUT,
            defaults: Defaults::default(),
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

    /// `path` is only used for error messages.
    pub fn parse(path: &Path, text: &str) -> Result<Self> {
        let config: Self = parse_toml(path, text)?;
        validate_env(&config.env).map_err(|message| invalid(path, message))?;
        Ok(config)
    }

    pub fn prefix_root(&self, paths: &Paths) -> Result<std::path::PathBuf> {
        match &self.defaults.prefix_root {
            Some(root) => paths.absolute(root),
            None => Ok(paths.default_prefix_root()),
        }
    }
}

impl EnvironmentConfig {
    /// `path` is only used for error messages.
    pub fn parse(path: &Path, text: &str) -> Result<Self> {
        let config: Self = parse_toml(path, text)?;
        config
            .validate()
            .map_err(|message| invalid(path, message))?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        validate_env(&self.env)?;
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

fn parse_toml<T: serde::de::DeserializeOwned>(path: &Path, text: &str) -> Result<T> {
    toml::from_str(text).map_err(|source| Error::ConfigSyntax {
        path: path.to_owned(),
        source: Box::new(source),
    })
}

fn invalid(path: &Path, message: String) -> Error {
    Error::ConfigInvalid {
        path: path.to_owned(),
        message,
    }
}

fn validate_env(env: &BTreeMap<String, String>) -> Result<(), String> {
    for (key, value) in env {
        if key == "WINEPREFIX" {
            return Err("WINEPREFIX is managed by wineo and can't be set in [env]".to_owned());
        }
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
        assert_eq!(config.prefix, "~/Apps/Photoshop");
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
    fn environment_requires_prefix_runner_and_arch() {
        assert!(environment("runner = \"wine\"\narch = \"win64\"").is_err());
        assert!(environment("prefix = \"/p\"\narch = \"win64\"").is_err());
        assert!(environment("prefix = \"/p\"\nrunner = \"wine\"").is_err());
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

    #[test]
    fn arch_round_trips_through_strings() {
        assert_eq!("win64".parse::<Arch>().unwrap(), Arch::Win64);
        assert_eq!("win32".parse::<Arch>().unwrap(), Arch::Win32);
        assert!("x86".parse::<Arch>().is_err());
        assert_eq!(Arch::Win32.to_string(), "win32");
    }
}
